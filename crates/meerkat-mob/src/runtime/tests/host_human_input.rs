#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]

use super::*;
use meerkat_core::lifecycle::run_primitive::TurnRequestContext;
use meerkat_core::skills::{SkillKey, SkillName, SourceUuid};
use meerkat_core::types::TranscriptUserRole;
use meerkat_runtime::{LogicalRuntimeId, RuntimeStore};

// Shares this module's real-factory autonomous fixture.
#[path = "delivery_terminal_wait.rs"]
mod delivery_terminal_wait;

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
    /// How many of the next requests answer at once with a `peers` tool
    /// call, released or not ([`Self::call_a_tool_next`]).
    tool_rounds: Arc<std::sync::atomic::AtomicUsize>,
}

impl ScriptedClient {
    fn new(released: bool) -> Self {
        let (released, _) = tokio::sync::watch::channel(released);
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            released,
            tool_rounds: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Answer the next request at once with a `peers` tool call, released or
    /// not: the run commits that round and asks the model again, so a run
    /// blocked on that next request has a committed round to continue from.
    fn call_a_tool_next(&self) {
        self.tool_rounds
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        let tool_round = self
            .tool_rounds
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |rounds| rounds.checked_sub(1),
            )
            .is_ok();
        let mut released = self.released.subscribe();
        Box::pin(async_stream::try_stream! {
            if tool_round {
                yield meerkat_client::LlmEvent::ToolCallComplete {
                    id: format!("round-call-{sequence}"),
                    name: "peers".to_string(),
                    args: serde_json::json!({}),
                    meta: None,
                };
            } else {
                while !*released.borrow_and_update() {
                    if released.changed().await.is_err() {
                        break;
                    }
                }
                yield meerkat_client::LlmEvent::TextDelta {
                    delta: format!("executor-answer-{sequence}"),
                    meta: None,
                };
            }
            yield meerkat_client::LlmEvent::UsageUpdate {
                usage: meerkat_core::TurnUsage::host_declared(
                    Provider::OpenAI,
                    &request.model,
                    Usage::default(),
                ),
            };
            let stop = if tool_round {
                meerkat_core::StopReason::ToolUse
            } else {
                meerkat_core::StopReason::EndTurn
            };
            yield meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::success(stop),
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
            let adapter = fixture
                .service
                .acquire_runtime_adapter(None)
                .expect("acquire runtime authority")
                .expect("runtime owner");
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
        Self::build(blocked, &[]).await
    }

    /// Settled fixture whose member agent has a native skill engine over one
    /// configured filesystem repository per `(source, skill, body)` entry.
    async fn with_skills(skills: &[(&SourceUuid, &SkillName, &str)]) -> Self {
        let fixture = Self::build(false, skills).await;
        fixture
            .handle
            .wait_for_kickoff_complete(Some(WAIT))
            .await
            .expect("real autonomous kickoff commits");
        fixture
    }

    async fn build(blocked: bool, skills: &[(&SourceUuid, &SkillName, &str)]) -> Self {
        // The system temp dir, not the source tree: a run killed before the
        // TempDir drops must not leave fixture state in the worktree.
        let root = tempfile::Builder::new()
            .prefix("meerkat-host-human-input-")
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
        for (index, (source_uuid, name, body)) in skills.iter().enumerate() {
            let repository = root_path.join(format!("skills-{index}"));
            let skill_dir = repository.join(name.as_str());
            std::fs::create_dir_all(&skill_dir).expect("skill fixture directory");
            std::fs::write(
                skill_dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: fixture skill\n---\n{body}\n"),
            )
            .expect("skill fixture document");
            config
                .skills
                .repositories
                .push(meerkat_core::skills_config::SkillRepositoryConfig {
                    name: format!("fixture-skills-{index}"),
                    source_uuid: (*source_uuid).clone(),
                    transport: meerkat_core::skills_config::SkillRepoTransport::Filesystem {
                        path: repository.to_string_lossy().into_owned(),
                    },
                });
        }
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

    async fn start_with_options(
        &self,
        spec: WorkSpec,
        options: MemberTurnOptions,
        delivery: MobDeliveryIdentity,
    ) -> Result<WorkTurnHandle, MobError> {
        self.handle
            .start_host_human_input_with_options_bounded(
                self.entry.agent_runtime_id.clone(),
                self.entry.fence_token,
                spec,
                HandlingMode::Queue,
                options,
                delivery,
                deadline(),
            )
            .await
    }

    async fn submit_with_options(
        &self,
        spec: WorkSpec,
        options: MemberTurnOptions,
        delivery: MobDeliveryIdentity,
    ) -> Result<WorkDeliveryReceipt, MobError> {
        self.handle
            .submit_host_human_input_with_options_bounded(
                self.entry.agent_runtime_id.clone(),
                self.entry.fence_token,
                spec,
                HandlingMode::Queue,
                options,
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
        let scope = self
            .handle
            .capture_member_delivery_scope(&self.entry.agent_identity)
            .await
            .expect("capture the member's delivery scope");
        self.handle
            .submit_work_with_mode_and_delivery_identity_bounded(
                &scope,
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
                .acquire_runtime_adapter(None)
                .expect("acquire runtime authority")
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

    /// Close the live context channel the fixture opened (with live
    /// support; nothing is open otherwise). The fixture composes no
    /// transport-neutral live cleanup host, so disposing of the member
    /// (retire, shutdown) while the channel is open is refused as a
    /// composition error: close it first.
    async fn close_channel(&mut self) {
        #[cfg(feature = "openai-live")]
        if let Some(binding) = self.binding.take() {
            self.service
                .acquire_runtime_adapter(None)
                .expect("acquire runtime authority")
                .expect("runtime owner")
                .__test_close_live_context_channel(&binding)
                .await
                .expect("close fixture channel without cancelling runtime work");
        }
    }

    async fn finish(mut self) {
        self.client.release();
        self.close_channel().await;
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
    let scope = fixture
        .handle
        .capture_member_delivery_scope(&fixture.entry.agent_identity)
        .await
        .expect("capture the member's delivery scope");
    fixture
        .handle
        .submit_work_with_mode_and_delivery_identity_bounded(
            &scope,
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
                .acquire_runtime_adapter(None)
                .expect("acquire runtime authority")
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

/// A host-human delivery carries selected skills to the exact fenced target.
/// Two sources host a skill with the same name; the source-pinned key picks
/// exactly one. The member resolves it natively (typed `SkillsResolved`,
/// durable `SkillContext`, the selected body and only that body in the
/// provider request). A retry after a lost response (same delivery identity,
/// same selection) returns the original admission with no second provider
/// call, and a retry under the same identity with a changed selection is a
/// typed idempotency conflict.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_human_selected_skills_resolve_natively_and_replay_exactly() {
    use futures::StreamExt;
    use meerkat_core::AgentEvent;

    let chosen_source =
        SourceUuid::parse("5b0c7c2e-4c61-4f2d-9d0e-2f5a8c6b1e11").expect("fixture source uuid");
    let other_source =
        SourceUuid::parse("9e4d2a10-7b3c-4c8e-a1f5-6d2b0c9e8f77").expect("fixture source uuid");
    let name = SkillName::parse("house-style").expect("fixture skill name");
    let chosen_body = "Answer in the chosen house style: numbered, terse, no hedging.";
    let other_body = "Answer in the other house style: long prose with caveats.";
    let fixture = Fixture::with_skills(&[
        (&chosen_source, &name, chosen_body),
        (&other_source, &name, other_body),
    ])
    .await;
    let chosen = SkillKey::new(chosen_source, name.clone());
    let other = SkillKey::new(other_source, name);
    let options = MemberTurnOptions::new().with_skill_references(vec![chosen.clone()]);
    let delivery = delivery("selected-skill");
    let id = interaction(&delivery);
    let spec = WorkSpec::new(HUMAN, WorkOrigin::External);
    let mut events = fixture
        .handle
        .subscribe_agent_events(&fixture.entry.agent_identity)
        .await
        .expect("member agent events");
    let requests_before = fixture.client.requests().len();

    let receipt = completed(
        fixture
            .start_with_options(spec.clone(), options.clone(), delivery.clone())
            .await
            .expect("selected-skill host input admitted"),
    )
    .await;

    let mut resolved = Vec::new();
    tokio::time::timeout(WAIT, async {
        while let Some(envelope) = events.next().await {
            match envelope.payload {
                AgentEvent::SkillsResolved { skills, .. } => resolved.push(skills),
                AgentEvent::SkillResolutionFailed { reason, .. } => {
                    panic!("selected skill must resolve, got {reason}")
                }
                // The member's own spawn turn may still be draining from
                // this subscription; the selected turn ends at the first
                // completion after its activation.
                AgentEvent::RunCompleted { .. } if !resolved.is_empty() => break,
                _ => {}
            }
        }
    })
    .await
    .expect("the selected turn's events arrive");
    assert_eq!(
        resolved,
        vec![vec![chosen.clone()]],
        "one native activation"
    );

    let durable = fixture.durable().await;
    let skill_rows = durable
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User(user) if user.identity.interaction_id == Some(id) => Some(user),
            _ => None,
        })
        .flat_map(|user| user.content.iter())
        .filter_map(|block| match block {
            meerkat_core::types::ContentBlock::SkillContext { skill_key, text } => {
                Some((skill_key.clone(), text.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(skill_rows.len(), 1, "exactly one durable SkillContext");
    assert_eq!(skill_rows[0].0, chosen);
    assert!(
        skill_rows[0].1.contains(chosen_body),
        "native rendered body"
    );
    let requests = fixture.client.requests();
    assert_eq!(requests.len(), requests_before + 1);
    let request_json =
        serde_json::to_string(&requests[requests_before].messages).expect("request serializes");
    assert!(
        request_json.contains(chosen_body),
        "selected body reaches the provider"
    );
    assert!(
        !request_json.contains(other_body),
        "the same-name skill is not selected"
    );
    let inputs = fixture.input_count().await;

    // The response is lost; the host replays the same delivery.
    let replay = fixture
        .submit_with_options(spec.clone(), options, delivery.clone())
        .await
        .expect("same identity and selection replays the original admission");
    assert_eq!(replay.work_ref, receipt.work_ref);
    assert_eq!(replay.runtime_id, receipt.runtime_id);
    assert_eq!(fixture.input_count().await, inputs);
    assert_eq!(fixture.client.requests().len(), requests_before + 1);

    let error = fixture
        .submit_with_options(
            spec,
            MemberTurnOptions::new().with_skill_references(vec![other]),
            delivery,
        )
        .await
        .expect_err("a changed selection under the same identity is not a replay");
    assert!(
        matches!(error, MobError::WorkInputIdempotencyConflict { .. }),
        "typed conflict, got {error:?}"
    );
    assert_eq!(fixture.input_count().await, inputs);
    assert_eq!(fixture.client.requests().len(), requests_before + 1);
    fixture.finish().await;
}

/// An empty skill selection is no selection (it keeps the metadata-less
/// replay identity), and an options interaction id that differs from the
/// delivery correlation is a typed refusal before admission.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_human_options_normalize_empty_selection_and_refuse_foreign_interaction() {
    let fixture = Fixture::new().await;
    let spec = WorkSpec::new(HUMAN, WorkOrigin::External);
    let empty = delivery("empty-selection");
    let first = fixture
        .submit_with_options(
            spec.clone(),
            MemberTurnOptions::new().with_skill_references(Vec::new()),
            empty.clone(),
        )
        .await
        .expect("empty selection admitted");
    let replay = fixture
        .submit(spec.clone(), HandlingMode::Queue, empty)
        .await
        .expect("an empty selection replays as no selection");
    assert_eq!(replay.work_ref, first.work_ref);

    let inputs = fixture.input_count().await;
    let foreign = delivery("foreign-interaction");
    let error = fixture
        .submit_with_options(
            spec,
            MemberTurnOptions::new().with_interaction_id(InteractionId(Uuid::new_v4())),
            foreign.clone(),
        )
        .await
        .expect_err("a foreign interaction id is not the delivery's interaction");
    assert!(
        matches!(
            &error,
            MobError::DeliveryInteractionConflict { correlation_id, .. }
                if *correlation_id == foreign.correlation_id
        ),
        "typed conflict, got {error:?}"
    );
    assert_eq!(fixture.input_count().await, inputs);
    fixture.finish().await;
}

const QUEUED_HUMAN: &str = "A second question, queued behind the running turn.";

/// One host process over the stop fixture's durable files: the realm's
/// SQLite session and runtime stores, the mob's SQLite storage and the
/// session event log.
struct StopHost {
    handle: MobHandle,
    runtime_store: Arc<dyn RuntimeStore>,
    adapter: Arc<meerkat_runtime::MeerkatMachine>,
    /// The process's session service, for its members' comms runtimes.
    service: Arc<Service>,
}

/// Open a host process over `root`: a new mob (`label`) or, without a label,
/// the same mob resumed from its durable history.
async fn open_stop_host(
    root: &std::path::Path,
    client: &ScriptedClient,
    label: Option<&str>,
) -> StopHost {
    open_stop_host_holding(root, client, label, &[]).await
}

/// [`open_stop_host`], with the run starts of the `held` members the resume
/// restores held by their host (`ToolsNotPublished`): they register held and
/// start no run.
async fn open_stop_host_holding(
    root: &std::path::Path,
    client: &ScriptedClient,
    label: Option<&str>,
    held: &[AgentIdentity],
) -> StopHost {
    open_stop_host_composed(root, client, label, held, false).await
}

/// [`open_stop_host_holding`], and with `llm_reconfigure` the session-LLM
/// reconfigure host a product surface installs on the service's runtime
/// before the mob opens, so a member's input can carry a per-turn model
/// override.
async fn open_stop_host_composed(
    root: &std::path::Path,
    client: &ScriptedClient,
    label: Option<&str>,
    held: &[AgentIdentity],
    llm_reconfigure: bool,
) -> StopHost {
    let factory = meerkat::AgentFactory::new(root.join("factory-store"))
        .user_config_root(root.join("config"))
        .runtime_root(root.join("runtime"))
        .project_root(root.join("project"))
        .context_root(root.join("context"))
        .builtins(false)
        .comms(true);
    let mut config = meerkat::Config::default();
    config.agent.model = "gpt-5.5".to_string();
    config.compaction.auto_compact_threshold = 1_000_000;
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, config);
    builder.default_llm_client = Some(Arc::new(client.clone()));
    let db = root.join("realm.db");
    let session_store =
        Arc::new(meerkat_store::SqliteSessionStore::open(&db).expect("session store"));
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    let runtime_store: Arc<dyn RuntimeStore> = Arc::new(
        meerkat_runtime::SqliteRuntimeStore::new_head_canonical(&db)
            .expect("co-tenant canonical runtime store"),
    );
    let llm_reconfigure_blueprint = llm_reconfigure.then(|| {
        meerkat::session_runtime::llm_reconfigure::SessionRuntimeLlmReconfigureHostBlueprint::new(
            &builder,
            root.join("runtime").join("llm-reconfigure-state.json"),
            Arc::new(std::sync::RwLock::new(builder.default_llm_client.clone())),
        )
    });
    let service = Arc::new(
        Service::new(
            builder,
            16,
            session_store,
            runtime_store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .with_event_projection(
            Arc::new(meerkat_session::event_store::FileEventStore::new(
                root.join("events"),
            )),
            Arc::new(meerkat_session::projector::SessionProjector::new(
                root.join("projection"),
            )),
        ),
    );
    if let Some(blueprint) = llm_reconfigure_blueprint {
        let runtime = service
            .canonical_runtime_adapter()
            .expect("the session service's runtime owner");
        let reconfigure_service: Arc<
            dyn meerkat::session_runtime::llm_reconfigure::SessionRuntimeLlmReconfigureService,
        > = service.clone();
        blueprint.install(&runtime, reconfigure_service);
    }
    let storage = MobStorage::persistent(root.join("mob.db")).expect("persistent mob storage");
    let handle = match label {
        Some(label) => {
            let mut definition = with_unique_mob_id(sample_definition(), label);
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
            MobBuilder::new(definition, storage)
                .with_session_service(service.clone())
                .with_default_llm_client(Arc::new(client.clone()))
                .create()
                .await
                .expect("create real autonomous mob")
        }
        None => tokio::time::timeout(
            WAIT,
            MobBuilder::for_resume(storage)
                .with_session_service(service.clone())
                .with_default_llm_client(Arc::new(client.clone()))
                .notify_orchestrator_on_resume(false)
                .hold_restored_member_run_starts(
                    crate::HostRunStartHoldReason::ToolsNotPublished,
                    held.iter().cloned(),
                )
                .resume(),
        )
        .await
        .expect("mob resume finishes")
        .expect("resume the same mob from its durable history"),
    };
    let adapter = service
        .acquire_runtime_adapter(None)
        .expect("acquire runtime authority")
        .expect("runtime owner");
    StopHost {
        handle,
        runtime_store,
        adapter,
        service,
    }
}

/// A fresh fixture root in the system temp dir.
fn stop_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::Builder::new()
        .prefix("meerkat-host-human-stop-")
        .tempdir()
        .expect("fixture storage in the system temp dir");
    let root_path = root.path().canonicalize().expect("absolute fixture root");
    for directory in ["config", "runtime", "project", "context"] {
        std::fs::create_dir_all(root_path.join(directory)).expect("fixture root");
    }
    (root, root_path)
}

/// Spawn the stop fixture's autonomous member and settle its kickoff.
async fn spawn_stop_member(host: &StopHost) -> (AgentIdentity, SessionId, RosterEntry) {
    // Unique per test: identity-keyed gates are process-wide.
    let identity = AgentIdentity::from(format!("host-human-stop-{}", Uuid::new_v4().simple()));
    let session_id = host
        .handle
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
    host.handle
        .wait_for_kickoff_complete(Some(WAIT))
        .await
        .expect("real autonomous kickoff commits");
    let entry = host
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("member binding");
    (identity, session_id, entry)
}

/// The Core input a delivery admitted. The mob admission returns no input
/// id, so the row is read once by the delivery's idempotency key and then
/// addressed by its own input id.
async fn admitted_input_id(
    runtime_store: &Arc<dyn RuntimeStore>,
    session_id: &SessionId,
    key: &MobDeliveryIdentity,
) -> meerkat_core::lifecycle::InputId {
    runtime_store
        .load_input_state_by_idempotency_key(
            &LogicalRuntimeId::for_session(session_id),
            &meerkat_runtime::identifiers::IdempotencyKey::new(key.idempotency_key.clone()),
        )
        .await
        .expect("exact runtime-owned key lookup")
        .expect("the delivery has a canonical input")
        .state()
        .state
        .input_id
        .clone()
}

/// The input's stored row by its input id, the exact digest of its stored
/// bytes, and whether it carries an interaction-terminal carrier or an
/// unfinished terminal-owner entry.
struct StopRow {
    stored: meerkat_runtime::input_state::StoredInputState,
    digest: String,
    has_outbox: bool,
    pending_terminal_owner: bool,
    /// The row's wire image, for facts a typed read cannot name yet.
    image: serde_json::Value,
}

async fn stop_row(
    runtime_store: &Arc<dyn RuntimeStore>,
    session_id: &SessionId,
    key: &MobDeliveryIdentity,
    input_id: &meerkat_core::lifecycle::InputId,
) -> StopRow {
    let runtime_id = LogicalRuntimeId::for_session(session_id);
    let stored = runtime_store
        .load_input_state(&runtime_id, input_id)
        .await
        .expect("runtime-owned input read")
        .expect("the input keeps its durable row");
    let exact = runtime_store
        .load_input_state_by_idempotency_key(
            &runtime_id,
            &meerkat_runtime::identifiers::IdempotencyKey::new(key.idempotency_key.clone()),
        )
        .await
        .expect("exact runtime-owned key lookup")
        .expect("the delivery still names its input");
    assert_eq!(&exact.state().state.input_id, input_id);
    let image = serde_json::to_value(&stored).expect("the row's stored wire image");
    let mut pending_terminal_owner = false;
    let mut after = None;
    loop {
        let page = runtime_store
            .load_pending_terminal_owner_ids_page(
                &runtime_id,
                after.as_ref(),
                meerkat_runtime::store::MAX_PENDING_TERMINAL_OWNER_PAGE,
            )
            .await
            .expect("unfinished terminal owners");
        pending_terminal_owner |= page.contains(input_id);
        if page.len() < meerkat_runtime::store::MAX_PENDING_TERMINAL_OWNER_PAGE {
            break;
        }
        after = page.last().cloned();
    }
    StopRow {
        has_outbox: !image["interaction_terminal_outbox"].is_null(),
        image: image.clone(),
        digest: exact.exact_row_digest().to_owned(),
        stored,
        pending_terminal_owner,
    }
}

/// The session's durable event log, read the way a fresh process reads it.
async fn stop_events(root: &std::path::Path, session_id: &SessionId) -> Vec<AgentEvent> {
    use meerkat_session::event_store::EventStore as _;
    meerkat_session::event_store::FileEventStore::new(root.join("events"))
        .read_from(session_id, 0)
        .await
        .expect("read the durable event log")
        .into_iter()
        .map(|stored| stored.event)
        .collect()
}

/// Every run the log started for `interaction_id`.
fn runs_started_for(
    events: &[AgentEvent],
    interaction_id: InteractionId,
) -> Vec<meerkat_core::lifecycle::RunId> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::RunStarted { identity, .. }
                if identity.interaction_id == Some(interaction_id) =>
            {
                identity.run_id.clone()
            }
            _ => None,
        })
        .collect()
}

/// Every failure the log recorded for `run_id`.
fn run_failures(
    events: &[AgentEvent],
    run_id: &meerkat_core::lifecycle::RunId,
) -> Vec<meerkat_core::event::AgentErrorReport> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::RunFailed {
                identity,
                error_report,
                ..
            } if identity.run_id.as_ref() == Some(run_id) => Some(error_report.clone()),
            _ => None,
        })
        .collect()
}

/// Every interaction failure in the session's log, under any interaction id.
/// No input in these rows is Core-directed, so a stop publishes none.
fn interaction_failed_events(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, AgentEvent::InteractionFailed { .. }))
        .count()
}

/// Events in the log that name `interaction_id` anywhere in their wire image.
fn events_naming(events: &[AgentEvent], interaction_id: InteractionId) -> usize {
    let id = interaction_id.0.to_string();
    events
        .iter()
        .filter(|event| {
            serde_json::to_string(event)
                .expect("event wire image")
                .contains(&id)
        })
        .count()
}

fn requests_carrying(requests: &[meerkat_client::LlmRequest], text: &str) -> usize {
    requests
        .iter()
        .filter(|request| {
            request.messages.iter().any(
                |message| matches!(message, Message::User(user) if user.text_content().contains(text)),
            )
        })
        .count()
}

/// How a stopped input's bounded waiter ended.
#[derive(Clone, Debug, PartialEq)]
enum StopAnswer {
    Succeeded,
    Cancelled,
    RuntimeTerminated {
        reason: String,
        kind: meerkat_core::TurnTerminalCauseKind,
        detail: Option<String>,
    },
    Other(String),
    Refused(String),
    Unresolved,
    RequeuedAcrossStop(meerkat_core::lifecycle::InputId),
    Abandoned(String),
    RequeueExpired(meerkat_core::lifecycle::InputId),
}

async fn stop_answer(turn: WorkTurnHandle, session_id: &SessionId) -> StopAnswer {
    let Ok(result) = tokio::time::timeout(
        WAIT,
        turn.wait_bounded(BoundedResultSpec::new("stopped human", 1024).expect("bound")),
    )
    .await
    else {
        return StopAnswer::Unresolved;
    };
    let Err(error) = result else {
        return StopAnswer::Succeeded;
    };
    match error.failure() {
        BoundedTurnFailure::Cancelled { session_id: failed } if failed == session_id => {
            StopAnswer::Cancelled
        }
        BoundedTurnFailure::RuntimeTerminated {
            session_id: failed,
            reason,
            error,
        } if failed == session_id => StopAnswer::RuntimeTerminated {
            reason: reason.clone(),
            kind: error.kind,
            detail: error.detail.clone(),
        },
        BoundedTurnFailure::RequeuedAcrossStop {
            session_id: failed,
            input_id,
        } if failed == session_id => StopAnswer::RequeuedAcrossStop(input_id.clone()),
        BoundedTurnFailure::Abandoned {
            session_id: failed,
            reason,
            ..
        } if failed == session_id => StopAnswer::Abandoned(reason.clone()),
        BoundedTurnFailure::RequeueExpired {
            session_id: failed,
            input_id,
        } if failed == session_id => StopAnswer::RequeueExpired(input_id.clone()),
        other => StopAnswer::Other(format!("{other:?}")),
    }
}

/// What the stopping process leaves for one input: its waiter's answer, its
/// row, and its runs and terminal events in the log.
#[derive(Debug, PartialEq)]
struct AtStop {
    waiter: StopAnswer,
    phase: meerkat_runtime::input_state::InputLifecycleState,
    outcome: Option<meerkat_runtime::input_state::InputTerminalOutcome>,
    attempt_count: u32,
    last_run_id: Option<meerkat_core::lifecycle::RunId>,
    payload_kept: bool,
    replay_identity_kept: bool,
    outbox: bool,
    pending_terminal_owner: bool,
    runs_started: usize,
    run_failures: Vec<(meerkat_core::event::AgentErrorClass, String)>,
    interaction_failed_events: usize,
}

fn observe_at_stop(
    waiter: StopAnswer,
    row: &StopRow,
    events: &[AgentEvent],
    interaction_id: InteractionId,
) -> AtStop {
    let runs = runs_started_for(events, interaction_id);
    AtStop {
        waiter,
        phase: row.stored.seed.phase,
        outcome: row.stored.seed.terminal_outcome.clone(),
        attempt_count: row.stored.seed.attempt_count,
        last_run_id: row.stored.seed.last_run_id.clone(),
        payload_kept: row.stored.state.persisted_input.is_some(),
        replay_identity_kept: row.stored.state.prompt_replay_identity.is_some(),
        outbox: row.has_outbox,
        pending_terminal_owner: row.pending_terminal_owner,
        runs_started: runs.len(),
        run_failures: runs
            .iter()
            .flat_map(|run| run_failures(events, run))
            .map(|report| (report.class, report.message))
            .collect(),
        interaction_failed_events: interaction_failed_events(events),
    }
}

/// What the restarted process does with a stopped input: recovery, a
/// same-key replay, and the log across both processes.
#[derive(Debug, PartialEq)]
struct AfterRestart {
    recovery_count: u32,
    row_unchanged_by_recovery: bool,
    pending_terminal_owner: bool,
    replay: StopAnswer,
    model_requests: usize,
    row_unchanged_by_replay: bool,
    runs_started: Vec<meerkat_core::lifecycle::RunId>,
    run_failures: usize,
    interaction_failed_events: usize,
}

/// Recovery's view of the row, then a same-key replay of `spec` in the
/// restarted process, then the row and the log after it.
async fn observe_after_restart(
    root: &std::path::Path,
    host: &StopHost,
    client: &ScriptedClient,
    at_exit: &StopRow,
    input: (
        &MobDeliveryIdentity,
        &meerkat_core::lifecycle::InputId,
        WorkSpec,
    ),
    identity: &AgentIdentity,
    session_id: &SessionId,
) -> AfterRestart {
    let (key, input_id, spec) = input;
    let after_recovery = stop_row(&host.runtime_store, session_id, key, input_id).await;
    let entry = host
        .handle
        .get_member(identity)
        .await
        .expect("roster read")
        .expect("the member survives the restart");
    let replay = match host
        .handle
        .start_host_human_input_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            spec,
            HandlingMode::Queue,
            key.clone(),
            deadline(),
        )
        .await
    {
        Ok(replay) => stop_answer(replay, session_id).await,
        Err(error) => StopAnswer::Refused(error.to_string()),
    };
    let after_replay = stop_row(&host.runtime_store, session_id, key, input_id).await;
    let events = stop_events(root, session_id).await;
    let runs = runs_started_for(&events, interaction(key));
    AfterRestart {
        recovery_count: after_recovery.stored.state.recovery_count,
        row_unchanged_by_recovery: after_recovery.digest == at_exit.digest,
        pending_terminal_owner: after_recovery.pending_terminal_owner,
        replay,
        model_requests: client.requests().len(),
        row_unchanged_by_replay: after_replay.digest == after_recovery.digest,
        run_failures: runs
            .iter()
            .map(|run| run_failures(&events, run).len())
            .sum(),
        runs_started: runs,
        interaction_failed_events: interaction_failed_events(&events),
    }
}

