//! Peers keep the original requester on the actual native owner: a peer work
//! that Core's binder binds names every work of the sending work as its
//! contributors, in the namespace where they resolve, so every operation of
//! it is narrowed by the requester's own current grant, at every hop. The
//! receiving agent's own grant never widens what the requester may.
use super::*;
use crate::input_authority::{
    GovernedPeerOrigin, OriginalRequesterPeerBinder, PeerIngressBindRequest, PeerIngressBinder,
    PeerIngressClaimKind, PeerIngressCommsOrigin, PeerIngressOrigin, PeerServiceBasis,
    PeerServiceFuture, PeerServiceGrant, PeerServiceRefusal, PeerServiceRequest,
};
use crate::meerkat_machine::{
    AuthenticatedOriginal, AuthenticatedOriginalSource, RetainedCustody, ScopedGrantComposition,
    ScopedInvocation, ScopedMandateOwner,
};
use meerkat_authorization::grants::LocalGrantAuthority;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, NativeWorkTarget, OriginalWorkRef, QualifiedIngressNamespace,
    WorkAuthorityBasis,
};
use std::collections::HashMap;
use std::sync::Mutex;

/// The source owner's custody of the originals it holds admitted, by work:
/// the requester's own originals and every peer original Core's binder
/// enrolled, first-writer-wins.
#[derive(Default)]
struct Custody(Mutex<HashMap<EvidenceId, AuthenticatedOriginal>>);

impl Custody {
    fn original_of(association: &InputAuthorityAssociation) -> AuthenticatedOriginal {
        let claims = association.candidate();
        AuthenticatedOriginal {
            original_work: claims.original_work.clone(),
            occurrence_scope: claims.ingress_namespace.occurrence_scope.clone(),
            original_authentication: claims.original_authentication.clone(),
            root_event: claims.root_event.clone(),
            contributing_work: claims.contributing_work.clone(),
            target: claims.target.clone(),
            authority_basis: claims.authority_basis.clone(),
        }
    }

    /// Hold `association`'s original, unless another is held for its work.
    fn hold(&self, association: &InputAuthorityAssociation) -> Result<(), PeerServiceRefusal> {
        let original = Self::original_of(association);
        let mut held = self.0.lock().expect("custody");
        match held.get(&original.original_work.work) {
            Some(first) if first != &original => Err(PeerServiceRefusal::EnrollmentConflict),
            Some(_) => Ok(()),
            None => {
                held.insert(original.original_work.work.clone(), original);
                Ok(())
            }
        }
    }
}

impl AuthenticatedOriginalSource for Custody {
    fn authenticated_original(
        &self,
        _ingress: &NativeIngressContext,
        _namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<
        Option<AuthenticatedOriginal>,
        meerkat_core::authorization::ControllerReadinessFailure,
    > {
        Ok(self.0.lock().expect("custody").get(&original.work).cloned())
    }

    fn retained_custody(
        &self,
        _namespace: &QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<RetainedCustody, meerkat_core::authorization::ControllerReadinessFailure> {
        Ok(self
            .0
            .lock()
            .expect("custody")
            .get(&original.work)
            .map_or(RetainedCustody::NotRetained, |held| {
                RetainedCustody::Retained(held.authority_basis.clone())
            }))
    }
}

/// The host's invocation policy: the requester may invoke any of the
/// fixture's agents.
struct Policy {
    requester: PrincipalRef,
    agents: Vec<PrincipalRef>,
}

impl Policy {
    fn allows(&self, association: &InputAuthorityAssociation) -> bool {
        let claims = association.candidate();
        claims.requester == self.requester
            && self.agents.contains(&claims.logical_executor)
            && claims.represented_subject.is_none()
    }
}

impl ScopedMandateOwner for Policy {
    fn authorize_invocation(
        &self,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        if ingress.requester() != &self.requester || !self.allows(association) {
            return Err(NativeAdmissionError::Refused(denied()));
        }
        Ok(())
    }
}

impl AdmittedWorkPolicyOwner for Policy {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        _: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        if !self.allows(association) {
            return Err(denied().into());
        }
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 10_000,
        })
    }
}

/// The fixture's operation owner for every agent: the agent's controller
/// inference, and its reads of the fixture's source records.
struct Agents {
    domain: ResourceDomain,
    agents: Vec<PrincipalRef>,
}

