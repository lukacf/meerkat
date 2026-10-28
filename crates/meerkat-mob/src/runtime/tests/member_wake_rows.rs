#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! Member wake rows: a seated member that is not materialized in its process
//! (dormant) is woken by its first delivery, through MobMachine's wake
//! verdict and the cold restore recipe, and every refusal is typed.
//!
//! Every test runs real turn-driven members with real in-process comms over a
//! real factory, a real SQLite session store and a real head-canonical
//! SQLite runtime store. A member is made dormant inside the process with
//! `MobHandle::release_member`, so the rows do not depend on how a start
//! plans its members. The member's residency is read from the machine state
//! the actor publishes; a recording `MemberBuildPreparer` counts the builds
//! (one call per wake), and the model double records every request.

use super::*;
use crate::machines::mob_machine as mob_dsl;
use crate::runtime::{
    MemberBuildPreparation, MemberBuildPreparationRefusal, MemberBuildPrepared,
    MemberBuildPreparer, MemberBuildReport,
};

const WAIT: Duration = Duration::from_secs(30);
/// How long a test waits for a step the row says must happen soon (a stop
/// landing while a wake is held).
const STEP: Duration = Duration::from_secs(5);

type Service = meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>;

/// Records every model request and holds each reply until released.
#[derive(Clone)]
struct ScriptedClient {
    requests: Arc<Mutex<Vec<meerkat_client::LlmRequest>>>,
    released: tokio::sync::watch::Sender<bool>,
    started: tokio::sync::watch::Sender<usize>,
}

impl ScriptedClient {
    fn new() -> Self {
        let (released, _) = tokio::sync::watch::channel(true);
        let (started, _) = tokio::sync::watch::channel(0);
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            released,
            started,
        }
    }

    fn block(&self) {
        self.released.send_replace(false);
    }

    fn release(&self) {
        self.released.send_replace(true);
    }

    fn started_count(&self) -> usize {
        *self.started.borrow()
    }

    /// Wait until `count` model requests have started.
    async fn wait_started(&self, count: usize) {
        let mut started = self.started.subscribe();
        tokio::time::timeout(WAIT, started.wait_for(|seen| *seen >= count))
            .await
            .expect("the model request starts")
            .expect("started channel open");
    }

    /// The tools, in request order, of the first recorded model request
    /// that mentions `token`.
    fn tools_of_request_mentioning(&self, token: &str) -> Vec<String> {
        self.requests
            .lock()
            .expect("recorded requests")
            .iter()
            .find(|request| format!("{:?}", request.messages).contains(token))
            .map(|request| {
                request
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str().to_string())
                    .collect()
            })
            .expect("a model request mentions the token")
    }

    /// How many recorded model requests mention `token` anywhere.
    fn requests_mentioning(&self, token: &str) -> usize {
        self.requests
            .lock()
            .expect("recorded requests")
            .iter()
            .filter(|request| format!("{:?}", request.messages).contains(token))
            .count()
    }
}

