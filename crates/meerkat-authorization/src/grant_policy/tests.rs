//! Real generated grant owner plus real work compiler. The native/resource
//! owners here are explicitly test fixtures with their own admitted record and
//! correlated access relation; they are not production ingress implementations.
#![allow(clippy::expect_used)]

mod audit;
mod tool_application;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, DelegationDepth, ExactRestriction, LifetimeRestriction, ProcessorRef,
    ResourceDomain, UnresolvedConstraint,
};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, InputAuthorityAssociationCandidate, NativeWorkTarget, OriginalWorkRef,
    QualifiedIngressNamespace,
};
use meerkat_core::authorization::{
    ModelAuthorizationFacts, OperationAuthorizationFacts, OwnerQualifiedTarget,
    SourceAuthorizationFacts, SourceAuthorizationTarget, SourceAuthorizationUse,
};
use meerkat_core::connection::{AuthCredentialIdentity, RealmId};
use meerkat_core::time_compat::Instant;
use meerkat_core::{
    ControllerModelSelection, PrincipalKind, PrincipalRef, Provider, ServerToolKind, SessionId,
    SessionLlmIdentity, TrustDomainId,
};

use super::*;
use crate::grants::LocalGrantConfiguration;
use crate::policy::{ExactOperationRelation, LocalOperationValues};
use crate::publication::LocalAuthorizationPublication;
use crate::work::{LocalAuthorizationClock, LocalAuthorizationTime};