/// A Queue-mode host human input whose own turn is in flight (its model
/// call held) when the host stops gracefully with an explicit
/// `QueuedInputsOnShutdown::Stop`, then a restart over the same durable
/// files.
///
/// The stop cancels the in-flight run: the input is abandoned as cancelled
/// and its payload retired in the same lifecycle commit, and the run fails
/// once as cancelled. A host human input is not a Core-directed input, so it
/// has no interaction-terminal carrier and no interaction terminal event.
/// The restart leaves the terminal row untouched, runs nothing, and a
/// same-key replay answers the failed terminal. Both processes are observed
/// before one comparison, so a failure shows the whole shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_human_mid_turn_at_graceful_stop_fails_once_after_restart_and_keeps_row() {
    use meerkat_core::event::AgentErrorClass;
    use meerkat_runtime::input_state::{
        InputAbandonReason, InputLifecycleState, InputTerminalOutcome,
    };

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("host-human-stop")).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let key = delivery("graceful-stop-in-flight");
    let interaction_id = interaction(&key);
    let spec = WorkSpec::new(HUMAN, WorkOrigin::External);
    let turn = host
        .handle
        .start_host_human_input_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            spec.clone(),
            HandlingMode::Queue,
            key.clone(),
            deadline(),
        )
        .await
        .expect("queued human admission");
    client.wait_for_requests(requests_before + 1).await;
    assert_eq!(
        requests_carrying(&client.requests()[requests_before..], HUMAN),
        1,
        "the held model call is the human input's own turn"
    );
    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;
    assert_ne!(
        input_id.0, interaction_id.0,
        "a host human input is not Core-directed: its input id is not the interaction id"
    );
    let in_flight = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    assert!(in_flight.stored.seed.terminal_outcome.is_none());
    assert!(in_flight.stored.state.persisted_input.is_some());

    tokio::time::timeout(WAIT, host.handle.shutdown_with_report(stop_options()))
        .await
        .expect("the graceful stop finishes")
        .expect("graceful stop");
    let waiter = stop_answer(turn, &session_id).await;
    let at_exit = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    let events = stop_events(&root_path, &session_id).await;
    let at_stop = observe_at_stop(waiter, &at_exit, &events, interaction_id);
    // The input's own run as the log names it; `runs_started: 1` pins that
    // it is the only one.
    let its_run = runs_started_for(&events, interaction_id).first().cloned();
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    // Restart: a fresh host over the same files, with a model that records
    // every request it gets.
    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    let after_restart = observe_after_restart(
        &root_path,
        &restarted,
        &restarted_client,
        &at_exit,
        (&key, &input_id, spec),
        &identity,
        &session_id,
    )
    .await;
    assert_eq!(
        (at_stop, after_restart),
        (
            AtStop {
                waiter: StopAnswer::Cancelled,
                phase: InputLifecycleState::Abandoned,
                outcome: Some(InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::Cancelled,
                }),
                attempt_count: 1,
                last_run_id: its_run.clone(),
                payload_kept: false,
                replay_identity_kept: true,
                outbox: false,
                pending_terminal_owner: false,
                runs_started: 1,
                run_failures: vec![(AgentErrorClass::Cancelled, "Run was cancelled".to_owned())],
                interaction_failed_events: 0,
            },
            AfterRestart {
                recovery_count: at_exit.stored.state.recovery_count,
                row_unchanged_by_recovery: true,
                pending_terminal_owner: false,
                replay: StopAnswer::Cancelled,
                model_requests: 0,
                row_unchanged_by_replay: true,
                runs_started: its_run.into_iter().collect(),
                run_failures: 1,
                interaction_failed_events: 0,
            },
        ),
        "the stop cancels the in-flight run once and retires the payload with the \
         terminal; the restart keeps that terminal and the replay answers it without a run"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// A Queue-mode host human input still queued behind a running turn when
/// the host stops gracefully with an explicit `QueuedInputsOnShutdown::Stop`,
/// then a restart over the same durable files.
///
/// The stop terminalizes it through the runtime stop: abandoned as stopped
/// with its payload retired, and its waiter gets the runtime termination.
/// The stop runs while the member's runtime registration drains, so the
/// termination reason is the unregistered one. The input never ran, so the
/// log has no run for it, and as a non-directed input it has no
/// interaction-terminal carrier or event. The restart leaves the row
/// untouched, runs nothing, and a same-key replay answers the same
/// termination.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_human_behind_a_running_turn_at_graceful_stop_is_terminated_once_after_restart_and_keeps_row()
 {
    use meerkat_runtime::input_state::{
        InputAbandonReason, InputLifecycleState, InputTerminalOutcome,
    };

    // Generated Draining classification: shutdown stops inside the unregister drain.
    const UNREGISTERED: &str = "runtime session unregistered";
    let terminated = StopAnswer::RuntimeTerminated {
        reason: UNREGISTERED.to_owned(),
        kind: meerkat_core::TurnTerminalCauseKind::FatalFailure,
        detail: Some(UNREGISTERED.to_owned()),
    };
    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("host-human-stop-queued")).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let running = host
        .handle
        .start_host_human_input_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            WorkSpec::new(HUMAN, WorkOrigin::External),
            HandlingMode::Queue,
            delivery("graceful-stop-running"),
            deadline(),
        )
        .await
        .expect("running human admission");
    client.wait_for_requests(requests_before + 1).await;
    let key = delivery("graceful-stop-queued-behind");
    let interaction_id = interaction(&key);
    let spec = WorkSpec::new(QUEUED_HUMAN, WorkOrigin::External);
    let queued = host
        .handle
        .start_host_human_input_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            spec.clone(),
            HandlingMode::Queue,
            key.clone(),
            deadline(),
        )
        .await
        .expect("queued human admission");
    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;
    assert_ne!(input_id.0, interaction_id.0);
    let before_stop = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    assert_eq!(before_stop.stored.seed.phase, InputLifecycleState::Queued);

    tokio::time::timeout(WAIT, host.handle.shutdown_with_report(stop_options()))
        .await
        .expect("the graceful stop finishes")
        .expect("graceful stop");
    let waiter = stop_answer(queued, &session_id).await;
    assert_eq!(
        stop_answer(running, &session_id).await,
        StopAnswer::Cancelled,
        "the running turn ahead of it is cancelled"
    );
    let at_exit = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    let events = stop_events(&root_path, &session_id).await;
    let at_stop = observe_at_stop(waiter, &at_exit, &events, interaction_id);
    let naming_at_stop = events_naming(&events, interaction_id);
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    let after_restart = observe_after_restart(
        &root_path,
        &restarted,
        &restarted_client,
        &at_exit,
        (&key, &input_id, spec),
        &identity,
        &session_id,
    )
    .await;
    let naming_after_restart =
        events_naming(&stop_events(&root_path, &session_id).await, interaction_id);
    assert_eq!(
        (
            at_stop,
            after_restart,
            (naming_at_stop, naming_after_restart),
        ),
        (
            AtStop {
                waiter: terminated.clone(),
                phase: InputLifecycleState::Abandoned,
                outcome: Some(InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::Stopped,
                }),
                attempt_count: 0,
                last_run_id: None,
                payload_kept: false,
                replay_identity_kept: true,
                outbox: false,
                pending_terminal_owner: false,
                runs_started: 0,
                run_failures: Vec::new(),
                interaction_failed_events: 0,
            },
            AfterRestart {
                recovery_count: at_exit.stored.state.recovery_count,
                row_unchanged_by_recovery: true,
                pending_terminal_owner: false,
                replay: terminated,
                model_requests: 0,
                row_unchanged_by_replay: true,
                runs_started: Vec::new(),
                run_failures: 0,
                interaction_failed_events: 0,
            },
            (0, 0),
        ),
        "the stop terminates the queued input without a run and retires its payload; \
         the restart keeps that terminal and the replay answers it without a run"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

const QUEUED_SECOND: &str = "A third question, queued behind the second.";
const AFTER_RESTART: &str = "A question sent after the restart.";

fn requeue_options(max_age: Duration) -> crate::ShutdownOptions {
    crate::ShutdownOptions::default().with_queued_inputs(crate::QueuedInputsOnShutdown::Requeue(
        crate::RequeueMaxAge::new(max_age).expect("an accepted requeue bound"),
    ))
}

/// Shutdown options that end every member's work as stopped, as the default
/// did before 0.9.5.
fn stop_options() -> crate::ShutdownOptions {
    crate::ShutdownOptions::default().with_queued_inputs(crate::QueuedInputsOnShutdown::Stop)
}

/// Start a Queue- or Steer-mode host human input on the stop fixture's member.
async fn start_stop_input(
    host: &StopHost,
    entry: &RosterEntry,
    spec: WorkSpec,
    mode: HandlingMode,
    key: &MobDeliveryIdentity,
) -> Result<WorkTurnHandle, MobError> {
    host.handle
        .start_host_human_input_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            spec,
            mode,
            key.clone(),
            deadline(),
        )
        .await
}

/// A requeued input as the stopping process leaves it.
#[derive(Debug, PartialEq)]
struct RequeuedAtStop {
    waiter: StopAnswer,
    phase: meerkat_runtime::input_state::InputLifecycleState,
    outcome: serde_json::Value,
    payload_kept: bool,
    replay_identity_kept: bool,
    requeue_mark: bool,
    outbox: bool,
    terminal_completion: bool,
    pending_terminal_owner: bool,
    runs_started: usize,
}

