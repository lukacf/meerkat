#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! Member-level safe-boundary instruction activation.
//!
//! A restored member's standing instructions cannot change through build
//! options: resume inherits persisted prompt state. These gates drive the
//! member-level door onto the native activation seam with a real factory, a
//! real persistent session service over SQLite session and runtime stores,
//! and a member restored onto an existing durable session through a Resume
//! launch. Turns are held at a deterministic barrier in the model double.

use super::*;
use crate::MemberInstructionActivationError;
use meerkat_core::lifecycle::core_executor::CoreApplyOutput;
use meerkat_core::service::MobToolAuthorityContext;
use meerkat_core::{
    AgentExecutionSnapshot, ExternalToolSurfaceSnapshot, InputId, PeerIngressRuntimeSnapshot,
    RunApplyBoundary, SessionHistoryPage, SessionHistoryQuery, SessionTranscriptRevisionList,
    SessionTranscriptRevisionListQuery, SessionTranscriptRevisionPage,
    SessionTranscriptRevisionQuery, StageToolResultsRequest, StageToolResultsResult,
    ToolScopeSnapshot,
};
use meerkat_core::{
    InstructionActivationAdmissionErrorCode, InstructionActivationDisposition,
    InstructionActivationExpectation, InstructionActivationId, InstructionActivationReadQuery,
    InstructionActivationRequest, InstructionContentDigest, InstructionKey, InstructionNamespace,
    InstructionRevisionId, InstructionRevisionRef,
};
use meerkat_runtime::RuntimeStore;

const WAIT: Duration = Duration::from_secs(30);
const BODY: &str = "Always state your uncertainty explicitly.";

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

    fn requests(&self) -> Vec<meerkat_client::LlmRequest> {
        self.requests.lock().expect("recorded requests").clone()
    }

    /// Wait until `count` model requests have started (a typed signal from
    /// the double, not a clock).
    async fn wait_started(&self, count: usize) {
        let mut started = self.started.subscribe();
        tokio::time::timeout(WAIT, started.wait_for(|seen| *seen >= count))
            .await
            .expect("the model request starts")
            .expect("started channel open");
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

type Service = meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>;

fn request(
    activation: &str,
    expectation: InstructionActivationExpectation,
) -> InstructionActivationRequest {
    InstructionActivationRequest {
        revision: InstructionRevisionRef {
            namespace: InstructionNamespace::new("host.standing").expect("namespace"),
            key: InstructionKey::new("member").expect("key"),
            revision_id: InstructionRevisionId::new(format!("rev-{activation}")).expect("revision"),
            content_sha256: InstructionContentDigest::for_body(BODY),
        },
        activation_id: InstructionActivationId::new(activation).expect("activation id"),
        expectation,
        supersedes: None,
        body: BODY.to_string(),
    }
}

fn system_rows_with_body(request: &meerkat_client::LlmRequest) -> usize {
    request
        .messages
        .iter()
        .filter(
            |message| matches!(message, Message::System(system) if system.content.contains(BODY)),
        )
        .count()
}

struct Fixture {
    _root: tempfile::TempDir,
    handle: MobHandle,
    identity: AgentIdentity,
    client: ScriptedClient,
    service: Arc<Service>,
    mob_service: Arc<dyn MobSessionService>,
}

impl Fixture {
    /// A turn-driven member restored onto a durable session that existed
    /// before the member: the case build-time instructions cannot reach.
    async fn restored_member() -> Self {
        Self::restored_member_through(false, None).await
    }

    /// [`Self::restored_member`] in a mob whose host set `policy` as its fork
    /// instruction policy.
    async fn restored_member_with_policy(policy: meerkat_core::ForkInstructionPolicy) -> Self {
        Self::restored_member_through(false, Some(policy)).await
    }

    /// [`Self::restored_member`], with the mob's session service optionally
    /// behind a decorator that forwards everything except the activation
    /// seam (the regression the typed default guards against).
    async fn restored_member_through(
        non_forwarding_decorator: bool,
        fork_instruction_policy: Option<meerkat_core::ForkInstructionPolicy>,
    ) -> Self {
        let root = tempfile::Builder::new()
            .prefix("meerkat-member-instruction-")
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
        let db = root_path.join("realm.db");
        let session_store =
            Arc::new(meerkat_store::SqliteSessionStore::open(&db).expect("session store"));
        builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
            session_store.clone(),
        )));
        let runtime_store: Arc<dyn RuntimeStore> = Arc::new(
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
        let mut definition = with_unique_mob_id(sample_definition(), "member-instruction");
        let lead = definition
            .profiles
            .get_mut(&ProfileName::from("lead"))
            .and_then(ProfileBinding::as_inline_mut)
            .expect("inline lead");
        lead.model = "gpt-5.5".to_string();
        lead.runtime_mode = crate::MobRuntimeMode::TurnDriven;
        lead.tools = ToolConfig {
            comms: true,
            ..ToolConfig::default()
        };
        let mob_id = definition.id.clone();
        let mob_service: Arc<dyn MobSessionService> = if non_forwarding_decorator {
            Arc::new(NonForwardingDecorator {
                inner: service.clone(),
            })
        } else {
            service.clone()
        };
        let mut builder = MobBuilder::new(definition, MobStorage::in_memory())
            .with_session_service(Arc::clone(&mob_service))
            .with_default_llm_client(Arc::new(client.clone()));
        if let Some(policy) = fork_instruction_policy {
            builder = builder.with_fork_instruction_policy(policy);
        }
        let handle = builder.create().await.expect("create real turn-driven mob");

        // The durable session exists before the member: created, persisted
        // and its creating actor discarded, so the member adopts it through
        // a Resume launch instead of minting it.
        let identity = AgentIdentity::from("restored-member");
        let created = service
            .create_session(CreateSessionRequest {
                injected_context: Vec::new(),
                model: "gpt-5.5".to_string(),
                prompt: "adopted session".to_string().into(),
                system_prompt: meerkat_core::SystemPromptOverride::Inherit,
                max_tokens: None,
                event_tx: None,
                build: Some(meerkat_core::service::SessionBuildOptions {
                    comms_name: Some(
                        super::actor::render_member_comms_name(
                            mob_id.as_str(),
                            "lead",
                            identity.as_str(),
                        )
                        .expect("comms name"),
                    ),
                    mob_member_binding: None,
                    ..Default::default()
                }),
                initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
                deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
                labels: None,
            })
            .await
            .expect("persist the session the member restores");
        MobSessionService::discard_live_session(&*service, &created.session_id)
            .await
            .expect("discard the creating actor");
        handle
            .spawn_spec(
                SpawnMemberSpec::new(ProfileName::from("lead"), identity.clone()).with_launch_mode(
                    crate::launch::MemberLaunchMode::Resume {
                        bridge_session_id: created.session_id.clone(),
                        resume_from_role: None,
                    },
                ),
            )
            .await
            .expect("restore the member onto its durable session");
        assert_eq!(
            handle.resolve_bridge_session_id(&identity).await.as_ref(),
            Some(&created.session_id),
            "the member runs on the restored session"
        );
        Self {
            _root: root,
            handle,
            identity,
            client,
            service,
            mob_service,
        }
    }

    async fn activate(
        &self,
        request: InstructionActivationRequest,
    ) -> Result<meerkat_core::InstructionActivationReceipt, MemberInstructionActivationError> {
        tokio::time::timeout(
            WAIT,
            self.handle
                .activate_member_instruction(&self.identity, request),
        )
        .await
        .expect("activation answers")
    }

    async fn records(&self) -> usize {
        self.handle
            .read_member_instruction_activations(
                &self.identity,
                InstructionActivationReadQuery::default(),
            )
            .await
            .expect("read activations")
            .records
            .len()
    }

    /// Start one turn and return its handle unawaited.
    async fn start_turn(&self, text: &str) -> WorkTurnHandle {
        self.start_turn_for(&self.identity, text).await
    }

    /// Start one turn on `identity` and return its handle unawaited.
    async fn start_turn_for(&self, identity: &AgentIdentity, text: &str) -> WorkTurnHandle {
        let entry = self
            .handle
            .get_member(identity)
            .await
            .expect("roster read")
            .expect("member binding");
        self.handle
            .start_work_with_mode(
                entry.agent_runtime_id,
                entry.fence_token,
                WorkRef::new(),
                WorkSpec::new(text, WorkOrigin::Internal),
                HandlingMode::Queue,
            )
            .await
            .expect("turn admitted")
    }

    async fn run_turn(&self, text: &str) {
        let turn = self.start_turn(text).await;
        tokio::time::timeout(WAIT, turn.wait())
            .await
            .expect("turn finishes")
            .expect("turn succeeds");
    }

    async fn finish(self) {
        self.client.release();
        let _ = tokio::time::timeout(WAIT, self.handle.shutdown()).await;
    }
}

/// An activation on a restored member applies durably and reaches the
/// member's next turn as an ordered System row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_activation_on_a_restored_member_reaches_its_next_turn() {
    let fixture = Fixture::restored_member().await;
    let receipt = fixture
        .activate(request(
            "standing-1",
            InstructionActivationExpectation::Absent,
        ))
        .await
        .expect("activation applies at a safe boundary");
    assert_eq!(
        receipt.disposition,
        InstructionActivationDisposition::Applied
    );
    assert_eq!(fixture.records().await, 1);

    let before = fixture.client.requests().len();
    fixture.run_turn("hello after the activation").await;
    let requests = fixture.client.requests();
    let next = requests
        .get(before)
        .expect("the member's next turn reached the model");
    assert_eq!(
        system_rows_with_body(next),
        1,
        "the next turn carries the activation exactly once"
    );
    fixture.finish().await;
}

fn standing_key() -> (InstructionNamespace, InstructionKey) {
    (
        InstructionNamespace::new("host.standing").expect("namespace"),
        InstructionKey::new("member").expect("key"),
    )
}

/// An activation request for `key` under the standing namespace.
fn keyed_request(
    key: &str,
    activation: &str,
    body: &str,
    expectation: InstructionActivationExpectation,
    supersedes: Option<&str>,
) -> InstructionActivationRequest {
    InstructionActivationRequest {
        revision: InstructionRevisionRef {
            namespace: InstructionNamespace::new("host.standing").expect("namespace"),
            key: InstructionKey::new(key).expect("key"),
            revision_id: InstructionRevisionId::new(format!("rev-{activation}")).expect("revision"),
            content_sha256: InstructionContentDigest::for_body(body),
        },
        activation_id: InstructionActivationId::new(activation).expect("activation id"),
        expectation,
        supersedes: supersedes.map(|id| InstructionActivationId::new(id).expect("supersedes")),
        body: body.to_string(),
    }
}

/// The activation rows of `session` whose origin is `session` itself, in
/// transcript order.
fn origin_local_rows(
    session: &meerkat_core::Session,
) -> Vec<meerkat_core::InstructionActivationIdentity> {
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::System(system) => system.instruction_activation.clone(),
            _ => None,
        })
        .filter(|identity| identity.origin_session_id == *session.id())
        .collect()
}

impl Fixture {
    async fn activate_standing(&self) {
        self.activate(request(
            "standing-1",
            InstructionActivationExpectation::Absent,
        ))
        .await
        .expect("activation applies at a safe boundary");
    }

    async fn source_session(&self) -> SessionId {
        self.handle
            .resolve_bridge_session_id(&self.identity)
            .await
            .expect("the restored member's session")
    }

    async fn durable(&self, id: &SessionId) -> Option<meerkat_core::Session> {
        self.service
            .load_durable_session_body(id)
            .await
            .expect("durable session read")
    }

    async fn session_count(&self) -> usize {
        meerkat_core::service::SessionService::list(
            &*self.service,
            meerkat_core::service::SessionQuery::default(),
        )
        .await
        .expect("session list")
        .len()
    }

    async fn member_session(&self, identity: &AgentIdentity) -> Option<SessionId> {
        self.handle.resolve_bridge_session_id(identity).await
    }

    async fn fork_at(
        &self,
        source: &SessionId,
        message_index: usize,
        inherited_instructions: Option<meerkat_core::ForkInstructionPolicy>,
    ) -> Result<meerkat_core::SessionForkResult, SessionError> {
        meerkat_core::service::SessionServiceTranscriptEditExt::fork_session_at(
            &*self.service,
            source,
            meerkat_core::SessionForkAtRequest {
                message_index,
                running_behavior: meerkat_core::TranscriptEditRunningBehavior::Reject,
                tool_access_policy: None,
                inherited_instructions,
            },
        )
        .await
    }
}

/// Assert `refusal` is the policy refusal for exactly the standing key.
fn assert_policy_refusal(refusal: &meerkat_core::ForkInstructionRefusal) {
    assert_eq!(
        refusal.cause,
        meerkat_core::ForkInstructionRefusalCause::PolicyRefuses,
        "the fork instruction policy refused the inherited key"
    );
    assert_eq!(
        refusal.keys,
        vec![standing_key()],
        "the refusal names the key"
    );
}

/// fork_off's composition (`fork_member_then_run_bounded`) starts the child's
/// first turn itself, so the host has no window to activate the inherited
/// key. Under the default policy the fork is refused, typed, before anything
/// is committed: no child session, no roster entry, no model request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_self_starting_fork_of_an_activated_member_is_refused_before_commit_by_default() {
    let fixture = Fixture::restored_member().await;
    fixture.activate_standing().await;
    let sessions_before = fixture.session_count().await;
    let requests_before = fixture.client.requests().len();

    let outcome = tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member_then_run_bounded(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), AgentIdentity::from("fork-child"))
                .with_initial_message("the forked task"),
            None,
            "result",
            4_096,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
        ),
    )
    .await
    .expect("the fork answers");
    match outcome {
        Err(BoundedMemberRunError::Admission(MobError::ForkInstructionRefused {
            source_member_id,
            refusal,
        })) => {
            assert_eq!(source_member_id, fixture.identity);
            assert_policy_refusal(&refusal);
        }
        other => panic!("expected a typed fork instruction refusal before commit, got {other:?}"),
    }
    assert!(
        fixture
            .handle
            .get_member(&AgentIdentity::from("fork-child"))
            .await
            .expect("roster read")
            .is_none(),
        "no child is seated"
    );
    assert_eq!(
        fixture.session_count().await,
        sessions_before,
        "no child session is committed"
    );
    assert_eq!(
        fixture.client.requests().len(),
        requests_before,
        "no model request is made"
    );
    fixture.finish().await;
}