#[async_trait]
impl meerkat_client::LlmClient for ScriptedClient {
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
        let sequence = {
            let mut requests = self.requests.lock().expect("record actual request");
            requests.push(request.clone());
            requests.len()
        };
        self.started.send_replace(sequence);
        let mut released = self.released.subscribe();
        Box::pin(async_stream::try_stream! {
            while !*released.borrow_and_update() {
                if released.changed().await.is_err() {
                    break;
                }
            }
            yield meerkat_client::LlmEvent::TextDelta {
                delta: format!("answer-{sequence}"),
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

/// What the recording preparer answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreparerAnswer {
    Prepared,
    Retryable,
    Final,
    /// Read the member's identity management through the handle Core hands
    /// the preparer, as a host that checks who manages an identity before it
    /// builds it: prepared once the read answers, retryable when it is
    /// refused.
    ReadsIdentityManagement,
}

/// One `prepare_member_build` call, as the preparer saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedBuild {
    identity: AgentIdentity,
    session_id: SessionId,
    wired_peers: BTreeSet<AgentIdentity>,
}

/// Records every build preparation and build report; can hold a build at
/// its preparation until the test opens the gate.
struct RecordingPreparer {
    answer: Mutex<PreparerAnswer>,
    prepared: Mutex<Vec<PreparedBuild>>,
    built: Mutex<Vec<MemberBuildReport>>,
    /// Each identity management read a preparation made, as it answered.
    management_reads: Mutex<Vec<Result<crate::MemberIdentityManagement, String>>>,
    /// Open (`true`) lets a preparation answer; closed holds it.
    gate: tokio::sync::watch::Sender<bool>,
    /// Preparations that reached the gate.
    entered: tokio::sync::watch::Sender<usize>,
}

impl RecordingPreparer {
    fn new() -> Self {
        let (gate, _) = tokio::sync::watch::channel(true);
        let (entered, _) = tokio::sync::watch::channel(0);
        Self {
            answer: Mutex::new(PreparerAnswer::Prepared),
            prepared: Mutex::new(Vec::new()),
            built: Mutex::new(Vec::new()),
            management_reads: Mutex::new(Vec::new()),
            gate,
            entered,
        }
    }

    fn answer_with(&self, answer: PreparerAnswer) {
        *self.answer.lock().expect("preparer answer") = answer;
    }

    fn prepared(&self) -> Vec<PreparedBuild> {
        self.prepared.lock().expect("preparations").clone()
    }

    fn prepared_count(&self) -> usize {
        self.prepared.lock().expect("preparations").len()
    }

    fn built(&self) -> Vec<MemberBuildReport> {
        self.built.lock().expect("build reports").clone()
    }

    fn management_reads(&self) -> Vec<Result<crate::MemberIdentityManagement, String>> {
        self.management_reads
            .lock()
            .expect("identity management reads")
            .clone()
    }

    fn close_gate(&self) {
        self.gate.send_replace(false);
    }

    fn open_gate(&self) {
        self.gate.send_replace(true);
    }

    async fn wait_entered(&self, count: usize) {
        let mut entered = self.entered.subscribe();
        tokio::time::timeout(WAIT, entered.wait_for(|seen| *seen >= count))
            .await
            .expect("the build preparation is asked")
            .expect("entered channel open");
    }

    /// Wait until `count` build reports arrived (they are delivered off the
    /// actor after the wake answered).
    async fn wait_built(&self, count: usize) {
        tokio::time::timeout(WAIT, async {
            while self.built.lock().expect("build reports").len() < count {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the build is reported");
    }
}

#[async_trait]
impl MemberBuildPreparer for RecordingPreparer {
    async fn prepare_member_build(
        &self,
        request: MemberBuildPreparation,
    ) -> Result<MemberBuildPrepared, MemberBuildPreparationRefusal> {
        self.prepared
            .lock()
            .expect("preparations")
            .push(PreparedBuild {
                identity: request.agent_identity.clone(),
                session_id: request.session_id.clone(),
                wired_peers: request.wired_peers.clone(),
            });
        self.entered.send_modify(|entered| *entered += 1);
        let mut gate = self.gate.subscribe();
        while !*gate.borrow_and_update() {
            if gate.changed().await.is_err() {
                break;
            }
        }
        let answer = *self.answer.lock().expect("preparer answer");
        match answer {
            PreparerAnswer::Prepared => Ok(MemberBuildPrepared::Prepared),
            PreparerAnswer::Retryable => Err(MemberBuildPreparationRefusal::Retryable {
                reason: "the host is not ready for this member yet".to_string(),
            }),
            PreparerAnswer::Final => Err(MemberBuildPreparationRefusal::Final {
                reason: "the host refuses to build this member".to_string(),
            }),
            PreparerAnswer::ReadsIdentityManagement => {
                let read = request
                    .handle
                    .identity_management(&request.agent_identity)
                    .await
                    .map_err(|error| error.to_string());
                self.management_reads
                    .lock()
                    .expect("identity management reads")
                    .push(read.clone());
                match read {
                    Ok(_) => Ok(MemberBuildPrepared::Prepared),
                    Err(error) => Err(MemberBuildPreparationRefusal::Retryable {
                        reason: format!(
                            "the member's identity management could not be read: {error}"
                        ),
                    }),
                }
            }
        }
    }

    async fn member_built(&self, report: MemberBuildReport) {
        self.built.lock().expect("build reports").push(report);
    }
}

/// A customizer whose resume answer is versioned: every resume records the
/// version it customized the member with. With overlay tools set, every
/// answer carries them as the member's external tools, each answer in
/// another order, as a host may return them.
struct VersionedCustomizer {
    version: AtomicU64,
    resumes: Mutex<Vec<(AgentIdentity, SessionId, u64)>>,
    overlay_tools: Mutex<Vec<&'static str>>,
    overlay_answers: AtomicU64,
    /// The auth binding every answer names, when set.
    auth_binding: Mutex<Option<meerkat_core::AuthBindingRef>>,
}

impl VersionedCustomizer {
    fn new() -> Self {
        Self {
            version: AtomicU64::new(1),
            resumes: Mutex::new(Vec::new()),
            overlay_tools: Mutex::new(Vec::new()),
            overlay_answers: AtomicU64::new(0),
            auth_binding: Mutex::new(None),
        }
    }

    fn set_auth_binding(&self, binding: meerkat_core::AuthBindingRef) {
        *self.auth_binding.lock().expect("auth binding") = Some(binding);
    }

    fn set_overlay_tools(&self, names: &[&'static str]) {
        *self.overlay_tools.lock().expect("overlay tools") = names.to_vec();
    }

    fn set_version(&self, version: u64) {
        self.version.store(version, Ordering::SeqCst);
    }

    fn resumes(&self) -> Vec<(AgentIdentity, SessionId, u64)> {
        self.resumes.lock().expect("resumes").clone()
    }
}

impl SpawnMemberCustomizer for VersionedCustomizer {
    fn customize_spawn(
        &self,
        _ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        if let Some(binding) = self.auth_binding.lock().expect("auth binding").clone() {
            spec.auth_binding = Some(binding);
        }
        let mut names = self.overlay_tools.lock().expect("overlay tools").clone();
        if !names.is_empty() {
            let answer = self.overlay_answers.fetch_add(1, Ordering::SeqCst);
            let shift = usize::try_from(answer).unwrap_or(0) % names.len();
            names.rotate_left(shift);
            spec.external_tools = Some(Arc::new(FixedOverlayTools(names)));
        }
        Ok(())
    }

    fn customize_resume(
        &self,
        ctx: &SpawnCustomizationContext,
        durable: &ResumedMemberView,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.resumes.lock().expect("resumes").push((
            spec.identity.clone(),
            durable.session_id.clone(),
            self.version.load(Ordering::SeqCst),
        ));
        self.customize_spawn(ctx, spec)
    }
}

fn wake_rows_definition(label: &str) -> MobDefinition {
    let mut definition = with_unique_mob_id(sample_definition(), label);
    let worker = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .and_then(ProfileBinding::as_inline_mut)
        .expect("inline worker");
    worker.model = "gpt-5.5".to_string();
    worker.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    worker.tools = ToolConfig {
        comms: true,
        ..ToolConfig::default()
    };
    definition
}

struct WakeFixture {
    _root: tempfile::TempDir,
    handle: MobHandle,
    service: Arc<Service>,
    client: ScriptedClient,
    preparer: Arc<RecordingPreparer>,
    customizer: Arc<VersionedCustomizer>,
}

impl WakeFixture {
    async fn new(label: &str) -> Self {
        Self::new_with_config(label, |_| {}).await
    }

    /// [`Self::new`], with `configure` applied to the factory's config.
    async fn new_with_config(label: &str, configure: impl FnOnce(&mut meerkat::Config)) -> Self {
        Self::new_with(label, configure, |_| {}).await
    }

    /// [`Self::new_with_config`], with `edit` applied to the mob's definition.
    async fn new_with(
        label: &str,
        configure: impl FnOnce(&mut meerkat::Config),
        edit: impl FnOnce(&mut MobDefinition),
    ) -> Self {
        let root = tempfile::Builder::new()
            .prefix("meerkat-member-wake-")
            .tempdir()
            .expect("fixture storage in the system temp dir");
        let root_path = root.path().canonicalize().expect("absolute fixture root");
        for directory in ["config", "runtime", "project", "context"] {
            std::fs::create_dir_all(root_path.join(directory)).expect("fixture root");
        }
        let factory = meerkat::AgentFactory::new(root_path.join("factory-store"))
            .user_config_root(root_path.join("config"))
            .runtime_root(root_path.join("runtime"))
            .project_root(root_path.join("project"))
            .context_root(root_path.join("context"))
            .builtins(false)
            .comms(true);
        let mut config = meerkat::Config::default();
        config.agent.model = "gpt-5.5".to_string();
        config.compaction.auto_compact_threshold = 1_000_000;
        configure(&mut config);
        let client = ScriptedClient::new();
        let mut builder = meerkat::FactoryAgentBuilder::new(factory, config);
        builder.default_llm_client = Some(Arc::new(client.clone()));
        let db = root_path.join("realm.db");
        let session_store =
            Arc::new(meerkat_store::SqliteSessionStore::open(&db).expect("session store"));
        builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
            session_store.clone(),
        )));
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
            meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&db)
                .expect("head-canonical runtime store"),
        );
        let service = Arc::new(Service::new(
            builder,
            16,
            session_store,
            runtime_store,
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ));
        let preparer = Arc::new(RecordingPreparer::new());
        let customizer = Arc::new(VersionedCustomizer::new());
        let mut definition = wake_rows_definition(label);
        edit(&mut definition);
        let handle = MobBuilder::new(definition, MobStorage::in_memory())
            .with_session_service(service.clone())
            .with_default_llm_client(Arc::new(client.clone()))
            .with_member_build_preparer(preparer.clone())
            .with_spawn_member_customizer(customizer.clone())
            .create()
            .await
            .expect("create the mob");
        Self {
            _root: root,
            handle,
            service,
            client,
            preparer,
            customizer,
        }
    }

    /// Spawn one durable turn-driven worker. Identities are unique per test:
    /// comms routes are process-wide.
    /// The member returned is Live and its registration's binding landed
    /// (MobMachine recorded the registration's epoch), so it can be released.
    async fn spawn(&self, label: &str) -> AgentIdentity {
        let identity = AgentIdentity::from(format!("{label}-{}", Uuid::new_v4().simple()));
        let identity = self.spawn_as(identity).await;
        self.wait_binding_landed(&identity).await;
        identity
    }

    /// Wait until MobMachine recorded the epoch of `identity`'s registration
    /// (the routed binding landed).
    async fn wait_binding_landed(&self, identity: &AgentIdentity) {
        let key = mob_dsl::AgentIdentity::from_domain(identity);
        let mut state = self.handle.machine_state_watch_rx.clone();
        let landed = tokio::time::timeout(
            WAIT,
            state.wait_for(|state| {
                state
                    .member_materialization_epoch
                    .get(&key)
                    .is_some_and(Option::is_some)
            }),
        )
        .await;
        assert!(
            matches!(landed, Ok(Ok(_))),
            "the binding of member {identity}'s registration never landed"
        );
    }

    /// Spawn `identity` and wait until it is Live (its binding may still be
    /// in flight).
    async fn spawn_as(&self, identity: AgentIdentity) -> AgentIdentity {
        let mut spec = SpawnMemberSpec::new("worker", identity.clone());
        spec.continuity_intent = SpawnContinuityIntent::DurableIdentity {
            continuity_key: identity.to_string(),
        };
        tokio::time::timeout(WAIT, self.handle.spawn_spec(spec))
            .await
            .expect("the spawn answers")
            .expect("spawn the worker");
        self.wait_residency(&identity, Some(mob_dsl::MemberMaterializationPhase::Live))
            .await;
        identity
    }

    async fn wire(&self, a: &AgentIdentity, b: &AgentIdentity) {
        tokio::time::timeout(WAIT, self.handle.wire(a.clone(), b.clone()))
            .await
            .expect("the wire answers")
            .expect("wire the members");
    }

    fn residency(&self, identity: &AgentIdentity) -> Option<mob_dsl::MemberMaterializationPhase> {
        self.handle
            .machine_state_watch_rx
            .borrow()
            .member_materialization
            .get(&mob_dsl::AgentIdentity::from_domain(identity))
            .copied()
    }

    async fn wait_residency(
        &self,
        identity: &AgentIdentity,
        expected: Option<mob_dsl::MemberMaterializationPhase>,
    ) {
        let key = mob_dsl::AgentIdentity::from_domain(identity);
        let mut state = self.handle.machine_state_watch_rx.clone();
        let reached = tokio::time::timeout(
            WAIT,
            state.wait_for(|state| state.member_materialization.get(&key).copied() == expected),
        )
        .await;
        assert!(
            matches!(reached, Ok(Ok(_))),
            "member {identity} never reached residency {expected:?}; it is {:?}",
            self.residency(identity)
        );
    }

    async fn session(&self, identity: &AgentIdentity) -> SessionId {
        self.handle
            .resolve_bridge_session_id(identity)
            .await
            .expect("a session-bound member")
    }

    async fn entry(&self, identity: &AgentIdentity) -> RosterEntry {
        self.handle
            .get_member(identity)
            .await
            .expect("roster read")
            .expect("a seated member")
    }

    /// Release a quiescent member and wait until MobMachine holds it
    /// Dormant (the runtime's unregister reaches the mob through the
    /// composition release).
    async fn release(&self, identity: &AgentIdentity) {
        tokio::time::timeout(WAIT, self.handle.release_member(identity))
            .await
            .expect("the release answers")
            .expect("release a quiescent member");
        self.wait_residency(identity, Some(mob_dsl::MemberMaterializationPhase::Dormant))
            .await;
        let session = self.session(identity).await;
        assert!(
            !MobSessionService::live_session_actor_registered(&*self.service, &session)
                .await
                .unwrap_or(false),
            "a released member keeps no live session actor"
        );
    }

    async fn ensure_live(&self, identity: &AgentIdentity) -> Result<(), MobError> {
        tokio::time::timeout(WAIT, self.handle.ensure_member_live(identity))
            .await
            .expect("the wake answers")
    }

    async fn comms(&self, identity: &AgentIdentity) -> Arc<dyn CoreCommsRuntime> {
        let session = self.session(identity).await;
        self.service
            .comms_runtime(&session)
            .await
            .expect("a live member's comms runtime")
    }

    fn comms_name(&self, identity: &AgentIdentity) -> String {
        super::actor::render_member_comms_name(
            self.handle.mob_id().as_str(),
            "worker",
            identity.as_str(),
        )
        .expect("member comms name")
    }

    /// A raw peer message from `from`'s own comms runtime to `to`, outside
    /// the mob's peer-message API (which refuses a broken or stopped
    /// recipient itself before any send).
    async fn raw_peer_send(
        &self,
        from: &AgentIdentity,
        to: &AgentIdentity,
        body: &str,
    ) -> Result<SendReceipt, SendError> {
        let comms = self.comms(from).await;
        let route = test_peer_route(&*comms, &self.comms_name(to)).await;
        tokio::time::timeout(
            WAIT,
            CoreCommsRuntime::send(
                &*comms,
                CommsCommand::PeerMessage {
                    objective_id: None,
                    content_taint: None,
                    to: route,
                    body: body.to_string(),
                    blocks: None,
                    handling_mode: HandlingMode::Queue,
                    in_reply_to: None,
                },
            ),
        )
        .await
        .expect("the send answers")
    }

    async fn transcript(&self, session: &SessionId) -> String {
        let page = SessionServiceHistoryExt::read_history(
            self.service.as_ref(),
            session,
            meerkat_core::service::SessionHistoryQuery {
                offset: 0,
                limit: None,
            },
        )
        .await
        .expect("read the transcript");
        serde_json::to_string(&page.messages).expect("transcript json")
    }

    /// Wait until `identity`'s durable transcript holds `token`, then
    /// answer how many times it holds it.
    async fn delivered_count(&self, identity: &AgentIdentity, token: &str) -> usize {
        let session = self.session(identity).await;
        tokio::time::timeout(WAIT, async {
            loop {
                if self.transcript(&session).await.contains(token) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the message reaches the member's transcript");
        self.transcript(&session).await.matches(token).count()
    }

    /// Run one host turn on `identity` with `handling_mode` and wait for it.
    async fn host_turn(&self, identity: &AgentIdentity, text: &str, handling_mode: HandlingMode) {
        let entry = self.entry(identity).await;
        let turn = tokio::time::timeout(
            WAIT,
            self.handle.start_work_with_mode(
                entry.agent_runtime_id,
                entry.fence_token,
                WorkRef::new(),
                WorkSpec::new(text, WorkOrigin::Internal),
                handling_mode,
            ),
        )
        .await
        .expect("the delivery is admitted in time")
        .expect("the delivery is admitted");
        tokio::time::timeout(WAIT, turn.wait())
            .await
            .expect("the turn finishes")
            .expect("the turn succeeds");
    }

    async fn finish(self) {
        self.client.release();
        self.preparer.open_gate();
        let _ = tokio::time::timeout(WAIT, self.handle.shutdown()).await;
    }
}

fn token(label: &str) -> String {
    format!("{label}-token-{}", Uuid::new_v4().simple())
}

/// A peer message to a released member wakes it once and is delivered once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_message_wakes_a_released_member_once_and_is_delivered_once() {
    let fixture = WakeFixture::new("wake-r1").await;
    let sender = fixture.spawn("r1-sender").await;
    let recipient = fixture.spawn("r1-recipient").await;
    fixture.wire(&sender, &recipient).await;
    fixture.release(&recipient).await;

    let body = token("r1");
    let receipt = tokio::time::timeout(
        WAIT,
        fixture.handle.send_peer_message(
            sender.clone(),
            recipient.clone(),
            body.as_str(),
            HandlingMode::Queue,
        ),
    )
    .await
    .expect("the send answers")
    .expect("the dormant recipient is woken and the message handed off");
    assert_eq!(receipt.to, recipient);

    assert_eq!(fixture.delivered_count(&recipient, &body).await, 1);
    fixture
        .wait_residency(&recipient, Some(mob_dsl::MemberMaterializationPhase::Live))
        .await;
    assert_eq!(
        fixture.preparer.prepared_count(),
        1,
        "one wake builds the member once"
    );
    fixture.finish().await;
}

/// Two peers messaging one released member share one wake.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_peers_messaging_one_released_member_share_one_wake() {
    let fixture = WakeFixture::new("wake-r2").await;
    let first = fixture.spawn("r2-first").await;
    let second = fixture.spawn("r2-second").await;
    let recipient = fixture.spawn("r2-recipient").await;
    fixture.wire(&first, &recipient).await;
    fixture.wire(&second, &recipient).await;
    fixture.release(&recipient).await;

    let first_body = token("r2-first");
    let second_body = token("r2-second");
    let (first_sent, second_sent) = tokio::join!(
        fixture.handle.send_peer_message(
            first.clone(),
            recipient.clone(),
            first_body.as_str(),
            HandlingMode::Queue,
        ),
        fixture.handle.send_peer_message(
            second.clone(),
            recipient.clone(),
            second_body.as_str(),
            HandlingMode::Queue,
        ),
    );
    first_sent.expect("the first sender gets its receipt");
    second_sent.expect("the second sender gets its receipt");

    assert_eq!(fixture.delivered_count(&recipient, &first_body).await, 1);
    assert_eq!(fixture.delivered_count(&recipient, &second_body).await, 1);
    assert_eq!(
        fixture.preparer.prepared_count(),
        1,
        "concurrent senders join one wake"
    );
    fixture.finish().await;
}

/// A final build preparation refusal breaks the member, and a broken member
/// answers peers typed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_final_preparation_refusal_breaks_the_member_and_peers_get_member_broken() {
    let fixture = WakeFixture::new("wake-r4").await;
    let sender = fixture.spawn("r4-sender").await;
    let recipient = fixture.spawn("r4-recipient").await;
    fixture.wire(&sender, &recipient).await;
    fixture.release(&recipient).await;

    fixture.preparer.answer_with(PreparerAnswer::Final);
    let refused = fixture.ensure_live(&recipient).await;
    assert!(
        matches!(refused, Err(MobError::MemberRestoreFailed { ref member_id, .. }) if member_id == &recipient),
        "a final refusal records the member broken, typed: {refused:?}"
    );
    assert_eq!(
        fixture.residency(&recipient),
        None,
        "a broken member holds no residency phase"
    );
    assert!(
        fixture
            .handle
            .get_member(&recipient)
            .await
            .expect("roster read")
            .is_some(),
        "a broken member stays seated"
    );

    let sent = fixture
        .raw_peer_send(&sender, &recipient, &token("r4"))
        .await;
    assert!(
        matches!(
            sent,
            Err(SendError::PeerWakeRefused {
                refusal: meerkat_core::comms::PeerWakeRefusal::MemberBroken { .. },
                ..
            })
        ),
        "a peer message to a broken member is refused member_broken, never PeerNotFound: {sent:?}"
    );
    fixture.finish().await;
}

/// Reads of a released member never wake it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reads_of_a_released_member_never_wake_it() {
    let fixture = WakeFixture::new("wake-r26").await;
    let member = fixture.spawn("r26-member").await;
    fixture.release(&member).await;

