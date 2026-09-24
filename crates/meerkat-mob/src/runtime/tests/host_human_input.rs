#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]

use super::*;
use meerkat_core::lifecycle::run_primitive::TurnRequestContext;
use meerkat_core::types::TranscriptUserRole;
use meerkat_runtime::{LogicalRuntimeId, RuntimeStore};

const WAIT: Duration = Duration::from_secs(30);
const HUMAN: &str = "Please compare these options.\nKeep my exact wording: café & <human>.";
const INJECTED: &str = "host-only retrieval context";
const TRANSIENT: &str = "request-only ephemeral context";
const SYSTEM: &str = "ordinary per-turn system instructions";

fn deadline() -> std::time::Instant {
    std::time::Instant::now() + WAIT
}

fn delivery(label: &str) -> MobDeliveryIdentity {
    MobDeliveryIdentity::new(label, Uuid::new_v4().to_string()).expect("stable delivery identity")
}

fn interaction(delivery: &MobDeliveryIdentity) -> InteractionId {
    InteractionId(Uuid::parse_str(&delivery.correlation_id).expect("correlation UUID"))
}

fn llm_identity(session: &Session) -> SessionLlmIdentity {
    session
        .try_session_metadata()
        .expect("valid typed session metadata")
        .expect("factory session has durable model metadata")
        .llm_identity()
}

#[derive(Clone)]
struct ScriptedClient {
    requests: Arc<Mutex<Vec<meerkat_client::LlmRequest>>>,
    released: tokio::sync::watch::Sender<bool>,
}