/// With `ReactivateInherited` the same self-starting fork commits a child
/// that carries exactly one child-origin re-activation of the inherited key,
/// and the child's first turn reaches the model with the instruction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_self_starting_fork_reactivates_inherited_instructions_when_the_host_opts_in() {
    let fixture = Fixture::restored_member_with_policy(
        meerkat_core::ForkInstructionPolicy::ReactivateInherited,
    )
    .await;
    fixture.activate_standing().await;
    let requests_before = fixture.client.requests().len();

    let outcome = tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member_then_run_bounded(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), AgentIdentity::from("fork-child"))
                .with_initial_message("the forked task"),
            None,
            "result",
            4_096,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
        ),
    )
    .await
    .expect("the fork answers");
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => panic!("the fork child runs its first turn: {error:?}"),
    };
    let requests = fixture.client.requests();
    let child_request = requests
        .get(requests_before)
        .expect("the child's first turn reached the model");
    // The inherited row reaches the model as a header that names the key;
    // the body comes once, from the child's own re-activation.
    assert_eq!(
        system_rows_with_body(child_request),
        1,
        "the child's request carries the body once, from its own activation"
    );
    assert_eq!(
        child_request
            .messages
            .iter()
            .filter(|message| matches!(
                message,
                Message::System(system)
                    if system.content.starts_with("[meerkat-inherited-instruction-v2]")
            ))
            .count(),
        1,
        "the inherited history row is a header without its body"
    );
    let child = fixture
        .durable(&outcome.fork.session_id)
        .await
        .expect("the child is committed");
    let local = origin_local_rows(&child);
    assert_eq!(local.len(), 1, "exactly one child-origin row: {local:?}");
    assert_eq!(
        local[0]
            .supersedes
            .as_ref()
            .map(InstructionActivationId::as_str),
        Some("standing-1"),
        "the re-activation supersedes the inherited head"
    );
    assert_eq!(local[0].revision.revision_id.as_str(), "rev-standing-1");
    assert!(
        local[0]
            .activation_id
            .as_str()
            .starts_with("fork-reactivation:"),
        "fork-written rows are recognizable to an operator: {}",
        local[0].activation_id
    );
    fixture.finish().await;
}

/// `Refuse` refuses even the host-driven fork whose host could activate the
/// child: typed, before commit, nothing seated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_refuse_policy_refuses_a_host_driven_fork_before_commit() {
    let fixture =
        Fixture::restored_member_with_policy(meerkat_core::ForkInstructionPolicy::Refuse).await;
    fixture.activate_standing().await;
    let sessions_before = fixture.session_count().await;
    let child = AgentIdentity::from("fork-child");

    let outcome = tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), child.clone()),
            None,
        ),
    )
    .await
    .expect("the fork answers");
    match outcome {
        Err(MobError::ForkInstructionRefused { refusal, .. }) => assert_policy_refusal(&refusal),
        other => panic!("expected a typed fork instruction refusal before commit, got {other:?}"),
    }
    assert!(
        fixture
            .handle
            .get_member(&child)
            .await
            .expect("roster read")
            .is_none(),
        "no child is seated"
    );
    assert_eq!(fixture.session_count().await, sessions_before);
    fixture.finish().await;
}

/// The host-driven fork under `ReactivateInherited` seats a child whose first
/// turn runs without any host activation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_driven_fork_reactivates_when_the_host_opts_in() {
    let fixture = Fixture::restored_member_with_policy(
        meerkat_core::ForkInstructionPolicy::ReactivateInherited,
    )
    .await;
    fixture.activate_standing().await;
    let child = AgentIdentity::from("fork-child");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), child.clone()),
            None,
        ),
    )
    .await
    .expect("the fork answers")
    .expect("the fork commits and seats the child");

    let before = fixture.client.requests().len();
    let turn = fixture.start_turn_for(&child, "the child's task").await;
    if let Err(error) = tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the child's turn finishes")
    {
        panic!("the child's first turn runs without a host activation: {error:?}");
    }
    assert!(
        fixture.client.requests().len() > before,
        "the child reached the model"
    );
    fixture.finish().await;
}

/// The live delegation's durable fork admits the child's bounded turn itself:
/// under the default policy the delegation is refused, typed, before commit.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_durable_fork_delegation_is_refused_before_commit_by_default() {
    let fixture = Fixture::restored_member().await;
    fixture.activate_standing().await;
    let sessions_before = fixture.session_count().await;
    let requests_before = fixture.client.requests().len();
    let child = AgentIdentity::from("delegated-child");
    let request = crate::runtime::delegation::DelegationExecutionRequest::new(
        child.clone(),
        "the delegated task",
        BoundedResultSpec::new("delegation-result", 4_096).expect("result spec"),
    )
    .with_durable_fork(fixture.identity.clone(), None);
    let started = tokio::time::timeout(
        WAIT,
        crate::runtime::delegation::DelegationExecutionService::new(fixture.handle.clone())
            .start(request),
    )
    .await
    .expect("the delegation answers");
    match started {
        Err(crate::runtime::delegation::DelegationExecutionError::Spawn(
            MobError::ForkInstructionRefused { refusal, .. },
        )) => assert_policy_refusal(&refusal),
        Err(other) => panic!("expected a typed fork instruction refusal, got {other:?}"),
        Ok(_) => {
            panic!("expected a typed fork instruction refusal before commit, got a started child")
        }
    }
    assert!(
        fixture
            .handle
            .get_member(&child)
            .await
            .expect("roster read")
            .is_none(),
        "no child is seated"
    );
    assert_eq!(fixture.session_count().await, sessions_before);
    assert_eq!(fixture.client.requests().len(), requests_before);
    fixture.finish().await;
}

/// With `ReactivateInherited` the delegated durable fork's bounded turn
/// completes and reaches the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_durable_fork_delegation_runs_when_the_host_opts_in() {
    let fixture = Fixture::restored_member_with_policy(
        meerkat_core::ForkInstructionPolicy::ReactivateInherited,
    )
    .await;
    fixture.activate_standing().await;
    let requests_before = fixture.client.requests().len();
    let child = AgentIdentity::from("delegated-child");
    let request = crate::runtime::delegation::DelegationExecutionRequest::new(
        child.clone(),
        "the delegated task",
        BoundedResultSpec::new("delegation-result", 4_096).expect("result spec"),
    )
    .with_durable_fork(fixture.identity.clone(), None);
    let service =
        crate::runtime::delegation::DelegationExecutionService::new(fixture.handle.clone());
    let execution = tokio::time::timeout(WAIT, service.start(request))
        .await
        .expect("the delegation answers")
        .expect("the delegation starts");
    let terminalized = tokio::time::timeout(WAIT, execution.await_terminal())
        .await
        .expect("the delegated turn ends");
    match terminalized.terminal() {
        crate::runtime::delegation::DelegationTurnTerminal::Completed(_) => {}
        terminal => panic!("the delegated child completes its bounded turn: {terminal:?}"),
    }
    assert!(
        fixture.client.requests().len() > requests_before,
        "the child reached the model"
    );
    fixture.finish().await;
}

/// RPC `session/fork_at` commits a child the host cannot activate (it is not
/// materialized), so the default refuses a source whose prefix carries
/// activation rows, typed, before commit, naming the field that clears it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generic_fork_at_of_an_activated_session_is_refused_before_commit_by_default() {
    let fixture = Fixture::restored_member().await;
    fixture.activate_standing().await;
    let source = fixture.source_session().await;
    let prefix = fixture
        .durable(&source)
        .await
        .expect("source")
        .messages()
        .len();
    let sessions_before = fixture.session_count().await;

    match fixture.fork_at(&source, prefix, None).await {
        Err(SessionError::ForkInstructionRefused(refusal)) => {
            assert_policy_refusal(&refusal);
            assert!(
                refusal.to_string().contains("reactivate_inherited"),
                "the refusal names the option that clears it: {refusal}"
            );
        }
        other => panic!("expected a typed fork instruction refusal before commit, got {other:?}"),
    }
    assert_eq!(fixture.session_count().await, sessions_before);
    fixture.finish().await;
}

/// RPC `session/fork_replace` (its own code path) refuses the same way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generic_fork_replace_of_an_activated_session_is_refused_before_commit_by_default() {
    let fixture = Fixture::restored_member().await;
    fixture.activate_standing().await;
    fixture.run_turn("a turn the replacement can address").await;
    let source = fixture.source_session().await;
    let source_body = fixture.durable(&source).await.expect("source");
    let user_index = source_body
        .messages()
        .iter()
        .rposition(|message| matches!(message, Message::User(_)))
        .expect("a user message");
    let sessions_before = fixture.session_count().await;

    let replaced = meerkat_core::service::SessionServiceTranscriptEditExt::fork_session_replace(
        &*fixture.service,
        &source,
        meerkat_core::SessionForkReplaceRequest {
            message_index: user_index,
            replacement: meerkat_core::TranscriptReplacement::Message {
                message: Message::User(meerkat_core::UserMessage::text("a replaced question")),
            },
            running_behavior: meerkat_core::TranscriptEditRunningBehavior::Reject,
            tool_access_policy: None,
            inherited_instructions: None,
        },
    )
    .await;
    match replaced {
        Err(SessionError::ForkInstructionRefused(refusal)) => assert_policy_refusal(&refusal),
        other => panic!("expected a typed fork instruction refusal before commit, got {other:?}"),
    }
    assert_eq!(fixture.session_count().await, sessions_before);
    fixture.finish().await;
}

/// `reactivate_inherited` on a generic fork appends one child-origin row per
/// effective key, in (namespace, key) order whatever the activation order,
/// and the result keeps the prefix meaning apart from the appended rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generic_fork_reactivates_every_effective_key_in_key_order() {
    let fixture = Fixture::restored_member().await;
    fixture.activate_standing().await;
    fixture
        .activate(keyed_request(
            "alpha",
            "alpha-1",
            "Prefer short answers.",
            InstructionActivationExpectation::Absent,
            None,
        ))
        .await
        .expect("a second key activates");
    let source = fixture.source_session().await;
    let prefix = fixture
        .durable(&source)
        .await
        .expect("source")
        .messages()
        .len();

    let forked = fixture
        .fork_at(
            &source,
            prefix,
            Some(meerkat_core::ForkInstructionPolicy::ReactivateInherited),
        )
        .await
        .expect("the fork commits");
    assert_eq!(forked.reactivated_instruction_count, 2);
    assert_eq!(forked.prefix_message_count(), prefix);
    assert_eq!(forked.message_count, prefix + 2);
    let child = fixture
        .durable(&forked.session_id)
        .await
        .expect("the child is committed");
    let keys = origin_local_rows(&child)
        .into_iter()
        .map(|identity| identity.revision.key.as_str().to_string())
        .collect::<Vec<_>>();
    assert_eq!(keys, vec!["alpha".to_string(), "member".to_string()]);
    child
        .validate_instruction_activation_turn_readiness()
        .expect("the child can run a turn");
    fixture.finish().await;
}

/// `reactivate_inherited` on `fork_replace` re-activates the same way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generic_fork_replace_reactivates_when_asked() {
    let fixture = Fixture::restored_member().await;
    fixture.activate_standing().await;
    fixture.run_turn("a turn the replacement can address").await;
    let source = fixture.source_session().await;
    let source_body = fixture.durable(&source).await.expect("source");
    let user_index = source_body
        .messages()
        .iter()
        .rposition(|message| matches!(message, Message::User(_)))
        .expect("a user message");

    let forked = meerkat_core::service::SessionServiceTranscriptEditExt::fork_session_replace(
        &*fixture.service,
        &source,
        meerkat_core::SessionForkReplaceRequest {
            message_index: user_index,
            replacement: meerkat_core::TranscriptReplacement::Message {
                message: Message::User(meerkat_core::UserMessage::text("a replaced question")),
            },
            running_behavior: meerkat_core::TranscriptEditRunningBehavior::Reject,
            tool_access_policy: None,
            inherited_instructions: Some(meerkat_core::ForkInstructionPolicy::ReactivateInherited),
        },
    )
    .await
    .expect("the fork commits");
    assert_eq!(forked.reactivated_instruction_count, 1);
    let child = fixture
        .durable(&forked.session_id)
        .await
        .expect("the child is committed");
    assert_eq!(origin_local_rows(&child).len(), 1);
    child
        .validate_instruction_activation_turn_readiness()
        .expect("the child can run a turn");
    fixture.finish().await;
}

