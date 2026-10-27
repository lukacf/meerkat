//! Copied from meerkat-mob's governed ingress fixture (tests/governed_support).
//! Shared fixture for governed member ingress: a real mob whose member
//! session runs on the real scoped grant composition (scoped original check,
//! host invocation policy and required scoped destination), with inputs bound
//! by `bind_scoped_original` and a gate that can hold one native admission.
#![allow(
    dead_code,
    unused_imports,
    clippy::expect_used,
    clippy::manual_assert_eq,
    clippy::panic,
    clippy::unwrap_used
)]

pub use std::collections::{BTreeMap, HashMap};
pub use std::sync::{Arc, Condvar, Mutex};
pub use std::time::Duration;

pub use async_trait::async_trait;
pub use meerkat::{AgentFactory, Config, EphemeralSessionService, FactoryAgentBuilder};
pub use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, ControllerAdmissionAllowance, OperationPolicyOwner, WorkOwnerAllowance,
};
pub use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
pub use meerkat_authorization::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose,
};
pub use meerkat_authorization::publication::LocalAuthorizationPublication;
pub use meerkat_authorization::work::HostAuthorizationClock;
pub use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, DomainRestriction, ExactRestriction, ExecutionRestrictions,
    ProcessorRef, ResourceDomain,
};
pub use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
pub use meerkat_authorization_contracts::protocol::ContractRequirements;
pub use meerkat_authorization_contracts::resource::ResourceRef;
pub use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
    NativeWorkTarget, OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
};
pub use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationFacts, ModelAuthorizationUse,
    OperationObservedOutcome, OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
};
pub use meerkat_core::connection::{AuthCredentialIdentity, RealmId};
pub use meerkat_core::types::HandlingMode;
pub use meerkat_core::{
    AgentError, AgentSessionStore, ControllerModelSelection, Message, PrincipalKind, PrincipalRef,
    Provider, Session, SessionId, SessionLlmIdentity, StopReason, TrustDomainId,
};
pub use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream, PreparedLlmRequest,
};
pub use meerkat_mob::{
    AgentIdentity, MobBuilder, MobDefinition, MobError, MobHandle, MobId, MobRuntimeMode,
    MobStorage, Profile, ProfileBinding, ProfileName, SpawnMemberSpec, ToolConfig,
};
pub use meerkat_runtime::RuntimeDriverError;
pub use meerkat_runtime::identifiers::LogicalRuntimeId;
pub use meerkat_runtime::input::{Input, PromptInput};
pub use meerkat_runtime::input_authority::{NativeAdmissionError, NativeIngressContext};
pub use meerkat_runtime::meerkat_machine::{
    AuthenticatedOriginal, AuthenticatedOriginalSource, MeerkatMachine, RetainedCustody,
    ScopedGrantComposition, ScopedInvocation, ScopedMandateOwner,
};
pub use meerkat_runtime::service_ext::SessionServiceRuntimeExt;

pub const MODEL: &str = "claude-sonnet-4-5";
pub const ENDPOINT: &str = "https://governed-mob.invalid/messages";
pub const PROMPT: &str = "Summarize the governed request";
pub const ANSWER: &str = "governed answer";
pub const MEMBER: &str = "governed-worker";
pub const WAIT: Duration = Duration::from_secs(30);

pub fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("bounded fixture identifier")
}

pub fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        value,
        TrustDomainId::new("governed-mob").expect("domain"),
    )
    .expect("qualified fixture principal")
}

pub fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}

pub fn evidence(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ResourceDomain {
                authority: principal("ingress"),
                namespace: "observations".into(),
            },
            resource_id: value.into(),
        },
        revision: id("observation-1"),
        digest: EvidenceDigest::from_array([1; 32]),
    }
}

pub fn identity() -> SessionLlmIdentity {
    SessionLlmIdentity {
        model: MODEL.into(),
        provider: Provider::Anthropic,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    }
}

pub fn credential() -> AuthCredentialIdentity {
    serde_json::from_value(
        serde_json::json!({"realm":"governed-mob", "account":"scripted-controller"}),
    )
    .expect("nonsecret identity of the actual fixture transport")
}

pub fn selection() -> ControllerModelSelection {
    ControllerModelSelection::new(
        identity(),
        credential(),
        "scripted-profile".into(),
        "scripted-transport".into(),
    )
}

pub fn model_facts() -> ModelAuthorizationFacts {
    ModelAuthorizationFacts {
        identity: Arc::new(identity()),
        wire_model: MODEL.into(),
        hosted_capabilities: Arc::from([]),
        backend_profile_id: Some("scripted-profile".into()),
        backend_kind: "scripted-transport".into(),
        endpoint: ENDPOINT.into(),
        credential: Some(credential()),
        usage: ModelAuthorizationUse::ControllerInference,
        live_channel: None,
    }
}

