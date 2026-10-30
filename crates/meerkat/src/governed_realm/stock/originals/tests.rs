use super::*;
use meerkat_authorization::clock::{LocalAuthorizationClock, LocalAuthorizationTime};
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, GrantBackedWorkPolicy, OperationPolicyOwner, WorkOwnerAllowance,
};
use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
use meerkat_authorization::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose,
};
use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, ExecutionRestrictions, ProcessorRef,
};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    GrantLineageRef, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
    NativeWorkTarget, WorkAuthorityBasis,
};
use meerkat_core::authorization::{
    AuthorizationOperation, OperationAuthorizationFacts, OwnerQualifiedTarget,
    PreparedAuthorizationBinding, PreparedOperationCheck, SourceAuthorizationFacts,
    SourceAuthorizationTarget, SourceAuthorizationUse,
};
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::{PrincipalKind, TrustDomainId};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::{Input, PromptInput};
use meerkat_runtime::input_authority::NativeAdmissionError;
use meerkat_runtime::meerkat_machine::{
    ScopedGrantComposition, ScopedInvocation, ScopedMandateOwner, ScopedOriginalIngress,
};

fn principal(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        TrustDomainId::new("stock-test").expect("domain"),
    )
    .expect("qualified")
}

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("id")
}

fn evidence(value: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: ResourceRef {
            domain: ResourceDomain {
                authority: principal("cli"),
                namespace: "evidence".into(),
            },
            resource_id: value.into(),
        },
        revision: id("r1"),
        digest: EvidenceDigest::from_array([7; 32]),
    }
}

fn namespace(occurrence: &str) -> QualifiedIngressNamespace {
    QualifiedIngressNamespace {
        realm: RealmId::parse("stock").expect("realm"),
        ingress: ResourceDomain {
            authority: principal("cli"),
            namespace: "prompts".into(),
        },
        occurrence_scope: id(occurrence),
    }
}

fn work(name: &str) -> OriginalWorkRef {
    OriginalWorkRef {
        authority: principal("cli"),
        work: id(name),
    }
}

fn target() -> NativeWorkTarget {
    NativeWorkTarget {
        logical_owner: principal("owner"),
        logical_runtime: id("rt"),
        context: id("session"),
        context_generation: 1,
        audience: AudienceRef::Principal {
            principal: principal("requester"),
        },
    }
}

/// The original of `name`, authenticated in its own occurrence `name`.
fn original(name: &str) -> AuthenticatedOriginal {
    AuthenticatedOriginal {
        original_work: work(name),
        occurrence_scope: id(name),
        original_authentication: evidence("local-os-user"),
        root_event: evidence(name),
        contributing_work: Vec::new(),
        target: target(),
        authority_basis: WorkAuthorityBasis::HostPolicy {
            policy: evidence("stock-policy"),
        },
    }
}

fn ingress(requester: &str) -> NativeIngressContext {
    let input = Input::Prompt(PromptInput::new("hello", None));
    NativeIngressContext::from_trusted_ingress(
        &input,
        principal(requester),
        principal("cli"),
        RealmId::parse("stock").expect("realm"),
        evidence("local-os-user"),
    )
    .expect("trusted ingress")
}

fn ledger() -> (Arc<StockOriginalLedger>, LocalAuthorizationPublication) {
    let publication = LocalAuthorizationPublication::new();
    (StockOriginalLedger::new(publication.clone()), publication)
}

fn held(ledger: &StockOriginalLedger, occurrence: &str, name: &str) -> bool {
    ledger
        .authenticated_original(&ingress("requester"), &namespace(occurrence), &work(name))
        .expect("readable")
        .is_some()
}

fn custody(ledger: &StockOriginalLedger, name: &str) -> RetainedCustody {
    ledger
        .retained_custody(&namespace("any"), &work(name))
        .expect("readable")
}

fn retained(ledger: &StockOriginalLedger, name: &str) -> bool {
    matches!(custody(ledger, name), RetainedCustody::Retained(_))
}

/// A candidate authenticates only its own submission; native acceptance
/// promotes it; refusal or abandonment removes it, and it never served as a
/// contributor or retained custody meanwhile.
#[test]
fn a_candidate_becomes_custody_only_through_native_acceptance() {
    let (ledger, _) = ledger();
    let pending = ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("recorded");
    assert!(held(&ledger, "a", "a"), "authenticates its own submission");
    assert!(
        !held(&ledger, "b", "a"),
        "never qualifies as a contributor of another original"
    );
    assert!(
        !retained(&ledger, "a"),
        "not retained custody before admission"
    );
    assert!(
        matches!(
            ledger.record_candidate(&namespace("a"), &principal("requester"), original("a")),
            Err(StockCustodyError::CandidatePending)
        ),
        "one pending submission per original"
    );
    drop(pending);
    assert!(
        !held(&ledger, "a", "a"),
        "a refused or abandoned candidate is gone"
    );

    ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("recorded")
        .admit()
        .expect("admitted by native acceptance");
    assert!(
        held(&ledger, "b", "a"),
        "an admitted original is a contributor"
    );
    assert!(retained(&ledger, "a"));
    assert!(
        ledger
            .authenticated_original(&ingress("someone-else"), &namespace("b"), &work("a"))
            .expect("readable")
            .is_none(),
        "only for the requester that admitted it"
    );
    ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("an exact replay keeps the admitted record")
        .admit()
        .expect("no-op");
    assert!(retained(&ledger, "a"));
    ledger
        .release(&namespace("a"), &work("a"))
        .expect("released");
    assert!(!retained(&ledger, "a"));
    assert!(!held(&ledger, "b", "a"));
}

/// The ledger's own bound on a pending admission: a candidate reads
/// `AdmissionPending` only while the one submission that recorded it holds its
/// [`PendingOriginal`]. Promotion makes it `Retained`; dropping the handle, or
/// a promotion that cannot publish, withdraws it for good (`NotRetained`). An
/// exact replay of an admitted original stays `Retained`.
#[test]
fn a_candidate_is_pending_only_while_its_submission_holds_it() {
    let (ledger, _) = ledger();
    let promoted = ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("recorded");
    assert_eq!(custody(&ledger, "a"), RetainedCustody::AdmissionPending);
    promoted.admit().expect("admitted");
    assert!(retained(&ledger, "a"));
    let replay = ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("an exact replay keeps the admitted record");
    assert!(
        retained(&ledger, "a"),
        "a replay of an admitted original is no candidate"
    );
    drop(replay);
    assert!(retained(&ledger, "a"));

    let dropped = ledger
        .record_candidate(&namespace("b"), &principal("requester"), original("b"))
        .expect("recorded");
    assert_eq!(custody(&ledger, "b"), RetainedCustody::AdmissionPending);
    drop(dropped);
    assert_eq!(custody(&ledger, "b"), RetainedCustody::NotRetained);

    let failing = ledger
        .record_candidate(&namespace("c"), &principal("requester"), original("c"))
        .expect("recorded");
    ledger.fail_next_promotion_for_test();
    assert!(matches!(
        failing.admit(),
        Err(StockCustodyError::Unavailable)
    ));
    assert_eq!(
        custody(&ledger, "c"),
        RetainedCustody::NotRetained,
        "a promotion that cannot publish withdraws the candidate"
    );
    assert!(!held(&ledger, "c", "c"), "and leaves no candidate behind");
}