/// Re-activation carries only what was effective in the source. A source that
/// itself only inherited a key (an unactivated fork child) has nothing
/// effective to carry: the fork is refused, typed, and nothing is committed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reactivation_refuses_a_key_the_source_never_activated() {
    let fixture = Fixture::restored_member().await;
    fixture.activate_standing().await;
    let unactivated = AgentIdentity::from("unactivated-child");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), unactivated.clone()),
            None,
        ),
    )
    .await
    .expect("the fork answers")
    .expect("the host-driven fork seats an unactivated child");
    let source = fixture
        .member_session(&unactivated)
        .await
        .expect("the unactivated child's session");
    let prefix = fixture
        .durable(&source)
        .await
        .expect("the unactivated child is committed")
        .messages()
        .len();
    let sessions_before = fixture.session_count().await;

    match fixture
        .fork_at(
            &source,
            prefix,
            Some(meerkat_core::ForkInstructionPolicy::ReactivateInherited),
        )
        .await
    {
        Err(SessionError::ForkInstructionRefused(refusal)) => {
            assert_eq!(
                refusal.cause,
                meerkat_core::ForkInstructionRefusalCause::NotEffectiveInSource
            );
            assert_eq!(refusal.keys, vec![standing_key()]);
        }
        other => panic!("expected a NotEffectiveInSource refusal, got {other:?}"),
    }
    assert_eq!(fixture.session_count().await, sessions_before);
    fixture.finish().await;
}

/// Re-activation that would cross the lineage's retained-body budget is
/// refused with the budget cause, and nothing is committed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reactivation_refuses_past_the_lineage_body_budget() {
    let fixture = Fixture::restored_member().await;
    let body = "b".repeat(250 * 1024);
    for (key, activation) in [("k1", "k1-1"), ("k2", "k2-1"), ("k3", "k3-1")] {
        fixture
            .activate(keyed_request(
                key,
                activation,
                &body,
                InstructionActivationExpectation::Absent,
                None,
            ))
            .await
            .expect("a large activation fits the source budget");
    }
    let source = fixture.source_session().await;
    let prefix = fixture
        .durable(&source)
        .await
        .expect("source")
        .messages()
        .len();
    let sessions_before = fixture.session_count().await;

    match fixture
        .fork_at(
            &source,
            prefix,
            Some(meerkat_core::ForkInstructionPolicy::ReactivateInherited),
        )
        .await
    {
        Err(SessionError::ForkInstructionRefused(refusal)) => assert_eq!(
            refusal.cause,
            meerkat_core::ForkInstructionRefusalCause::LineageBodyBudgetExceeded {
                max_bytes: 1024 * 1024
            }
        ),
        other => panic!("expected a lineage budget refusal, got {other:?}"),
    }
    assert_eq!(fixture.session_count().await, sessions_before);
    fixture.finish().await;
}

/// A prefix fork re-activates the key's head at the fork boundary, not a
/// later activation the source made after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_prefix_fork_reactivates_the_boundary_head() {
    let fixture = Fixture::restored_member().await;
    fixture.activate_standing().await;
    fixture.run_turn("a turn between the activations").await;
    fixture
        .activate(keyed_request(
            "member",
            "standing-2",
            BODY,
            InstructionActivationExpectation::Effective(
                InstructionActivationId::new("standing-1").expect("id"),
            ),
            Some("standing-1"),
        ))
        .await
        .expect("the later activation applies");
    let source = fixture.source_session().await;
    let body = fixture.durable(&source).await.expect("source");
    let boundary = body
        .messages()
        .iter()
        .position(|message| {
            matches!(
                message,
                Message::System(system)
                    if system
                        .instruction_activation
                        .as_ref()
                        .is_some_and(|identity| identity.activation_id.as_str() == "standing-2")
            )
        })
        .expect("the later activation row");

    let forked = fixture
        .fork_at(
            &source,
            boundary,
            Some(meerkat_core::ForkInstructionPolicy::ReactivateInherited),
        )
        .await
        .expect("the prefix fork commits");
    let child = fixture
        .durable(&forked.session_id)
        .await
        .expect("the child is committed");
    let local = origin_local_rows(&child);
    assert_eq!(local.len(), 1, "one re-activation: {local:?}");
    assert_eq!(
        local[0]
            .supersedes
            .as_ref()
            .map(InstructionActivationId::as_str),
        Some("standing-1"),
        "the boundary head, not the later activation"
    );
    assert_eq!(local[0].revision.revision_id.as_str(), "rev-standing-1");

    // A fork whose prefix holds both rows re-activates the later one, the
    // head at that boundary.
    let full = fixture
        .fork_at(
            &source,
            body.messages().len(),
            Some(meerkat_core::ForkInstructionPolicy::ReactivateInherited),
        )
        .await
        .expect("the full fork commits");
    let full_child = fixture
        .durable(&full.session_id)
        .await
        .expect("the child is committed");
    let local = origin_local_rows(&full_child);
    assert_eq!(local.len(), 1, "one re-activation: {local:?}");
    assert_eq!(
        local[0]
            .supersedes
            .as_ref()
            .map(InstructionActivationId::as_str),
        Some("standing-2")
    );
    fixture.finish().await;
}

/// An explicit `refuse` refuses a generic fork whose child would inherit rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_explicit_refuse_refuses_a_generic_fork() {
    let fixture = Fixture::restored_member().await;
    fixture.activate_standing().await;
    let source = fixture.source_session().await;
    let prefix = fixture
        .durable(&source)
        .await
        .expect("source")
        .messages()
        .len();
    match fixture
        .fork_at(
            &source,
            prefix,
            Some(meerkat_core::ForkInstructionPolicy::Refuse),
        )
        .await
    {
        Err(SessionError::ForkInstructionRefused(refusal)) => assert_policy_refusal(&refusal),
        other => panic!("expected a typed fork instruction refusal, got {other:?}"),
    }
    fixture.finish().await;
}

/// Council crash recovery measures a planned child's prefix without the
/// fork's re-activation rows, so the evidence of a re-activated child agrees
/// with the provenance the fork reported.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn planned_child_evidence_measures_the_prefix_without_the_reactivation_rows() {
    let fixture = Fixture::restored_member().await;
    fixture.activate_standing().await;
    let source = fixture.source_session().await;
    let evidence =
        crate::forked_participant::ForkedParticipantSourceRuntime::session_execution_evidence(
            &*fixture.service,
            &source,
        )
        .await
        .expect("source evidence read")
        .expect("source evidence");
    let planned = SessionId::new();
    let outcome = crate::forked_participant::ForkedParticipantSourceRuntime::fork_planned_child(
        &*fixture.service,
        crate::forked_participant::PlannedForkRequest {
            source_identity: evidence.agent_identity.clone().expect("the source member"),
            owner_realm: evidence.realm_id.clone().expect("the source realm"),
            source_session_id: source.clone(),
            planned_child_session_id: planned.clone(),
            prefix_message_count: None,
            inherited_instructions: meerkat_core::ForkInstructionPolicy::ReactivateInherited,
            declared_instructions: meerkat_core::ForkInstructionPolicy::ReactivateInherited,
        },
    )
    .await
    .expect("the planned fork commits");
    let child = crate::forked_participant::ForkedParticipantSourceRuntime::planned_child_evidence(
        &*fixture.service,
        &planned,
    )
    .await
    .expect("evidence read")
    .expect("the planned child exists");
    assert_eq!(child.prefix_message_count, outcome.prefix_message_count);
    assert_eq!(child.prefix_digest, outcome.prefix_digest);
    let body = fixture.durable(&planned).await.expect("the child");
    assert_eq!(
        body.messages().len(),
        outcome.prefix_message_count + 1,
        "one re-activation row follows the prefix"
    );
    fixture.finish().await;
}

/// The fork_off job record keeps the source prefix length: the re-activation
/// rows the fork appends belong to the child, not to the prefix it was forked
/// with.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_fork_job_record_keeps_the_source_prefix_length() {
    let fixture = Fixture::restored_member_with_policy(
        meerkat_core::ForkInstructionPolicy::ReactivateInherited,
    )
    .await;
    fixture.activate_standing().await;
    let source = fixture.source_session().await;
    let prefix = fixture
        .durable(&source)
        .await
        .expect("source")
        .messages()
        .len();
    let child = AgentIdentity::from("job-child");
    let started = tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member_then_run_detached(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), child.clone())
                .with_initial_message("the job task"),
            None,
            "result",
            4_096,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            Some(ForkJobBinding {
                job_id: "job-1".to_string(),
                owner_session_id: source.clone(),
                retained_work: None,
            }),
        ),
    )
    .await
    .expect("the fork answers");
    let (fork, _run) = match started {
        Ok(started) => started,
        Err(error) => panic!("the detached fork starts: {error:?}"),
    };
    let entry = fixture
        .handle
        .get_member(&child)
        .await
        .expect("roster read")
        .expect("the child is seated");
    let job = entry.fork_job.expect("the child carries its job record");
    assert_eq!(
        job.prefix_message_count, prefix,
        "the record counts the source prefix, not the re-activation rows"
    );
    let body = fixture
        .durable(&fork.session_id)
        .await
        .expect("the child is committed");
    assert!(
        matches!(
            body.messages().get(prefix),
            Some(Message::System(system))
                if system
                    .instruction_activation
                    .as_ref()
                    .is_some_and(|identity| identity.origin_session_id == fork.session_id)
        ),
        "the child's own re-activation follows the prefix"
    );
    fixture.finish().await;
}

/// A child's own fork re-activation rows, with the child's id and each row's
/// derived id replaced by placeholders, so rows of two children compare.
fn normalized_reactivation_rows(
    session: &meerkat_core::Session,
) -> Vec<(
    meerkat_core::InstructionRevisionRef,
    Option<String>,
    String,
    String,
)> {
    let child = session.id().to_string();
    session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::System(system) => system
                .instruction_activation
                .as_ref()
                .filter(|identity| identity.origin_session_id == *session.id())
                .map(|identity| {
                    (
                        identity.revision.clone(),
                        identity
                            .supersedes
                            .as_ref()
                            .map(|id| id.as_str().to_string()),
                        system
                            .content
                            .replace(identity.activation_id.as_str(), "<activation>")
                            .replace(&child, "<child>"),
                        system.created_at.to_rfc3339(),
                    )
                }),
            _ => None,
        })
        .collect()
}

/// The rows a generic (RPC) fork appends are the rows a mob fork appends,
/// whose child then runs its first turn on them: one function writes both at
/// the one fork commit. Compared row by row, each child's id standing for
/// itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generic_fork_writes_the_rows_a_running_mob_fork_writes() {
    let fixture = Fixture::restored_member_with_policy(
        meerkat_core::ForkInstructionPolicy::ReactivateInherited,
    )
    .await;
    fixture.activate_standing().await;
    let source = fixture.source_session().await;
    let prefix = fixture
        .durable(&source)
        .await
        .expect("source")
        .messages()
        .len();

    let generic = fixture
        .fork_at(
            &source,
            prefix,
            Some(meerkat_core::ForkInstructionPolicy::ReactivateInherited),
        )
        .await
        .expect("the generic fork commits");
    let mob = tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member_then_run_bounded(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), AgentIdentity::from("fork-child"))
                .with_initial_message("the forked task"),
            None,
            "result",
            4_096,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
        ),
    )
    .await
    .expect("the fork answers");
    let mob = match mob {
        Ok(outcome) => outcome,
        Err(error) => panic!("the mob fork child runs its first turn: {error:?}"),
    };

    let generic_rows = normalized_reactivation_rows(
        &fixture
            .durable(&generic.session_id)
            .await
            .expect("the generic child"),
    );
    let mob_rows = normalized_reactivation_rows(
        &fixture
            .durable(&mob.fork.session_id)
            .await
            .expect("the mob child"),
    );
    assert_eq!(generic_rows.len(), 1, "one re-activation row");
    assert_eq!(
        generic_rows, mob_rows,
        "the generic fork writes the rows the running mob fork wrote"
    );
    fixture.finish().await;
}

/// Control: a source without activation rows forks exactly as before under
/// every policy, and the child carries no activation rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_source_without_activation_rows_forks_as_before() {
    let fixture = Fixture::restored_member().await;
    fixture.run_turn("a turn to fork after").await;
    let source = fixture.source_session().await;
    let prefix = fixture
        .durable(&source)
        .await
        .expect("source")
        .messages()
        .len();
    for policy in [
        None,
        Some(meerkat_core::ForkInstructionPolicy::Refuse),
        Some(meerkat_core::ForkInstructionPolicy::ReactivateInherited),
    ] {
        let forked = fixture
            .fork_at(&source, prefix, policy)
            .await
            .expect("a source without activation rows forks");
        assert_eq!(forked.reactivated_instruction_count, 0);
        assert_eq!(forked.message_count, prefix);
        let child = fixture
            .durable(&forked.session_id)
            .await
            .expect("the child is committed");
        assert!(origin_local_rows(&child).is_empty());
    }
    fixture.finish().await;
}

/// The host-driven fork (`MobHandle::fork_member`, the path a console or RPC
/// fork wraps) seats a child whose first turn is refused for the inherited
/// key before any model request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_fork_of_an_activated_member_refuses_its_first_turn() {
    let fixture = Fixture::restored_member().await;
    fixture
        .activate(request(
            "standing-1",
            InstructionActivationExpectation::Absent,
        ))
        .await
        .expect("activation applies at a safe boundary");
    let child = AgentIdentity::from("fork-child");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), child.clone()),
            None,
        ),
    )
    .await
    .expect("the fork answers")
    .expect("the fork commits and seats the child");
    let before = fixture.client.requests().len();

    let turn = fixture.start_turn_for(&child, "the child's task").await;
    let Err(error) = tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the child's turn finishes")
    else {
        panic!("the child's turn is refused");
    };
    assert!(
        format!("{error:?}")
            .contains("forked session must explicitly activate inherited instruction keys"),
        "the child's turn is refused for the inherited key: {error:?}"
    );
    assert_eq!(
        fixture.client.requests().len(),
        before,
        "the child never reached the model"
    );
    fixture.finish().await;
}