pub fn action(value: &str) -> ActionRef {
    ActionRef {
        feature: "governed-mob".into(),
        action: value.into(),
    }
}

pub fn domain() -> ResourceDomain {
    ResourceDomain {
        authority: principal("resource-owner"),
        namespace: "records".into(),
    }
}

pub fn ceiling(action_name: &str) -> ExecutionRestrictions {
    let mut bounds = ExecutionRestrictions::unrestricted();
    bounds.actions = ExactRestriction::exact([action(action_name)]);
    bounds.resource_domains = DomainRestriction::exact([domain()]);
    bounds.processors = ExactRestriction::exact([ProcessorRef::Principal {
        principal: principal("executor"),
    }]);
    bounds.audiences = ExactRestriction::exact([AudienceRef::Principal {
        principal: principal("requester"),
    }]);
    bounds
}

pub fn values(verb: &str) -> LocalOperationValues {
    LocalOperationValues {
        action: action(verb),
        resource_domain: domain(),
        processor: ProcessorRef::Principal {
            principal: principal("executor"),
        },
        audience: AudienceRef::Principal {
            principal: principal("requester"),
        },
    }
}

/// Application entitlement: the scripted controller's inference only.
pub struct ControllerOwner;
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
pub struct GrantedInvocation {
    pub grants: Arc<LocalGrantAuthority>,
    pub lineage: GrantLineageRef,
}
impl GrantedInvocation {
    pub fn invocable(&self, association: &InputAuthorityAssociation) -> bool {
        let claims = association.candidate();
        claims.requester == principal("requester")
            && claims.ingress_actor == principal("ingress")
            && claims.logical_executor == principal("executor")
            && claims.represented_subject.is_none()
            && self
                .grants
                .resolve_lineage(
                    std::slice::from_ref(&self.lineage),
                    &principal("requester"),
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
        if ingress.requester() != &principal("requester") || !self.invocable(association) {
            return Err(NativeAdmissionError::Refused(denied()));
        }
        Ok(())
    }
}

pub fn sources() -> Vec<OriginalWorkRef> {
    ["source-a", "source-b"]
        .into_iter()
        .map(|work| OriginalWorkRef {
            authority: principal("source-owner"),
            work: id(work),
        })
        .collect()
}

/// The host adapter's custody: original `work-7` with its two sources, each
/// held as an original of its own, addressed to the current `target`.
pub struct Adapter {
    pub target: Mutex<Option<NativeWorkTarget>>,
    pub basis: Mutex<Option<WorkAuthorityBasis>>,
    pub gate: AdmissionGate,
    /// Panic inside the custody read of `work-7` with this ordinal (counted
    /// by the gate; 0 never panics). Native admission reads it to take
    /// credential custody, to authenticate before resolution, and again
    /// inside the admission attempt, so the third read fails with the
    /// outcome unknown.
    pub panic_at_custody_read: std::sync::atomic::AtomicUsize,
    /// Answer the retained custody of both sources as `AdmissionPending`: the
    /// source owner has not settled their admission yet.
    pub sources_pending: std::sync::atomic::AtomicBool,
}

/// Holds the native admission of original `work-7` inside the adapter's
/// custody read, so a test can keep one admission in flight in the member's
/// lane. Counts every admission that reached the custody read.
#[derive(Default)]
pub struct AdmissionGate {
    pub state: Mutex<GateState>,
    pub changed: Condvar,
}
#[derive(Default)]
pub struct GateState {
    pub held: bool,
    pub entered: usize,
    /// The transport authentication of each admission that reached it.
    pub authentications: Vec<String>,
}
impl AdmissionGate {
    pub fn hold(&self) {
        self.state.lock().expect("gate").held = true;
    }
    pub fn release(&self) {
        self.state.lock().expect("gate").held = false;
        self.changed.notify_all();
    }
    pub fn entered(&self) -> usize {
        self.state.lock().expect("gate").entered
    }
    pub fn authentications(&self) -> Vec<String> {
        self.state.lock().expect("gate").authentications.clone()
    }
    pub fn pass(&self, authentication: &str) {
        let wait = || {
            let mut state = self.state.lock().expect("gate");
            state.entered += 1;
            state.authentications.push(authentication.to_string());
            while state.held {
                state = self.changed.wait(state).expect("gate");
            }
        };
        // The custody read is synchronous; give up this worker while held.
        if tokio::runtime::Handle::try_current().is_ok_and(|runtime| {
            runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        }) {
            tokio::task::block_in_place(wait);
        } else {
            wait();
        }
    }
}
impl AuthenticatedOriginalSource for Adapter {
    fn authenticated_original(
        &self,
        ingress: &NativeIngressContext,
        _namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<
        Option<AuthenticatedOriginal>,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        let target = self.target.lock().expect("fixture").clone();
        let basis = self.basis.lock().expect("fixture").clone();
        if ingress.requester() != &principal("requester") {
            return Ok(None);
        }
        if let Some(source) = sources().into_iter().find(|source| source == original) {
            return Ok(target.map(|target| AuthenticatedOriginal {
                occurrence_scope: id(&format!("occurrence-{}", source.work.as_str())),
                original_work: source,
                original_authentication: evidence("original-authentication"),
                root_event: evidence("original-event"),
                contributing_work: Vec::new(),
                target,
                authority_basis: WorkAuthorityBasis::HostPolicy {
                    policy: evidence("source-policy"),
                },
            }));
        }
        if original.work != id("work-7") {
            return Ok(None);
        }
        self.gate
            .pass(&ingress.authentication().resource.resource_id);
        let panic_at = self
            .panic_at_custody_read
            .load(std::sync::atomic::Ordering::SeqCst);
        assert!(
            panic_at == 0 || self.gate.entered() != panic_at,
            "injected failure inside the native admission attempt"
        );
        Ok(target
            .zip(basis)
            .map(|(target, authority_basis)| AuthenticatedOriginal {
                original_work: OriginalWorkRef {
                    authority: principal("source-owner"),
                    work: id("work-7"),
                },
                occurrence_scope: id("occurrence-7"),
                original_authentication: evidence("original-authentication"),
                root_event: evidence("original-event"),
                contributing_work: sources(),
                target,
                authority_basis,
            }))
    }