impl Agents {
    fn values(&self, action: &str, agent: &PrincipalRef) -> LocalOperationValues {
        LocalOperationValues {
            action: ActionRef {
                feature: "native-test".into(),
                action: action.into(),
            },
            resource_domain: self.domain.clone(),
            processor: ProcessorRef::Principal {
                principal: agent.clone(),
            },
            audience: AudienceRef::Principal {
                principal: agent.clone(),
            },
        }
    }
}

impl OperationPolicyOwner for Agents {
    fn review_action_account(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: u64,
    ) -> Result<Option<String>, meerkat_core::OperationAuthorizationError> {
        Ok(None)
    }

    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        _: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        let agent = &association.candidate().logical_executor;
        if !self.agents.contains(agent)
            || association.candidate().controller_model.as_ref() != Some(facts.selection())
            || facts.endpoint() != "https://controller.invalid/model"
        {
            return Err(denied().into());
        }
        Ok(
            meerkat_authorization::grant_policy::ControllerAdmissionAllowance {
                operation_values: vec![self.values("infer", agent)],
                restrictions: ExecutionRestrictions::unrestricted(),
            },
        )
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        _: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let agent = &association.candidate().logical_executor;
        let AuthorizationOperation::Source(facts) = &binding.facts().operation else {
            return Err(denied().into());
        };
        let SourceAuthorizationTarget::External(target) = &facts.target else {
            return Err(denied().into());
        };
        if purpose != LocalPolicyPurpose::Operation
            || facts.usage != SourceAuthorizationUse::Read
            || !self.agents.contains(agent)
            || target.authority != self.domain.authority
            || target.namespace.as_ref() != self.domain.namespace
        {
            return Err(denied().into());
        }
        Ok(LocalPolicyAllowance {
            operation_values: vec![self.values("read", agent)],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 10_000,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
}

/// Every receiving agent's side for Core's binder: the agent each session
/// runs, the grant it serves peers under, its controller grant, and the one
/// selected controller; enrollment into the source owner's custody.
struct Receivers {
    custody: Arc<Custody>,
    selection: ControllerModelSelection,
    by_session: Mutex<HashMap<SessionId, Receiver>>,
}

#[derive(Clone)]
struct Receiver {
    agent: PrincipalRef,
    basis: GrantLineageRef,
    controller: GrantLineageRef,
}

impl PeerServiceBasis for Receivers {
    fn basis_for<'a>(&'a self, request: &'a PeerServiceRequest) -> PeerServiceFuture<'a> {
        let receiver = self
            .by_session
            .lock()
            .expect("receivers")
            .get(&request.session_id)
            .cloned();
        Box::pin(async move {
            let receiver = receiver.ok_or(PeerServiceRefusal::NoBasis)?;
            Ok(PeerServiceGrant {
                receiver: receiver.agent,
                represented_subject: None,
                authority_basis: WorkAuthorityBasis::GrantLineage {
                    lineage: vec![receiver.basis],
                },
                target: NativeWorkTarget {
                    logical_owner: principal("native-owner"),
                    logical_runtime: EvidenceId::new(request.runtime_id.to_string())
                        .expect("runtime"),
                    context: EvidenceId::new("context").expect("context"),
                    context_generation: 1,
                    audience: AudienceRef::Principal {
                        principal: principal("audience"),
                    },
                },
                controller_grant_lineage: vec![receiver.controller],
                controller_ceiling: ExecutionRestrictions::unrestricted(),
                admitted_ceiling: ExecutionRestrictions::unrestricted(),
                controller: ControllerModelClient::new(
                    self.selection.clone(),
                    Arc::new(SelectedClient(self.selection.clone())),
                ),
            })
        })
    }

    fn enroll(
        &self,
        _request: &PeerServiceRequest,
        association: &InputAuthorityAssociation,
    ) -> Result<
        Option<Box<dyn crate::input_admission_custody::NativeInputAdmissionCustody>>,
        PeerServiceRefusal,
    > {
        self.custody.hold(association).map(|()| None)
    }
}

fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        meerkat_core::PrincipalKind::ServiceAccount,
        value,
        meerkat_core::TrustDomainId::new("native-test").expect("domain"),
    )
    .expect("qualified")
}

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("id")
}