fn observe_requeued_at_stop(
    waiter: StopAnswer,
    row: &StopRow,
    events: &[AgentEvent],
    interaction_id: InteractionId,
) -> RequeuedAtStop {
    RequeuedAtStop {
        waiter,
        phase: row.stored.seed.phase,
        outcome: serde_json::to_value(&row.stored.seed.terminal_outcome)
            .expect("terminal outcome wire image"),
        payload_kept: row.stored.state.persisted_input.is_some(),
        replay_identity_kept: row.stored.state.prompt_replay_identity.is_some(),
        requeue_mark: !row.image["stop_requeue"].is_null(),
        outbox: row.has_outbox,
        terminal_completion: !row.image["terminal_completion"].is_null(),
        pending_terminal_owner: row.pending_terminal_owner,
        runs_started: runs_started_for(events, interaction_id).len(),
    }
}

/// The restarted process's handling of a requeued input: a same-key replay,
/// then the row, the model requests and the log.
#[derive(Debug, PartialEq)]
struct RequeuedAfterRestart {
    replay: StopAnswer,
    phase: meerkat_runtime::input_state::InputLifecycleState,
    outcome: serde_json::Value,
    payload_kept: bool,
    requests_carrying: usize,
    runs_started: usize,
}

async fn observe_requeued_after_restart(
    root: &std::path::Path,
    host: &StopHost,
    client: &ScriptedClient,
    input: (
        &MobDeliveryIdentity,
        &meerkat_core::lifecycle::InputId,
        WorkSpec,
    ),
    identity: &AgentIdentity,
    session_id: &SessionId,
    text: &str,
) -> RequeuedAfterRestart {
    let (key, input_id, spec) = input;
    let entry = host
        .handle
        .get_member(identity)
        .await
        .expect("roster read")
        .expect("the member survives the restart");
    let replay = match start_stop_input(host, &entry, spec, HandlingMode::Queue, key).await {
        Ok(replay) => stop_answer(replay, session_id).await,
        Err(error) => StopAnswer::Refused(error.to_string()),
    };
    let row = stop_row(&host.runtime_store, session_id, key, input_id).await;
    let events = stop_events(root, session_id).await;
    RequeuedAfterRestart {
        replay,
        phase: row.stored.seed.phase,
        outcome: serde_json::to_value(&row.stored.seed.terminal_outcome)
            .expect("terminal outcome wire image"),
        payload_kept: row.stored.state.persisted_input.is_some(),
        requests_carrying: requests_carrying(&client.requests(), text),
        runs_started: runs_started_for(&events, interaction(key)).len(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requeue_shutdown_keeps_a_queued_human_input_and_runs_it_once_after_restart() {
    keeps_a_queued_human_input_across_the_shutdown(
        "host-human-requeue",
        requeue_options(crate::RequeueMaxAge::DEFAULT),
    )
    .await;
}

/// The default Shutdown (`QueuedInputsOnShutdown::RequeueDurable`) over this
/// fixture's persistent session service and runtime resolves to a requeue
/// with the default bound: the queued human input is kept and runs once after
/// the restart, exactly as under an explicit requeue.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_default_shutdown_keeps_a_queued_human_input_as_a_requeue_shutdown_does() {
    keeps_a_queued_human_input_across_the_shutdown(
        "host-human-default-shutdown",
        crate::ShutdownOptions::default(),
    )
    .await;
}

/// Under a requeue Shutdown, a Queue-mode host human input still queued
/// behind a running turn stays durably queued: its waiter is told it was
/// requeued, its row keeps its payload, its replay identity and a requeue
/// mark, it has no terminal completion and no carrier, and nothing ran it.
/// The member is a durable autonomous member, so the Shutdown keeps its
/// running turn too: that input's waiter is told it was kept, and after the
/// restart it runs to its end before the requeued input, which runs exactly
/// once, its first model request carrying the running turn's user message,
/// and a same-key replay re-attaches to its result.
async fn keeps_a_queued_human_input_across_the_shutdown(
    mob_name: &str,
    options: crate::ShutdownOptions,
) {
    use meerkat_runtime::input_state::InputLifecycleState;

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some(mob_name)).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let running_key = delivery("requeue-running");
    let running = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &running_key,
    )
    .await
    .expect("running human admission");
    client.wait_for_requests(requests_before + 1).await;
    let running_id = admitted_input_id(&host.runtime_store, &session_id, &running_key).await;
    let key = delivery("requeue-queued-behind");
    let interaction_id = interaction(&key);
    let spec = WorkSpec::new(QUEUED_HUMAN, WorkOrigin::External);
    let queued = start_stop_input(&host, &entry, spec.clone(), HandlingMode::Queue, &key)
        .await
        .expect("queued human admission");
    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;
    let before_stop = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    assert_eq!(before_stop.stored.seed.phase, InputLifecycleState::Queued);

    tokio::time::timeout(WAIT, host.handle.shutdown_with_report(options))
        .await
        .expect("the requeue stop finishes")
        .expect("requeue stop");
    let waiter = stop_answer(queued, &session_id).await;
    let running_answer = stop_answer(running, &session_id).await;
    let at_exit = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    let events = stop_events(&root_path, &session_id).await;
    let at_stop = observe_requeued_at_stop(waiter, &at_exit, &events, interaction_id);
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    let after_restart = observe_requeued_after_restart(
        &root_path,
        &restarted,
        &restarted_client,
        (&key, &input_id, spec),
        &identity,
        &session_id,
        QUEUED_HUMAN,
    )
    .await;
    // The kept running turn ran to its end after the restart, before the
    // requeued input, whose replay above waited for it.
    let running_phase = stop_row(
        &restarted.runtime_store,
        &session_id,
        &running_key,
        &running_id,
    )
    .await
    .stored
    .seed
    .phase;
    // The kept input's first model request after the restart shows whether
    // the running turn's user message is in the transcript before it.
    let restart_requests = restarted_client.requests();
    let first_kept_request_carries_running_text = restart_requests
        .iter()
        .find(|request| requests_carrying(std::slice::from_ref(*request), QUEUED_HUMAN) == 1)
        .map(|request| requests_carrying(std::slice::from_ref(request), HUMAN) == 1);
    assert_eq!(
        first_kept_request_carries_running_text,
        Some(true),
        "the kept input's first request after the restart carries the kept running \
         turn's user message"
    );
    assert_eq!(
        (running_answer, at_stop, after_restart, running_phase),
        (
            StopAnswer::RequeuedAcrossStop(running_id.clone()),
            RequeuedAtStop {
                waiter: StopAnswer::RequeuedAcrossStop(input_id.clone()),
                phase: InputLifecycleState::Queued,
                outcome: serde_json::Value::Null,
                payload_kept: true,
                replay_identity_kept: true,
                requeue_mark: true,
                outbox: false,
                terminal_completion: false,
                pending_terminal_owner: false,
                runs_started: 0,
            },
            RequeuedAfterRestart {
                replay: StopAnswer::Succeeded,
                phase: InputLifecycleState::Consumed,
                outcome: serde_json::json!({"outcome_type": "consumed"}),
                payload_kept: false,
                requests_carrying: 1,
                runs_started: 1,
            },
            InputLifecycleState::Consumed,
        ),
        "the requeued input stays queued across the stop and runs once after the restart, \
         after the kept running turn"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// Every durable input row of `session_id`'s runtime as its stored wire
/// image, in input id order.
async fn member_input_rows(
    runtime_store: &Arc<dyn RuntimeStore>,
    session_id: &SessionId,
) -> Vec<(String, serde_json::Value)> {
    let mut rows = runtime_store
        .load_input_states(&LogicalRuntimeId::for_session(session_id))
        .await
        .expect("runtime-owned input rows")
        .into_iter()
        .map(|row| match row {
            meerkat_runtime::store::InputStateRow::Decoded(stored) => (
                stored.state.input_id.to_string(),
                serde_json::to_value(&*stored).expect("the row's stored wire image"),
            ),
            meerkat_runtime::store::InputStateRow::Corrupt { input_id, detail } => {
                panic!("input row {input_id} does not decode: {detail}")
            }
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    rows
}

/// A Shutdown with `QueuedInputsOnShutdown::Keep` over a member whose
/// registration served nothing changes nothing it finds. The first process
/// shuts down with a requeue while a turn runs, its first round committed (a
/// tool call) and its second request blocked, and a second input waits
/// behind it, so the store holds the member's open run with a round to
/// continue from and its requeued input. The second process restores the mob
/// with the member's run starts held by its host, so the member's
/// registration recovers both and runs nothing, and shuts down with Keep:
/// every input row of the member reads the same before the Keep (after the
/// held restore, whose ordinary recovery may normalize an input as every
/// start does) and after it, and its model saw no request. The third process
/// resumes the run and runs the queued input, once each.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_keep_shutdown_of_a_held_member_leaves_its_work_as_the_store_held_it() {
    use meerkat_runtime::input_state::InputLifecycleState;

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("keep-shutdown")).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    // The running turn's first request calls a tool, which commits that
    // round; its second request blocks, so the run stays open with a round
    // to continue from, which a held restart marks as resumable.
    client.call_a_tool_next();
    let requests_before = client.requests().len();
    let running_key = delivery("keep-running");
    let running = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &running_key,
    )
    .await
    .expect("running human admission");
    client.wait_for_requests(requests_before + 2).await;
    let running_id = admitted_input_id(&host.runtime_store, &session_id, &running_key).await;
    let key = delivery("keep-queued-behind");
    let spec = WorkSpec::new(QUEUED_HUMAN, WorkOrigin::External);
    let queued = start_stop_input(&host, &entry, spec.clone(), HandlingMode::Queue, &key)
        .await
        .expect("queued human admission");
    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;
    tokio::time::timeout(
        WAIT,
        host.handle
            .shutdown_with_report(requeue_options(crate::RequeueMaxAge::DEFAULT)),
    )
    .await
    .expect("the requeue stop finishes")
    .expect("requeue stop");
    assert_eq!(
        (
            stop_answer(running, &session_id).await,
            stop_answer(queued, &session_id).await,
        ),
        (
            StopAnswer::RequeuedAcrossStop(running_id.clone()),
            StopAnswer::RequeuedAcrossStop(input_id.clone()),
        ),
        "the first process keeps the running turn and the queued input"
    );
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    // The second process holds the member and serves it nothing.
    let held_client = ScriptedClient::new(true);
    let held = open_stop_host_holding(
        &root_path,
        &held_client,
        None,
        std::slice::from_ref(&identity),
    )
    .await;
    let mut parks = held.adapter.run_start_held_parks();
    // Hang guard only: the park is the positive event.
    tokio::time::timeout(WAIT, parks.wait_for(|parks| *parks >= 1))
        .await
        .expect("the member's runtime loop parks on the hold")
        .expect("park signal");
    assert!(
        held.adapter.resumable_run_id(&session_id).await.is_some(),
        "the held registration keeps the open run marked"
    );
    // The baseline is the store as the held restore left it: the boot's
    // ordinary recovery, which every start runs, may already have
    // normalized an input. The Keep is measured across its own Shutdown.
    let rows_before = member_input_rows(&held.runtime_store, &session_id).await;
    let digests_before = (
        stop_row(&held.runtime_store, &session_id, &running_key, &running_id)
            .await
            .digest,
        stop_row(&held.runtime_store, &session_id, &key, &input_id)
            .await
            .digest,
    );
    tokio::time::timeout(
        WAIT,
        held.handle.shutdown_with_report(
            crate::ShutdownOptions::default()
                .with_queued_inputs(crate::QueuedInputsOnShutdown::Keep),
        ),
    )
    .await
    .expect("the keep stop finishes")
    .expect("keep stop");
    let rows_after = member_input_rows(&held.runtime_store, &session_id).await;
    let digests_after = (
        stop_row(&held.runtime_store, &session_id, &running_key, &running_id)
            .await
            .digest,
        stop_row(&held.runtime_store, &session_id, &key, &input_id)
            .await
            .digest,
    );
    assert_eq!(
        held_client.requests().len(),
        0,
        "the held member ran nothing"
    );
    assert_eq!(
        (rows_after, digests_after),
        (rows_before, digests_before),
        "the Keep itself changes no input row: a keep stop of a registration that served \
         nothing leaves every row as the held restore left it"
    );
    drop(held);
    await_supervisor_route_release(&mob_id).await;

    // The third process runs the kept run and the queued input, once each.
    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    let after_restart = observe_requeued_after_restart(
        &root_path,
        &restarted,
        &restarted_client,
        (&key, &input_id, spec),
        &identity,
        &session_id,
        QUEUED_HUMAN,
    )
    .await;
    let running_phase = stop_row(
        &restarted.runtime_store,
        &session_id,
        &running_key,
        &running_id,
    )
    .await
    .stored
    .seed
    .phase;
    assert_eq!(
        (after_restart, running_phase),
        (
            RequeuedAfterRestart {
                replay: StopAnswer::Succeeded,
                phase: InputLifecycleState::Consumed,
                outcome: serde_json::json!({"outcome_type": "consumed"}),
                payload_kept: false,
                requests_carrying: 1,
                runs_started: 1,
            },
            InputLifecycleState::Consumed,
        ),
        "the next start resumes the kept run and runs the queued input once"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// A requeued input older than the requeue bound when the next start
/// recovers it ends as expired instead of running: payload retired, no run,
/// no model request, and a same-key replay answers the expiry. A later start
/// serves nothing: the expiry's commit left the row terminal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requeue_shutdown_expires_a_queued_human_input_older_than_the_bound_at_restart() {
    use meerkat_runtime::input_state::InputLifecycleState;

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("host-human-requeue-expiry")).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let running = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &delivery("requeue-expiry-running"),
    )
    .await
    .expect("running human admission");
    client.wait_for_requests(requests_before + 1).await;
    let key = delivery("requeue-expiry-queued");
    let interaction_id = interaction(&key);
    let spec = WorkSpec::new(QUEUED_HUMAN, WorkOrigin::External);
    let queued = start_stop_input(&host, &entry, spec.clone(), HandlingMode::Queue, &key)
        .await
        .expect("queued human admission");
    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;

    tokio::time::timeout(
        WAIT,
        host.handle
            .shutdown_with_report(requeue_options(Duration::from_millis(1))),
    )
    .await
    .expect("the requeue stop finishes")
    .expect("requeue stop");
    let waiter = stop_answer(queued, &session_id).await;
    let _ = stop_answer(running, &session_id).await;
    let at_exit = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    let events = stop_events(&root_path, &session_id).await;
    let at_stop = observe_requeued_at_stop(waiter, &at_exit, &events, interaction_id);
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    let after_restart = observe_requeued_after_restart(
        &root_path,
        &restarted,
        &restarted_client,
        (&key, &input_id, spec),
        &identity,
        &session_id,
        QUEUED_HUMAN,
    )
    .await;
    assert_eq!(
        (at_stop.waiter.clone(), at_stop.requeue_mark, after_restart),
        (
            StopAnswer::RequeuedAcrossStop(input_id.clone()),
            true,
            RequeuedAfterRestart {
                replay: StopAnswer::RequeueExpired(input_id.clone()),
                phase: InputLifecycleState::Abandoned,
                outcome: serde_json::json!({
                    "outcome_type": "abandoned",
                    "reason": "requeue_expired",
                }),
                payload_kept: false,
                requests_carrying: 0,
                runs_started: 0,
            },
        ),
        "a requeued input past its bound at restart ends as expired and never runs"
    );
    // The input-status read, what `session/input_status` serves from, names
    // the expiry typed.
    let status = {
        use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
        restarted
            .adapter
            .input_state(&session_id, &input_id)
            .await
            .expect("input-status read")
            .expect("the expired input keeps its status")
    };
    assert_eq!(
        status.seed.terminal_outcome,
        Some(meerkat_runtime::InputTerminalOutcome::Abandoned {
            reason: meerkat_runtime::InputAbandonReason::RequeueExpired,
        }),
        "the input-status read carries the typed expiry"
    );

    // Once the expiry has committed, the row is terminal: the next start
    // serves nothing, and the row stays expired. (The process stops
    // gracefully here; the store then holds exactly what the expiry's own
    // commit wrote, which is what a cut right after it leaves.)
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
    drop(restarted);
    await_supervisor_route_release(&mob_id).await;
    let third_client = ScriptedClient::new(true);
    let third = open_stop_host(&root_path, &third_client, None).await;
    let after_cut = stop_row(&third.runtime_store, &session_id, &key, &input_id).await;
    let events = stop_events(&root_path, &session_id).await;
    assert_eq!(
        (
            after_cut.stored.seed.phase,
            serde_json::to_value(&after_cut.stored.seed.terminal_outcome)
                .expect("terminal outcome wire image"),
            runs_started_for(&events, interaction(&key)).len(),
            requests_carrying(&third_client.requests(), QUEUED_HUMAN),
        ),
        (
            InputLifecycleState::Abandoned,
            serde_json::json!({
                "outcome_type": "abandoned",
                "reason": "requeue_expired",
            }),
            0,
            0,
        ),
        "a start after the expiry committed serves nothing"
    );
    tokio::time::timeout(WAIT, third.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// Two inputs requeued in admission order, then one admitted right after the
/// restart: the restarted member runs them in that order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requeue_shutdown_runs_requeued_inputs_in_admission_order_before_new_work() {
    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("host-human-requeue-order")).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let running = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &delivery("requeue-order-running"),
    )
    .await
    .expect("running human admission");
    client.wait_for_requests(requests_before + 1).await;
    let first_key = delivery("requeue-order-first");
    let first_spec = WorkSpec::new(QUEUED_HUMAN, WorkOrigin::External);
    let first = start_stop_input(
        &host,
        &entry,
        first_spec.clone(),
        HandlingMode::Queue,
        &first_key,
    )
    .await
    .expect("first queued admission");
    let second_key = delivery("requeue-order-second");
    let second_spec = WorkSpec::new(QUEUED_SECOND, WorkOrigin::External);
    let second = start_stop_input(
        &host,
        &entry,
        second_spec.clone(),
        HandlingMode::Queue,
        &second_key,
    )
    .await
    .expect("second queued admission");

    tokio::time::timeout(
        WAIT,
        host.handle
            .shutdown_with_report(requeue_options(crate::RequeueMaxAge::DEFAULT)),
    )
    .await
    .expect("the requeue stop finishes")
    .expect("requeue stop");
    let _ = stop_answer(first, &session_id).await;
    let _ = stop_answer(second, &session_id).await;
    let _ = stop_answer(running, &session_id).await;
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    let restarted_entry = restarted
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member survives the restart");
    let later = start_stop_input(
        &restarted,
        &restarted_entry,
        WorkSpec::new(AFTER_RESTART, WorkOrigin::External),
        HandlingMode::Queue,
        &delivery("requeue-order-after-restart"),
    )
    .await
    .expect("post-restart admission");
    let later_answer = stop_answer(later, &session_id).await;
    let first_replay = match start_stop_input(
        &restarted,
        &restarted_entry,
        first_spec,
        HandlingMode::Queue,
        &first_key,
    )
    .await
    {
        Ok(replay) => stop_answer(replay, &session_id).await,
        Err(error) => StopAnswer::Refused(error.to_string()),
    };
    let second_replay = match start_stop_input(
        &restarted,
        &restarted_entry,
        second_spec,
        HandlingMode::Queue,
        &second_key,
    )
    .await
    {
        Ok(replay) => stop_answer(replay, &session_id).await,
        Err(error) => StopAnswer::Refused(error.to_string()),
    };
    // The last request carries the whole transcript; read the order of the
    // three user messages from it.
    let requests = restarted_client.requests();
    let order = requests.last().map(|request| {
        [QUEUED_HUMAN, QUEUED_SECOND, AFTER_RESTART].map(|text| {
            request.messages.iter().position(|message| {
                matches!(message, Message::User(user) if user.text_content().contains(text))
            })
        })
    });
    let in_order = matches!(
        order,
        Some([Some(first), Some(second), Some(later)]) if first < second && second < later
    );
    assert_eq!(
        (first_replay, second_replay, later_answer, in_order),
        (
            StopAnswer::Succeeded,
            StopAnswer::Succeeded,
            StopAnswer::Succeeded,
            true
        ),
        "requeued inputs run in admission order, before work admitted after the restart: \
         {order:?}"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// Requeue keeps only eligible inputs: a Steer-mode input queued behind the
/// running turn, not yet joined to its run, is not requeued and never runs
/// after the restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requeue_shutdown_does_not_keep_a_steer_input_queued_behind_the_running_turn() {
    use meerkat_runtime::input_state::InputLifecycleState;

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("host-human-requeue-steer")).await;
    let mob_id = host.handle.mob_id().clone();
    let (_identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let running = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &delivery("requeue-steer-running"),
    )
    .await
    .expect("running human admission");
    client.wait_for_requests(requests_before + 1).await;
    let key = delivery("requeue-steer-queued");
    let interaction_id = interaction(&key);
    let steer = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(QUEUED_HUMAN, WorkOrigin::External),
        HandlingMode::Steer,
        &key,
    )
    .await
    .expect("steer human admission");
    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;

    tokio::time::timeout(
        WAIT,
        host.handle
            .shutdown_with_report(requeue_options(crate::RequeueMaxAge::DEFAULT)),
    )
    .await
    .expect("the requeue stop finishes")
    .expect("requeue stop");
    let _ = stop_answer(steer, &session_id).await;
    let _ = stop_answer(running, &session_id).await;
    let at_exit = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    let after_restart = stop_row(&restarted.runtime_store, &session_id, &key, &input_id).await;
    let events = stop_events(&root_path, &session_id).await;
    assert_eq!(
        (
            at_exit.stored.seed.phase,
            !at_exit.image["stop_requeue"].is_null(),
            after_restart.digest == at_exit.digest,
            requests_carrying(&restarted_client.requests(), QUEUED_HUMAN),
            runs_started_for(&events, interaction_id).len(),
        ),
        (InputLifecycleState::Abandoned, false, true, 0, 0),
        "a steer input is not requeued: terminal at the stop, never run after the restart"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// Requeue keeps no input bound to an objective. A member's kickoff is one
/// (its input carries the kickoff objective), and the mob records the
/// kickoff's outcome in the stopping process, so a kept kickoff would run
/// after a restart that record already calls failed. An objective-bound
/// input queued behind the running turn therefore ends as a Stop Shutdown
/// leaves it: terminal and unmarked, its waiter answered with the
/// unregistered termination, and never run after the restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requeue_shutdown_does_not_keep_an_objective_bound_input_queued_behind_the_running_turn() {
    use meerkat_runtime::input_state::InputLifecycleState;

    const UNREGISTERED: &str = "runtime session unregistered";
    let terminated = StopAnswer::RuntimeTerminated {
        reason: UNREGISTERED.to_owned(),
        kind: meerkat_core::TurnTerminalCauseKind::FatalFailure,
        detail: Some(UNREGISTERED.to_owned()),
    };
    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("host-human-requeue-objective")).await;
    let mob_id = host.handle.mob_id().clone();
    let (_identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let running = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &delivery("requeue-objective-running"),
    )
    .await
    .expect("running human admission");
    client.wait_for_requests(requests_before + 1).await;
    let key = delivery("requeue-objective-queued");
    let interaction_id = interaction(&key);
    let bound = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(QUEUED_HUMAN, WorkOrigin::External)
            .with_objective_id(meerkat_core::interaction::ObjectiveId::new()),
        HandlingMode::Queue,
        &key,
    )
    .await
    .expect("objective-bound human admission");
    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;
    let before_stop = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    assert_eq!(before_stop.stored.seed.phase, InputLifecycleState::Queued);

    tokio::time::timeout(
        WAIT,
        host.handle
            .shutdown_with_report(requeue_options(crate::RequeueMaxAge::DEFAULT)),
    )
    .await
    .expect("the requeue stop finishes")
    .expect("requeue stop");
    let waiter = stop_answer(bound, &session_id).await;
    let _ = stop_answer(running, &session_id).await;
    let at_exit = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    let after_restart = stop_row(&restarted.runtime_store, &session_id, &key, &input_id).await;
    let events = stop_events(&root_path, &session_id).await;
    assert_eq!(
        (
            waiter,
            at_exit.stored.seed.phase,
            !at_exit.image["stop_requeue"].is_null(),
            after_restart.digest == at_exit.digest,
            requests_carrying(&restarted_client.requests(), QUEUED_HUMAN),
            runs_started_for(&events, interaction_id).len(),
        ),
        (
            terminated,
            InputLifecycleState::Abandoned,
            false,
            true,
            0,
            0
        ),
        "an objective-bound input is not requeued: terminal at the stop, never run after the restart"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

const KEPT_OUT_EVENT: &str = "requeue-kept-out-event";
const DIRECTED_STEP: &str = "A directed flow step, queued behind the running turn.";
const BARRIER_HUMAN: &str = "New work after the restart.";

/// An input a requeue Shutdown does not keep, admitted straight into the
/// member's runtime: its waiter, the row the stopping process leaves, and
/// what the restarted process does with it.
#[derive(Debug, PartialEq)]
struct NotKeptAcrossStop {
    waiter: String,
    phase: meerkat_runtime::input_state::InputLifecycleState,
    outcome: serde_json::Value,
    payload_kept: bool,
    requeue_mark: bool,
    carrier: bool,
    row_unchanged_by_restart: bool,
    requests_after_restart: usize,
}

/// How the waiter of an input admitted straight into the runtime ended.
async fn runtime_waiter_answer(waiter: meerkat_runtime::completion::CompletionHandle) -> String {
    match tokio::time::timeout(WAIT, waiter.try_wait()).await {
        Err(_) => "unresolved".to_owned(),
        Ok(Ok(meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated { .. })) => {
            "runtime_terminated".to_owned()
        }
        Ok(Ok(other)) => format!("completed: {other:?}"),
        Ok(Err(error)) if error.is_requeued_across_stop() => "requeued_across_stop".to_owned(),
        Ok(Err(error)) => format!("wait failed: {error}"),
    }
}

/// Interaction failures the log recorded for `interaction_id`.
fn interaction_failures_for(events: &[AgentEvent], interaction_id: InteractionId) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::InteractionFailed { interaction_id: failed, .. }
                    if *failed == interaction_id
            )
        })
        .count()
}