/// Two distinct candidates never share an occurrence: the second is refused,
/// so an unadmitted original of that occurrence cannot authenticate as a
/// contributor of the other submission.
#[test]
fn one_pending_candidate_per_occurrence() {
    let (ledger, _) = ledger();
    let mut b = original("b");
    b.occurrence_scope = id("a");
    let _a = ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("recorded");
    assert!(matches!(
        ledger.record_candidate(&namespace("a"), &principal("requester"), b.clone()),
        Err(StockCustodyError::CandidatePending)
    ));
    assert!(
        !held(&ledger, "a", "b"),
        "the refused candidate cannot authenticate as a contributor of the submission"
    );
    drop(_a);
    let pending_b = ledger
        .record_candidate(&namespace("a"), &principal("requester"), b)
        .expect("the occurrence is free once the first submission resolved");
    assert!(
        held(&ledger, "a", "b"),
        "and authenticates its own submission"
    );
    drop(pending_b);
}

struct Clock;
impl LocalAuthorizationClock for Clock {
    fn now(&self) -> Result<LocalAuthorizationTime, meerkat_authorization::clock::LocalClockError> {
        Ok(LocalAuthorizationTime {
            unix_ms: 100,
            monotonic: meerkat_core::time_compat::Instant::now(),
        })
    }
}

/// The stock host invocation policy of this fixture: the requester may
/// invoke the executor.
struct Policy;
impl ScopedMandateOwner for Policy {
    fn authorize_invocation(
        &self,
        _: &NativeIngressContext,
        _: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        Ok(())
    }
}
impl AdmittedWorkPolicyOwner for Policy {
    fn authorize_admitted_work(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        _: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 10_000,
        })
    }
}

/// The resource owner: reading the fixture record is allowed.
struct Records;
impl OperationPolicyOwner for Records {
    fn authorize_operation(
        &self,
        _: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        _: LocalPolicyPurpose,
        _: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        Ok(LocalPolicyAllowance {
            operation_values: vec![LocalOperationValues {
                action: ActionRef {
                    feature: "stock-test".into(),
                    action: "read".into(),
                },
                resource_domain: ResourceDomain {
                    authority: principal("owner"),
                    namespace: "records".into(),
                },
                processor: ProcessorRef::Principal {
                    principal: principal("executor"),
                },
                audience: AudienceRef::Principal {
                    principal: principal("requester"),
                },
            }],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 10_000,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
}

/// Work admitted from original `x` that names the held original `a`.
fn association_naming_a() -> InputAuthorityAssociation {
    InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester: principal("requester"),
        ingress_actor: principal("cli"),
        represented_subject: None,
        original_authentication: evidence("local-os-user"),
        logical_executor: principal("executor"),
        target: target(),
        original_work: work("x"),
        root_event: evidence("x"),
        contributing_work: vec![work("a")],
        authority_basis: WorkAuthorityBasis::HostPolicy {
            policy: evidence("stock-policy"),
        },
        controller_grant_lineage: vec![GrantLineageRef {
            root_authority: principal("grant-owner"),
            authority_namespace: id("controller"),
            authority_generation: 1,
            authority_incarnation: serde_json::from_value(serde_json::json!(
                "00000000-0000-4000-8000-000000000001"
            ))
            .expect("fixture incarnation data"),
            grant_id: id("controller-leaf"),
            issued_revision: 1,
        }],
        controller_model: None,
        controller_ceiling: ExecutionRestrictions::unrestricted(),
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: namespace("x"),
        contract: ContractRequirements::local_governed_v1(),
    })
    .expect("well-formed claims")
}

fn scope() -> OperationExecutionScope {
    OperationExecutionScope::RuntimeInput {
        owner_session_id: meerkat_core::SessionId::new(),
        runtime_epoch_id: meerkat_core::RuntimeEpochId::new(),
        submitted_input_id: meerkat_core::InputId::new(),
        canonical_input_id: meerkat_core::InputId::new(),
    }
}

/// On the actual native path: an operation prepared while the named
/// contributor is held is no longer current after the contributor is
/// released, before its entry; a fresh preparation sees no custody.
#[test]
fn a_release_invalidates_an_operation_prepared_under_the_held_original() {
    let (ledger, publication) = ledger();
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("controller"),
                generation: 1,
            },
            publication,
            Arc::new(Clock),
        )
        .expect("actual generated grant owner"),
    );
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: namespace("x").ingress,
        originals: Arc::clone(&ledger) as Arc<dyn AuthenticatedOriginalSource>,
        invocation: ScopedInvocation::from_policy(Arc::new(Policy)),
        operation_owner: Arc::new(Records),
        retained_resume: None,
    }
    .into_configuration();
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        grants,
        configuration.invocation_owner,
        configuration.operation_owner,
    ));
    ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("recorded")
        .admit()
        .expect("admitted");
    let context = policy
        .work_context(Arc::from([association_naming_a()]), scope())
        .expect("work context");
    let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: meerkat_core::OperationId::new(),
        execution_scope: context.execution_scope().clone(),
        run_id: Some(meerkat_core::RunId::new()),
        context_revision: None,
        operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                authority: principal("owner"),
                namespace: "records".into(),
                id: "record".into(),
            }),
            usage: SourceAuthorizationUse::Read,
        }),
    });
    let prepared = PreparedOperationCheck::prepare(context.clone(), binding.clone())
        .expect("prepared while the contributor is held");
    prepared
        .current()
        .expect("current while the contributor is held");

    ledger
        .release(&namespace("a"), &work("a"))
        .expect("released");
    assert!(
        prepared.current().is_err(),
        "the operation prepared before the release is refused at its entry"
    );
    assert!(
        PreparedOperationCheck::prepare(context, binding).is_err(),
        "a fresh preparation sees no custody"
    );
}

/// The association of `original`, submitted by the requester in `space`.
fn association_of(
    original: &AuthenticatedOriginal,
    space: &QualifiedIngressNamespace,
) -> InputAuthorityAssociation {
    InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester: principal("requester"),
        ingress_actor: principal("cli"),
        represented_subject: None,
        original_authentication: original.original_authentication.clone(),
        logical_executor: principal("executor"),
        target: original.target.clone(),
        original_work: original.original_work.clone(),
        root_event: original.root_event.clone(),
        contributing_work: original.contributing_work.clone(),
        authority_basis: original.authority_basis.clone(),
        controller_grant_lineage: vec![GrantLineageRef {
            root_authority: principal("grant-owner"),
            authority_namespace: id("controller"),
            authority_generation: 1,
            authority_incarnation: serde_json::from_value(serde_json::json!(
                "00000000-0000-4000-8000-000000000001"
            ))
            .expect("fixture incarnation data"),
            grant_id: id("controller-leaf"),
            issued_revision: 1,
        }],
        controller_model: None,
        controller_ceiling: ExecutionRestrictions::unrestricted(),
        admitted_ceiling: ExecutionRestrictions::unrestricted(),
        source_observations: Vec::new(),
        ingress_namespace: space.clone(),
        contract: ContractRequirements::local_governed_v1(),
    })
    .expect("well-formed claims")
}