/// The fixture's agents: the requester's own agent, which sends, and two
/// agents that serve peers.
fn agents(sender: &PrincipalRef) -> Vec<PrincipalRef> {
    vec![
        sender.clone(),
        principal("owner-agent"),
        principal("third-agent"),
    ]
}

/// One governed machine on the scoped composition, with Core's binder over
/// its agents' receiving sides installed, and the requester's original
/// (`original-a`, under `requester_grant`) held by the source owner.
struct Peers {
    machine: MeerkatMachine,
    grants: Arc<LocalGrantAuthority>,
    root: PrincipalRef,
    custody: Arc<Custody>,
    receivers: Arc<Receivers>,
    binder: Arc<OriginalRequesterPeerBinder>,
    domain: ResourceDomain,
    sender: InputAuthorityAssociation,
}

impl Peers {
    async fn new(requester_bounds: ExecutionRestrictions) -> Self {
        let (configuration, prompt, domain) = configuration();
        let template = prompt
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate()
            .clone();
        let grants = Arc::clone(&configuration.grants);
        let root = template.controller_grant_lineage[0].root_authority.clone();
        let selection = template
            .controller_model
            .clone()
            .expect("selected controller");
        let sender_agent = template.logical_executor.clone();
        let custody = Arc::new(Custody::default());
        let policy = Arc::new(Policy {
            requester: template.requester.clone(),
            agents: agents(&sender_agent),
        });
        let composed = ScopedGrantComposition {
            grants: Arc::clone(&grants),
            ingress_domain: template.ingress_namespace.ingress.clone(),
            originals: custody.clone(),
            invocation: ScopedInvocation::from_policy(policy),
            operation_owner: Arc::new(Agents {
                domain: domain.clone(),
                agents: agents(&sender_agent),
            }),
            retained_resume: None,
        }
        .into_configuration();
        let mut composed = composed;
        // These rows bind no scoped session document; destination currency
        // is the scoped context binding's.
        composed.require_scoped_destination = false;
        let machine = MeerkatMachine::ephemeral()
            .with_local_grant_authorization(composed)
            .expect("exclusive setup before Weak construction");
        let receivers = Arc::new(Receivers {
            custody: Arc::clone(&custody),
            selection: selection.clone(),
            by_session: Mutex::new(HashMap::new()),
        });
        let binder = Arc::new(OriginalRequesterPeerBinder::new(
            Arc::clone(&receivers) as Arc<dyn PeerServiceBasis>
        ));
        machine
            .install_peer_ingress_binder(Arc::clone(&binder) as Arc<dyn PeerIngressBinder>)
            .expect("binder installs before any session");
        meerkat_core::publish_token_lifecycle_acquired_for_identity(
            &machine.generated_auth_lease_handle(),
            selection.credential(),
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-peer-owner-fixture"),
        )
        .expect("actual initial credential owner");

        // The requester's own original on the sending agent, as the source
        // owner holds it admitted: the requester's delegation as its basis.
        let requester_grant = grants
            .issue_root(
                &root,
                id("requester-delegation"),
                sender_agent.clone(),
                None,
                requester_bounds,
            )
            .expect("the requester's delegation");
        let mut sender = template.clone();
        sender.original_work.work = id("original-a");
        sender.authority_basis = WorkAuthorityBasis::GrantLineage {
            lineage: vec![requester_grant],
        };
        let sender = InputAuthorityAssociation::new(sender).expect("the sending work");
        custody
            .hold(&sender)
            .expect("the requester's original is held");
        Self {
            machine,
            grants,
            root,
            custody,
            receivers,
            binder,
            domain,
            sender,
        }
    }

    /// A registered session the receiving `agent` runs, serving peers under
    /// a grant of `bounds`.
    async fn receiver(&self, agent: &str, bounds: ExecutionRestrictions) -> SessionId {
        let session = SessionId::new();
        self.machine
            .register_session(session.clone())
            .await
            .expect("actual registration");
        let issue = |name: String, bounds| {
            self.grants
                .issue_root(&self.root, id(&name), principal(agent), None, bounds)
                .expect("the receiving agent's grant")
        };
        let receiver = Receiver {
            agent: principal(agent),
            basis: issue(format!("{agent}-serves-peers"), bounds),
            controller: issue(
                format!("{agent}-controller"),
                ExecutionRestrictions::unrestricted(),
            ),
        };
        self.receivers
            .by_session
            .lock()
            .expect("receivers")
            .insert(session.clone(), receiver);
        session
    }

