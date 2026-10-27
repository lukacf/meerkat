//! The production private caller runs through real native admission and source
//! custody. Faults wrap the real bridge before lookup or after fresh acceptance.

use meerkat_authorization_contracts::constraints::DomainRestriction;
use super::*;

// Test-only result context. Production admission types do not
// implement Debug; retain their existing serialized variant in failed checks.
fn submission_context<E: std::fmt::Debug>(result: &Result<PrivateOriginalSubmission, E>) -> String {
    match result {
        Ok(outcome) => format!("Ok({:?})", serde_json::to_value(outcome)),
        Err(error) => format!("Err({error:?})"),
    }
}

use meerkat_core::lifecycle::InputId;
use crate::original_ingress::tests::{
    claims, initialized, principal, private_event, source as source_custody,
};
use futures::FutureExt;
use meerkat::{AgentFactory, Config, EphemeralSessionService, FactoryAgentBuilder};
use meerkat_authorization_contracts::constraints::AudienceRef;
use meerkat_core::{
    AgentError, AgentSessionStore, ControllerModelSelection, Message, Provider, Session,
    SessionLlmIdentity, StopReason,
};
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream, PlainModelRoute,
    PreparedLlmRequest,
};
use meerkat_mob::{
    AgentIdentity as MemberIdentity, MobDefinition, MobId, MobRuntimeMode,
    MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
use meerkat_mobkit::identity_first::{
    AgentAddressability, AgentIdentity, AgentRuntimeId, CheckpointVersion, ContinuityGeneration,
    ContinuityRecord, DurabilityPolicy, DurableAgentSpec, FencingToken, IdentityLifecycleState,
    IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig, LeaseAcquireResult,
    LeaseProvider, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge, MutableRosterProvider,
    PublicIdentityRuntime, TurnOutcome, TurnOutput, TurnTicket,
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

use meerkat_authorization::{
    grant_policy::{
        AdmittedWorkPolicyOwner, ControllerAdmissionAllowance, OperationPolicyOwner,
        WorkOwnerAllowance,
    },
    grants::LocalGrantAuthority,
    policy::{LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose},
};
use meerkat_authorization_contracts::{
    constraints::{ActionRef, ExactRestriction, ExecutionRestrictions, ProcessorRef},
    work_association::{GrantLineageRef, InputAuthorityAssociation, WorkAuthorityBasis},
};
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationFacts, ModelAuthorizationUse,
    OperationObservedOutcome, OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
};
use meerkat_runtime::{
    input_authority::{NativeAdmissionError, NativeIngressContext},
    meerkat_machine::{AuthenticatedOriginalSource, ScopedInvocation, ScopedMandateOwner},
};

#[path = "governed_submit_test_bridge.rs"]
mod observed;
#[path = "custody_recovery_tests.rs"]
mod custody_recovery_tests;
#[path = "custody_recovery_writer_tests.rs"]
mod custody_recovery_writer_tests;
#[path = "custody_recovery_budget_tests.rs"]
mod custody_recovery_budget_tests;
#[path = "custody_recovery_stopped_traversal_tests.rs"]
mod custody_recovery_stopped_traversal_tests;
#[path = "first_input_tests.rs"]
pub(in crate::private_originals) mod first_input_tests;

#[derive(Clone, Copy)]
pub(in crate::private_originals) enum DeliveryMode {
    Normal,
    HoldAccepted,
    LoseAcceptedAnswer,
    PoisonPublicationAfterAcceptance,
    PoisonPublicationAfterSecondAcceptance,
    LoseSecondAcceptedAnswer,
    RefuseSecondBeforeLookup,
    // These two controls target only the fixture's actual helper session.
    HoldHelperBeforeLookup,
    HoldHelperAcceptedReply,
    // These controls invalidate the actual controller-admission observation.
    HelperPolicyChangesOnce,
    HelperPolicyChangesAlways,
    HelperPolicyChangesThenUnresolved,
    HelperPolicyChangesThenRetirement,
    HelperPolicyChangesThenDeadline,
}

impl DeliveryMode {
    fn helper_reprepare(self) -> bool {
        matches!(self, Self::HelperPolicyChangesOnce | Self::HelperPolicyChangesAlways
            | Self::HelperPolicyChangesThenUnresolved | Self::HelperPolicyChangesThenRetirement
            | Self::HelperPolicyChangesThenDeadline)
    }
}

struct DeliveryFault {
    mode: DeliveryMode,
    accepted: AtomicUsize,
    refused: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Semaphore,
    helper_session: std::sync::OnceLock<SessionId>,
    helper_input: Mutex<Option<InputId>>,
    helper_returns: AtomicUsize,
    helper_inputs: Mutex<Vec<serde_json::Value>>,
    helper_policy_changed: AtomicUsize,
    helper_admission_change: Mutex<Option<InputAuthorityAssociation>>,
    publication_changes: AtomicUsize,
    publication_change_failures: AtomicUsize,
}
impl DeliveryFault {
    fn new(mode: DeliveryMode) -> Self {
        Self {
            mode,
            accepted: AtomicUsize::new(0),
            refused: AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Semaphore::new(0),
            helper_session: std::sync::OnceLock::new(),
            helper_input: Mutex::new(None),
            helper_returns: AtomicUsize::new(0),
            helper_inputs: Mutex::new(Vec::new()),
            helper_policy_changed: AtomicUsize::new(0),
            helper_admission_change: Mutex::new(None),
            publication_changes: AtomicUsize::new(0),
            publication_change_failures: AtomicUsize::new(0),
        }
    }
}
fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).unwrap()
}
fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}
fn model_facts() -> ModelAuthorizationFacts {
    let selected = selection();
    ModelAuthorizationFacts {
        identity: Arc::new(SessionLlmIdentity {
            model: MODEL.into(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }),
        wire_model: MODEL.into(),
        hosted_capabilities: Arc::from([]),
        backend_profile_id: Some("preparation-profile".into()),
        backend_kind: "preparation-transport".into(),
        endpoint: ENDPOINT.into(),
        credential: Some(selected.credential().clone()),
        usage: ModelAuthorizationUse::ControllerInference,
        live_channel: None,
    }
}

fn action(value: &str) -> ActionRef {
    ActionRef {
        feature: "governed-mob".into(),
        action: value.into(),
    }
}

fn domain() -> ResourceDomain {
    ResourceDomain {
        authority: principal("resource-owner"),
        namespace: "records".into(),
    }
}

fn ceiling(action_name: &str) -> ExecutionRestrictions {
    let mut bounds = ExecutionRestrictions::unrestricted();
    bounds.actions = ExactRestriction::exact([action(action_name)]);
    bounds.resource_domains = DomainRestriction::exact([domain()]);
    bounds.processors = ExactRestriction::exact([ProcessorRef::Principal {
        principal: principal("agent"),
    }]);
    bounds.audiences = ExactRestriction::exact([AudienceRef::Principal {
        principal: principal("user"),
    }]);
    bounds
}

fn values(verb: &str) -> LocalOperationValues {
    LocalOperationValues {
        action: action(verb),
        resource_domain: domain(),
        processor: ProcessorRef::Principal {
            principal: principal("agent"),
        },
        audience: AudienceRef::Principal {
            principal: principal("user"),
        },
    }
}

/// Application entitlement: the scripted controller's inference only.
struct ControllerOwner;
impl OperationPolicyOwner for ControllerOwner {
    fn authorize_controller_admission(
        &self,
        _: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        _: u64,
    ) -> Result<ControllerAdmissionAllowance, meerkat_core::OperationAuthorizationError> {
        if facts.selection() != &selection()
            || facts.endpoint() != ENDPOINT
            || facts.wire_model() != MODEL
        {
            return Err(denied().into());
        }
        Ok(ControllerAdmissionAllowance {
            operation_values: vec![values("infer")],
            restrictions: ExecutionRestrictions::unrestricted(),
        })
    }

    fn authorize_operation(
        &self,
        _: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        match &binding.facts().operation {
            AuthorizationOperation::Model(facts)
                if purpose == LocalPolicyPurpose::Controller
                    && selection().matches_model_facts(facts)
                    && facts.endpoint.as_ref() == ENDPOINT
                    && facts.wire_model.as_ref() == MODEL => {}
            _ => return Err(denied().into()),
        }
        Ok(LocalPolicyAllowance {
            operation_values: vec![values("infer")],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
}

/// One host invocation policy over the real grant owner, serving both the
/// admission check and the admitted-work owner.
struct GrantedInvocation {
    pub grants: Arc<LocalGrantAuthority>,
    pub lineage: GrantLineageRef,
}
impl GrantedInvocation {
    fn invocable(&self, association: &InputAuthorityAssociation) -> bool {
        let claims = association.candidate();
        claims.requester == principal("user")
            && claims.ingress_actor == principal("frontend")
            && claims.logical_executor == principal("agent")
            && claims.represented_subject.is_none()
            && self
                .grants
                .resolve_lineage(
                    std::slice::from_ref(&self.lineage),
                    &principal("user"),
                    None,
                )
                .is_ok()
    }
}
impl AdmittedWorkPolicyOwner for GrantedInvocation {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        if !self.invocable(association) {
            return Err(denied().into());
        }
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms + 60_000,
        })
    }
}
impl ScopedMandateOwner for GrantedInvocation {
    fn authorize_invocation(
        &self,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        if ingress.requester() != &principal("user") || !self.invocable(association) {
            return Err(NativeAdmissionError::Refused(denied()));
        }
        Ok(())
    }
}

const MODEL: &str = "claude-sonnet-4-5";
const MEMBER: &str = "agent";
const PROMPT: &str = "Read my latest messages";
const ANSWER: &str = "The original request completed";
const ENDPOINT: &str = "https://controller-preparation.invalid/messages";
const WAIT: Duration = Duration::from_secs(10);

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
struct ScriptedProvider(AtomicUsize, Mutex<Option<Arc<PreparedResponsePause>>>);

#[derive(Default)]
struct PreparedResponsePause {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

/// Opt-in fixture transport hold after actual model authorization. The test
/// owns release; dropping that owner always lets the already entered response
/// settle before normal fixture shutdown. Default transport stays unchanged.
pub(crate) struct PreparedResponseHold(Arc<PreparedResponsePause>);
impl PreparedResponseHold {
    pub(crate) async fn entered(&self) { self.0.entered.notified().await; }
    pub(crate) fn release(&self) { self.0.release.notify_one(); }
}
impl Drop for PreparedResponseHold {
    fn drop(&mut self) { self.release(); }
}
impl ScriptedProvider {
    fn hold_next_response(&self) -> PreparedResponseHold {
        let pause = Arc::new(PreparedResponsePause::default());
        assert!(self.1.lock().unwrap().replace(pause.clone()).is_none());
        PreparedResponseHold(pause)
    }
}

#[async_trait]
impl LlmClient for ScriptedProvider {
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
        PlainModelRoute::new(ENDPOINT, MODEL)
    }

    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(selection())
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        self.0.fetch_add(1, Ordering::SeqCst);
        panic!("governed work must use the prepared native request entry")
    }

    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let authorization = request.authorization().unwrap();
        // Return native refusals through the provider contract. Core owns the
        // bounded reprepare/controller recovery; the fixture never retries.
        let prepared = match authorization.prepare(model_facts()) {
            Ok(prepared) => prepared,
            Err(error) => return Box::pin(futures::stream::iter([
                Err(LlmError::from_operation_authorization(error)),
            ])),
        };
        let current = match prepared.current() {
            Ok(current) => current,
            Err(error) => return Box::pin(futures::stream::iter([
                Err(LlmError::from_operation_authorization(error)),
            ])),
        };
        if let Err(error) = current.observe_entry(meerkat_core::OperationEntryBoundary::ModelRequest) {
            return Box::pin(futures::stream::iter([
                Err(LlmError::from_operation_observation(error)),
            ]));
        }
        if let Err(error) = current
            .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
        {
            return Box::pin(futures::stream::iter([
                Err(LlmError::from_operation_observation(error)),
            ]));
        }
        self.0.fetch_add(1, Ordering::SeqCst);
        let response = futures::stream::iter([
            Ok(LlmEvent::TextDelta {
                delta: ANSWER.into(),
                meta: None,
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            }),
        ]);
        let pause = self.1.lock().unwrap().take();
        match pause {
            None => Box::pin(response),
            Some(pause) => Box::pin(futures::StreamExt::flatten(
                futures::stream::once(async move {
                    pause.entered.notify_one();
                    pause.release.notified().await;
                    response
                }),
            )),
        }
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

// One native composition owns the existing service and machine throughout the
// fixture. There are no discovery modules and no additional session runtime.
async fn bootstrap_fixture(spec: MobBootstrapSpec, namespace: String) -> UnifiedRuntime {
    UnifiedRuntime::bootstrap(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec { namespace, modules: Vec::new() },
            pre_spawn: Vec::new(),
        },
        WAIT,
    )
    .await
    .unwrap()
}