/// On the host-driven fork the host can still activate the inherited key on
/// the child before its first turn (the explicit flow the activation design
/// names), and the child then runs with the activation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_fork_runs_after_the_host_activates_the_inherited_key() {
    let fixture = Fixture::restored_member().await;
    fixture
        .activate(request(
            "standing-1",
            InstructionActivationExpectation::Absent,
        ))
        .await
        .expect("activation applies at a safe boundary");
    let child = AgentIdentity::from("fork-child");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), child.clone()),
            None,
        ),
    )
    .await
    .expect("the fork answers")
    .expect("the fork commits and seats the child");

    let mut reactivation = request("standing-1", InstructionActivationExpectation::Absent);
    reactivation.activation_id = InstructionActivationId::new("child-standing-1").expect("id");
    reactivation.supersedes =
        Some(InstructionActivationId::new("standing-1").expect("inherited head"));
    let receipt = tokio::time::timeout(
        WAIT,
        fixture
            .handle
            .activate_member_instruction(&child, reactivation),
    )
    .await
    .expect("activation answers")
    .expect("the host activates the inherited key on the child");
    assert_eq!(
        receipt.disposition,
        InstructionActivationDisposition::Applied
    );

    let before = fixture.client.requests().len();
    let turn = fixture.start_turn_for(&child, "the child's task").await;
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the child's turn finishes")
        .expect("the child's turn runs");
    assert!(
        fixture.client.requests().len() > before,
        "the child reached the model"
    );
    fixture.finish().await;
}

/// Re-applying the effective activation is a typed Duplicate: no second
/// durable record and no accreting System row.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reapplied_activation_is_a_typed_duplicate() {
    let fixture = Fixture::restored_member().await;
    let first = fixture
        .activate(request(
            "standing-1",
            InstructionActivationExpectation::Absent,
        ))
        .await
        .expect("first activation applies");
    assert_eq!(first.disposition, InstructionActivationDisposition::Applied);
    let again = fixture
        .activate(request(
            "standing-1",
            InstructionActivationExpectation::Absent,
        ))
        .await
        .expect("a re-apply answers typed");
    assert_eq!(
        again.disposition,
        InstructionActivationDisposition::Duplicate
    );
    assert_eq!(
        again.record, first.record,
        "the duplicate names the original record"
    );
    assert_eq!(fixture.records().await, 1);

    let before = fixture.client.requests().len();
    fixture.run_turn("hello after the re-apply").await;
    let requests = fixture.client.requests();
    assert_eq!(
        system_rows_with_body(requests.get(before).expect("next turn")),
        1,
        "a re-apply never adds a second System row"
    );
    fixture.finish().await;
}

/// An activation issued mid-turn never interleaves with that turn: it waits
/// for the member's runtime turn-finalization boundary, which the running
/// turn holds, and applies only after the turn ends. The held turn's own
/// model request carries no activation; the next turn carries it once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_activation_mid_turn_waits_for_the_safe_boundary_and_never_interleaves() {
    let fixture = Fixture::restored_member().await;
    fixture.client.block();
    let before = fixture.client.requests().len();
    let held = fixture.start_turn("a turn that holds the boundary").await;
    fixture.client.wait_started(before + 1).await;

    let activation = tokio::spawn({
        let handle = fixture.handle.clone();
        let identity = fixture.identity.clone();
        async move {
            handle
                .activate_member_instruction(
                    &identity,
                    request("standing-1", InstructionActivationExpectation::Absent),
                )
                .await
        }
    });
    fixture.client.release();
    tokio::time::timeout(WAIT, held.wait())
        .await
        .expect("held turn finishes")
        .expect("held turn succeeds");
    let receipt = tokio::time::timeout(WAIT, activation)
        .await
        .expect("activation answers once the boundary is free")
        .expect("activation task")
        .expect("activation applies at the boundary after the turn");
    assert_eq!(
        receipt.disposition,
        InstructionActivationDisposition::Applied
    );

    let requests = fixture.client.requests();
    assert_eq!(
        system_rows_with_body(requests.get(before).expect("held turn request")),
        0,
        "the held turn never saw the activation"
    );
    let next_index = requests.len();
    fixture.run_turn("hello after the boundary").await;
    let requests = fixture.client.requests();
    assert_eq!(
        system_rows_with_body(requests.get(next_index).expect("next turn request")),
        1,
        "the next turn carries the activation once"
    );
    fixture.finish().await;
}

/// A session-service decorator that forgets to forward the activation seam
/// gets the trait's typed default: the activation is refused as
/// `DurabilityUnavailable` and nothing is appended, never a silent success.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_decorator_that_does_not_forward_the_seam_is_refused_typed() {
    let fixture = Fixture::restored_member_through(true, None).await;
    let error = fixture
        .activate(request(
            "standing-1",
            InstructionActivationExpectation::Absent,
        ))
        .await
        .expect_err("a non-forwarding decorator never applies silently");
    assert_eq!(
        error.admission_code(),
        Some(InstructionActivationAdmissionErrorCode::DurabilityUnavailable),
        "{error:?}"
    );
    assert_eq!(fixture.records().await, 0, "nothing was appended");
    let before = fixture.client.requests().len();
    fixture.run_turn("hello through the decorator").await;
    assert_eq!(
        system_rows_with_body(fixture.client.requests().get(before).expect("next turn")),
        0
    );
    fixture.finish().await;
}

/// A decorator that does not forward the controller pin under the turn
/// boundary gets the trait's typed default: `Unsupported`, never a no-op pin,
/// though it forwards the boundary itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_decorator_that_does_not_forward_the_controller_pin_is_refused_unsupported() {
    let fixture = Fixture::restored_member_through(true, None).await;
    let session_id = fixture
        .handle
        .resolve_bridge_session_id(&fixture.identity)
        .await
        .expect("the member's session");
    let actor = fixture
        .service
        .live_session_actor_witness(&session_id)
        .await
        .expect("the member's actor");
    let boundary = MobSessionService::acquire_runtime_turn_finalization_guard(
        &*fixture.mob_service,
        &session_id,
    )
    .await
    .expect("the forwarded boundary");
    let refused = fixture
        .mob_service
        .pin_controller_client_under_runtime_turn_boundary(&actor)
        .await
        .err();
    assert!(
        matches!(refused, Some(SessionError::Unsupported(_))),
        "{refused:?}"
    );
    drop(boundary);
    fixture.finish().await;
}

/// A member without a session binding is refused typed before any boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_member_is_refused_not_materialized() {
    let fixture = Fixture::restored_member().await;
    let error = fixture
        .handle
        .activate_member_instruction(
            &AgentIdentity::from("no-such-member"),
            request("standing-1", InstructionActivationExpectation::Absent),
        )
        .await
        .expect_err("no member, no activation");
    assert_eq!(
        error.admission_code(),
        Some(InstructionActivationAdmissionErrorCode::TargetNotMaterialized)
    );
    fixture.finish().await;
}

// Prompt-context forks (`MemberLaunchMode::Fork`): the source's history is
// rendered as text into the child's first prompt. Under the default policy
// and under `refuse` the source's System rows (its system prompt and its
// activated instruction rows) are not carried, and `refuse` refuses a fork
// whose rows hold an activation; `reactivate_inherited` carries them. The
// block ends with a notice line naming what happened.

const FORK_HEADER: &str = "[Forked conversation context from member";
const FORK_END: &str = "[End of forked context]";

/// The rendered fork-context block in `request`'s user rows, if any.
fn fork_block(request: &meerkat_client::LlmRequest) -> Option<String> {
    request.messages.iter().find_map(|message| {
        let Message::User(user) = message else {
            return None;
        };
        let text = user.text_content();
        let start = text.find(FORK_HEADER)?;
        let block = &text[start..];
        let end = block
            .find(FORK_END)
            .map_or(block.len(), |end| end + FORK_END.len());
        Some(block[..end].to_string())
    })
}

/// Whether any row of `request` carries the standing activation body.
fn carries_body(request: &meerkat_client::LlmRequest) -> bool {
    format!("{:?}", request.messages).contains(BODY)
}

/// The text of the typed fork notice `request` holds, as the model reads it.
fn fork_notice_summary(request: &meerkat_client::LlmRequest) -> Option<String> {
    request.messages.iter().find_map(|message| {
        let Message::SystemNotice(notice) = message else {
            return None;
        };
        notice.blocks.iter().find_map(|block| {
            let block = serde_json::to_value(block).expect("notice block wire json");
            (block["type"] == "fork_boundary")
                .then(|| block["summary"].as_str().unwrap_or_default().to_string())
        })
    })
}

fn context_fork_spec(
    identity: &str,
    source: &AgentIdentity,
    fork_context: crate::launch::ForkContext,
) -> SpawnMemberSpec {
    SpawnMemberSpec::new(ProfileName::from("lead"), AgentIdentity::from(identity)).with_launch_mode(
        crate::launch::MemberLaunchMode::Fork {
            source_member_id: source.clone(),
            fork_context,
        },
    )
}

impl Fixture {
    /// The standing activation and one completed turn, so the source's
    /// transcript is its system prompt, the activation row, a user row and
    /// an assistant row.
    async fn activated_source_with_a_turn(&self) {
        self.activate_standing().await;
        self.run_turn("source turn after the activation").await;
    }

    /// The first model request started after `before` requests.
    async fn first_request_after(&self, before: usize) -> meerkat_client::LlmRequest {
        self.client.wait_started(before + 1).await;
        self.client
            .requests()
            .get(before)
            .cloned()
            .expect("the child's first model request")
    }
}

/// A context fork delivered with an initial message used to render the
/// activation row, body included, and the source's system prompt into the
/// child's first prompt. Under the default policy neither is carried, the
/// conversation rows are, and the notice names the key that was not carried.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_context_fork_of_an_activated_member_carries_no_instruction_body_by_default() {
    let fixture = Fixture::restored_member().await;
    fixture.activated_source_with_a_turn().await;
    let before = fixture.client.requests().len();
    fixture
        .handle
        .spawn_spec(
            context_fork_spec(
                "context-child",
                &fixture.identity,
                crate::launch::ForkContext::FullHistory,
            )
            .with_initial_message("the forked task")
            .with_runtime_mode(crate::MobRuntimeMode::TurnDriven),
        )
        .await
        .expect("the context fork spawns");

    let request = fixture.first_request_after(before).await;
    assert!(
        !carries_body(&request),
        "no row of the child's first request carries the activation body"
    );
    let block = fork_block(&request).expect("the first request carries the fork context");
    assert!(
        block.contains("[user]: source turn after the activation"),
        "the conversation rows are carried: {block}"
    );
    assert!(
        !block.contains("[system]:"),
        "no System row of the source is rendered: {block}"
    );
    let notice = fork_notice_summary(&request).expect("the child reads its typed fork notice");
    assert!(
        notice.contains("Instructions the source had that you do not: host.standing/member")
            && notice.contains("system prompt and host system context were not carried"),
        "the notice names what was not carried: {notice}"
    );
    fixture.finish().await;
}

/// An autonomous context fork delivers the rendered context in its kickoff.
/// Under the default policy the kickoff carries no activation body either.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_autonomous_context_fork_carries_no_instruction_body_by_default() {
    let fixture = Fixture::restored_member().await;
    fixture.activated_source_with_a_turn().await;
    let before = fixture.client.requests().len();
    fixture
        .handle
        .spawn_spec(
            context_fork_spec(
                "autonomous-child",
                &fixture.identity,
                crate::launch::ForkContext::FullHistory,
            )
            .with_runtime_mode(crate::MobRuntimeMode::AutonomousHost),
        )
        .await
        .expect("the autonomous context fork spawns");

    let request = fixture.first_request_after(before).await;
    assert!(
        !carries_body(&request),
        "the kickoff carries no activation body"
    );
    let block = fork_block(&request).expect("the kickoff carries the fork context");
    let notice = fork_notice_summary(&request).expect("the kickoff reads its typed fork notice");
    assert!(
        !block.contains("[system]:") && notice.contains("host.standing/member"),
        "no System row is rendered and the notice names the key: {block} / {notice}"
    );
    fixture.finish().await;
}

/// Under `refuse`, a context fork whose rows hold an activation row is
/// refused, typed, before any spawn effect: no member, no session, no model
/// request. fork_helper surfaces the same refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_refuse_policy_refuses_a_context_fork_of_an_activated_member_before_any_effect() {
    let fixture =
        Fixture::restored_member_with_policy(meerkat_core::ForkInstructionPolicy::Refuse).await;
    fixture.activated_source_with_a_turn().await;
    let sessions_before = fixture.session_count().await;
    let requests_before = fixture.client.requests().len();

    let refused = tokio::time::timeout(
        WAIT,
        fixture.handle.spawn_spec(
            context_fork_spec(
                "refused-child",
                &fixture.identity,
                crate::launch::ForkContext::FullHistory,
            )
            .with_initial_message("the forked task"),
        ),
    )
    .await
    .expect("the spawn answers");
    match refused {
        Err(MobError::ForkInstructionRefused {
            source_member_id,
            refusal,
        }) => {
            assert_eq!(source_member_id, fixture.identity);
            assert_policy_refusal(&refusal);
        }
        other => panic!("expected a typed fork instruction refusal, got {other:?}"),
    }

    let helper = tokio::time::timeout(
        WAIT,
        fixture.handle.fork_helper(
            &fixture.identity,
            AgentIdentity::from("refused-helper"),
            "helper task text",
            crate::launch::ForkContext::FullHistory,
            HelperOptions {
                role_name: Some(ProfileName::from("lead")),
                ..HelperOptions::default()
            },
            "refused-helper-result",
            256,
        ),
    )
    .await
    .expect("fork_helper answers");
    match helper {
        Err(BoundedMemberRunError::Admission(MobError::ForkInstructionRefused {
            refusal, ..
        })) => assert_policy_refusal(&refusal),
        other => panic!("expected fork_helper's typed refusal, got {other:?}"),
    }

    for child in ["refused-child", "refused-helper"] {
        assert!(
            fixture
                .handle
                .get_member(&AgentIdentity::from(child))
                .await
                .expect("roster read")
                .is_none(),
            "{child} is not seated"
        );
    }
    assert_eq!(
        fixture.session_count().await,
        sessions_before,
        "no child session is created"
    );
    assert_eq!(
        fixture.client.requests().len(),
        requests_before,
        "no model request is made"
    );
    fixture.finish().await;
}

