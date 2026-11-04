//! The binder that keeps the original requester, row by row: what it binds,
//! what it refuses (typed), and its custody of what it answered.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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
use meerkat_core::comms::PeerId;
use meerkat_core::connection::RealmId;
use meerkat_core::interaction::{
    InteractionId, PeerIngressAuthDecision, PeerIngressKind, PeerIngressSenderAuthentication,
    PeerInputClass,
};
use meerkat_core::types::SessionId;
use meerkat_core::{
    ControllerModelClient, ControllerModelSelection, PrincipalKind, PrincipalRef, TrustDomainId,
};

use super::*;
use crate::identifiers::{IdempotencyKey, LogicalRuntimeId};
use crate::input::{Input, PromptInput};
use crate::input_authority::{
    BoundOriginalPin, GovernedPeerOrigin, PeerIngressBindDenial, PeerIngressBindRefusal,
    PeerIngressBindRequest, PeerIngressClaimKind, PeerIngressCommsOrigin, PeerIngressOrigin,
    check_bound_original,
};

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("bounded fixture identifier")
}

fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        value,
        TrustDomainId::new("peer-binder").expect("domain"),
    )
    .expect("qualified fixture principal")
}

fn evidence(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ResourceDomain {
                authority: principal("ingress"),
                namespace: "observations".into(),
            },
            resource_id: value.into(),
        },
        revision: id("observation-1"),
        digest: EvidenceDigest::from_array([9; 32]),
    }
}

fn realm() -> RealmId {
    RealmId::parse("peer-binder").expect("realm")
}

fn ingress_domain(namespace: &str) -> ResourceDomain {
    ResourceDomain {
        authority: principal("ingress"),
        namespace: namespace.into(),
    }
}

fn lineage(grant: &str) -> GrantLineageRef {
    GrantLineageRef {
        root_authority: principal("grant-owner"),
        authority_namespace: id("grants"),
        authority_generation: 1,
        authority_incarnation: serde_json::from_value(serde_json::json!(
            "00000000-0000-4000-8000-000000000009"
        ))
        .expect("fixture incarnation"),
        grant_id: id(grant),
        issued_revision: 1,
    }
}

fn work(name: &str) -> OriginalWorkRef {
    OriginalWorkRef {
        authority: principal("ingress"),
        work: id(name),
    }
}

fn selection(account: &str) -> ControllerModelSelection {
    ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: "controller".into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(serde_json::json!({"realm": "peer-binder", "account": account}))
            .expect("credential identity"),
        "profile".into(),
        "fixture".into(),
    )
}

/// A controller transport of `selection` that is never called.
struct SelectedController(ControllerModelSelection);

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl meerkat_core::AgentLlmClient for SelectedController {
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(self.0.clone())
    }
    async fn stream_response(
        &self,
        _: &[meerkat_core::Message],
        _: &[Arc<meerkat_core::ToolDef>],
        _: u32,
        _: Option<f32>,
        _: Option<&meerkat_core::ProviderParamsOverride>,
    ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
        Err(meerkat_core::AgentError::ConfigError(
            "binder fixture never calls a model".into(),
        ))
    }
    fn provider(&self) -> meerkat_core::Provider {
        self.0.provider()
    }
    fn model(&self) -> &str {
        self.0.model()
    }
}

fn controller(account: &str) -> ControllerModelClient {
    let selection = selection(account);
    ControllerModelClient::new(selection.clone(), Arc::new(SelectedController(selection)))
}