async fn shutdown_fixture(runtime: &UnifiedRuntime) -> Result<(), String> {
    // Await the native owner through supervisor, lease and actor cleanup. Do
    // not drop an entered shutdown behind the fixture's input timeout.
    let report = runtime.shutdown().await;
    let terminal_clean = match &report.mob_terminal_shutdown {
        MobTerminalShutdownOutcome::Completed(report) => report.is_clean(),
        MobTerminalShutdownOutcome::AlreadyShutDown => true,
        _ => false,
    };
    if report.cleanup_completed() && terminal_clean {
        Ok(())
    } else {
        Err(format!("native fixture cleanup incomplete: {report:?}"))
    }
}

struct Fixture {
    // Keep the real private durable owners alive through native shutdown.
    // Only a synthetic credential is installed; no network transport is used.
    _directory: Arc<tempfile::TempDir>,
    source: Arc<PrivateOriginals>,
    machine: Arc<MeerkatMachine>,
    native: UnifiedRuntime,
    identity_runtime: PublicIdentityRuntime,
    identity: AgentIdentity,
    owner: Arc<ToolkitOriginalOwner>,
    association: meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    fault: Arc<DeliveryFault>,
    session: SessionId,
    provider: Arc<ScriptedProvider>,
    submission: PrivateOriginalSubmit,
}

