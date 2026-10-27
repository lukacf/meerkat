//! Preparation crosses the actual identity runtime and mob session bridge.
//! The machine's input ledger and the physical provider entry are the oracles;
//! no fake binder, admission counter or reconstructed controller supplies them.

use super::*;
use crate::original_ingress::tests::{initialized, machine, principal, private_event};
use futures::FutureExt;
use meerkat::{AgentFactory, Config, EphemeralSessionService, FactoryAgentBuilder};
use meerkat_authorization_contracts::constraints::AudienceRef;
use meerkat_core::{
    AgentError, AgentSessionStore, ControllerModelSelection, Message, Provider, Session,
    SessionLlmIdentity,
};
use meerkat_llm_core::{
    LlmClient, LlmError, LlmRequest, LlmStream, PlainModelRoute, PreparedLlmRequest,
};
use meerkat_mob::{
    AgentIdentity as MemberIdentity, MobDefinition, MobId, MobRuntimeMode,
    MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_mobkit::identity_first::{
    AgentAddressability, AgentIdentity, AgentRuntimeId, CheckpointVersion, ContinuityGeneration,
    ContinuityRecord, DurabilityPolicy, DurableAgentSpec, IdentityLifecycleState,
    IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig, LeaseAcquireResult,
    LeaseProvider, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge, MutableRosterProvider,
    PublicIdentityRuntime,
};
use meerkat_mobkit::{
    DiscoverySpec, MetaConfig, MobBootstrapSpec, MobTerminalShutdownOutcome, UnifiedRuntime,
};
use meerkat_runtime::service_ext::SessionServiceRuntimeExt;
use std::{
    collections::{BTreeMap, HashMap},
    os::unix::fs::PermissionsExt,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

const WAIT: Duration = Duration::from_secs(10);
const MODEL: &str = "claude-sonnet-4-5";
const MEMBER: &str = "agent";
const PROMPT: &str = "Read my latest messages";

fn selection() -> ControllerModelSelection {
    ControllerModelSelection::new(
        SessionLlmIdentity {
            model: MODEL.into(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(serde_json::json!({
            "realm": "test-realm", "account": "controller-preparation-fixture"
        }))
        .unwrap(),
        "preparation-profile".into(),
        "preparation-transport".into(),
    )
}

// Observe the actual final factory client without replacing or retaining it.
// A failure here separates adapter selection/pin loss from the later actor
// witness and persisted-session identity checks. All diagnostics are labels.
fn fixture_controller_decorator() -> meerkat_core::AgentLlmClientDecorator {
    Arc::new(|client| {
        let expected = selection();
        assert!(client.controller_model_selection().as_ref() == Some(&expected),
            "factory client must preserve the fixture controller selection");
        let pin = client.clone().pin_controller()
            .expect("actual factory client must supply its native controller pin");
        assert!(pin.selection() == &expected,
            "factory pin must preserve the selected fixture controller");
        assert!(pin.client().controller_model_selection().as_ref() == Some(&expected),
            "pinned factory client must preserve the selected fixture controller");
        client
    })
}

#[derive(Default)]
struct NoRequestProvider(AtomicUsize);

#[async_trait]
impl LlmClient for NoRequestProvider {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        // This fixture owns a fixed synthetic credential. Maintenance performs
        // no acquisition, provider request or lease publication; native admission
        // still checks the machine's actual credential lifecycle separately.
        Ok(())
    }

    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if logical_model != MODEL {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        PlainModelRoute::new("https://controller-preparation.invalid/messages", MODEL)
    }

    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(selection())
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("controller preparation must not enter a model request")
    }

    fn stream_prepared<'a>(&'a self, _: &'a PreparedLlmRequest) -> LlmStream<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("controller preparation must not enter a prepared model request")
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

#[derive(Default)]
struct SessionStore(Mutex<HashMap<SessionId, Session>>);

#[async_trait]
impl AgentSessionStore for SessionStore {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.0
            .lock()
            .unwrap()
            .insert(session.id().clone(), session.clone());
        Ok(())
    }

    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        Ok(SessionId::parse(id)
            .ok()
            .and_then(|id| self.0.lock().unwrap().get(&id).cloned()))
    }
}

fn definition() -> MobDefinition {
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "toolkit-preparation-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(Profile {
            model_fallback: None,
            model: MODEL.into(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: Vec::new(),
            tools: ToolConfig {
                comms: true,
                ..Default::default()
            },
            peer_description: "controller preparation fixture".into(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
        })),
    );
    definition
}

struct Fixture {
    // Keep the private durable owners and their directory alive until native
    // shutdown has joined. No credential or network connection is installed.
    _directory: Arc<tempfile::TempDir>,
    source: Arc<PrivateOriginals>,
    machine: Arc<MeerkatMachine>,
    native: UnifiedRuntime,
    _identity_runtime: PublicIdentityRuntime,
    session: SessionId,
    pin: ControllerModelClient,
    provider: Arc<NoRequestProvider>,
    submission: PrivateOriginalSubmit,
}