/// Admitted B, then admitted A naming B, in one occurrence; B is released
/// and recorded again as the occurrence's only pending candidate; the
/// unchanged admitted A is replayed. The native scoped check of that replay
/// must not take the pending B as A's admitted contributor.
#[test]
fn a_pending_candidate_never_authenticates_as_a_contributor_of_a_replay() {
    let (ledger, publication) = ledger();
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("controller"),
                generation: 1,
            },
            publication,
            Arc::new(Clock),
        )
        .expect("actual generated grant owner"),
    );
    let space = namespace("s");
    let check = ScopedOriginalIngress::new(
        grants,
        space.ingress.clone(),
        Arc::clone(&ledger) as Arc<dyn AuthenticatedOriginalSource>,
        &ScopedInvocation::from_policy(Arc::new(Policy)),
    );
    let mut b = original("b");
    b.occurrence_scope = id("s");
    let mut a = original("a");
    a.occurrence_scope = id("s");
    a.contributing_work = vec![work("b")];
    let association = association_of(&a, &space);
    let mut prompt = PromptInput::new("replay a", None);
    prompt.header.authority_association = Some(association.clone());
    let input = Input::Prompt(prompt);
    let replay_ingress = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("cli"),
        RealmId::parse("stock").expect("realm"),
        evidence("local-os-user"),
    )
    .expect("trusted ingress");
    let runtime = LogicalRuntimeId::new("rt");

    ledger
        .record_candidate(&space, &principal("requester"), b.clone())
        .expect("recorded")
        .admit()
        .expect("b admitted");
    ledger
        .record_candidate(&space, &principal("requester"), a.clone())
        .expect("recorded")
        .admit()
        .expect("a admitted while b is held");
    assert!(
        check
            .check(&runtime, &input, &replay_ingress, &association)
            .is_ok(),
        "control: a with its admitted contributor b authenticates"
    );

    ledger.release(&space, &work("b")).expect("b released");
    let _pending_b = ledger
        .record_candidate(&space, &principal("requester"), b)
        .expect("b is the occurrence's only pending candidate");
    let replay = ledger
        .record_candidate(&space, &principal("requester"), a)
        .expect("a's exact replay keeps its admitted record");
    assert!(
        check
            .check(&runtime, &input, &replay_ingress, &association)
            .is_err(),
        "the pending b is not a's admitted contributor"
    );
    drop(replay);
}

/// The scoped native check over this ledger, and the input of original `a`
/// in occurrence `s2` naming `b` (whose own occurrence is `s1`) as its
/// contributor.
fn contributor_check(
    ledger: &Arc<StockOriginalLedger>,
) -> (
    ScopedOriginalIngress,
    Input,
    NativeIngressContext,
    InputAuthorityAssociation,
) {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("controller"),
                generation: 1,
            },
            ledger.publication.clone(),
            Arc::new(Clock),
        )
        .expect("actual generated grant owner"),
    );
    let space = namespace("s2");
    let check = ScopedOriginalIngress::new(
        grants,
        space.ingress.clone(),
        Arc::clone(ledger) as Arc<dyn AuthenticatedOriginalSource>,
        &ScopedInvocation::from_policy(Arc::new(Policy)),
    );
    let mut a = original("a");
    a.occurrence_scope = id("s2");
    a.contributing_work = vec![work("b")];
    let association = association_of(&a, &space);
    let mut prompt = PromptInput::new("a names b", None);
    prompt.header.authority_association = Some(association.clone());
    let input = Input::Prompt(prompt);
    let ingress = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("cli"),
        RealmId::parse("stock").expect("realm"),
        evidence("local-os-user"),
    )
    .expect("trusted ingress");
    let _a = ledger
        .record_candidate(&space, &principal("requester"), a)
        .expect("a's own submission")
        .admit();
    (check, input, ingress, association)
}

fn b_in_its_own_occurrence() -> AuthenticatedOriginal {
    let mut b = original("b");
    b.occurrence_scope = id("s1");
    b
}

/// RED-1 (R1): input naming a contributor whose admission its owner has not
/// settled yet is not yet admissible, never a malformed claim. The check reads
/// the contributor's custody before the requester-bound lookup, which never
/// answers for another occurrence's candidate. Once the owner promotes the
/// contributor, the same check passes.
#[test]
fn a_pending_contributor_is_not_yet_admissible() {
    let (ledger, _) = ledger();
    let (check, input, ingress, association) = contributor_check(&ledger);
    let runtime = LogicalRuntimeId::new("rt");
    let pending_b = ledger
        .record_candidate(
            &namespace("s1"),
            &principal("requester"),
            b_in_its_own_occurrence(),
        )
        .expect("b recorded in its own occurrence");
    let not_yet = check.check(&runtime, &input, &ingress, &association);
    assert!(
        matches!(
            not_yet,
            Err(NativeAdmissionError::Readiness(
                meerkat_core::authorization::ControllerReadinessFailure::OriginalAdmissionPending
            ))
        ),
        "not yet admissible while b is pending: {not_yet:?}"
    );
    pending_b.admit().expect("b admitted");
    let admitted = check.check(&runtime, &input, &ingress, &association);
    assert!(
        admitted.is_ok(),
        "admissible once b is promoted: {admitted:?}"
    );
}

/// The sequence pin for R1's read order: a contributor that another requester
/// recorded gets the not-yet answer while it is pending (its custody is read
/// before the requester-bound lookup), and after its promotion the
/// requester-bound lookup still refuses it as malformed, never `Ok`.
#[test]
fn another_requesters_pending_contributor_is_not_yet_then_still_malformed() {
    let (ledger, _) = ledger();
    let (check, input, ingress, association) = contributor_check(&ledger);
    let runtime = LogicalRuntimeId::new("rt");
    let pending_b = ledger
        .record_candidate(
            &namespace("s1"),
            &principal("someone-else"),
            b_in_its_own_occurrence(),
        )
        .expect("b recorded by another requester");
    let not_yet = check.check(&runtime, &input, &ingress, &association);
    assert!(
        matches!(
            not_yet,
            Err(NativeAdmissionError::Readiness(
                meerkat_core::authorization::ControllerReadinessFailure::OriginalAdmissionPending
            ))
        ),
        "not yet while b is pending: {not_yet:?}"
    );
    pending_b.admit().expect("b admitted for its own requester");
    let settled = check.check(&runtime, &input, &ingress, &association);
    assert!(
        matches!(
            &settled,
            Err(NativeAdmissionError::Refused(refusal))
                if refusal.kind() == meerkat_core::OperationRefusalKind::MalformedFacts
        ),
        "the requester-bound lookup still decides once b is settled: {settled:?}"
    );
}

/// CONTROL-cancelled (R1): a pending contributor whose operations are
/// cancelled is still a definite refusal, never "not yet". CONTROL-notheld
/// (R1): a contributor the owner never recorded stays malformed.
#[test]
fn a_cancelled_or_unknown_contributor_stays_a_definite_refusal() {
    let (ledger, _) = ledger();
    let (check, input, ingress, association) = contributor_check(&ledger);
    let runtime = LogicalRuntimeId::new("rt");
    let unknown = check.check(&runtime, &input, &ingress, &association);
    assert!(
        matches!(
            &unknown,
            Err(NativeAdmissionError::Refused(refusal))
                if refusal.kind() == meerkat_core::OperationRefusalKind::MalformedFacts
        ),
        "a contributor never recorded: {unknown:?}"
    );
    let _pending_b = ledger
        .record_candidate(
            &namespace("s1"),
            &principal("requester"),
            b_in_its_own_occurrence(),
        )
        .expect("b recorded in its own occurrence");
    ledger
        .cancel_operations(&namespace("s1"), &work("b"), evidence("cancel-b"))
        .expect("a pending candidate can be cancelled");
    let cancelled = check.check(&runtime, &input, &ingress, &association);
    assert!(
        matches!(
            &cancelled,
            Err(NativeAdmissionError::Refused(refusal))
                if refusal.kind() == meerkat_core::OperationRefusalKind::Denied
        ),
        "a cancellation wins over a pending admission: {cancelled:?}"
    );
}