impl ScriptedClient {
    fn new(released: bool) -> Self {
        let (released, _) = tokio::sync::watch::channel(released);
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            released,
        }
    }

    fn requests(&self) -> Vec<meerkat_client::LlmRequest> {
        self.requests.lock().expect("recorded requests").clone()
    }

    fn block(&self) {
        self.released.send_replace(false);
    }

    fn release(&self) {
        self.released.send_replace(true);
    }

    async fn wait_for_requests(&self, count: usize) {
        tokio::time::timeout(WAIT, async {
            loop {
                if self.requests.lock().expect("recorded requests").len() >= count {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("actual LLM request must start");
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
        let mut released = self.released.subscribe();
        Box::pin(async_stream::try_stream! {
            while !*released.borrow_and_update() {
                if released.changed().await.is_err() {
                    break;
                }
            }
            yield meerkat_client::LlmEvent::TextDelta {
                delta: format!("executor-answer-{sequence}"),
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
                outcome: meerkat_client::LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
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

type Service = meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>;

#[cfg(feature = "openai-live")]
struct ContextHost {
    service: Arc<Service>,
    appends: Mutex<Vec<String>>,
}

#[cfg(feature = "openai-live")]
#[async_trait]
impl meerkat_runtime::live_context_mirror::LiveContextMirrorHost for ContextHost {
    async fn committed_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<
        (
            meerkat_core::lifecycle::core_executor::BoundSessionCommit,
            String,
        ),
        String,
    > {
        self.service
            .export_live_context_committed_boundary(session_id)
            .await
            .map_err(|error| error.to_string())
    }

    async fn append_context(
        &self,
        authority: meerkat_runtime::live_execution::LiveContextAppendAuthority,
        context: String,
    ) -> Result<
        (
            meerkat_runtime::live_execution::LiveContextAppendAuthority,
            meerkat_core::LiveAppendDeliveryOutcome,
        ),
        String,
    > {
        self.appends
            .lock()
            .expect("committed context")
            .push(context);
        Ok((
            authority,
            meerkat_core::LiveAppendDeliveryOutcome::Acknowledged,
        ))
    }

    async fn recover_ambiguous_append(
        &self,
        _authority: meerkat_runtime::live_execution::LiveContextAmbiguityRecoveryAuthority,
    ) -> Result<(), String> {
        panic!("acknowledged fixture context must not require recovery");
    }

    async fn recover_ambiguous_delegation_result(
        &self,
        _authority: meerkat_runtime::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
    ) -> Result<(), String> {
        panic!("host human input is not a delegation result");
    }
}

struct Fixture {
    root: tempfile::TempDir,
    handle: MobHandle,
    service: Arc<Service>,
    runtime_store: Arc<dyn RuntimeStore>,
    entry: RosterEntry,
    session_id: SessionId,
    client: ScriptedClient,
    #[cfg(feature = "openai-live")]
    context: Arc<ContextHost>,
    #[cfg(feature = "openai-live")]
    binding: Option<meerkat_runtime::live_execution::LiveDelegationRuntimeBinding>,
}

impl Fixture {
    async fn new() -> Self {
        let mut fixture = Self::with_kickoff_blocked(false).await;
        fixture
            .handle
            .wait_for_kickoff_complete(Some(WAIT))
            .await
            .expect("real autonomous kickoff commits");
        #[cfg(feature = "openai-live")]
        {
            let (seed, _) = fixture
                .service
                .export_live_context_committed_boundary(&fixture.session_id)
                .await
                .expect("store-sealed autonomous seed");
            let cursor = seed
                .session()
                .expect("typed canonical seed")
                .messages()
                .len() as u64;
            let adapter = fixture.service.runtime_adapter().expect("runtime owner");
            adapter.set_live_context_mirror_host(fixture.context.clone());
            fixture.binding = Some(
                adapter
                    .__test_open_live_context_channel(&fixture.session_id, cursor)
                    .await
                    .expect("generated context channel"),
            );
        }
        // Without live support no field needs mutation, but the same fixture
        // still exercises the actual autonomous executor and durable store.
        #[cfg(not(feature = "openai-live"))]
        let _ = &mut fixture;
        fixture
    }

    async fn with_kickoff_blocked(blocked: bool) -> Self {
        let root = tempfile::Builder::new()
            .prefix(".host-human-input-")
            .tempdir_in(".")
            .expect("fixture storage stays inside this worktree");
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
        let client = ScriptedClient::new(!blocked);
        let mut builder = meerkat::FactoryAgentBuilder::new(factory, config);
        // A second controlling Mob reuses this service without the local
        // actor override. Its peer-only bridge session must stay scripted too.
        builder.default_llm_client = Some(Arc::new(client.clone()));
        let db = root_path.join("realm.db");
        let session_store =
            Arc::new(meerkat_store::SqliteSessionStore::open(&db).expect("session store"));
        builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
            session_store.clone(),
        )));
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(
            meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&db)
                .expect("co-tenant canonical runtime store"),
        );
        let service = Arc::new(Service::new(
            builder,
            16,
            session_store,
            runtime_store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ));
        let mut definition = with_unique_mob_id(sample_definition(), "host-human");
        let lead = definition
            .profiles
            .get_mut(&ProfileName::from("lead"))
            .and_then(ProfileBinding::as_inline_mut)
            .expect("inline lead");
        lead.model = "gpt-5.5".to_string();
        lead.runtime_mode = crate::MobRuntimeMode::AutonomousHost;
        lead.tools = ToolConfig {
            comms: true,
            ..ToolConfig::default()
        };
        let handle = MobBuilder::new(definition, MobStorage::in_memory())
            .with_session_service(service.clone())
            .with_default_llm_client(Arc::new(client.clone()))
            .create()
            .await
            .expect("create real autonomous mob");
        let identity = AgentIdentity::from("host-human-member");
        let session_id = handle
            .spawn(
                ProfileName::from("lead"),
                identity.clone(),
                Some("fixture kickoff".into()),
            )
            .await
            .expect("spawn autonomous member")
            .bridge_session_id()
            .expect("local session member")
            .clone();
        let entry = handle
            .get_member(&identity)
            .await
            .expect("roster read")
            .expect("member binding");
        assert_eq!(entry.runtime_mode, crate::MobRuntimeMode::AutonomousHost);
        Self {
            root,
            handle,
            #[cfg(feature = "openai-live")]
            context: Arc::new(ContextHost {
                service: service.clone(),
                appends: Mutex::new(Vec::new()),
            }),
            service,
            runtime_store,
            entry,
            session_id,
            client,
            #[cfg(feature = "openai-live")]
            binding: None,
        }
    }

    async fn start(
        &self,
        spec: WorkSpec,
        mode: HandlingMode,
        delivery: MobDeliveryIdentity,
    ) -> Result<WorkTurnHandle, MobError> {
        self.handle
            .start_host_human_input_bounded(
                self.entry.agent_runtime_id.clone(),
                self.entry.fence_token,
                spec,
                mode,
                delivery,
                deadline(),
            )
            .await
    }

    async fn submit(
        &self,
        spec: WorkSpec,
        mode: HandlingMode,
        delivery: MobDeliveryIdentity,
    ) -> Result<WorkDeliveryReceipt, MobError> {
        self.handle
            .submit_host_human_input_bounded(
                self.entry.agent_runtime_id.clone(),
                self.entry.fence_token,
                spec,
                mode,
                delivery,
                deadline(),
            )
            .await
    }

    async fn durable(&self) -> Session {
        tokio::time::timeout(WAIT, self.service.load_persisted_session(&self.session_id))
            .await
            .expect("durable read finishes")
            .expect("canonical read succeeds")
            .expect("member is durable")
    }

    async fn input_count(&self) -> usize {
        self.runtime_store
            .load_input_states(&LogicalRuntimeId::for_session(&self.session_id))
            .await
            .expect("runtime-owned input rows")
            .len()
    }

    async fn input_for_delivery(
        &self,
        delivery: &MobDeliveryIdentity,
    ) -> meerkat_runtime::store::ExactInputStateObservation {
        self.runtime_store
            .load_input_state_by_idempotency_key(
                &LogicalRuntimeId::for_session(&self.session_id),
                &meerkat_runtime::identifiers::IdempotencyKey::new(
                    delivery.idempotency_key.clone(),
                ),
            )
            .await
            .expect("exact runtime-owned key lookup")
            .expect("delivery has a canonical input owner")
    }

    async fn submit_generic(
        &self,
        spec: WorkSpec,
        delivery: MobDeliveryIdentity,
    ) -> WorkDeliveryReceipt {
        self.handle
            .submit_work_with_mode_and_delivery_identity_bounded(
                self.entry.agent_runtime_id.clone(),
                self.entry.fence_token,
                spec,
                HandlingMode::Queue,
                delivery,
                deadline(),
            )
            .await
            .expect("generic ingress receipt is not an authorship or completion receipt")
    }

    #[cfg(feature = "openai-live")]
    async fn mirrored(&self) -> Vec<MirroredRow> {
        tokio::time::timeout(
            WAIT,
            self.service
                .runtime_adapter()
                .expect("runtime owner")
                .drain_live_context_outbox(&self.session_id),
        )
        .await
        .expect("post-commit outbox drains")
        .expect("actual committed boundary reaches mirror");
        self.context
            .appends
            .lock()
            .expect("context appends")
            .iter()
            .map(|text| serde_json::from_str(text).expect("typed context row"))
            .collect()
    }

    #[cfg(feature = "openai-live")]
    async fn close_channel(&mut self) {
        if let Some(binding) = self.binding.take() {
            self.service
                .runtime_adapter()
                .expect("runtime owner")
                .__test_close_live_context_channel(&binding)
                .await
                .expect("close fixture channel without cancelling runtime work");
        }
    }

    async fn finish(mut self) {
        self.client.release();
        #[cfg(feature = "openai-live")]
        self.close_channel().await;
        #[cfg(not(feature = "openai-live"))]
        let _ = &mut self;
        tokio::time::timeout(WAIT, self.handle.shutdown())
            .await
            .expect("mob shutdown finishes")
            .expect("mob shutdown");
    }
}

async fn completed(turn: WorkTurnHandle) -> WorkDeliveryReceipt {
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("exact runtime completion arrives")
        .expect("exact runtime boundary commits")
}

fn assert_human(session: &Session, text: &str, id: InteractionId) {
    let users = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User(user) if user.identity.interaction_id == Some(id) => Some(user),
            _ => None,
        })
        .filter(|user| user.transcript_role == TranscriptUserRole::Conversational)
        .collect::<Vec<_>>();
    assert_eq!(
        users.len(),
        1,
        "exactly one conversational human per delivery"
    );
    assert_eq!(users[0].text_content(), text);
    assert!(users[0].identity.realtime_origin.is_none());
    assert!(
        external_event_sources(session.messages(), text).is_empty(),
        "host authorship must not become an ExternalEvent notice"
    );
}

fn external_event_sources<'a>(messages: &'a [Message], text: &str) -> Vec<&'a str> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::SystemNotice(notice) => Some(notice),
            _ => None,
        })
        .flat_map(|notice| notice.blocks.iter())
        .filter_map(|block| match block {
            SystemNoticeBlock::ExternalEvent {
                source,
                body,
                content,
                ..
            } if body.as_deref() == Some(text)
                || meerkat_core::types::text_content(content) == text =>
            {
                Some(source.as_str())
            }
            _ => None,
        })
        .collect()
}

