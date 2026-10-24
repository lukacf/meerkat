//! A governed placed member for the cross-host rows: the member host's
//! machine is governed by a static native host, and every peer delivery it
//! is offered is bound by a peer ingress binder that pins the member's live
//! controller and binds Core's exact input with `bind_scoped_original`, as a
//! trusted host's binder does.
#![allow(dead_code, clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use futures::StreamExt as _;
use meerkat_authorization_contracts::constraints::{
    AudienceRef, ExecutionRestrictions, ResourceDomain,
};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
    NativeWorkTarget, OriginalWorkRef, QualifiedIngressNamespace, WorkAuthorityBasis,
};
use meerkat_core::authorization::{
    ControllerReadinessFailure, OperationRefusalKind, OperationRefused,
    PreparedAuthorizationBinding, PreparedOperationAuthorization, WorkAuthorization,
    WorkAuthorizationContext,
};
use meerkat_core::connection::{AuthCredentialIdentity, RealmId};
use meerkat_core::{
    ControllerModelSelection, Message, PrincipalKind, PrincipalRef, Provider, SessionId,
    SessionLlmIdentity, StopReason, TrustDomainId,
};
use meerkat_llm_core::{
    LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest, LlmStream, PlainModelRoute,
    PreparedLlmRequest,
};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::Input;
use meerkat_runtime::input_authority::{
    NativeAdmissionError, NativeIngressContext, NativeWorkAuthorizationHost, NativeWorkBatch,
    NativeWorkContextError, PeerIngressBindFuture, PeerIngressBindRefusal, PeerIngressBindRequest,
    PeerIngressBinder, PeerIngressBinding, PeerIngressOrigin,
};
use meerkat_runtime::meerkat_machine::MeerkatMachine;

use super::support::GovernedMemberMachine;

/// The placed worker profile's model.
pub const MODEL: &str = "claude-haiku-4-5-20251001";
const ENDPOINT: &str = "https://governed-member.invalid/messages";
const ANSWER: &str = "governed member answer";

fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        value,
        TrustDomainId::new("governed-member").expect("domain"),
    )
    .expect("qualified fixture principal")
}

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("bounded fixture identifier")
}

fn evidence(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ResourceDomain {
                authority: principal("peer-ingress"),
                namespace: "observations".into(),
            },
            resource_id: value.into(),
        },
        revision: id("observation-1"),
        digest: EvidenceDigest::from_array([7; 32]),
    }
}

fn realm() -> RealmId {
    RealmId::parse("governed-member").expect("realm")
}

/// The member's selected controller: the scripted transport below.
pub fn selection() -> ControllerModelSelection {
    ControllerModelSelection::new(
        SessionLlmIdentity {
            model: MODEL.into(),
            provider: Provider::Anthropic,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        credential(),
        "governed-member-profile".into(),
        "scripted-transport".into(),
    )
}

fn credential() -> AuthCredentialIdentity {
    serde_json::from_value(
        serde_json::json!({"realm": "governed-member", "account": "scripted-controller"}),
    )
    .expect("nonsecret identity of the fixture transport")
}

/// The member's controller transport. Governed work reaches it through the
/// prepared entry, with its work authorization; while held, every governed
/// request it enters waits there until released.
pub struct GovernedMemberClient {
    released: tokio::sync::watch::Sender<bool>,
    governed_entered: AtomicUsize,
    ungoverned: AtomicUsize,
}

impl Default for GovernedMemberClient {
    fn default() -> Self {
        Self {
            released: tokio::sync::watch::channel(true).0,
            governed_entered: AtomicUsize::new(0),
            ungoverned: AtomicUsize::new(0),
        }
    }
}

impl GovernedMemberClient {
    /// Hold every governed request from now on.
    pub fn hold(&self) {
        self.released.send_replace(false);
    }

    /// Release every held governed request.
    pub fn release(&self) {
        self.released.send_replace(true);
    }

    /// How many governed requests reached the transport.
    pub fn governed_entered(&self) -> usize {
        self.governed_entered.load(Ordering::SeqCst)
    }

    /// How many requests reached the transport without work authorization.
    pub fn ungoverned(&self) -> usize {
        self.ungoverned.load(Ordering::SeqCst)
    }

    fn answer<'a>(&self) -> LlmStream<'a> {
        let mut released = self.released.subscribe();
        Box::pin(
            futures::stream::once(async move {
                while !*released.borrow_and_update() {
                    if released.changed().await.is_err() {
                        break;
                    }
                }
                Ok(LlmEvent::TextDelta {
                    delta: ANSWER.into(),
                    meta: None,
                })
            })
            .chain(futures::stream::iter([Ok(LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::EndTurn),
            })])),
        )
    }
}