/// RED-4 (R3): a contributor released and recorded again as a candidate is
/// pending, not gone. A fresh preparation of an operation of work that names
/// it is unavailable (retried), never denied, until the owner settles it.
/// CONTROL-cancelled (R3): cancelled while pending, it is denied.
#[test]
fn a_re_presented_pending_contributor_makes_operations_unavailable_not_denied() {
    let (ledger, publication) = ledger();
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("controller"),
                generation: 1,
            },
            publication,
            Arc::new(Clock),
        )
        .expect("actual generated grant owner"),
    );
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: namespace("x").ingress,
        originals: Arc::clone(&ledger) as Arc<dyn AuthenticatedOriginalSource>,
        invocation: ScopedInvocation::from_policy(Arc::new(Policy)),
        operation_owner: Arc::new(Records),
        retained_resume: None,
    }
    .into_configuration();
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        grants,
        configuration.invocation_owner,
        configuration.operation_owner,
    ));
    ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("recorded")
        .admit()
        .expect("admitted");
    let context = policy
        .work_context(Arc::from([association_naming_a()]), scope())
        .expect("work context");
    let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: meerkat_core::OperationId::new(),
        execution_scope: context.execution_scope().clone(),
        run_id: Some(meerkat_core::RunId::new()),
        context_revision: None,
        operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                authority: principal("owner"),
                namespace: "records".into(),
                id: "record".into(),
            }),
            usage: SourceAuthorizationUse::Read,
        }),
    });
    PreparedOperationCheck::prepare(context.clone(), binding.clone())
        .expect("prepared while the contributor is held");

    ledger
        .release(&namespace("a"), &work("a"))
        .expect("released");
    let pending_a = ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("presented again");
    assert!(
        matches!(
            PreparedOperationCheck::prepare(context.clone(), binding.clone()),
            Err(meerkat_core::OperationAuthorizationError::Unavailable)
        ),
        "a pending contributor is not yet, never denied"
    );
    ledger
        .cancel_operations(&namespace("a"), &work("a"), evidence("cancel-a"))
        .expect("a pending candidate can be cancelled");
    assert!(
        matches!(
            PreparedOperationCheck::prepare(context, binding),
            Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == meerkat_core::OperationRefusalKind::Denied
        ),
        "a cancellation wins over a pending admission"
    );
    drop(pending_a);
}

/// Cancelling an original's operations keeps its identity and admitted
/// custody (so its controller evidence survives) and is published on the
/// grant publication; its disposition is then cancelled, for a sibling it is
/// still active, and an unknown original cannot be cancelled.
#[test]
fn a_cancelled_original_keeps_its_custody_and_reports_cancelled() {
    let (ledger, publication) = ledger();
    for name in ["a", "sibling"] {
        ledger
            .record_candidate(&namespace(name), &principal("requester"), original(name))
            .expect("recorded")
            .admit()
            .expect("admitted");
    }
    let before = publication.observe(|| ()).expect("observable").1;
    ledger
        .cancel_operations(&namespace("a"), &work("a"), evidence("cancel-a"))
        .expect("cancelled");
    assert!(
        before.check_current().is_err(),
        "the cancellation is published on the grant publication"
    );
    assert_eq!(
        ledger
            .original_disposition(&namespace("any"), &work("a"))
            .expect("readable"),
        OriginalDisposition::OperationCancelled {
            evidence: evidence("cancel-a")
        }
    );
    assert_eq!(
        ledger
            .original_disposition(&namespace("any"), &work("sibling"))
            .expect("readable"),
        OriginalDisposition::Active
    );
    assert!(retained(&ledger, "a"), "its identity and custody stay held");
    assert!(matches!(
        ledger.cancel_operations(&namespace("x"), &work("unknown"), evidence("x")),
        Err(StockCustodyError::NotHeld)
    ));

    // The cancellation outlives custody: releasing the admitted row, or
    // abandoning a cancelled pending candidate, and presenting the same
    // original again never clears it.
    ledger
        .release(&namespace("a"), &work("a"))
        .expect("released");
    let pending = ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("presented again");
    assert!(matches!(
        ledger.original_disposition(&namespace("a"), &work("a")),
        Ok(OriginalDisposition::OperationCancelled { .. })
    ));
    drop(pending);
    let pending = ledger
        .record_candidate(&namespace("p"), &principal("requester"), original("p"))
        .expect("a pending candidate");
    ledger
        .cancel_operations(&namespace("p"), &work("p"), evidence("cancel-p"))
        .expect("a pending candidate can be cancelled");
    drop(pending);
    ledger
        .record_candidate(&namespace("p"), &principal("requester"), original("p"))
        .expect("the identical original presented again");
    assert!(matches!(
        ledger.original_disposition(&namespace("p"), &work("p")),
        Ok(OriginalDisposition::OperationCancelled { .. })
    ));
}

/// A helper's keyed WorkGraph admission commits, and a replay is disclosed,
/// only while the actual prepared setup operation is current for the child
/// and every relevant original, including a retained ancestor the child does
/// not name; the helper's own native admission refuses on the same
/// cancellation. The fence admits only its bound admission and never waits.
#[tokio::test]
async fn a_helper_work_item_is_admitted_only_while_its_child_original_is_current() {
    use crate::governed_realm::helper_admission::{
        GovernedHelperAdmissionFence, HelperAdmissionRefusal, helper_contributing_work,
    };
    use meerkat_workgraph::{
        CreateWorkItemRequest, MemoryWorkGraphStore, WorkAdmissionKey, WorkAdmissionOutcome,
        WorkGraphError, WorkGraphService, WorkNamespace,
    };

    let (ledger, publication) = ledger();
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("controller"),
                generation: 1,
            },
            publication.clone(),
            Arc::new(Clock),
        )
        .expect("grant owner on the ledger's publication"),
    );
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: namespace("x").ingress,
        originals: Arc::clone(&ledger) as Arc<dyn AuthenticatedOriginalSource>,
        invocation: ScopedInvocation::from_policy(Arc::new(Policy)),
        operation_owner: Arc::new(Records),
        retained_resume: None,
    }
    .into_configuration();
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        Arc::clone(&grants),
        configuration.invocation_owner,
        configuration.operation_owner,
    ));
    let admit = |original: AuthenticatedOriginal, occurrence: &str| {
        ledger
            .record_candidate(&namespace(occurrence), &principal("requester"), original)
            .expect("recorded")
            .admit()
            .expect("admitted");
    };
    // The child `a` names `b`; the host also retains the ancestor `c`.
    admit(original("b"), "b");
    admit(original("c"), "c");
    let mut a = original("a");
    a.contributing_work = vec![work("b")];
    admit(a.clone(), "a");
    let child = association_of(&a, &namespace("a"));
    let retained: Arc<[meerkat_authorization::work::RetainedOriginal]> =
        Arc::from([meerkat_authorization::work::RetainedOriginal {
            namespace: namespace("c"),
            original: work("c"),
        }]);
    assert_eq!(
        helper_contributing_work(&child, &retained),
        vec![work("a"), work("b"), work("c")]
    );

    // The actual setup operation, prepared in the child's work context.
    let context = policy
        .work_context(Arc::from([child.clone()]), scope())
        .expect("work context");
    let setup = PreparedOperationCheck::prepare(
        context.clone(),
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: meerkat_core::OperationId::new(),
            execution_scope: context.execution_scope().clone(),
            run_id: Some(meerkat_core::RunId::new()),
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                    authority: principal("owner"),
                    namespace: "records".into(),
                    id: "record".into(),
                }),
                usage: SourceAuthorizationUse::Read,
            }),
        }),
    )
    .expect("the setup operation is prepared");

    let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
    let request = || CreateWorkItemRequest {
        title: "setup helper".to_string(),
        ..Default::default()
    };
    let key = |value: &str| WorkAdmissionKey::new(value).expect("key");
    let fence = |value: &str| {
        GovernedHelperAdmissionFence::new(
            Arc::clone(&grants),
            ledger.clone(),
            child.clone(),
            Arc::clone(&retained),
            setup.clone(),
            Arc::clone(&policy),
            Arc::new(Clock),
            "default",
            WorkNamespace::default(),
            service
                .admission_identity(key(value), request())
                .expect("identity"),
        )
    };
    let refused = |result: Result<WorkAdmissionOutcome, WorkGraphError>,
                   fence: &GovernedHelperAdmissionFence,
                   expected: HelperAdmissionRefusal,
                   label: &str| {
        assert!(
            matches!(result, Err(WorkGraphError::Conflict(_))),
            "{label}"
        );
        assert_eq!(fence.refusal(), Some(expected), "{label}");
    };

    let ((), stamp) = publication.observe(|| ()).expect("observed");
    let current = fence("helper-1");
    assert!(matches!(
        service
            .create_idempotent_with_fence(key("helper-1"), request(), current.clone())
            .await,
        Ok(WorkAdmissionOutcome::Created(_))
    ));
    assert_eq!(current.refusal(), None);
    assert!(
        stamp.check_current().is_ok(),
        "the admission held the reservation without publishing an owner change"
    );

    assert!(
        matches!(
            service
                .create_idempotent_with_fence(key("helper-1"), request(), current.clone())
                .await,
            Err(WorkGraphError::Conflict(_))
        ),
        "a fence serves one attempt"
    );
    assert_eq!(current.refusal(), None, "its own attempt's outcome is kept");

    let other = fence("helper-1");
    refused(
        service
            .create_idempotent_with_fence(key("helper-2"), request(), other.clone())
            .await,
        &other,
        HelperAdmissionRefusal::Denied,
        "a fence admits only the admission it was bound to",
    );
    {
        let _change = publication.begin_owner_change().expect("owner change");
        let busy = fence("helper-2");
        refused(
            service
                .create_idempotent_with_fence(key("helper-2"), request(), busy.clone())
                .await,
            &busy,
            HelperAdmissionRefusal::Unavailable,
            "an owner change in progress refuses without waiting",
        );
    }
    assert!(matches!(
        service
            .create_idempotent_with_fence(key("helper-1"), request(), fence("helper-1"))
            .await,
        Ok(WorkAdmissionOutcome::Replayed(_))
    ));

    // The helper's own native admission names the complete set.
    let check = ScopedOriginalIngress::new(
        Arc::clone(&grants),
        namespace("h").ingress,
        Arc::clone(&ledger) as Arc<dyn AuthenticatedOriginalSource>,
        &ScopedInvocation::from_policy(Arc::new(Policy)),
    );
    let mut helper = original("h");
    helper.contributing_work = helper_contributing_work(&child, &retained);
    let pending = ledger
        .record_candidate(&namespace("h"), &principal("requester"), helper.clone())
        .expect("helper original recorded");
    let helper_association = association_of(&helper, &namespace("h"));
    let admit_helper = || {
        check.check(
            &LogicalRuntimeId::new("rt"),
            &Input::Prompt(PromptInput::new("set up", None)),
            &ingress("requester"),
            &helper_association,
        )
    };
    assert!(admit_helper().is_ok(), "every relevant original is live");

    // Cancelling the retained ancestor the child does not name refuses the
    // WorkGraph admission (new and replay) and the helper's admission.
    ledger
        .cancel_operations(&namespace("c"), &work("c"), evidence("cancel"))
        .expect("the ancestor is cancelled");
    for value in ["helper-1", "helper-3"] {
        let after = fence(value);
        refused(
            service
                .create_idempotent_with_fence(key(value), request(), after.clone())
                .await,
            &after,
            HelperAdmissionRefusal::Denied,
            value,
        );
    }
    assert!(
        admit_helper().is_err(),
        "the helper's native admission refuses on the ancestor"
    );
    drop(pending);
    assert_eq!(
        service
            .list(meerkat_workgraph::WorkItemFilter {
                include_terminal: true,
                ..Default::default()
            })
            .await
            .expect("list")
            .len(),
        1,
        "refused admissions wrote nothing"
    );
}