fn who(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        TrustDomainId::new("grant-policy-fixture").expect("domain"),
    )
    .expect("principal")
}
fn id(name: &str) -> EvidenceId {
    EvidenceId::new(name).expect("id")
}
fn domain(name: &str) -> ResourceDomain {
    ResourceDomain {
        authority: who("resource"),
        namespace: name.into(),
    }
}
fn evidence(name: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: domain("evidence"),
            resource_id: name.into(),
        },
        revision: id("revision"),
        digest: EvidenceDigest::from_array([17; 32]),
    }
}
fn action(name: &str) -> ActionRef {
    ActionRef {
        feature: "fixture".into(),
        action: name.into(),
    }
}
fn tuple(name: &str, namespace: &str) -> LocalOperationValues {
    LocalOperationValues {
        action: action(name),
        resource_domain: domain(namespace),
        processor: ProcessorRef::Principal {
            principal: who("executor"),
        },
        audience: AudienceRef::Principal {
            principal: who("requester"),
        },
    }
}
fn scope() -> OperationExecutionScope {
    OperationExecutionScope::RuntimeInput {
        owner_session_id: SessionId::parse("00000000-0000-0000-0000-000000000001")
            .expect("session"),
        runtime_epoch_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"")
            .expect("epoch"),
        submitted_input_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000003\"")
            .expect("input"),
        canonical_input_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000003\"")
            .expect("input"),
    }
}
fn bind(operation: AuthorizationOperation) -> PreparedAuthorizationBinding {
    PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: serde_json::from_str("\"00000000-0000-0000-0000-000000000004\"")
            .expect("operation"),
        execution_scope: scope(),
        run_id: None,
        context_revision: None,
        operation,
    })
}
fn source(usage: SourceAuthorizationUse, namespace: &str) -> PreparedAuthorizationBinding {
    bind(AuthorizationOperation::Source(SourceAuthorizationFacts {
        usage,
        target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
            authority: who("resource"),
            namespace: namespace.into(),
            id: "record".into(),
        }),
    }))
}
fn model(usage: ModelAuthorizationUse, hosted: Vec<ServerToolKind>) -> ModelAuthorizationFacts {
    let credential: AuthCredentialIdentity =
        serde_json::from_str(r#"{"realm":"fixture","account":"controller-account"}"#)
            .expect("credential identity");
    ModelAuthorizationFacts {
        identity: Arc::new(SessionLlmIdentity {
            model: "controller-model".into(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            auth_binding: None,
            provider_params: None,
        }),
        wire_model: "controller-wire-model".into(),
        hosted_capabilities: hosted.into(),
        backend_profile_id: Some("fixture-profile".into()),
        backend_kind: "fixture-backend".into(),
        endpoint: "https://fixture.invalid/v1/responses".into(),
        credential: Some(credential),
        usage,
        live_channel: None,
    }
}
fn selection() -> ControllerModelSelection {
    let facts = model(ModelAuthorizationUse::ControllerInference, vec![]);
    ControllerModelSelection::new(
        facts.identity.as_ref().clone(),
        facts.credential.expect("credential"),
        "fixture-profile".into(),
        "fixture-backend".into(),
    )
}
fn bounded(actions: &[&str], depth: u32) -> ExecutionRestrictions {
    ExecutionRestrictions {
        actions: ExactRestriction::exact(actions.iter().map(|name| action(name))),
        lifetime: LifetimeRestriction::window(100, 300),
        delegation_depth: DelegationDepth::remaining(depth),
        ..ExecutionRestrictions::unrestricted()
    }
}
struct Clock(AtomicU64, AtomicBool);
impl LocalAuthorizationClock for Clock {
    fn now(&self) -> Result<LocalAuthorizationTime, crate::clock::LocalClockError> {
        if self.1.load(Ordering::SeqCst) {
            return Err(crate::clock::LocalClockError::Unavailable);
        }
        Ok(LocalAuthorizationTime {
            unix_ms: self.0.load(Ordering::SeqCst),
            monotonic: Instant::now(),
        })
    }
}
struct Admitted {
    original: InputAuthorityAssociation,
    requester_allowed: AtomicBool,
    ordinary_allowed: AtomicBool,
    controller_allowed: AtomicBool,
    calls: Mutex<Vec<LocalPolicyPurpose>>,
}
impl AdmittedWorkPolicyOwner for Admitted {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        _now: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        self.calls.lock().expect("calls").push(purpose);
        if association != &self.original
            || binding.facts().execution_scope != scope()
            || !self.requester_allowed.load(Ordering::SeqCst)
            || !(match purpose {
                LocalPolicyPurpose::Controller => self.controller_allowed.load(Ordering::SeqCst),
                LocalPolicyPurpose::Operation => self.ordinary_allowed.load(Ordering::SeqCst),
            })
        {
            return Err(denied().into());
        }
        // Test-only explicit host/mandate ownership: an arbitrary replacement
        // basis or occurrence fails the whole original equality above.
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 1_000,
        })
    }
}
struct Resources {
    relation: ExactOperationRelation,
    unknown: AtomicBool,
    /// Owner-resolved review tier, changed only under the publication.
    review_tier: Mutex<meerkat_core::authorization::OperationReviewTier>,
}
impl OperationPolicyOwner for Resources {
    fn authorize_operation(
        &self,
        _association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        _now: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        let mut actual = vec![];
        match &binding.facts().operation {
            AuthorizationOperation::Source(facts) => {
                let SourceAuthorizationTarget::External(target) = &facts.target else {
                    return Err(denied().into());
                };
                if target.authority != who("resource") || target.id.as_ref() != "record" {
                    return Err(denied().into());
                }
                actual.push(tuple(
                    match facts.usage {
                        SourceAuthorizationUse::Read | SourceAuthorizationUse::Hydrate => "read",
                        SourceAuthorizationUse::Retain => "write",
                    },
                    &target.namespace,
                ));
            }
            AuthorizationOperation::Model(facts) => {
                if facts.endpoint.as_ref() != "https://fixture.invalid/v1/responses"
                    || facts.wire_model.as_ref() != "controller-wire-model"
                {
                    return Err(denied().into());
                }
                if purpose == LocalPolicyPurpose::Controller
                    || facts.usage != ModelAuthorizationUse::ControllerInference
                {
                    actual.push(tuple("infer", "models"));
                }
                if purpose == LocalPolicyPurpose::Operation {
                    for tool in facts.hosted_capabilities.iter() {
                        if !matches!(tool, ServerToolKind::WebSearch) {
                            return Err(denied().into());
                        }
                        actual.push(tuple("web_search", "web"));
                    }
                }
            }
            _ => return Err(denied().into()),
        }
        if actual.is_empty() || actual.iter().any(|values| !self.relation.contains(values)) {
            return Err(denied().into());
        }
        let restrictions = if self.unknown.load(Ordering::SeqCst) {
            ExecutionRestrictions {
                actions: ExactRestriction::unresolved(UnresolvedConstraint::Unavailable),
                ..ExecutionRestrictions::unrestricted()
            }
        } else {
            ExecutionRestrictions::unrestricted()
        };
        Ok(LocalPolicyAllowance {
            operation_values: actual,
            restrictions,
            expires_at_ms: 800,
            review_tier: *self.review_tier.lock().expect("review tier"),
        })
    }
}
struct Fixture {
    grants: Arc<LocalGrantAuthority>,
    publication: LocalAuthorizationPublication,
    clock: Arc<Clock>,
    lineage: Vec<GrantLineageRef>,
    controller: GrantLineageRef,
}
impl Fixture {
    fn new() -> Self {
        let publication = LocalAuthorizationPublication::new();
        let clock = Arc::new(Clock(AtomicU64::new(100), AtomicBool::new(false)));
        let grants = Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: who("root"),
                    namespace: id("grants"),
                    generation: 1,
                },
                publication.clone(),
                clock.clone(),
            )
            .expect("grant owner"),
        );
        let root = grants
            .issue_root(
                &who("root"),
                id("root-grant"),
                who("first"),
                Some(who("subject")),
                bounded(&["read", "write", "web_search"], 2),
            )
            .expect("root");
        let child = grants
            .issue_child(
                &who("first"),
                &root,
                id("child"),
                who("second"),
                bounded(&["read", "web_search"], 1),
            )
            .expect("child");
        let leaf = grants
            .issue_child(
                &who("second"),
                &child,
                id("leaf"),
                who("executor"),
                bounded(&["read", "web_search"], 0),
            )
            .expect("leaf");
        let controller = grants
            .issue_root(
                &who("root"),
                id("controller"),
                who("executor"),
                Some(who("subject")),
                ExecutionRestrictions {
                    actions: ExactRestriction::exact([action("infer")]),
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .expect("controller");
        Self {
            grants,
            publication,
            clock,
            lineage: vec![root, child, leaf],
            controller,
        }
    }
    fn association(&self) -> InputAuthorityAssociation {
        InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
            requester: who("requester"),
            ingress_actor: who("ingress"),
            represented_subject: Some(who("subject")),
            original_authentication: evidence("authentication"),
            logical_executor: who("executor"),
            target: NativeWorkTarget {
                logical_owner: who("session"),
                logical_runtime: id("runtime"),
                context: id("context"),
                context_generation: 1,
                audience: AudienceRef::Principal {
                    principal: who("requester"),
                },
            },
            original_work: OriginalWorkRef {
                authority: who("ingress"),
                work: id("work"),
            },
            root_event: evidence("event"),
            contributing_work: vec![],
            authority_basis: WorkAuthorityBasis::GrantLineage {
                lineage: self.lineage.clone(),
            },
            controller_grant_lineage: vec![self.controller.clone()],
            controller_model: Some(selection()),
            controller_ceiling: ExecutionRestrictions {
                actions: ExactRestriction::exact([action("infer")]),
                ..ExecutionRestrictions::unrestricted()
            },
            admitted_ceiling: bounded(&["read", "web_search"], 0),
            source_observations: vec![],
            ingress_namespace: QualifiedIngressNamespace {
                realm: RealmId::parse("fixture").expect("realm"),
                ingress: domain("ingress"),
                occurrence_scope: id("occurrence"),
            },
            contract: ContractRequirements::local_governed_v1(),
        })
        .expect("association")
    }
    fn policy(
        &self,
        association: InputAuthorityAssociation,
    ) -> (Arc<GrantBackedWorkPolicy>, Arc<Admitted>, Arc<Resources>) {
        let admitted = Arc::new(Admitted {
            original: association,
            requester_allowed: AtomicBool::new(true),
            ordinary_allowed: AtomicBool::new(true),
            controller_allowed: AtomicBool::new(true),
            calls: Mutex::new(vec![]),
        });
        let resources = Arc::new(Resources {
            relation: ExactOperationRelation::new(vec![
                tuple("read", "public"),
                tuple("write", "private"),
                tuple("infer", "models"),
                tuple("web_search", "web"),
            ]),
            unknown: AtomicBool::new(false),
            review_tier: Mutex::new(meerkat_core::authorization::OperationReviewTier::R1),
        });
        (
            Arc::new(GrantBackedWorkPolicy::new(
                self.grants.clone(),
                admitted.clone(),
                resources.clone(),
            )),
            admitted,
            resources,
        )
    }
    fn context(
        &self,
        policy: &Arc<GrantBackedWorkPolicy>,
        association: &InputAuthorityAssociation,
    ) -> WorkAuthorizationContext {
        policy
            .work_context(vec![association.clone()].into(), scope())
            .expect("context")
    }
}