    let _ = fixture.handle.list_members().await;
    let _ = fixture.handle.roster().await;
    let _ = fixture.entry(&member).await;
    let _ = fixture.handle.member_status(&member).await;
    let _ = fixture.session(&member).await;

    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Dormant),
        "reads leave the member dormant"
    );
    assert_eq!(fixture.preparer.prepared_count(), 0, "no read builds it");
    fixture.finish().await;
}

/// `MobHandle::member_unmaterialized` reads the member's residency from the
/// machine state the actor publishes, with no command: a released member
/// reads unmaterialized. A subscription to its agent events opens meanwhile
/// with nothing to attach to yet (no actor, no epoch); neither the read nor
/// the subscription wakes it. While its wake is held at the build
/// preparer's gate the member is Waking, still reads unmaterialized, and the
/// subscription has delivered nothing. The end of the wake changes the
/// machine state, after which the member reads materialized and the
/// subscription attaches to the woken actor from its first retained event,
/// so the turn it then runs is read from its `RunStarted`, and the
/// subscription names the woken actor's sequence space. A subscription made
/// after the wake replays the same turn. An identity the mob does not seat
/// reads materialized: there is no wake to wait for.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_released_member_reads_unmaterialized_until_its_wake_and_a_subscription_attaches_at_it() {
    let fixture = WakeFixture::new("wake-unmaterialized").await;
    let member = fixture.spawn("unmaterialized-member").await;
    assert!(
        !fixture.handle.member_unmaterialized(&member),
        "a live member"
    );
    fixture.release(&member).await;
    assert!(
        fixture.handle.member_unmaterialized(&member),
        "a released member"
    );
    let early = fixture
        .handle
        .subscribe_agent_events_from(&member, meerkat_core::comms::SessionEventCursor::Earliest)
        .await
        .expect("a released member's agent events subscribe");
    assert!(
        early.actor.is_none() && early.epoch.is_none(),
        "nothing is attached before the wake"
    );
    let attached_epoch = early
        .attached_epoch
        .clone()
        .expect("the subscription names the woken actor's epoch once attached");
    assert!(attached_epoch.get().is_none(), "not attached yet");
    let mut early = early.stream;
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Dormant),
        "neither the read nor the subscription woke the member"
    );
    assert!(
        !fixture
            .handle
            .member_unmaterialized(&AgentIdentity::from("never-seated")),
        "an identity the mob does not seat"
    );

    let mut changes = fixture.handle.machine_state_changes();
    fixture.preparer.close_gate();
    let wake = tokio::spawn({
        let handle = fixture.handle.clone();
        let member = member.clone();
        async move { handle.ensure_member_live(&member).await }
    });
    fixture.preparer.wait_entered(1).await;
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Waking))
        .await;
    assert!(
        fixture.handle.member_unmaterialized(&member),
        "a waking member"
    );
    assert!(
        futures::FutureExt::now_or_never(futures::StreamExt::next(&mut early)).is_none(),
        "nothing is delivered before the wake ends"
    );
    fixture.preparer.open_gate();
    tokio::time::timeout(WAIT, wake)
        .await
        .expect("the wake answers")
        .expect("the wake task")
        .expect("the wake ends live");
    tokio::time::timeout(WAIT, changes.changed())
        .await
        .expect("the wake changes the machine state")
        .expect("the mob runs");
    assert!(
        !fixture.handle.member_unmaterialized(&member),
        "the woken member"
    );
    let body = token("unmaterialized");
    fixture.host_turn(&member, &body, HandlingMode::Queue).await;
    tokio::time::timeout(WAIT, async {
        loop {
            let event = futures::StreamExt::next(&mut early)
                .await
                .expect("the early subscription attaches at the wake");
            if matches!(event.payload, AgentEvent::RunStarted { .. }) {
                break;
            }
        }
    })
    .await
    .expect("the early subscription carries the turn from its start");
    assert!(
        matches!(attached_epoch.get(), Some(Some(_))),
        "the attached subscription names the woken actor's epoch"
    );
    let mut events = fixture
        .handle
        .subscribe_agent_events_from(&member, meerkat_core::comms::SessionEventCursor::Earliest)
        .await
        .expect("the woken member's agent events subscribe")
        .stream;
    tokio::time::timeout(WAIT, async {
        loop {
            let event = futures::StreamExt::next(&mut events)
                .await
                .expect("the replay reaches the turn's start");
            if matches!(event.payload, AgentEvent::RunStarted { .. }) {
                break;
            }
        }
    })
    .await
    .expect("the replay holds the turn the woken member ran");
    fixture.finish().await;
}

/// A host that subscribes to a released member's agent events and then
/// sends to it receives the run its send starts, from `RunStarted`: the
/// subscription opens without waking the member and attaches at the wake
/// the send causes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subscription_before_a_released_members_wake_receives_the_run_its_send_starts() {
    let fixture = WakeFixture::new("wake-subscribe-first").await;
    let member = fixture.spawn("subscribe-first-member").await;
    fixture.release(&member).await;

    let mut events = fixture
        .handle
        .subscribe_agent_events(&member)
        .await
        .expect("a released member's agent events subscribe");
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Dormant),
        "the subscription wakes nothing"
    );

    let body = token("subscribe-first");
    fixture.host_turn(&member, &body, HandlingMode::Queue).await;
    tokio::time::timeout(WAIT, async {
        loop {
            let event = futures::StreamExt::next(&mut events)
                .await
                .expect("the subscription attaches at the wake");
            if matches!(event.payload, AgentEvent::RunStarted { .. }) {
                break;
            }
        }
    })
    .await
    .expect("the run the send started arrives from its start");
    fixture.finish().await;
}

/// A subscription to every member's agent events opens while members are
/// dormant, without waking any of them, and carries a member's events from
/// its wake; the members it does not wake stay dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_all_members_subscription_carries_a_dormant_member_from_its_wake() {
    let fixture = WakeFixture::new("wake-subscribe-all").await;
    let woken = fixture.spawn("subscribe-all-woken").await;
    let dormant = fixture.spawn("subscribe-all-dormant").await;
    fixture.release(&woken).await;
    fixture.release(&dormant).await;

    let streams = fixture
        .handle
        .subscribe_all_agent_events()
        .await
        .expect("the subscription opens with dormant members");
    assert!(
        streams.iter().any(|(identity, _)| identity == &dormant),
        "the dormant member has its stream"
    );
    let mut events = streams
        .into_iter()
        .find_map(|(identity, stream)| (identity == woken).then_some(stream))
        .expect("the member to wake has its stream");
    for identity in [&woken, &dormant] {
        assert_eq!(
            fixture.residency(identity),
            Some(mob_dsl::MemberMaterializationPhase::Dormant),
            "the subscription wakes nothing"
        );
    }

    let body = token("subscribe-all");
    fixture.host_turn(&woken, &body, HandlingMode::Queue).await;
    tokio::time::timeout(WAIT, async {
        loop {
            let event = futures::StreamExt::next(&mut events)
                .await
                .expect("the member's stream attaches at its wake");
            if matches!(event.payload, AgentEvent::RunStarted { .. }) {
                break;
            }
        }
    })
    .await
    .expect("the woken member's run arrives from its start");
    assert_eq!(
        fixture.residency(&dormant),
        Some(mob_dsl::MemberMaterializationPhase::Dormant),
        "the member nothing woke stays dormant"
    );
    fixture.finish().await;
}

/// A steer to a released member wakes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_to_a_released_member_wakes_it() {
    let fixture = WakeFixture::new("wake-r30").await;
    let member = fixture.spawn("r30-member").await;
    fixture.release(&member).await;

    let body = token("r30");
    fixture.host_turn(&member, &body, HandlingMode::Steer).await;

    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live)
    );
    assert_eq!(fixture.preparer.prepared_count(), 1);
    assert!(
        fixture.client.requests_mentioning(&body) >= 1,
        "the steered content reaches the woken member's model"
    );
    fixture.finish().await;
}

/// A host turn to a released member wakes it and runs once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_turn_to_a_released_member_wakes_it_and_runs_once() {
    let fixture = WakeFixture::new("wake-r50").await;
    let member = fixture.spawn("r50-member").await;
    fixture.release(&member).await;

    let body = token("r50");
    fixture.host_turn(&member, &body, HandlingMode::Queue).await;

    assert_eq!(fixture.delivered_count(&member, &body).await, 1);
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live)
    );
    assert_eq!(fixture.preparer.prepared_count(), 1);
    fixture.finish().await;
}

/// A member whose runtime has no comms (its session builder makes none) has
/// no peer endpoint and owes none, and needs no comms drain as an unwired
/// turn-driven member: once released, a host turn wakes it like any member
/// and it serves the turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_comms_less_member_wakes_and_serves() {
    let handle = create_test_mob_with_persistent_service(with_unique_mob_id(
        sample_definition(),
        "wake-comms-less",
    ))
    .await;
    let member = AgentIdentity::from(format!("comms-less-{}", Uuid::new_v4().simple()));
    let key = mob_dsl::AgentIdentity::from_domain(&member);
    let wait_residency = |expected: mob_dsl::MemberMaterializationPhase| {
        let mut state = handle.machine_state_watch_rx.clone();
        let key = key.clone();
        async move {
            tokio::time::timeout(
                WAIT,
                state.wait_for(|state| state.member_materialization.get(&key) == Some(&expected)),
            )
            .await
            .expect("the member reaches the residency in time")
            .expect("the machine state stays published");
        }
    };
    tokio::time::timeout(
        WAIT,
        handle.spawn_with_options(
            ProfileName::from("worker"),
            member.clone(),
            None,
            Some(crate::MobRuntimeMode::TurnDriven),
            None,
        ),
    )
    .await
    .expect("the spawn answers")
    .expect("spawn a member whose runtime has no comms");
    wait_residency(mob_dsl::MemberMaterializationPhase::Live).await;
    assert!(
        !handle
            .machine_state_watch_rx
            .borrow()
            .member_peer_endpoints
            .contains_key(&key),
        "a member whose runtime has no comms has no peer endpoint"
    );
    // The release needs the registration's binding to have landed.
    let mut state = handle.machine_state_watch_rx.clone();
    tokio::time::timeout(
        WAIT,
        state.wait_for(|state| {
            state
                .member_materialization_epoch
                .get(&key)
                .is_some_and(Option::is_some)
        }),
    )
    .await
    .expect("the member's binding lands in time")
    .expect("the machine state stays published");

    tokio::time::timeout(WAIT, handle.release_member(&member))
        .await
        .expect("the release answers")
        .expect("release the quiescent member");
    wait_residency(mob_dsl::MemberMaterializationPhase::Dormant).await;

    let entry = handle
        .get_member(&member)
        .await
        .expect("roster read")
        .expect("a seated member");
    let turn = tokio::time::timeout(
        WAIT,
        handle.start_work_with_mode(
            entry.agent_runtime_id,
            entry.fence_token,
            WorkRef::new(),
            WorkSpec::new("serve this turn", WorkOrigin::Internal),
            HandlingMode::Queue,
        ),
    )
    .await
    .expect("the delivery is admitted in time")
    .expect("the delivery wakes the member and is admitted");
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the turn finishes")
        .expect("the woken member serves the turn");
    assert_eq!(
        handle
            .machine_state_watch_rx
            .borrow()
            .member_materialization
            .get(&key),
        Some(&mob_dsl::MemberMaterializationPhase::Live)
    );
    let _ = tokio::time::timeout(WAIT, handle.shutdown()).await;
}

/// An operator wake, and the attempt-keyed waiters: concurrent callers share
/// one wake.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ensure_member_live_wakes_a_released_member_once_for_concurrent_callers() {
    let fixture = WakeFixture::new("wake-ensure").await;
    let member = fixture.spawn("ensure-member").await;
    fixture.release(&member).await;

    let (first, second) = tokio::join!(fixture.ensure_live(&member), fixture.ensure_live(&member));
    first.expect("the first caller sees the member ready");
    second.expect("the second caller joins the same wake");
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live)
    );
    assert_eq!(fixture.preparer.prepared_count(), 1, "one wake, one build");
    fixture.finish().await;
}

/// A release keeps the member seated, and its wake restores the same
/// incarnation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn release_keeps_the_seat_and_the_wake_restores_the_same_incarnation() {
    let fixture = WakeFixture::new("wake-r65").await;
    let member = fixture.spawn("r65-member").await;
    let before = fixture.entry(&member).await;
    let session_before = fixture.session(&member).await;

    fixture.release(&member).await;
    let seated = fixture.entry(&member).await;
    assert_eq!(
        seated.generation, before.generation,
        "the seat keeps its generation"
    );
    assert_eq!(
        seated.fence_token, before.fence_token,
        "the seat keeps its fence"
    );

    fixture
        .ensure_live(&member)
        .await
        .expect("the released member wakes");
    let after = fixture.entry(&member).await;
    assert_eq!(after.generation, before.generation);
    assert_eq!(after.fence_token, before.fence_token);
    assert_eq!(after.agent_runtime_id, before.agent_runtime_id);
    assert_eq!(
        fixture.session(&member).await,
        session_before,
        "the wake restores the same session"
    );
    fixture.finish().await;
}