    fn retained_custody(
        &self,
        _namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<RetainedCustody, meerkat_core::authorization::ControllerReadinessFailure> {
        Ok(if sources().contains(original) {
            if self
                .sources_pending
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                RetainedCustody::AdmissionPending
            } else {
                RetainedCustody::Retained(WorkAuthorityBasis::HostPolicy {
                    policy: evidence("source-policy"),
                })
            }
        } else {
            RetainedCustody::NotRetained
        })
    }
}

#[derive(Default)]
pub struct RecordingStore(Mutex<HashMap<SessionId, Session>>);
#[async_trait]
impl AgentSessionStore for RecordingStore {
    async fn save(&self, session: &Session) -> Result<(), AgentError> {
        self.0
            .lock()
            .expect("fixture session store")
            .insert(session.id().clone(), session.clone());
        Ok(())
    }
    async fn load(&self, id: &str) -> Result<Option<Session>, AgentError> {
        Ok(SessionId::parse(id).ok().and_then(|id| {
            self.0
                .lock()
                .expect("fixture session store")
                .get(&id)
                .cloned()
        }))
    }
}

/// A scripted physical transport that only answers governed requests: it
/// checks the native controller authorization before entry.
#[derive(Default)]
pub struct ScriptedProvider {
    pub requests: Mutex<Vec<Vec<Message>>>,
}
impl ScriptedProvider {
    pub fn requests(&self) -> Vec<Vec<Message>> {
        self.requests.lock().expect("request observations").clone()
    }
}

#[async_trait]
impl LlmClient for ScriptedProvider {
    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if logical_model != MODEL {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        meerkat_llm_core::PlainModelRoute::new(ENDPOINT, MODEL)
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(selection())
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }
    fn stream<'a>(&'a self, _: &'a LlmRequest) -> LlmStream<'a> {
        panic!("governed member work must reach the prepared provider entry")
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let check = request
            .authorization()
            .expect("native work reaches actual provider")
            .prepare(model_facts())
            .expect("actual native/grant controller authorization")
            .current()
            .expect("final controller entry check");
        check
            .observe_entry(meerkat_core::OperationEntryBoundary::ModelRequest)
            .expect("native audit stages actual transport entry");
        check
            .observe_outcome(OperationObservedOutcome::HttpResponse { status: 200 })
            .expect("native audit stages fixture response headers");
        self.requests
            .lock()
            .expect("request observations")
            .push(request.request().messages.clone());
        Box::pin(futures::stream::iter([
            Ok(LlmEvent::TextDelta {
                delta: ANSWER.into(),
                meta: None,
            }),
            Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            }),
        ]))
    }
    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