/// Under a requeue Shutdown only a queued prompt is kept. A durable external
/// event and a directed flow step queued behind the running turn end as
/// stopped and unmarked, as a Stop Shutdown ends them: the event's waiter
/// ends with the runtime's termination and its payload is retired, the
/// directed step keeps its interaction carrier and its interaction fails
/// exactly once, and the restarted process runs neither (new work admitted
/// after the restart runs, and anything kept would have run before it).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requeue_shutdown_does_not_keep_a_queued_event_or_a_directed_step() {
    use meerkat_runtime::input_state::InputLifecycleState;

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("host-human-requeue-event-step")).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let running = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &delivery("requeue-event-step-running"),
    )
    .await
    .expect("running human admission");
    client.wait_for_requests(requests_before + 1).await;

    let event = meerkat_runtime::Input::ExternalEvent(meerkat_runtime::input::ExternalEventInput {
        header: meerkat_runtime::input::InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: meerkat_runtime::input::InputOrigin::External {
                source_name: "webhook".into(),
            },
            durability: meerkat_runtime::input::InputDurability::Durable,
            visibility: meerkat_runtime::input::InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        event_type: "build".into(),
        payload: serde_json::json!({ "status": KEPT_OUT_EVENT }),
        blocks: None,
        handling_mode: HandlingMode::Queue,
        render_metadata: None,
        objective_id: None,
    });
    let event_id = event.id().clone();
    let directed_interaction = InteractionId(Uuid::now_v7());
    let directed = meerkat_runtime::mob_adapter::create_tracked_flow_step_input(
        "requeue-directed",
        meerkat_core::types::ContentInput::Text(DIRECTED_STEP.to_owned()),
        "requeue-flow",
        None,
        &directed_interaction.0.to_string(),
    )
    .expect("tracked flow step");
    let directed_id = directed.id().clone();
    let mut waiters = Vec::new();
    for input in [event, directed] {
        let (outcome, waiter) = host
            .adapter
            .accept_input_with_completion(&session_id, input)
            .await
            .expect("admitted behind the running turn");
        assert!(outcome.is_accepted(), "{outcome:?}");
        waiters.push(waiter.expect("an admitted input has a waiter"));
    }
    for queued in [&event_id, &directed_id] {
        let (stored, _) = row_image(&host.runtime_store, &session_id, queued).await;
        assert_eq!(stored.seed.phase, InputLifecycleState::Queued, "{queued}");
    }

    tokio::time::timeout(
        WAIT,
        host.handle
            .shutdown_with_report(requeue_options(crate::RequeueMaxAge::DEFAULT)),
    )
    .await
    .expect("the requeue stop finishes")
    .expect("requeue stop");
    let _ = stop_answer(running, &session_id).await;
    let mut answers = Vec::new();
    for waiter in waiters {
        answers.push(runtime_waiter_answer(waiter).await);
    }
    let mut at_exit = Vec::new();
    for input_id in [&event_id, &directed_id] {
        at_exit.push(row_image(&host.runtime_store, &session_id, input_id).await);
    }
    let failures_at_exit = interaction_failures_for(
        &stop_events(&root_path, &session_id).await,
        directed_interaction,
    );
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    let entry = restarted
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member survives the restart");
    let barrier = start_stop_input(
        &restarted,
        &entry,
        WorkSpec::new(BARRIER_HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &delivery("requeue-event-step-barrier"),
    )
    .await
    .expect("new work after the restart");
    assert_eq!(
        stop_answer(barrier, &session_id).await,
        StopAnswer::Succeeded
    );
    let requests = restarted_client.requests();
    assert_eq!(requests_carrying(&requests, BARRIER_HUMAN), 1);
    let mut observed = Vec::new();
    for ((input_id, waiter), ((stored, image), text)) in [&event_id, &directed_id]
        .into_iter()
        .zip(answers)
        .zip(at_exit.into_iter().zip([KEPT_OUT_EVENT, DIRECTED_STEP]))
    {
        let after = row_image(&restarted.runtime_store, &session_id, input_id)
            .await
            .1;
        observed.push(NotKeptAcrossStop {
            waiter,
            phase: stored.seed.phase,
            outcome: serde_json::to_value(&stored.seed.terminal_outcome)
                .expect("terminal outcome wire image"),
            payload_kept: stored.state.persisted_input.is_some(),
            requeue_mark: !image["stop_requeue"].is_null(),
            carrier: !image["interaction_terminal_outbox"].is_null(),
            row_unchanged_by_restart: after == image,
            requests_after_restart: requests_carrying(&requests, text),
        });
    }
    let failures_after_restart = interaction_failures_for(
        &stop_events(&root_path, &session_id).await,
        directed_interaction,
    );
    let stopped = |carrier: bool| NotKeptAcrossStop {
        waiter: "runtime_terminated".to_owned(),
        phase: InputLifecycleState::Abandoned,
        outcome: serde_json::json!({"outcome_type": "abandoned", "reason": "stopped"}),
        payload_kept: false,
        requeue_mark: false,
        carrier,
        row_unchanged_by_restart: true,
        requests_after_restart: 0,
    };
    assert_eq!(
        (observed, failures_at_exit, failures_after_restart),
        (vec![stopped(false), stopped(true)], 1, 1),
        "an event and a directed step are not requeued: stopped at the stop, the directed \
         interaction failed once, neither run after the restart"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

const BOUND_HUMAN: &str = "A host question bound to the member's objective.";

/// One objective-bound input as a Shutdown leaves it and as the restarted
/// process finds it. Ids and times differ between runs, so the row's
/// unchanged-by-restart fact is compared, not its bytes.
#[derive(Debug, PartialEq)]
struct ObjectiveBoundAfterShutdown {
    waiter: StopAnswer,
    phase: meerkat_runtime::input_state::InputLifecycleState,
    outcome: serde_json::Value,
    payload_kept: bool,
    requeue_mark: bool,
    row_unchanged_by_restart: bool,
    requests_after_restart: usize,
    runs_started: usize,
}

/// What one Shutdown left of a plain queued host human input and of one
/// bound to an objective. `plain_runs` counts the plain input's runs in the
/// whole log, so a kept input that ran twice shows.
struct ObjectiveShutdownScenario {
    plain_kept: bool,
    plain_runs: usize,
    bound: ObjectiveBoundAfterShutdown,
}

/// The stored row of `input_id` and its wire image.
async fn row_image(
    runtime_store: &Arc<dyn RuntimeStore>,
    session_id: &SessionId,
    input_id: &meerkat_core::lifecycle::InputId,
) -> (
    meerkat_runtime::input_state::StoredInputState,
    serde_json::Value,
) {
    let stored = runtime_store
        .load_input_state(&LogicalRuntimeId::for_session(session_id), input_id)
        .await
        .expect("runtime-owned input read")
        .expect("the input keeps its durable row");
    let image = serde_json::to_value(&stored).expect("the row's stored wire image");
    (stored, image)
}

/// Run one Shutdown, `requeue` or an explicit Stop, over a plain host human
/// input and an objective-bound one queued behind one running turn, then
/// restart and let the restarted process run whatever was kept.
async fn objective_bound_shutdown(mob_name: &str, requeue: bool) -> ObjectiveShutdownScenario {
    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some(mob_name)).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;
    let objective = meerkat_core::interaction::ObjectiveId::new();

    client.block();
    let requests_before = client.requests().len();
    let running = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &delivery("objective-scenario-running"),
    )
    .await
    .expect("running human admission");
    client.wait_for_requests(requests_before + 1).await;
    let plain_key = delivery("objective-scenario-plain");
    let plain = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(QUEUED_HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &plain_key,
    )
    .await
    .expect("plain human admission");
    let bound_key = delivery("objective-scenario-bound");
    let bound_human = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(BOUND_HUMAN, WorkOrigin::External).with_objective_id(objective),
        HandlingMode::Queue,
        &bound_key,
    )
    .await
    .expect("objective-bound human admission");
    let plain_id = admitted_input_id(&host.runtime_store, &session_id, &plain_key).await;
    let bound_id = admitted_input_id(&host.runtime_store, &session_id, &bound_key).await;

    let options = if requeue {
        requeue_options(crate::RequeueMaxAge::DEFAULT)
    } else {
        stop_options()
    };
    tokio::time::timeout(WAIT, host.handle.shutdown_with_report(options))
        .await
        .expect("the stop finishes")
        .expect("graceful stop");
    let plain_waiter = stop_answer(plain, &session_id).await;
    let bound_waiter = stop_answer(bound_human, &session_id).await;
    let _ = stop_answer(running, &session_id).await;
    let plain_marked = !row_image(&host.runtime_store, &session_id, &plain_id)
        .await
        .1["stop_requeue"]
        .is_null();
    let (stored, image) = row_image(&host.runtime_store, &session_id, &bound_id).await;
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    // A same-key replay of the plain input waits for whatever the restart
    // made of it: the kept input's run, or the stop's answer again.
    let entry = restarted
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member survives the restart");
    let plain_replay = match start_stop_input(
        &restarted,
        &entry,
        WorkSpec::new(QUEUED_HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &plain_key,
    )
    .await
    {
        Ok(replay) => stop_answer(replay, &session_id).await,
        Err(error) => StopAnswer::Refused(error.to_string()),
    };
    let requests = restarted_client.requests();
    let events = stop_events(&root_path, &session_id).await;
    let after = row_image(&restarted.runtime_store, &session_id, &bound_id)
        .await
        .1;
    let bound = ObjectiveBoundAfterShutdown {
        waiter: bound_waiter,
        phase: stored.seed.phase,
        outcome: serde_json::to_value(&stored.seed.terminal_outcome)
            .expect("terminal outcome wire image"),
        payload_kept: stored.state.persisted_input.is_some(),
        requeue_mark: !image["stop_requeue"].is_null(),
        row_unchanged_by_restart: after == image,
        requests_after_restart: requests_carrying(&requests, BOUND_HUMAN),
        runs_started: runs_started_for(&events, interaction(&bound_key)).len(),
    };
    let plain_runs = runs_started_for(&events, interaction(&plain_key)).len();
    let plain_kept = matches!(plain_waiter, StopAnswer::RequeuedAcrossStop(_))
        && plain_marked
        && plain_replay == StopAnswer::Succeeded
        && requests_carrying(&requests, QUEUED_HUMAN) == 1;
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
    ObjectiveShutdownScenario {
        plain_kept,
        plain_runs,
        bound,
    }
}

/// Requeue keeps no input bound to an objective. A member's kickoff is one
/// (its input carries the kickoff objective), and so is a turn-driven
/// member's delegated or helper work while it works an objective; their
/// owners record their outcome in the stopping process, and nothing runs
/// them again after a restart. So an objective-bound input queued behind the
/// running turn ends under a requeue Shutdown exactly as under a Stop
/// Shutdown, compared field by field against that baseline: the same waiter
/// answer, terminal, unmarked, untouched by the restart and never run after
/// it. The plain input queued beside it is kept, and runs once after the
/// restart, only under the requeue Shutdown, so the comparison tells kept
/// from stopped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requeue_shutdown_ends_objective_bound_inputs_exactly_as_a_stop_shutdown() {
    use meerkat_runtime::input_state::InputLifecycleState;

    let stop = objective_bound_shutdown("host-human-objective-stop", false).await;
    let requeue = objective_bound_shutdown("host-human-objective-requeue", true).await;
    assert_eq!(
        (
            stop.plain_kept,
            stop.plain_runs,
            requeue.plain_kept,
            requeue.plain_runs
        ),
        (false, 0, true, 1),
        "only the requeue Shutdown keeps the plain input, which then runs once"
    );
    assert_eq!(
        requeue.bound, stop.bound,
        "an objective-bound input ends under requeue exactly as under Stop"
    );
    assert_eq!(
        (
            requeue.bound.phase,
            requeue.bound.requeue_mark,
            requeue.bound.row_unchanged_by_restart,
            requeue.bound.requests_after_restart,
            requeue.bound.runs_started,
        ),
        (InputLifecycleState::Abandoned, false, true, 0, 0),
        "terminal at the stop, never run, before or after the restart"
    );
}