/// A `spawn_many` row whose prompt-context fork the policy refuses fails
/// alone, before any effect: its error keeps the typed refusal, the wire code
/// and the structured kind (the spawn-many cause is `internal`, since the mob
/// machine models no spawn cause for a fork refusal), and the other row
/// spawns.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_many_row_whose_fork_is_refused_keeps_its_typed_refusal() {
    let fixture =
        Fixture::restored_member_with_policy(meerkat_core::ForkInstructionPolicy::Refuse).await;
    fixture.activated_source_with_a_turn().await;

    let rows = tokio::time::timeout(
        WAIT,
        fixture.handle.spawn_many(vec![
            context_fork_spec(
                "refused-row",
                &fixture.identity,
                crate::launch::ForkContext::FullHistory,
            )
            .with_initial_message("the forked task"),
            SpawnMemberSpec::new(ProfileName::from("lead"), AgentIdentity::from("plain-row")),
        ]),
    )
    .await
    .expect("spawn_many answers")
    .expect("the batch is admitted");
    let [refused, plain] = rows.as_slice() else {
        panic!("one result per row: {rows:?}");
    };
    let failure = refused.as_ref().expect_err("the fork row is refused");
    match failure.error() {
        MobError::ForkInstructionRefused {
            source_member_id,
            refusal,
        } => {
            assert_eq!(source_member_id, &fixture.identity);
            assert_policy_refusal(refusal);
        }
        other => panic!("expected the typed fork instruction refusal, got {other:?}"),
    }
    assert_eq!(
        failure.cause(),
        meerkat_contracts::MobSpawnManyFailureCause::Internal
    );
    assert_eq!(
        failure.error().wire_error_code(),
        Some(meerkat_contracts::ErrorCode::ForkInstructionRefused)
    );
    assert_eq!(
        failure
            .error()
            .structured_data()
            .and_then(|data| data.get("kind").cloned()),
        Some(serde_json::json!(
            meerkat_core::service::SessionError::FORK_INSTRUCTION_REFUSED_KIND
        ))
    );
    assert!(plain.is_ok(), "the other row spawns: {plain:?}");
    assert!(
        fixture
            .handle
            .get_member(&AgentIdentity::from("refused-row"))
            .await
            .expect("roster read")
            .is_none(),
        "the refused row is not seated"
    );
    fixture.finish().await;
}

/// The budget twin: a `spawn_many` row whose fork context does not fit the
/// child's budget fails alone, before any effect. Its error keeps the
/// typed `ForkContextTooLarge`, the wire code (`INVALID_PARAMS`) and the
/// structured kind, the spawn-many cause is `internal`, and the other row
/// spawns.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_many_row_whose_fork_context_is_too_large_keeps_its_typed_refusal() {
    let fixture = Fixture::restored_member_with_policy(
        meerkat_core::ForkInstructionPolicy::ReactivateInherited,
    )
    .await;
    // The source's newest row is an activation whose body alone fills the
    // budget: `reactivate_inherited` carries it, so it cannot fit.
    let body = "x".repeat(meerkat_core::MAX_INSTRUCTION_BODY_BYTES);
    let mut large = request("large-1", InstructionActivationExpectation::Absent);
    large.revision.content_sha256 = InstructionContentDigest::for_body(&body);
    large.body = body;
    fixture
        .activate(large)
        .await
        .expect("the large activation applies");

    let rows = tokio::time::timeout(
        WAIT,
        fixture.handle.spawn_many(vec![
            context_fork_spec(
                "too-large-row",
                &fixture.identity,
                crate::launch::ForkContext::FullHistory,
            )
            .with_initial_message("the forked task"),
            SpawnMemberSpec::new(ProfileName::from("lead"), AgentIdentity::from("plain-row")),
        ]),
    )
    .await
    .expect("spawn_many answers")
    .expect("the batch is admitted");
    let [refused, plain] = rows.as_slice() else {
        panic!("one result per row: {rows:?}");
    };
    let failure = refused.as_ref().expect_err("the fork row is refused");
    match failure.error() {
        MobError::ForkContextTooLarge {
            source_member_id,
            max_bytes,
            required_bytes,
        } => {
            assert_eq!(source_member_id, &fixture.identity);
            assert!(required_bytes > max_bytes, "{required_bytes} > {max_bytes}");
        }
        other => panic!("expected the typed budget refusal, got {other:?}"),
    }
    assert_eq!(
        failure.cause(),
        meerkat_contracts::MobSpawnManyFailureCause::Internal
    );
    assert_eq!(
        failure.error().wire_error_code(),
        Some(meerkat_contracts::ErrorCode::InvalidParams)
    );
    assert_eq!(
        failure
            .error()
            .structured_data()
            .and_then(|data| data.get("kind").cloned()),
        Some(serde_json::json!("fork_context_too_large"))
    );
    assert!(plain.is_ok(), "the other row spawns: {plain:?}");
    assert!(
        fixture
            .handle
            .get_member(&AgentIdentity::from("too-large-row"))
            .await
            .expect("roster read")
            .is_none(),
        "the refused row is not seated"
    );
    fixture.finish().await;
}

/// `reactivate_inherited` carries the source's activated instructions, body
/// included, and the notice says so. The source's system prompt is never
/// carried, under any policy.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reactivate_inherited_keeps_instruction_bodies_in_a_context_fork() {
    let fixture = Fixture::restored_member_with_policy(
        meerkat_core::ForkInstructionPolicy::ReactivateInherited,
    )
    .await;
    fixture.activated_source_with_a_turn().await;
    let before = fixture.client.requests().len();
    fixture
        .handle
        .spawn_spec(
            context_fork_spec(
                "carrying-child",
                &fixture.identity,
                crate::launch::ForkContext::FullHistory,
            )
            .with_initial_message("the forked task")
            .with_runtime_mode(crate::MobRuntimeMode::TurnDriven),
        )
        .await
        .expect("the context fork spawns");

    let request = fixture.first_request_after(before).await;
    assert!(carries_body(&request), "the activation body is carried");
    let block = fork_block(&request).expect("the first request carries the fork context");
    assert!(
        block.contains("[system]: [meerkat-instruction-activation-v1]"),
        "the activation row is carried: {block}"
    );
    assert!(
        !block.contains("[system]: You are"),
        "the source's system prompt is not carried: {block}"
    );
    let notice = fork_notice_summary(&request).expect("the child reads its typed fork notice");
    assert!(
        notice.contains(
            "Instructions of the source carried in the context above: host.standing/member"
        ) && notice.contains("system prompt and host system context were not carried"),
        "the notice says what was and was not carried: {notice}"
    );
    fixture.finish().await;
}

/// `refuse` decides on the rows the fork would render: a `LastMessages` tail
/// without an activation row carries no body and forks; a tail that reaches
/// the activation row is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_last_messages_context_fork_is_refused_only_when_its_tail_holds_an_activation_row() {
    let fixture =
        Fixture::restored_member_with_policy(meerkat_core::ForkInstructionPolicy::Refuse).await;
    fixture.activated_source_with_a_turn().await;

    fixture
        .handle
        .spawn_spec(
            context_fork_spec(
                "tail-of-two",
                &fixture.identity,
                crate::launch::ForkContext::LastMessages { count: 2 },
            )
            .with_initial_message("the forked task"),
        )
        .await
        .expect("a tail of the user and assistant rows forks");

    let refused = fixture
        .handle
        .spawn_spec(
            context_fork_spec(
                "tail-of-three",
                &fixture.identity,
                crate::launch::ForkContext::LastMessages { count: 3 },
            )
            .with_initial_message("the forked task"),
        )
        .await;
    match refused {
        Err(MobError::ForkInstructionRefused { refusal, .. }) => {
            assert_policy_refusal(&refusal);
        }
        other => panic!("a tail holding the activation row is refused, got {other:?}"),
    }
    fixture.finish().await;
}

/// Host-injected context (here a per-turn injected row, the carrier of
/// memory recall) is host-attached, not conversation: under the default
/// policy a context fork does not carry it, and the notice counts it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_context_fork_does_not_carry_host_injected_context_by_default() {
    let fixture = Fixture::restored_member().await;
    fixture.activated_source_with_a_turn().await;
    let delivery = crate::store::MobDeliveryIdentity::new(
        "injected-context-probe",
        "7d2c4f0e-5b1a-4c3e-9f2d-6a8b1c0e4d71",
    )
    .expect("canonical delivery identity");
    let injected = fixture
        .handle
        .start_injected_context_work_for_identity_bounded(
            fixture.identity.clone(),
            WorkSpec::new("INJECTED-RECALL-MARKER", WorkOrigin::Internal),
            delivery,
            BoundedResultSpec::new("injected-result", 4_096).expect("result spec"),
        )
        .await
        .expect("the injected work is admitted");
    tokio::time::timeout(WAIT, injected.wait())
        .await
        .expect("the injected turn finishes")
        .expect("the injected turn succeeds");

    let before = fixture.client.requests().len();
    fixture
        .handle
        .spawn_spec(
            context_fork_spec(
                "injected-child",
                &fixture.identity,
                crate::launch::ForkContext::FullHistory,
            )
            .with_initial_message("the forked task")
            .with_runtime_mode(crate::MobRuntimeMode::TurnDriven),
        )
        .await
        .expect("the context fork spawns");

    let request = fixture.first_request_after(before).await;
    let block = fork_block(&request).expect("the first request carries the fork context");
    assert!(
        !block.contains("INJECTED-RECALL-MARKER"),
        "the injected row is not carried: {block}"
    );
    let notice = fork_notice_summary(&request).expect("the child reads its typed fork notice");
    assert!(
        notice.contains("Host-injected context rows of the source not carried: 1"),
        "the notice counts it: {notice}"
    );
    fixture.finish().await;
}

/// A turn-driven fork_helper is created with no initial message. Its fork
/// context used to go into a create request that discards its prompt, so the
/// helper saw only its task. The planned context now reaches the helper's
/// task turn, without the activation body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_driven_fork_helper_sees_its_planned_context() {
    let fixture = Fixture::restored_member().await;
    fixture.activated_source_with_a_turn().await;
    let before = fixture.client.requests().len();
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_helper(
            &fixture.identity,
            AgentIdentity::from("context-helper"),
            "helper task text",
            crate::launch::ForkContext::FullHistory,
            HelperOptions {
                role_name: Some(ProfileName::from("lead")),
                runtime_mode: Some(crate::MobRuntimeMode::TurnDriven),
                ..HelperOptions::default()
            },
            "context-helper-result",
            256,
        ),
    )
    .await
    .expect("fork_helper answers")
    .expect("fork_helper succeeds");

    let request = fixture
        .client
        .requests()
        .get(before)
        .cloned()
        .expect("the helper's model request");
    let block = fork_block(&request).expect("the helper's task turn carries the fork context");
    assert!(
        format!("{:?}", request.messages).contains("helper task text"),
        "the task follows the context"
    );
    assert!(
        !carries_body(&request) && !block.contains("[system]:"),
        "the context carries no System row of the source: {block}"
    );
    let notice = fork_notice_summary(&request).expect("the helper reads its typed fork notice");
    assert!(
        notice.contains("host.standing/member"),
        "the notice names the key: {notice}"
    );
    fixture.finish().await;
}

// Slice B: a fork child holds what it was given as typed rows of its own
// transcript, delivered once by Core before its first input: the planned
// context as an injected-context row (prompt-context forks) and a typed
// fork notice naming its source and what it lost or gained.