/// RED-2 (R2): a retained ancestor released and recorded again as a pending
/// candidate makes the helper's WorkGraph admission unavailable (retried),
/// never denied, and nothing is created; once its owner promotes it, the
/// admission is created. CONTROL-cancelled (R2): cancelled while pending, the
/// ancestor refuses the admission.
#[tokio::test]
async fn a_pending_retained_ancestor_makes_the_helper_admission_unavailable() {
    use crate::governed_realm::helper_admission::{
        GovernedHelperAdmissionFence, HelperAdmissionRefusal,
    };
    use meerkat_workgraph::{
        CreateWorkItemRequest, MemoryWorkGraphStore, WorkAdmissionKey, WorkAdmissionOutcome,
        WorkGraphError, WorkGraphService, WorkNamespace,
    };

    let (ledger, publication) = ledger();
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("controller"),
                generation: 1,
            },
            publication,
            Arc::new(Clock),
        )
        .expect("grant owner on the ledger's publication"),
    );
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: namespace("x").ingress,
        originals: Arc::clone(&ledger) as Arc<dyn AuthenticatedOriginalSource>,
        invocation: ScopedInvocation::from_policy(Arc::new(Policy)),
        operation_owner: Arc::new(Records),
        retained_resume: None,
    }
    .into_configuration();
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        Arc::clone(&grants),
        configuration.invocation_owner,
        configuration.operation_owner,
    ));
    let admit = |original: AuthenticatedOriginal, occurrence: &str| {
        ledger
            .record_candidate(&namespace(occurrence), &principal("requester"), original)
            .expect("recorded")
            .admit()
            .expect("admitted");
    };
    admit(original("b"), "b");
    admit(original("c"), "c");
    let mut a = original("a");
    a.contributing_work = vec![work("b")];
    admit(a.clone(), "a");
    let child = association_of(&a, &namespace("a"));
    let retained: Arc<[meerkat_authorization::work::RetainedOriginal]> =
        Arc::from([meerkat_authorization::work::RetainedOriginal {
            namespace: namespace("c"),
            original: work("c"),
        }]);
    let context = policy
        .work_context(Arc::from([child.clone()]), scope())
        .expect("work context");
    let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: meerkat_core::OperationId::new(),
        execution_scope: context.execution_scope().clone(),
        run_id: Some(meerkat_core::RunId::new()),
        context_revision: None,
        operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                authority: principal("owner"),
                namespace: "records".into(),
                id: "record".into(),
            }),
            usage: SourceAuthorizationUse::Read,
        }),
    });
    // Prepared after the last custody change, so the fence reads the
    // ancestor's custody, never a stale preparation.
    let setup = || {
        PreparedOperationCheck::prepare(context.clone(), binding.clone())
            .expect("the setup operation is prepared")
    };
    let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
    let request = || CreateWorkItemRequest {
        title: "setup helper".to_string(),
        ..Default::default()
    };
    let key = |value: &str| WorkAdmissionKey::new(value).expect("key");
    let fence = |value: &str| {
        GovernedHelperAdmissionFence::new(
            Arc::clone(&grants),
            ledger.clone(),
            child.clone(),
            Arc::clone(&retained),
            setup(),
            Arc::clone(&policy),
            Arc::new(Clock),
            "default",
            WorkNamespace::default(),
            service
                .admission_identity(key(value), request())
                .expect("identity"),
        )
    };
    let refused = |result: Result<WorkAdmissionOutcome, WorkGraphError>,
                   fence: &GovernedHelperAdmissionFence,
                   expected: HelperAdmissionRefusal,
                   label: &str| {
        assert!(
            matches!(result, Err(WorkGraphError::Conflict(_))),
            "{label}"
        );
        assert_eq!(fence.refusal(), Some(expected), "{label}");
    };

    ledger
        .release(&namespace("c"), &work("c"))
        .expect("the ancestor is released");
    let pending_c = ledger
        .record_candidate(&namespace("c"), &principal("requester"), original("c"))
        .expect("and presented again");
    let not_yet = fence("helper-1");
    refused(
        service
            .create_idempotent_with_fence(key("helper-1"), request(), not_yet.clone())
            .await,
        &not_yet,
        HelperAdmissionRefusal::Unavailable,
        "a pending ancestor is not yet, never denied",
    );
    pending_c.admit().expect("the ancestor is promoted");
    let current = fence("helper-1");
    assert!(matches!(
        service
            .create_idempotent_with_fence(key("helper-1"), request(), current.clone())
            .await,
        Ok(WorkAdmissionOutcome::Created(_))
    ));

    ledger
        .release(&namespace("c"), &work("c"))
        .expect("released again");
    let _pending_again = ledger
        .record_candidate(&namespace("c"), &principal("requester"), original("c"))
        .expect("presented again");
    ledger
        .cancel_operations(&namespace("c"), &work("c"), evidence("cancel"))
        .expect("cancelled while pending");
    let cancelled = fence("helper-2");
    refused(
        service
            .create_idempotent_with_fence(key("helper-2"), request(), cancelled.clone())
            .await,
        &cancelled,
        HelperAdmissionRefusal::Denied,
        "a cancellation wins over a pending admission",
    );
    assert_eq!(
        service
            .list(meerkat_workgraph::WorkItemFilter {
                include_terminal: true,
                ..Default::default()
            })
            .await
            .expect("list")
            .len(),
        1,
        "the refused admissions wrote nothing"
    );
}