    /// Deliver to `session`, in process, under the governed `origin`, bind
    /// it with Core's binder, admit it natively and stage its run: the
    /// receiving work's association and run context.
    async fn deliver(
        &self,
        session: &SessionId,
        origin: GovernedPeerOrigin,
        event: &str,
    ) -> (
        InputAuthorityAssociation,
        meerkat_core::RunId,
        WorkAuthorizationContext,
    ) {
        let runtime = {
            let sessions = self.machine.sessions.read().await;
            sessions.get(session).expect("entry").runtime_id.clone()
        };
        let mut handed = Input::Prompt(crate::input::PromptInput::new(
            "please answer for my requester",
            None,
        ));
        handed.header_mut().idempotency_key = Some(crate::identifiers::IdempotencyKey::new(event));
        let request = PeerIngressBindRequest::new(
            runtime,
            session.clone(),
            None,
            PeerIngressOrigin::Comms(PeerIngressCommsOrigin {
                sender: meerkat_core::comms::PeerId::new(),
                signing_key: None,
                route_label: None,
                sender_authentication: meerkat_core::PeerIngressSenderAuthentication::InProcess,
                auth: meerkat_core::PeerIngressAuthDecision::Required,
                class: meerkat_core::PeerInputClass::ActionableMessage,
                kind: meerkat_core::PeerIngressKind::Message,
                interaction_id: meerkat_core::InteractionId(uuid::Uuid::new_v4()),
                in_reply_to: None,
                claim: PeerIngressClaimKind::Volatile,
                governed_origin: Some(origin),
            }),
            crate::identifiers::IdempotencyKey::new(event),
            handed,
        )
        .expect("Core's request");
        let (bound, _) = self
            .binder
            .bind_peer_ingress(request)
            .await
            .expect("Core's binder binds the delivery")
            .into_parts();
        let association = bound
            .header()
            .authority_association
            .clone()
            .expect("bound association");
        let id = bound.id().clone();
        let driver = Arc::clone(
            &self
                .machine
                .sessions
                .read()
                .await
                .get(session)
                .expect("entry")
                .driver,
        );
        let run = meerkat_core::RunId::new();
        let context = {
            let mut locked = driver.lock().await;
            let DriverEntry::Ephemeral(driver) = &mut *locked else {
                panic!("storeless")
            };
            driver.set_executor_work_authorization_support(true);
            driver
                .accept_input(bound)
                .await
                .expect("the scoped composition admits the receiving work");
            let context = driver
                .batch_work_authorization(&run, std::slice::from_ref(&id))
                .expect("structural batch")
                .expect("governed context");
            driver
                .contract_begin_run_authority(run.clone())
                .expect("actual run");
            driver
                .machine_realize_authorized_stage_batch(
                    crate::meerkat_machine::driver::test_authorized_stage_for_run(
                        vec![id.clone()],
                        run.clone(),
                    ),
                )
                .expect("stage the receiving work");
            context
        };
        (association, run, context)
    }

    /// The origin Core minted for the requester's own work.
    fn sender_origin(&self) -> GovernedPeerOrigin {
        GovernedPeerOrigin::from_associations([&self.sender]).expect("the sending work's origin")
    }

    /// Whether a run's read of the fixture's source record is allowed now.
    fn reads(
        &self,
        context: &WorkAuthorizationContext,
        run: &meerkat_core::RunId,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        context
            .authorization()
            .prepare(&binding(context, run, &self.domain))
            .map(|_| ())
    }
}

/// Bounds that allow only `action` on the fixture's source.
fn only(action: &str) -> ExecutionRestrictions {
    let mut bounds = ExecutionRestrictions::unrestricted();
    bounds.actions =
        meerkat_authorization_contracts::constraints::ExactRestriction::exact([ActionRef {
            feature: "native-test".into(),
            action: action.into(),
        }]);
    bounds
}

fn denied_kind(
    result: Result<(), meerkat_core::OperationAuthorizationError>,
) -> Option<OperationRefusalKind> {
    match result {
        Err(meerkat_core::OperationAuthorizationError::Refused(refusal)) => Some(refusal.kind()),
        _ => None,
    }
}