#[test]
fn actual_full_chain_and_correlated_resource_rule_are_both_required() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let context = fixture.context(&policy, &association);
    assert!(
        context
            .authorization()
            .prepare(&source(SourceAuthorizationUse::Read, "public"))
            .is_ok()
    );
    // Each crossed pair is forbidden even though both coordinates occur in the relation.
    assert!(
        context
            .authorization()
            .prepare(&source(SourceAuthorizationUse::Read, "private"))
            .is_err()
    );
    assert!(
        context
            .authorization()
            .prepare(&source(SourceAuthorizationUse::Retain, "public"))
            .is_err()
    );
    // Resource owner allows this tuple, but the actual child grant removed write.
    assert!(
        context
            .authorization()
            .prepare(&source(SourceAuthorizationUse::Retain, "private"))
            .is_err()
    );
}

#[test]
fn held_grant_does_not_authorize_requester_or_another_admitted_scope() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, owner, _) = fixture.policy(association.clone());
    let context = fixture.context(&policy, &association);
    let binding = source(SourceAuthorizationUse::Read, "public");
    owner.requester_allowed.store(false, Ordering::SeqCst);
    assert!(context.authorization().prepare(&binding).is_err());
    owner.requester_allowed.store(true, Ordering::SeqCst);
    let mut wrong = binding.facts().clone();
    wrong.execution_scope = OperationExecutionScope::Domain;
    assert!(
        context
            .authorization()
            .prepare(&PreparedAuthorizationBinding::new(wrong))
            .is_err()
    );
    let mut forged = association.candidate().clone();
    forged.requester = who("other-requester");
    let forged = InputAuthorityAssociation::new(forged).expect("claim");
    assert!(
        fixture
            .context(&policy, &forged)
            .authorization()
            .prepare(&binding)
            .is_err()
    );
}