type RecoverySourceStore = Box<dyn FnOnce(Arc<dyn toolkit_store::ToolkitStore>, Arc<DeliveryFault>)
    -> Arc<dyn toolkit_store::ToolkitStore> + Send>;
impl Fixture {
    async fn new(mode: DeliveryMode, wrong_destination: bool) -> Self {
        Self::with_source_store(mode, wrong_destination, None).await
    }
    async fn with_source_store(mode: DeliveryMode, wrong_destination: bool,
        source_store: Option<RecoverySourceStore>) -> Self {
        let directory = Arc::new(
            tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap(),
        );
        // Retain the actual composition and temporary directory if setup times out
        // or a construction assertion panics, so native shutdown precedes the
        // final failure. This is only fixture cleanup, not a runtime owner.
        let mut opened = None;
        let constructed = std::panic::AssertUnwindSafe(tokio::time::timeout(
            WAIT,
            Self::build(directory.clone(), mode, wrong_destination, &mut opened, source_store),
        ))
        .catch_unwind()
        .await;
        match constructed {
            Ok(Ok(fixture)) => fixture,
            failed => {
                let cleanup = match opened {
                    Some(runtime) => shutdown_fixture(&runtime).await,
                    None => Ok(()),
                };
                assert!(cleanup.is_ok(), "fixture setup cleanup failed: {cleanup:?}");
                match failed {
                    Ok(Err(_)) => panic!("native fixture construction timed out"),
                    Err(payload) => std::panic::resume_unwind(payload),
                    Ok(Ok(_)) => unreachable!(),
                }
            }
        }
    }