impl Fixture {
    /// The child's durable transcript once its first turn committed: a user
    /// row containing `needle` and an answer after it. A read that races the
    /// child's own commit is retried.
    async fn transcript_after_first_turn(
        &self,
        child: &AgentIdentity,
        needle: &str,
    ) -> (SessionId, Vec<Message>) {
        tokio::time::timeout(WAIT, async {
            loop {
                if let Some(id) = self.member_session(child).await
                    && let Ok(Some(session)) = self.service.load_durable_session_body(&id).await
                {
                    let rows = session.messages().to_vec();
                    let task = rows.iter().position(
                        |row| matches!(row, Message::User(user) if user.text_content().contains(needle)),
                    );
                    if let Some(task) = task
                        && rows[task..]
                            .iter()
                            .any(|row| matches!(row, Message::BlockAssistant(_)))
                    {
                        return (id, rows);
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the child's first turn commits")
    }
}

/// The typed fork notice blocks in `rows`, as wire JSON, with their row index.
fn fork_notices(rows: &[Message]) -> Vec<(usize, serde_json::Value)> {
    rows.iter()
        .enumerate()
        .filter_map(|(index, row)| {
            let Message::SystemNotice(notice) = row else {
                return None;
            };
            notice
                .blocks
                .iter()
                .map(|block| serde_json::to_value(block).expect("notice block wire json"))
                .find(|block| block["type"] == "fork_boundary")
                .map(|block| (index, block))
        })
        .collect()
}

/// The index of the first user row containing `needle`.
fn user_row(rows: &[Message], needle: &str) -> Option<usize> {
    rows.iter()
        .position(|row| matches!(row, Message::User(user) if user.text_content().contains(needle)))
}

async fn spawn_context_child_with_task(
    fixture: &Fixture,
    child: &str,
    source: &AgentIdentity,
    task: &str,
) {
    fixture
        .handle
        .spawn_spec(
            context_fork_spec(child, source, crate::launch::ForkContext::FullHistory)
                .with_initial_message(task)
                .with_runtime_mode(crate::MobRuntimeMode::TurnDriven),
        )
        .await
        .expect("the context fork spawns");
}

/// The planned context is the child's own typed injected-context row, before
/// its first input, and the first input is a row of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_context_fork_child_holds_its_context_as_an_injected_row_before_its_first_input() {
    let fixture = Fixture::restored_member().await;
    fixture.activated_source_with_a_turn().await;
    let child = AgentIdentity::from("context-child");
    spawn_context_child_with_task(
        &fixture,
        "context-child",
        &fixture.identity,
        "the forked task",
    )
    .await;

    let (_, rows) = fixture
        .transcript_after_first_turn(&child, "the forked task")
        .await;
    let injected = rows.iter().position(|row| {
        matches!(
            row,
            Message::User(user)
                if user.transcript_role == meerkat_core::types::TranscriptUserRole::InjectedContext
                    && user.text_content().contains(FORK_HEADER)
        )
    });
    let task = user_row(&rows, "the forked task");
    let (Some(injected), Some(task)) = (injected, task) else {
        panic!("the context is a typed injected row and the task a row of its own: {rows:#?}");
    };
    assert!(
        injected < task,
        "the context precedes the first input: {rows:#?}"
    );
    let Message::User(task_row) = &rows[task] else {
        unreachable!("user_row found a user row");
    };
    assert!(
        !task_row.text_content().contains(FORK_HEADER),
        "the first input carries no context: {rows:#?}"
    );
    fixture.finish().await;
}

/// The child holds exactly one typed fork notice, after its context and
/// before its first input, naming its source, itself and the key it did not
/// get; the model reads it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_context_fork_child_holds_one_typed_fork_notice_naming_its_source() {
    let fixture = Fixture::restored_member().await;
    fixture.activated_source_with_a_turn().await;
    let child = AgentIdentity::from("context-child");
    spawn_context_child_with_task(
        &fixture,
        "context-child",
        &fixture.identity,
        "the forked task",
    )
    .await;

    let (child_session, rows) = fixture
        .transcript_after_first_turn(&child, "the forked task")
        .await;
    let notices = fork_notices(&rows);
    assert_eq!(notices.len(), 1, "exactly one fork notice row: {rows:#?}");
    let (at, block) = &notices[0];
    assert_eq!(block["fork"], "prompt_context", "{block}");
    assert_eq!(
        block["child_session_id"],
        child_session.to_string(),
        "{block}"
    );
    assert_eq!(
        block["source"]["member"]["member"], "restored-member",
        "{block}"
    );
    assert_eq!(
        block["instructions_removed"][0]["namespace"], "host.standing",
        "{block}"
    );
    assert_eq!(block["instructions_removed"][0]["key"], "member", "{block}");
    let task = user_row(&rows, "the forked task").expect("the task row");
    assert!(*at < task, "the notice precedes the first input: {rows:#?}");

    let request = fixture
        .client
        .requests()
        .into_iter()
        .find(|request| format!("{:?}", request.messages).contains("the forked task"))
        .expect("the child's first request");
    assert!(
        fork_notices(&request.messages).len() == 1,
        "the model reads the notice row: {:?}",
        request.messages
    );
    fixture.finish().await;
}

/// A durable member fork's child holds one typed fork notice of kind
/// transcript at its first turn: after the copied prefix, before its input.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_durable_fork_child_holds_a_fork_notice_from_its_first_turn() {
    let fixture = Fixture::restored_member().await;
    fixture.run_turn("source turn before the fork").await;
    let child = AgentIdentity::from("fork-child");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), child.clone()),
            None,
        ),
    )
    .await
    .expect("the fork answers")
    .expect("the fork commits and seats the child");
    let turn = fixture.start_turn_for(&child, "the child's task").await;
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the child's turn finishes")
        .expect("the child's turn runs");

    let (child_session, rows) = fixture
        .transcript_after_first_turn(&child, "the child's task")
        .await;
    let notices = fork_notices(&rows);
    assert_eq!(notices.len(), 1, "exactly one fork notice row: {rows:#?}");
    let (at, block) = &notices[0];
    assert_eq!(block["fork"], "transcript", "{block}");
    assert_eq!(
        block["child_session_id"],
        child_session.to_string(),
        "{block}"
    );
    assert_eq!(
        block["source"]["member"]["member"], "restored-member",
        "{block}"
    );
    let copied = user_row(&rows, "source turn before the fork").expect("the copied prefix");
    let task = user_row(&rows, "the child's task").expect("the task row");
    assert!(
        copied < *at && *at < task,
        "the notice follows the copied prefix and precedes the first input: {rows:#?}"
    );
    fixture.finish().await;
}

/// The fork notice names, by name only, a tool the source could call and the
/// child cannot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_notice_names_a_tool_the_child_cannot_call() {
    let fixture = Fixture::restored_member().await;
    fixture.run_turn("source turn before the fork").await;
    let denied = fixture
        .client
        .requests()
        .last()
        .and_then(|request| request.tools.iter().map(|tool| tool.name.to_string()).min())
        .expect("the source can call a tool");
    let before = fixture.client.requests().len();
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_helper(
            &fixture.identity,
            AgentIdentity::from("narrow-helper"),
            "helper task text",
            crate::launch::ForkContext::FullHistory,
            HelperOptions {
                role_name: Some(ProfileName::from("lead")),
                runtime_mode: Some(crate::MobRuntimeMode::TurnDriven),
                tool_access_policy: Some(meerkat_core::ops::ToolAccessPolicy::DenyList(
                    [denied.as_str()].into_iter().collect(),
                )),
                ..HelperOptions::default()
            },
            "narrow-helper-result",
            256,
        ),
    )
    .await
    .expect("fork_helper answers")
    .expect("fork_helper succeeds");

    let request = fixture
        .client
        .requests()
        .get(before)
        .cloned()
        .expect("the helper's first request");
    assert!(
        request.tools.iter().all(|tool| tool.name != denied),
        "the helper cannot call {denied}"
    );
    let notices = fork_notices(&request.messages);
    assert_eq!(notices.len(), 1, "one fork notice: {:?}", request.messages);
    let removed = notices[0].1["tools"]["removed"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        removed.iter().any(|name| name == denied.as_str()),
        "the notice names {denied} as removed: {}",
        notices[0].1
    );
    fixture.finish().await;
}

/// A fork of a fork reads one current notice, naming its own source: the
/// grandparent's context and notice in its source's transcript are not
/// carried as text.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_of_a_fork_reads_one_current_notice() {
    let fixture = Fixture::restored_member().await;
    fixture.activated_source_with_a_turn().await;
    let child = AgentIdentity::from("context-child");
    spawn_context_child_with_task(
        &fixture,
        "context-child",
        &fixture.identity,
        "the forked task",
    )
    .await;
    fixture
        .transcript_after_first_turn(&child, "the forked task")
        .await;

    let before = fixture.client.requests().len();
    spawn_context_child_with_task(
        &fixture,
        "context-grandchild",
        &child,
        "the grandchild task",
    )
    .await;
    let request = fixture.first_request_after(before).await;
    let text = format!("{:?}", request.messages);
    assert_eq!(
        text.matches("You are a fork of member").count(),
        1,
        "one notice reaches the grandchild: {text}"
    );
    assert!(
        text.contains("You are a fork of member 'context-child'"),
        "the notice names the grandchild's own source: {text}"
    );
    fixture.finish().await;
}

/// Under `reactivate_inherited` a durable fork child's model reads the
/// activation body once, from the child's own re-activation row: the
/// inherited row it copied is a header that names the key, without the body.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_durable_fork_child_request_carries_inherited_rows_without_bodies() {
    let fixture = Fixture::restored_member_with_policy(
        meerkat_core::ForkInstructionPolicy::ReactivateInherited,
    )
    .await;
    fixture.activated_source_with_a_turn().await;
    let child = AgentIdentity::from("fork-child");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), child.clone()),
            None,
        ),
    )
    .await
    .expect("the fork answers")
    .expect("the fork commits and seats the child");
    let before = fixture.client.requests().len();
    let turn = fixture.start_turn_for(&child, "the child's task").await;
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the child's turn finishes")
        .expect("the child's turn runs");

    let request = fixture.first_request_after(before).await;
    assert_eq!(
        system_rows_with_body(&request),
        1,
        "the body reaches the child's model once: {:?}",
        request.messages
    );
    let headers = request
        .messages
        .iter()
        .filter_map(|message| match message {
            Message::System(system)
                if system
                    .content
                    .starts_with("[meerkat-inherited-instruction-v2]") =>
            {
                Some(system.content.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(headers.len(), 1, "one inherited header: {headers:?}");
    assert!(
        headers[0].contains("status=not_active_in_child")
            && headers[0].contains("\"member\"")
            && !headers[0].contains(BODY),
        "the header names the key, not the body: {}",
        headers[0]
    );
    fixture.finish().await;
}

/// A durable fork of a durable fork child reads one current notice, naming
/// its own source: the notice its source was given is history there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_durable_fork_of_a_fork_reads_one_current_notice() {
    let fixture = Fixture::restored_member().await;
    fixture.run_turn("source turn before the forks").await;
    let child = AgentIdentity::from("durable-child");
    let grandchild = AgentIdentity::from("durable-grandchild");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), child.clone()),
            None,
        ),
    )
    .await
    .expect("the fork answers")
    .expect("the fork commits");
    let turn = fixture.start_turn_for(&child, "the child's task").await;
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the child's turn finishes")
        .expect("the child's turn runs");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &child,
            SpawnMemberSpec::new(ProfileName::from("lead"), grandchild.clone()),
            None,
        ),
    )
    .await
    .expect("the second fork answers")
    .expect("the second fork commits");
    let before = fixture.client.requests().len();
    let turn = fixture
        .start_turn_for(&grandchild, "the grandchild's task")
        .await;
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the grandchild's turn finishes")
        .expect("the grandchild's turn runs");

    let request = fixture.first_request_after(before).await;
    let text = format!("{:?}", request.messages);
    assert_eq!(
        text.matches("You are a fork of").count(),
        1,
        "one current notice reaches the grandchild: {text}"
    );
    assert!(
        text.contains("You are a fork of member 'durable-child'"),
        "the notice names the grandchild's own source: {text}"
    );
    fixture.finish().await;
}

/// A durable fork of a durable fork child names, as its source's
/// instructions, only the keys that child activated itself: the rows it
/// copied from its own source are history there.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_durable_fork_of_a_fork_names_only_its_sources_own_keys() {
    let fixture = Fixture::restored_member().await;
    fixture.activated_source_with_a_turn().await;
    let child = AgentIdentity::from("durable-child");
    let grandchild = AgentIdentity::from("durable-grandchild");
    for (source, fork) in [(&fixture.identity, &child), (&child, &grandchild)] {
        tokio::time::timeout(
            WAIT,
            fixture.handle.fork_member(
                source,
                SpawnMemberSpec::new(ProfileName::from("lead"), fork.clone()),
                None,
            ),
        )
        .await
        .expect("the fork answers")
        .expect("the fork commits");
    }
    let session = fixture
        .member_session(&grandchild)
        .await
        .expect("the grandchild's session");
    let body = fixture
        .service
        .load_durable_session_body(&session)
        .await
        .expect("the grandchild loads")
        .expect("the grandchild is durable");
    let seed: meerkat_core::fork_notice::ForkSeed = serde_json::from_value(
        body.metadata()
            .get(meerkat_core::fork_notice::SESSION_FORK_SEED_KEY)
            .cloned()
            .expect("the grandchild holds a fork seed"),
    )
    .expect("the seed decodes");
    assert!(
        seed.source_instructions.is_empty(),
        "the child never activated the key it copied: {:?}",
        seed.source_instructions
    );
    fixture.finish().await;
}

/// A prompt-context fork of a durable fork child names, as its source's
/// instructions, only the keys that child activated itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_context_fork_of_a_fork_names_only_its_sources_own_keys() {
    let fixture = Fixture::restored_member().await;
    fixture.activated_source_with_a_turn().await;
    let child = AgentIdentity::from("durable-child");
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_member(
            &fixture.identity,
            SpawnMemberSpec::new(ProfileName::from("lead"), child.clone()),
            None,
        ),
    )
    .await
    .expect("the fork answers")
    .expect("the fork commits");
    let before = fixture.client.requests().len();
    tokio::time::timeout(
        WAIT,
        fixture.handle.fork_helper(
            &child,
            AgentIdentity::from("helper-of-the-child"),
            "helper task text",
            crate::launch::ForkContext::FullHistory,
            HelperOptions {
                role_name: Some(ProfileName::from("lead")),
                runtime_mode: Some(crate::MobRuntimeMode::TurnDriven),
                ..HelperOptions::default()
            },
            "helper-of-the-child-result",
            256,
        ),
    )
    .await
    .expect("fork_helper answers")
    .expect("fork_helper succeeds");
    let request = fixture
        .client
        .requests()
        .get(before)
        .cloned()
        .expect("the helper's first request");
    let notice = fork_notice_summary(&request).expect("the helper reads its fork notice");
    assert!(
        !notice.contains("host.standing/member"),
        "the child never activated the key it copied: {notice}"
    );
    fixture.finish().await;
}