/// A release has nothing to release on a dormant member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_of_a_dormant_member_changes_nothing() {
    let fixture = WakeFixture::new("wake-release-twice").await;
    let member = fixture.spawn("release-twice").await;
    fixture.release(&member).await;

    tokio::time::timeout(WAIT, fixture.handle.release_member(&member))
        .await
        .expect("the release answers")
        .expect("releasing a dormant member is a no-op");
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Dormant)
    );
    assert_eq!(fixture.preparer.prepared_count(), 0);
    fixture.finish().await;
}

/// A release never interrupts work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_with_a_run_in_flight_is_refused_retryably() {
    let fixture = WakeFixture::new("wake-release-busy").await;
    let member = fixture.spawn("release-busy").await;
    fixture.client.block();
    let started_before = fixture.client.started_count();
    let entry = fixture.entry(&member).await;
    let turn = fixture
        .handle
        .start_work_with_mode(
            entry.agent_runtime_id,
            entry.fence_token,
            WorkRef::new(),
            WorkSpec::new(token("busy"), WorkOrigin::Internal),
            HandlingMode::Queue,
        )
        .await
        .expect("the turn is admitted");
    fixture.client.wait_started(started_before + 1).await;

    let refused = tokio::time::timeout(WAIT, fixture.handle.release_member(&member))
        .await
        .expect("the release answers");
    assert!(
        matches!(refused, Err(MobError::LifecycleOperationPending { .. })),
        "a release with work in flight is refused retryably: {refused:?}"
    );

    fixture.client.release();
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the turn finishes")
        .expect("the turn succeeds");
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live),
        "the refused release changed nothing"
    );
    fixture.finish().await;
}

/// A stopped mob refuses peer messages to dormant and live members alike.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stopped_mob_refuses_peer_messages_to_dormant_and_live_members_alike() {
    let fixture = WakeFixture::new("wake-r98").await;
    let sender = fixture.spawn("r98-sender").await;
    let live = fixture.spawn("r98-live").await;
    let dormant = fixture.spawn("r98-dormant").await;
    fixture.wire(&sender, &live).await;
    fixture.wire(&sender, &dormant).await;
    fixture.release(&dormant).await;

    tokio::time::timeout(WAIT, fixture.handle.stop())
        .await
        .expect("the stop answers")
        .expect("stop the mob");

    for recipient in [&dormant, &live] {
        let sent = fixture
            .raw_peer_send(&sender, recipient, &token("r98-stopped"))
            .await;
        assert!(
            matches!(
                sent,
                Err(SendError::PeerWakeRefused {
                    refusal: meerkat_core::comms::PeerWakeRefusal::MobNotRunning,
                    ..
                })
            ),
            "a stopped mob refuses {recipient} mob_not_running: {sent:?}"
        );
    }

    tokio::time::timeout(WAIT, fixture.handle.resume())
        .await
        .expect("the resume answers")
        .expect("resume the mob");
    for recipient in [&dormant, &live] {
        let body = token("r98-resumed");
        fixture
            .raw_peer_send(&sender, recipient, &body)
            .await
            .expect("after the resume the message is delivered");
        assert_eq!(fixture.delivered_count(recipient, &body).await, 1);
    }
    fixture.finish().await;
}

/// A wake asks the host to prepare the member build once and reports it
/// built.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_prepares_the_member_build_once_and_reports_it_built() {
    let fixture = WakeFixture::new("wake-r109").await;
    let peer = fixture.spawn("r109-peer").await;
    let member = fixture.spawn("r109-member").await;
    fixture.wire(&peer, &member).await;
    assert_eq!(
        fixture.preparer.prepared_count(),
        0,
        "a spawn is no wake: the preparer is asked for wakes and restores"
    );
    // A spawn is a build all the same: each is reported once its activation
    // committed, with the tools its member sees.
    fixture.preparer.wait_built(2).await;
    let spawned = fixture.preparer.built();
    assert_eq!(spawned.len(), 2, "one build report per spawn: {spawned:?}");
    for identity in [&peer, &member] {
        assert!(
            spawned.iter().any(
                |report| &report.agent_identity == identity && !report.visible_tools.is_empty()
            ),
            "the spawn of {identity} is reported with its visible tools: {spawned:?}"
        );
    }
    fixture.release(&member).await;
    let session = fixture.session(&member).await;

    fixture
        .ensure_live(&member)
        .await
        .expect("the released member wakes");

    let prepared = fixture.preparer.prepared();
    assert_eq!(
        prepared,
        vec![PreparedBuild {
            identity: member.clone(),
            session_id: session.clone(),
            wired_peers: BTreeSet::from([peer.clone()]),
        }],
        "one preparation, with the member, its session and its wired peers"
    );
    fixture.preparer.wait_built(3).await;
    let built = fixture.preparer.built();
    assert_eq!(built.len(), 3, "one build report per build");
    assert_eq!(built[2].agent_identity, member);
    assert_eq!(built[2].session_id, session);
    fixture.finish().await;
}

/// A retryable build preparation refusal leaves the member dormant until
/// its next delivery.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retryable_preparation_refusal_leaves_the_member_dormant_until_the_next_delivery() {
    let fixture = WakeFixture::new("wake-r112").await;
    let member = fixture.spawn("r112-member").await;
    fixture.release(&member).await;

    fixture.preparer.answer_with(PreparerAnswer::Retryable);
    let refused = fixture.ensure_live(&member).await;
    assert!(
        matches!(
            refused,
            Err(MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::HostPreparationPending,
                ..
            })
        ),
        "a retryable refusal is the typed retryable wake refusal: {refused:?}"
    );
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Dormant))
        .await;

    fixture.preparer.answer_with(PreparerAnswer::Prepared);
    fixture
        .ensure_live(&member)
        .await
        .expect("the next delivery wakes the member");
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live)
    );
    assert_eq!(fixture.preparer.prepared_count(), 2);
    fixture.finish().await;
}

/// The host's build preparer reads through the mob's own read-only view on
/// a wake, as on a boot restore: the authority the mob's launch minted (the
/// owner's), never the agent lane of the member tools the wake builds. A
/// preparer that reads who manages the member's identity before it builds
/// the member is answered, so the member's on-demand wake goes ahead. On the
/// agent lane that read is refused (it needs the List scope), the preparer
/// answers retryable, and every wake of the member is refused
/// `HostPreparationPending` again: the member never wakes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_hands_the_build_preparer_the_mobs_own_read_view() {
    let fixture = WakeFixture::new("wake-preparer-read-view").await;
    let member = fixture.spawn("preparer-read-view").await;
    fixture.release(&member).await;
    fixture
        .preparer
        .answer_with(PreparerAnswer::ReadsIdentityManagement);

    let woken = fixture.ensure_live(&member).await;
    let reads = fixture.preparer.management_reads();
    assert!(
        woken.is_ok(),
        "the wake goes ahead once its preparer could read the member's identity \
         management: {woken:?}, reads {reads:?}"
    );
    assert_eq!(
        reads,
        vec![Ok(crate::MemberIdentityManagement::CommandDriven)],
        "one preparation, whose read of the member's identity management was answered"
    );
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live)
    );
    // The member tools' lane is unchanged: on the agent lane the same read
    // is still refused for want of the List scope.
    let agent_lane_read = fixture
        .handle
        .clone()
        .with_command_authority(crate::control_policy::CommandAuthority::agent_lane())
        .read_handle()
        .identity_management(&member)
        .await;
    assert!(
        matches!(
            &agent_lane_read,
            Err(MobError::ScopeDenied(denial))
                if denial.required == crate::ControlScope::List && denial.presented.is_empty()
        ),
        "the member tools' lane still cannot read the member's identity management: \
         {agent_lane_read:?}"
    );
    fixture.finish().await;
}

/// A stop during a wake abandons the wake without a fail-stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_during_a_wake_abandons_the_wake_without_fail_stop() {
    let fixture = WakeFixture::new("wake-r113").await;
    let member = fixture.spawn("r113-member").await;
    fixture.release(&member).await;

    fixture.preparer.close_gate();
    let handle = fixture.handle.clone();
    let woken = member.clone();
    let wake = tokio::spawn(async move { handle.ensure_member_live(&woken).await });
    fixture.preparer.wait_entered(1).await;
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Waking))
        .await;

    let handle = fixture.handle.clone();
    let stop = tokio::spawn(async move { handle.stop().await });
    let stopped = tokio::time::timeout(STEP, async {
        loop {
            if matches!(fixture.handle.status().await, Ok(MobState::Stopped)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    fixture.preparer.open_gate();
    assert!(
        stopped.is_ok(),
        "the stop lands while the wake is held, not after it"
    );

    let answer = tokio::time::timeout(WAIT, wake)
        .await
        .expect("the wake answers")
        .expect("the wake task joins");
    assert!(
        matches!(
            answer,
            Err(MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::MobNotRunning,
                ..
            })
        ),
        "the abandoned wake answers mob_not_running: {answer:?}"
    );
    tokio::time::timeout(WAIT, stop)
        .await
        .expect("the stop answers")
        .expect("the stop task joins")
        .expect("the stop succeeds: no fail-stop");
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Dormant),
        "nothing was materialized"
    );

    tokio::time::timeout(WAIT, fixture.handle.resume())
        .await
        .expect("the resume answers")
        .expect("the mob resumes normally");
    fixture
        .ensure_live(&member)
        .await
        .expect("the next delivery wakes the member");
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live)
    );
    fixture.finish().await;
}

/// A destroy during a wake waits for the wake's in-flight step: it tears
/// down the storage and runtime that step works on, so it does not preempt
/// it as a stop does. Once the held wake finishes, the destroy runs and
/// leaves no session behind: no live session actor, and no resumable
/// durable session.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_destroy_during_a_wake_waits_for_it_and_leaves_no_session_behind() {
    let fixture = WakeFixture::new("wake-destroy").await;
    let member = fixture.spawn("destroy-member").await;
    let session = fixture.session(&member).await;
    fixture.release(&member).await;

    fixture.preparer.close_gate();
    let entered = fixture.preparer.prepared_count();
    let wake = {
        let handle = fixture.handle.clone();
        let woken = member.clone();
        tokio::spawn(async move { handle.ensure_member_live(&woken).await })
    };
    fixture.preparer.wait_entered(entered + 1).await;
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Waking))
        .await;

    let mut destroy = fixture
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::Destroy { reply_tx })
        .await
        .expect("enqueue the destroy");
    // The actor reads its commands in order: a status read sent after the
    // destroy is answered Running only if the destroy was deferred (an
    // admitted destroy runs first and leaves the mob Destroyed).
    let status = tokio::time::timeout(STEP, fixture.handle.status())
        .await
        .expect("the status answers while the destroy waits")
        .expect("read the mob's status");
    assert_eq!(
        status,
        MobState::Running,
        "the destroy waits for the held wake"
    );
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Waking),
        "the wake's attempt is still open"
    );
    assert!(
        matches!(
            destroy.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "the destroy has not answered while the wake is held"
    );

    fixture.preparer.open_gate();
    tokio::time::timeout(WAIT, wake)
        .await
        .expect("the wake answers")
        .expect("the wake task joins")
        .expect("the wake the destroy waited for completes");
    tokio::time::timeout(WAIT, destroy)
        .await
        .expect("the destroy answers once the wake settled")
        .expect("the destroy's reply")
        .expect("the mob is destroyed");
    assert!(
        !MobSessionService::live_session_actor_registered(&*fixture.service, &session)
            .await
            .unwrap_or(false),
        "the destroy leaves no live session actor"
    );
    let authority =
        MobSessionService::observe_session_resume_authority(&*fixture.service, &session)
            .await
            .expect("read the session's durable authority");
    let lifecycle = authority.lifecycle();
    assert!(
        !matches!(
            lifecycle,
            crate::runtime::session_service::SessionResumeLifecycle::Active { .. }
        ),
        "the destroy leaves no resumable session: {lifecycle:?}"
    );
    fixture.finish().await;
}