/// The admitted association of one original of the sending work: the
/// requester's work on the sending agent, in `namespace`, naming
/// `contributors`, under `basis`.
fn sender_original(
    original: &str,
    namespace: &str,
    contributors: &[OriginalWorkRef],
    basis: WorkAuthorityBasis,
) -> InputAuthorityAssociation {
    InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester: principal("requester"),
        ingress_actor: principal("ingress"),
        represented_subject: None,
        original_authentication: evidence("requester-authentication"),
        logical_executor: principal("data-agent"),
        target: NativeWorkTarget {
            logical_owner: principal("native-owner"),
            logical_runtime: id("sender-runtime"),
            context: id("sender-context"),
            context_generation: 1,
            audience: AudienceRef::Principal {
                principal: principal("requester"),
            },
        },
        original_work: work(original),
        root_event: evidence("requester-event"),
        contributing_work: contributors.to_vec(),
        authority_basis: basis,
        controller_grant_lineage: vec![lineage("sender-controller")],
        controller_model: Some(selection("sender")),
        controller_ceiling: ExecutionRestrictions::unrestricted(),
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: QualifiedIngressNamespace {
            realm: realm(),
            ingress: ingress_domain(namespace),
            occurrence_scope: id(&format!("{original}-occurrence")),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .expect("the sending work's claims")
}

fn delegation() -> WorkAuthorityBasis {
    WorkAuthorityBasis::GrantLineage {
        lineage: vec![lineage("requester-delegation")],
    }
}

/// The origin of a sending work whose one staged original `question` names
/// the requester's two sources.
fn origin() -> GovernedPeerOrigin {
    GovernedPeerOrigin::from_associations([&sender_original(
        "question",
        "prompts",
        &[work("source-a"), work("source-b")],
        delegation(),
    )])
    .expect("one staged original")
}

/// A comms delivery's facts, with `governed_origin` and how the transport
/// established the sender.
fn comms_origin(
    authentication: PeerIngressSenderAuthentication,
    governed_origin: Option<GovernedPeerOrigin>,
) -> PeerIngressOrigin {
    PeerIngressOrigin::Comms(PeerIngressCommsOrigin {
        sender: sender_peer(),
        signing_key: None,
        route_label: None,
        sender_authentication: authentication,
        auth: PeerIngressAuthDecision::Required,
        class: PeerInputClass::ActionableMessage,
        kind: PeerIngressKind::Message,
        interaction_id: InteractionId(uuid::Uuid::from_u128(7)),
        in_reply_to: None,
        claim: PeerIngressClaimKind::Volatile,
        governed_origin,
    })
}

fn sender_peer() -> PeerId {
    PeerId(uuid::Uuid::from_u128(42))
}

fn receiver_session() -> SessionId {
    SessionId::from_uuid(uuid::Uuid::from_u128(1001))
}

/// Core's request to bind delivery `event` for the receiving session.
fn request(origin: PeerIngressOrigin, event: &str) -> PeerIngressBindRequest {
    let session_id = receiver_session();
    let mut input = Input::Prompt(PromptInput::new("Please answer for my requester", None));
    input.header_mut().idempotency_key = Some(IdempotencyKey::new(event));
    PeerIngressBindRequest::new(
        LogicalRuntimeId::for_session(&session_id),
        session_id,
        None,
        origin,
        IdempotencyKey::new(event),
        input,
    )
    .expect("Core's request")
}

fn governed_request(event: &str) -> PeerIngressBindRequest {
    request(
        comms_origin(PeerIngressSenderAuthentication::InProcess, Some(origin())),
        event,
    )
}

/// What the scripted host answers `basis_for` with.
#[derive(Clone)]
enum Answer {
    /// The receiving agent's side under its basis grant `basis`, with a
    /// controller client of the account `controller`.
    Grant {
        basis: &'static str,
        controller: &'static str,
    },
    NoBasis,
    NotReady,
    /// Wait until the test releases the read, then answer `Grant`.
    Held,
}

/// A host's receiving side over a first-writer-wins custody of originals.
struct ScriptedHost {
    answer: Mutex<Answer>,
    reads: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    /// The association enrolled per original work: the first one stands.
    enrolled: Mutex<HashMap<String, InputAuthorityAssociation>>,
    enrollments: AtomicUsize,
}

impl ScriptedHost {
    fn new(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer),
            reads: AtomicUsize::new(0),
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            enrolled: Mutex::new(HashMap::new()),
            enrollments: AtomicUsize::new(0),
        })
    }

    fn answer(&self, answer: Answer) {
        *self.answer.lock().expect("answer") = answer;
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    fn enrolled(&self, delivery_work: &EvidenceId) -> Option<InputAuthorityAssociation> {
        self.enrolled
            .lock()
            .expect("enrolled")
            .get(delivery_work.as_str())
            .cloned()
    }

    fn grant(request: &PeerServiceRequest, basis: &str, account: &str) -> PeerServiceGrant {
        PeerServiceGrant {
            receiver: principal("owner-agent"),
            represented_subject: Some(principal("data-owner")),
            authority_basis: WorkAuthorityBasis::GrantLineage {
                lineage: vec![lineage(basis)],
            },
            target: NativeWorkTarget {
                logical_owner: principal("native-owner"),
                logical_runtime: id(&request.runtime_id.to_string()),
                context: id("receiver-context"),
                context_generation: 1,
                audience: AudienceRef::Principal {
                    principal: principal("requester"),
                },
            },
            controller_grant_lineage: vec![lineage("receiver-controller")],
            controller_ceiling: ExecutionRestrictions::unrestricted(),
            admitted_ceiling: ExecutionRestrictions::unrestricted(),
            controller: controller(account),
        }
    }
}