#[cfg(feature = "openai-live")]
#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MirroredRow {
    role: MirrorRole,
    text: String,
}

#[cfg(feature = "openai-live")]
#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum MirrorRole {
    User,
    Assistant,
}

#[cfg(feature = "openai-live")]
fn conversational_rows(messages: &[Message]) -> Vec<MirroredRow> {
    messages
        .iter()
        .filter_map(|message| match message {
            Message::User(user) if user.transcript_role.is_conversational() => Some(MirroredRow {
                role: MirrorRole::User,
                text: user.text_content(),
            }),
            Message::BlockAssistant(assistant) => Some(MirroredRow {
                role: MirrorRole::Assistant,
                text: assistant.to_string(),
            }),
            _ => None,
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn autonomous_queue_commits_exact_human_answer_and_shared_context() {
    let fixture = Fixture::new().await;
    let before = fixture.durable().await;
    let identity_before = llm_identity(&before);
    let requests_before = fixture.client.requests().len();
    let delivery = delivery("canonical-human");
    let id = interaction(&delivery);
    let spec = WorkSpec::new(HUMAN, WorkOrigin::External)
        .with_interaction_id(id)
        .with_system_prompt(SYSTEM)
        .with_injected_context(vec![INJECTED.into()]);
    let turn = fixture
        .start(spec, HandlingMode::Queue, delivery)
        .await
        .expect("host Queue supports separate System and injected-context slots");
    let result = tokio::time::timeout(
        WAIT,
        turn.wait_bounded(BoundedResultSpec::new("human answer", 1024).expect("bound")),
    )
    .await
    .expect("bounded completion")
    .expect("Queue owns its exact answer");
    assert_eq!(result.result().session_id(), &fixture.session_id);

    let durable = fixture.durable().await;
    assert_human(&durable, HUMAN, id);
    assert_eq!(llm_identity(&durable), identity_before);
    let appended = &durable.messages()[before.messages().len()..];
    let slots = appended
        .iter()
        .filter_map(|message| match message {
            Message::User(user) => Some((user.transcript_role, user.text_content())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        slots,
        [
            (TranscriptUserRole::InjectedContext, INJECTED.to_string()),
            (TranscriptUserRole::Conversational, HUMAN.to_string()),
        ]
    );
    assert!(
        appended
            .iter()
            .any(|message| matches!(message, Message::System(system) if system.content == SYSTEM))
    );
    let requests = fixture.client.requests();
    assert_eq!(requests.len(), requests_before + 1);
    let request = requests.last().expect("human request");
    assert_eq!(request.model, "gpt-5.5");
    assert!(
        request
            .messages
            .iter()
            .any(|message| matches!(message, Message::System(system) if system.content == SYSTEM))
    );
    assert!(
        request
            .messages
            .iter()
            .any(|message| matches!(message, Message::User(user)
            if user.transcript_role == TranscriptUserRole::InjectedContext
                && user.text_content() == INJECTED))
    );
    let answer = format!("executor-answer-{}", requests_before + 1);
    assert!(appended.iter().any(
        |message| matches!(message, Message::BlockAssistant(assistant)
            if assistant.to_string() == answer && assistant.identity.interaction_id == Some(id))
    ));
    assert_eq!(result.result().result().text(), answer);

    #[cfg(feature = "openai-live")]
    {
        let mirrored = fixture.mirrored().await;
        assert_eq!(mirrored, conversational_rows(appended));
        assert!(
            mirrored
                .iter()
                .all(|row| row.text != SYSTEM && row.text != INJECTED)
        );
    }
    let connection =
        rusqlite::Connection::open(fixture.root.path().join("realm.db")).expect("canonical DB");
    let (runtime_token, session_token): (String, String) = connection
        .query_row(
            "SELECT r.committed_head_token, s.cas_token \
             FROM runtime_session_authority r JOIN session_heads s USING (session_id) \
             WHERE r.session_id = ?1",
            [fixture.session_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("one exact store-owned canonical head");
    assert_eq!(runtime_token, session_token);
    drop(connection);
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn generic_autonomous_work_remains_external_event_not_mirrored_human() {
    let fixture = Fixture::new().await;
    let before = fixture.durable().await;
    let generic_delivery = delivery("generic-work");
    let id = interaction(&generic_delivery);
    let requests_before = fixture.client.requests().len();
    fixture
        .handle
        .submit_work_with_mode_and_delivery_identity_bounded(
            fixture.entry.agent_runtime_id.clone(),
            fixture.entry.fence_token,
            WorkSpec::new(HUMAN, WorkOrigin::External).with_interaction_id(id),
            HandlingMode::Queue,
            generic_delivery,
            deadline(),
        )
        .await
        .expect("generic autonomous ingress");
    fixture.client.wait_for_requests(requests_before + 1).await;
    // A real subsequent completion is an executor barrier, not a synthetic
    // event or a call that manually tells the mirror work has committed.
    let barrier = fixture
        .start(
            WorkSpec::new("generic settlement witness", WorkOrigin::Internal),
            HandlingMode::Queue,
            delivery("generic-settlement"),
        )
        .await
        .expect("settlement admission");
    completed(barrier).await;
    let durable = fixture.durable().await;
    let appended = &durable.messages()[before.messages().len()..];
    assert_eq!(
        external_event_sources(appended, HUMAN),
        ["rpc"],
        "generic transport provenance is unchanged"
    );
    assert!(
        !appended
            .iter()
            .any(|message| matches!(message, Message::User(user) if user.text_content() == HUMAN))
    );
    assert_eq!(llm_identity(&durable), llm_identity(&before));
    #[cfg(feature = "openai-live")]
    {
        let mirrored = fixture.mirrored().await;
        assert_eq!(mirrored, conversational_rows(appended));
        assert!(!mirrored.iter().any(|row| row.text == HUMAN));
    }
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_kind_generic_then_host_human_replay_refuses_reclassification() {
    let fixture = Fixture::new().await;
    let before = fixture.durable().await;
    let requests_before = fixture.client.requests().len();
    let key = delivery("generic-then-human");
    let id = interaction(&key);
    let spec = WorkSpec::new(HUMAN, WorkOrigin::External).with_interaction_id(id);
    fixture.submit_generic(spec.clone(), key.clone()).await;
    fixture.client.wait_for_requests(requests_before + 1).await;
    completed(
        fixture
            .start(
                WorkSpec::new("cross-kind settlement witness", WorkOrigin::Internal),
                HandlingMode::Queue,
                delivery("cross-kind-settlement"),
            )
            .await
            .expect("real executor completion settles the original external event"),
    )
    .await;
    let committed = fixture.durable().await;
    let original = fixture.input_for_delivery(&key).await;
    let inputs_before = fixture.input_count().await;
    let requests_before_replay = fixture.client.requests().len();
    assert!(original.state().state.prompt_replay_identity.is_none());
    assert!(
        original.state().state.persisted_input.is_none(),
        "cross-kind replay must still refuse after the original payload retires"
    );
    assert_eq!(
        external_event_sources(&committed.messages()[before.messages().len()..], HUMAN),
        ["rpc"]
    );
    #[cfg(feature = "openai-live")]
    let mirrored_before = fixture.mirrored().await;

    let error = fixture
        .submit(spec, HandlingMode::Queue, key.clone())
        .await
        .expect_err("a generic key is not proof of prior human admission");
    assert!(
        matches!(
            error,
            MobError::WorkInputIdempotencyConflict { session_id, input_id }
                if session_id == fixture.session_id
                    && input_id == original.state().state.input_id
        ),
        "strict host replay must return the original input's typed conflict"
    );
    let replayed = fixture.input_for_delivery(&key).await;
    assert_eq!(replayed.exact_row_digest(), original.exact_row_digest());
    assert_eq!(fixture.input_count().await, inputs_before);
    assert_eq!(fixture.client.requests().len(), requests_before_replay);
    let after = fixture.durable().await;
    assert_eq!(after.messages(), committed.messages());
    assert_eq!(llm_identity(&after), llm_identity(&before));
    assert!(
        !after
            .messages()
            .iter()
            .any(|message| matches!(message, Message::User(user) if user.text_content() == HUMAN))
    );
    #[cfg(feature = "openai-live")]
    {
        assert_eq!(fixture.mirrored().await, mirrored_before);
        assert!(!mirrored_before.iter().any(|row| row.text == HUMAN));
    }
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cross_kind_host_human_then_generic_replay_preserves_original_human_fate() {
    let fixture = Fixture::new().await;
    let before = fixture.durable().await;
    let key = delivery("human-then-generic");
    let id = interaction(&key);
    let spec = WorkSpec::new(HUMAN, WorkOrigin::External).with_interaction_id(id);
    let original_receipt = completed(
        fixture
            .start(spec.clone(), HandlingMode::Queue, key.clone())
            .await
            .expect("original human admission"),
    )
    .await;
    let committed = fixture.durable().await;
    assert_human(&committed, HUMAN, id);
    let original = fixture.input_for_delivery(&key).await;
    assert!(original.state().state.prompt_replay_identity.is_some());
    assert!(
        original.state().state.persisted_input.is_none(),
        "the typed prompt replay witness must outlive its payload"
    );
    let inputs_before = fixture.input_count().await;
    let requests_before = fixture.client.requests().len();
    #[cfg(feature = "openai-live")]
    let mirrored_before = fixture.mirrored().await;
    let comms = fixture
        .service
        .comms_runtime(&fixture.session_id)
        .await
        .expect("real autonomous ingress owner");
    let ingress_before = comms
        .peer_ingress_queue_snapshot()
        .await
        .expect("read ingress counters");

    let generic_receipt = fixture.submit_generic(spec, key.clone()).await;
    assert_eq!(generic_receipt.work_ref, original_receipt.work_ref);
    assert_eq!(generic_receipt.runtime_id, original_receipt.runtime_id);
    // Generic submission acknowledges the inbox, not runtime admission.
    // Its legacy key-only policy preserves the original human input rather
    // than reclassifying it as an ExternalEvent.
    // Observe the actual queue owner's terminal handover before asserting
    // absence of execution; otherwise a late duplicate could evade the test.
    let ingress_after = tokio::time::timeout(WAIT, async {
        loop {
            let snapshot = comms
                .peer_ingress_queue_snapshot()
                .await
                .expect("read actual generic handover");
            if snapshot.runtime_handover_count > ingress_before.runtime_handover_count {
                break snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("generic retry reaches authoritative runtime handover");
    assert_eq!(
        ingress_after.terminal_outcomes.deduplicated,
        ingress_before.terminal_outcomes.deduplicated + 1
    );
    assert_eq!(
        ingress_after.terminal_outcomes.accepted,
        ingress_before.terminal_outcomes.accepted
    );
    assert_eq!(
        ingress_after.terminal_outcomes.rejected,
        ingress_before.terminal_outcomes.rejected
    );
    let correlation = ingress_after
        .last_delivery_correlation
        .expect("exact runtime handover correlation");
    assert_eq!(
        correlation.outcome,
        meerkat_core::PeerIngressTerminalOutcomeKind::Deduplicated
    );
    assert_eq!(correlation.interaction_id, Some(id));
    assert_eq!(
        correlation.existing_runtime_input_id.as_ref(),
        Some(&original.state().state.input_id)
    );
    let replayed = fixture.input_for_delivery(&key).await;
    assert_eq!(replayed.exact_row_digest(), original.exact_row_digest());
    assert_eq!(fixture.input_count().await, inputs_before);
    assert_eq!(fixture.client.requests().len(), requests_before);
    let after = fixture.durable().await;
    assert_eq!(after.messages(), committed.messages());
    assert_human(&after, HUMAN, id);
    assert_eq!(llm_identity(&after), llm_identity(&before));
    #[cfg(feature = "openai-live")]
    {
        assert_eq!(fixture.mirrored().await, mirrored_before);
        assert_eq!(
            mirrored_before,
            conversational_rows(&committed.messages()[before.messages().len()..])
        );
    }
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_queue_admissions_return_before_llm_and_same_key_executes_once() {
    let fixture = Fixture::new().await;
    let before = fixture.durable().await;
    let requests_before = fixture.client.requests().len();
    fixture.client.block();
    let first_delivery = delivery("held-human");
    let first_id = interaction(&first_delivery);
    let first = fixture
        .start(
            WorkSpec::new("held first human", WorkOrigin::Internal),
            HandlingMode::Queue,
            first_delivery,
        )
        .await
        .expect("first admission");
    fixture.client.wait_for_requests(requests_before + 1).await;
    let second_delivery = delivery("queued-human");
    let second_id = interaction(&second_delivery);
    let spec = WorkSpec::new(HUMAN, WorkOrigin::External);
    let second = tokio::time::timeout(
        Duration::from_secs(5),
        fixture.start(spec.clone(), HandlingMode::Queue, second_delivery.clone()),
    )
    .await
    .expect("active Queue admission must not wait for the blocked LLM")
    .expect("queued human accepted");
    let receipt = second.receipt().clone();
    let replay = fixture
        .handle
        .submit_host_human_input_for_identity_bounded(
            fixture.entry.agent_identity.clone(),
            spec.clone(),
            HandlingMode::Queue,
            second_delivery.clone(),
            deadline(),
        )
        .await
        .expect("identity-first same-key pending replay");
    assert_eq!(replay.work_ref, receipt.work_ref);
    assert_eq!(replay.runtime_id, receipt.runtime_id);
    assert_eq!(fixture.client.requests().len(), requests_before + 1);
    #[cfg(feature = "openai-live")]
    assert!(fixture.context.appends.lock().expect("context").is_empty());

    fixture.client.release();
    completed(first).await;
    let bound = BoundedResultSpec::new("original queued answer", 1024).expect("result bound");
    let original_answer = tokio::time::timeout(WAIT, second.wait_bounded(bound.clone()))
        .await
        .expect("original queued completion")
        .expect("original queued answer");
    let replay = fixture
        .submit(spec.clone(), HandlingMode::Queue, second_delivery.clone())
        .await
        .expect("same-key committed replay");
    assert_eq!(replay.work_ref, receipt.work_ref);
    assert_eq!(replay.runtime_id, receipt.runtime_id);
    let replay = fixture
        .start(spec, HandlingMode::Queue, second_delivery)
        .await
        .expect("completion-bearing terminal replay admission");
    let replayed_answer = tokio::time::timeout(WAIT, replay.wait_bounded(bound))
        .await
        .expect("terminal replay resolves")
        .expect("terminal replay retains its original answer, not CompletedWithoutResult");
    assert_eq!(replayed_answer.result(), original_answer.result());
    assert_eq!(replayed_answer.receipt().work_ref, receipt.work_ref);
    completed(
        fixture
            .start(
                WorkSpec::new("replay settlement witness", WorkOrigin::Internal),
                HandlingMode::Queue,
                delivery("replay-settlement"),
            )
            .await
            .expect("settlement barrier"),
    )
    .await;
    assert_eq!(fixture.client.requests().len(), requests_before + 3);
    let durable = fixture.durable().await;
    assert_human(&durable, "held first human", first_id);
    assert_human(&durable, HUMAN, second_id);
    assert_eq!(llm_identity(&durable), llm_identity(&before));
    #[cfg(feature = "openai-live")]
    assert_eq!(
        fixture.mirrored().await,
        conversational_rows(&durable.messages()[before.messages().len()..])
    );
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_queued_human_terminal_replay_cannot_become_success() {
    use meerkat_runtime::input_state::{
        InputAbandonReason, InputLifecycleState, InputTerminalOutcome,
    };

    const CANCELLATION: &str = "host cancelled this exact queued human";
    let fixture = Fixture::new().await;
    let before = fixture.durable().await;
    let requests_before = fixture.client.requests().len();
    fixture.client.block();
    let active = fixture
        .start(
            WorkSpec::new("uncancelled active work", WorkOrigin::Internal),
            HandlingMode::Queue,
            delivery("cancel-active"),
        )
        .await
        .expect("active predecessor");
    fixture.client.wait_for_requests(requests_before + 1).await;
    let key = delivery("cancel-queued-human");
    let spec = WorkSpec::new(HUMAN, WorkOrigin::External);
    let queued = fixture
        .start(spec.clone(), HandlingMode::Queue, key.clone())
        .await
        .expect("queued human admission");
    let admitted = fixture.input_for_delivery(&key).await;
    assert_eq!(admitted.state().seed.phase, InputLifecycleState::Queued);
    assert!(
        tokio::time::timeout(
            WAIT,
            fixture
                .service
                .runtime_adapter()
                .expect("runtime cancellation owner")
                .cancel_input_if_present(
                    &fixture.session_id,
                    &admitted.state().state.input_id,
                    CANCELLATION,
                ),
        )
        .await
        .expect("exact queued cancellation settles")
        .expect("cancel through runtime authority")
    );
    let cancelled = fixture.input_for_delivery(&key).await;
    assert!(matches!(
        cancelled.state().seed.terminal_outcome,
        Some(InputTerminalOutcome::Abandoned {
            reason: InputAbandonReason::Cancelled,
        })
    ));
    assert!(cancelled.state().state.persisted_input.is_none());
    assert!(cancelled.state().state.prompt_replay_identity.is_some());

    let original_failure = tokio::time::timeout(
        WAIT,
        queued.wait_bounded(BoundedResultSpec::new("cancelled human", 1024).expect("bound")),
    )
    .await
    .expect("original cancellation completion")
    .expect_err("queued cancellation cannot succeed");
    assert!(matches!(
        original_failure.failure(),
        BoundedTurnFailure::RuntimeTerminated { session_id, reason, .. }
            if session_id == &fixture.session_id && reason == CANCELLATION
    ));
    let replay = fixture
        .start(spec, HandlingMode::Queue, key.clone())
        .await
        .expect("same-key replay observes the known durable terminal");
    let replay_failure = tokio::time::timeout(
        WAIT,
        replay.wait_bounded(BoundedResultSpec::new("cancelled human", 1024).expect("bound")),
    )
    .await
    .expect("cancelled terminal replay resolves")
    .expect_err("replay must not fabricate CompletedWithoutResult success");
    assert!(matches!(
        replay_failure.failure(),
        BoundedTurnFailure::RuntimeTerminated { session_id, reason, .. }
            if session_id == &fixture.session_id && reason == CANCELLATION
    ));
    assert_eq!(
        fixture.input_for_delivery(&key).await.exact_row_digest(),
        cancelled.exact_row_digest()
    );
    assert_eq!(fixture.client.requests().len(), requests_before + 1);
    fixture.client.release();
    completed(active).await;
    completed(
        fixture
            .start(
                WorkSpec::new("cancellation settlement witness", WorkOrigin::Internal),
                HandlingMode::Queue,
                delivery("cancel-settlement"),
            )
            .await
            .expect("subsequent work remains usable"),
    )
    .await;
    let after = fixture.durable().await;
    assert_eq!(fixture.client.requests().len(), requests_before + 2);
    assert!(
        !after
            .messages()
            .iter()
            .any(|message| matches!(message, Message::User(user) if user.text_content() == HUMAN))
    );
    assert_eq!(llm_identity(&after), llm_identity(&before));
    #[cfg(feature = "openai-live")]
    assert_eq!(
        fixture.mirrored().await,
        conversational_rows(&after.messages()[before.messages().len()..])
    );
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_external_host_human_cannot_autospawn_through_enabled_policy() {
    let fixture = Fixture::new().await;
    fixture
        .handle
        .set_spawn_policy(Some(Arc::new(StaticLeadSpawnPolicy)))
        .await
        .expect("install real external auto-spawn policy");
    assert!(
        fixture
            .handle
            .query_machine_state()
            .await
            .expect("policy authority")
            .spawn_policy_enabled
    );
    let missing = AgentIdentity::from("missing-host-human");
    let runtime_id = AgentRuntimeId::initial(missing.clone());
    let requests_before = fixture.client.requests().len();
    let before = fixture.durable().await;
    let spec = WorkSpec::new(HUMAN, WorkOrigin::External);
    let bound_error = fixture
        .handle
        .submit_host_human_input_bounded(
            runtime_id.clone(),
            FenceToken::new(0),
            spec.clone(),
            HandlingMode::Queue,
            delivery("missing-bound"),
            deadline(),
        )
        .await
        .expect_err("bound host input must not auto-provision a missing external target");
    assert!(matches!(bound_error, MobError::MemberNotFound(identity) if identity == missing));
    let start_error = fixture
        .handle
        .start_host_human_input_bounded(
            runtime_id,
            FenceToken::new(0),
            spec.clone(),
            HandlingMode::Queue,
            delivery("missing-start"),
            deadline(),
        )
        .await
        .expect_err("completion-bearing host input cannot retarget a missing binding");
    assert!(matches!(start_error, MobError::MemberNotFound(identity) if identity == missing));
    let identity_error = fixture
        .handle
        .submit_host_human_input_for_identity_bounded(
            missing.clone(),
            spec,
            HandlingMode::Queue,
            delivery("missing-identity"),
            deadline(),
        )
        .await
        .expect_err("identity-first host input must also refuse absence");
    assert!(matches!(identity_error, MobError::MemberNotFound(identity) if identity == missing));
    assert!(
        fixture
            .handle
            .get_member(&missing)
            .await
            .expect("roster observation after rejection")
            .is_none()
    );
    assert_eq!(fixture.client.requests().len(), requests_before);
    assert_eq!(fixture.durable().await.messages(), before.messages());
    #[cfg(feature = "openai-live")]
    assert!(fixture.mirrored().await.is_empty());
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_steer_commits_human_and_runtime_owned_answer() {
    let fixture = Fixture::new().await;
    let before = fixture.durable().await;
    let requests_before = fixture.client.requests().len();
    fixture.client.block();
    let active = fixture
        .start(
            WorkSpec::new("active executor work", WorkOrigin::Internal),
            HandlingMode::Queue,
            delivery("active-for-steer"),
        )
        .await
        .expect("active turn");
    fixture.client.wait_for_requests(requests_before + 1).await;
    let delivery = delivery("steered-human");
    let id = interaction(&delivery);
    let steer = tokio::time::timeout(
        Duration::from_secs(5),
        fixture.start(
            WorkSpec::new(HUMAN, WorkOrigin::External).with_interaction_id(id),
            HandlingMode::Steer,
            delivery.clone(),
        ),
    )
    .await
    .expect("Steer admission returns before the LLM")
    .expect("active human Steer admission");
    let admitted = fixture.input_for_delivery(&delivery).await;
    let semantics = admitted
        .state()
        .state
        .runtime_semantics
        .as_ref()
        .expect("generated runtime admission semantics");
    assert_eq!(
        semantics.boundary(),
        meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint
    );
    assert_eq!(
        semantics.execution_kind(),
        meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn
    );
    assert_eq!(
        admitted
            .state()
            .state
            .persisted_input
            .as_ref()
            .expect("active Steer retains its admitted input")
            .handling_mode(),
        Some(HandlingMode::Steer)
    );
    assert_eq!(
        admitted
            .state()
            .state
            .policy
            .as_ref()
            .expect("generated admission policy")
            .decision
            .routing_disposition,
        meerkat_runtime::policy::RoutingDisposition::Steer
    );
    fixture.client.release();
    completed(active).await;
    let result = tokio::time::timeout(
        WAIT,
        steer.wait_bounded(BoundedResultSpec::new("steer", 1024).expect("bound")),
    )
    .await
    .expect("runtime-owned Steer terminal")
    .expect("generated ContentTurn Steer owns its actual committed answer");
    assert_eq!(result.result().session_id(), &fixture.session_id);
    let durable = fixture.durable().await;
    assert_human(&durable, HUMAN, id);
    assert_eq!(llm_identity(&durable), llm_identity(&before));
    let requests = fixture.client.requests();
    let answered_request = requests
        .iter()
        .rposition(|request| {
            request.messages.iter().any(
                |message| matches!(message, Message::User(user) if user.text_content() == HUMAN),
            )
        })
        .expect("the real executor requested an answer with the steered human");
    assert!(answered_request >= requests_before);
    assert_eq!(
        result.result().result().text(),
        format!("executor-answer-{}", answered_request + 1)
    );
    assert!(durable.messages()[before.messages().len()..].iter().any(
        |message| matches!(message, Message::BlockAssistant(assistant)
                if assistant.to_string() == result.result().result().text())
    ));
    #[cfg(feature = "openai-live")]
    assert_eq!(
        fixture.mirrored().await,
        conversational_rows(&durable.messages()[before.messages().len()..])
    );
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unsupported_host_context_is_refused_before_runtime_admission() {
    let fixture = Fixture::new().await;
    let before = fixture.durable().await;
    let input_count = fixture.input_count().await;
    let request_count = fixture.client.requests().len();
    let specs = [
        (
            HandlingMode::Steer,
            WorkSpec::new(HUMAN, WorkOrigin::External).with_injected_context(vec![INJECTED.into()]),
        ),
        (
            HandlingMode::Steer,
            WorkSpec::new(HUMAN, WorkOrigin::External)
                .with_transient_turn_context(TurnRequestContext::new(TRANSIENT).expect("context")),
        ),
        (
            HandlingMode::Queue,
            WorkSpec::new(HUMAN, WorkOrigin::External)
                .with_transient_turn_context(TurnRequestContext::new(TRANSIENT).expect("context")),
        ),
        (
            HandlingMode::Steer,
            WorkSpec::new(HUMAN, WorkOrigin::External).with_system_prompt(SYSTEM),
        ),
    ];
    for (index, (mode, spec)) in specs.into_iter().enumerate() {
        let error = fixture
            .submit(spec, mode, delivery(&format!("refused-{index}")))
            .await
            .expect_err("host input must reject unsupported context before admission");
        if index == 0 {
            assert!(matches!(
                error,
                MobError::InjectedContextUndeliverable { member_id, .. }
                    if member_id == fixture.entry.agent_identity
            ));
        } else {
            assert!(matches!(
                error,
                MobError::UnsupportedForMode {
                    mode: crate::MobRuntimeMode::AutonomousHost,
                    ..
                }
            ));
        }
        assert_eq!(fixture.input_count().await, input_count);
        assert_eq!(fixture.client.requests().len(), request_count);
        assert_eq!(fixture.durable().await.messages(), before.messages());
    }
    #[cfg(feature = "openai-live")]
    assert!(fixture.mirrored().await.is_empty());
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unresolved_kickoff_rejects_host_steer_instead_of_falling_back_to_queue() {
    let fixture = Fixture::with_kickoff_blocked(true).await;
    fixture.client.wait_for_requests(1).await;
    let inputs_before = fixture.input_count().await;
    let error = fixture
        .submit(
            WorkSpec::new(HUMAN, WorkOrigin::External),
            HandlingMode::Steer,
            delivery("unresolved-kickoff"),
        )
        .await
        .expect_err("human Steer cannot silently become Queue");
    assert!(matches!(
        error,
        MobError::UnsupportedForMode {
            mode: crate::MobRuntimeMode::AutonomousHost,
            ..
        }
    ));
    assert_eq!(fixture.input_count().await, inputs_before);
    fixture.client.release();
    fixture
        .handle
        .wait_for_kickoff_complete(Some(WAIT))
        .await
        .expect("kickoff completes normally");
    completed(
        fixture
            .start(
                WorkSpec::new("kickoff settlement witness", WorkOrigin::Internal),
                HandlingMode::Queue,
                delivery("kickoff-settlement"),
            )
            .await
            .expect("later Queue remains usable"),
    )
    .await;
    assert_eq!(fixture.client.requests().len(), 2);
    assert!(
        !fixture
            .durable()
            .await
            .messages()
            .iter()
            .any(|message| matches!(message, Message::User(user) if user.text_content() == HUMAN))
    );
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bound_host_input_refuses_wrong_fence_and_runtime_without_retargeting() {
    let fixture = Fixture::new().await;
    let input_count = fixture.input_count().await;
    let before = fixture.durable().await;
    let wrong_fence = FenceToken::new(fixture.entry.fence_token.get() + 1);
    let error = fixture
        .handle
        .submit_host_human_input_bounded(
            fixture.entry.agent_runtime_id.clone(),
            wrong_fence,
            WorkSpec::new(HUMAN, WorkOrigin::External),
            HandlingMode::Queue,
            delivery("wrong-fence"),
            deadline(),
        )
        .await
        .expect_err("stale host lease must not be reacquired");
    assert!(matches!(
        error,
        MobError::StaleFenceToken { runtime_id, expected, actual }
            if runtime_id == fixture.entry.agent_runtime_id
                && expected == fixture.entry.fence_token && actual == wrong_fence
    ));
    let wrong_runtime = AgentRuntimeId::new(
        fixture.entry.agent_identity.clone(),
        crate::ids::Generation::new(fixture.entry.generation.get() + 1),
    );
    let error = fixture
        .handle
        .submit_host_human_input_bounded(
            wrong_runtime.clone(),
            fixture.entry.fence_token,
            WorkSpec::new(HUMAN, WorkOrigin::External),
            HandlingMode::Queue,
            delivery("wrong-runtime"),
            deadline(),
        )
        .await
        .expect_err("a runtime incarnation is not silently retargeted");
    assert!(matches!(
        error,
        MobError::StaleFenceToken { runtime_id, expected, actual }
            if runtime_id == wrong_runtime
                && expected == fixture.entry.fence_token && actual == fixture.entry.fence_token
    ));
    assert_eq!(fixture.input_count().await, input_count);
    assert_eq!(fixture.durable().await.messages(), before.messages());
    #[cfg(feature = "openai-live")]
    assert!(fixture.mirrored().await.is_empty());
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_deadline_reports_typed_fate_and_same_identity_can_be_retried() {
    let fixture = Fixture::new().await;
    let delivery = delivery("expired-observation");
    let id = interaction(&delivery);
    let spec = WorkSpec::new(HUMAN, WorkOrigin::External);
    let before = fixture.client.requests().len();
    let error = fixture
        .handle
        .submit_host_human_input_bounded(
            fixture.entry.agent_runtime_id.clone(),
            fixture.entry.fence_token,
            spec.clone(),
            HandlingMode::Queue,
            delivery.clone(),
            std::time::Instant::now(),
        )
        .await
        .expect_err("elapsed observation budget");
    assert!(matches!(
        error,
        MobError::ActorCommandTimedOut {
            command_kind: "SubmitWork",
            ..
        }
    ));
    let data = error.structured_data().expect("typed timeout data");
    assert!(data.get("executed").is_none());
    assert!(data.get("retryable").is_none());
    completed(
        fixture
            .start(spec, HandlingMode::Queue, delivery)
            .await
            .expect("retry preserves the exact delivery identity"),
    )
    .await;
    assert_human(&fixture.durable().await, HUMAN, id);
    assert_eq!(fixture.client.requests().len(), before + 1);
    fixture.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_and_peer_only_members_refuse_host_human_before_transport_delivery() {
    use super::placement_support as support;

    let service = Arc::new(MockSessionService::new());
    service.set_runtime_adapter(Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()));
    let mob_id = MobId::from(format!("host-human-remote-{}", Uuid::new_v4()));
    let mut definition = support::controlling_mob_definition(mob_id.clone());
    for profile in definition.profiles.values_mut() {
        if let Some(profile) = profile.as_inline_mut() {
            profile.model = "gpt-5.5".to_string();
        }
    }
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service)
        .with_owner_bridge_session_create_authority(SessionId::new(), false, false)
        .with_spawn_base_prompt_source(Arc::new(crate::StaticSpawnBasePromptSource(
            "Remote host-human refusal fixture".to_string(),
        )))
        .create()
        .await
        .expect("controlling mob without a non-portable client override");
    let host = support::spawn_scripted_host_peer(&format!("{mob_id}-host")).await;
    let endpoint =
        Arc::new(support::spawn_peer_comms_endpoint(&format!("{mob_id}-remote"), true, None).await);
    let responder = support::spawn_scripted_member_turn_responder(endpoint.clone());
    host.script_member_identity("remote", support::member_identity_of(&endpoint));
    host.bind_member_endpoint("remote", endpoint);
    let report = handle
        .bind_host(support::descriptor_to_bind_request(&host.descriptor))
        .await
        .expect("bind real scripted host");
    handle
        .spawn_spec(support::placed_spawn_spec(
            "worker",
            "remote",
            &report.host_id,
        ))
        .await
        .expect("spawn placed member through the actual host protocol");

    let peer = spawn_live_external_peer(&test_comms_name_for(&mob_id, "worker", "peer-only")).await;
    let member_ref = handle
        .spawn_with_binding(
            ProfileName::from("worker"),
            AgentIdentity::from("peer-only"),
            None,
            peer.binding(),
        )
        .await
        .expect("bind real peer-only member");
    assert!(matches!(
        member_ref,
        MemberRef::BackendPeer {
            session_id: None,
            ..
        }
    ));
    for name in ["remote", "peer-only"] {
        let entry = handle
            .get_member(&AgentIdentity::from(name))
            .await
            .expect("binding read")
            .expect("bound member");
        for mode in [HandlingMode::Queue, HandlingMode::Steer] {
            let error = handle
                .submit_host_human_input_bounded(
                    entry.agent_runtime_id.clone(),
                    entry.fence_token,
                    WorkSpec::new(HUMAN, WorkOrigin::Internal),
                    mode,
                    delivery(&format!("{name}-{mode:?}")),
                    deadline(),
                )
                .await
                .expect_err("host human semantics must not degrade to remote or peer notices");
            assert!(matches!(error, MobError::UnsupportedForMode { .. }));
        }
    }
    assert!(responder.received_deliveries().is_empty());
    assert!(peer.delivered_input_ids().await.is_empty());
    tokio::time::timeout(WAIT, handle.shutdown())
        .await
        .expect("remote fixture shutdown")
        .expect("remote mob stops");
    responder.shutdown_and_join().await;
    host.shutdown();
}

#[cfg(feature = "openai-live")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_live_channel_does_not_cancel_queued_host_human_input() {
    let mut fixture = Fixture::new().await;
    let before = fixture.durable().await;
    let requests_before = fixture.client.requests().len();
    fixture.client.block();
    let active = fixture
        .start(
            WorkSpec::new("work surviving channel close", WorkOrigin::Internal),
            HandlingMode::Queue,
            delivery("close-active"),
        )
        .await
        .expect("active work");
    fixture.client.wait_for_requests(requests_before + 1).await;
    let delivery = delivery("close-queued-human");
    let id = interaction(&delivery);
    let queued = fixture
        .start(
            WorkSpec::new(HUMAN, WorkOrigin::External),
            HandlingMode::Queue,
            delivery,
        )
        .await
        .expect("queued human");
    assert!(fixture.context.appends.lock().expect("context").is_empty());
    fixture.close_channel().await;
    fixture.client.release();
    completed(active).await;
    completed(queued).await;
    let durable = fixture.durable().await;
    assert_human(&durable, HUMAN, id);
    assert_eq!(fixture.client.requests().len(), requests_before + 2);
    assert_eq!(llm_identity(&durable), llm_identity(&before));
    assert!(fixture.context.appends.lock().expect("context").is_empty());
    fixture.finish().await;
}
