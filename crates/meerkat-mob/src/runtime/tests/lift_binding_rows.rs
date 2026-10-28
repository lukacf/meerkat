#![cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
//! The auth binding a member's live actor runs across the lift of a mob
//! stopped in this process. Stop keeps each member's live actor, so the
//! explicit resume keeps it only when it runs the binding the member's
//! resume request names; otherwise the member is rebuilt on that binding.
//!
//! Real turn-driven members over a real factory, a real SQLite session store
//! and a real head-canonical SQLite runtime store, so the kept actor, its
//! exact runtime attachment and the binding its session recorded are the
//! production ones.

use super::*;
use crate::machines::mob_machine as mob_dsl;

const WAIT: Duration = Duration::from_secs(30);

type Service = meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>;

/// Answers every turn with one fixed text block, and records for each model
/// request whether the probed member's live actor then ran the probed
/// binding (by the one check every live-actor reuse runs).
#[derive(Clone, Default)]
struct BindingProbeClient {
    probe: Arc<Mutex<Option<BindingProbe>>>,
    requests: Arc<Mutex<Vec<bool>>>,
}

#[derive(Clone)]
struct BindingProbe {
    service: Arc<Service>,
    member: AgentIdentity,
    session: SessionId,
    binding: meerkat_core::AuthBindingRef,
}

impl BindingProbeClient {
    fn probe(&self, probe: BindingProbe) {
        *self.probe.lock().expect("probe") = Some(probe);
    }

    /// For each model request so far, in order, whether it ran on the probed
    /// binding.
    fn requests(&self) -> Vec<bool> {
        self.requests.lock().expect("requests").clone()
    }
}