/// The receiving agent runs an operation that both the requester's
/// delegation and its own peer basis allow: the receiving work's
/// contributors resolve where Core bound it, so the requester's ceiling is
/// read, not missing.
#[tokio::test]
async fn the_receiver_runs_an_operation_both_the_requester_and_its_peer_basis_allow() {
    let peers = Peers::new(ExecutionRestrictions::unrestricted()).await;
    let owner = peers
        .receiver("owner-agent", ExecutionRestrictions::unrestricted())
        .await;
    let (association, run, context) = peers
        .deliver(&owner, peers.sender_origin(), "ask-owner")
        .await;
    let claims = association.candidate();
    assert_eq!(claims.requester, peers.sender.candidate().requester);
    assert!(
        claims.contributing_work == vec![peers.sender.candidate().original_work.clone()],
        "the receiving work names the requester's work"
    );
    assert!(
        claims.ingress_namespace.ingress == peers.sender.candidate().ingress_namespace.ingress
            && claims.ingress_namespace.realm == peers.sender.candidate().ingress_namespace.realm,
        "bound in the sending work's realm and ingress domain"
    );
    peers
        .reads(&context, &run)
        .expect("both the requester's delegation and the agent's own basis allow the read");
}

/// The receiving agent cannot run what the requester may not: its own peer
/// basis allows the read, the requester's delegation does not, and the read
/// is denied.
#[tokio::test]
async fn the_receiver_cannot_run_what_the_requester_may_not() {
    let peers = Peers::new(only("write")).await;
    let owner = peers
        .receiver("owner-agent", ExecutionRestrictions::unrestricted())
        .await;
    let (_, run, context) = peers
        .deliver(&owner, peers.sender_origin(), "ask-owner")
        .await;
    assert_eq!(
        denied_kind(peers.reads(&context, &run)),
        Some(OperationRefusalKind::Denied),
        "the requester's ceiling narrows the receiving agent's own access"
    );
}

/// Two hops: the requester's agent asks the owner's agent, whose peer work
/// asks a third agent. The third agent's work names the owner's agent's
/// peer work and, through it, the requester's own work, so it cannot exceed
/// the requester's delegation even though both agents' own grants allow the
/// read. The second hop's origin is the one Core minted for the owner's
/// agent's staged run.
#[tokio::test]
async fn a_two_hop_peer_cannot_exceed_the_original_requesters_delegation() {
    // The narrower delegation first: the read's outcome is checked before
    // the claims that explain it, so a binding that drops the requester's
    // work fails on the escalation itself.
    for (requester_bounds, allowed) in [
        (only("write"), false),
        (ExecutionRestrictions::unrestricted(), true),
    ] {
        let peers = Peers::new(requester_bounds).await;
        let owner = peers
            .receiver("owner-agent", ExecutionRestrictions::unrestricted())
            .await;
        let third = peers
            .receiver("third-agent", ExecutionRestrictions::unrestricted())
            .await;
        let (first_hop, _, owner_context) = peers
            .deliver(&owner, peers.sender_origin(), "ask-owner")
            .await;
        let minted = owner_context
            .governed_send_origin()
            .and_then(GovernedPeerOrigin::from_send_origin)
            .expect("Core attached the staged run's origin");
        assert_eq!(minted.requester(), &peers.sender.candidate().requester);
        assert_eq!(minted.sender_executor(), &principal("owner-agent"));
        let (second_hop, run, context) = peers.deliver(&third, minted, "ask-third").await;
        let read = peers.reads(&context, &run);
        if allowed {
            read.expect("every hop allows the read");
        } else {
            assert_eq!(
                denied_kind(read),
                Some(OperationRefusalKind::Denied),
                "the third agent is held to the original requester's delegation"
            );
        }
        let claims = second_hop.candidate();
        assert_eq!(claims.requester, peers.sender.candidate().requester);
        assert_eq!(claims.ingress_actor, principal("owner-agent"));
        assert!(
            claims.contributing_work
                == vec![
                    first_hop.candidate().original_work.clone(),
                    peers.sender.candidate().original_work.clone(),
                ],
            "the second hop names the first hop's work and the requester's own"
        );
    }
}