impl PeerServiceBasis for ScriptedHost {
    fn basis_for<'a>(&'a self, request: &'a PeerServiceRequest) -> PeerServiceFuture<'a> {
        Box::pin(async move {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let answer = self.answer.lock().expect("answer").clone();
            match answer {
                Answer::Grant { basis, controller } => Ok(Self::grant(request, basis, controller)),
                Answer::NoBasis => Err(PeerServiceRefusal::NoBasis),
                Answer::NotReady => Err(PeerServiceRefusal::NotReady(
                    meerkat_core::authorization::ControllerReadinessFailure::ExecutorUnavailable,
                )),
                Answer::Held => {
                    self.entered.notify_one();
                    self.release.notified().await;
                    Ok(Self::grant(request, "owner-delegation", "receiver"))
                }
            }
        })
    }

    fn enroll(
        &self,
        request: &PeerServiceRequest,
        association: &InputAuthorityAssociation,
    ) -> Result<Option<Box<dyn NativeInputAdmissionCustody>>, PeerServiceRefusal> {
        self.enrollments.fetch_add(1, Ordering::SeqCst);
        let mut enrolled = self.enrolled.lock().expect("enrolled");
        match enrolled.get(request.delivery_work.as_str()) {
            Some(first) if first != association => Err(PeerServiceRefusal::EnrollmentConflict),
            Some(_) => Ok(None),
            None => {
                enrolled.insert(
                    request.delivery_work.as_str().to_owned(),
                    association.clone(),
                );
                Ok(None)
            }
        }
    }
}

fn owner_grant() -> Answer {
    Answer::Grant {
        basis: "owner-delegation",
        controller: "receiver",
    }
}

fn binder(host: &Arc<ScriptedHost>) -> OriginalRequesterPeerBinder {
    OriginalRequesterPeerBinder::new(Arc::clone(host) as Arc<dyn PeerServiceBasis>)
}

/// Bind `request`, and Core's check of the binding under the pin Core
/// uses for a delivery with its governed origin.
async fn bind(
    binder: &OriginalRequesterPeerBinder,
    request: PeerIngressBindRequest,
) -> Result<InputAuthorityAssociation, PeerIngressBindRefusal> {
    let handed = request.input.clone();
    let runtime = request.runtime_id.clone();
    let work = request.delivery_work.clone();
    let PeerIngressOrigin::Comms(comms) = &request.origin else {
        panic!("comms delivery")
    };
    let origin = comms.governed_origin.clone();
    let binding = binder.bind_peer_ingress(request).await?;
    let (bound, _) = binding.into_parts();
    let pin = match &origin {
        Some(origin) => BoundOriginalPin::GovernedDelivery {
            work: &work,
            origin,
        },
        None => BoundOriginalPin::DeliveryWork(&work),
    };
    check_bound_original(&handed, &bound, &runtime, pin)
        .expect("Core accepts the binder's binding");
    Ok(bound
        .header()
        .authority_association
        .clone()
        .expect("bound association"))
}