impl Fixture {
    async fn new(with_controller_source: bool) -> Self {
        let directory = Arc::new(tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap());
        let mut opened = None;
        let built = std::panic::AssertUnwindSafe(tokio::time::timeout(
            WAIT, Self::build(directory.clone(), with_controller_source, &mut opened),
        )).catch_unwind().await;
        match built {
            Ok(Ok(fixture)) => fixture,
            failed => {
                let cleanup = match opened {
                    Some(native) => Self::shutdown_native(&native).await,
                    None => Ok(()),
                };
                // The directory remains alive through native teardown.
                assert!(cleanup.is_ok(), "controller fixture cleanup failed: {cleanup:?}");
                match failed {
                    Err(payload) => std::panic::resume_unwind(payload),
                    _ => panic!("controller fixture construction timed out"),
                }
            }
        }
    }

    async fn build(
        directory: Arc<tempfile::TempDir>,
        with_controller_source: bool,
        opened: &mut Option<UnifiedRuntime>,
    ) -> Self {
        let owner = initialized(&directory.path().join("grants"));
        let machine = Arc::new(machine(&owner));
        let provider = Arc::new(NoRequestProvider::default());
        let mut builder =
            FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
        builder.default_llm_client = Some(provider.clone());
        *builder.default_agent_llm_client_decorator.write().unwrap() =
            Some(fixture_controller_decorator());
        builder.default_session_store = Some(Arc::new(SessionStore::default()));
        let service = Arc::new(EphemeralSessionService::new(builder, 4));
        let definition = definition();
        let namespace = definition.id.to_string();
        let spec = MobBootstrapSpec::new(definition, MobStorage::in_memory(), service.clone())
            .with_session_runtime_adapter(machine.clone()).unwrap()
            .with_declared_ephemeral_mob_storage();
        *opened = Some(UnifiedRuntime::bootstrap(
            spec,
            MetaConfig {
                modules: Vec::new(),
                discovery: DiscoverySpec { namespace, modules: Vec::new() },
                pre_spawn: Vec::new(),
            },
            WAIT,
        ).await.unwrap());
        let native = opened.as_mut().unwrap();
        let handle = native.mob_handle();
        let member = MemberIdentity::from(MEMBER);
        handle
            .spawn_spec(SpawnMemberSpec::new("worker", member.clone()))
            .await
            .unwrap();
        let session = handle
            .get_member(&member)
            .await
            .unwrap()
            .unwrap()
            .bridge_session_id()
            .cloned()
            .unwrap();
        let actor = service.live_session_actor_witness(&session).await.unwrap();
        let pin = service
            .pin_controller_client_for_actor(&actor)
            .await
            .unwrap_or_else(|error| {
                if let meerkat_core::service::SessionError::Agent(
                    AgentError::OperationRefused { refusal }
                ) = &error {
                    panic!("actual actor controller pin refused: {:?}", refusal.kind());
                }
                panic!("actual actor controller pin failed: {error:?}");
            });

        let mut bridge = MobSessionBridge::new(handle.observer());
        if with_controller_source {
            bridge = bridge.with_controller_pin_source(service);
        }
        let lease_provider = Arc::new(LocalLeaseProvider::new());
        let runtime_instance = "toolkit-controller-preparation";
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory().unwrap()),
            lease_provider: lease_provider.clone(),
            runtime_instance_id: runtime_instance.into(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(bridge)),
            default_timeout: None,
        }).with_runtime_services(
            meerkat_mobkit::identity_first::AgentRuntimeServices::new(handle.clone()),
        ));
        let identity = AgentIdentity::parse(MEMBER).unwrap();
        let member_spec = DurableAgentSpec {
            identity: identity.clone(),
            profile: ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: Some(MobRuntimeMode::TurnDriven),
            backend: None,
            binding: None,
            placement: None,
        };
        // Core already owns this spawned member and session; registration adds its roster spec.
        runtime.register(member_spec.clone()).await;
        let status = runtime.status(&identity).await.unwrap();
        assert_eq!(status.state, IdentityLifecycleState::Active);
        assert_eq!(status.session_id, Some(session.clone()));
        assert!(status.agent_runtime_id.is_some(), "spawned Core member has a runtime identity");
        assert_eq!(runtime.core_session_owner(&session).await.unwrap(), Some(identity.clone()));
        native.attach_identity_first_context(Arc::new(IdentityFirstRuntimeContext::new(
            runtime,
            Arc::new(MutableRosterProvider::new(vec![member_spec])),
            None, None, Some(handle.definition().clone()),
        )));
        let runtime = native.identity_runtime().unwrap();
        let domain = owner.ingress_domain().clone();
        let source = Arc::new(
            PrivateOriginals::new(
                StoreScope {
                    realm: "test-realm".into(),
                    owner: "originals".into(),
                },
                Arc::new(
                    toolkit_store::SqliteStore::open(directory.path().join("originals.sqlite3"))
                        .unwrap(),
                ),
                owner,
                vec![PrivateOriginalRoute {
                    id: Id::new("route").unwrap(),
                    grant_caller: principal("admin"),
                    application_parent: None,
                    controller_parent: None,
                    grant_target: Id::new(MEMBER).unwrap(),
                    grant_audience: AudienceRef::Principal {
                        principal: principal("user"),
                    },
                    realm: RealmId::parse("test-realm").unwrap(),
                    requester: principal("user"),
                    ingress_actor: principal("frontend"),
                    logical_executor: principal("agent"),
                    represented_subject: None,
                    source_authority: principal("source"),
                    authentication_domain: domain.clone(),
                    event_domain: domain,
                }],
            )
            .await
            .unwrap(),
        );
        source.bind_machine(machine.clone()).unwrap();
        source.bind_identity_runtime(&runtime).unwrap();
        let mut event = private_event("prepared-original");
        event.content_sha256 = digest(PROMPT.as_bytes());
        let authentication = event.original_authentication.clone();
        let original = source.retain_event(event).await.unwrap();
        Self {
            _directory: directory,
            source,
            machine,
            native: opened.take().unwrap(),
            _identity_runtime: runtime,
            session,
            pin,
            provider,
            submission: PrivateOriginalSubmit {
                original,
                content: PROMPT.into(),
                transport_authentication: authentication,
            },
        }
    }

    async fn shutdown_native(native: &UnifiedRuntime) -> Result<(), String> {
        // Attaching the identity context starts native supervisors. Await
        // their real owner, including lease release and terminal actor cleanup.
        let report = native.shutdown().await;
        let terminal_clean = match &report.mob_terminal_shutdown {
            MobTerminalShutdownOutcome::Completed(report) => report.is_clean(),
            MobTerminalShutdownOutcome::AlreadyShutDown => true,
            _ => false,
        };
        if report.cleanup_completed() && terminal_clean {
            Ok(())
        } else {
            Err(format!("native controller fixture cleanup incomplete: {report:?}"))
        }
    }

    async fn shutdown(&self) -> Result<(), String> {
        Self::shutdown_native(&self.native).await
    }

    async fn assert_no_admission_or_request(&self) {
        let snapshot = self
            .machine
            .meerkat_machine_spine_snapshot(&self.session)
            .await
            .unwrap();
        assert!(snapshot.inputs.admission_order.is_empty());
        assert!(snapshot.inputs.queue.is_empty());
        assert!(snapshot.inputs.current_run_id.is_none());
        assert_eq!(snapshot.ledger.input_count, 0);
        assert_eq!(snapshot.ledger.accepted_count, 0);
        assert_eq!(self.provider.0.load(Ordering::SeqCst), 0);
        assert!(
            self.source
                .binding_evidence(&self.submission.original)
                .await
                .unwrap()
                .is_none()
        );
        assert!(self.source.retained.lock().unwrap().is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_controller_preparation_releases_native_lease_without_admitting_input_or_request() {
    let fixture = Fixture::new(true).await;
    fixture.assert_no_admission_or_request().await;
    // A second preparation is deliberate: the first tracked call must release
    // native custody before the human review can begin or another call can run.
    for _ in 0..2 {
        let prepared = tokio::time::timeout(
            Duration::from_secs(10),
            fixture
                .source
                .prepare_local_controller_setup(fixture.submission.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(prepared.session_id(), &fixture.session);
        // Native pinning may wrap the same selected client in a fresh taint
        // observer. Compare its actual selection and complete lowered route,
        // not the allocation identity of two independent native pin calls.
        assert!(prepared.controller().selection() == fixture.pin.selection());
        let prepared_facts = prepared.controller().plain_facts()
            .expect("prepared native pin must retain its selected client and route facts");
        let actor_facts = fixture.pin.plain_facts()
            .expect("fixture actor pin must retain its selected client and route facts");
        assert!(prepared_facts.route_key() == actor_facts.route_key(),
            "prepared controller must preserve the actor pin's complete native route");
        assert!(Arc::ptr_eq(
            &prepared.source_owner().unwrap(),
            &fixture.source
        ));
        assert!(prepared.original() == &fixture.submission.original);
        fixture.assert_no_admission_or_request().await;
    }
    fixture.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_native_pre_bind_failure_returns_no_usable_controller_setup() {
    // The actual member, runtime and client exist; only the native bridge's
    // required ControllerPinSource is absent. Its refusal precedes our binder.
    let fixture = Fixture::new(false).await;
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        fixture
            .source
            .prepare_local_controller_setup(fixture.submission.clone()),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(PrivateOriginalError::Unavailable)));
    fixture.assert_no_admission_or_request().await;
    fixture.shutdown().await.unwrap();
}