#[async_trait::async_trait]
impl LlmClient for GovernedMemberClient {
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

    fn stream<'a>(&'a self, _request: &'a LlmRequest) -> LlmStream<'a> {
        self.ungoverned.fetch_add(1, Ordering::SeqCst);
        self.answer()
    }

    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        if request.authorization().is_none() {
            self.ungoverned.fetch_add(1, Ordering::SeqCst);
        } else {
            self.governed_entered.fetch_add(1, Ordering::SeqCst);
        }
        self.answer()
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }

    async fn health_check(&self) -> Result<(), LlmError> {
        Ok(())
    }
}

/// Prepares no operation: the member's governed turns call no tool.
struct NoOperations;

impl WorkAuthorization for NoOperations {
    fn prepare(
        &self,
        _binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, meerkat_core::OperationAuthorizationError>
    {
        Err(OperationRefused::new(OperationRefusalKind::Denied).into())
    }
}

/// A static native host: it admits an association bound by the binder below
/// (its requester and ingress, the member's controller) and refuses every
/// other.
struct StaticHost;

impl NativeWorkAuthorizationHost for StaticHost {
    fn authenticate_association(
        &self,
        _runtime_id: &LogicalRuntimeId,
        _input: &Input,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        let candidate = association.candidate();
        if ingress.requester() == &candidate.requester
            && candidate.requester == principal("peer-requester")
            && ingress.ingress_actor() == &principal("peer-ingress")
            && candidate.controller_model.as_ref() == Some(&selection())
        {
            Ok(())
        } else {
            Err(OperationRefused::new(OperationRefusalKind::MalformedFacts).into())
        }
    }

    fn work_context(
        &self,
        batch: &NativeWorkBatch,
    ) -> Result<WorkAuthorizationContext, NativeWorkContextError> {
        Ok(WorkAuthorizationContext::new(
            Arc::new(NoOperations),
            batch.execution_scope().clone(),
        ))
    }
}

/// One bind the binder was asked for.
#[derive(Clone, Debug)]
pub struct BindCall {
    pub session_id: SessionId,
    /// `Some(tracked)` for a supervisor delivery, `None` for a comms one.
    pub supervisor_tracked: Option<bool>,
    pub event_key: String,
    pub input_id: String,
}

/// The trusted host's peer delivery binder: it pins the receiving member's
/// live controller without waiting behind a turn and binds Core's exact input
/// under an association naming Core's work for the delivery.
#[derive(Default)]
struct Binder {
    service: OnceLock<Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>>,
    calls: Mutex<Vec<BindCall>>,
}

impl Binder {
    async fn bind(
        &self,
        request: PeerIngressBindRequest,
    ) -> Result<PeerIngressBinding, PeerIngressBindRefusal> {
        let not_ready = |failure| PeerIngressBindRefusal::NotReady(failure);
        let service = self
            .service
            .get()
            .ok_or(not_ready(ControllerReadinessFailure::ExecutorUnavailable))?;
        let witness = service
            .live_session_actor_witness(&request.session_id)
            .await
            .ok_or(not_ready(ControllerReadinessFailure::ExecutorUnavailable))?;
        let pin = match service.try_pin_controller_client_for_actor(&witness).await {
            Ok(pin) => pin,
            Err(meerkat_session::TryPinControllerError::TurnBoundaryHeld) => {
                return Err(not_ready(ControllerReadinessFailure::TurnBoundaryHeld));
            }
            Err(error) => return Err(PeerIngressBindRefusal::Unavailable(error.to_string())),
        };
        let association = InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: principal("peer-requester"),
            ingress_actor: principal("peer-ingress"),
            represented_subject: None,
            original_authentication: evidence("supervisor-transport"),
            logical_executor: principal("executor"),
            target: NativeWorkTarget {
                logical_owner: principal("native-owner"),
                logical_runtime: id(&request.runtime_id.to_string()),
                context: id("member-context"),
                context_generation: 1,
                audience: AudienceRef::Principal {
                    principal: principal("peer-requester"),
                },
            },
            original_work: OriginalWorkRef {
                authority: principal("peer-ingress"),
                work: request.delivery_work.clone(),
            },
            root_event: evidence("peer-delivery"),
            contributing_work: Vec::new(),
            authority_basis: WorkAuthorityBasis::HostPolicy {
                policy: evidence("peer-policy"),
            },
            controller_grant_lineage: vec![GrantLineageRef {
                root_authority: principal("grant-owner"),
                authority_namespace: id("controller"),
                authority_generation: 1,
                authority_incarnation: serde_json::from_value(serde_json::json!(
                    "00000000-0000-4000-8000-000000000007"
                ))
                .expect("fixture incarnation"),
                grant_id: id("controller-leaf"),
                issued_revision: 1,
            }],
            controller_model: Some(pin.selection().clone()),
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
            source_observations: Vec::new(),
            ingress_namespace: QualifiedIngressNamespace {
                realm: realm(),
                ingress: ResourceDomain {
                    authority: principal("peer-ingress"),
                    namespace: "peers".into(),
                },
                occurrence_scope: id("peer-deliveries"),
            },
            contract: ContractRequirements::local_governed_v1(),
        })
        .map_err(|error| PeerIngressBindRefusal::Unavailable(format!("{error:?}")))?;
        let bound = meerkat_runtime::meerkat_machine::bind_scoped_original(
            request.input,
            association,
            request.event_key,
            principal("peer-requester"),
            principal("peer-ingress"),
            realm(),
            evidence("supervisor-transport"),
            pin,
        )
        .map_err(|error| PeerIngressBindRefusal::Unavailable(error.to_string()))?;
        Ok(PeerIngressBinding::new(bound))
    }
}