#[test]
fn admitted_claim_still_cannot_forge_generated_grant_lineage_executor_or_subject() {
    for variant in 0..4 {
        let fixture = Fixture::new();
        let mut candidate = fixture.association().candidate().clone();
        match variant {
            0 => {
                candidate.authority_basis = WorkAuthorityBasis::GrantLineage {
                    lineage: fixture.lineage[1..].to_vec(),
                };
            }
            1 => {
                candidate.logical_executor = who("other-executor");
            }
            2 => {
                candidate.represented_subject = Some(who("other-subject"));
            }
            _ => {
                let WorkAuthorityBasis::GrantLineage { lineage } = &mut candidate.authority_basis
                else {
                    unreachable!()
                };
                lineage[2].authority_generation += 1;
            }
        }
        let admitted = InputAuthorityAssociation::new(candidate).expect("well-shaped claim");
        let (policy, _, _) = fixture.policy(admitted.clone());
        assert!(
            fixture
                .context(&policy, &admitted)
                .authorization()
                .prepare(&source(SourceAuthorizationUse::Read, "public"))
                .is_err()
        );
    }
}

#[test]
fn factory_uses_actual_grant_publication_and_expiry() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let context = fixture.context(&policy, &association);
    let binding = source(SourceAuthorizationUse::Read, "public");
    let prepared = context
        .authorization()
        .prepare(&binding)
        .expect("before revocation");
    fixture
        .grants
        .revoke(
            &who("root"),
            &fixture.lineage[0],
            &mut crate::grants::IsolatedGrantTestCustody,
        )
        .expect("revoke ancestor");
    assert!(prepared.check_current(&binding).is_err());
    assert!(context.authorization().prepare(&binding).is_err());
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let context = fixture.context(&policy, &association);
    let prepared = context
        .authorization()
        .prepare(&binding)
        .expect("before expiry");
    fixture.clock.0.store(300, Ordering::SeqCst);
    assert!(prepared.check_current(&binding).is_err());
    assert!(context.authorization().prepare(&binding).is_err());
}