/// A wake that starts while a stop left an earlier wake's step in flight
/// waits for that step: one effect of a member at a time. The earlier step
/// returns after the resume, fails its attempt fence and rolls back; only
/// then does the new wake restore the member, which ends Live on that one
/// restore (one build report after the spawn's own) with its session actor
/// kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_after_a_stop_and_resume_waits_for_the_preempted_wakes_step() {
    let fixture = WakeFixture::new("wake-after-preempted-step").await;
    let member = fixture.spawn("preempted-step-member").await;
    // The spawn is a build of its own, reported off the actor: wait for its
    // report so the wakes' reports are counted from it.
    fixture.preparer.wait_built(1).await;
    let session = fixture.session(&member).await;
    fixture.release(&member).await;

    fixture.preparer.close_gate();
    let entered = fixture.preparer.prepared_count();
    let first = {
        let handle = fixture.handle.clone();
        let woken = member.clone();
        tokio::spawn(async move { handle.ensure_member_live(&woken).await })
    };
    fixture.preparer.wait_entered(entered + 1).await;
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Waking))
        .await;
    tokio::time::timeout(WAIT, fixture.handle.stop())
        .await
        .expect("the stop answers")
        .expect("the stop lands while the first wake is held");
    let first = tokio::time::timeout(WAIT, first)
        .await
        .expect("the first wake answers")
        .expect("the first wake task joins");
    assert!(
        matches!(
            first,
            Err(MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::MobNotRunning,
                ..
            })
        ),
        "the stop ended the first wake: {first:?}"
    );

    // The resume finishes once the member lane is empty; the mob runs again
    // before that.
    let resume = {
        let handle = fixture.handle.clone();
        tokio::spawn(async move { handle.resume().await })
    };
    tokio::time::timeout(STEP, async {
        while !matches!(fixture.handle.status().await, Ok(MobState::Running)) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the mob runs again");
    let built_before = fixture.preparer.built().len();
    let second = {
        let handle = fixture.handle.clone();
        let woken = member.clone();
        tokio::spawn(async move { handle.ensure_member_live(&woken).await })
    };
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Waking))
        .await;
    // The second wake's restore does not start beside the first wake's step.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fixture.preparer.prepared_count(),
        entered + 1,
        "the second wake's restore waits for the first wake's step"
    );

    fixture.preparer.open_gate();
    tokio::time::timeout(WAIT, second)
        .await
        .expect("the second wake answers")
        .expect("the second wake task joins")
        .expect("the second wake makes the member live");
    tokio::time::timeout(WAIT, resume)
        .await
        .expect("the resume answers")
        .expect("the resume task joins")
        .expect("the mob resumes");
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live)
    );
    assert!(
        MobSessionService::live_session_actor_registered(&*fixture.service, &session)
            .await
            .expect("live session read"),
        "the member keeps the session actor its wake built"
    );
    fixture.preparer.wait_built(built_before + 1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let built = fixture.preparer.built();
    assert_eq!(
        built.len(),
        built_before + 1,
        "one restore materialized the member and reported its build: {built:?}"
    );
    assert_eq!(built[built_before].agent_identity, member);
    assert_eq!(built[built_before].session_id, session);
    fixture.finish().await;
}

/// A stop after a cold wake's Bound step (its plan applied, its readiness
/// held) closes the attempt: the readiness step's late commit fails its
/// attempt fence and rolls the provision back. The mob stops without a
/// fail-stop, the member is Dormant, and no live session actor is left.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_after_a_cold_wakes_bound_step_rolls_its_provision_back() {
    let fixture = WakeFixture::new("wake-stop-after-bound").await;
    let member = fixture.spawn("stop-after-bound-member").await;
    let session = fixture.session(&member).await;
    fixture.release(&member).await;

    let release = crate::runtime::actor::member_wake_turn::hold_wake_readiness_for_test(&member);
    let wake = {
        let handle = fixture.handle.clone();
        let woken = member.clone();
        tokio::spawn(async move { handle.ensure_member_live(&woken).await })
    };
    // The plan applied: the member's binding is requested (Bound).
    let key = mob_dsl::AgentIdentity::from_domain(&member);
    let mut state = fixture.handle.machine_state_watch_rx.clone();
    tokio::time::timeout(
        WAIT,
        state.wait_for(|state| state.member_wake_bound.contains(&key)),
    )
    .await
    .expect("the wake reaches its Bound step")
    .expect("the machine state stays published");

    let stop = {
        let handle = fixture.handle.clone();
        tokio::spawn(async move { handle.stop().await })
    };
    tokio::time::timeout(STEP, async {
        while !matches!(fixture.handle.status().await, Ok(MobState::Stopped)) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the stop lands while the wake's readiness is held");
    let _ = release.send(true);
    let woken = tokio::time::timeout(WAIT, wake)
        .await
        .expect("the wake answers")
        .expect("the wake task joins");
    assert!(
        matches!(
            woken,
            Err(MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::MobNotRunning,
                ..
            })
        ),
        "the stop ended the wake: {woken:?}"
    );
    tokio::time::timeout(WAIT, stop)
        .await
        .expect("the stop answers")
        .expect("the stop task joins")
        .expect("the stop succeeds: no fail-stop");
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Dormant),
        "the stop left the member dormant"
    );
    tokio::time::timeout(WAIT, async {
        while MobSessionService::live_session_actor_registered(&*fixture.service, &session)
            .await
            .unwrap_or(false)
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the wake's provision is rolled back: no live session actor is left");
    fixture.finish().await;
}

/// A complete during a wake is admitted at once: it retires the waking
/// member, whose retirement start closes the wake (its callers are answered
/// as retiring, or as a mob that is not running) and whose own steps wait
/// for the held wake step, which rolls back. The complete then finishes,
/// and no live session actor is left.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_complete_during_a_wake_retires_the_member_after_its_wake_step() {
    let fixture = WakeFixture::new("wake-complete").await;
    let member = fixture.spawn("complete-member").await;
    let session = fixture.session(&member).await;
    fixture.release(&member).await;

    fixture.preparer.close_gate();
    let entered = fixture.preparer.prepared_count();
    let wake = {
        let handle = fixture.handle.clone();
        let woken = member.clone();
        tokio::spawn(async move { handle.ensure_member_live(&woken).await })
    };
    fixture.preparer.wait_entered(entered + 1).await;
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Waking))
        .await;

    let complete = {
        let handle = fixture.handle.clone();
        tokio::spawn(async move { handle.complete().await })
    };
    let woken = tokio::time::timeout(WAIT, wake)
        .await
        .expect("the wake's callers are answered while its step is held")
        .expect("the wake task joins");
    assert!(
        matches!(
            &woken,
            Err(MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::Retiring
                    | crate::MemberWakeRefusal::MobNotRunning,
                ..
            })
        ),
        "the complete ended the wake: {woken:?}"
    );

    fixture.preparer.open_gate();
    tokio::time::timeout(WAIT, complete)
        .await
        .expect("the complete answers once the wake step settled")
        .expect("the complete task joins")
        .expect("the mob completes");
    assert_eq!(
        fixture.handle.status().await.expect("mob status"),
        MobState::Completed
    );
    assert!(
        !MobSessionService::live_session_actor_registered(&*fixture.service, &session)
            .await
            .unwrap_or(false),
        "the complete leaves no live session actor"
    );
    fixture.finish().await;
}

/// A wake rebuilds the member through the customizer's resume hook,
/// with what the customizer answers now, never a previous build's spec.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_asks_the_customizer_again_with_its_current_answer() {
    let fixture = WakeFixture::new("wake-r123").await;
    let member = fixture.spawn("r123-member").await;
    fixture.release(&member).await;
    let session = fixture.session(&member).await;
    let resumes_before = fixture.customizer.resumes().len();

    fixture.customizer.set_version(2);
    fixture
        .ensure_live(&member)
        .await
        .expect("the released member wakes");

    let resumes = fixture.customizer.resumes();
    assert_eq!(
        resumes.len(),
        resumes_before + 1,
        "the wake asks the customizer's resume hook exactly once"
    );
    assert_eq!(
        resumes.last(),
        Some(&(member.clone(), session, 2)),
        "the member is rebuilt with the customizer's current answer"
    );
    fixture.finish().await;
}