/// A source forked while its own turn runs (as when a member forks itself
/// from one of its tool calls) still has its tools compared: the notice
/// reads the tools the source's session last published, without waiting on
/// that turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_of_a_running_source_names_a_tool_the_child_cannot_call() {
    let fixture = Fixture::restored_member().await;
    fixture.run_turn("source turn before the fork").await;
    let denied = fixture
        .client
        .requests()
        .last()
        .and_then(|request| request.tools.iter().map(|tool| tool.name.to_string()).min())
        .expect("the source can call a tool");
    fixture.client.block();
    let before = fixture.client.requests().len();
    let held = fixture
        .start_turn("a source turn that runs through the fork")
        .await;
    fixture.client.wait_started(before + 1).await;

    let helper = tokio::spawn({
        let handle = fixture.handle.clone();
        let identity = fixture.identity.clone();
        let denied = denied.clone();
        async move {
            handle
                .fork_helper(
                    &identity,
                    AgentIdentity::from("running-source-helper"),
                    "helper task text",
                    crate::launch::ForkContext::FullHistory,
                    HelperOptions {
                        role_name: Some(ProfileName::from("lead")),
                        runtime_mode: Some(crate::MobRuntimeMode::TurnDriven),
                        tool_access_policy: Some(meerkat_core::ops::ToolAccessPolicy::DenyList(
                            [denied.as_str()].into_iter().collect(),
                        )),
                        ..HelperOptions::default()
                    },
                    "running-source-helper-result",
                    256,
                )
                .await
        }
    });
    // The helper's first model request starts while the source's turn is
    // still held: the child was created, and its notice built, mid-turn.
    fixture.client.wait_started(before + 2).await;
    fixture.client.release();
    tokio::time::timeout(WAIT, held.wait())
        .await
        .expect("the source's turn finishes")
        .expect("the source's turn runs");
    tokio::time::timeout(WAIT, helper)
        .await
        .expect("fork_helper answers")
        .expect("fork_helper task")
        .expect("fork_helper succeeds");

    let request = fixture
        .client
        .requests()
        .into_iter()
        .skip(before)
        .find(|request| format!("{:?}", request.messages).contains("helper task text"))
        .expect("the helper's first request");
    let notices = fork_notices(&request.messages);
    assert_eq!(notices.len(), 1, "one fork notice: {:?}", request.messages);
    let tools = &notices[0].1["tools"];
    assert_eq!(
        tools["comparison"], "compared",
        "the running source's tools are compared: {}",
        notices[0].1
    );
    assert!(
        tools["removed"]
            .as_array()
            .is_some_and(|removed| removed.iter().any(|name| name == denied.as_str())),
        "the notice names {denied} as removed: {}",
        notices[0].1
    );
    fixture.finish().await;
}

/// Forwards every session-service method to the real persistent service
/// except `activate_instruction_under_runtime_turn_boundary`, which keeps the
/// trait default. Generated mechanically from the trait signatures.
struct NonForwardingDecorator {
    inner: Arc<Service>,
}