pub fn association(
    runtime: &LogicalRuntimeId,
    controller: GrantLineageRef,
    operation: GrantLineageRef,
    selected: ControllerModelSelection,
) -> InputAuthorityAssociationCandidate {
    InputAuthorityAssociationCandidate {
        requester: principal("requester"),
        ingress_actor: principal("ingress"),
        represented_subject: None,
        original_authentication: evidence("original-authentication"),
        logical_executor: principal("executor"),
        target: NativeWorkTarget {
            logical_owner: principal("native-owner"),
            logical_runtime: id(&runtime.to_string()),
            context: id("native-context"),
            context_generation: 1,
            audience: AudienceRef::Principal {
                principal: principal("requester"),
            },
        },
        // The original's source owner is not the ingress authority.
        original_work: OriginalWorkRef {
            authority: principal("source-owner"),
            work: id("work-7"),
        },
        root_event: evidence("original-event"),
        contributing_work: sources(),
        authority_basis: WorkAuthorityBasis::GrantLineage {
            lineage: vec![operation],
        },
        controller_grant_lineage: vec![controller],
        controller_model: Some(selected),
        controller_ceiling: ceiling("infer"),
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: QualifiedIngressNamespace {
            realm: RealmId::parse("governed-mob").expect("realm"),
            ingress: ResourceDomain {
                authority: principal("ingress"),
                namespace: "prompts".into(),
            },
            occurrence_scope: id("occurrence-7"),
        },
        contract: ContractRequirements::local_governed_v1(),
    }
}

/// The governed mob, whose worker profile admits external work only when
/// `external_addressable`.
pub fn mob_definition_with(external_addressable: bool) -> MobDefinition {
    let mut profiles = BTreeMap::new();
    profiles.insert(
        ProfileName::from("worker"),
        ProfileBinding::Inline(Box::new(Profile {
            model_fallback: None,
            model: MODEL.to_string(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: Vec::new(),
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..Default::default()
            },
            peer_description: "governed member".to_string(),
            external_addressable,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
            durable: false,
        })),
    );
    let mut definition = MobDefinition::explicit(MobId::from(format!(
        "governed-ingress-{}",
        uuid::Uuid::new_v4().simple()
    )));
    definition.profiles = profiles;
    definition
}

/// A mob whose member session runs on the real scoped grant composition,
/// with the member's session document bound to the original's destination.
pub struct Fixture {
    pub handle: MobHandle,
    pub machine: Arc<MeerkatMachine>,
    pub provider: Arc<ScriptedProvider>,
    pub adapter: Arc<Adapter>,
    pub member: AgentIdentity,
    pub session_id: SessionId,
    pub runtime: LogicalRuntimeId,
    pub controller: GrantLineageRef,
    pub operation: GrantLineageRef,
    pub pin: meerkat_core::ControllerModelClient,
    pub service: Arc<EphemeralSessionService<FactoryAgentBuilder>>,
    pub target: NativeWorkTarget,
}

impl Fixture {
    pub async fn new() -> Self {
        Self::composed(true).await
    }

    /// The fixture whose member profile does not admit external work.
    pub async fn non_addressable() -> Self {
        Self::composed(false).await
    }