/// An external wire or unwire whose trust row belongs to a member that is
/// being woken waits for the wake, then lands on the woken member's runtime
/// (the command is parked while the member's wake attempt is open).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_external_wire_during_a_wake_waits_for_it_and_installs_on_the_woken_member() {
    let fixture = WakeFixture::new("wake-external-wire").await;
    let member = fixture.spawn("ext-wire-member").await;
    let external_name = format!("external-peer-{}", Uuid::new_v4().simple());
    let external =
        test_trusted_peer_descriptor(&external_name, &format!("inproc://{external_name}"));
    let trusts_external = |peers: &[meerkat_core::comms::PeerDirectoryEntry]| {
        peers
            .iter()
            .any(|peer| peer.name.as_str() == external_name.as_str())
    };

    // A wire while the member's wake is held.
    fixture.release(&member).await;
    fixture.preparer.close_gate();
    let entered = fixture.preparer.prepared_count();
    let wake = {
        let handle = fixture.handle.clone();
        let woken = member.clone();
        tokio::spawn(async move { handle.ensure_member_live(&woken).await })
    };
    fixture.preparer.wait_entered(entered + 1).await;
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Waking))
        .await;
    let wire = {
        let handle = fixture.handle.clone();
        let local = member.clone();
        let target = PeerTarget::External(external.clone());
        tokio::spawn(async move { handle.wire(local, target).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !wire.is_finished(),
        "the wire waits for the member's wake while it is held"
    );
    fixture.preparer.open_gate();
    tokio::time::timeout(WAIT, wake)
        .await
        .expect("the wake answers")
        .expect("the wake task joins")
        .expect("the member wakes");
    tokio::time::timeout(WAIT, wire)
        .await
        .expect("the wire answers after the wake")
        .expect("the wire task joins")
        .expect("the external wire lands");
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live)
    );
    assert!(
        trusts_external(&fixture.comms(&member).await.peers().await),
        "the woken member's runtime trusts the external peer"
    );
    let wired_events = fixture
        .handle
        .poll_events(0, 100_000)
        .await
        .expect("read the journal")
        .into_iter()
        .filter(|event| {
            matches!(
                &event.kind,
                crate::event::MobEventKind::ExternalPeerWired { local, spec }
                    if local == &member && spec.name == external.name
            )
        })
        .count();
    assert_eq!(wired_events, 1, "the journal holds one ExternalPeerWired");

    // The mirror: an unwire while the member's wake is held.
    fixture.release(&member).await;
    fixture.preparer.close_gate();
    let entered = fixture.preparer.prepared_count();
    let wake = {
        let handle = fixture.handle.clone();
        let woken = member.clone();
        tokio::spawn(async move { handle.ensure_member_live(&woken).await })
    };
    fixture.preparer.wait_entered(entered + 1).await;
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Waking))
        .await;
    let unwire = {
        let handle = fixture.handle.clone();
        let local = member.clone();
        let target = PeerTarget::External(external.clone());
        tokio::spawn(async move { handle.unwire(local, target).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !unwire.is_finished(),
        "the unwire waits for the member's wake while it is held"
    );
    fixture.preparer.open_gate();
    tokio::time::timeout(WAIT, wake)
        .await
        .expect("the wake answers")
        .expect("the wake task joins")
        .expect("the member wakes");
    tokio::time::timeout(WAIT, unwire)
        .await
        .expect("the unwire answers after the wake")
        .expect("the unwire task joins")
        .expect("the external unwire lands");
    assert!(
        !trusts_external(&fixture.comms(&member).await.peers().await),
        "the woken member's runtime no longer trusts the external peer"
    );
    fixture.finish().await;
}

/// A background completion on an idle member is an empty resume: the wake
/// finds no pending boundary. A mob member's registration is
/// composition-owned, so MeerkatMachine keeps it serving
/// (`ResolveEmptyResumeRetirement` answers `KeepServing`). Nothing is
/// unregistered or released, the member's other background operation keeps
/// running, and a peer message reaches the same materialization, with no wake
/// and no rebuild.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_background_completion_on_an_idle_member_keeps_it_loaded() {
    use meerkat_core::ops_lifecycle::{
        CompletionCursorConsumer, OperationId, OperationKind, OperationResult, OperationSpec,
        OperationStatus, OpsLifecycleRegistry as _, OpsOwnerAdmission,
    };

    let fixture = WakeFixture::new("wake-keep-serving").await;
    let sender = fixture.spawn("keep-sender").await;
    let member = fixture.spawn("keep-member").await;
    fixture.wire(&sender, &member).await;
    // The member is idle on a finished turn: nothing is pending.
    fixture
        .host_turn(&member, &token("keep-warm"), HandlingMode::Queue)
        .await;

    let session = fixture.session(&member).await;
    let machine = MobSessionService::acquire_runtime_adapter(&*fixture.service, None)
        .expect("the service's runtime")
        .expect("a runtime-backed service");
    let registration = machine
        .current_session_registration_witness(&session)
        .await
        .expect("the live member is registered");
    let registry = machine
        .ops_lifecycle_registry(&session)
        .await
        .expect("the member's operation registry");
    let background = |name: &str| OperationSpec {
        id: OperationId::new(),
        kind: OperationKind::BackgroundToolOp,
        owner_session_id: session.clone(),
        display_name: name.to_string(),
        source_label: "member-wake-rows".to_string(),
        operation_source: None,
        child_session_id: None,
        expect_peer_channel: false,
    };
    let finished = background("finished-job");
    let running = background("still-running-job");
    let finished_id = finished.id.clone();
    let running_id = running.id.clone();
    for spec in [finished, running] {
        let id = spec.id.clone();
        registry
            .register_operation(spec)
            .expect("register a background operation");
        registry
            .provisioning_succeeded(&id)
            .expect("the background operation runs");
    }
    let injected_cursor = || {
        registry
            .completion_cursor(CompletionCursorConsumer::RuntimeInjected)
            .expect("the runtime's injected cursor")
            .unwrap_or(0)
    };
    let injected_before = injected_cursor();
    let builds_before = fixture.preparer.prepared_count();

    registry
        .complete_operation(
            &finished_id,
            OperationResult {
                id: finished_id.clone(),
                content: "done".to_string(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("the first background operation completes");

    // The runtime injects the completion wake and applies it: an empty
    // resume of the idle session.
    tokio::time::timeout(WAIT, async {
        loop {
            if injected_cursor() > injected_before
                && !machine.session_has_active_inputs(&session).await
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the completion wake is injected and its empty resume applied");

    // A retiring registration would release the member to Dormant: give a
    // teardown the time it takes, and see the member stay Live.
    let key = mob_dsl::AgentIdentity::from_domain(&member);
    let mut machine_state = fixture.handle.machine_state_watch_rx.clone();
    let left_live = tokio::time::timeout(
        STEP,
        machine_state.wait_for(|current| {
            current.member_materialization.get(&key).copied()
                != Some(mob_dsl::MemberMaterializationPhase::Live)
        }),
    )
    .await
    .is_ok();
    assert!(
        !left_live,
        "the member stays Live after the empty resume; it is {:?}",
        fixture.residency(&member)
    );
    let after = machine
        .current_session_registration_witness(&session)
        .await
        .expect("the member's registration is still current");
    assert_eq!(
        after.epoch_id(),
        registration.epoch_id(),
        "the same registration keeps serving: nothing was unregistered"
    );
    assert_eq!(
        registry
            .snapshot(&running_id)
            .expect("read the second operation")
            .map(|operation| operation.status),
        Some(OperationStatus::Running),
        "the member's other background operation keeps running"
    );
    assert_eq!(registry.owner_admission(), Ok(OpsOwnerAdmission::Open));

    let body = token("keep");
    tokio::time::timeout(
        WAIT,
        fixture.handle.send_peer_message(
            sender.clone(),
            member.clone(),
            body.as_str(),
            HandlingMode::Queue,
        ),
    )
    .await
    .expect("the send answers")
    .expect("the loaded member takes the message");
    assert_eq!(fixture.delivered_count(&member, &body).await, 1);
    assert_eq!(
        fixture.preparer.prepared_count(),
        builds_before,
        "nothing woke or rebuilt the member"
    );
    assert_eq!(
        machine
            .current_session_registration_witness(&session)
            .await
            .map(|current| current.epoch_id().clone()),
        Some(registration.epoch_id().clone()),
        "the message ran on the registration the completion left serving"
    );
    fixture.finish().await;
}

/// A customizer may return a member's external tools in any order, and a
/// wake asks it again. The member lists them in name order, so the woken
/// member's first request carries the tools array it sent before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_lists_the_customizers_tools_in_the_order_it_listed_them_before() {
    let fixture = WakeFixture::new("wake-tool-order").await;
    fixture
        .customizer
        .set_overlay_tools(&["zeta_overlay", "alpha_overlay", "mid_overlay"]);
    let member = fixture.spawn("tool-order-member").await;

    let before_body = token("tool-order-before");
    fixture
        .host_turn(&member, &before_body, HandlingMode::Queue)
        .await;
    let before = fixture.client.tools_of_request_mentioning(&before_body);
    let overlay_before: Vec<&str> = before
        .iter()
        .map(String::as_str)
        .filter(|name| name.ends_with("_overlay"))
        .collect();
    assert_eq!(
        overlay_before,
        ["alpha_overlay", "mid_overlay", "zeta_overlay"],
        "the customizer's tools are listed in name order: {before:?}"
    );

    fixture.release(&member).await;
    let after_body = token("tool-order-after");
    fixture
        .host_turn(&member, &after_body, HandlingMode::Queue)
        .await;
    assert!(
        fixture.customizer.overlay_answers.load(Ordering::SeqCst) >= 2,
        "the wake asked the customizer again, which answered in another order"
    );
    assert_eq!(
        fixture.client.tools_of_request_mentioning(&after_body),
        before,
        "the woken member's first request carries the tools array it sent before"
    );
    fixture.finish().await;
}

/// A member rebuilt on another auth binding (here a wake whose customizer
/// now names one) has that binding persisted as its session's recorded
/// binding before it serves: a read right after the wake, before any turn,
/// sees the new binding, so a follow-up attach or ensure that checks the
/// recorded binding never sees the old one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rebuilt_members_binding_is_recorded_before_it_serves() {
    let fixture = WakeFixture::new_with_config("wake-binding-recorded", |config| {
        config.realm.insert(
            "global".to_string(),
            meerkat_core::RealmConfigSection::from_inline_api_keys(&[("openai", "test-key")]),
        );
    })
    .await;
    let member = fixture.spawn("binding-member").await;
    let session = fixture.session(&member).await;
    let recorded = |metadata: Option<meerkat_core::PersistedSessionMetadataView>| {
        metadata
            .and_then(|view| view.session_metadata)
            .and_then(|metadata| metadata.auth_binding)
    };
    let binding = meerkat_core::AuthBindingRef {
        realm: meerkat_core::RealmId::global(),
        binding: meerkat_core::BindingId::parse("default_openai").expect("binding id"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    };
    assert_ne!(
        recorded(
            MobSessionService::load_persisted_session_metadata(&*fixture.service, &session)
                .await
                .expect("read the recorded metadata")
        ),
        Some(binding.clone()),
        "precondition: the member was not built on the binding its wake names"
    );
    fixture.customizer.set_auth_binding(binding.clone());
    fixture.release(&member).await;
    fixture
        .ensure_live(&member)
        .await
        .expect("the released member wakes on the new binding");

    assert_eq!(
        recorded(
            MobSessionService::load_persisted_session_metadata(&*fixture.service, &session)
                .await
                .expect("read the recorded metadata")
        ),
        Some(binding),
        "the woken member's binding is recorded before it runs any turn"
    );
    fixture.finish().await;
}

/// A retirement that starts while its member is being woken closes the wake,
/// and the wake's callers are told the member is retiring (not a fenced
/// lifecycle origin): the retire start removes the member's phase, and its
/// retiring marker is the answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retire_during_a_wake_answers_its_callers_member_retiring() {
    let fixture = WakeFixture::new("wake-retiring").await;
    let member = fixture.spawn("retiring-member").await;
    fixture.release(&member).await;

    fixture.preparer.close_gate();
    let entered = fixture.preparer.prepared_count();
    let wake = {
        let handle = fixture.handle.clone();
        let woken = member.clone();
        tokio::spawn(async move { handle.ensure_member_live(&woken).await })
    };
    fixture.preparer.wait_entered(entered + 1).await;
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Waking))
        .await;

    let retire = {
        let handle = fixture.handle.clone();
        let retired = member.clone();
        tokio::spawn(async move { handle.retire(retired).await })
    };
    let woken = tokio::time::timeout(WAIT, wake)
        .await
        .expect("the wake's callers are answered at the retire start")
        .expect("the wake task joins");
    assert!(
        matches!(
            &woken,
            Err(MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::Retiring,
                ..
            })
        ),
        "a wake a retirement closes answers member_retiring: {woken:?}"
    );

    fixture.preparer.open_gate();
    tokio::time::timeout(WAIT, retire)
        .await
        .expect("the retirement finishes once the wake's provision rolls back")
        .expect("the retire task joins")
        .expect("the member retires");
    fixture.finish().await;
}

/// The composition carries each registration's epoch on both signals, the
/// binding's (`RuntimeBound`) and the release's
/// (`RuntimeRegistrationReleased`), through the mob's projection. A member
/// registered, released and woken again holds a fresh epoch, and a release
/// that states the earlier registration's epoch, with the same runtime id
/// and fence, is refused at the mob: the member stays Live on its new
/// registration. Were the epoch dropped on both signals, the two absent
/// epochs would match and that stale release would demote the member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_stating_an_earlier_registrations_epoch_is_refused_at_the_mob() {
    let fixture = WakeFixture::new("wake-epoch-aba").await;
    let member = fixture.spawn("epoch-aba-member").await;
    let key = mob_dsl::AgentIdentity::from_domain(&member);
    let epoch = || {
        fixture
            .handle
            .machine_state_watch_rx
            .borrow()
            .member_materialization_epoch
            .get(&key)
            .cloned()
            .flatten()
    };
    let first = epoch().expect("the first registration's epoch reached the mob with its binding");
    let seated = fixture.entry(&member).await;

    // The release demotes the member only when it states the epoch the mob
    // recorded, so the release signal carried it.
    fixture.release(&member).await;
    fixture
        .ensure_live(&member)
        .await
        .expect("a delivery wakes the released member");
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Live))
        .await;
    // The woken registration's binding lands with a fresh epoch.
    let mut state = fixture.handle.machine_state_watch_rx.clone();
    tokio::time::timeout(
        WAIT,
        state.wait_for(|state| {
            state
                .member_materialization_epoch
                .get(&key)
                .is_some_and(|recorded| recorded.as_ref().is_some_and(|epoch| *epoch != first))
        }),
    )
    .await
    .expect("the new registration's epoch reaches the mob with its binding")
    .expect("the machine state stays published");
    let second = epoch().expect("the new registration's epoch is recorded");
    assert_ne!(first, second, "a new registration has a fresh epoch");
    let woken = fixture.entry(&member).await;
    assert_eq!(
        (&woken.agent_runtime_id, woken.fence_token),
        (&seated.agent_runtime_id, seated.fence_token),
        "the wake restores the same incarnation, so only the epoch tells the registrations apart"
    );

    // The earlier registration's release, routed to the mob the way the
    // runtime routes a final unregister.
    let session = fixture.session(&member).await;
    let machine = MobSessionService::acquire_runtime_adapter(&*fixture.service, None)
        .expect("the service's runtime")
        .expect("a runtime-backed service");
    let dispatcher = machine
        .session_composition_signal_dispatcher_for_test(&session)
        .await
        .expect("the member's registration routes its signals to the mob");
    meerkat_runtime::meerkat_machine::composition::dispatch_routed_signal(
        &dispatcher,
        meerkat_runtime::meerkat_machine::composition::MeerkatSeamSignal::RuntimeRegistrationReleased {
            agent_runtime_id: Some(meerkat_runtime::meerkat_machine::dsl::AgentRuntimeId::from(
                mob_dsl::AgentRuntimeId::from_domain(&seated.agent_runtime_id).0,
            )),
            fence_token: Some(meerkat_runtime::meerkat_machine::dsl::FenceToken::from(
                seated.fence_token.get(),
            )),
            runtime_epoch_id: Some(meerkat_runtime::meerkat_machine::dsl::RuntimeEpochId(
                first.0.clone(),
            )),
        },
    )
    .await
    .expect("the stale release reaches the mob");
    // The actor handles its commands in order: a poll answered after the
    // signal was queued was handled after it.
    fixture
        .handle
        .poll_events(0, 1)
        .await
        .expect("an actor round trip after the signal");

    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live),
        "a release stating an earlier registration's epoch demotes nothing"
    );
    assert_eq!(
        epoch(),
        Some(second),
        "the mob keeps the new registration's epoch"
    );
    fixture.finish().await;
}