impl PeerIngressBinder for Binder {
    fn bind_peer_ingress(&self, request: PeerIngressBindRequest) -> PeerIngressBindFuture<'_> {
        self.calls.lock().expect("calls").push(BindCall {
            session_id: request.session_id.clone(),
            supervisor_tracked: match &request.origin {
                PeerIngressOrigin::SupervisorDelivery(origin) => Some(origin.tracked),
                _ => None,
            },
            event_key: request.event_key.to_string(),
            input_id: request.input.id().to_string(),
        });
        Box::pin(self.bind(request))
    }
}

/// The governed member composition handed to the host fixture.
pub struct GovernedMember {
    client: Arc<GovernedMemberClient>,
    binder: Arc<Binder>,
}

impl GovernedMember {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            client: Arc::new(GovernedMemberClient::default()),
            binder: Arc::new(Binder::default()),
        })
    }

    /// The member's controller transport.
    pub fn client(&self) -> Arc<GovernedMemberClient> {
        Arc::clone(&self.client)
    }

    /// Every bind the binder was asked for, in order.
    pub fn calls(&self) -> Vec<BindCall> {
        self.binder.calls.lock().expect("calls").clone()
    }

    /// The supervisor deliveries the binder was offered, in order.
    pub fn supervisor_calls(&self) -> Vec<BindCall> {
        self.calls()
            .into_iter()
            .filter(|call| call.supervisor_tracked.is_some())
            .collect()
    }
}

impl GovernedMemberMachine for GovernedMember {
    fn native_host(&self, machine: &MeerkatMachine) -> Arc<dyn NativeWorkAuthorizationHost> {
        // The controller's credential, through the machine's own generated
        // credential owner, as a host that acquired it would supply it.
        meerkat_core::publish_token_lifecycle_acquired_for_identity(
            &machine.generated_auth_lease_handle(),
            selection().credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-governed-member-credential"),
        )
        .expect("the controller credential is acquired");
        Arc::new(StaticHost)
    }

    fn peer_ingress_binder(&self) -> Arc<dyn PeerIngressBinder> {
        Arc::clone(&self.binder) as Arc<dyn PeerIngressBinder>
    }

    fn attach_member_service(
        &self,
        service: Arc<meerkat_session::PersistentSessionService<meerkat::FactoryAgentBuilder>>,
    ) {
        assert!(
            self.binder.service.set(service).is_ok(),
            "one member service"
        );
    }
}