    async fn build(
        directory: Arc<tempfile::TempDir>,
        mode: DeliveryMode,
        wrong_destination: bool,
        opened: &mut Option<UnifiedRuntime>,
        source_store: Option<RecoverySourceStore>,
    ) -> Self {
        let owner = initialized(&directory.path().join("grants"));
        let grants = owner.grants();
        let controller = grants
            .issue_root(
                &principal("admin"),
                id("controller"),
                principal("agent"),
                None,
                ceiling("infer"),
            )
            .unwrap();
        let invocation = grants
            .issue_root(
                &principal("admin"),
                id("invocation"),
                principal("user"),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap();
        let configuration = owner.configuration(
            ScopedInvocation::from_policy(Arc::new(GrantedInvocation {
                grants,
                lineage: invocation,
            })),
            Arc::new(ControllerOwner),
            None,
        );
        let machine = Arc::new(
            MeerkatMachine::ephemeral()
                .with_local_grant_authorization(configuration)
                .unwrap(),
        );
        let provider = Arc::new(ScriptedProvider::default());
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
        *opened = Some(bootstrap_fixture(spec, namespace).await);
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

        meerkat_core::publish_token_lifecycle_acquired_for_identity(
            &machine.generated_auth_lease_handle(),
            pin.selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-native-caller-fixture"),
        )
        .unwrap();
        let fault = Arc::new(DeliveryFault::new(mode));
        let bridge = observed::ObservedBridge {
            inner: MobSessionBridge::new(handle.observer()).with_controller_pin_source(service),
            fault: fault.clone(),
            publication: owner.publication(),
        };
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
            None,
            None,
            Some(handle.definition().clone()),
        )));
        let runtime = native.identity_runtime().unwrap();
        let domain = owner.ingress_domain().clone();
        let raw_source: Arc<dyn toolkit_store::ToolkitStore> = Arc::new(
            toolkit_store::SqliteStore::open(directory.path().join("originals.sqlite3")).unwrap());
        let selected_source = match source_store {
            Some(select) => select(raw_source, fault.clone()),
            None => raw_source,
        };
        let source = Arc::new(
            PrivateOriginals::new(
                StoreScope {
                    realm: "test-realm".into(),
                    owner: "originals".into(),
                },
                selected_source,
                owner.clone(),
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
        let mut candidate = claims("prepared-original", &controller).candidate().clone();
        // These admission controls select no service operation grant. Match the
        // production no-grant binder while retaining the separate controller.
        candidate.authority_basis = WorkAuthorityBasis::TargetedGrantLineages {
            base: None,
            targets: Vec::new(),
        };
        candidate.target.logical_runtime =
            id(&meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string());
        candidate.target.context = id(&session.to_string());
        candidate.controller_grant_lineage = vec![controller];
        candidate.controller_model = Some(pin.selection().clone());
        candidate.controller_ceiling = ceiling("infer");
        let actual_target = candidate.target.clone();
        let registration = machine
            .current_session_registration_witness(&session)
            .await
            .unwrap();
        machine
            .bind_scoped_context(&registration, &actual_target)
            .await
            .unwrap();
        if wrong_destination {
            candidate.target.context_generation += 1;
        }
        let association =
            meerkat_authorization_contracts::work_association::InputAuthorityAssociation::new(
                candidate,
            )
            .unwrap();
        let retained = owner
            .bind_controller_only_async(
                association.clone(),
                source_custody(&association),
                &principal("user"),
                &principal("frontend"),
                |final_association| {
                    let source = source.clone();
                    let reference = original.clone();
                    async move { source.persist_binding(&reference, &final_association).await }
                },
            )
            .await
            .unwrap();
        let association = retained.association().clone();
        source
            .hold_binding(original.clone(), retained)
            .await
            .unwrap();
        Self {
            _directory: directory,
            source,
            machine,
            native: opened.take().unwrap(),
            identity_runtime: runtime,
            identity,
            owner,
            association,
            fault,
            session,
            provider,
            submission: PrivateOriginalSubmit {
                original,
                content: PROMPT.into(),
                transport_authentication: authentication,
            },
        }
    }

    async fn submit_with(
        &self,
        submission: PrivateOriginalSubmit,
    ) -> Result<PrivateOriginalSubmission, String> {
        tokio::time::timeout(WAIT, self.source.submit_authenticated_original(submission))
            .await
            .map_err(|_| "private submission timed out".to_owned())?
            .map_err(|error| format!("private submission failed: {error:?}"))
    }

    async fn submit(&self) -> Result<PrivateOriginalSubmission, String> {
        self.submit_with(self.submission.clone()).await
    }

    fn contributor(&self) -> Result<bool, meerkat_core::authorization::ControllerReadinessFailure> {
        let c = self.association.candidate();
        self.owner
            .admitted_original_basis(&c.ingress_namespace, &c.original_work)
            .map(|original| original.is_some())
    }

    async fn observe_contributor(&self) -> Result<bool, String> {
        tokio::time::timeout(WAIT, async {
            loop {
                match self.contributor() {
                    Err(
                        meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable,
                    ) => {}
                    observation => return observation.map_err(|error| error.to_string()),
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .map_err(|_| "source contributor observation timed out".to_owned())?
    }

    async fn wait_for_contributor(&self) -> Result<(), String> {
        tokio::time::timeout(WAIT, async {
            loop {
                match self.contributor() {
                    Ok(true) => return Ok(()),
                    Ok(false)
                    | Err(
                        meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable,
                    ) => {}
                    Err(error) => return Err(error.to_string()),
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .map_err(|_| "admitted source contributor was not observed".to_owned())?
    }

    async fn unresolved_count(&self) -> Result<usize, String> {
        tokio::time::timeout(WAIT, async {
            loop {
                let held = match self.source.retained.try_lock() {
                    Ok(retained) => retained
                        .iter()
                        .find(|held| held.reference == self.submission.original)
                        .cloned()
                        .ok_or_else(|| "retained original missing".to_owned())?,
                    Err(std::sync::TryLockError::WouldBlock) => {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                        continue;
                    }
                    Err(std::sync::TryLockError::Poisoned(_)) => {
                        return Err("retained originals lock poisoned".to_owned());
                    }
                };
                match held.unresolved.try_lock() {
                    Ok(unresolved) => return Ok(unresolved.len()),
                    Err(std::sync::TryLockError::WouldBlock) => {}
                    Err(std::sync::TryLockError::Poisoned(_)) => {
                        return Err("unresolved custody lock poisoned".to_owned());
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .map_err(|_| "unresolved custody observation timed out".to_owned())?
    }

    async fn native_inputs(&self) -> Result<(usize, usize), String> {
        let snapshot = tokio::time::timeout(
            WAIT,
            self.machine.meerkat_machine_spine_snapshot(&self.session),
        )
        .await
        .map_err(|_| "native input observation timed out".to_owned())?
        .ok_or_else(|| "native session snapshot missing".to_owned())?;
        Ok((
            snapshot.ledger.input_count,
            snapshot.inputs.admission_order.len(),
        ))
    }

    async fn finish(&self, ticket: &str) -> Result<(), String> {
        let ticket = TurnTicket::parse(ticket).map_err(|error| format!("{error:?}"))?;
        let outcome = tokio::time::timeout(
            WAIT,
            self.identity_runtime
                .wait_for_turn(&self.identity, ticket, WAIT),
        )
        .await
        .map_err(|_| "native turn completion timed out".to_owned())?;
        if matches!(outcome, TurnOutcome::Completed { output: TurnOutput::Text { text, .. } } if text == ANSWER)
        {
            Ok(())
        } else {
            Err("native ticket did not complete the original answer".to_owned())
        }
    }

    async fn shutdown(&self) -> Result<(), String> {
        // A failed observation must never leave the real bridge held while
        // native shutdown waits for its admitted operation to settle.
        self.fault.release.add_permits(1);
        shutdown_fixture(&self.native).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_private_caller_promotes_fresh_acceptance_and_native_ticket_completes_original() {
    let fixture = Fixture::new(DeliveryMode::Normal, false).await;
    let before = fixture.observe_contributor().await;
    let result = fixture.submit().await;
    let completion = match &result {
        Ok(PrivateOriginalSubmission::Admitted {
            turn_ticket: Some(ticket),
            ..
        }) => Some(fixture.finish(ticket).await),
        _ => None,
    };
    let contributor = fixture.observe_contributor().await;
    let unresolved = fixture.unresolved_count().await;
    let accepted = fixture.fault.accepted.load(Ordering::SeqCst);
    let inputs = fixture.native_inputs().await;
    let requests = fixture.provider.0.load(Ordering::SeqCst);
    let cleanup = fixture.shutdown().await;

    assert!(cleanup.is_ok(), "native cleanup failed: {cleanup:?}");
    assert_eq!(before, Ok(false));
    let Ok(PrivateOriginalSubmission::Admitted {
        session_id,
        deduplicated: false,
        turn_ticket: Some(_),
        source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted,
        ..
    }) = result
    else {
        panic!("actual native fresh admission and source promotion required: {}", submission_context(&result))
    };
    assert_eq!(session_id, fixture.session.to_string());
    assert_eq!(completion, Some(Ok(())));
    assert_eq!(contributor, Ok(true));
    assert_eq!(unresolved, Ok(0));
    assert_eq!(accepted, 1);
    assert_eq!(inputs, Ok((1, 1)));
    assert_eq!(requests, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_private_retry_uses_native_replay_without_a_second_input_or_controller_request() {
    let fixture = Fixture::new(DeliveryMode::Normal, false).await;
    let first = fixture.submit().await;
    let completion = match &first {
        Ok(PrivateOriginalSubmission::Admitted {
            turn_ticket: Some(ticket),
            ..
        }) => Some(fixture.finish(ticket).await),
        _ => None,
    };
    let first_contributor = fixture.observe_contributor().await;
    let mut retry = fixture.submission.clone();
    retry.transport_authentication.resource_id = id("fresh-transport-authentication");
    let result = fixture.submit_with(retry).await;
    let contributor = fixture.observe_contributor().await;
    let unresolved = fixture.unresolved_count().await;
    let accepted = fixture.fault.accepted.load(Ordering::SeqCst);
    let inputs = fixture.native_inputs().await;
    let requests = fixture.provider.0.load(Ordering::SeqCst);
    let cleanup = fixture.shutdown().await;

    assert!(cleanup.is_ok(), "native cleanup failed: {cleanup:?}");
    let Ok(PrivateOriginalSubmission::Admitted {
        input_id: first_input,
        ..
    }) = first
    else {
        panic!("first native admission required: {}", submission_context(&first))
    };
    assert_eq!(completion, Some(Ok(())));
    assert_eq!(first_contributor, Ok(true));
    let Ok(PrivateOriginalSubmission::Admitted {
        input_id,
        deduplicated: true,
        source_settlement: PrivateSourceSettlement::RetryWithdrawn,
        ..
    }) = result
    else {
        panic!("native canonical replay must withdraw the retry guard: {}", submission_context(&result))
    };
    assert_eq!(input_id, first_input);
    assert_eq!(contributor, Ok(true));
    assert_eq!(unresolved, Ok(0));
    assert_eq!(accepted, 1);
    assert_eq!(inputs, Ok((1, 1)));
    assert_eq!(requests, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_native_destination_refusal_withdraws_private_source_candidate() {
    let fixture = Fixture::new(DeliveryMode::Normal, true).await;
    let first = fixture.submit().await;
    let contributor = fixture.observe_contributor().await;
    let unresolved = fixture.unresolved_count().await;
    let accepted = fixture.fault.accepted.load(Ordering::SeqCst);
    let first_inputs = fixture.native_inputs().await;
    let requests = fixture.provider.0.load(Ordering::SeqCst);
    let first_refused = fixture.fault.refused.load(Ordering::SeqCst);
    // Definite refusal released the actual guard: retry can bind and reach the
    // same native refusal again, rather than stopping at Candidate conflict.
    let retry = fixture.submit().await;
    let final_refused = fixture.fault.refused.load(Ordering::SeqCst);
    let final_inputs = fixture.native_inputs().await;
    let cleanup = fixture.shutdown().await;

    assert!(cleanup.is_ok(), "native cleanup failed: {cleanup:?}");
    assert!(matches!(
        first,
        Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)
    ), "{}", submission_context(&first));
    assert_eq!(contributor, Ok(false));
    assert_eq!(unresolved, Ok(0));
    assert_eq!(accepted, 0);
    assert_eq!(first_inputs, Ok((0, 0)));
    assert_eq!(requests, 0);
    assert_eq!(first_refused, 1);
    assert!(matches!(
        retry,
        Ok(PrivateOriginalSubmission::NativeOutcomeUnavailable)
    ), "{}", submission_context(&retry));
    assert_eq!(final_refused, 2);
    assert_eq!(final_inputs, Ok((0, 0)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_post_admission_publication_failure_preserves_admission_and_native_ticket() {
    let fixture = Fixture::new(DeliveryMode::PoisonPublicationAfterAcceptance, false).await;
    let result = fixture.submit().await;
    let accepted = fixture.fault.accepted.load(Ordering::SeqCst);
    let contributor = fixture.observe_contributor().await;
    let unresolved = fixture.unresolved_count().await;
    let retry = fixture.submit().await;
    let inputs = fixture.native_inputs().await;
    let cleanup = fixture.shutdown().await;

    assert!(cleanup.is_ok(), "native cleanup failed: {cleanup:?}");
    let Ok(PrivateOriginalSubmission::Admitted {
        session_id,
        input_id: _,
        deduplicated: false,
        turn_ticket: Some(ticket),
        source_settlement: PrivateSourceSettlement::PostAdmissionPublicationFailed,
    }) = result
    else {
        panic!("publication failure cannot relabel native acceptance as refusal: {}", submission_context(&result))
    };
    assert_eq!(session_id, fixture.session.to_string());
    assert!(TurnTicket::parse(&ticket).is_ok());
    assert_eq!(accepted, 1);
    assert_eq!(contributor, Ok(false));
    assert_eq!(unresolved, Ok(1));
    assert!(matches!(retry, Ok(PrivateOriginalSubmission::Admitted {
        source_settlement: PrivateSourceSettlement::PostAdmissionPublicationFailed, ..
    })));
    assert_eq!(inputs, Ok((1, 1)));
    // admit() makes the guard non-pending before publication. Poison clears
    // only its unpromoted Candidate projection; Drop does not withdraw it.
    // The durable association and known native admission remain. Publication
    // is unavailable, so this is not successful completion or retry permission.
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_lost_native_acceptance_answer_reconciles_exact_input_without_resubmission() {
    let fixture = Fixture::new(DeliveryMode::LoseAcceptedAnswer, false).await;
    let result = fixture.submit().await;
    let accepted = fixture.fault.accepted.load(Ordering::SeqCst);
    let inputs = fixture.native_inputs().await;
    let contributor = fixture.observe_contributor().await;
    let unresolved = fixture.unresolved_count().await;
    let retry = fixture.submit().await;
    let after_inputs = fixture.native_inputs().await;
    let after_contributor = fixture.observe_contributor().await;
    let after_unresolved = fixture.unresolved_count().await;
    let after_accepted = fixture.fault.accepted.load(Ordering::SeqCst);
    let cleanup = fixture.shutdown().await;

    assert!(cleanup.is_ok(), "native cleanup failed: {cleanup:?}");
    assert!(matches!(
        result,
        Ok(PrivateOriginalSubmission::AdmissionUnresolved)
    ), "{}", submission_context(&result));
    assert_eq!(accepted, 1);
    assert_eq!(inputs, Ok((1, 1)));
    assert_eq!(contributor, Ok(false));
    assert_eq!(unresolved, Ok(1));
    assert!(matches!(retry, Ok(PrivateOriginalSubmission::Admitted {
        deduplicated: false, turn_ticket: None,
        source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, ..
    })));
    assert_eq!(after_inputs, Ok((1, 1)));
    assert_eq!(after_contributor, Ok(true));
    assert_eq!(after_unresolved, Ok(0));
    assert_eq!(after_accepted, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_caller_receiver_loss_cannot_withdraw_a_native_accepted_original() {
    let fixture = Fixture::new(DeliveryMode::HoldAccepted, false).await;
    let mut caller = tokio::spawn({
        let source = fixture.source.clone();
        let submission = fixture.submission.clone();
        async move {
            // The fault can be held through both bounded acceptance and source
            // observations before the receiver is deliberately aborted.
            tokio::time::timeout(WAIT * 3, source.submit_authenticated_original(submission)).await
        }
    });
    let entered = tokio::time::timeout(WAIT, fixture.fault.entered.notified()).await;
    // Native already accepted, but its receipt has not reached the Toolkit
    // settlement adapter. Collect fallible observations before releasing it.
    let accepted_before_release = fixture.fault.accepted.load(Ordering::SeqCst);
    let contributor_before_release = if entered.is_ok() {
        Some(fixture.observe_contributor().await)
    } else {
        None
    };
    caller.abort();
    let joined = tokio::time::timeout(WAIT, &mut caller).await;
    fixture.fault.release.add_permits(1);
    let promoted = if entered.is_ok() {
        Some(fixture.wait_for_contributor().await)
    } else {
        None
    };
    let unresolved = fixture.unresolved_count().await;
    // Read completion using the existing native replay ticket and identity,
    // not a second completion ledger after the first receiver disappeared.
    let replay = if matches!(&promoted, Some(Ok(()))) {
        Some(fixture.submit().await)
    } else {
        None
    };
    let completion = match &replay {
        Some(Ok(PrivateOriginalSubmission::Admitted {
            turn_ticket: Some(ticket),
            ..
        })) => Some(fixture.finish(ticket).await),
        _ => None,
    };
    let inputs = fixture.native_inputs().await;
    let requests = fixture.provider.0.load(Ordering::SeqCst);
    let cleanup = fixture.shutdown().await;
    // A timed-out join stays a failure. After native teardown, make one final
    // bounded collection attempt so even that path does not detach the waiter.
    let join_cleanup = if joined.is_err() {
        caller.abort();
        Some(tokio::time::timeout(WAIT, &mut caller).await)
    } else {
        None
    };

    assert!(cleanup.is_ok(), "native cleanup failed: {cleanup:?}");
    assert!(
        !matches!(join_cleanup, Some(Err(_))),
        "caller cleanup timed out"
    );
    assert!(
        entered.is_ok(),
        "real native fresh acceptance was not reached; wait: {entered:?}; accepted: {accepted_before_release}; caller: {:?}",
        joined.as_ref().map(|join| join.as_ref().map(|timed| timed.as_ref().map(submission_context)))
    );
    assert!(matches!(joined, Ok(Err(ref error)) if error.is_cancelled()));
    assert_eq!(accepted_before_release, 1);
    assert_eq!(contributor_before_release, Some(Ok(false)));
    assert_eq!(promoted, Some(Ok(())));
    assert_eq!(unresolved, Ok(0));
    assert!(matches!(
        replay,
        Some(Ok(PrivateOriginalSubmission::Admitted {
            deduplicated: true,
            turn_ticket: Some(_),
            source_settlement: PrivateSourceSettlement::RetryWithdrawn,
            ..
        }))
    ));
    assert_eq!(completion, Some(Ok(())));
    assert_eq!(inputs, Ok((1, 1)));
    assert_eq!(requests, 1);
}


/// Corrupt only the host's expected observation after a real native admission.
/// The actual machine row, receipt and admission count are never fabricated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_answer_wrong_content_authority_session_or_input_keeps_candidate() {
    use std::panic::AssertUnwindSafe;
    let fixture = Fixture::new(DeliveryMode::LoseAcceptedAnswer, false).await;
    let result = AssertUnwindSafe(async {
        let submitted = fixture.submit().await;
        assert!(matches!(submitted, Ok(PrivateOriginalSubmission::AdmissionUnresolved)), "{}", submission_context(&submitted));
        let held = fixture.source.retained.lock().unwrap().iter()
            .find(|held| held.reference == fixture.submission.original).cloned().unwrap();
        for fault in 0..4 {
            let pending = held.unresolved.lock().unwrap().pop().unwrap();
            let mut pending = match Arc::try_unwrap(pending) {
                Ok(value) => value,
                Err(_) => panic!("native unresolved custody was not returned to its keeper"),
            };
            let original_input = pending.input.clone();
            let original_session = pending.session.clone();
            match fault {
                0 => { let Input::Prompt(prompt) = &mut pending.input else { panic!("prompt") };
                    prompt.content = PromptInput::new("different instruction", None).content; },
                1 => {
                    let Input::Prompt(prompt) = &pending.input else { panic!("prompt") };
                    assert!(prompt.header.authority_association.is_some());
                    pending.input = Input::Prompt(PromptInput {
                        header: meerkat_runtime::InputHeader {
                            authority_association: None,
                            ..prompt.header.clone()
                        },
                        ..prompt.clone()
                    });
                    assert_eq!(pending.input.id(), original_input.id());
                },
                2 => pending.session = SessionId::new(),
                _ => {
                    let Input::Prompt(prompt) = &pending.input else { panic!("prompt") };
                    pending.input = Input::Prompt(PromptInput {
                        header: meerkat_runtime::InputHeader {
                            id: InputId::new(),
                            ..prompt.header.clone()
                        },
                        ..prompt.clone()
                    });
                    assert_ne!(pending.input.id(), original_input.id());
                    assert_eq!(pending.input.header().authority_association,
                        original_input.header().authority_association);
                },
            }
            held.unresolved.lock().unwrap().push(Arc::new(pending));
            let observed = fixture.source.reconcile_original_submission(&fixture.submission.original).await;
            assert!(matches!(observed,
                Err(PrivateOriginalError::Conflict) |
                Ok(Some(PrivateOriginalSubmission::AdmissionUnresolved))));
            assert_eq!(fixture.observe_contributor().await, Ok(false));
            assert_eq!(fixture.native_inputs().await, Ok((1, 1)));
            assert_eq!(fixture.unresolved_count().await, Ok(1));
            let pending = held.unresolved.lock().unwrap().pop().unwrap();
            let mut pending = match Arc::try_unwrap(pending) { Ok(value) => value, Err(_) => panic!("custody retained by observation") };
            pending.input = original_input; pending.session = original_session;
            held.unresolved.lock().unwrap().push(Arc::new(pending));
        }
        // Dropping an unpolled reader leaves the same keeper untouched. The
        // existing HoldAccepted control covers receiver loss during delivery.
        drop(fixture.source.reconcile_original_submission(&fixture.submission.original));
        assert_eq!(fixture.unresolved_count().await, Ok(1));
        assert!(matches!(fixture.submit().await, Ok(PrivateOriginalSubmission::Admitted {
            source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, ..
        })));
        assert_eq!(fixture.native_inputs().await, Ok((1, 1)));
    }).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closed_native_owner_does_not_withdraw_unresolved_candidate() {
    let fixture = Fixture::new(DeliveryMode::LoseAcceptedAnswer, false).await;
    let submitted = fixture.submit().await;
    let inputs = fixture.native_inputs().await;
    let source = fixture.source.clone();
    let reference = fixture.submission.original.clone();
    let held = source.retained.lock().unwrap().iter()
        .find(|held| held.reference == reference).cloned().unwrap();
    let cleanup = fixture.shutdown().await;
    drop(fixture);
    let observed = source.reconcile_original_submission(&reference).await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    assert!(matches!(submitted, Ok(PrivateOriginalSubmission::AdmissionUnresolved)), "{}", submission_context(&submitted));
    assert_eq!(inputs, Ok((1, 1)));
    assert!(matches!(observed, Err(PrivateOriginalError::Unavailable)));
    assert_eq!(held.unresolved.lock().unwrap().len(), 1);
    assert!(matches!(&*held.unresolved.lock().unwrap()[0].pending.lock().unwrap(), SourcePublication::Pending(_)));
}


#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_exact_observers_share_the_same_source_publication_result() {
    use std::panic::AssertUnwindSafe;
    let fixture = Fixture::new(DeliveryMode::LoseAcceptedAnswer, false).await;
    let result = AssertUnwindSafe(async {
        let submitted = fixture.submit().await;
        assert!(matches!(submitted, Ok(PrivateOriginalSubmission::AdmissionUnresolved)), "{}", submission_context(&submitted));
        let held = fixture.source.retained.lock().unwrap().iter()
            .find(|held| held.reference == fixture.submission.original).cloned().unwrap();
        let pending = held.unresolved.lock().unwrap()[0].clone();
        let stored = fixture.machine.input_state(&pending.session, pending.input.id()).await.unwrap().unwrap();
        verify_exact_replay(&stored.state, &pending.input).unwrap();
        // Both observers already hold the same guard and exact native evidence.
        // Exercise its serialized consumption directly to force the overlap;
        // neither observer submits another input or invents native acceptance.
        let start = Arc::new(tokio::sync::Barrier::new(3));
        let spawn = || {
            let pending = pending.clone(); let start = start.clone();
            tokio::spawn(async move { start.wait().await; pending.admit() })
        };
        let first = spawn(); let second = spawn();
        start.wait().await;
        let (first, second) = tokio::join!(first, second);
        assert_eq!(first.unwrap(), Ok(()));
        assert_eq!(second.unwrap(), Ok(()));
        assert!(matches!(fixture.source.reconcile_original_submission(&fixture.submission.original).await,
            Ok(Some(PrivateOriginalSubmission::Admitted {
                source_settlement: PrivateSourceSettlement::FreshAcceptancePromoted, ..
            }))));
        assert_eq!(fixture.observe_contributor().await, Ok(true));
        assert_eq!(fixture.native_inputs().await, Ok((1, 1)));
        assert_eq!(fixture.unresolved_count().await, Ok(0));
    }).catch_unwind().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test]
async fn fixture_controller_adapter_preserves_pin_and_prepares_without_model_requests() {
    let provider = Arc::new(ScriptedProvider::default());
    let direct: Arc<dyn LlmClient> = provider.clone();
    let probe: Arc<dyn LlmClient> = Arc::new(
        crate::installed_host::acquisition::ProbeProvider::new(provider.clone())
    );
    for raw in [direct, probe] {
        let adapter: Arc<dyn meerkat_core::AgentLlmClient> = Arc::new(
            meerkat_llm_core::LlmClientAdapter::try_for_provider_identity(
                raw, MODEL.to_owned(), Provider::Anthropic
            ).unwrap()
        );
        let observed = fixture_controller_decorator()(adapter.clone());
        assert!(Arc::ptr_eq(&adapter, &observed), "the diagnostic preserves the actual client");
        let pin = observed.pin_controller().unwrap();
        pin.client().prepare_controller_credential().await.unwrap();
        assert_eq!(provider.0.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn controller_only_owner_refuses_operation_basis_before_persistence() {
    let directory = tempfile::tempdir().unwrap();
    let owner = initialized(&directory.path().join("grants"));
    let grants = owner.grants();
    let controller = grants.issue_root(
        &principal("admin"), id("controller"), principal("agent"), None,
        ceiling("infer"),
    ).unwrap();
    let operation = grants.issue_root(
        &principal("admin"), id("operation"), principal("admin"), None,
        ceiling("read"),
    ).unwrap();
    let mut candidate = claims("wrong-controller-only-basis", &operation).candidate().clone();
    candidate.controller_grant_lineage = vec![controller];
    candidate.controller_model = Some(selection());
    candidate.controller_ceiling = ceiling("infer");
    let association = InputAuthorityAssociation::new(candidate).unwrap();
    let persistence_calls = AtomicUsize::new(0);
    let result = owner.bind_controller_only_async(
        association.clone(), source_custody(&association),
        &principal("user"), &principal("frontend"),
        |_| async {
            persistence_calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        },
    ).await;
    assert_eq!(persistence_calls.load(Ordering::SeqCst), 0);
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("controller-only binding accepted an operation grant basis"),
    };
    assert!(matches!(error, crate::original_ingress::OriginalWriterError::UnsupportedMandate),
        "{error:?}");
}