/// A release in the window after a member's spawn committed it Live and
/// before the routed binding recorded its registration's epoch keeps the
/// member's session: MobMachine answers that the member is not yet
/// releasable, the release is refused retryably and nothing is closed. Once
/// the binding landed, the same release makes the member dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_before_the_binding_lands_keeps_the_session_and_a_retry_releases_it() {
    let fixture = WakeFixture::new("wake-release-window").await;
    let member = AgentIdentity::from(format!("release-window-{}", Uuid::new_v4().simple()));
    let binding = crate::runtime::composition::ready_signal_test_hold::hold(member.as_str());
    let member = fixture.spawn_as(member).await;
    let key = mob_dsl::AgentIdentity::from_domain(&member);
    let epoch_recorded = |state: &mob_dsl::MobMachineState| {
        state
            .member_materialization_epoch
            .get(&key)
            .is_some_and(Option::is_some)
    };
    assert!(
        !epoch_recorded(&fixture.handle.machine_state_watch_rx.borrow()),
        "the held binding has not recorded the registration's epoch"
    );

    let refused = tokio::time::timeout(WAIT, fixture.handle.release_member(&member))
        .await
        .expect("the release answers");
    assert!(
        matches!(&refused, Err(MobError::LifecycleOperationPending { .. })),
        "a release before the binding lands is refused retryably: {refused:?}"
    );
    let session = fixture.session(&member).await;
    assert!(
        MobSessionService::live_session_actor_registered(&*fixture.service, &session)
            .await
            .expect("live session read"),
        "the refused release closed nothing"
    );
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live)
    );

    binding.send(true).expect("release the held binding");
    let mut state = fixture.handle.machine_state_watch_rx.clone();
    tokio::time::timeout(WAIT, state.wait_for(epoch_recorded))
        .await
        .expect("the binding records the registration's epoch in time")
        .expect("the machine state stays published");

    fixture.release(&member).await;
    fixture.finish().await;
}

/// A routed binding refusal of an abandoned wake attempt that lands while a
/// newer wake of the member is in flight is answered stale: the routed flush
/// drops it, the newer wake keeps its attempt and finishes, and the mob
/// stays up. Before the attempt fence, the late refusal broke the member
/// and closed the newer wake's attempt; with the fence but no stale handling
/// in the flush, the mob actor would exit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_binding_refusal_of_an_abandoned_attempt_leaves_the_newer_wake_running() {
    let fixture = WakeFixture::new("wake-late-refusal").await;
    let member = fixture.spawn("late-refusal-member").await;
    let seated = fixture.entry(&member).await;
    let session = fixture.session(&member).await;
    fixture.release(&member).await;
    let key = mob_dsl::AgentIdentity::from_domain(&member);
    let open_attempt = || {
        fixture
            .handle
            .machine_state_watch_rx
            .borrow()
            .member_wake_attempts
            .get(&key)
            .copied()
    };
    let wake_in_background = || {
        let handle = fixture.handle.clone();
        let member = member.clone();
        tokio::spawn(async move { handle.ensure_member_live(&member).await })
    };

    // The first wake is held in its build preparation, then refused
    // retryably: its attempt is abandoned and the member is Dormant again.
    fixture.preparer.close_gate();
    fixture.preparer.answer_with(PreparerAnswer::Retryable);
    let first_wake = wake_in_background();
    fixture.preparer.wait_entered(1).await;
    let abandoned = open_attempt().expect("the first wake's attempt is open");
    fixture.preparer.open_gate();
    let _ = tokio::time::timeout(WAIT, first_wake)
        .await
        .expect("the first wake answers")
        .expect("the first wake task joins");
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Dormant))
        .await;

    // The newer wake is held in its build preparation.
    fixture.preparer.close_gate();
    fixture.preparer.answer_with(PreparerAnswer::Prepared);
    let newer_wake = wake_in_background();
    fixture.preparer.wait_entered(2).await;
    let current = open_attempt().expect("the newer wake's attempt is open");
    assert_ne!(current, abandoned, "each wake has its own attempt");

    // The abandoned attempt's binding request reaches the runtime late. The
    // member's session has no registration while the newer wake is held,
    // so the runtime refuses it.
    let late = crate::runtime::composition::MobSeamEffect::routed(
        mob_dsl::MobMachineEffect::RequestRuntimeBinding {
            agent_identity: key.clone(),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&seated.agent_runtime_id),
            fence_token: mob_dsl::FenceToken(seated.fence_token.get()),
            generation: Some(mob_dsl::Generation(seated.generation.get())),
            session_id: mob_dsl::SessionId::from_domain(&session),
            attempt: Some(abandoned),
        },
    )
    .expect("a binding request is a routed effect");
    fixture
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::QueueRoutedEffectForTest {
            effect: late,
            reply_tx,
        })
        .await
        .expect("queue the late binding request")
        .await
        .expect("the actor answers the queue command")
        .expect("the actor queues the late binding request");
    // The boundary flush after the queue command dispatched it; a poll
    // answered after that flush proves the actor is still running.
    fixture
        .handle
        .poll_events(0, 1)
        .await
        .expect("the mob actor survives the late refusal");
    assert_eq!(
        open_attempt(),
        Some(current),
        "the newer wake keeps its attempt"
    );
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Waking)
    );

    fixture.preparer.open_gate();
    tokio::time::timeout(WAIT, newer_wake)
        .await
        .expect("the newer wake answers")
        .expect("the newer wake task joins")
        .expect("the newer wake finishes");
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live)
    );
    assert_eq!(
        fixture.handle.status().await.expect("mob status"),
        MobState::Running
    );
    fixture.finish().await;
}

/// A wake queued from a member-effect completion, not from a command,
/// starts with no further command: the actor starts the materializations
/// MobMachine handed off on every loop pass, not only after a command, so a
/// first touch never waits for an unrelated command.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_queued_from_a_member_effect_completion_starts_with_no_further_command() {
    let fixture = WakeFixture::new("wake-from-effect").await;
    let member = fixture.spawn("from-effect-member").await;
    fixture.release(&member).await;

    fixture
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::WakeMemberFromEffectForTest {
            agent_identity: member.clone(),
            reply_tx,
        })
        .await
        .expect("send the effect-completion wake")
        .await
        .expect("the actor answers")
        .expect("the actor dispatched the effect");

    // No further command is sent: the member is woken all the same.
    fixture
        .wait_residency(&member, Some(mob_dsl::MemberMaterializationPhase::Live))
        .await;
    fixture.finish().await;
}

/// Every path that keeps a member's live actor instead of rebuilding it (a
/// Resume-mode spawn, a restore that keeps the actor or a live successor, a
/// wake's live successor, a restore or a warm reload whose build found the
/// member's session already live, and the explicit resume of a mob stopped
/// in this process, at its preparation and at its already-live observation)
/// calls the one shared binding check, so none of them keeps an
/// actor that runs another binding than the member's spec. Where a restore,
/// a wake or a reload records the member broken on a conflict, it also
/// retires the live actor, so no surface reaches the old binding. Read from
/// the shipped source with a lexer pass (comments and literals blanked,
/// test-only items removed), never a text search.
#[test]
fn every_live_actor_reuse_path_calls_the_shared_binding_check() {
    use super::fail_stop_cause::{function_body, shipped_code, whole_identifier};

    const CHECK: &str = "verify_live_actor_auth_binding";
    // A path that records the member broken on a conflict retires its live
    // actor through either shared retire.
    const RETIRES: [&str; 2] = [
        "refuse_conflicting_live_actor",
        "retire_conflicting_live_actor",
    ];
    let runtime = std::path::PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set for test processes"),
    )
    .join("src")
    .join("runtime");
    // (file, the item the function is declared in, its signature, whether a
    // conflict there retires the live actor)
    let paths = [
        (
            "actor.rs",
            None,
            "async fn enqueue_spawn_with_origin(",
            false,
        ),
        (
            "actor.rs",
            Some("impl ExplicitResumePreparationContext"),
            "async fn prepare(",
            false,
        ),
        (
            "actor.rs",
            None,
            "fn spawn_explicit_resume_member_observation(",
            true,
        ),
        ("builder.rs", None, "async fn restore_resumed_member(", true),
        (
            "builder.rs",
            None,
            "async fn provision_durable_member_restore(",
            true,
        ),
        (
            "builder.rs",
            None,
            "async fn restore_member_on_successor(",
            true,
        ),
        (
            "actor/member_wake_turn.rs",
            None,
            "async fn plan_member_wake(",
            true,
        ),
        (
            "actor/reload_revival.rs",
            None,
            "async fn commit_warm_revival(",
            true,
        ),
    ];
    for (file, within, signature, retires) in paths {
        let source = std::fs::read_to_string(runtime.join(file))
            .unwrap_or_else(|error| panic!("read {file}: {error}"));
        let code = shipped_code(&source);
        let from = within.map_or(0, |item| {
            code.find(item)
                .unwrap_or_else(|| panic!("{file} has no shipped `{item}`"))
        });
        let body = function_body(&code[from..], signature)
            .map(|body| from + body.start..from + body.end)
            .unwrap_or_else(|| panic!("{file} has no shipped `{signature}`"));
        let calls = |name: &str| {
            code[body.clone()]
                .match_indices(name)
                .any(|(offset, found)| whole_identifier(&code, body.start + offset, found))
        };
        assert!(
            calls(CHECK),
            "{file} `{signature}` keeps a live actor without calling {CHECK}"
        );
        if retires {
            assert!(
                RETIRES.iter().any(|retire| calls(retire)),
                "{file} `{signature}` records a binding conflict without retiring the live actor"
            );
        }
    }
}

/// A transient failure of one of a woken member's wiring edges never breaks
/// or un-wakes the member: the wake closes Live and the member serves, the
/// edge is owed a repair in MobMachine, and the repair timer re-realizes it,
/// after which the owed repair is cleared and the member reaches its peer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transient_edge_failure_at_a_wake_is_repaired_and_the_member_keeps_serving() {
    let fixture = WakeFixture::new("wake-edge-repair").await;
    let member = fixture.spawn("edge-repair-member").await;
    let peer = fixture.spawn("edge-repair-peer").await;
    fixture.wire(&member, &peer).await;
    fixture.release(&member).await;
    let edge = mob_dsl::WiringEdge::new(
        mob_dsl::AgentIdentity::from_domain(&member),
        mob_dsl::AgentIdentity::from_domain(&peer),
    );

    crate::runtime::actor::member_wake_turn::fail_next_wake_edge_for_test(&member);
    fixture
        .ensure_live(&member)
        .await
        .expect("the wake closes Live although one of its edges failed");
    let mut state = fixture.handle.machine_state_watch_rx.clone();
    tokio::time::timeout(
        STEP,
        state.wait_for(|state| state.owed_edge_repairs.contains(&edge)),
    )
    .await
    .expect("the failed edge is owed a repair")
    .expect("the machine state stays published");

    let body = token("edge-repair-turn");
    fixture.host_turn(&member, &body, HandlingMode::Queue).await;
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live),
        "the member serves while its edge is owed"
    );

    tokio::time::timeout(
        WAIT,
        state.wait_for(|state| !state.owed_edge_repairs.contains(&edge)),
    )
    .await
    .expect("the repair timer realizes the edge")
    .expect("the machine state stays published");
    let peer_body = token("edge-repair-peer-message");
    fixture
        .raw_peer_send(&member, &peer, &peer_body)
        .await
        .expect("the repaired edge carries the member's message");
    assert_eq!(fixture.delivered_count(&peer, &peer_body).await, 1);
    fixture.finish().await;
}

/// A member woken among Dormant neighbours realizes its own side of each
/// edge to them (each Dormant end realizes its side at its own wake), so
/// every edge is reported realized: the member owes no edge repair, no
/// repair timer is armed (only an owed repair arms one), and the neighbours
/// stay Dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_waking_among_dormant_neighbours_owes_no_edge_repairs() {
    let fixture = WakeFixture::new("wake-dormant-neighbours").await;
    let member = fixture.spawn("dormant-neighbours-member").await;
    let first = fixture.spawn("dormant-neighbour-one").await;
    let second = fixture.spawn("dormant-neighbour-two").await;
    fixture.wire(&member, &first).await;
    fixture.wire(&member, &second).await;
    fixture.release(&first).await;
    fixture.release(&second).await;
    fixture.release(&member).await;

    fixture
        .ensure_live(&member)
        .await
        .expect("the member wakes among dormant neighbours");
    {
        let state = fixture.handle.machine_state_watch_rx.borrow();
        assert!(
            state.owed_edge_repairs.is_empty() && state.owed_external_edge_repairs.is_empty(),
            "an edge to a dormant neighbour is realized on the live side, never owed: {:?}",
            state.owed_edge_repairs
        );
    }
    for neighbour in [&first, &second] {
        assert_eq!(
            fixture.residency(neighbour),
            Some(mob_dsl::MemberMaterializationPhase::Dormant),
            "the member's wake leaves its neighbours dormant"
        );
    }
    fixture.finish().await;
}