#[test]
fn controller_and_hosted_capability_use_independent_correlated_grants() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, owner, _) = fixture.policy(association.clone());
    let context = fixture.context(&policy, &association);
    let hosted = bind(AuthorizationOperation::Model(model(
        ModelAuthorizationUse::ControllerInference,
        vec![ServerToolKind::WebSearch],
    )));
    let prepared_hosted = context
        .authorization()
        .prepare(&hosted)
        .expect("both independent roles");
    assert_eq!(
        *owner.calls.lock().expect("calls"),
        vec![
            LocalPolicyPurpose::Controller,
            LocalPolicyPurpose::Operation
        ]
    );
    // Ordinary inference, compaction and live cannot borrow controller grants.
    for usage in [
        ModelAuthorizationUse::Inference,
        ModelAuthorizationUse::Compaction,
        ModelAuthorizationUse::Live,
    ] {
        let ordinary = bind(AuthorizationOperation::Model(model(usage, vec![])));
        assert!(context.authorization().prepare(&ordinary).is_err());
    }
    // Ordinary authority expires, but a bare controller can still return feedback.
    fixture.clock.0.store(300, Ordering::SeqCst);
    assert!(prepared_hosted.check_current(&hosted).is_err());
    owner.calls.lock().expect("calls").clear();
    let bare = bind(AuthorizationOperation::Model(model(
        ModelAuthorizationUse::ControllerInference,
        vec![],
    )));
    assert!(context.authorization().prepare(&bare).is_ok());
    assert_eq!(
        *owner.calls.lock().expect("calls"),
        vec![LocalPolicyPurpose::Controller]
    );
    assert!(context.authorization().prepare(&hosted).is_err());
}

#[test]
fn exact_controller_selection_and_actual_endpoint_are_independent_checks() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let context = fixture.context(&policy, &association);
    for changed in 0..4 {
        let mut facts = model(ModelAuthorizationUse::ControllerInference, vec![]);
        match changed {
            0 => facts.backend_profile_id = Some("other".into()),
            1 => facts.endpoint = "https://other.invalid/v1/responses".into(),
            2 => facts.credential = None,
            _ => facts.wire_model = "other-wire-model".into(),
        }
        assert!(
            context
                .authorization()
                .prepare(&bind(AuthorizationOperation::Model(facts)))
                .is_err()
        );
    }
    fixture
        .grants
        .revoke(
            &who("root"),
            &fixture.controller,
            &mut crate::grants::IsolatedGrantTestCustody,
        )
        .expect("test-only revocation");
    assert!(
        context
            .authorization()
            .prepare(&bind(AuthorizationOperation::Model(model(
                ModelAuthorizationUse::ControllerInference,
                vec![]
            ))))
            .is_err()
    );
}

#[test]
fn host_policy_and_service_mandate_require_actual_owner_and_unknown_resource_refuses() {
    for basis in [
        WorkAuthorityBasis::HostPolicy {
            policy: evidence("owned-host-policy"),
        },
        WorkAuthorityBasis::ServiceMandate {
            mandate: evidence("owned-mandate"),
            commissioning_actor: who("commissioner"),
            occurrence: id("occurrence"),
        },
    ] {
        let fixture = Fixture::new();
        let mut candidate = fixture.association().candidate().clone();
        candidate.authority_basis = basis;
        let association = InputAuthorityAssociation::new(candidate).expect("association");
        let (policy, owner, resources) = fixture.policy(association.clone());
        let context = fixture.context(&policy, &association);
        let binding = source(SourceAuthorizationUse::Read, "public");
        assert!(context.authorization().prepare(&binding).is_ok());
        {
            let _publication = fixture
                .publication
                .begin_owner_change()
                .expect("owner mutation");
            owner.ordinary_allowed.store(false, Ordering::SeqCst);
        }
        assert!(context.authorization().prepare(&binding).is_err());
        {
            let _publication = fixture
                .publication
                .begin_owner_change()
                .expect("owner mutation");
            owner.ordinary_allowed.store(true, Ordering::SeqCst);
            resources.unknown.store(true, Ordering::SeqCst);
        }
        assert!(context.authorization().prepare(&binding).is_err());
    }
}