/// The receiving work is admitted for the sending work's requester, with the
/// sending agent as its ingress actor: on the association and on the
/// trusted ingress observation alike.
#[tokio::test]
async fn a_governed_in_process_delivery_binds_the_senders_requester() {
    let host = ScriptedHost::new(owner_grant());
    let binder = binder(&host);
    let request = governed_request("delivery-1");
    let handed = request.input.clone();
    let binding = binder
        .bind_peer_ingress(request)
        .await
        .expect("a governed in-process delivery is bound");
    let (bound, custody) = binding.into_parts();
    assert!(custody.is_none(), "the host kept no admission custody");
    let claims = bound
        .header()
        .authority_association
        .as_ref()
        .expect("bound association")
        .candidate();
    assert_eq!(claims.requester, principal("requester"));
    assert_eq!(claims.ingress_actor, principal("data-agent"));
    assert_ne!(
        claims.ingress_actor, claims.requester,
        "the requester is carried, never the delivery's sender"
    );
    let ingress = bound
        .header()
        .ingress_context
        .as_ref()
        .expect("trusted ingress observation");
    assert_eq!(ingress.requester(), &principal("requester"));
    assert_eq!(ingress.ingress_actor(), &principal("data-agent"));
    assert!(
        ingress.authentication() == &claims.original_authentication,
        "the first authentication is the original's own"
    );
    assert_eq!(
        bound.id(),
        handed.id(),
        "Core's exact input, under Core's id for the delivery"
    );
}

/// The receiving work names every work of the sending work as a contributor,
/// so the requester's own current ceiling narrows each of its operations.
#[tokio::test]
async fn the_receiving_work_names_the_sending_work_as_a_contributor() {
    let host = ScriptedHost::new(owner_grant());
    let claims = bind(&binder(&host), governed_request("delivery-1"))
        .await
        .expect("bound")
        .candidate()
        .clone();
    assert!(
        claims.contributing_work == vec![work("question"), work("source-a"), work("source-b")],
        "the staged original first, then every original it names"
    );
}

/// A sending work that is itself a peer work names its sender's whole chain,
/// so the next hop names the original requester's work too: every hop is
/// narrowed by the requester's own ceiling.
#[tokio::test]
async fn a_peer_work_names_its_senders_whole_chain_at_the_next_hop() {
    let host = ScriptedHost::new(owner_grant());
    let first = bind(&binder(&host), governed_request("hop-1"))
        .await
        .expect("first hop bound");
    let next = GovernedPeerOrigin::from_associations([&first]).expect("the peer work's origin");
    assert_eq!(next.requester(), &principal("requester"));
    assert_eq!(next.sender_executor(), &principal("owner-agent"));
    let second = bind(
        &binder(&host),
        request(
            comms_origin(PeerIngressSenderAuthentication::InProcess, Some(next)),
            "hop-2",
        ),
    )
    .await
    .expect("second hop bound");
    let claims = second.candidate();
    assert_eq!(claims.requester, principal("requester"));
    assert!(
        claims.contributing_work
            == vec![
                first.candidate().original_work.clone(),
                work("question"),
                work("source-a"),
                work("source-b"),
            ],
        "the second hop names the first hop's work and everything it named"
    );
}

/// The receiving work is bound in the sending work's realm and ingress
/// domain, where its contributors resolve, with the delivery's own
/// occurrence scope and Core's work for the delivery as its original. The
/// receiving agent's own facts are the host's: its executor, its subject,
/// its basis, its controller.
#[tokio::test]
async fn the_receiving_work_is_bound_where_the_sending_work_resolves() {
    let host = ScriptedHost::new(owner_grant());
    let request = governed_request("delivery-1");
    let delivery_work = request.delivery_work.clone();
    let claims = bind(&binder(&host), request)
        .await
        .expect("bound")
        .candidate()
        .clone();
    assert_eq!(claims.ingress_namespace.realm, realm());
    assert_eq!(claims.ingress_namespace.ingress, ingress_domain("prompts"));
    assert_eq!(claims.ingress_namespace.occurrence_scope, delivery_work);
    assert_eq!(claims.original_work.work, delivery_work);
    assert_eq!(claims.original_work.authority, principal("ingress"));
    assert_eq!(claims.logical_executor, principal("owner-agent"));
    assert_eq!(
        claims.represented_subject,
        Some(principal("data-owner")),
        "the receiving agent's own subject, never the sending work's"
    );
    assert_eq!(
        claims.authority_basis,
        WorkAuthorityBasis::GrantLineage {
            lineage: vec![lineage("owner-delegation")],
        }
    );
    assert_eq!(
        claims.controller_grant_lineage,
        vec![lineage("receiver-controller")]
    );
    assert_eq!(claims.controller_model, Some(selection("receiver")));
    assert!(
        host.enrolled(&delivery_work)
            .as_ref()
            .map(|a| a.candidate())
            == Some(&claims),
        "the exact minted association is in the host's custody"
    );
}