/// A woken member's edges to Dormant neighbours are realized a chunk at a
/// time, each chunk one machine step: at two edges a chunk, five neighbours
/// take three batches of two, two and one edges, the turn yielding to the
/// actor between them. The woken member then trusts every neighbour's
/// durable endpoint, no edge owes a repair, and the neighbours stay Dormant.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_woken_among_dormant_neighbours_realizes_its_edges_a_chunk_at_a_time() {
    let fixture = WakeFixture::new("wake-topology-chunks").await;
    let member = fixture.spawn("chunked-member").await;
    let mut neighbours = Vec::new();
    for index in 0..5 {
        let neighbour = fixture.spawn(&format!("chunked-neighbour-{index}")).await;
        fixture.wire(&member, &neighbour).await;
        neighbours.push(neighbour);
    }
    for neighbour in &neighbours {
        fixture.release(neighbour).await;
    }
    fixture.release(&member).await;
    crate::runtime::actor::wake_topology::set_wake_topology_chunk_for_test(
        fixture.handle.mob_id(),
        2,
    );

    fixture
        .ensure_live(&member)
        .await
        .expect("the member wakes among dormant neighbours");
    assert_eq!(
        crate::runtime::actor::wake_topology::wake_topology_batches_for_test(
            fixture.handle.mob_id()
        ),
        vec![2, 2, 1],
        "each chunk of the woken member's edges is one machine step"
    );
    let trusted = fixture
        .comms(&member)
        .await
        .peers()
        .await
        .into_iter()
        .map(|peer| peer.peer_id.to_string())
        .collect::<BTreeSet<_>>();
    {
        let state = fixture.handle.machine_state_watch_rx.borrow();
        for neighbour in &neighbours {
            let peer_id = state
                .member_peer_ids
                .get(&mob_dsl::AgentIdentity::from_domain(neighbour))
                .expect("the neighbour's recorded endpoint");
            assert!(
                trusted.contains(&peer_id.0),
                "the woken member trusts {neighbour}'s durable endpoint: {trusted:?}"
            );
        }
        assert!(
            state.owed_edge_repairs.is_empty(),
            "no edge owes a repair: {:?}",
            state.owed_edge_repairs
        );
    }
    for neighbour in &neighbours {
        assert_eq!(
            fixture.residency(neighbour),
            Some(mob_dsl::MemberMaterializationPhase::Dormant),
            "the member's wake leaves its neighbours dormant"
        );
    }
    fixture.finish().await;
}

/// A mob over an in-memory session service (one that keeps no durable
/// sessions), with a real factory and real in-process comms: a member's
/// session exists only while the member is materialized.
struct InMemoryWakeFixture {
    _root: tempfile::TempDir,
    handle: MobHandle,
    client: ScriptedClient,
}

impl InMemoryWakeFixture {
    async fn new(label: &str) -> Self {
        let root = tempfile::Builder::new()
            .prefix("meerkat-member-wake-in-memory-")
            .tempdir()
            .expect("fixture storage in the system temp dir");
        let root_path = root.path().canonicalize().expect("absolute fixture root");
        for directory in ["config", "runtime", "project", "context"] {
            std::fs::create_dir_all(root_path.join(directory)).expect("fixture root");
        }
        let factory = meerkat::AgentFactory::new(root_path.join("factory-store"))
            .user_config_root(root_path.join("config"))
            .runtime_root(root_path.join("runtime"))
            .project_root(root_path.join("project"))
            .context_root(root_path.join("context"))
            .builtins(false)
            .comms(true);
        let mut config = meerkat::Config::default();
        config.agent.model = "gpt-5.5".to_string();
        config.compaction.auto_compact_threshold = 1_000_000;
        let client = ScriptedClient::new();
        let mut builder = meerkat::FactoryAgentBuilder::new(factory, config);
        builder.default_llm_client = Some(Arc::new(client.clone()));
        let service = Arc::new(meerkat_session::EphemeralSessionService::new(builder, 16));
        let handle = MobBuilder::new(wake_rows_definition(label), MobStorage::in_memory())
            .with_session_service(service)
            .allow_ephemeral_sessions(true)
            .with_default_llm_client(Arc::new(client.clone()))
            .create()
            .await
            .expect("create the mob");
        Self {
            _root: root,
            handle,
            client,
        }
    }

    /// Spawn one turn-driven worker and wait until it is Live and its
    /// registration's binding landed, so it can be released.
    async fn spawn(&self, label: &str) -> AgentIdentity {
        let identity = AgentIdentity::from(format!("{label}-{}", Uuid::new_v4().simple()));
        tokio::time::timeout(
            WAIT,
            self.handle
                .spawn_spec(SpawnMemberSpec::new("worker", identity.clone())),
        )
        .await
        .expect("the spawn answers")
        .expect("spawn the worker");
        let key = mob_dsl::AgentIdentity::from_domain(&identity);
        let mut state = self.handle.machine_state_watch_rx.clone();
        let ready = tokio::time::timeout(
            WAIT,
            state.wait_for(|state| {
                state.member_materialization.get(&key).copied()
                    == Some(mob_dsl::MemberMaterializationPhase::Live)
                    && state
                        .member_materialization_epoch
                        .get(&key)
                        .is_some_and(Option::is_some)
            }),
        )
        .await;
        assert!(
            matches!(ready, Ok(Ok(_))),
            "member {identity} never became Live with its binding landed"
        );
        identity
    }

    fn residency(&self, identity: &AgentIdentity) -> Option<mob_dsl::MemberMaterializationPhase> {
        self.handle
            .machine_state_watch_rx
            .borrow()
            .member_materialization
            .get(&mob_dsl::AgentIdentity::from_domain(identity))
            .copied()
    }

    /// The member's durable endpoint and its restore failure, if any.
    fn endpoint_and_failure(
        &self,
        identity: &AgentIdentity,
    ) -> (Option<mob_dsl::MemberPeerEndpoint>, Option<String>) {
        let key = mob_dsl::AgentIdentity::from_domain(identity);
        let state = self.handle.machine_state_watch_rx.borrow();
        (
            state.member_peer_endpoints.get(&key).cloned(),
            state.member_restore_failures.get(&key).cloned(),
        )
    }

    async fn session(&self, identity: &AgentIdentity) -> SessionId {
        self.handle
            .resolve_bridge_session_id(identity)
            .await
            .expect("a session-bound member")
    }

    async fn entry(&self, identity: &AgentIdentity) -> RosterEntry {
        self.handle
            .get_member(identity)
            .await
            .expect("roster read")
            .expect("a seated member")
    }

    /// Admit one host turn on `identity` without waiting for it.
    async fn start_turn(&self, identity: &AgentIdentity, text: &str) -> WorkTurnHandle {
        let entry = self.entry(identity).await;
        tokio::time::timeout(
            WAIT,
            self.handle.start_work_with_mode(
                entry.agent_runtime_id,
                entry.fence_token,
                WorkRef::new(),
                WorkSpec::new(text, WorkOrigin::Internal),
                HandlingMode::Queue,
            ),
        )
        .await
        .expect("the delivery is admitted in time")
        .expect("the delivery is admitted")
    }

    async fn ensure_live(&self, identity: &AgentIdentity) -> Result<(), MobError> {
        tokio::time::timeout(WAIT, self.handle.ensure_member_live(identity))
            .await
            .expect("the wake answers")
    }

    async fn host_turn(&self, identity: &AgentIdentity, text: &str) {
        let turn = self.start_turn(identity, text).await;
        tokio::time::timeout(WAIT, turn.wait())
            .await
            .expect("the turn finishes")
            .expect("the turn succeeds");
    }

    /// Release `identity` once it is quiescent: a release refused retryably
    /// because its last turn's input has not reached its terminal yet is
    /// asked again.
    async fn release_when_quiescent(&self, identity: &AgentIdentity) {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            match tokio::time::timeout(WAIT, self.handle.release_member(identity))
                .await
                .expect("the release answers")
            {
                Ok(()) => return,
                Err(MobError::LifecycleOperationPending { .. })
                    if tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(error) => panic!("release the quiescent member: {error:?}"),
            }
        }
    }

    async fn finish(self) {
        self.client.release();
        let _ = tokio::time::timeout(WAIT, self.handle.shutdown()).await;
    }
}

/// On a session service that keeps no durable sessions, a member's session
/// exists only while the member is materialized. A release keeps such a
/// member materialized and serving: it is refused retryably while the
/// member's turn runs, and otherwise answers with nothing released.
/// The member's next delivery finds it live on the same session, at the same
/// incarnation and endpoint, and its turns continue that session's
/// transcript. Releasing it would have discarded the session, and its next
/// wake could only build a new session whose comms identity its durable
/// endpoint refuses: the member would read Broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_release_without_durable_sessions_keeps_the_member_live_on_its_session() {
    let fixture = InMemoryWakeFixture::new("wake-in-memory-release").await;
    let member = fixture.spawn("in-memory-release").await;
    let before = fixture.entry(&member).await;
    let session = fixture.session(&member).await;
    let (endpoint, failure) = fixture.endpoint_and_failure(&member);
    assert!(
        endpoint.is_some(),
        "a spawned member has a durable endpoint"
    );
    assert_eq!(failure, None);
    let first = token("in-memory-first");
    fixture.host_turn(&member, &first).await;

    // Work in flight: refused retryably, nothing changes.
    fixture.client.block();
    let started_before = fixture.client.started_count();
    let busy = fixture.start_turn(&member, &token("in-memory-busy")).await;
    fixture.client.wait_started(started_before + 1).await;
    let refused = tokio::time::timeout(WAIT, fixture.handle.release_member(&member))
        .await
        .expect("the release answers");
    assert!(
        matches!(refused, Err(MobError::LifecycleOperationPending { .. })),
        "a release with work in flight is refused retryably: {refused:?}"
    );
    fixture.client.release();
    tokio::time::timeout(WAIT, busy.wait())
        .await
        .expect("the turn finishes")
        .expect("the turn succeeds");

    // Quiescent: answered, and nothing is released.
    fixture.release_when_quiescent(&member).await;
    let woken = fixture.ensure_live(&member).await;
    let (endpoint_after, failure_after) = fixture.endpoint_and_failure(&member);
    assert!(
        woken.is_ok() && failure_after.is_none(),
        "the released member's next delivery finds it on its session, not Broken: \
         {woken:?}, restore failure {failure_after:?}"
    );
    assert_eq!(
        fixture.residency(&member),
        Some(mob_dsl::MemberMaterializationPhase::Live),
        "the member stays materialized"
    );
    assert_eq!(
        fixture.session(&member).await,
        session,
        "no new session is minted"
    );
    let after = fixture.entry(&member).await;
    assert_eq!(
        (after.agent_runtime_id, after.generation, after.fence_token),
        (
            before.agent_runtime_id,
            before.generation,
            before.fence_token
        ),
        "the member keeps its incarnation"
    );
    assert_eq!(endpoint_after, endpoint, "the member keeps its endpoint");

    // Its next turn continues the same session's transcript.
    let second = token("in-memory-second");
    fixture.host_turn(&member, &second).await;
    let requests = fixture.client.requests.lock().expect("recorded requests");
    let carrying_second = requests
        .iter()
        .map(|request| format!("{:?}", request.messages))
        .filter(|messages| messages.contains(&second))
        .collect::<Vec<_>>();
    assert!(
        !carrying_second.is_empty()
            && carrying_second
                .iter()
                .all(|messages| messages.contains(&first)),
        "the turn after the release runs on the session that holds the first turn"
    );
    drop(requests);
    fixture.finish().await;
}

/// A kickoff status notice is content: a live member's kickoff notices to a
/// wired Dormant peer wake that peer once and are delivered, never dropped.
/// The notice leaves through the sender's own comms send, and the
/// recipient's wake route wakes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kickoff_notices_to_a_dormant_peer_wake_it_once() {
    let fixture = WakeFixture::new_with(
        "wake-kickoff-notice",
        |_| {},
        |definition| {
            definition.wiring.role_wiring = vec![RoleWiringRule {
                a: ProfileName::from("worker"),
                b: ProfileName::from("worker"),
            }];
        },
    )
    .await;
    let dormant = fixture.spawn("notice-dormant").await;
    fixture.release(&dormant).await;

    // A new autonomous worker is wired to the dormant one by the role rule
    // at its spawn, before its kickoff starts, so its kickoff notices go to
    // the dormant peer.
    let live = AgentIdentity::from(format!("notice-live-{}", Uuid::new_v4().simple()));
    let mut spec = SpawnMemberSpec::new("worker", live.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::AutonomousHost);
    tokio::time::timeout(WAIT, fixture.handle.spawn_spec(spec))
        .await
        .expect("the spawn answers")
        .expect("spawn the live member");
    fixture
        .handle
        .wait_for_members_kickoff_complete(std::slice::from_ref(&live), Some(WAIT))
        .await
        .expect("the live member's kickoff resolves");
    fixture
        .wait_residency(&dormant, Some(mob_dsl::MemberMaterializationPhase::Live))
        .await;
    let builds = fixture
        .preparer
        .prepared()
        .into_iter()
        .filter(|build| build.identity == dormant)
        .count();
    assert_eq!(builds, 1, "the notices woke the dormant peer once");
    fixture.finish().await;
}