#[test]
fn requeue_max_age_refuses_zero_and_over_the_cap() {
    use crate::{RequeueMaxAge, ShutdownOptionsError};

    let over = RequeueMaxAge::CAP + Duration::from_millis(1);
    assert_eq!(
        (
            RequeueMaxAge::new(Duration::ZERO),
            RequeueMaxAge::new(Duration::from_micros(999)),
            RequeueMaxAge::new(over),
            RequeueMaxAge::new(RequeueMaxAge::CAP).map(RequeueMaxAge::get),
            RequeueMaxAge::new(Duration::from_millis(1)).map(RequeueMaxAge::get),
            RequeueMaxAge::default().get(),
        ),
        (
            Err(ShutdownOptionsError::RequeueMaxAgeUnderOneMillisecond {
                max_age: Duration::ZERO,
            }),
            Err(ShutdownOptionsError::RequeueMaxAgeUnderOneMillisecond {
                max_age: Duration::from_micros(999),
            }),
            Err(ShutdownOptionsError::RequeueMaxAgeOverCap {
                max_age: over,
                cap: RequeueMaxAge::CAP,
            }),
            Ok(Duration::from_hours(24)),
            Ok(Duration::from_millis(1)),
            Duration::from_mins(15),
        )
    );
}

/// The default Shutdown choice is `RequeueDurable`. It resolves per session:
/// a requeue with the default bound for a session that commits each round
/// durably, a stop for any other. An explicit choice applies as given, and a
/// keep is the runtime's keep for every session.
#[test]
fn the_default_shutdown_requeues_only_sessions_that_commit_each_round() {
    use crate::{QueuedInputsOnShutdown, RequeueMaxAge, ShutdownOptions};
    use meerkat_runtime::UnregisterQueuedInputs;

    let bound = RequeueMaxAge::new(Duration::from_secs(90)).expect("an accepted requeue bound");
    assert_eq!(
        (
            QueuedInputsOnShutdown::default(),
            ShutdownOptions::default().queued_inputs,
        ),
        (
            QueuedInputsOnShutdown::RequeueDurable,
            QueuedInputsOnShutdown::RequeueDurable,
        )
    );
    assert_eq!(
        [
            QueuedInputsOnShutdown::RequeueDurable.unregister_queued_inputs(true),
            QueuedInputsOnShutdown::RequeueDurable.unregister_queued_inputs(false),
            QueuedInputsOnShutdown::Stop.unregister_queued_inputs(true),
            QueuedInputsOnShutdown::Requeue(bound).unregister_queued_inputs(false),
            QueuedInputsOnShutdown::Keep.unregister_queued_inputs(true),
            QueuedInputsOnShutdown::Keep.unregister_queued_inputs(false),
        ],
        [
            UnregisterQueuedInputs::Requeue {
                max_age: RequeueMaxAge::DEFAULT,
            },
            UnregisterQueuedInputs::Stop,
            UnregisterQueuedInputs::Stop,
            UnregisterQueuedInputs::Requeue {
                max_age: Duration::from_secs(90),
            },
            UnregisterQueuedInputs::Keep,
            UnregisterQueuedInputs::Keep,
        ]
    );
}

/// A requeue Shutdown whose deadline passes before a member's unregister is
/// admitted (its registration transaction is held) reports the member
/// pending and hands the unregister to its settlement. The settlement begins
/// that unregister with the Shutdown's choice: once the transaction is free
/// and the settlement settles, the input queued behind the running turn is
/// kept durably queued and marked for the next start, exactly as when
/// Shutdown begins the unregister itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requeue_shutdown_whose_admission_misses_the_deadline_still_keeps_the_queued_input() {
    use crate::runtime::{MemberShutdownOutcome, MemberUnregisterState};
    use meerkat_core::time_compat::Instant;
    use meerkat_runtime::input_state::InputLifecycleState;

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("host-human-requeue-settlement")).await;
    let (identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let running = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &delivery("requeue-settlement-running"),
    )
    .await
    .expect("running human admission");
    client.wait_for_requests(requests_before + 1).await;
    let key = delivery("requeue-settlement-queued");
    let interaction_id = interaction(&key);
    let queued = start_stop_input(
        &host,
        &entry,
        WorkSpec::new(QUEUED_HUMAN, WorkOrigin::External),
        HandlingMode::Queue,
        &key,
    )
    .await
    .expect("queued human admission");
    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;

    let guard = host
        .adapter
        .hold_session_registration_transaction_for_test(&session_id)
        .await;
    let report = tokio::time::timeout(
        WAIT,
        host.handle.shutdown_with_report(
            requeue_options(crate::RequeueMaxAge::DEFAULT)
                .with_deadline(Instant::now() + Duration::from_secs(1)),
        ),
    )
    .await
    .expect("the requeue stop returns at its deadline")
    .expect("requeue stop");
    let mut settlement = match report.members.get(&identity) {
        Some(MemberShutdownOutcome::UnregisterPending {
            stage, settlement, ..
        }) => {
            assert_eq!(stage, "registration_transaction_admission");
            settlement.clone()
        }
        other => panic!("expected the member pending at its unregister admission, got {other:?}"),
    };
    drop(guard);
    assert_eq!(
        settlement.settle_within(Instant::now() + WAIT).await,
        MemberUnregisterState::Settled,
        "the settlement begins and finishes the member's unregister"
    );

    let waiter = stop_answer(queued, &session_id).await;
    let _ = stop_answer(running, &session_id).await;
    let at_exit = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    let events = stop_events(&root_path, &session_id).await;
    assert_eq!(
        observe_requeued_at_stop(waiter, &at_exit, &events, interaction_id),
        RequeuedAtStop {
            waiter: StopAnswer::RequeuedAcrossStop(input_id.clone()),
            phase: InputLifecycleState::Queued,
            outcome: serde_json::Value::Null,
            payload_kept: true,
            replay_identity_kept: true,
            requeue_mark: true,
            outbox: false,
            terminal_completion: false,
            pending_terminal_owner: false,
            runs_started: 0,
        },
        "an unregister the settlement begins keeps the queued input, as Shutdown's own does"
    );
}

/// A Stop that leaves runs running holds run starts and returns without
/// waiting for any turn.
const STOP_RETURNS_WITHIN: Duration = Duration::from_secs(10);
/// A drain bound the rows whose run ends never reach.
const UNREACHED_DRAIN_BOUND: Duration = Duration::from_secs(600);
/// The drain bound of the rows whose run outlives the drain.
const DRAIN_BOUND: Duration = Duration::from_millis(500);

/// Stop the host's mob, leaving every member's current run running.
async fn stop_leaving_runs_running(host: &StopHost) -> crate::MobStopReport {
    tokio::time::timeout(
        STOP_RETURNS_WITHIN,
        host.handle
            .stop_with_options(crate::StopOptions::default().leave_runs_running()),
    )
    .await
    .expect("a Stop that leaves runs running returns without waiting for them")
    .expect("stop leaving runs running")
}

/// Drain the stopped mob's runs in two steps: take the drain's snapshot,
/// then `release` the held model call, then wait within `bound`. The run the
/// snapshot captures cannot end before the snapshot sees it.
async fn drain_after_snapshot(
    host: &StopHost,
    release: impl FnOnce(),
    bound: Duration,
) -> crate::MobRunDrainReport {
    let snapshot = host
        .handle
        .drain_runs_snapshot()
        .await
        .expect("a stopped mob's drain snapshot");
    release();
    tokio::time::timeout(
        WAIT,
        crate::runtime::run_drain::drain(
            snapshot,
            meerkat_core::time_compat::Instant::now() + bound,
        ),
    )
    .await
    .expect("the drain returns once the run it captured ends")
}

/// How a drained input's bounded waiter ended, with its answer text when it
/// succeeded.
async fn drained_answer(turn: WorkTurnHandle) -> (StopAnswer, Option<String>) {
    match tokio::time::timeout(
        WAIT,
        turn.wait_bounded(BoundedResultSpec::new("drained human", 1024).expect("bound")),
    )
    .await
    {
        Ok(Ok(result)) => (
            StopAnswer::Succeeded,
            Some(result.result().result().text().to_owned()),
        ),
        Ok(Err(error)) => (StopAnswer::Other(format!("{:?}", error.failure())), None),
        Err(_) => (StopAnswer::Unresolved, None),
    }
}

/// Reopen the host over `root` after a Stop and its Shutdown. The Stop is
/// durable, so the mob comes back Stopped; it is resumed, as a gateway does
/// at boot. Returns the reopened host and the state the mob came back in.
async fn reopen_stopped_host(
    root: &std::path::Path,
    client: &ScriptedClient,
) -> (StopHost, MobState) {
    let host = open_stop_host(root, client, None).await;
    let restored = host
        .handle
        .status()
        .await
        .expect("the reopened mob's state");
    tokio::time::timeout(WAIT, host.handle.resume())
        .await
        .expect("the resume finishes")
        .expect("resume the stopped mob");
    (host, restored)
}

/// How many times the log recorded `run_id` completing.
fn runs_completed(events: &[AgentEvent], run_id: &meerkat_core::lifecycle::RunId) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(event, AgentEvent::RunCompleted { identity, .. }
                if identity.run_id.as_ref() == Some(run_id))
        })
        .count()
}

/// A Queue-mode host human input whose own turn is in flight (its model
/// call held) when the host stops with `StopOptions::leave_runs_running`,
/// then drains, then shuts down, then restarts over the same durable files.
///
/// The Stop holds the member's run starts and leaves its run running. The
/// drain captures that run while the model call is held and, once it is
/// released, reports it ended as completed. The input is consumed, its
/// waiter gets the model's answer, and the run completes once with no
/// failure, so the Shutdown that follows has nothing to cancel. The restart
/// keeps the consumed row, runs nothing, and a same-key replay answers the
/// recorded result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drain_lets_a_stopped_members_in_flight_human_turn_complete() {
    use meerkat_core::turn_execution_authority::TurnTerminalOutcome;
    use meerkat_runtime::input_state::{InputLifecycleState, InputTerminalOutcome};

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("drain-in-flight")).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let key = delivery("drain-in-flight");
    let interaction_id = interaction(&key);
    let spec = WorkSpec::new(HUMAN, WorkOrigin::External);
    let turn = host
        .handle
        .start_host_human_input_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            spec.clone(),
            HandlingMode::Queue,
            key.clone(),
            deadline(),
        )
        .await
        .expect("queued human admission");
    client.wait_for_requests(requests_before + 1).await;
    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;

    let stop = stop_leaving_runs_running(&host).await;
    let drain = drain_after_snapshot(&host, || client.release(), UNREACHED_DRAIN_BOUND).await;
    let (waiter, answer) = drained_answer(turn).await;
    tokio::time::timeout(WAIT, host.handle.shutdown())
        .await
        .expect("the graceful stop finishes")
        .expect("graceful stop");
    let at_exit = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    let events = stop_events(&root_path, &session_id).await;
    let at_stop = observe_at_stop(waiter, &at_exit, &events, interaction_id);
    let its_run = runs_started_for(&events, interaction_id).first().cloned();
    let completions = its_run
        .as_ref()
        .map(|run_id| runs_completed(&events, run_id));
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let (restarted, restored) = reopen_stopped_host(&root_path, &restarted_client).await;
    let after_restart = observe_after_restart(
        &root_path,
        &restarted,
        &restarted_client,
        &at_exit,
        (&key, &input_id, spec),
        &identity,
        &session_id,
    )
    .await;
    assert_eq!(
        (
            stop.members.get(&identity).cloned(),
            drain.members.get(&identity).cloned(),
            answer,
            completions,
            restored,
            at_stop,
            after_restart,
        ),
        (
            its_run.clone().map(|run_id| crate::MemberStopOutcome {
                run: crate::MemberStopRun::LeftRunning { run_id },
                starts: crate::MemberRunStarts::Held,
            }),
            its_run.clone().map(|run_id| crate::MemberRunDrainOutcome {
                run: crate::MemberRunDrain::Ended {
                    run_id,
                    terminal: Some(TurnTerminalOutcome::Completed),
                },
                starts: Some(crate::MemberRunStarts::Held),
            }),
            Some(format!("executor-answer-{}", requests_before + 1)),
            Some(1),
            MobState::Stopped,
            AtStop {
                waiter: StopAnswer::Succeeded,
                phase: InputLifecycleState::Consumed,
                outcome: Some(InputTerminalOutcome::Consumed),
                attempt_count: 1,
                last_run_id: its_run.clone(),
                payload_kept: false,
                replay_identity_kept: true,
                outbox: false,
                pending_terminal_owner: false,
                runs_started: 1,
                run_failures: Vec::new(),
                interaction_failed_events: 0,
            },
            AfterRestart {
                recovery_count: at_exit.stored.state.recovery_count,
                row_unchanged_by_recovery: true,
                pending_terminal_owner: false,
                replay: StopAnswer::Succeeded,
                model_requests: 0,
                row_unchanged_by_replay: true,
                runs_started: its_run.into_iter().collect(),
                run_failures: 0,
                interaction_failed_events: 0,
            },
        ),
        "the Stop leaves the turn running, the drain waits for it, and it commits its \
         answer; the restart keeps the consumed row and the replay answers it without a run"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// The same in-flight host human input, with its model call held through
/// the whole drain: the drain reports the run still running once its bound
/// has passed, and cancels nothing. The Shutdown that follows, an explicit
/// Stop, cancels the run as it does without a drain, so the input ends
/// exactly as in
/// `queued_human_mid_turn_at_graceful_stop_fails_once_after_restart_and_keeps_row`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drain_at_its_bound_leaves_the_run_to_the_shutdown_cancel() {
    use meerkat_core::event::AgentErrorClass;
    use meerkat_runtime::input_state::{
        InputAbandonReason, InputLifecycleState, InputTerminalOutcome,
    };

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("drain-bound")).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let key = delivery("drain-bound");
    let interaction_id = interaction(&key);
    let spec = WorkSpec::new(HUMAN, WorkOrigin::External);
    let turn = host
        .handle
        .start_host_human_input_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            spec.clone(),
            HandlingMode::Queue,
            key.clone(),
            deadline(),
        )
        .await
        .expect("queued human admission");
    client.wait_for_requests(requests_before + 1).await;
    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;

    let stop = stop_leaving_runs_running(&host).await;
    let started = std::time::Instant::now();
    let drain = tokio::time::timeout(WAIT, host.handle.drain_stopped_runs(started + DRAIN_BOUND))
        .await
        .expect("the drain returns at its bound")
        .expect("drain a stopped mob");
    let waited = started.elapsed();
    tokio::time::timeout(WAIT, host.handle.shutdown_with_report(stop_options()))
        .await
        .expect("the graceful stop finishes")
        .expect("graceful stop");
    let waiter = stop_answer(turn, &session_id).await;
    let at_exit = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    let events = stop_events(&root_path, &session_id).await;
    let at_stop = observe_at_stop(waiter, &at_exit, &events, interaction_id);
    let its_run = runs_started_for(&events, interaction_id).first().cloned();
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let (restarted, restored) = reopen_stopped_host(&root_path, &restarted_client).await;
    let after_restart = observe_after_restart(
        &root_path,
        &restarted,
        &restarted_client,
        &at_exit,
        (&key, &input_id, spec),
        &identity,
        &session_id,
    )
    .await;
    assert_eq!(
        (
            stop.members
                .get(&identity)
                .map(|outcome| outcome.run.clone()),
            drain.members.get(&identity).cloned(),
            waited >= DRAIN_BOUND,
            restored,
            at_stop,
            after_restart,
        ),
        (
            its_run
                .clone()
                .map(|run_id| crate::MemberStopRun::LeftRunning { run_id }),
            its_run.clone().map(|run_id| crate::MemberRunDrainOutcome {
                run: crate::MemberRunDrain::StillRunning { run_id },
                starts: Some(crate::MemberRunStarts::Held),
            }),
            true,
            MobState::Stopped,
            AtStop {
                waiter: StopAnswer::Cancelled,
                phase: InputLifecycleState::Abandoned,
                outcome: Some(InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::Cancelled,
                }),
                attempt_count: 1,
                last_run_id: its_run.clone(),
                payload_kept: false,
                replay_identity_kept: true,
                outbox: false,
                pending_terminal_owner: false,
                runs_started: 1,
                run_failures: vec![(AgentErrorClass::Cancelled, "Run was cancelled".to_owned())],
                interaction_failed_events: 0,
            },
            AfterRestart {
                recovery_count: at_exit.stored.state.recovery_count,
                row_unchanged_by_recovery: true,
                pending_terminal_owner: false,
                replay: StopAnswer::Cancelled,
                model_requests: 0,
                row_unchanged_by_replay: true,
                runs_started: its_run.into_iter().collect(),
                run_failures: 1,
                interaction_failed_events: 0,
            },
        ),
        "the drain reports the run still running only after its bound, and the Shutdown \
         cancels it as without a drain"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// A host human input queued behind the in-flight one when the host stops