/// A delivery without an origin Core established is refused
/// `origin_unverifiable`, typed, before the host is asked: a network
/// delivery (even with a verified signature), and an in-process delivery
/// from an ungoverned sender.
#[tokio::test]
async fn a_network_delivery_without_an_origin_is_refused_origin_unverifiable() {
    for authentication in [
        PeerIngressSenderAuthentication::SignatureVerified,
        PeerIngressSenderAuthentication::InProcess,
    ] {
        let host = ScriptedHost::new(owner_grant());
        let refused = binder(&host)
            .bind_peer_ingress(request(comms_origin(authentication, None), "delivery-1"))
            .await
            .expect_err("refused");
        assert_eq!(
            refused,
            PeerIngressBindRefusal::Refused(PeerIngressBindDenial::OriginUnverifiable),
            "{authentication:?}"
        );
        assert_eq!(
            host.reads(),
            0,
            "{authentication:?}: the host is never asked"
        );
    }
}

/// A receiver the host holds no basis for is refused
/// `no_peer_service_basis`, typed, and the refusal is recorded: a later bind
/// of the same delivery answers it again without asking the host, even once
/// the host would grant.
#[tokio::test]
async fn no_peer_service_basis_refuses_the_delivery() {
    let host = ScriptedHost::new(Answer::NoBasis);
    let binder = binder(&host);
    for attempt in 0..2 {
        let refused = binder
            .bind_peer_ingress(governed_request("delivery-1"))
            .await
            .expect_err("refused");
        assert_eq!(
            refused,
            PeerIngressBindRefusal::Refused(PeerIngressBindDenial::NoPeerServiceBasis),
            "attempt {attempt}"
        );
        host.answer(owner_grant());
    }
    assert_eq!(
        host.reads(),
        1,
        "the recorded refusal answers the redelivery"
    );
    assert!(
        binder
            .bind_peer_ingress(governed_request("delivery-2"))
            .await
            .is_ok(),
        "another delivery is the host's to answer again"
    );
}

/// A pending host answer records nothing: the delivery is bound once the
/// host can answer.
#[tokio::test]
async fn a_pending_host_answer_records_nothing() {
    let host = ScriptedHost::new(Answer::NotReady);
    let binder = binder(&host);
    assert!(matches!(
        binder
            .bind_peer_ingress(governed_request("delivery-1"))
            .await,
        Err(PeerIngressBindRefusal::NotReady(_))
    ));
    host.answer(owner_grant());
    assert!(bind(&binder, governed_request("delivery-1")).await.is_ok());
}

/// A sending work with an original under a host-policy (or service-mandate)
/// basis is refused `sender_basis_unbounded`: that original would narrow
/// nothing, and the receiver could not be held to the requester's ceiling.
#[tokio::test]
async fn a_sender_with_a_host_policy_basis_is_refused_sender_basis_unbounded() {
    let host = ScriptedHost::new(owner_grant());
    let policy_sender = GovernedPeerOrigin::from_associations([&sender_original(
        "question",
        "prompts",
        &[],
        WorkAuthorityBasis::HostPolicy {
            policy: evidence("host-policy"),
        },
    )])
    .expect("one staged original");
    let refused = binder(&host)
        .bind_peer_ingress(request(
            comms_origin(
                PeerIngressSenderAuthentication::InProcess,
                Some(policy_sender),
            ),
            "delivery-1",
        ))
        .await
        .expect_err("refused");
    assert_eq!(
        refused,
        PeerIngressBindRefusal::Refused(PeerIngressBindDenial::SenderBasisUnbounded)
    );
    assert_eq!(host.reads(), 0);
}