/// A retained ancestor's REAL grant bounds the setup operation: a current
/// permissive grant admits, a restrictive grant whose actions exclude the
/// operation refuses, and a revoked grant refuses. A default dispatch context
/// cannot build a fence.
#[tokio::test]
async fn a_retained_ancestor_grant_bounds_the_setup_operation() {
    use crate::governed_realm::helper_admission::{
        GovernedHelperAdmissionFence, HelperAdmissionRefusal,
    };
    use meerkat_authorization_contracts::constraints::ExactRestriction;
    use meerkat_authorization_contracts::grant_mutation::{
        ControllerCustodyRefusal, ControllerGrantMutationCustody,
    };
    use meerkat_workgraph::{
        CreateWorkItemRequest, MemoryWorkGraphStore, WorkAdmissionKey, WorkAdmissionOutcome,
        WorkGraphService, WorkNamespace,
    };

    struct NoController;
    impl ControllerGrantMutationCustody for NoController {
        fn with_unreferenced_controller_grant<T, E>(
            &mut self,
            _: &GrantLineageRef,
            mutate: impl FnOnce() -> Result<T, E>,
        ) -> Result<Result<T, E>, ControllerCustodyRefusal> {
            Ok(mutate())
        }
    }

    let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("args");
    assert_eq!(
        GovernedHelperAdmissionFence::from_dispatch(
            meerkat_core::ToolCallView {
                id: "setup",
                name: "setup",
                args: &args,
            },
            &meerkat_core::ToolDispatchContext::default(),
            StockOriginalLedger::new(LocalAuthorizationPublication::new()),
            Arc::<[meerkat_authorization::work::RetainedOriginal]>::from([]),
            "default",
            WorkNamespace::default(),
            WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()))
                .admission_identity(
                    WorkAdmissionKey::new("k").expect("key"),
                    CreateWorkItemRequest::default(),
                )
                .expect("identity"),
        )
        .err(),
        Some(HelperAdmissionRefusal::Denied),
        "a default dispatch context has no governed setup operation"
    );

    let (ledger, publication) = ledger();
    let root = principal("grant-owner");
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: root.clone(),
                namespace: id("controller"),
                generation: 1,
            },
            publication.clone(),
            Arc::new(Clock),
        )
        .expect("grant owner on the ledger's publication"),
    );
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: namespace("x").ingress,
        originals: Arc::clone(&ledger) as Arc<dyn AuthenticatedOriginalSource>,
        invocation: ScopedInvocation::from_policy(Arc::new(Policy)),
        operation_owner: Arc::new(Records),
        retained_resume: None,
    }
    .into_configuration();
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        Arc::clone(&grants),
        configuration.invocation_owner,
        configuration.operation_owner,
    ));
    // The child `a` (host policy) and three retained ancestors with real
    // grants: permissive, restrictive and later revoked.
    let issue = |name: &str, restrictions: ExecutionRestrictions| {
        grants
            .issue_root(&root, id(name), principal("executor"), None, restrictions)
            .expect("issued")
    };
    let permissive = issue("permissive", ExecutionRestrictions::unrestricted());
    let restrictive = issue(
        "restrictive",
        ExecutionRestrictions {
            actions: ExactRestriction::exact([ActionRef {
                feature: "stock-test".into(),
                action: "write".into(),
            }]),
            ..ExecutionRestrictions::unrestricted()
        },
    );
    let granted = |name: &str, lineage: GrantLineageRef| {
        let mut original = original(name);
        original.authority_basis = WorkAuthorityBasis::GrantLineage {
            lineage: vec![lineage],
        };
        ledger
            .record_candidate(&namespace(name), &principal("requester"), original)
            .expect("recorded")
            .admit()
            .expect("admitted");
    };
    granted("p", permissive.clone());
    granted("r", restrictive);
    ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("recorded")
        .admit()
        .expect("admitted");
    let child = association_of(&original("a"), &namespace("a"));
    let context = policy
        .work_context(Arc::from([child.clone()]), scope())
        .expect("work context");
    let setup = PreparedOperationCheck::prepare(
        context.clone(),
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: meerkat_core::OperationId::new(),
            execution_scope: context.execution_scope().clone(),
            run_id: Some(meerkat_core::RunId::new()),
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                    authority: principal("owner"),
                    namespace: "records".into(),
                    id: "record".into(),
                }),
                usage: SourceAuthorizationUse::Read,
            }),
        }),
    )
    .expect("prepared");
    let service = WorkGraphService::new(Arc::new(MemoryWorkGraphStore::new()));
    let request = || CreateWorkItemRequest {
        title: "setup helper".to_string(),
        ..Default::default()
    };
    let attempt = |key: &str, retained: &[&str]| {
        let key = WorkAdmissionKey::new(key).expect("key");
        let fence = GovernedHelperAdmissionFence::new(
            Arc::clone(&grants),
            ledger.clone(),
            child.clone(),
            retained
                .iter()
                .map(|name| meerkat_authorization::work::RetainedOriginal {
                    namespace: namespace(name),
                    original: work(name),
                })
                .collect::<Vec<_>>()
                .into(),
            setup.clone(),
            Arc::clone(&policy),
            Arc::new(Clock),
            "default",
            WorkNamespace::default(),
            service
                .admission_identity(key.clone(), request())
                .expect("identity"),
        );
        (key, fence)
    };

    let (key, fence) = attempt("with-permissive", &["p"]);
    assert!(matches!(
        service
            .create_idempotent_with_fence(key, request(), fence.clone())
            .await,
        Ok(WorkAdmissionOutcome::Created(_))
    ));
    let (key, fence) = attempt("with-restrictive", &["p", "r"]);
    assert!(
        service
            .create_idempotent_with_fence(key, request(), fence.clone())
            .await
            .is_err()
    );
    assert_eq!(
        fence.refusal(),
        Some(HelperAdmissionRefusal::Denied),
        "a restrictive ancestor grant excludes the operation"
    );
    // The dispatch path's native work owner (the context's own
    // authorization chain) applies the same conjunction under its own
    // exclusion, and runs the operation only when permitted.
    let run_within = |retained: &[&str]| {
        let further = meerkat_authorization::work::FurtherOriginals {
            originals: retained
                .iter()
                .map(|name| meerkat_authorization::work::RetainedOriginal {
                    namespace: namespace(name),
                    original: work(name),
                })
                .collect::<Vec<_>>()
                .into(),
            source: Arc::new(meerkat_runtime::meerkat_machine::SourceOriginalBasis::new(
                ledger.clone(),
            )),
        };
        let mut ran = false;
        let result = context.authorization().run_if_current_within(
            setup.binding(),
            setup.validity().as_ref(),
            &further,
            &mut || ran = true,
        );
        (result, ran)
    };
    assert_eq!(run_within(&["p"]), (Ok(()), true));
    let (result, ran) = run_within(&["p", "r"]);
    assert!(
        matches!(
            result,
            Err(meerkat_core::OperationAuthorizationError::Refused(_))
        ) && !ran,
        "the work owner refuses a restrictive further original"
    );
    {
        // Its writer is held, so the step refuses to rebuild at once, with
        // no verdict on the operation: a later attempt may succeed.
        let _change = publication.begin_owner_change().expect("owner change");
        let (result, ran) = run_within(&["p"]);
        assert!(
            matches!(
                result,
                Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                    if refusal.kind() == meerkat_core::OperationRefusalKind::ReprepareRequired
            ) && !ran,
            "an owner change in progress refuses to rebuild without waiting: {result:?}"
        );
    }

    grants
        .revoke(&root, &permissive, &mut NoController)
        .expect("revoked");
    let (result, ran) = run_within(&["p"]);
    assert!(
        matches!(
            result,
            Err(meerkat_core::OperationAuthorizationError::Refused(_))
        ) && !ran,
        "the work owner refuses a revoked further original"
    );
    let (key, fence) = attempt("after-revoke", &["p"]);
    assert!(
        service
            .create_idempotent_with_fence(key, request(), fence.clone())
            .await
            .is_err()
    );
    assert_eq!(
        fence.refusal(),
        Some(HelperAdmissionRefusal::Denied),
        "a revoked ancestor grant refuses"
    );
}