#[async_trait]
impl meerkat_client::LlmClient for BindingProbeClient {
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
        let probe = self.probe.lock().expect("probe").clone();
        let requests = Arc::clone(&self.requests);
        Box::pin(async_stream::try_stream! {
            let on_binding = match probe.as_ref() {
                Some(probe) => crate::runtime::live_actor_binding::verify_live_actor_auth_binding(
                    probe.service.as_ref(),
                    &probe.member,
                    &probe.session,
                    &probe.binding,
                )
                .await
                .is_ok(),
                None => false,
            };
            requests.lock().expect("requests").push(on_binding);
            yield meerkat_client::LlmEvent::TextDelta {
                delta: "lift-answer".to_string(),
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

/// A host customizer whose answer names the auth binding it holds now, so a
/// row can change it while the mob is stopped. Its resume answer is its
/// spawn answer (the trait's default `customize_resume`).
#[derive(Default)]
struct BindingCustomizer {
    auth_binding: Mutex<Option<meerkat_core::AuthBindingRef>>,
}

impl BindingCustomizer {
    fn name_binding(&self, binding: meerkat_core::AuthBindingRef) {
        *self.auth_binding.lock().expect("auth binding") = Some(binding);
    }
}

impl SpawnMemberCustomizer for BindingCustomizer {
    fn customize_spawn(
        &self,
        _ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        if let Some(binding) = self.auth_binding.lock().expect("auth binding").clone() {
            spec.auth_binding = Some(binding);
        }
        Ok(())
    }
}

/// The binding the rows move a member to: the realm's configured OpenAI key.
fn configured_openai_binding() -> meerkat_core::AuthBindingRef {
    meerkat_core::AuthBindingRef {
        realm: meerkat_core::RealmId::global(),
        binding: meerkat_core::BindingId::parse("default_openai").expect("binding id"),
        profile: None,
        origin: meerkat_core::BindingOrigin::Configured,
    }
}

fn lift_definition(label: &str) -> MobDefinition {
    let mut definition = with_unique_mob_id(sample_definition(), label);
    let worker = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .and_then(ProfileBinding::as_inline_mut)
        .expect("inline worker");
    worker.model = "gpt-5.5".to_string();
    worker.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    definition
}

struct LiftFixture {
    _root: tempfile::TempDir,
    handle: MobHandle,
    service: Arc<Service>,
    customizer: Arc<BindingCustomizer>,
    client: BindingProbeClient,
}

impl LiftFixture {
    async fn new(label: &str) -> Self {
        let root = tempfile::Builder::new()
            .prefix("meerkat-lift-binding-")
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
        config.realm.insert(
            "global".to_string(),
            meerkat_core::RealmConfigSection::from_inline_api_keys(&[("openai", "test-key")]),
        );
        let client = BindingProbeClient::default();
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
        let customizer = Arc::new(BindingCustomizer::default());
        let handle = MobBuilder::new(lift_definition(label), MobStorage::in_memory())
            .with_session_service(service.clone())
            .with_default_llm_client(Arc::new(client.clone()))
            .with_spawn_member_customizer(customizer.clone())
            .create()
            .await
            .expect("create the mob");
        Self {
            _root: root,
            handle,
            service,
            customizer,
            client,
        }
    }

    /// Spawn one durable turn-driven worker and wait until it is Live.
    /// Identities are unique per test: comms routes are process-wide.
    async fn spawn(&self, label: &str) -> AgentIdentity {
        let identity = AgentIdentity::from(format!("{label}-{}", Uuid::new_v4().simple()));
        let mut spec = SpawnMemberSpec::new("worker", identity.clone());
        spec.continuity_intent = SpawnContinuityIntent::DurableIdentity {
            continuity_key: identity.to_string(),
        };
        tokio::time::timeout(WAIT, self.handle.spawn_spec(spec))
            .await
            .expect("the spawn answers")
            .expect("spawn the worker");
        self.wait_live(&identity).await;
        identity
    }

    async fn wait_live(&self, identity: &AgentIdentity) {
        let key = mob_dsl::AgentIdentity::from_domain(identity);
        let mut state = self.handle.machine_state_watch_rx.clone();
        let reached = tokio::time::timeout(
            WAIT,
            state.wait_for(|state| {
                state.member_materialization.get(&key)
                    == Some(&mob_dsl::MemberMaterializationPhase::Live)
            }),
        )
        .await;
        assert!(
            matches!(reached, Ok(Ok(_))),
            "member {identity} never became Live"
        );
    }

    async fn session(&self, identity: &AgentIdentity) -> SessionId {
        self.handle
            .resolve_bridge_session_id(identity)
            .await
            .expect("a session-bound member")
    }

    /// Whether `session`'s live actor runs `binding`, by the one check every
    /// live-actor reuse runs.
    async fn live_actor_runs(
        &self,
        identity: &AgentIdentity,
        session: &SessionId,
        binding: &meerkat_core::AuthBindingRef,
    ) -> bool {
        crate::runtime::live_actor_binding::verify_live_actor_auth_binding(
            self.service.as_ref(),
            identity,
            session,
            binding,
        )
        .await
        .is_ok()
    }

    /// Wait until the model has been asked at least `count` times.
    async fn wait_requests(&self, count: usize) -> Vec<bool> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let requests = self.client.requests();
            if requests.len() >= count {
                return requests;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the model was asked {} times, not {count}",
                requests.len()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn finish(self) {
        let _ = tokio::time::timeout(WAIT, self.handle.shutdown()).await;
    }
}

/// A host changes a member's auth binding while its mob is stopped in this
/// process. Stop kept the member's live actor on the old binding; the lift
/// does not keep it: the member is rebuilt on the binding its resume request
/// names, serves Live, and its session records that binding.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lift_after_the_binding_changed_rebuilds_the_member_on_the_new_binding() {
    let fixture = LiftFixture::new("lift-binding").await;
    let member = fixture.spawn("lift-binding-member").await;
    let session = fixture.session(&member).await;
    let binding = configured_openai_binding();
    assert!(
        !fixture.live_actor_runs(&member, &session, &binding).await,
        "precondition: the member was not built on the binding the lift names"
    );

    tokio::time::timeout(WAIT, fixture.handle.stop())
        .await
        .expect("the stop answers")
        .expect("stop the mob");
    assert!(
        MobSessionService::live_session_actor_registered(&*fixture.service, &session)
            .await
            .unwrap_or(false),
        "precondition: the stop kept the member's live actor"
    );
    fixture.customizer.name_binding(binding.clone());
    tokio::time::timeout(WAIT, fixture.handle.resume())
        .await
        .expect("the lift answers")
        .expect("lift the mob");

    fixture.wait_live(&member).await;
    assert_eq!(
        fixture.session(&member).await,
        session,
        "the member is rebuilt on its own session"
    );
    assert!(
        MobSessionService::live_session_actor_registered(&*fixture.service, &session)
            .await
            .unwrap_or(false),
        "the rebuilt member serves from a live actor"
    );
    assert!(
        fixture.live_actor_runs(&member, &session, &binding).await,
        "the member's live actor runs the binding its resume request names"
    );
    fixture.finish().await;
}

/// A run queued while the mob is stopped (here an input admitted straight to
/// the member's runtime, as a peer message or an external event is) starts
/// only on the binding the lift's request names: the run starts the stop held
/// stay held until the lift's binding check retired the actor on the old
/// binding, so no model request is ever made on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_queued_while_stopped_starts_only_on_the_new_binding() {
    let fixture = LiftFixture::new("lift-queued-run").await;
    let member = fixture.spawn("lift-queued-member").await;
    let session = fixture.session(&member).await;
    let binding = configured_openai_binding();
    assert!(
        !fixture.live_actor_runs(&member, &session, &binding).await,
        "precondition: the member was not built on the binding the lift names"
    );
    fixture.client.probe(BindingProbe {
        service: fixture.service.clone(),
        member: member.clone(),
        session: session.clone(),
        binding: binding.clone(),
    });

    tokio::time::timeout(WAIT, fixture.handle.stop())
        .await
        .expect("the stop answers")
        .expect("stop the mob");
    let runtime = MobSessionService::acquire_runtime_adapter(&*fixture.service, None)
        .expect("the service's runtime")
        .expect("a runtime-backed service");
    let (admitted, _completion) = runtime
        .accept_input_with_completion(
            &session,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                "queued while the mob is stopped",
                None,
            )),
        )
        .await
        .expect("the member's runtime admits the input while the mob is stopped");
    assert!(
        matches!(admitted, meerkat_runtime::AcceptOutcome::Accepted { .. }),
        "the input is queued: {admitted:?}"
    );
    assert!(
        fixture.client.requests().is_empty(),
        "no run starts while the mob is stopped"
    );
    fixture.customizer.name_binding(binding.clone());
    tokio::time::timeout(WAIT, fixture.handle.resume())
        .await
        .expect("the lift answers")
        .expect("lift the mob");

    let requests = fixture.wait_requests(1).await;
    assert!(
        requests.iter().all(|on_binding| *on_binding),
        "every model request of the queued run ran on the binding the lift names: {requests:?}"
    );
    fixture.finish().await;
}