#[async_trait]
impl meerkat_core::service::SessionService for NonForwardingDecorator {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        <Service as meerkat_core::service::SessionService>::create_session(&self.inner, req).await
    }
    async fn start_turn(
        &self,
        id: &SessionId,
        req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        <Service as meerkat_core::service::SessionService>::start_turn(&self.inner, id, req).await
    }
    async fn reconcile_runtime_compaction_projections(
        &self,
        id: &SessionId,
        intents: Vec<meerkat_core::CompactionProjectionIntent>,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::reconcile_runtime_compaction_projections(&self.inner, id, intents).await
    }
    async fn abort_uncommitted_compaction_projections(
        &self,
        id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::abort_uncommitted_compaction_projections(&self.inner, id).await
    }
    async fn abort_rejected_runtime_run_projections(
        &self,
        id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::abort_rejected_runtime_run_projections(
            &self.inner,
            id,
        )
        .await
    }
    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::interrupt(&self.inner, id).await
    }
    async fn interrupt_run_if_current(
        &self,
        id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
    ) -> Result<bool, SessionError> {
        <Service as meerkat_core::service::SessionService>::interrupt_run_if_current(
            &self.inner,
            id,
            expected_run_id,
        )
        .await
    }
    async fn cancel_after_boundary(&self, id: &SessionId) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::cancel_after_boundary(&self.inner, id)
            .await
    }
    async fn cancel_after_boundary_for_run(
        &self,
        id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::cancel_after_boundary_for_run(
            &self.inner,
            id,
            expected_run_id,
        )
        .await
    }
    async fn set_session_client(
        &self,
        id: &SessionId,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::set_session_client(
            &self.inner,
            id,
            client,
        )
        .await
    }
    async fn hot_swap_session_llm_identity(
        &self,
        id: &SessionId,
        client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        identity: SessionLlmIdentity,
        request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::hot_swap_session_llm_identity(
            &self.inner,
            id,
            client,
            identity,
            request_policy,
        )
        .await
    }
    async fn set_session_tool_visibility_state(
        &self,
        id: &SessionId,
        state: Option<meerkat_core::SessionToolVisibilityState>,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::set_session_tool_visibility_state(
            &self.inner,
            id,
            state,
        )
        .await
    }
    async fn update_session_mob_authority_context(
        &self,
        id: &SessionId,
        authority_context: Option<MobToolAuthorityContext>,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::update_session_mob_authority_context(
            &self.inner,
            id,
            authority_context,
        )
        .await
    }
    async fn has_live_session(&self, id: &SessionId) -> Result<bool, SessionError> {
        <Service as meerkat_core::service::SessionService>::has_live_session(&self.inner, id).await
    }
    async fn set_session_tool_filter(
        &self,
        id: &SessionId,
        filter: meerkat_core::ToolFilter,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::set_session_tool_filter(
            &self.inner,
            id,
            filter,
        )
        .await
    }
    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        <Service as meerkat_core::service::SessionService>::read(&self.inner, id).await
    }
    async fn list(&self, query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        <Service as meerkat_core::service::SessionService>::list(&self.inner, query).await
    }
    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::archive(&self.inner, id).await
    }
    async fn subscribe_session_events(&self, id: &SessionId) -> Result<EventStream, StreamError> {
        <Service as meerkat_core::service::SessionService>::subscribe_session_events(
            &self.inner,
            id,
        )
        .await
    }
    async fn subscribe_session_events_from(
        &self,
        id: &SessionId,
        cursor: meerkat_core::comms::SessionEventCursor,
    ) -> Result<meerkat_core::comms::SessionEventSubscription, StreamError> {
        <Service as meerkat_core::service::SessionService>::subscribe_session_events_from(
            &self.inner,
            id,
            cursor,
        )
        .await
    }
    async fn record_live_terminal_error(
        &self,
        id: &SessionId,
        cause: meerkat_core::live_adapter::LiveAdapterErrorCode,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::record_live_terminal_error(
            &self.inner,
            id,
            cause,
        )
        .await
    }
    async fn record_live_output_audio_degraded(
        &self,
        id: &SessionId,
        dropped: u64,
    ) -> Result<(), SessionError> {
        <Service as meerkat_core::service::SessionService>::record_live_output_audio_degraded(
            &self.inner,
            id,
            dropped,
        )
        .await
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceCommsExt for NonForwardingDecorator {
    async fn comms_runtime(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::agent::CommsRuntime>> {
        <Service as meerkat_core::service::SessionServiceCommsExt>::comms_runtime(
            &self.inner,
            session_id,
        )
        .await
    }
    async fn send_comms(
        &self,
        session_id: &SessionId,
        command: meerkat_core::CommsCommand,
    ) -> Option<Result<meerkat_core::SendReceipt, meerkat_core::SendError>> {
        <Service as meerkat_core::service::SessionServiceCommsExt>::send_comms(
            &self.inner,
            session_id,
            command,
        )
        .await
    }
    async fn event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::EventInjector>> {
        <Service as meerkat_core::service::SessionServiceCommsExt>::event_injector(
            &self.inner,
            session_id,
        )
        .await
    }
    #[doc(hidden)]
    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn meerkat_core::event_injector::SubscribableInjector>> {
        <Service as meerkat_core::service::SessionServiceCommsExt>::interaction_event_injector(
            &self.inner,
            session_id,
        )
        .await
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceControlExt for NonForwardingDecorator {
    async fn append_system_context(
        &self,
        id: &SessionId,
        req: AppendSystemContextRequest,
    ) -> Result<AppendSystemContextResult, SessionControlError> {
        <Service as meerkat_core::service::SessionServiceControlExt>::append_system_context(
            &self.inner,
            id,
            req,
        )
        .await
    }
    async fn stage_tool_results(
        &self,
        id: &SessionId,
        req: StageToolResultsRequest,
    ) -> Result<StageToolResultsResult, SessionError> {
        <Service as meerkat_core::service::SessionServiceControlExt>::stage_tool_results(
            &self.inner,
            id,
            req,
        )
        .await
    }
}

#[async_trait]
impl meerkat_core::service::SessionServiceHistoryExt for NonForwardingDecorator {
    async fn read_instruction_activation_records(
        &self,
        id: &SessionId,
        query: meerkat_core::InstructionActivationReadQuery,
    ) -> Result<meerkat_core::InstructionActivationReadPage, SessionError> {
        <Service as meerkat_core::service::SessionServiceHistoryExt>::read_instruction_activation_records(&self.inner, id, query).await
    }
    async fn read_history(
        &self,
        id: &SessionId,
        query: SessionHistoryQuery,
    ) -> Result<SessionHistoryPage, SessionError> {
        <Service as meerkat_core::service::SessionServiceHistoryExt>::read_history(
            &self.inner,
            id,
            query,
        )
        .await
    }
    async fn read_transcript_revision(
        &self,
        id: &SessionId,
        query: SessionTranscriptRevisionQuery,
    ) -> Result<SessionTranscriptRevisionPage, SessionError> {
        <Service as meerkat_core::service::SessionServiceHistoryExt>::read_transcript_revision(
            &self.inner,
            id,
            query,
        )
        .await
    }
    async fn list_transcript_revisions(
        &self,
        id: &SessionId,
        query: SessionTranscriptRevisionListQuery,
    ) -> Result<SessionTranscriptRevisionList, SessionError> {
        <Service as meerkat_core::service::SessionServiceHistoryExt>::list_transcript_revisions(
            &self.inner,
            id,
            query,
        )
        .await
    }
}

#[async_trait]
impl MobSessionService for NonForwardingDecorator {
    async fn append_system_notice_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
        record: meerkat_core::types::SystemNoticeRecord,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::append_system_notice_under_runtime_turn_boundary(
            &self.inner,
            session_id,
            record,
        )
        .await
    }
    async fn commit_live_delegation_final_transcript(
        &self,
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
    ) -> Result<meerkat_core::FinalLiveUserTranscriptCommitEvidence, SessionError> {
        <Service as MobSessionService>::commit_live_delegation_final_transcript(
            &self.inner,
            machine,
            session_id,
            provisional,
            final_event,
        )
        .await
    }
    async fn commit_live_delegation_final_transcript_at_turn_boundary(
        &self,
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
        bound: std::time::Duration,
    ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
        <Service as MobSessionService>::commit_live_delegation_final_transcript_at_turn_boundary(
            &self.inner,
            machine,
            session_id,
            provisional,
            final_event,
            bound,
        )
        .await
    }
    async fn commit_live_delegation_represented_transcript_at_turn_boundary(
        &self,
        machine: &meerkat_runtime::MeerkatMachine,
        session_id: &SessionId,
        provisional: meerkat_core::ProvisionalLiveHandoff,
        final_event: meerkat_core::RealtimeTranscriptEvent,
        represented: Vec<meerkat_core::RepresentedLiveUserRow>,
        bound: std::time::Duration,
    ) -> Result<meerkat_core::LiveFinalTranscriptCommitAtTurnBoundary, SessionError> {
        <Service as MobSessionService>::commit_live_delegation_represented_transcript_at_turn_boundary(&self.inner, machine, session_id, provisional, final_event, represented, bound).await
    }
    #[cfg(feature = "openai-live")]
    async fn validate_live_bridge_member_eligibility(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::validate_live_bridge_member_eligibility(
            &self.inner,
            session_id,
        )
        .await
    }
    #[cfg(feature = "openai-live")]
    async fn capture_live_bridge_execution_snapshot(
        &self,
        session_id: &SessionId,
        agent_identity: &str,
    ) -> Result<super::LiveBridgeExecutionSnapshot, SessionError> {
        <Service as MobSessionService>::capture_live_bridge_execution_snapshot(
            &self.inner,
            session_id,
            agent_identity,
        )
        .await
    }
    #[cfg(feature = "openai-live")]
    async fn start_live_bridge_member_operation(
        &self,
        request: super::LiveBridgeOperationRequest,
        cancellation: super::LiveBridgeOperationCancellationSignal,
    ) -> Result<super::LiveBridgeOperationTerminalFuture, super::LiveBridgeOperationStartError>
    {
        <Service as MobSessionService>::start_live_bridge_member_operation(
            &self.inner,
            request,
            cancellation,
        )
        .await
    }
    fn forked_participant_source_runtime(
        self: Arc<Self>,
    ) -> Option<Arc<dyn crate::forked_participant::ForkedParticipantSourceRuntime>> {
        <Service as MobSessionService>::forked_participant_source_runtime(Arc::clone(&self.inner))
    }
    async fn create_session_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Service as MobSessionService>::create_session_under_runtime_turn_boundary(&self.inner, req)
            .await
    }
    async fn create_session_with_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        resume_preparation: Option<SessionResumePreparationReceipt>,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Service as MobSessionService>::create_session_with_actor_witness_under_runtime_turn_boundary(&self.inner, req, resume_preparation, actor_witness_slot).await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Service as MobSessionService>::create_session_with_machine_archived_resume_authority(
            &self.inner,
            req,
            authorization,
        )
        .await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Service as MobSessionService>::create_session_with_machine_archived_resume_authority_under_runtime_turn_boundary(&self.inner, req, authorization).await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(
        &self,
        req: meerkat_core::service::CreateSessionRequest,
        authorization: meerkat_runtime::ArchivedSessionActorMaterializationAuthorization,
        resume_preparation: SessionResumePreparationReceipt,
        actor_witness_slot: &meerkat_session::LiveSessionActorWitnessSlot,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Service as MobSessionService>::create_session_with_machine_archived_resume_authority_and_actor_witness_under_runtime_turn_boundary(&self.inner, req, authorization, resume_preparation, actor_witness_slot).await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn authorize_revivable_retired_session(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::PreparedArchivedResumeCommitLease,
    ) -> Result<meerkat_runtime::AuthorizedArchivedResumeCommitLease, SessionError> {
        <Service as MobSessionService>::authorize_revivable_retired_session(
            &self.inner,
            session_id,
            authority,
        )
        .await
    }
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<EventStream, StreamError> {
        <Service as MobSessionService>::subscribe_session_events(&self.inner, session_id).await
    }
    async fn subscribe_agent_session_events_from(
        &self,
        session_id: &SessionId,
        cursor: meerkat_core::comms::SessionEventCursor,
    ) -> Result<AgentEventSubscription, StreamError> {
        <Service as MobSessionService>::subscribe_agent_session_events_from(
            &self.inner,
            session_id,
            cursor,
        )
        .await
    }
    fn supports_persistent_sessions(&self) -> bool {
        <Service as MobSessionService>::supports_persistent_sessions(&self.inner)
    }
    fn persisted_session_authority_read_cost(&self) -> PersistedSessionAuthorityReadCost {
        <Service as MobSessionService>::persisted_session_authority_read_cost(&self.inner)
    }
    async fn observe_persisted_session_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<crate::identity::IdentitySessionStoreAuthority>, SessionError> {
        <Service as MobSessionService>::observe_persisted_session_authority(&self.inner, session_id)
            .await
    }
    async fn subscribe_session_activity(
        &self,
        session_id: &SessionId,
    ) -> Result<MemberSessionActivity, SessionError> {
        <Service as MobSessionService>::subscribe_session_activity(&self.inner, session_id).await
    }
    async fn live_session_actor_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::live_session_actor_registered(&self.inner, session_id).await
    }
    async fn start_turn_with_admission_notification(
        &self,
        session_id: &SessionId,
        req: meerkat_core::service::StartTurnRequest,
        admitted: tokio::sync::oneshot::Sender<()>,
    ) -> Result<meerkat_core::RunResult, SessionError> {
        <Service as MobSessionService>::start_turn_with_admission_notification(
            &self.inner,
            session_id,
            req,
            admitted,
        )
        .await
    }
    #[cfg(feature = "runtime-adapter")]
    fn acquire_runtime_adapter(
        &self,
        explicit: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    ) -> Result<Option<Arc<meerkat_runtime::MeerkatMachine>>, meerkat_runtime::RuntimeDriverError>
    {
        <Service as MobSessionService>::acquire_runtime_adapter(&self.inner, explicit)
    }
    #[cfg(feature = "runtime-adapter")]
    fn supports_runtime_turn_apply(&self) -> bool {
        <Service as MobSessionService>::supports_runtime_turn_apply(&self.inner)
    }
    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::interrupt_with_machine_authority(
            &self.inner,
            session_id,
            authority,
        )
        .await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn interrupt_run_with_machine_authority(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::interrupt_run_with_machine_authority(
            &self.inner,
            session_id,
            expected_run_id,
            authority,
        )
        .await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn cancel_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::cancel_after_boundary_with_machine_authority(
            &self.inner,
            session_id,
            expected_run_id,
            authority,
        )
        .await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn cancel_current_after_boundary_with_machine_authority(
        &self,
        session_id: &SessionId,
        authority: meerkat_runtime::MachineSessionControlAuthority,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::cancel_current_after_boundary_with_machine_authority(
            &self.inner,
            session_id,
            authority,
        )
        .await
    }
    async fn execution_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentExecutionSnapshot>, SessionError> {
        <Service as MobSessionService>::execution_snapshot(&self.inner, session_id).await
    }
    async fn observe_member_status_view(
        &self,
        session_id: &SessionId,
    ) -> Result<MemberStatusSessionView, SessionError> {
        <Service as MobSessionService>::observe_member_status_view(&self.inner, session_id).await
    }
    async fn observe_live_durable_source(
        &self,
        session_id: &SessionId,
    ) -> Result<LiveDurableSourceObservation, SessionError> {
        <Service as MobSessionService>::observe_live_durable_source(&self.inner, session_id).await
    }
    async fn tool_scope_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ToolScopeSnapshot>, SessionError> {
        <Service as MobSessionService>::tool_scope_snapshot(&self.inner, session_id).await
    }
    async fn external_tool_surface_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<ExternalToolSurfaceSnapshot>, SessionError> {
        <Service as MobSessionService>::external_tool_surface_snapshot(&self.inner, session_id)
            .await
    }
    async fn peer_ingress_runtime_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<PeerIngressRuntimeSnapshot>, SessionError> {
        <Service as MobSessionService>::peer_ingress_runtime_snapshot(&self.inner, session_id).await
    }
    async fn session_known_to_archive_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::session_known_to_archive_authority(&self.inner, session_id)
            .await
    }
    async fn session_belongs_to_mob(
        &self,
        session_id: &SessionId,
        mob_id: &crate::ids::MobId,
    ) -> bool {
        <Service as MobSessionService>::session_belongs_to_mob(&self.inner, session_id, mob_id)
            .await
    }
    async fn load_persisted_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        <Service as MobSessionService>::load_persisted_session(&self.inner, session_id).await
    }
    async fn fork_persisted_session(
        &self,
        source_session_id: &SessionId,
        message_count: Option<usize>,
        tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        target: meerkat_core::DurableSessionForkTarget,
    ) -> Result<meerkat_core::SessionForkResult, SessionError> {
        <Service as MobSessionService>::fork_persisted_session(
            &self.inner,
            source_session_id,
            message_count,
            tool_access_policy,
            target,
        )
        .await
    }
    async fn fork_persisted_session_at_turn_boundary(
        &self,
        source_session_id: &SessionId,
        message_count: Option<usize>,
        tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
        target: meerkat_core::DurableSessionForkTarget,
        bound: std::time::Duration,
    ) -> Result<meerkat_core::DurableForkAtTurnBoundary, SessionError> {
        <Service as MobSessionService>::fork_persisted_session_at_turn_boundary(
            &self.inner,
            source_session_id,
            message_count,
            tool_access_policy,
            target,
            bound,
        )
        .await
    }
    async fn load_revivable_retired_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<Session>, SessionError> {
        <Service as MobSessionService>::load_revivable_retired_session(&self.inner, session_id)
            .await
    }
    async fn load_session_for_resume(
        &self,
        session_id: &SessionId,
    ) -> Result<ResumeSessionLoad, SessionError> {
        <Service as MobSessionService>::load_session_for_resume(&self.inner, session_id).await
    }
    async fn observe_session_resume_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionResumeAuthority, SessionError> {
        <Service as MobSessionService>::observe_session_resume_authority(&self.inner, session_id)
            .await
    }
    async fn revalidate_session_resume_authority(
        &self,
        session_id: &SessionId,
        expected: &SessionResumeAuthority,
    ) -> Result<Result<(), SessionResumeRejection>, SessionError> {
        <Service as MobSessionService>::revalidate_session_resume_authority(
            &self.inner,
            session_id,
            expected,
        )
        .await
    }
    async fn materialize_session_resume_verdict(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionResumeVerdict, SessionError> {
        <Service as MobSessionService>::materialize_session_resume_verdict(&self.inner, session_id)
            .await
    }
    async fn settle_durable_tail_before_cold_start(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::settle_durable_tail_before_cold_start(
            &self.inner,
            session_id,
        )
        .await
    }
    async fn settle_durable_tail_before_cold_start_reporting(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::ColdStartTailSettlement, SessionError> {
        <Service as MobSessionService>::settle_durable_tail_before_cold_start_reporting(
            &self.inner,
            session_id,
        )
        .await
    }
    async fn load_persisted_session_metadata(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::PersistedSessionMetadataView>, SessionError> {
        <Service as MobSessionService>::load_persisted_session_metadata(&self.inner, session_id)
            .await
    }
    async fn session_projection_visible(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::session_projection_visible(&self.inner, session_id).await
    }
    async fn archive_with_mob_lifecycle_authority(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::archive_with_mob_lifecycle_authority(
            &self.inner,
            session_id,
        )
        .await
    }
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(&self.inner, session_id).await
    }
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(
        &self,
        session_id: &SessionId,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_before(&self.inner, session_id, deadline).await
    }
    #[cfg(feature = "runtime-adapter")]
    async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(
        &self,
        session_id: &SessionId,
        deadline: meerkat_core::time_compat::Instant,
        post_commit_hook: Option<Arc<dyn meerkat_runtime::MachineSessionArchivePostCommitHook>>,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::archive_with_mob_lifecycle_authority_under_runtime_turn_boundary_and_hook_before(&self.inner, session_id, deadline, post_commit_hook).await
    }
    async fn apply_runtime_turn(
        &self,
        session_id: &SessionId,
        run_id: meerkat_core::RunId,
        req: StartTurnRequest,
        boundary: RunApplyBoundary,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<CoreApplyOutput, SessionError> {
        <Service as MobSessionService>::apply_runtime_turn(
            &self.inner,
            session_id,
            run_id,
            req,
            boundary,
            contributing_input_ids,
        )
        .await
    }
    async fn prepare_turn_boundary_delivery_for_active_turn(
        &self,
        session_id: &SessionId,
        expected_run_id: &meerkat_core::RunId,
        delivery: meerkat_core::TurnBoundaryDelivery,
    ) -> Result<meerkat_core::CoreBoundaryStageOutput, meerkat_core::CoreBoundaryStageError> {
        <Service as MobSessionService>::prepare_turn_boundary_delivery_for_active_turn(
            &self.inner,
            session_id,
            expected_run_id,
            delivery,
        )
        .await
    }
    async fn checkpoint_committed_runtime_session_snapshot(
        &self,
        session_id: &SessionId,
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::checkpoint_committed_runtime_session_snapshot(
            &self.inner,
            session_id,
            session_snapshot,
        )
        .await
    }
    async fn acquire_runtime_turn_finalization_guard(
        &self,
        session_id: &SessionId,
    ) -> Result<Box<dyn meerkat_core::lifecycle::CoreExecutorTurnFinalizationGuard>, SessionError>
    {
        <Service as MobSessionService>::acquire_runtime_turn_finalization_guard(
            &self.inner,
            session_id,
        )
        .await
    }
    async fn checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::checkpoint_committed_runtime_session_snapshot_under_turn_finalization_boundary(&self.inner, session_id, session_snapshot).await
    }
    async fn acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
        authority: &meerkat_core::CommittedSessionBoundaryAuthority,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary(&self.inner, session_id, authority).await
    }
    async fn enqueue_committed_parent_session_boundary_after_runtime_turn(
        &self,
        session_id: &SessionId,
        runtime_adapter: &meerkat_runtime::MeerkatMachine,
    ) -> Result<usize, SessionError> {
        <Service as MobSessionService>::enqueue_committed_parent_session_boundary_after_runtime_turn(&self.inner, session_id, runtime_adapter).await
    }
    async fn discard_live_session_after_runtime_stop_terminalized(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::discard_live_session_after_runtime_stop_terminalized(
            &self.inner,
            session_id,
        )
        .await
    }
    async fn discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::discard_live_session_after_runtime_stop_terminalized_under_turn_finalization_boundary(&self.inner, session_id).await
    }
    async fn publish_boundary_appends_discarded_for_actor(
        &self,
        actor_witness: &meerkat_session::LiveSessionActorWitness,
        discarded: &meerkat_core::event::BoundaryAppendsDiscarded,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::publish_boundary_appends_discarded_for_actor(
            &self.inner,
            actor_witness,
            discarded,
        )
        .await
    }
    async fn publish_interaction_terminals_for_actor(
        &self,
        actor_witness: &meerkat_session::LiveSessionActorWitness,
        events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        SessionError,
    > {
        <Service as MobSessionService>::publish_interaction_terminals_for_actor(
            &self.inner,
            actor_witness,
            events,
        )
        .await
    }
    async fn discard_live_session(&self, session_id: &SessionId) -> Result<(), SessionError> {
        <Service as MobSessionService>::discard_live_session(&self.inner, session_id).await
    }
    async fn discard_live_session_under_runtime_turn_boundary(
        &self,
        session_id: &SessionId,
    ) -> Result<(), SessionError> {
        <Service as MobSessionService>::discard_live_session_under_runtime_turn_boundary(
            &self.inner,
            session_id,
        )
        .await
    }
    async fn discard_live_session_actor_under_runtime_turn_boundary(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::discard_live_session_actor_under_runtime_turn_boundary(
            &self.inner,
            witness,
        )
        .await
    }
    async fn discard_live_session_actor_after_durability_reload_required(
        &self,
        witness: &meerkat_session::LiveSessionActorWitness,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::discard_live_session_actor_after_durability_reload_required(
            &self.inner,
            witness,
        )
        .await
    }
    async fn await_event_projection_drain(
        &self,
        session_id: &SessionId,
    ) -> Result<bool, SessionError> {
        <Service as MobSessionService>::await_event_projection_drain(&self.inner, session_id).await
    }
    async fn cancel_all_checkpointers(&self) {
        <Service as MobSessionService>::cancel_all_checkpointers(&self.inner).await;
    }
    async fn rearm_all_checkpointers(&self) {
        <Service as MobSessionService>::rearm_all_checkpointers(&self.inner).await;
    }
}