/// Each original resolves under its own ingress, and an owner that cannot
/// say now is Unavailable (not Denied) without running the operation.
#[test]
fn each_original_resolves_under_its_own_ingress_and_unavailable_stays_typed() {
    use meerkat_authorization::work::{FurtherOriginals, RetainedOriginal};
    use meerkat_runtime::meerkat_machine::SourceOriginalBasis;

    /// The ledger, except that `broken` cannot be read now.
    struct Unreadable {
        ledger: Arc<StockOriginalLedger>,
        broken: OriginalWorkRef,
    }
    impl AuthenticatedOriginalSource for Unreadable {
        fn authenticated_original(
            &self,
            ingress: &NativeIngressContext,
            namespace: &QualifiedIngressNamespace,
            original: &OriginalWorkRef,
        ) -> Result<Option<AuthenticatedOriginal>, ControllerReadinessFailure> {
            self.ledger
                .authenticated_original(ingress, namespace, original)
        }

        fn retained_custody(
            &self,
            namespace: &QualifiedIngressNamespace,
            original: &OriginalWorkRef,
        ) -> Result<RetainedCustody, ControllerReadinessFailure> {
            if original == &self.broken {
                return Err(ControllerReadinessFailure::PolicyUnavailable);
            }
            self.ledger.retained_custody(namespace, original)
        }
    }

    let other = |occurrence: &str| QualifiedIngressNamespace {
        ingress: ResourceDomain {
            authority: principal("cli"),
            namespace: "other-ingress".into(),
        },
        ..namespace(occurrence)
    };
    let (ledger, publication) = ledger();
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("controller"),
                generation: 1,
            },
            publication,
            Arc::new(Clock),
        )
        .expect("grant owner"),
    );
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: namespace("x").ingress,
        originals: Arc::clone(&ledger) as Arc<dyn AuthenticatedOriginalSource>,
        invocation: ScopedInvocation::from_policy(Arc::new(Policy)),
        operation_owner: Arc::new(Records),
        retained_resume: None,
    }
    .into_configuration();
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        grants,
        configuration.invocation_owner,
        configuration.operation_owner,
    ));
    // `a` is admitted under the prompts ingress, `b` only under another
    // ingress; one work batch carries both associations, `a` first.
    ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("recorded")
        .admit()
        .expect("admitted");
    ledger
        .record_candidate(&other("b"), &principal("requester"), original("b"))
        .expect("recorded")
        .admit()
        .expect("admitted");
    let context = policy
        .work_context(
            Arc::from([
                association_of(&original("a"), &namespace("a")),
                association_of(&original("b"), &other("b")),
            ]),
            scope(),
        )
        .expect("one batch across two ingress namespaces");
    let setup = PreparedOperationCheck::prepare(
        context.clone(),
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: meerkat_core::OperationId::new(),
            execution_scope: context.execution_scope().clone(),
            run_id: Some(meerkat_core::RunId::new()),
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                    authority: principal("owner"),
                    namespace: "records".into(),
                    id: "record".into(),
                }),
                usage: SourceAuthorizationUse::Read,
            }),
        }),
    )
    .expect("prepared");
    let run = |source: Arc<dyn AuthenticatedOriginalSource>, further: Vec<RetainedOriginal>| {
        let further = FurtherOriginals {
            originals: further.into(),
            source: Arc::new(SourceOriginalBasis::new(source)),
        };
        let mut ran = false;
        let result = context.authorization().run_if_current_within(
            setup.binding(),
            setup.validity().as_ref(),
            &further,
            &mut || ran = true,
        );
        (result, ran)
    };
    assert_eq!(
        run(ledger.clone(), Vec::new()),
        (Ok(()), true),
        "`b` resolves under its own ingress, not the first association's"
    );
    assert!(
        matches!(
            run(
                ledger.clone(),
                vec![RetainedOriginal {
                    namespace: namespace("b"),
                    original: work("b"),
                }],
            ),
            (
                Err(meerkat_core::OperationAuthorizationError::Refused(_)),
                false
            )
        ),
        "`b` is not an original of the prompts ingress"
    );
    ledger
        .record_candidate(&namespace("c"), &principal("requester"), original("c"))
        .expect("recorded")
        .admit()
        .expect("admitted");
    assert_eq!(
        run(
            Arc::new(Unreadable {
                ledger,
                broken: work("c"),
            }),
            vec![RetainedOriginal {
                namespace: namespace("c"),
                original: work("c"),
            }],
        ),
        (
            Err(meerkat_core::OperationAuthorizationError::Unavailable),
            false
        ),
        "an owner that cannot say now stays Unavailable"
    );
}