/// with `StopOptions::leave_runs_running`: the running turn drains to
/// completion, and the queued input starts no run while the mob is stopped,
/// during the drain or after it. The Shutdown, an explicit Stop, then
/// terminates it exactly as in
/// `queued_human_behind_a_running_turn_at_graceful_stop_is_terminated_once_after_restart_and_keeps_row`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queued_input_starts_no_run_during_the_drain() {
    use meerkat_core::turn_execution_authority::TurnTerminalOutcome;
    use meerkat_runtime::input_state::{
        InputAbandonReason, InputLifecycleState, InputTerminalOutcome,
    };

    const UNREGISTERED: &str = "runtime session unregistered";
    let terminated = StopAnswer::RuntimeTerminated {
        reason: UNREGISTERED.to_owned(),
        kind: meerkat_core::TurnTerminalCauseKind::FatalFailure,
        detail: Some(UNREGISTERED.to_owned()),
    };
    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("drain-queued")).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let running_key = delivery("drain-running-ahead");
    let running = host
        .handle
        .start_host_human_input_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            WorkSpec::new(HUMAN, WorkOrigin::External),
            HandlingMode::Queue,
            running_key.clone(),
            deadline(),
        )
        .await
        .expect("running human admission");
    client.wait_for_requests(requests_before + 1).await;
    let key = delivery("drain-queued-behind");
    let interaction_id = interaction(&key);
    let spec = WorkSpec::new(QUEUED_HUMAN, WorkOrigin::External);
    let queued = host
        .handle
        .start_host_human_input_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            spec.clone(),
            HandlingMode::Queue,
            key.clone(),
            deadline(),
        )
        .await
        .expect("queued human admission");
    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;

    stop_leaving_runs_running(&host).await;
    let drain = drain_after_snapshot(&host, || client.release(), UNREACHED_DRAIN_BOUND).await;
    let (running_waiter, _) = drained_answer(running).await;
    let after_drain = (
        stop_row(&host.runtime_store, &session_id, &key, &input_id)
            .await
            .stored
            .seed
            .phase,
        runs_started_for(&stop_events(&root_path, &session_id).await, interaction_id).len(),
        client.requests().len() - requests_before,
    );
    tokio::time::timeout(WAIT, host.handle.shutdown_with_report(stop_options()))
        .await
        .expect("the graceful stop finishes")
        .expect("graceful stop");
    let waiter = stop_answer(queued, &session_id).await;
    let at_exit = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    let events = stop_events(&root_path, &session_id).await;
    let at_stop = observe_at_stop(waiter, &at_exit, &events, interaction_id);
    let running_run = runs_started_for(&events, interaction(&running_key))
        .first()
        .cloned();
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let (restarted, restored) = reopen_stopped_host(&root_path, &restarted_client).await;
    let after_restart = observe_after_restart(
        &root_path,
        &restarted,
        &restarted_client,
        &at_exit,
        (&key, &input_id, spec),
        &identity,
        &session_id,
    )
    .await;
    assert_eq!(
        (
            drain
                .members
                .get(&identity)
                .map(|outcome| outcome.run.clone()),
            running_waiter,
            after_drain,
            restored,
            at_stop,
            after_restart,
        ),
        (
            running_run.map(|run_id| crate::MemberRunDrain::Ended {
                run_id,
                terminal: Some(TurnTerminalOutcome::Completed),
            }),
            StopAnswer::Succeeded,
            (InputLifecycleState::Queued, 0, 1),
            MobState::Stopped,
            AtStop {
                waiter: terminated.clone(),
                phase: InputLifecycleState::Abandoned,
                outcome: Some(InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::Stopped,
                }),
                attempt_count: 0,
                last_run_id: None,
                payload_kept: false,
                replay_identity_kept: true,
                outbox: false,
                pending_terminal_owner: false,
                runs_started: 0,
                run_failures: Vec::new(),
                interaction_failed_events: 0,
            },
            AfterRestart {
                recovery_count: at_exit.stored.state.recovery_count,
                row_unchanged_by_recovery: true,
                pending_terminal_owner: false,
                replay: terminated,
                model_requests: 0,
                row_unchanged_by_replay: true,
                runs_started: Vec::new(),
                run_failures: 0,
                interaction_failed_events: 0,
            },
        ),
        "the running turn drains to completion while the queued input stays queued and \
         starts no run; the Shutdown terminates it without a run"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// One input of the crash-mid-drain row across the crash: its phase when the
/// process died and when the restarted process opened, whether the reopen
/// changed its row, and what the resumed mob then did with it.
#[derive(Debug, PartialEq)]
struct AcrossTheCrash {
    phase_at_crash: meerkat_runtime::input_state::InputLifecycleState,
    phase_at_reopen: meerkat_runtime::input_state::InputLifecycleState,
    row_unchanged_at_reopen: bool,
    recovery_count_at_reopen: u32,
    replay: StopAnswer,
    phase_after_resume: meerkat_runtime::input_state::InputLifecycleState,
    runs_started: usize,
    runs_completed: usize,
    run_failures: usize,
}

/// A host that dies in the middle of a drain: it stopped with
/// `StopOptions::leave_runs_running`, its member's turn is in flight (the
/// model call held) with a second input queued behind it, and the drain is
/// waiting for that turn when the process dies.
///
/// The drain writes nothing, so the restart finds what a crash between a
/// Stop and its Shutdown always left. The mob comes back Stopped. The queued
/// input is still Queued with its row unchanged, and no run started for it.
/// The interrupted turn's input is still Staged with its row unchanged too:
/// a stopped start materializes no member (the on-demand default), so the
/// recovery that puts a run a crash cut off back in Queued waits for the
/// resume's wake of the member. Once the restarted mob resumes, the
/// interrupted input runs to completion first and the queued one after it,
/// each exactly once, and a same-key replay of each answers its result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crash_mid_drain_leaves_the_queued_input_for_the_restarted_mob() {
    use meerkat_runtime::input_state::InputLifecycleState;

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    // The first process runs on its own runtime, so the crash below drops
    // every task it spawned: the actor, the held turn and the drain.
    let incarnation = CrashableMobIncarnation::new();
    let (host, member, requests_before, inputs, captured, drain) = {
        let root_path = root_path.clone();
        let client = client.clone();
        incarnation
            .install(async move {
                let host = open_stop_host(&root_path, &client, Some("drain-crash")).await;
                let (identity, session_id, entry) = spawn_stop_member(&host).await;
                let comms = host
                    .service
                    .comms_runtime(&session_id)
                    .await
                    .map(|runtime| Arc::downgrade(&runtime))
                    .expect("the member's comms runtime");
                client.block();
                let requests_before = client.requests().len();
                let mut inputs = Vec::new();
                for (label, text) in [
                    ("drain-crash-running", HUMAN),
                    ("drain-crash-queued", QUEUED_HUMAN),
                ] {
                    let key = delivery(label);
                    let spec = WorkSpec::new(text, WorkOrigin::External);
                    let waiter = host
                        .handle
                        .start_host_human_input_bounded(
                            entry.agent_runtime_id.clone(),
                            entry.fence_token,
                            spec.clone(),
                            HandlingMode::Queue,
                            key.clone(),
                            deadline(),
                        )
                        .await
                        .expect("human admission");
                    if inputs.is_empty() {
                        client.wait_for_requests(requests_before + 1).await;
                    }
                    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;
                    inputs.push((key, input_id, spec, waiter));
                }
                stop_leaving_runs_running(&host).await;
                let snapshot = host
                    .handle
                    .drain_runs_snapshot()
                    .await
                    .expect("a stopped mob's drain snapshot");
                let captured: Vec<_> = snapshot
                    .members
                    .iter()
                    .map(|member| (member.identity.clone(), member.target.clone()))
                    .collect();
                let mut drain = tokio::spawn(crate::runtime::run_drain::drain(
                    snapshot,
                    meerkat_core::time_compat::Instant::now() + UNREACHED_DRAIN_BOUND,
                ));
                assert!(
                    tokio::time::timeout(Duration::from_millis(300), &mut drain)
                        .await
                        .is_err(),
                    "the drain waits for the held turn"
                );
                (
                    host,
                    (identity, session_id, comms),
                    requests_before,
                    inputs,
                    captured,
                    drain,
                )
            })
            .await
    };
    let (identity, session_id, comms) = member;
    let mut at_crash = Vec::new();
    for (key, input_id, _, _) in &inputs {
        at_crash.push(stop_row(&host.runtime_store, &session_id, key, input_id).await);
    }
    let requests_at_crash = client.requests().len() - requests_before;
    let events_at_crash = stop_events(&root_path, &session_id).await.len();

    incarnation.crash().await;
    // The kill landed: the actor is gone and the drain never returned. Only
    // the durable runtime store survives a crash: the incarnation's handle,
    // its runtime adapter and its session service are in-memory state, and
    // they go with it.
    let StopHost {
        handle,
        runtime_store,
        adapter,
        service,
    } = host;
    let actor_gone = handle.command_tx.is_closed();
    let drain_cancelled = drain.await.err().is_some_and(|error| error.is_cancelled());
    let mob_id = handle.mob_id().clone();
    drop((handle, adapter, service));
    let inputs: Vec<_> = inputs
        .into_iter()
        .map(|(key, input_id, spec, waiter)| {
            drop(waiter);
            (key, input_id, spec)
        })
        .collect();
    // A process exit frees the member's comms name. The crashed
    // incarnation's in-memory session state outlives its runtime in this
    // process, so the restart retires its route itself. The Stop already
    // closed that route for peer ingress, and a closed route holds no name:
    // the retire only forgets the closed route and answers that it freed no
    // name. That writes nothing: both rows and the event log read the same
    // as at the crash.
    let name_freed = comms.upgrade().map(|runtime| runtime.retire_inproc_route());
    let mut unchanged_since_crash = Vec::new();
    for (index, (key, input_id, _)) in inputs.iter().enumerate() {
        let row = stop_row(&runtime_store, &session_id, key, input_id).await;
        unchanged_since_crash.push(row.digest == at_crash[index].digest);
    }
    unchanged_since_crash.push(stop_events(&root_path, &session_id).await.len() == events_at_crash);
    drop(runtime_store);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    let restored = restarted
        .handle
        .status()
        .await
        .expect("the reopened mob's state");
    let mut at_reopen = Vec::new();
    for (key, input_id, _) in &inputs {
        at_reopen.push(stop_row(&restarted.runtime_store, &session_id, key, input_id).await);
    }
    tokio::time::timeout(WAIT, restarted.handle.resume())
        .await
        .expect("the resume finishes")
        .expect("resume the stopped mob");
    let entry = restarted
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member survives the restart");
    let mut replays = Vec::new();
    for (key, _, spec) in &inputs {
        let replay = restarted
            .handle
            .start_host_human_input_bounded(
                entry.agent_runtime_id.clone(),
                entry.fence_token,
                spec.clone(),
                HandlingMode::Queue,
                key.clone(),
                deadline(),
            )
            .await
            .expect("same-key replay admission");
        replays.push(drained_answer(replay).await.0);
    }
    // A replay answers once its input settles, which can be before the run's
    // RunCompleted reaches the event log. The log is complete once the
    // shutdown returns, so it is read after it, as in the other Stop rows.
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
    let events = stop_events(&root_path, &session_id).await;
    let mut across = Vec::new();
    for (index, (key, input_id, _)) in inputs.iter().enumerate() {
        let after = stop_row(&restarted.runtime_store, &session_id, key, input_id).await;
        let runs = runs_started_for(&events, interaction(key));
        across.push(AcrossTheCrash {
            phase_at_crash: at_crash[index].stored.seed.phase,
            phase_at_reopen: at_reopen[index].stored.seed.phase,
            row_unchanged_at_reopen: at_reopen[index].digest == at_crash[index].digest,
            recovery_count_at_reopen: at_reopen[index].stored.state.recovery_count,
            replay: replays[index].clone(),
            phase_after_resume: after.stored.seed.phase,
            runs_started: runs.len(),
            runs_completed: runs.iter().map(|run| runs_completed(&events, run)).sum(),
            run_failures: runs
                .iter()
                .map(|run| run_failures(&events, run).len())
                .sum(),
        });
    }
    let interrupted_run = runs_started_for(&events, interaction(&inputs[0].0))
        .first()
        .cloned()
        .expect("the interrupted turn's run is in the log");
    let requests: Vec<_> = restarted_client
        .requests()
        .iter()
        .map(|request| {
            (
                requests_carrying(std::slice::from_ref(request), HUMAN),
                requests_carrying(std::slice::from_ref(request), QUEUED_HUMAN),
            )
        })
        .collect();
    assert_eq!(
        (
            (actor_gone, drain_cancelled, captured, requests_at_crash),
            (name_freed, unchanged_since_crash),
            restored,
            across,
            requests,
        ),
        (
            (
                true,
                true,
                vec![(
                    identity.clone(),
                    crate::runtime::run_drain::DrainTarget::Wait {
                        session_id: session_id.clone(),
                        run_id: interrupted_run,
                    },
                )],
                1,
            ),
            (Some(false), vec![true, true, true]),
            MobState::Stopped,
            vec![
                AcrossTheCrash {
                    phase_at_crash: InputLifecycleState::Staged,
                    phase_at_reopen: InputLifecycleState::Staged,
                    row_unchanged_at_reopen: true,
                    recovery_count_at_reopen: 0,
                    replay: StopAnswer::Succeeded,
                    phase_after_resume: InputLifecycleState::Consumed,
                    runs_started: 2,
                    runs_completed: 1,
                    run_failures: 0,
                },
                AcrossTheCrash {
                    phase_at_crash: InputLifecycleState::Queued,
                    phase_at_reopen: InputLifecycleState::Queued,
                    row_unchanged_at_reopen: true,
                    recovery_count_at_reopen: 0,
                    replay: StopAnswer::Succeeded,
                    phase_after_resume: InputLifecycleState::Consumed,
                    runs_started: 1,
                    runs_completed: 1,
                    run_failures: 0,
                },
            ],
            vec![(1, 0), (1, 1)],
        ),
        "a crash while the drain waits leaves the Stop's state: the queued input stays Queued \
         and runs once after the restart, after the interrupted input runs again"
    );
}