/// A sending work that names more works than one association may name is
/// refused `contributor_bound_exceeded`: the bound is the association's own
/// contributor bound, so no work of the chain is ever dropped to fit.
#[tokio::test]
async fn a_sending_work_past_the_contributor_bound_is_refused() {
    let host = ScriptedHost::new(owner_grant());
    let at_bound: Vec<_> = (1..MAX_ASSOCIATION_REFERENCES)
        .map(|n| work(&format!("source-{n}")))
        .collect();
    let fits = GovernedPeerOrigin::from_associations([&sender_original(
        "question",
        "prompts",
        &at_bound,
        delegation(),
    )])
    .expect("one staged original");
    assert_eq!(fits.sender_work().len(), MAX_ASSOCIATION_REFERENCES);
    assert!(
        bind(
            &binder(&host),
            request(
                comms_origin(PeerIngressSenderAuthentication::InProcess, Some(fits)),
                "at-bound",
            ),
        )
        .await
        .is_ok(),
        "a sending work of exactly the bound is named whole"
    );
    // A second staged original takes the chain past the bound.
    let past = GovernedPeerOrigin::from_associations([
        &sender_original("question", "prompts", &at_bound, delegation()),
        &sender_original("follow-up", "prompts", &[], delegation()),
    ])
    .expect("two staged originals");
    let refused = binder(&host)
        .bind_peer_ingress(request(
            comms_origin(PeerIngressSenderAuthentication::InProcess, Some(past)),
            "past-bound",
        ))
        .await
        .expect_err("refused");
    assert_eq!(
        refused,
        PeerIngressBindRefusal::Refused(PeerIngressBindDenial::ContributorBoundExceeded)
    );
}

/// A sending work whose originals were admitted in two ingress domains
/// cannot be named in the one namespace a receiving work has: refused
/// `origin_unverifiable`.
#[tokio::test]
async fn a_sending_work_in_two_ingress_domains_is_refused_origin_unverifiable() {
    let host = ScriptedHost::new(owner_grant());
    let mixed = GovernedPeerOrigin::from_associations([
        &sender_original("question", "prompts", &[], delegation()),
        &sender_original("callback", "callbacks", &[], delegation()),
    ])
    .expect("two staged originals");
    assert!(mixed.namespace().is_none());
    let refused = binder(&host)
        .bind_peer_ingress(request(
            comms_origin(PeerIngressSenderAuthentication::InProcess, Some(mixed)),
            "delivery-1",
        ))
        .await
        .expect_err("refused");
    assert_eq!(
        refused,
        PeerIngressBindRefusal::Refused(PeerIngressBindDenial::OriginUnverifiable)
    );
}

/// After Core refused a binding (for example, a replay conflict at
/// admission), a re-bind of the same delivery returns the association the
/// binder minted, verbatim.
#[tokio::test]
async fn rebind_after_core_refusal_returns_the_same_association() {
    let host = ScriptedHost::new(owner_grant());
    let binder = binder(&host);
    let first = bind(&binder, governed_request("delivery-1"))
        .await
        .expect("bound");
    // Core refused the binding: the binding and its custody are dropped.
    let again = bind(&binder, governed_request("delivery-1"))
        .await
        .expect("bound again");
    assert!(again == first, "the re-bind is verbatim");
    assert_eq!(
        host.enrollments.load(Ordering::SeqCst),
        2,
        "each bind enrolls the same original again"
    );
}

/// A re-bind after the requester's or the receiver's delegation changed
/// still returns the association the binder minted first, verbatim: only
/// the controller client is the host's current one.
#[tokio::test]
async fn rebind_after_delegation_change_returns_the_same_association() {
    let host = ScriptedHost::new(owner_grant());
    let binder = binder(&host);
    let first = bind(&binder, governed_request("delivery-1"))
        .await
        .expect("bound");
    host.answer(Answer::Grant {
        basis: "reissued-owner-delegation",
        controller: "receiver",
    });
    let again = bind(&binder, governed_request("delivery-1"))
        .await
        .expect("bound again");
    assert!(again == first, "the first association stands verbatim");
    assert!(
        host.enrolled(&governed_request("delivery-1").delivery_work) == Some(first),
        "the host's custody holds the first association"
    );
}