/// The work owner evaluates the complete current entry decision inside its
/// own exclusion: an owner's review tier that changed to R2 after the
/// prepared check (which an outer check would still accept) refuses, and the
/// operation never runs.
#[test]
fn a_review_tier_change_after_the_entry_check_refuses_inside_the_exclusion() {
    use meerkat_authorization::work::FurtherOriginals;
    use meerkat_runtime::meerkat_machine::SourceOriginalBasis;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The fixture records owner, whose review tier the host can raise.
    struct TieredRecords(AtomicBool);
    impl OperationPolicyOwner for TieredRecords {
        fn authorize_operation(
            &self,
            association: &InputAuthorityAssociation,
            binding: &PreparedAuthorizationBinding,
            purpose: LocalPolicyPurpose,
            now_ms: u64,
        ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
            let mut allowance =
                Records.authorize_operation(association, binding, purpose, now_ms)?;
            if self.0.load(Ordering::SeqCst) {
                allowance.review_tier = meerkat_core::authorization::OperationReviewTier::R2;
            }
            Ok(allowance)
        }
    }

    let (ledger, publication) = ledger();
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("controller"),
                generation: 1,
            },
            publication,
            Arc::new(Clock),
        )
        .expect("grant owner"),
    );
    let tier = Arc::new(TieredRecords(AtomicBool::new(false)));
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: namespace("x").ingress,
        originals: Arc::clone(&ledger) as Arc<dyn AuthenticatedOriginalSource>,
        invocation: ScopedInvocation::from_policy(Arc::new(Policy)),
        operation_owner: tier.clone(),
        retained_resume: None,
    }
    .into_configuration();
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        grants,
        configuration.invocation_owner,
        configuration.operation_owner,
    ));
    ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("recorded")
        .admit()
        .expect("admitted");
    let context = policy
        .work_context(
            Arc::from([association_of(&original("a"), &namespace("a"))]),
            scope(),
        )
        .expect("work context");
    let setup = PreparedOperationCheck::prepare(
        context.clone(),
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: meerkat_core::OperationId::new(),
            execution_scope: context.execution_scope().clone(),
            run_id: Some(meerkat_core::RunId::new()),
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                    authority: principal("owner"),
                    namespace: "records".into(),
                    id: "record".into(),
                }),
                usage: SourceAuthorizationUse::Read,
            }),
        }),
    )
    .expect("prepared at R1");
    assert_eq!(
        setup.review_tier(),
        meerkat_core::authorization::OperationReviewTier::R1
    );
    let further = FurtherOriginals {
        originals: Arc::from([]),
        source: Arc::new(SourceOriginalBasis::new(ledger)),
    };
    let mut ran = false;
    assert_eq!(
        context.authorization().run_if_current_within(
            setup.binding(),
            setup.validity().as_ref(),
            &further,
            &mut || ran = true
        ),
        Ok(())
    );
    assert!(ran, "an R1 entry runs");

    tier.0.store(true, Ordering::SeqCst);
    assert!(
        setup.current().is_ok(),
        "an outer check of the prepared decision still accepts it"
    );
    let mut ran = false;
    assert!(
        matches!(
            context.authorization().run_if_current_within(
                setup.binding(),
                setup.validity().as_ref(),
                &further,
                &mut || ran = true
            ),
            Err(meerkat_core::OperationAuthorizationError::Refused(_))
        ),
        "the decision inside the exclusion is now R2"
    );
    assert!(!ran, "and the operation never runs");
}

mod child_issuance;

/// The held decision's own window and monotonic cutoff bound the entry: a
/// two-clock divergence (the wall clock barely advances while monotonic time
/// runs past the held cutoff) that would let a fresh decision live longer does not
/// extend the decision the caller actually holds, and a caller without a
/// reported validity refuses.
#[test]
fn the_held_decision_cutoff_bounds_the_entry_after_two_clock_divergence() {
    use meerkat_authorization::work::FurtherOriginals;
    use meerkat_runtime::meerkat_machine::SourceOriginalBasis;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Wall and monotonic time set explicitly.
    struct Manual {
        base: meerkat_core::time_compat::Instant,
        wall: AtomicU64,
        mono: AtomicU64,
    }
    impl LocalAuthorizationClock for Manual {
        fn now(
            &self,
        ) -> Result<LocalAuthorizationTime, meerkat_authorization::clock::LocalClockError> {
            Ok(LocalAuthorizationTime {
                unix_ms: self.wall.load(Ordering::SeqCst),
                monotonic: self
                    .base
                    .checked_add(std::time::Duration::from_millis(
                        self.mono.load(Ordering::SeqCst),
                    ))
                    .expect("monotonic offset"),
            })
        }
    }

    let clock = Arc::new(Manual {
        base: meerkat_core::time_compat::Instant::now(),
        wall: AtomicU64::new(100),
        mono: AtomicU64::new(0),
    });
    let (ledger, publication) = ledger();
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("controller"),
                generation: 1,
            },
            publication,
            Arc::clone(&clock) as Arc<dyn LocalAuthorizationClock>,
        )
        .expect("grant owner"),
    );
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&grants),
        ingress_domain: namespace("x").ingress,
        originals: Arc::clone(&ledger) as Arc<dyn AuthenticatedOriginalSource>,
        invocation: ScopedInvocation::from_policy(Arc::new(Policy)),
        operation_owner: Arc::new(Records),
        retained_resume: None,
    }
    .into_configuration();
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        grants,
        configuration.invocation_owner,
        configuration.operation_owner,
    ));
    ledger
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("recorded")
        .admit()
        .expect("admitted");
    let context = policy
        .work_context(
            Arc::from([association_of(&original("a"), &namespace("a"))]),
            scope(),
        )
        .expect("work context");
    // Prepared at wall 100 / mono 0 with expiry 10_000: its cutoff is
    // mono 9_900.
    let setup = PreparedOperationCheck::prepare(
        context.clone(),
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: meerkat_core::OperationId::new(),
            execution_scope: context.execution_scope().clone(),
            run_id: Some(meerkat_core::RunId::new()),
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                    authority: principal("owner"),
                    namespace: "records".into(),
                    id: "record".into(),
                }),
                usage: SourceAuthorizationUse::Read,
            }),
        }),
    )
    .expect("prepared");
    let held = setup
        .validity()
        .expect("the prepared decision reports its validity");
    let further = FurtherOriginals {
        originals: Arc::from([]),
        source: Arc::new(SourceOriginalBasis::new(ledger)),
    };
    let run = |prepared: Option<&meerkat_core::authorization::OperationValidity>| {
        let mut ran = false;
        let result = context.authorization().run_if_current_within(
            setup.binding(),
            prepared,
            &further,
            &mut || ran = true,
        );
        (result, ran)
    };
    assert_eq!(run(Some(&held)), (Ok(()), true));

    // The wall clock advances only to 102 while 9_901 ms elapse: a fresh
    // decision would live until mono 19_799, the held one ended at 9_900.
    clock.wall.store(102, Ordering::SeqCst);
    clock.mono.store(9_901, Ordering::SeqCst);
    let (result, ran) = run(Some(&held));
    assert!(
        matches!(
            result,
            Err(meerkat_core::OperationAuthorizationError::Refused(_))
        ) && !ran,
        "the held cutoff bounds the entry"
    );

    clock.mono.store(0, Ordering::SeqCst);
    clock.wall.store(100, Ordering::SeqCst);
    assert_eq!(
        run(None),
        (
            Err(meerkat_core::OperationAuthorizationError::Unavailable),
            false
        ),
        "a caller without a reported validity refuses"
    );
}

/// Test-support seam for a downstream app's real-holder failure controls: the
/// promotion fault belongs to ONE ledger and replaces only the promotion's
/// publication begin. Recording a candidate, an exact replay of an admitted
/// original and another ledger leave it armed; the faulted promotion takes
/// the ordinary refusal (candidate dropped, nothing published) and is
/// consumed once.
#[test]
fn promotion_fault_targets_one_ledger_at_the_promotion_boundary() {
    let (armed, publication) = ledger();
    let (other, _) = ledger();
    armed
        .record_candidate(
            &namespace("kept"),
            &principal("requester"),
            original("kept"),
        )
        .expect("recorded")
        .admit()
        .expect("admitted before arming");
    armed.fail_next_promotion_for_test();

    let pending = armed
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("recording does not consume the fault");
    armed
        .record_candidate(
            &namespace("kept"),
            &principal("requester"),
            original("kept"),
        )
        .expect("an exact replay of an admitted original")
        .admit()
        .expect("a replay promotes nothing, so it does not consume the fault");
    other
        .record_candidate(&namespace("a"), &principal("requester"), original("a"))
        .expect("recorded")
        .admit()
        .expect("another ledger is not armed");
    assert!(retained(&other, "a"));

    let stamp = publication.observe(|| ()).expect("observable").1;
    assert!(
        matches!(pending.admit(), Err(StockCustodyError::Unavailable)),
        "the armed promotion takes the publication-failure refusal"
    );
    assert!(
        stamp.check_current().is_ok(),
        "the faulted promotion published nothing"
    );
    assert!(!held(&armed, "a", "a"), "the candidate is dropped");
    assert!(!retained(&armed, "a"), "the original is not held");

    armed
        .record_candidate(&namespace("b"), &principal("requester"), original("b"))
        .expect("recorded")
        .admit()
        .expect("the fault fired once");
    assert!(retained(&armed, "b"));
}