    async fn composed(external_addressable: bool) -> Self {
        let grants = Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: principal("grant-owner"),
                    namespace: id("governed-grants"),
                    generation: 1,
                },
                LocalAuthorizationPublication::new(),
                Arc::new(HostAuthorizationClock),
            )
            .expect("actual generated grant owner"),
        );
        let controller = grants
            .issue_root(
                &principal("grant-owner"),
                id("controller"),
                principal("executor"),
                None,
                ceiling("infer"),
            )
            .expect("actual controller lineage");
        let operation = grants
            .issue_root(
                &principal("grant-owner"),
                id("read-only"),
                principal("executor"),
                None,
                ceiling("read"),
            )
            .expect("actual operation grant");
        let invocation = grants
            .issue_root(
                &principal("grant-owner"),
                id("invocation"),
                principal("requester"),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .expect("actual requester invocation grant");
        let adapter = Arc::new(Adapter {
            target: Mutex::new(None),
            basis: Mutex::new(None),
            gate: AdmissionGate::default(),
            panic_at_custody_read: Default::default(),
            sources_pending: Default::default(),
        });
        let configuration = ScopedGrantComposition {
            grants: Arc::clone(&grants),
            ingress_domain: ResourceDomain {
                authority: principal("ingress"),
                namespace: "prompts".into(),
            },
            originals: adapter.clone(),
            invocation: ScopedInvocation::from_policy(Arc::new(GrantedInvocation {
                grants: Arc::clone(&grants),
                lineage: invocation,
            })),
            operation_owner: Arc::new(ControllerOwner),
            retained_resume: None,
        }
        .into_configuration();
        assert!(configuration.require_scoped_destination);
        let machine = Arc::new(
            MeerkatMachine::ephemeral()
                .with_local_grant_authorization(configuration)
                .expect("exclusive actual native host setup"),
        );
        let provider = Arc::new(ScriptedProvider::default());
        let mut builder =
            FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
        builder.default_llm_client = Some(provider.clone());
        builder.default_session_store = Some(Arc::new(RecordingStore::default()));
        let service = Arc::new(EphemeralSessionService::new(builder, 4));
        let handle = MobBuilder::new(
            mob_definition_with(external_addressable),
            MobStorage::in_memory(),
        )
        .with_session_service(service.clone())
        .with_runtime_adapter(Arc::clone(&machine))
        .allow_ephemeral_sessions(true)
        .create()
        .await
        .expect("mob over the governed machine");
        let member = AgentIdentity::from(MEMBER);
        handle
            .spawn_spec(SpawnMemberSpec::new("worker", member.clone()))
            .await
            .expect("spawn the governed member");
        let session_id = handle
            .get_member(&member)
            .await
            .expect("roster read")
            .expect("spawned member")
            .bridge_session_id()
            .cloned()
            .expect("session-backed member");
        let actor = service
            .live_session_actor_witness(&session_id)
            .await
            .expect("actual live member actor");
        let pin = service
            .pin_controller_client_for_actor(&actor)
            .await
            .expect("actual immutable selected controller client");
        assert!(pin.selection() == &selection());
        // Publication only tries the machine's controller custody: while the
        // spawned member's own session work holds a session lock, it answers
        // the typed Busy refusal instead of waiting. Core's bounded
        // publication retries exactly that refusal until the deadline.
        meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
            &machine.generated_auth_lease_handle(),
            pin.selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-governed-mob-credential"),
            tokio::time::Instant::now() + WAIT,
        )
        .await
        .expect("actual generated fixture credential before native admission");
        let runtime = LogicalRuntimeId::for_session(&session_id);
        let target = association(
            &runtime,
            controller.clone(),
            operation.clone(),
            pin.selection().clone(),
        )
        .target;
        *adapter.target.lock().expect("fixture") = Some(target.clone());
        *adapter.basis.lock().expect("fixture") = Some(WorkAuthorityBasis::GrantLineage {
            lineage: vec![operation.clone()],
        });
        let registration = machine
            .current_session_registration_witness(&session_id)
            .await
            .expect("current member registration");
        machine
            .bind_scoped_context(&registration, &target)
            .await
            .expect("bind the member session document's destination");
        Self {
            handle,
            machine,
            provider,
            adapter,
            member,
            session_id,
            runtime,
            controller,
            operation,
            pin,
            service,
            target,
        }
    }

    /// The original event `event-7`, bound to its final input by the
    /// embedder with `bind_scoped_original` under its own first
    /// authentication.
    pub fn bound(&self, change: impl FnOnce(&mut InputAuthorityAssociationCandidate)) -> Input {
        self.bound_under(change, "original-authentication")
    }

    /// The same original event bound under one transport authentication
    /// observation (a retry carries a fresh one).
    pub fn bound_under(
        &self,
        change: impl FnOnce(&mut InputAuthorityAssociationCandidate),
        transport_authentication: &str,
    ) -> Input {
        self.bound_with(&self.pin.clone(), change, transport_authentication)
    }

    /// [`Self::bound_under`] with the controller the binder was handed.
    pub fn bound_with(
        &self,
        controller: &meerkat_core::ControllerModelClient,
        change: impl FnOnce(&mut InputAuthorityAssociationCandidate),
        transport_authentication: &str,
    ) -> Input {
        let mut candidate = association(
            &self.runtime,
            self.controller.clone(),
            self.operation.clone(),
            controller.selection().clone(),
        );
        change(&mut candidate);
        meerkat_runtime::meerkat_machine::bind_scoped_original(
            Input::Prompt(PromptInput::new(PROMPT, None)),
            InputAuthorityAssociation::new(candidate).expect("claims"),
            meerkat_runtime::identifiers::IdempotencyKey::new("event-7"),
            principal("requester"),
            principal("ingress"),
            RealmId::parse("governed-mob").expect("realm"),
            evidence(transport_authentication),
            controller.clone(),
        )
        .expect("the final input is bound to its trusted ingress observation")
    }
}

pub fn mentions_prompt(messages: &[Message]) -> bool {
    messages.iter().any(|message| {
        serde_json::to_string(message)
            .expect("message renders")
            .contains(PROMPT)
    })
}

pub async fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}