#[allow(clippy::panic)] // A fabricated operation is a failing test oracle.
mod controller_admission;

#[test]
fn unavailable_grant_clock_is_not_an_authoritative_policy_denial() {
    let fixture = Fixture::new();
    let association = fixture.association();
    let (policy, _, _) = fixture.policy(association.clone());
    let allowed = source(SourceAuthorizationUse::Read, "public");
    policy
        .evaluate_for(&association, &allowed, LocalPolicyPurpose::Operation, 100)
        .expect("live generated grant and correlated owner permit the control");
    let denied_binding = source(SourceAuthorizationUse::Retain, "private");
    let denial: meerkat_core::OperationAuthorizationError = policy
        .evaluate_for(
            &association,
            &denied_binding,
            LocalPolicyPurpose::Operation,
            100,
        )
        .expect_err("real child grant excludes write");
    assert!(
        matches!(denial, meerkat_core::OperationAuthorizationError::Refused(refusal)
        if refusal.kind() == OperationRefusalKind::Denied)
    );

    fixture.clock.1.store(true, Ordering::SeqCst);
    assert!(
        matches!(
            fixture.grants.resolve_lineage(
                &fixture.lineage,
                &who("executor"),
                Some(&who("subject"))
            ),
            Err(GrantRefusal::Unavailable)
        ),
        "the actual generated grant owner reports unavailability"
    );
    let error: meerkat_core::OperationAuthorizationError = policy
        .evaluate_for(&association, &allowed, LocalPolicyPurpose::Operation, 100)
        .expect_err("unavailable clock cannot issue allowance");
    fixture.clock.1.store(false, Ordering::SeqCst);
    policy
        .evaluate_for(&association, &allowed, LocalPolicyPurpose::Operation, 100)
        .expect("the same owner and binding work when its clock recovers");
    let projected = meerkat_core::ToolError::from(error);
    assert_eq!(
        projected.error_code(),
        "operation_authorization_unavailable",
        "lack of an authoritative grant observation is not a permission denial"
    );
    assert!(
        !projected
            .to_transcript_content()
            .contains("operation_refused")
    );
}

#[test]
fn joined_work_compiler_rejects_a_previous_grant_owner_incarnation() {
    let original = Fixture::new();
    let old_association = original.association();
    let (old_policy, _, _) = original.policy(old_association.clone());
    let binding = source(SourceAuthorizationUse::Read, "public");
    let _old_prepared = original
        .context(&old_policy, &old_association)
        .authorization()
        .prepare(&binding)
        .expect("original owner allows the actual read");

    let restarted = Fixture::new();
    assert_ne!(
        original.lineage[0].authority_incarnation,
        restarted.lineage[0].authority_incarnation
    );
    assert_eq!(original.lineage[0].grant_id, restarted.lineage[0].grant_id);
    assert_eq!(
        original.lineage[0].authority_generation,
        restarted.lineage[0].authority_generation
    );
    // The configured application fixture accepts the exact old association.
    // Only the actual selected generated grant owner's incarnation rejects it.
    let (stale_policy, _, _) = restarted.policy(old_association.clone());
    assert!(matches!(restarted.context(&stale_policy, &old_association)
        .authorization().prepare(&binding),
        Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == OperationRefusalKind::Denied));
    let fresh_association = restarted.association();
    let (fresh_policy, _, _) = restarted.policy(fresh_association.clone());
    let _fresh_prepared = restarted
        .context(&fresh_policy, &fresh_association)
        .authorization()
        .prepare(&binding)
        .expect("fresh same-owner lineage allows the read");
}

#[test]
fn administrative_controller_custody_is_not_projected_as_operation_denial() {
    assert!(matches!(
        grant_refusal(GrantRefusal::ControllerInUse),
        meerkat_core::OperationAuthorizationError::Unavailable
    ));
    assert!(matches!(grant_refusal(GrantRefusal::Denied),
        meerkat_core::OperationAuthorizationError::Refused(refusal)
            if refusal.kind() == OperationRefusalKind::Denied));
}