/// A re-bind whose host now pins a controller of another selection than the
/// minted association's is refused for good, and recorded.
#[tokio::test]
async fn a_rebind_with_another_controller_selection_is_refused_and_recorded() {
    let host = ScriptedHost::new(owner_grant());
    let binder = binder(&host);
    bind(&binder, governed_request("delivery-1"))
        .await
        .expect("bound");
    host.answer(Answer::Grant {
        basis: "owner-delegation",
        controller: "another-receiver-account",
    });
    assert_eq!(
        binder
            .bind_peer_ingress(governed_request("delivery-1"))
            .await
            .expect_err("refused"),
        PeerIngressBindRefusal::Denied
    );
    host.answer(owner_grant());
    assert_eq!(
        binder
            .bind_peer_ingress(governed_request("delivery-1"))
            .await
            .expect_err("still refused"),
        PeerIngressBindRefusal::Denied,
        "the refusal is recorded"
    );
}

/// The host's custody of originals is first-writer-wins. A binder holding no
/// record of a delivery (a second binder over the same host, as after its own
/// record was dropped) mints from the host's current answer; an association
/// that differs from the one the host enrolled first is refused
/// `enrollment_conflict`, never enrolled over the first, and the refusal is
/// recorded and answered again without another host read.
#[tokio::test]
async fn a_differing_mint_after_custody_loss_is_refused_enrollment_conflict() {
    let host = ScriptedHost::new(owner_grant());
    let request = governed_request("delivery-1");
    let delivery_work = request.delivery_work.clone();
    let first = bind(&binder(&host), request).await.expect("first bind");
    host.answer(Answer::Grant {
        basis: "reissued-owner-delegation",
        controller: "receiver",
    });
    let without_record = binder(&host);
    for attempt in 0..2 {
        assert_eq!(
            without_record
                .bind_peer_ingress(governed_request("delivery-1"))
                .await
                .expect_err("a differing association is refused"),
            PeerIngressBindRefusal::Refused(PeerIngressBindDenial::EnrollmentConflict),
            "attempt {attempt}"
        );
    }
    assert_eq!(
        host.reads(),
        2,
        "the recorded refusal answers the second attempt"
    );
    assert!(
        host.enrolled(&delivery_work).as_ref() == Some(&first),
        "the first enrollment stands"
    );
}

/// A bind dropped while the host's read is in flight leaves nothing minted,
/// enrolled or recorded: the next bind of the delivery is answered as if it
/// were the first.
#[tokio::test]
async fn dropped_bind_leaves_custody_consistent() {
    let host = ScriptedHost::new(Answer::Held);
    let binder = binder(&host);
    let request = governed_request("delivery-1");
    let delivery_work = request.delivery_work.clone();
    {
        let pending = binder.bind_peer_ingress(request);
        tokio::pin!(pending);
        tokio::select! {
            _ = &mut pending => panic!("the held read does not answer"),
            () = host.entered.notified() => {}
        }
        // Dropped here, inside `basis_for`.
    }
    assert_eq!(host.enrollments.load(Ordering::SeqCst), 0);
    assert!(host.enrolled(&delivery_work).is_none());
    host.answer(owner_grant());
    assert!(bind(&binder, governed_request("delivery-1")).await.is_ok());
    assert_eq!(host.enrollments.load(Ordering::SeqCst), 1);
}

/// A bind that finds a record made while its own host read was in flight
/// binds that record verbatim, not its own answer.
#[tokio::test]
async fn a_record_made_during_the_host_read_wins_verbatim() {
    let host = ScriptedHost::new(Answer::Held);
    let binder = Arc::new(binder(&host));
    let held = {
        let binder = Arc::clone(&binder);
        tokio::spawn(async move { bind(&binder, governed_request("delivery-1")).await })
    };
    host.entered.notified().await;
    host.answer(Answer::Grant {
        basis: "concurrent-owner-delegation",
        controller: "receiver",
    });
    let concurrent = bind(&binder, governed_request("delivery-1"))
        .await
        .expect("the concurrent bind");
    host.release.notify_one();
    let late = held.await.expect("task").expect("the held bind");
    assert!(
        late == concurrent,
        "the bind that read first binds the record the other made meanwhile"
    );
}