/// The runtime a host ran its mob on stops in the middle of a member's turn
/// (the model call held), with a second input queued behind it, and the
/// process keeps running.
///
/// The member's session task ran on the host's runtime and ended with it,
/// but its runtime loop runs on a runtime the process owns, which outlives
/// the host's. A runtime stop is crash-equivalent: within a bound the loop
/// stops serving the session without writing anything for it (no failed
/// run, no replay, no stop terminal), and leaves the session's registration
/// to a cold reload. Both rows and the event log read as they did at the
/// stop. A new host, on another runtime of the same process, then opens the
/// mob over the same store: the interrupted input runs to completion once
/// and the queued one after it, and a same-key replay of each answers its
/// result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_runtime_that_stops_mid_turn_leaves_the_run_for_the_next_start() {
    use meerkat_runtime::input_state::InputLifecycleState;

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    // The first host runs the mob on a runtime of its own, so stopping it
    // drops every task the host spawned: the mob actor, the member's session
    // task and its turn.
    let incarnation = CrashableMobIncarnation::new();
    let (host, member, requests_before, inputs) = {
        let root_path = root_path.clone();
        let client = client.clone();
        incarnation
            .install(async move {
                let host = open_stop_host(&root_path, &client, Some("runtime-stop")).await;
                let (identity, session_id, entry) = spawn_stop_member(&host).await;
                let comms = host
                    .service
                    .comms_runtime(&session_id)
                    .await
                    .map(|runtime| Arc::downgrade(&runtime))
                    .expect("the member's comms runtime");
                client.block();
                let requests_before = client.requests().len();
                let mut inputs = Vec::new();
                for (label, text) in [
                    ("runtime-stop-running", HUMAN),
                    ("runtime-stop-queued", QUEUED_HUMAN),
                ] {
                    let key = delivery(label);
                    let spec = WorkSpec::new(text, WorkOrigin::External);
                    let waiter = host
                        .handle
                        .start_host_human_input_bounded(
                            entry.agent_runtime_id.clone(),
                            entry.fence_token,
                            spec.clone(),
                            HandlingMode::Queue,
                            key.clone(),
                            deadline(),
                        )
                        .await
                        .expect("human admission");
                    if inputs.is_empty() {
                        client.wait_for_requests(requests_before + 1).await;
                    }
                    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;
                    inputs.push((key, input_id, spec, waiter));
                }
                (host, (identity, session_id, comms), requests_before, inputs)
            })
            .await
    };
    let (identity, session_id, comms) = member;
    let mut at_stop = Vec::new();
    for (key, input_id, _, _) in &inputs {
        at_stop.push(stop_row(&host.runtime_store, &session_id, key, input_id).await);
    }
    let requests_at_stop = client.requests().len() - requests_before;
    let events_at_stop = stop_events(&root_path, &session_id).await.len();

    incarnation.crash().await;
    let StopHost {
        handle,
        runtime_store,
        adapter,
        service,
    } = host;
    let actor_gone = handle.command_tx.is_closed();
    // The member's runtime loop outlives the host's runtime. It stops serving
    // the session within the bound, and the session's registration waits for
    // a cold reload from the store instead of a terminal.
    let loop_ended = tokio::time::timeout(WAIT, async {
        while adapter
            .session_has_live_executor_attachment(&session_id)
            .await
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    let reload_required = adapter
        .durability_reload_required(&session_id)
        .await
        .is_some();
    let inputs: Vec<_> = inputs
        .into_iter()
        .map(|(key, input_id, spec, waiter)| {
            drop(waiter);
            (key, input_id, spec)
        })
        .collect();
    let mut unchanged_since_stop = Vec::new();
    for (index, (key, input_id, _)) in inputs.iter().enumerate() {
        let row = stop_row(&runtime_store, &session_id, key, input_id).await;
        unchanged_since_stop.push(row.digest == at_stop[index].digest);
    }
    unchanged_since_stop.push(stop_events(&root_path, &session_id).await.len() == events_at_stop);
    // A process exit frees the member's comms name. This process keeps the
    // stopped host's in-memory session state, so the route is retired here
    // for the next host, as the crash rows do.
    if let Some(runtime) = comms.upgrade() {
        runtime.retire_inproc_route();
    }
    let mob_id = handle.mob_id().clone();
    drop((handle, adapter, service, runtime_store));
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    tokio::time::timeout(WAIT, restarted.handle.ensure_member_live(&identity))
        .await
        .expect("the member's wake finishes")
        .expect("wake the member on the new host");
    let entry = restarted
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member survives the stop");
    let mut replays = Vec::new();
    for (key, _, spec) in &inputs {
        let replay = restarted
            .handle
            .start_host_human_input_bounded(
                entry.agent_runtime_id.clone(),
                entry.fence_token,
                spec.clone(),
                HandlingMode::Queue,
                key.clone(),
                deadline(),
            )
            .await
            .expect("same-key replay admission");
        replays.push(drained_answer(replay).await.0);
    }
    // The log is complete once the shutdown returns (see the crash row).
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
    let events = stop_events(&root_path, &session_id).await;
    let mut across = Vec::new();
    for (index, (key, input_id, _)) in inputs.iter().enumerate() {
        let after = stop_row(&restarted.runtime_store, &session_id, key, input_id).await;
        let runs = runs_started_for(&events, interaction(key));
        across.push((
            at_stop[index].stored.seed.phase,
            replays[index].clone(),
            after.stored.seed.phase,
            runs.len(),
            runs.iter()
                .map(|run| runs_completed(&events, run))
                .sum::<usize>(),
            runs.iter()
                .map(|run| run_failures(&events, run).len())
                .sum::<usize>(),
        ));
    }
    let requests: Vec<_> = restarted_client
        .requests()
        .iter()
        .map(|request| {
            (
                requests_carrying(std::slice::from_ref(request), HUMAN),
                requests_carrying(std::slice::from_ref(request), QUEUED_HUMAN),
            )
        })
        .collect();
    assert_eq!(
        (
            (actor_gone, requests_at_stop),
            (loop_ended, reload_required, unchanged_since_stop),
            across,
            requests,
        ),
        (
            (true, 1),
            (true, true, vec![true, true, true]),
            vec![
                (
                    InputLifecycleState::Staged,
                    StopAnswer::Succeeded,
                    InputLifecycleState::Consumed,
                    2,
                    1,
                    0,
                ),
                (
                    InputLifecycleState::Queued,
                    StopAnswer::Succeeded,
                    InputLifecycleState::Consumed,
                    1,
                    1,
                    0,
                ),
            ],
            vec![(1, 0), (1, 1)],
        ),
        "a host runtime that stops mid-turn is a crash for its member: the surviving runtime \
         loop ends without writing, and the next host runs the interrupted input once and the \
         queued one after it"
    );
}

/// The model the per-turn override of
/// [`an_input_to_an_idle_member_after_its_host_runtime_stopped_waits_for_the_next_start`]
/// names: an OpenAI model other than the member's own, so the override
/// reconfigures the session's live LLM client.
const OVERRIDE_MODEL: &str = "gpt-5.4";
/// The prompt of that row's input.
const OVERRIDE_PROMPT: &str = "Answer this one with the overriding model.";

/// The runtime a host ran its mob on stops while a member is idle, and the
/// process keeps running. Another part of the process then admits an input
/// to the member's runtime, with a per-turn model override.
///
/// The member's session task ended with the stopped runtime, so the input
/// cannot run there: the stop is a crash for the member, not a failure of
/// the input. Applying the override reconfigures the live actor ahead of the
/// turn; without a check before that step, the runtime loop would record the
/// input as a failed run, retry it and abandon it. With the check, the loop
/// stops serving the session within its bound, the registration waits for a
/// reload, the input stays non-terminal and the event log is unchanged, and
/// a new host on another runtime of the same process runs the input once,
/// with its override, and records no failed run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_input_to_an_idle_member_after_its_host_runtime_stopped_waits_for_the_next_start() {
    use meerkat_runtime::input_state::InputLifecycleState;

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    // The first host runs the mob on a runtime of its own.
    let incarnation = CrashableMobIncarnation::new();
    let (host, member) = {
        let root_path = root_path.clone();
        let client = client.clone();
        incarnation
            .install(async move {
                let host = open_stop_host_composed(
                    &root_path,
                    &client,
                    Some("idle-runtime-stop"),
                    &[],
                    true,
                )
                .await;
                let (identity, session_id, _entry) = spawn_stop_member(&host).await;
                let comms = host
                    .service
                    .comms_runtime(&session_id)
                    .await
                    .map(|runtime| Arc::downgrade(&runtime))
                    .expect("the member's comms runtime");
                (host, (identity, session_id, comms))
            })
            .await
    };
    let (identity, session_id, comms) = member;
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let events_at_stop = stop_events(&root_path, &session_id).await.len();

    incarnation.crash().await;
    let StopHost {
        handle,
        runtime_store,
        adapter,
        service,
    } = host;
    // Another part of the process, which outlives the host's runtime, sends
    // the idle member an input with a per-turn model override.
    let prompt = meerkat_runtime::input::PromptInput::new(
        OVERRIDE_PROMPT,
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                    OVERRIDE_MODEL,
                )),
                ..Default::default()
            },
        ),
    );
    let input_id = prompt.header.id.clone();
    let (_admitted, waiter) = adapter
        .accept_input_with_completion(&session_id, meerkat_runtime::input::Input::Prompt(prompt))
        .await
        .expect("the input is admitted");
    let loop_ended = tokio::time::timeout(WAIT, async {
        while adapter
            .session_has_live_executor_attachment(&session_id)
            .await
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    let reload_required = adapter
        .durability_reload_required(&session_id)
        .await
        .is_some();
    let phase_after_stop = runtime_store
        .load_input_state(&runtime_id, &input_id)
        .await
        .expect("runtime-owned input read")
        .expect("the input keeps its durable row")
        .seed
        .phase;
    let input_left_open = matches!(
        phase_after_stop,
        InputLifecycleState::Queued | InputLifecycleState::Staged
    );
    let events_unchanged = stop_events(&root_path, &session_id).await.len() == events_at_stop;
    drop(waiter);
    // A process exit frees the member's comms name; this process keeps the
    // stopped host's in-memory session state, so the route is retired here
    // for the next host, as the crash rows do.
    if let Some(runtime) = comms.upgrade() {
        runtime.retire_inproc_route();
    }
    let mob_id = handle.mob_id().clone();
    drop((handle, adapter, service, runtime_store));
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host_composed(&root_path, &restarted_client, None, &[], true).await;
    tokio::time::timeout(WAIT, restarted.handle.ensure_member_live(&identity))
        .await
        .expect("the member's wake finishes")
        .expect("wake the member on the new host");
    let consumed = tokio::time::timeout(WAIT, async {
        loop {
            let phase = restarted
                .runtime_store
                .load_input_state(&runtime_id, &input_id)
                .await
                .expect("runtime-owned input read")
                .expect("the input keeps its durable row")
                .seed
                .phase;
            if phase == InputLifecycleState::Consumed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    // The log is complete once the shutdown returns (see the crash row).
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
    let run_failures = stop_events(&root_path, &session_id)
        .await
        .iter()
        .filter(|event| matches!(event, AgentEvent::RunFailed { .. }))
        .count();
    assert_eq!(
        (
            (
                loop_ended,
                reload_required,
                input_left_open,
                events_unchanged
            ),
            (
                consumed,
                requests_carrying(&client.requests(), OVERRIDE_PROMPT),
                requests_carrying(&restarted_client.requests(), OVERRIDE_PROMPT),
                run_failures,
            ),
        ),
        ((true, true, true, true), (true, 0, 1, 0)),
        "an input that reaches an idle member after its host runtime stopped waits for the next \
         start: the surviving runtime loop records no failed run, and the next host runs the \
         input once"
    );
}

/// A run that a requeue stop kept open, its input carrying a per-turn model
/// override, is resumed by a host that holds the member's run starts, and
/// that host's runtime stops before the hold is released; the process keeps
/// running.
///
/// The runtime loop skips the pre-dequeue step for a resumed run, and the
/// member executor runs no per-turn LLM reconfigure for one: the run
/// continues under the model its first dispatch applied and saved. So no
/// step ahead of the service apply needs the lost actor, and the service
/// apply's own check would degrade the registration. The member executor's
/// check at the top of its apply still runs first and degrades it; it stays
/// as a backstop, and this row no longer isolates it. Once another part of
/// the process releases the hold, the loop stops serving the session within
/// its bound, the registration waits for a reload, the run's input stays
/// bound to the open run, and a third host resumes the run once, on the
/// overriding model, and records no failed run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resumed_run_after_its_host_runtime_stopped_waits_for_the_next_start() {
    use meerkat_runtime::input_state::InputLifecycleState;

    let (_root, root_path) = stop_root();
    // The first process runs the turn, with the override, to a committed
    // round and keeps it open across a requeue stop.
    let client = ScriptedClient::new(true);
    let host =
        open_stop_host_composed(&root_path, &client, Some("resumed-runtime-stop"), &[], true).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    client.block();
    client.call_a_tool_next();
    let requests_before = client.requests().len();
    let running_key = delivery("resumed-runtime-stop-running");
    let running = host
        .handle
        .start_host_human_input_with_options_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            WorkSpec::new(HUMAN, WorkOrigin::External),
            HandlingMode::Queue,
            crate::MemberTurnOptions {
                model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                    OVERRIDE_MODEL,
                )),
                ..Default::default()
            },
            running_key.clone(),
            deadline(),
        )
        .await
        .expect("running human admission");
    client.wait_for_requests(requests_before + 2).await;
    let running_id = admitted_input_id(&host.runtime_store, &session_id, &running_key).await;
    tokio::time::timeout(
        WAIT,
        host.handle
            .shutdown_with_report(requeue_options(crate::RequeueMaxAge::DEFAULT)),
    )
    .await
    .expect("the requeue stop finishes")
    .expect("requeue stop");
    let kept_across_stop = stop_answer(running, &session_id).await
        == StopAnswer::RequeuedAcrossStop(running_id.clone());
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    // The second process holds the member's run starts, on a runtime of its
    // own, with the member's actor live there.
    let held_client = ScriptedClient::new(true);
    let incarnation = CrashableMobIncarnation::new();
    let (held, comms) = {
        let root_path = root_path.clone();
        let held_client = held_client.clone();
        let identity = identity.clone();
        let session_id = session_id.clone();
        incarnation
            .install(async move {
                let held = open_stop_host_composed(
                    &root_path,
                    &held_client,
                    None,
                    std::slice::from_ref(&identity),
                    true,
                )
                .await;
                let mut parks = held.adapter.run_start_held_parks();
                tokio::time::timeout(WAIT, parks.wait_for(|parks| *parks >= 1))
                    .await
                    .expect("the member's runtime loop parks on the hold")
                    .expect("park signal");
                tokio::time::timeout(WAIT, held.handle.ensure_member_live(&identity))
                    .await
                    .expect("the member's wake finishes")
                    .expect("the held member's actor is live");
                let comms = held
                    .service
                    .comms_runtime(&session_id)
                    .await
                    .map(|runtime| Arc::downgrade(&runtime));
                (held, comms)
            })
            .await
    };
    let resumable_at_stop = held.adapter.resumable_run_id(&session_id).await.is_some();
    let actor_at_stop = held
        .service
        .live_session_actor_registered(&session_id)
        .await;

    incarnation.crash().await;
    let StopHost {
        handle,
        runtime_store,
        adapter,
        service,
    } = held;
    // Another part of the process releases the hold; the member's runtime
    // loop resumes the kept run.
    adapter
        .release_run_starts(
            &session_id,
            meerkat_runtime::RunStartHoldReason::ToolsNotPublished,
        )
        .await
        .expect("release the held run starts");
    let loop_ended = tokio::time::timeout(WAIT, async {
        while adapter
            .session_has_live_executor_attachment(&session_id)
            .await
        {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    let reload_required = adapter
        .durability_reload_required(&session_id)
        .await
        .is_some();
    let running_phase = runtime_store
        .load_input_state(&runtime_id, &running_id)
        .await
        .expect("runtime-owned input read")
        .expect("the input keeps its durable row")
        .seed
        .phase;
    let running_left_open = matches!(
        running_phase,
        InputLifecycleState::Staged
            | InputLifecycleState::Applied
            | InputLifecycleState::AppliedPendingConsumption
    );
    if let Some(runtime) = comms.and_then(|comms| comms.upgrade()) {
        runtime.retire_inproc_route();
    }
    drop((handle, adapter, service, runtime_store));
    await_supervisor_route_release(&mob_id).await;

    // The third process resumes the run once.
    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host_composed(&root_path, &restarted_client, None, &[], true).await;
    tokio::time::timeout(WAIT, restarted.handle.ensure_member_live(&identity))
        .await
        .expect("the member's wake finishes")
        .expect("wake the member on the third host");
    let consumed = tokio::time::timeout(WAIT, async {
        loop {
            let phase = restarted
                .runtime_store
                .load_input_state(&runtime_id, &running_id)
                .await
                .expect("runtime-owned input read")
                .expect("the input keeps its durable row")
                .seed
                .phase;
            if phase == InputLifecycleState::Consumed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    // The log is complete once the shutdown returns (see the crash row).
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
    let resumed_requests = restarted_client.requests();
    let run_failures = stop_events(&root_path, &session_id)
        .await
        .iter()
        .filter(|event| matches!(event, AgentEvent::RunFailed { .. }))
        .count();
    assert_eq!(
        (
            (kept_across_stop, resumable_at_stop, actor_at_stop),
            (loop_ended, reload_required, running_left_open),
            (consumed, run_failures),
            (
                resumed_requests
                    .first()
                    .map(|request| request.model.clone()),
                requests_carrying(&resumed_requests, HUMAN),
            ),
        ),
        (
            (true, true, true),
            (true, true, true),
            (true, 0),
            (Some(OVERRIDE_MODEL.to_string()), 1),
        ),
        "a resumed run after the host runtime stopped waits for the next start: the member \
         executor's check leaves it to the next registration, and the third host resumes it \
         once, on the overriding model"
    );
}

/// A run that a requeue stop kept open, its input carrying a per-turn model
/// override, resumes once at the next start, on the model the override
/// selected. No host runtime stops here.
///
/// The override was applied, and the session saved with its model, before
/// the run's first round commit, so the resumed run continues under it. The
/// member executor must not apply it again ahead of the resumed dispatch:
/// the resumed actor continues the run's provisional tail, which the
/// reconfigure's live reads refuse until the run commits, so the dispatch
/// would be refused before its turn on every attempt and the input would
/// never be consumed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kept_run_with_a_model_override_resumes_once_at_the_next_start() {
    use meerkat_runtime::input_state::InputLifecycleState;

    let (_root, root_path) = stop_root();
    // The first process runs the turn, with the override, to a committed
    // round and keeps it open across a requeue stop.
    let client = ScriptedClient::new(true);
    let host =
        open_stop_host_composed(&root_path, &client, Some("kept-override-run"), &[], true).await;
    let mob_id = host.handle.mob_id().clone();
    let (identity, session_id, entry) = spawn_stop_member(&host).await;
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    client.block();
    client.call_a_tool_next();
    let requests_before = client.requests().len();
    let running_key = delivery("kept-override-run-running");
    let running = host
        .handle
        .start_host_human_input_with_options_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            WorkSpec::new(HUMAN, WorkOrigin::External),
            HandlingMode::Queue,
            crate::MemberTurnOptions {
                model: Some(meerkat_core::lifecycle::run_primitive::ModelId::new(
                    OVERRIDE_MODEL,
                )),
                ..Default::default()
            },
            running_key.clone(),
            deadline(),
        )
        .await
        .expect("running human admission");
    client.wait_for_requests(requests_before + 2).await;
    let running_id = admitted_input_id(&host.runtime_store, &session_id, &running_key).await;
    tokio::time::timeout(
        WAIT,
        host.handle
            .shutdown_with_report(requeue_options(crate::RequeueMaxAge::DEFAULT)),
    )
    .await
    .expect("the requeue stop finishes")
    .expect("requeue stop");
    let kept_across_stop = stop_answer(running, &session_id).await
        == StopAnswer::RequeuedAcrossStop(running_id.clone());
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    // The next process resumes the kept run.
    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host_composed(&root_path, &restarted_client, None, &[], true).await;
    tokio::time::timeout(WAIT, restarted.handle.ensure_member_live(&identity))
        .await
        .expect("the member's wake finishes")
        .expect("wake the member on the next host");
    let consumed = tokio::time::timeout(WAIT, async {
        loop {
            let phase = restarted
                .runtime_store
                .load_input_state(&runtime_id, &running_id)
                .await
                .expect("runtime-owned input read")
                .expect("the input keeps its durable row")
                .seed
                .phase;
            if phase == InputLifecycleState::Consumed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    // The log is complete once the shutdown returns (see the crash row).
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
    let resumed_requests = restarted_client.requests();
    let run_failures = stop_events(&root_path, &session_id)
        .await
        .iter()
        .filter(|event| matches!(event, AgentEvent::RunFailed { .. }))
        .count();
    assert_eq!(
        (
            (kept_across_stop, consumed),
            (
                resumed_requests
                    .first()
                    .map(|request| request.model.clone()),
                requests_carrying(&resumed_requests, HUMAN),
            ),
            run_failures,
        ),
        ((true, true), (Some(OVERRIDE_MODEL.to_string()), 1), 0),
        "a kept run whose input carries a per-turn model override resumes once at the next \
         start, on the overriding model, and records no failed run"
    );
}

/// A plain Stop, without `StopOptions::leave_runs_running`, still cancels an
/// autonomous member's in-flight turn at its next boundary and waits for it
/// to end: the default Stop is unchanged. The Stop dispatches its cancel
/// before the held model call is released, so the cancel reaches the run
/// first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plain_stop_still_cancels_an_autonomous_members_in_flight_turn() {
    use meerkat_core::event::AgentErrorClass;
    use meerkat_runtime::input_state::{
        InputAbandonReason, InputLifecycleState, InputTerminalOutcome,
    };

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("plain-stop-cancels")).await;
    let (identity, session_id, entry) = spawn_stop_member(&host).await;

    client.block();
    let requests_before = client.requests().len();
    let key = delivery("plain-stop-in-flight");
    let interaction_id = interaction(&key);
    let turn = host
        .handle
        .start_host_human_input_bounded(
            entry.agent_runtime_id.clone(),
            entry.fence_token,
            WorkSpec::new(HUMAN, WorkOrigin::External),
            HandlingMode::Queue,
            key.clone(),
            deadline(),
        )
        .await
        .expect("queued human admission");
    client.wait_for_requests(requests_before + 1).await;
    let input_id = admitted_input_id(&host.runtime_store, &session_id, &key).await;

    let (reached, let_cancel_run, answered) =
        crate::runtime::provisioner::arm_stop_member_cancel_for_test(session_id.clone());
    let stop = tokio::spawn({
        let handle = host.handle.clone();
        async move { handle.stop().await }
    });
    tokio::time::timeout(WAIT, reached)
        .await
        .expect("the Stop reaches the member's cancel")
        .expect("the armed cancel hook");
    let_cancel_run
        .send(())
        .expect("the Stop waits at the member's cancel");
    // Resolves when the member's stop returns, its cancel dispatched.
    let _ = tokio::time::timeout(WAIT, answered)
        .await
        .expect("the member's cancel is dispatched");
    client.release();
    let report = tokio::time::timeout(WAIT, stop)
        .await
        .expect("the Stop finishes once the cancelled turn ends")
        .expect("stop task")
        .expect("plain stop");
    let waiter = stop_answer(turn, &session_id).await;
    let row = stop_row(&host.runtime_store, &session_id, &key, &input_id).await;
    let events = stop_events(&root_path, &session_id).await;
    let at_stop = observe_at_stop(waiter, &row, &events, interaction_id);
    let its_run = runs_started_for(&events, interaction_id).first().cloned();
    assert_eq!(
        (
            report
                .members
                .get(&identity)
                .map(|outcome| outcome.run.clone()),
            at_stop,
        ),
        (
            its_run
                .clone()
                .map(|run_id| crate::MemberStopRun::CancelledAtBoundary { run_id }),
            AtStop {
                waiter: StopAnswer::Cancelled,
                phase: InputLifecycleState::Abandoned,
                outcome: Some(InputTerminalOutcome::Abandoned {
                    reason: InputAbandonReason::Cancelled,
                }),
                attempt_count: 1,
                last_run_id: its_run,
                payload_kept: false,
                replay_identity_kept: true,
                outbox: false,
                pending_terminal_owner: false,
                runs_started: 1,
                run_failures: vec![(AgentErrorClass::Cancelled, "Run was cancelled".to_owned())],
                interaction_failed_events: 0,
            },
        ),
        "a plain Stop cancels the autonomous member's turn at its boundary"
    );
    tokio::time::timeout(WAIT, host.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// An autonomous member whose current run is its kickoff, its model call
/// held, when the host stops with `StopOptions::leave_runs_running`: the
/// Stop leaves the kickoff run running, the drain reports it still running
/// once its bound has passed, and the Shutdown that follows, an explicit
/// `QueuedInputsOnShutdown::Stop`, cancels it as it would without a drain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kickoff_run_still_running_at_the_drain_bound_is_cancelled_by_the_shutdown() {
    use meerkat_core::event::AgentErrorClass;

    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(false);
    let host = open_stop_host(&root_path, &client, Some("drain-kickoff")).await;
    let identity = AgentIdentity::from(format!("host-human-stop-{}", Uuid::new_v4().simple()));
    let session_id = host
        .handle
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
    client.wait_for_requests(1).await;

    let stop = stop_leaving_runs_running(&host).await;
    let started = std::time::Instant::now();
    let drain = tokio::time::timeout(WAIT, host.handle.drain_stopped_runs(started + DRAIN_BOUND))
        .await
        .expect("the drain returns at its bound")
        .expect("drain a stopped mob");
    let waited = started.elapsed();
    tokio::time::timeout(WAIT, host.handle.shutdown_with_report(stop_options()))
        .await
        .expect("the graceful stop finishes")
        .expect("graceful stop");
    let events = stop_events(&root_path, &session_id).await;
    let kickoff_run = events.iter().find_map(|event| match event {
        AgentEvent::RunStarted { identity, .. } => identity.run_id.clone(),
        _ => None,
    });
    let kickoff_failures = kickoff_run.as_ref().map(|run_id| {
        run_failures(&events, run_id)
            .into_iter()
            .map(|report| (report.class, report.message))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        (
            stop.members
                .get(&identity)
                .map(|outcome| outcome.run.clone()),
            drain.members.get(&identity).cloned(),
            waited >= DRAIN_BOUND,
            kickoff_failures,
            client.requests().len(),
        ),
        (
            kickoff_run
                .clone()
                .map(|run_id| crate::MemberStopRun::LeftRunning { run_id }),
            kickoff_run.map(|run_id| crate::MemberRunDrainOutcome {
                run: crate::MemberRunDrain::StillRunning { run_id },
                starts: Some(crate::MemberRunStarts::Held),
            }),
            true,
            Some(vec![(
                AgentErrorClass::Cancelled,
                "Run was cancelled".to_owned()
            )]),
            1,
        ),
        "the kickoff run outlives the drain's bound and the Shutdown cancels it"
    );
}

/// A durable autonomous member caught mid-turn by the default Shutdown keeps
/// its work for the next start: the Shutdown neither closes its kickoff nor
/// cancels its run (no run outcome in the report, no failure in the log), and
/// after the restart the run comes back and the kickoff resolves as started
/// under the same objective, its prompt asked of the model once more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_default_shutdown_keeps_a_durable_autonomous_members_kickoff_for_the_next_start() {
    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(false);
    let host = open_stop_host(&root_path, &client, Some("kickoff-kept")).await;
    let mob_id = host.handle.mob_id().clone();
    let identity = AgentIdentity::from(format!("host-human-stop-{}", Uuid::new_v4().simple()));
    let session_id = host
        .handle
        .spawn(
            ProfileName::from("lead"),
            identity.clone(),
            Some(KEPT_KICKOFF.into()),
        )
        .await
        .expect("spawn autonomous member")
        .bridge_session_id()
        .expect("local session member")
        .clone();
    client.wait_for_requests(1).await;
    let before = host
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member is in the roster")
        .kickoff
        .expect("the member has a kickoff");

    let report = tokio::time::timeout(
        WAIT,
        host.handle
            .shutdown_with_report(crate::ShutdownOptions::default()),
    )
    .await
    .expect("the graceful stop finishes")
    .expect("graceful stop");
    let events = stop_events(&root_path, &session_id).await;
    let kickoff_run = events.iter().find_map(|event| match event {
        AgentEvent::RunStarted { identity, .. } => identity.run_id.clone(),
        _ => None,
    });
    let failures_at_stop = kickoff_run
        .as_ref()
        .map(|run_id| run_failures(&events, run_id).len());
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    tokio::time::timeout(WAIT, restarted.handle.wait_for_kickoff_complete(Some(WAIT)))
        .await
        .expect("the kept kickoff resolves after the restart")
        .expect("kickoff wait");
    let after = restarted
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member survives the restart")
        .kickoff
        .expect("the member keeps its kickoff");
    assert_eq!(
        (
            report.runs.get(&identity).cloned(),
            failures_at_stop,
            after.phase,
            after.objective_id == before.objective_id,
            requests_carrying(&restarted_client.requests(), KEPT_KICKOFF),
        ),
        (
            None,
            Some(0),
            crate::roster::MobMemberKickoffPhase::Started,
            true,
            1,
        ),
        "the Shutdown keeps the kickoff and its run; the restart resumes it under the same \
         objective"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// The kickoff prompt of the kept-kickoff row.
const KEPT_KICKOFF: &str = "A kickoff a deploy keeps for the next start.";

/// The same member under an explicit `QueuedInputsOnShutdown::Stop`: the
/// Shutdown closes its kickoff and cancels its run, as before 0.9.5.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_explicit_stop_shutdown_still_ends_a_durable_autonomous_members_kickoff() {
    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(false);
    let host = open_stop_host(&root_path, &client, Some("kickoff-stopped")).await;
    let identity = AgentIdentity::from(format!("host-human-stop-{}", Uuid::new_v4().simple()));
    let session_id = host
        .handle
        .spawn(
            ProfileName::from("lead"),
            identity.clone(),
            Some(KEPT_KICKOFF.into()),
        )
        .await
        .expect("spawn autonomous member")
        .bridge_session_id()
        .expect("local session member")
        .clone();
    client.wait_for_requests(1).await;

    let report = tokio::time::timeout(WAIT, host.handle.shutdown_with_report(stop_options()))
        .await
        .expect("the graceful stop finishes")
        .expect("graceful stop");
    let events = stop_events(&root_path, &session_id).await;
    let kickoff_run = events.iter().find_map(|event| match event {
        AgentEvent::RunStarted { identity, .. } => identity.run_id.clone(),
        _ => None,
    });
    assert!(
        matches!(
            report.runs.get(&identity),
            Some(crate::MemberStopRun::CancelledByShutdown { .. })
        ),
        "an explicit Stop cancels the kickoff run: {report:?}"
    );
    assert_eq!(
        kickoff_run.as_ref().map(|run_id| {
            run_failures(&events, run_id)
                .into_iter()
                .map(|report| report.class)
                .collect::<Vec<_>>()
        }),
        Some(vec![meerkat_core::event::AgentErrorClass::Cancelled]),
        "the kickoff run ends cancelled"
    );
}

/// A draining deploy over a durable autonomous member whose kickoff run is
/// in flight: a Stop that leaves runs running, a drain that reaches its
/// bound, then the default Shutdown. The Stop leaves the kickoff open for the
/// Shutdown to decide, and the Shutdown keeps it with its run. After the
/// restart and the mob's resume, the run comes back and the kickoff resolves
/// as started under the same objective, its prompt asked of the model once
/// more.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_draining_deploy_keeps_a_durable_autonomous_members_kickoff_for_the_next_start() {
    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(false);
    let host = open_stop_host(&root_path, &client, Some("drain-kickoff-kept")).await;
    let mob_id = host.handle.mob_id().clone();
    let identity = AgentIdentity::from(format!("host-human-stop-{}", Uuid::new_v4().simple()));
    let session_id = host
        .handle
        .spawn(
            ProfileName::from("lead"),
            identity.clone(),
            Some(KEPT_KICKOFF.into()),
        )
        .await
        .expect("spawn autonomous member")
        .bridge_session_id()
        .expect("local session member")
        .clone();
    client.wait_for_requests(1).await;
    let before = host
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member is in the roster")
        .kickoff
        .expect("the member has a kickoff");

    let stop = stop_leaving_runs_running(&host).await;
    let started = std::time::Instant::now();
    let drain = tokio::time::timeout(WAIT, host.handle.drain_stopped_runs(started + DRAIN_BOUND))
        .await
        .expect("the drain returns at its bound")
        .expect("drain a stopped mob");
    let report = tokio::time::timeout(
        WAIT,
        host.handle
            .shutdown_with_report(crate::ShutdownOptions::default()),
    )
    .await
    .expect("the graceful stop finishes")
    .expect("graceful stop");
    let events = stop_events(&root_path, &session_id).await;
    let kickoff_run = events.iter().find_map(|event| match event {
        AgentEvent::RunStarted { identity, .. } => identity.run_id.clone(),
        _ => None,
    });
    let failures_at_stop = kickoff_run
        .as_ref()
        .map(|run_id| run_failures(&events, run_id).len());
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    let restarted_client = ScriptedClient::new(true);
    let (restarted, restored) = reopen_stopped_host(&root_path, &restarted_client).await;
    tokio::time::timeout(WAIT, restarted.handle.wait_for_kickoff_complete(Some(WAIT)))
        .await
        .expect("the kept kickoff resolves after the restart")
        .expect("kickoff wait");
    let after = restarted
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member survives the restart")
        .kickoff
        .expect("the member keeps its kickoff");
    assert_eq!(
        (
            stop.members
                .get(&identity)
                .map(|outcome| outcome.run.clone()),
            drain.members.get(&identity).cloned(),
            report.runs.get(&identity).cloned(),
            failures_at_stop,
            restored,
            after.phase,
            after.objective_id == before.objective_id,
            requests_carrying(&restarted_client.requests(), KEPT_KICKOFF),
        ),
        (
            kickoff_run
                .clone()
                .map(|run_id| crate::MemberStopRun::LeftRunning { run_id }),
            kickoff_run.map(|run_id| crate::MemberRunDrainOutcome {
                run: crate::MemberRunDrain::StillRunning { run_id },
                starts: Some(crate::MemberRunStarts::Held),
            }),
            None,
            Some(0),
            MobState::Stopped,
            crate::roster::MobMemberKickoffPhase::Started,
            true,
            1,
        ),
        "the Stop leaves the kickoff open, the default Shutdown keeps it with its run, and the \
         restart resumes it under the same objective"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// A kickoff the durable record still calls starting while its input has
/// already ended (the run finished just as the process stopped, before its
/// kickoff record): the restarted mob finds the input by its incarnation key
/// and, with no completion left to wait on, resolves the kickoff from the
/// input's receipt instead of waiting. No second kickoff is admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_kept_kickoff_whose_input_already_ended_resolves_from_its_receipt() {
    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(true);
    let host = open_stop_host(&root_path, &client, Some("kickoff-ended")).await;
    let mob_id = host.handle.mob_id().clone();
    let identity = AgentIdentity::from(format!("host-human-stop-{}", Uuid::new_v4().simple()));
    host.handle
        .spawn(
            ProfileName::from("lead"),
            identity.clone(),
            Some(KEPT_KICKOFF.into()),
        )
        .await
        .expect("spawn autonomous member");
    tokio::time::timeout(WAIT, host.handle.wait_for_kickoff_complete(Some(WAIT)))
        .await
        .expect("the kickoff resolves")
        .expect("kickoff wait");
    let before = host
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member is in the roster")
        .kickoff
        .expect("the member has a kickoff");
    assert_eq!(before.phase, crate::roster::MobMemberKickoffPhase::Started);
    tokio::time::timeout(WAIT, host.handle.shutdown_with_report(stop_options()))
        .await
        .expect("the graceful stop finishes")
        .expect("graceful stop");
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    // The durable record calls the kickoff starting again, as a stop that
    // landed between the run's end and its kickoff record leaves it.
    let storage = MobStorage::persistent(root_path.join("mob.db")).expect("reopen the mob store");
    storage
        .events
        .append(crate::event::NewMobEvent::for_test(
            mob_id.clone(),
            crate::event::MobEventKind::MemberKickoffUpdated {
                member: identity.clone(),
                kickoff: crate::roster::MobMemberKickoffSnapshot {
                    phase: crate::roster::MobMemberKickoffPhase::Starting,
                    error: None,
                    ..before.clone()
                },
            },
        ))
        .await
        .expect("record the kickoff starting");
    drop(storage);

    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host(&root_path, &restarted_client, None).await;
    tokio::time::timeout(WAIT, restarted.handle.wait_for_kickoff_complete(Some(WAIT)))
        .await
        .expect("the kickoff resolves instead of waiting")
        .expect("kickoff wait");
    let after = restarted
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member survives the restart")
        .kickoff
        .expect("the member keeps its kickoff");
    assert_eq!(
        (
            after.phase,
            after.objective_id == before.objective_id,
            requests_carrying(&restarted_client.requests(), KEPT_KICKOFF),
        ),
        (crate::roster::MobMemberKickoffPhase::Started, true, 0),
        "the ended kickoff input's receipt resolves the kickoff, and nothing is admitted again"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}

/// The Keep row of a member whose kickoff run is still in flight at the
/// first process's requeue Shutdown, its first round committed (a tool call)
/// and its second request blocked, which keeps the kickoff and its run. The
/// second process restores the mob with the member's run starts held by its
/// host and shuts down with Keep. While the host holds the member, its kept
/// kickoff is not re-attached (a re-attach is a deduplicated resubmission,
/// live work after which the Keep keeps nothing and settles the kickoff), so
/// every input row of the member is the same before and after the Keep, and
/// its model saw no request. The third process holds the member again until
/// its host releases it: the release re-attaches the kickoff, whose run
/// resumes and resolves it as started under the same objective, its prompt
/// asked of the model once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_keep_shutdown_of_a_held_member_keeps_its_kickoff_for_the_hosts_release() {
    let (_root, root_path) = stop_root();
    let client = ScriptedClient::new(false);
    let host = open_stop_host(&root_path, &client, Some("keep-kickoff")).await;
    let mob_id = host.handle.mob_id().clone();
    let identity = AgentIdentity::from(format!("host-human-stop-{}", Uuid::new_v4().simple()));
    // The kickoff run's first request calls a tool, which commits that
    // round; its second request blocks, so the run stays open with a round
    // to continue from, which a held restart marks as resumable.
    client.call_a_tool_next();
    let session_id = host
        .handle
        .spawn(
            ProfileName::from("lead"),
            identity.clone(),
            Some(KEPT_KICKOFF.into()),
        )
        .await
        .expect("spawn autonomous member")
        .bridge_session_id()
        .expect("local session member")
        .clone();
    client.wait_for_requests(2).await;
    let before = host
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member is in the roster")
        .kickoff
        .expect("the member has a kickoff");
    let report = tokio::time::timeout(
        WAIT,
        host.handle
            .shutdown_with_report(requeue_options(crate::RequeueMaxAge::DEFAULT)),
    )
    .await
    .expect("the requeue stop finishes")
    .expect("requeue stop");
    let events = stop_events(&root_path, &session_id).await;
    let kickoff_run = events.iter().find_map(|event| match event {
        AgentEvent::RunStarted { identity, .. } => identity.run_id.clone(),
        _ => None,
    });
    assert_eq!(
        (report.runs.get(&identity), kickoff_run.is_some()),
        (None, true),
        "the first process keeps the kickoff and its run"
    );
    drop(host);
    await_supervisor_route_release(&mob_id).await;

    // The second process holds the member and serves it nothing.
    let held_client = ScriptedClient::new(true);
    let held = open_stop_host_holding(
        &root_path,
        &held_client,
        None,
        std::slice::from_ref(&identity),
    )
    .await;
    let mut parks = held.adapter.run_start_held_parks();
    // Hang guard only: the park is the positive event.
    tokio::time::timeout(WAIT, parks.wait_for(|parks| *parks >= 1))
        .await
        .expect("the member's runtime loop parks on the hold")
        .expect("park signal");
    let held_kickoff = held
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member is restored")
        .kickoff
        .expect("the member has a kickoff");
    assert_eq!(
        (
            held.adapter.resumable_run_id(&session_id).await,
            held_kickoff.phase,
        ),
        (
            kickoff_run.clone(),
            crate::roster::MobMemberKickoffPhase::Starting,
        ),
        "the held registration keeps the kickoff's run marked, and the kickoff is still starting"
    );
    // The baseline is the store as the held restore left it: the boot's
    // ordinary recovery, which every start runs, may already have
    // normalized an input. The Keep is measured across its own Shutdown.
    let rows_before = member_input_rows(&held.runtime_store, &session_id).await;
    tokio::time::timeout(
        WAIT,
        held.handle.shutdown_with_report(
            crate::ShutdownOptions::default()
                .with_queued_inputs(crate::QueuedInputsOnShutdown::Keep),
        ),
    )
    .await
    .expect("the keep stop finishes")
    .expect("keep stop");
    let rows_after = member_input_rows(&held.runtime_store, &session_id).await;
    assert_eq!(
        (held_client.requests().len(), rows_after),
        (0, rows_before),
        "the Keep itself changes no input row: a keep stop of a held member leaves the rows of \
         its kept kickoff as the held restore left them, and the member ran nothing"
    );
    drop(held);
    await_supervisor_route_release(&mob_id).await;

    // The third process holds the member until its host releases it.
    let restarted_client = ScriptedClient::new(true);
    let restarted = open_stop_host_holding(
        &root_path,
        &restarted_client,
        None,
        std::slice::from_ref(&identity),
    )
    .await;
    let mut parks = restarted.adapter.run_start_held_parks();
    tokio::time::timeout(WAIT, parks.wait_for(|parks| *parks >= 1))
        .await
        .expect("the member's runtime loop parks on the hold")
        .expect("park signal");
    let requests_while_held = restarted_client.requests().len();
    let run_while_held = restarted.adapter.resumable_run_id(&session_id).await;
    let phase_while_held = restarted
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member is restored")
        .kickoff
        .expect("the member has a kickoff")
        .phase;
    restarted
        .handle
        .release_member_run_starts(&identity, crate::HostRunStartHoldReason::ToolsNotPublished)
        .await
        .expect("the host releases the member's run starts");
    tokio::time::timeout(WAIT, restarted.handle.wait_for_kickoff_complete(Some(WAIT)))
        .await
        .expect("the kept kickoff resolves after the release")
        .expect("kickoff wait");
    let after = restarted
        .handle
        .get_member(&identity)
        .await
        .expect("roster read")
        .expect("the member survives the restarts")
        .kickoff
        .expect("the member keeps its kickoff");
    assert_eq!(
        (
            requests_while_held,
            run_while_held,
            phase_while_held,
            after.phase,
            after.objective_id == before.objective_id,
            requests_carrying(&restarted_client.requests(), KEPT_KICKOFF),
        ),
        (
            0,
            kickoff_run,
            crate::roster::MobMemberKickoffPhase::Starting,
            crate::roster::MobMemberKickoffPhase::Started,
            true,
            1,
        ),
        "the Keep left the kickoff starting with its run marked; the host's release re-attaches \
         it, and its run resumes once and resolves it under the same objective"
    );
    tokio::time::timeout(WAIT, restarted.handle.shutdown())
        .await
        .expect("mob shutdown finishes")
        .expect("mob shutdown");
}