/// A supervisor-bridge member delivery is not this binder's: without a
/// supervisor binder it answers `NotOffered`, and with one it hands the
/// delivery to it.
#[tokio::test]
async fn a_supervisor_delivery_goes_to_the_hosts_supervisor_binder() {
    struct Supervisor(AtomicUsize);
    impl PeerIngressBinder for Supervisor {
        fn bind_peer_ingress(&self, _: PeerIngressBindRequest) -> PeerIngressBindFuture<'_> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(PeerIngressBindRefusal::Denied) })
        }
    }
    let supervisor_delivery = || {
        request(
            PeerIngressOrigin::SupervisorDelivery(
                crate::input_authority::PeerIngressSupervisorOrigin {
                    supervisor: sender_peer(),
                    sender_authentication: PeerIngressSenderAuthentication::SignatureVerified,
                    interaction_id: InteractionId(uuid::Uuid::from_u128(8)),
                    tracked: true,
                },
            ),
            "bridge-delivery",
        )
    };
    let host = ScriptedHost::new(owner_grant());
    assert_eq!(
        binder(&host)
            .bind_peer_ingress(supervisor_delivery())
            .await
            .expect_err("not offered"),
        PeerIngressBindRefusal::NotOffered
    );
    let supervisor = Arc::new(Supervisor(AtomicUsize::new(0)));
    let composed =
        binder(&host).with_supervisor_binder(Arc::clone(&supervisor) as Arc<dyn PeerIngressBinder>);
    assert_eq!(
        composed
            .bind_peer_ingress(supervisor_delivery())
            .await
            .expect_err("the supervisor binder's answer"),
        PeerIngressBindRefusal::Denied
    );
    assert_eq!(supervisor.0.load(Ordering::SeqCst), 1);
    assert_eq!(host.reads(), 0);
}

/// Core refuses any binder's binding of a delivery with a governed origin
/// that does not keep the origin's requester, does not name the sending
/// agent as its ingress actor, or drops a work of the sending work.
#[tokio::test]
async fn core_refuses_a_binding_that_drops_the_governed_requester_or_its_works() {
    let host = ScriptedHost::new(owner_grant());
    let request = governed_request("delivery-1");
    let handed = request.input.clone();
    let runtime = request.runtime_id.clone();
    let work_id = request.delivery_work.clone();
    let origin = origin();
    let (bound, _) = binder(&host)
        .bind_peer_ingress(request)
        .await
        .expect("bound")
        .into_parts();
    let pin = BoundOriginalPin::GovernedDelivery {
        work: &work_id,
        origin: &origin,
    };
    check_bound_original(&handed, &bound, &runtime, pin).expect("the binder's own binding");
    let claims = bound
        .header()
        .authority_association
        .as_ref()
        .expect("association")
        .candidate()
        .clone();
    let rebound = |change: &dyn Fn(&mut InputAuthorityAssociationCandidate)| {
        let mut candidate = claims.clone();
        change(&mut candidate);
        let ingress = bound.header().ingress_context.as_ref().expect("ingress");
        super::super::bind_original_input(
            handed.clone(),
            InputAuthorityAssociation::new(candidate.clone()).expect("claims"),
            bound.header().idempotency_key.clone().expect("key"),
            candidate.requester.clone(),
            candidate.ingress_actor.clone(),
            realm(),
            ingress.authentication().clone(),
            controller("receiver"),
        )
        .expect("bound input")
    };
    for (label, change) in [
        (
            "another requester",
            Box::new(|candidate: &mut InputAuthorityAssociationCandidate| {
                candidate.requester = principal("peer-host");
            }) as Box<dyn Fn(&mut InputAuthorityAssociationCandidate)>,
        ),
        (
            "another ingress actor",
            Box::new(|candidate: &mut InputAuthorityAssociationCandidate| {
                candidate.ingress_actor = principal("requester");
            }),
        ),
        (
            "a dropped work of the sending work",
            Box::new(|candidate: &mut InputAuthorityAssociationCandidate| {
                candidate
                    .contributing_work
                    .retain(|named| named != &work("source-b"));
            }),
        ),
    ] {
        let malformed = rebound(change.as_ref());
        assert!(
            check_bound_original(&handed, &malformed, &runtime, pin).is_err(),
            "{label} is a malformed binding of a governed delivery"
        );
        assert!(
            check_bound_original(
                &handed,
                &malformed,
                &runtime,
                BoundOriginalPin::DeliveryWork(&work_id)
            )
            .is_ok(),
            "{label}: only the governed pin refuses it"
        );
    }
}
