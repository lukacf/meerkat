//! The authenticated-original check against real grant and fixture owners.

use std::sync::Mutex;

use super::*;
use crate::input_authority::tests::input;
use meerkat_authorization::grant_policy::GrantBackedWorkPolicy;
use meerkat_authorization::grants::LocalGrantConfiguration;
use meerkat_authorization::policy::{
    LocalOperationValues, LocalPolicyAllowance, LocalPolicyPurpose, LocalWorkPolicy,
};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization::work::{LocalAuthorizationClock, LocalAuthorizationTime};
use meerkat_authorization_contracts::constraints::AudienceRef;
use meerkat_authorization_contracts::constraints::ExecutionRestrictions;
use meerkat_authorization_contracts::constraints::LifetimeRestriction;
use meerkat_authorization_contracts::constraints::{
    ActionRef, DelegationDepth, DomainBound, DomainRestriction, ExactRestriction, ProcessorRef,
    SetBound, UnresolvedConstraint,
};
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::work_association::GrantLineageRef;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociationCandidate;
use meerkat_core::{PrincipalKind, TrustDomainId};

struct Clock;
impl LocalAuthorizationClock for Clock {
    fn now(&self) -> Result<LocalAuthorizationTime, meerkat_authorization::clock::LocalClockError> {
        Ok(LocalAuthorizationTime {
            unix_ms: 100,
            monotonic: meerkat_core::time_compat::Instant::now(),
        })
    }
}

fn principal(value: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        value,
        TrustDomainId::new("native-test").expect("domain"),
    )
    .expect("qualified")
}

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("fixture id")
}

/// The adapter's authenticated originals, keyed by the authenticated
/// requester of the ingress observation.
struct Originals {
    requester: PrincipalRef,
    original: Mutex<Result<Option<AuthenticatedOriginal>, ControllerReadinessFailure>>,
    /// The custody of the originals that the main original names as its
    /// contributors, keyed by work.
    contributors: Mutex<std::collections::HashMap<EvidenceId, AuthenticatedOriginal>>,
    /// Contributors the source owner no longer retains, though the ingress
    /// still authenticates them.
    released: Mutex<std::collections::HashSet<EvidenceId>>,
    /// Originals whose operations the source owner cancelled, with its
    /// evidence.
    cancelled: Mutex<std::collections::HashMap<EvidenceId, HistoricalEvidenceRef>>,
    /// Originals whose retained custody the source owner cannot read now.
    unreadable: Mutex<std::collections::HashSet<EvidenceId>>,
    /// Contributors that are still the source owner's candidates: it has not
    /// settled their admission yet.
    pending: Mutex<std::collections::HashSet<EvidenceId>>,
}
impl AuthenticatedOriginalSource for Originals {
    fn authenticated_original(
        &self,
        ingress: &NativeIngressContext,
        _namespace: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<AuthenticatedOriginal>, ControllerReadinessFailure> {
        if ingress.requester() != &self.requester {
            return Ok(None);
        }
        let main = self.original.lock().expect("fixture").clone()?;
        if main
            .as_ref()
            .is_some_and(|main| &main.original_work == original)
        {
            return Ok(main);
        }
        Ok(self
            .contributors
            .lock()
            .expect("fixture")
            .get(&original.work)
            .cloned())
    }

    fn retained_custody(
        &self,
        _namespace: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<RetainedCustody, ControllerReadinessFailure> {
        if self
            .unreadable
            .lock()
            .expect("fixture")
            .contains(&original.work)
        {
            return Err(ControllerReadinessFailure::PolicyUnavailable);
        }
        if self
            .released
            .lock()
            .expect("fixture")
            .contains(&original.work)
        {
            return Ok(RetainedCustody::NotRetained);
        }
        if self
            .pending
            .lock()
            .expect("fixture")
            .contains(&original.work)
        {
            return Ok(RetainedCustody::AdmissionPending);
        }
        Ok(self
            .contributors
            .lock()
            .expect("fixture")
            .get(&original.work)
            .map_or(RetainedCustody::NotRetained, |custody| {
                RetainedCustody::Retained(custody.authority_basis.clone())
            }))
    }

    fn original_disposition(
        &self,
        _namespace: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<OriginalDisposition, ControllerReadinessFailure> {
        Ok(self
            .cancelled
            .lock()
            .expect("fixture")
            .get(&original.work)
            .map_or(OriginalDisposition::Active, |evidence| {
                OriginalDisposition::OperationCancelled {
                    evidence: evidence.clone(),
                }
            }))
    }
}

/// The custody of one contributing original under `basis`.
fn contributor_custody(
    claims: &InputAuthorityAssociationCandidate,
    work: &OriginalWorkRef,
    basis: WorkAuthorityBasis,
) -> AuthenticatedOriginal {
    AuthenticatedOriginal {
        original_work: work.clone(),
        occurrence_scope: id(&format!("occurrence-{}", work.work.as_str())),
        original_authentication: claims.original_authentication.clone(),
        root_event: claims.root_event.clone(),
        contributing_work: Vec::new(),
        target: claims.target.clone(),
        authority_basis: basis,
    }
}

struct Mandates(Mutex<Result<(), NativeAdmissionError>>);
impl ScopedMandateOwner for Mandates {
    fn authorize_invocation(
        &self,
        _ingress: &NativeIngressContext,
        _association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        self.0.lock().expect("fixture").clone()
    }
}
/// The same policy object is the native admitted-work owner.
impl AdmittedWorkPolicyOwner for Mandates {
    fn authorize_admitted_work(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        _purpose: meerkat_authorization::policy::LocalPolicyPurpose,
        _now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::WorkOwnerAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }
}

/// RED-6 (R6): a source original whose admission its owner has not settled
/// yet is not yet current, never refused; it is current again once the owner
/// promotes it. CONTROL-cancelled (R6): cancelled while pending, it is
/// refused.
#[test]
fn a_pending_source_original_is_not_yet_current() {
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let current = || {
        require_source_original_live(
            &f.grants,
            f.originals.as_ref(),
            &f.claims.ingress_namespace,
            &source,
            &f.claims.logical_executor,
            None,
        )
    };
    assert!(current().is_ok(), "control: held and active");
    f.originals
        .pending
        .lock()
        .expect("fixture")
        .insert(source.work.clone());
    let not_yet = current();
    assert!(
        matches!(
            not_yet,
            Err(NativeAdmissionError::Readiness(
                ControllerReadinessFailure::OriginalAdmissionPending
            ))
        ),
        "{not_yet:?}"
    );
    f.originals
        .cancelled
        .lock()
        .expect("fixture")
        .insert(source.work.clone(), fixture_evidence(&f, "cancel-source"));
    assert_eq!(
        refusal(current()),
        Some(OperationRefusalKind::Denied),
        "a cancellation wins over a pending admission"
    );
    f.originals
        .cancelled
        .lock()
        .expect("fixture")
        .remove(&source.work);
    f.originals
        .pending
        .lock()
        .expect("fixture")
        .remove(&source.work);
    assert!(current().is_ok(), "current once promoted");
}

struct Fixture {
    check: ScopedOriginalIngress,
    originals: Arc<Originals>,
    mandates: Arc<Mandates>,
    grants: Arc<LocalGrantAuthority>,
    /// The grant owner's publication, shared by every owner of the work.
    publication: LocalAuthorizationPublication,
    root: PrincipalRef,
    input: Input,
    ingress: Arc<NativeIngressContext>,
    claims: InputAuthorityAssociationCandidate,
    runtime: LogicalRuntimeId,
}

/// An authenticated original with two source contributors, admitted to its
/// current destination under a host-policy mandate.
fn fixture() -> Fixture {
    let input = input("caller");
    let ingress = Arc::clone(input.header().ingress_context.as_ref().expect("ingress"));
    let mut claims = input
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    // The source owner of the original is not the ingress authority.
    claims.original_work.authority = principal("source-owner");
    assert_ne!(
        claims.original_work.authority,
        claims.ingress_namespace.ingress.authority
    );
    claims.contributing_work = vec![
        OriginalWorkRef {
            authority: claims.original_work.authority.clone(),
            work: id("source-a"),
        },
        OriginalWorkRef {
            authority: claims.original_work.authority.clone(),
            work: id("source-b"),
        },
    ];
    let root = principal("grant-owner");
    let publication = LocalAuthorizationPublication::new();
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: root.clone(),
                namespace: id("work"),
                generation: 1,
            },
            publication.clone(),
            Arc::new(Clock),
        )
        .expect("real generated grant owner"),
    );
    let originals = Arc::new(Originals {
        requester: claims.requester.clone(),
        original: Mutex::new(Ok(Some(AuthenticatedOriginal {
            original_work: claims.original_work.clone(),
            occurrence_scope: claims.ingress_namespace.occurrence_scope.clone(),
            original_authentication: claims.original_authentication.clone(),
            root_event: claims.root_event.clone(),
            contributing_work: claims.contributing_work.clone(),
            target: claims.target.clone(),
            authority_basis: claims.authority_basis.clone(),
        }))),
        contributors: Mutex::new(
            claims
                .contributing_work
                .iter()
                .map(|work| {
                    (
                        work.work.clone(),
                        contributor_custody(&claims, work, claims.authority_basis.clone()),
                    )
                })
                .collect(),
        ),
        released: Mutex::new(std::collections::HashSet::new()),
        cancelled: Mutex::new(std::collections::HashMap::new()),
        unreadable: Mutex::new(std::collections::HashSet::new()),
        pending: Mutex::new(std::collections::HashSet::new()),
    });
    let mandates = Arc::new(Mandates(Mutex::new(Ok(()))));
    let invocation = ScopedInvocation::from_policy(mandates.clone());
    let check = ScopedOriginalIngress::new(
        Arc::clone(&grants),
        claims.ingress_namespace.ingress.clone(),
        originals.clone(),
        &invocation,
    );
    // Admission and the admitted-work owner are one policy object.
    assert!(Arc::ptr_eq(
        &(invocation.admitted_work() as Arc<dyn AdmittedWorkPolicyOwner>),
        &(mandates.clone() as Arc<dyn AdmittedWorkPolicyOwner>),
    ));
    Fixture {
        check,
        originals,
        mandates,
        grants,
        publication,
        root,
        input,
        ingress,
        claims,
        runtime: LogicalRuntimeId::new("native-test"),
    }
}

impl Fixture {
    /// The source owner's original was authenticated by `authentication`.
    fn set_authentication(&self, authentication: &HistoricalEvidenceRef) {
        if let Ok(Some(original)) = self.originals.original.lock().expect("fixture").as_mut() {
            original.original_authentication = authentication.clone();
        }
    }

    fn admit_with(
        &self,
        claims: InputAuthorityAssociationCandidate,
        ingress: &NativeIngressContext,
    ) -> Result<(), NativeAdmissionError> {
        let association = InputAuthorityAssociation::new(claims).expect("well-formed claims");
        self.check
            .check(&self.runtime, &self.input, ingress, &association)
    }

    /// The source owner holds the main original under `basis`.
    fn set_basis(&self, basis: &WorkAuthorityBasis) {
        if let Ok(Some(original)) = self.originals.original.lock().expect("fixture").as_mut() {
            original.authority_basis = basis.clone();
        }
    }

    fn admit(
        &self,
        claims: InputAuthorityAssociationCandidate,
    ) -> Result<(), NativeAdmissionError> {
        let association = InputAuthorityAssociation::new(claims).expect("well-formed claims");
        self.check
            .check(&self.runtime, &self.input, &self.ingress, &association)
    }
}

fn refusal(result: Result<(), NativeAdmissionError>) -> Option<OperationRefusalKind> {
    match result {
        Err(NativeAdmissionError::Refused(refusal)) => Some(refusal.kind()),
        _ => None,
    }
}

fn readiness(result: Result<(), NativeAdmissionError>) -> Option<ControllerReadinessFailure> {
    match result {
        Err(NativeAdmissionError::Readiness(reason)) => Some(reason),
        _ => None,
    }
}

/// The authenticated original, its two contributors, its mandate and its
/// current destination admit; the check composes as the installed ingress.
#[test]
fn an_authenticated_original_to_its_current_destination_admits() {
    let f = fixture();
    assert!(f.admit(f.claims.clone()).is_ok());
    let association = InputAuthorityAssociation::new(f.claims.clone()).expect("claims");
    let installed = fixture().check.into_check();
    assert!(installed(&f.runtime, &f.input, &f.ingress, &association).is_ok());
}

/// Every source contributor is authenticated, in order: a reordered,
/// missing or extra contributor, another root event or another original
/// authentication is refused, and so is an original the ingress did not
/// authenticate or one outside the installed ingress domain.
#[test]
fn every_contributor_and_the_original_itself_are_authenticated() {
    let f = fixture();
    let malformed = Some(OperationRefusalKind::MalformedFacts);

    let mut reordered = f.claims.clone();
    reordered.contributing_work.reverse();
    assert_eq!(refusal(f.admit(reordered)), malformed, "reordered");
    let mut missing = f.claims.clone();
    missing.contributing_work.pop();
    assert_eq!(refusal(f.admit(missing)), malformed, "missing");
    let mut extra = f.claims.clone();
    extra.contributing_work.push(OriginalWorkRef {
        authority: f.claims.original_work.authority.clone(),
        work: id("source-c"),
    });
    assert_eq!(refusal(f.admit(extra)), malformed, "extra");

    let other_event = fixture_evidence(&f, "another-event");
    let mut root = f.claims.clone();
    root.root_event = other_event.clone();
    assert_eq!(refusal(f.admit(root)), malformed, "root event");
    let mut authentication = f.claims.clone();
    authentication.original_authentication = other_event;
    assert_eq!(
        refusal(f.admit(authentication)),
        malformed,
        "original authentication"
    );

    let mut another_source = f.claims.clone();
    another_source.original_work.authority = principal("another-source");
    assert_eq!(
        refusal(f.admit(another_source)),
        malformed,
        "another source authority"
    );
    let mut another_occurrence = f.claims.clone();
    another_occurrence.ingress_namespace.occurrence_scope = id("another-occurrence");
    assert_eq!(
        refusal(f.admit(another_occurrence)),
        malformed,
        "another occurrence"
    );
    let mut outside = f.claims.clone();
    outside.ingress_namespace.ingress.namespace = "another-namespace".into();
    assert_eq!(
        refusal(f.admit(outside)),
        malformed,
        "outside the installed ingress domain"
    );

    *f.originals.original.lock().expect("fixture") = Ok(None);
    assert_eq!(
        refusal(f.admit(f.claims.clone())),
        malformed,
        "not authenticated by this ingress"
    );
    *f.originals.original.lock().expect("fixture") =
        Err(ControllerReadinessFailure::PolicyUnavailable);
    assert_eq!(
        readiness(f.admit(f.claims.clone())),
        Some(ControllerReadinessFailure::PolicyUnavailable),
        "an adapter that cannot say now is readiness, not a verdict"
    );
}

/// The destination is the one the adapter authenticated for this original,
/// on the admitting runtime: another context, generation, owner or audience,
/// or another runtime, is refused. Whether the session currently serves it
/// is the machine-owned scoped context binding's check.
#[test]
fn the_destination_is_the_authenticated_one_on_this_runtime() {
    let f = fixture();
    let malformed = Some(OperationRefusalKind::MalformedFacts);
    for (label, change) in [
        (
            "context",
            Box::new(|t: &mut NativeWorkTarget| t.context = id("other-context"))
                as Box<dyn Fn(&mut NativeWorkTarget)>,
        ),
        (
            "generation",
            Box::new(|t: &mut NativeWorkTarget| t.context_generation += 1),
        ),
        (
            "owner",
            Box::new(|t: &mut NativeWorkTarget| t.logical_owner = principal("other-owner")),
        ),
        (
            "audience",
            Box::new(|t: &mut NativeWorkTarget| {
                t.audience = AudienceRef::Principal {
                    principal: principal("other-audience"),
                };
            }),
        ),
    ] {
        let mut claims = f.claims.clone();
        change(&mut claims.target);
        assert_eq!(refusal(f.admit(claims)), malformed, "{label}");
    }

    let mut foreign = f.claims.clone();
    foreign.target.logical_runtime = id("another-runtime");
    if let Ok(Some(original)) = f.originals.original.lock().expect("fixture").as_mut() {
        original.target = foreign.target.clone();
    }
    assert_eq!(
        refusal(f.admit(foreign)),
        malformed,
        "an authenticated destination on another runtime"
    );
}

/// The mandate is the host owner's current verdict, and a grant-lineage
/// mandate must also resolve in the actual grant owner for the executor and
/// the represented subject.
#[test]
fn the_mandate_is_authorized_by_its_actual_owners() {
    let f = fixture();
    *f.mandates.0.lock().expect("fixture") = Err(NativeAdmissionError::Refused(
        OperationRefused::new(OperationRefusalKind::Denied),
    ));
    assert_eq!(
        refusal(f.admit(f.claims.clone())),
        Some(OperationRefusalKind::Denied),
        "the host mandate owner refuses"
    );
    *f.mandates.0.lock().expect("fixture") = Ok(());

    let subject = principal("represented-person");
    let lineage = f
        .grants
        .issue_root(
            &f.root,
            id("work-leaf"),
            f.claims.logical_executor.clone(),
            Some(subject.clone()),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("real issued grant");
    let mut granted = f.claims.clone();
    granted.represented_subject = Some(subject);
    granted.authority_basis = WorkAuthorityBasis::GrantLineage {
        lineage: vec![lineage],
    };
    f.set_basis(&granted.authority_basis);
    assert!(
        f.admit(granted.clone()).is_ok(),
        "the issued grant resolves"
    );

    let mut other_subject = granted.clone();
    other_subject.represented_subject = Some(principal("someone-else"));
    assert_eq!(
        refusal(f.admit(other_subject)),
        Some(OperationRefusalKind::Denied),
        "the grant does not represent another subject"
    );
    let mut other_executor = granted;
    other_executor.logical_executor = principal("other-executor");
    assert_eq!(
        refusal(f.admit(other_executor)),
        Some(OperationRefusalKind::Denied),
        "the grant does not cover another executor"
    );
}

/// The final payload is bound: the bound input admits through the existing
/// submission check, and any later change of it is refused.
#[test]
fn the_final_submitted_payload_is_bound() {
    let f = fixture();
    let association = InputAuthorityAssociation::new(f.claims.clone()).expect("claims");
    let controller = f.ingress.controller_client().cloned().expect("controller");
    let bound = bind_scoped_original(
        crate::input::Input::Prompt(crate::input::PromptInput::new("exact original", None)),
        association,
        IdempotencyKey::new("original-event"),
        f.claims.requester.clone(),
        f.claims.ingress_actor.clone(),
        f.claims.ingress_namespace.realm.clone(),
        f.claims.original_authentication.clone(),
        controller,
    )
    .expect("bound to the final input");
    let ingress = bound.header().ingress_context.clone().expect("ingress");
    assert!(ingress.verify_submission(&bound).is_ok());

    let Input::Prompt(mut changed) = bound.clone() else {
        panic!("prompt")
    };
    changed.content = meerkat_core::types::ContentInput::Text("another payload".into());
    assert!(
        ingress.verify_submission(&Input::Prompt(changed)).is_err(),
        "a changed payload is not the bound submission"
    );
}

fn fixture_evidence(f: &Fixture, resource: &str) -> HistoricalEvidenceRef {
    let mut evidence = f.claims.root_event.clone();
    evidence.resource.resource_id = resource.into();
    evidence
}

/// The adapter's own custody view is the authority, not the claims: claims
/// that are internally consistent but disagree with that view on a single
/// field are refused, and the same claims admit once the view agrees.
#[test]
fn an_independently_disagreeing_original_is_refused() {
    let f = fixture();
    let malformed = Some(OperationRefusalKind::MalformedFacts);
    let agreeing = f
        .originals
        .original
        .lock()
        .expect("fixture")
        .clone()
        .expect("view")
        .expect("authenticated");
    for (label, change) in [
        (
            "occurrence",
            Box::new(|o: &mut AuthenticatedOriginal| {
                o.occurrence_scope = id("occurrence-2");
            }) as Box<dyn Fn(&mut AuthenticatedOriginal)>,
        ),
        (
            "original work",
            Box::new(|o: &mut AuthenticatedOriginal| o.original_work.work = id("other-work")),
        ),
        (
            "original authentication",
            Box::new(|o: &mut AuthenticatedOriginal| {
                o.original_authentication.resource.resource_id = "other-authentication".into();
            }),
        ),
        (
            "root event",
            Box::new(|o: &mut AuthenticatedOriginal| {
                o.root_event.resource.resource_id = "other-event".into();
            }),
        ),
        (
            "contributor order",
            Box::new(|o: &mut AuthenticatedOriginal| o.contributing_work.reverse()),
        ),
        (
            "addressed destination",
            Box::new(|o: &mut AuthenticatedOriginal| o.target.context_generation += 1),
        ),
    ] {
        let mut view = agreeing.clone();
        change(&mut view);
        *f.originals.original.lock().expect("fixture") = Ok(Some(view));
        assert_eq!(refusal(f.admit(f.claims.clone())), malformed, "{label}");
    }
    *f.originals.original.lock().expect("fixture") = Ok(Some(agreeing));
    assert!(
        f.admit(f.claims.clone()).is_ok(),
        "the agreeing view admits"
    );
}

/// A valid grant lineage for the executor and subject does not authorize the
/// requester to invoke it: the host invocation owner's refusal stands even
/// though the lineage resolves, and the same claims admit once it allows.
#[test]
fn an_unauthorized_requester_with_a_valid_lineage_is_refused() {
    let f = fixture();
    let lineage = f
        .grants
        .issue_root(
            &f.root,
            id("valid-leaf"),
            f.claims.logical_executor.clone(),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("real issued grant");
    let mut granted = f.claims.clone();
    granted.authority_basis = WorkAuthorityBasis::GrantLineage {
        lineage: vec![lineage.clone()],
    };
    f.set_basis(&granted.authority_basis);
    assert!(
        f.grants
            .resolve_lineage(&[lineage], &granted.logical_executor, None)
            .is_ok(),
        "the lineage itself resolves"
    );
    *f.mandates.0.lock().expect("fixture") = Err(NativeAdmissionError::Refused(
        OperationRefused::new(OperationRefusalKind::Denied),
    ));
    assert_eq!(
        refusal(f.admit(granted.clone())),
        Some(OperationRefusalKind::Denied),
        "the requester may not invoke it"
    );
    *f.mandates.0.lock().expect("fixture") = Ok(());
    assert!(f.admit(granted).is_ok(), "an authorized requester admits");
}

/// A host invocation policy over the real grant owner: the requester may
/// invoke while its issued invocation grant resolves. Both faces read the
/// same grant publication.
struct GrantedInvocation {
    grants: Arc<LocalGrantAuthority>,
    requester: PrincipalRef,
    lineage: meerkat_authorization_contracts::work_association::GrantLineageRef,
}
impl GrantedInvocation {
    fn resolve(
        &self,
        requester: &PrincipalRef,
    ) -> Result<meerkat_authorization::grants::ResolvedGrant, GrantRefusal> {
        if requester != &self.requester {
            return Err(GrantRefusal::Denied);
        }
        self.grants
            .resolve_lineage(std::slice::from_ref(&self.lineage), requester, None)
    }
}
impl ScopedMandateOwner for GrantedInvocation {
    fn authorize_invocation(
        &self,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        if ingress.requester() != &association.candidate().requester {
            return Err(refused(OperationRefusalKind::Denied));
        }
        self.resolve(ingress.requester())
            .map(|_| ())
            .map_err(grant_refusal)
    }
}
impl AdmittedWorkPolicyOwner for GrantedInvocation {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        _purpose: meerkat_authorization::policy::LocalPolicyPurpose,
        _now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::WorkOwnerAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        match self.resolve(&association.candidate().requester) {
            Ok(grant) => Ok(meerkat_authorization::grant_policy::WorkOwnerAllowance {
                restrictions: grant.restrictions().clone(),
                expires_at_ms: grant.expires_at_ms(),
            }),
            Err(GrantRefusal::Denied) => {
                Err(OperationRefused::new(OperationRefusalKind::Denied).into())
            }
            Err(_) => Err(meerkat_core::OperationAuthorizationError::Unavailable),
        }
    }
}

struct AnyCustody;
impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody
    for AnyCustody
{
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _reference: &meerkat_authorization_contracts::work_association::GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<
        Result<T, E>,
        meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal,
    > {
        Ok(mutate())
    }
}

fn read_binding() -> meerkat_core::authorization::PreparedAuthorizationBinding {
    use meerkat_core::authorization::{
        AuthorizationOperation, OperationAuthorizationFacts, SourceAuthorizationFacts,
        SourceAuthorizationTarget, SourceAuthorizationUse,
    };
    meerkat_core::authorization::PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: meerkat_core::OperationId::new(),
        execution_scope: meerkat_core::exact_operation::OperationExecutionScope::Domain,
        run_id: None,
        context_revision: None,
        operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::Memory(
                meerkat_core::memory::MemorySearchScope::for_session(meerkat_core::SessionId::new()),
            ),
            usage: SourceAuthorizationUse::Read,
        }),
    })
}

/// The composition installs the admitted-work face of the same policy as the
/// native invocation owner and requires a current scoped destination; when
/// the host policy changes (its invocation grant is revoked), both the
/// admission face and the admitted-work face refuse.
#[test]
fn both_faces_of_the_invocation_policy_follow_one_grant_publication() {
    let f = fixture();
    let lineage = f
        .grants
        .issue_root(
            &f.root,
            id("invocation"),
            f.claims.requester.clone(),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("real issued invocation grant");
    let policy = Arc::new(GrantedInvocation {
        grants: Arc::clone(&f.grants),
        requester: f.claims.requester.clone(),
        lineage: lineage.clone(),
    });
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&f.grants),
        ingress_domain: f.claims.ingress_namespace.ingress.clone(),
        originals: f.originals.clone(),
        invocation: ScopedInvocation::from_policy(Arc::clone(&policy)),
        operation_owner: Arc::new(NoOperations),
        retained_resume: None,
    }
    .into_configuration();
    assert!(configuration.require_scoped_destination);
    let association = InputAuthorityAssociation::new(f.claims.clone()).expect("claims");
    let binding = read_binding();
    let admitted = |configuration: &NativeGrantWorkConfiguration| {
        configuration.invocation_owner.authorize_admitted_work(
            &association,
            &binding,
            meerkat_authorization::policy::LocalPolicyPurpose::Operation,
            100,
        )
    };

    assert!((configuration.ingress)(&f.runtime, &f.input, &f.ingress, &association).is_ok());
    assert!(admitted(&configuration).is_ok());

    f.grants
        .revoke(&f.root, &lineage, &mut AnyCustody)
        .expect("the host revokes the invocation grant");
    assert_eq!(
        refusal((configuration.ingress)(
            &f.runtime,
            &f.input,
            &f.ingress,
            &association
        )),
        Some(OperationRefusalKind::Denied),
        "the admission face refuses"
    );
    assert!(
        matches!(
            admitted(&configuration),
            Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == OperationRefusalKind::Denied
        ),
        "the installed admitted-work face refuses"
    );
}

struct NoOperations;
impl OperationPolicyOwner for NoOperations {
    fn authorize_operation(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        _purpose: meerkat_authorization::policy::LocalPolicyPurpose,
        _now_ms: u64,
    ) -> Result<
        meerkat_authorization::policy::LocalPolicyAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    }
}

/// Every named contributor is an actually admitted original of this
/// requester in the source owner's custody, and its own mandate is a
/// conjunct: a contributor the adapter does not hold is refused, and a
/// contributor whose independently issued grant is revoked refuses the
/// original that names it while the original's own mandate still resolves.
#[test]
fn every_contributor_is_an_admitted_original_whose_grant_is_a_conjunct() {
    let f = fixture();
    let contributor = f.claims.contributing_work[1].clone();
    let lineage = f
        .grants
        .issue_root(
            &f.root,
            id("contributor-leaf"),
            f.claims.logical_executor.clone(),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("an independently issued contributor grant");
    f.originals.contributors.lock().expect("fixture").insert(
        contributor.work.clone(),
        contributor_custody(
            &f.claims,
            &contributor,
            WorkAuthorityBasis::GrantLineage {
                lineage: vec![lineage.clone()],
            },
        ),
    );
    assert!(
        f.admit(f.claims.clone()).is_ok(),
        "both contributors are held and their mandates resolve"
    );

    f.grants
        .revoke(&f.root, &lineage, &mut AnyCustody)
        .expect("the source owner's grant for one contributor is revoked");
    assert_eq!(
        refusal(f.admit(f.claims.clone())),
        Some(OperationRefusalKind::Denied),
        "a revoked contributor grant refuses the original"
    );

    f.originals
        .contributors
        .lock()
        .expect("fixture")
        .remove(&contributor.work);
    assert_eq!(
        refusal(f.admit(f.claims.clone())),
        Some(OperationRefusalKind::MalformedFacts),
        "a contributor the adapter does not hold is not an admitted original"
    );
}

/// The executor a helper's source original was granted to, other than the
/// helper's own.
fn source_executor() -> PrincipalRef {
    principal("source-executor")
}

/// The subject the source's grant represents, other than the helper's.
fn source_subject() -> PrincipalRef {
    principal("source-person")
}

impl Fixture {
    /// A root grant to the source's executor for the source's subject.
    fn issue_source_grant(
        &self,
        grant: &str,
        restrictions: ExecutionRestrictions,
    ) -> GrantLineageRef {
        self.grants
            .issue_root(
                &self.root,
                id(grant),
                source_executor(),
                Some(source_subject()),
                restrictions,
            )
            .expect("the source's own grant")
    }

    /// The source owner holds `work` as an admitted original under
    /// `lineage`.
    fn hold_contributor(&self, work: &OriginalWorkRef, lineage: Vec<GrantLineageRef>) {
        self.originals.contributors.lock().expect("fixture").insert(
            work.work.clone(),
            contributor_custody(
                &self.claims,
                work,
                WorkAuthorityBasis::GrantLineage { lineage },
            ),
        );
    }

    /// A helper's claims: the fixture's executor (not the source's) for its
    /// own represented subject, under its own grant, which the main
    /// original's custody holds.
    fn helper_claims(&self) -> InputAuthorityAssociationCandidate {
        self.helper_claims_under(ExecutionRestrictions::unrestricted())
    }

    /// [`Self::helper_claims`] whose own grant is bounded by `restrictions`.
    fn helper_claims_under(
        &self,
        restrictions: ExecutionRestrictions,
    ) -> InputAuthorityAssociationCandidate {
        let subject = principal("helper-person");
        let own = self
            .grants
            .issue_root(
                &self.root,
                id("helper-leaf"),
                self.claims.logical_executor.clone(),
                Some(subject.clone()),
                restrictions,
            )
            .expect("the helper's own grant");
        let mut helper = self.claims.clone();
        helper.represented_subject = Some(subject);
        helper.authority_basis = WorkAuthorityBasis::GrantLineage { lineage: vec![own] };
        self.set_basis(&helper.authority_basis);
        helper
    }
}

/// A helper, admitted under its own executor and represented subject, names
/// its source's admitted original, whose grant was issued to the source's
/// executor for the source's subject. At admission that contributor's
/// conjunct is the liveness of its lineage for its own leaf identity: the
/// helper admits, though the lineage does not resolve for the helper.
/// Liveness is not operation authority: an operation of the helper's work
/// resolves the contributor's lineage for the helper and is refused.
#[test]
fn a_live_contributor_of_another_executor_and_subject_admits() {
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let lineage = vec![f.issue_source_grant("source-leaf", ExecutionRestrictions::unrestricted())];
    f.hold_contributor(&source, lineage.clone());
    let helper = f.helper_claims();
    assert_ne!(helper.logical_executor, source_executor());
    assert_ne!(helper.represented_subject, Some(source_subject()));
    assert_eq!(
        f.grants
            .resolve_lineage(
                &lineage,
                &helper.logical_executor,
                helper.represented_subject.as_ref()
            )
            .err(),
        Some(GrantRefusal::Denied),
        "the source's lineage is not the helper's"
    );

    assert!(
        f.admit(helper.clone()).is_ok(),
        "the live contributor admits the helper"
    );

    let operation = |executor: &PrincipalRef, subject: Option<&PrincipalRef>| {
        original_operation_grant(
            &f.grants,
            f.originals.as_ref(),
            &helper.ingress_namespace,
            &source,
            &read_binding(),
            executor,
            subject,
        )
    };
    assert!(
        matches!(
            operation(&helper.logical_executor, helper.represented_subject.as_ref()),
            Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == OperationRefusalKind::Denied
        ),
        "an operation of the helper's work still resolves the contributor for the helper"
    );
    assert!(
        matches!(
            operation(&source_executor(), Some(&source_subject())),
            Ok(Some(_))
        ),
        "the lineage is operation authority for its own executor and subject only"
    );
}

/// Liveness keeps every local refusal of a contributor, with the same
/// refusal as before: a revoked or expired lineage, a cancelled original,
/// and a reference to another grant owner, namespace or generation are
/// denied; a contributor its source owner no longer retains is not an
/// admitted original.
#[test]
fn a_contributor_that_is_not_live_refuses_locally() {
    let denied = Some(OperationRefusalKind::Denied);
    let unrestricted = ExecutionRestrictions::unrestricted;

    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let lineage = f.issue_source_grant("source-leaf", unrestricted());
    f.hold_contributor(&source, vec![lineage.clone()]);
    let helper = f.helper_claims();
    assert!(
        f.admit(helper.clone()).is_ok(),
        "live before the revocation"
    );
    f.grants
        .revoke(&f.root, &lineage, &mut AnyCustody)
        .expect("the source's grant is revoked");
    assert_eq!(refusal(f.admit(helper)), denied, "a revoked lineage");

    // The grant owner's time is 100: this window has ended.
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let expired = f.issue_source_grant(
        "source-leaf",
        ExecutionRestrictions {
            lifetime: LifetimeRestriction::window(0, 100),
            ..unrestricted()
        },
    );
    f.hold_contributor(&source, vec![expired]);
    assert_eq!(
        refusal(f.admit(f.helper_claims())),
        denied,
        "an expired lineage"
    );

    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let live = f.issue_source_grant("source-leaf", unrestricted());
    f.hold_contributor(&source, vec![live]);
    let helper = f.helper_claims();
    assert!(
        f.admit(helper.clone()).is_ok(),
        "live before the cancellation"
    );
    f.originals
        .cancelled
        .lock()
        .expect("fixture")
        .insert(source.work.clone(), fixture_evidence(&f, "cancellation"));
    assert_eq!(
        refusal(f.admit(helper.clone())),
        denied,
        "a cancelled contributor"
    );
    f.originals.cancelled.lock().expect("fixture").clear();
    f.originals
        .released
        .lock()
        .expect("fixture")
        .insert(source.work.clone());
    assert_eq!(
        refusal(f.admit(helper)),
        Some(OperationRefusalKind::MalformedFacts),
        "a contributor its source owner no longer retains"
    );

    // Another owner of the same configuration, root and grant id: only its
    // incarnation differs.
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let live = f.issue_source_grant("source-leaf", unrestricted());
    let other = LocalGrantAuthority::new(
        LocalGrantConfiguration {
            root: f.root.clone(),
            namespace: id("work"),
            generation: 1,
        },
        LocalAuthorizationPublication::new(),
        Arc::new(Clock),
    )
    .expect("another grant owner");
    let foreign = other
        .issue_root(
            &f.root,
            id("source-leaf"),
            source_executor(),
            Some(source_subject()),
            unrestricted(),
        )
        .expect("the other owner's grant");
    assert_eq!(
        other.require_live_lineage(std::slice::from_ref(&foreign)),
        Ok(()),
        "live in its own owner"
    );
    let helper = f.helper_claims();
    for (label, reference) in [
        ("another grant owner", foreign),
        (
            "another namespace",
            GrantLineageRef {
                authority_namespace: id("other-work"),
                ..live.clone()
            },
        ),
        (
            "another generation",
            GrantLineageRef {
                authority_generation: live.authority_generation + 1,
                ..live.clone()
            },
        ),
    ] {
        f.hold_contributor(&source, vec![reference]);
        assert_eq!(refusal(f.admit(helper.clone())), denied, "{label}");
    }
    f.hold_contributor(&source, vec![live]);
    assert!(f.admit(helper).is_ok(), "the exact reference is live");
}

/// The admitted original's own mandate stays bound to the association's
/// executor and represented subject: a live lineage that admits as a
/// contributor's mandate refuses as the helper's own, and so does the
/// helper executor's own grant for another represented subject.
#[test]
fn the_admitted_originals_own_basis_stays_bound_to_its_executor_and_subject() {
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let lineage = vec![f.issue_source_grant("source-leaf", ExecutionRestrictions::unrestricted())];
    f.hold_contributor(&source, lineage.clone());
    let helper = f.helper_claims();
    assert!(
        f.admit(helper.clone()).is_ok(),
        "the live lineage as a contributor's mandate"
    );

    let mut borrowed = helper.clone();
    borrowed.authority_basis = WorkAuthorityBasis::GrantLineage { lineage };
    f.set_basis(&borrowed.authority_basis);
    assert_eq!(
        refusal(f.admit(borrowed.clone())),
        Some(OperationRefusalKind::Denied),
        "the same live lineage as the helper's own mandate"
    );
    borrowed.represented_subject = Some(source_subject());
    assert_eq!(
        refusal(f.admit(borrowed)),
        Some(OperationRefusalKind::Denied),
        "naming the source's subject does not make it the helper's"
    );

    let other_subject = f
        .grants
        .issue_root(
            &f.root,
            id("helper-other-subject"),
            helper.logical_executor.clone(),
            Some(principal("someone-else")),
            ExecutionRestrictions::unrestricted(),
        )
        .expect("the helper executor's grant for another subject");
    let mut misbound = helper;
    misbound.authority_basis = WorkAuthorityBasis::GrantLineage {
        lineage: vec![other_subject],
    };
    f.set_basis(&misbound.authority_basis);
    assert_eq!(
        refusal(f.admit(misbound)),
        Some(OperationRefusalKind::Denied),
        "the helper's executor, but another represented subject"
    );
}

/// A host invocation policy that admits and allows the work without
/// narrowing it, so an operation's bounds are its other owners'.
struct AllowWork;
impl ScopedMandateOwner for AllowWork {
    fn authorize_invocation(
        &self,
        _ingress: &NativeIngressContext,
        _association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        Ok(())
    }
}
impl AdmittedWorkPolicyOwner for AllowWork {
    fn authorize_admitted_work(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::WorkOwnerAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        Ok(meerkat_authorization::grant_policy::WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: u64::MAX,
        })
    }
}

/// The operation owner of exactly one actual operation tuple.
struct ExactOperation(LocalOperationValues);
impl OperationPolicyOwner for ExactOperation {
    fn authorize_operation(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        Ok(LocalPolicyAllowance {
            operation_values: vec![self.0.clone()],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: u64::MAX,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }

    /// The fixture tuple has no separately selected action account.
    fn review_action_account(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<Option<String>, meerkat_core::OperationAuthorizationError> {
        Ok(None)
    }
}

/// A host invocation policy that allows the work and holds a current
/// mandate for it.
struct MandateWork;
impl ScopedMandateOwner for MandateWork {
    fn authorize_invocation(
        &self,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        AllowWork.authorize_invocation(ingress, association)
    }
}
impl AdmittedWorkPolicyOwner for MandateWork {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<
        meerkat_authorization::grant_policy::WorkOwnerAllowance,
        meerkat_core::OperationAuthorizationError,
    > {
        AllowWork.authorize_admitted_work(association, binding, purpose, now_ms)
    }

    fn review_mandate(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<Option<String>, meerkat_core::OperationAuthorizationError> {
        Ok(Some(POLICY_MANDATE.to_owned()))
    }
}

const POLICY_MANDATE: &str = "Prepare the setup report in the helper's own session";

fn report_action(name: &str) -> ActionRef {
    ActionRef {
        feature: "helper-test".into(),
        action: name.into(),
    }
}

/// The session resource domain of `owner`.
fn session_of(owner: &str) -> ResourceDomain {
    ResourceDomain {
        authority: principal(owner),
        namespace: "session".into(),
    }
}

fn processor_of(executor: &PrincipalRef) -> ProcessorRef {
    ProcessorRef::Principal {
        principal: executor.clone(),
    }
}

/// Bound values as review text states them: in their serialized form.
fn json<T: serde::Serialize>(values: &T) -> String {
    serde_json::to_string(values).expect("serializable bound values")
}

/// Bounds on actions, sessions and processors, unrestricted otherwise.
fn bounds(
    actions: &[&str],
    sessions: &[&str],
    processors: &[PrincipalRef],
) -> ExecutionRestrictions {
    ExecutionRestrictions {
        actions: ExactRestriction::exact(actions.iter().copied().map(report_action)),
        resource_domains: DomainRestriction::exact(sessions.iter().copied().map(session_of)),
        processors: ExactRestriction::exact(processors.iter().map(processor_of)),
        ..ExecutionRestrictions::unrestricted()
    }
}

impl Fixture {
    /// The helper's operation `action` on its own session, processed by its
    /// own executor, for the original's audience.
    fn helper_operation(&self, action: &str) -> LocalOperationValues {
        LocalOperationValues {
            action: report_action(action),
            resource_domain: session_of("helper"),
            processor: processor_of(&self.claims.logical_executor),
            audience: self.claims.target.audience.clone(),
        }
    }

    /// The installed composition whose host invocation policy allows the
    /// work and whose operation owner allows exactly `operation`, with the
    /// grant-backed policy over its owners.
    fn operation_policy(
        &self,
        operation: LocalOperationValues,
    ) -> (NativeGrantWorkConfiguration, GrantBackedWorkPolicy) {
        let configuration = ScopedGrantComposition {
            grants: Arc::clone(&self.grants),
            ingress_domain: self.claims.ingress_namespace.ingress.clone(),
            originals: self.originals.clone(),
            invocation: ScopedInvocation::from_policy(Arc::new(AllowWork)),
            operation_owner: Arc::new(ExactOperation(operation)),
            retained_resume: None,
        }
        .into_configuration();
        let policy = GrantBackedWorkPolicy::new(
            Arc::clone(&self.grants),
            Arc::clone(&configuration.invocation_owner),
            Arc::clone(&configuration.operation_owner),
        );
        (configuration, policy)
    }
}

/// The grant-backed policy's Operation answer for the work of `claims`.
fn operate(
    policy: &GrantBackedWorkPolicy,
    claims: &InputAuthorityAssociationCandidate,
) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
    let association = InputAuthorityAssociation::new(claims.clone()).expect("well-formed claims");
    policy.evaluate_for(
        &association,
        &read_binding(),
        LocalPolicyPurpose::Operation,
        100,
    )
}

fn operation_refusal(
    result: Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError>,
) -> Option<OperationRefusalKind> {
    match result {
        Err(meerkat_core::OperationAuthorizationError::Refused(refusal)) => Some(refusal.kind()),
        _ => None,
    }
}

/// A contributor of another executor and subject narrows the helper's
/// operation to the intersection of its ceiling with the helper's own grant,
/// and its expiry bounds the operation. An operation the helper's own grant
/// allows outside that ceiling is refused.
#[test]
fn another_executors_contributor_narrows_the_operation_to_the_intersection() {
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let executor = f.claims.logical_executor.clone();
    let contributor = f.issue_source_grant(
        "source-leaf",
        ExecutionRestrictions {
            lifetime: LifetimeRestriction::window(0, 5_000),
            ..bounds(
                &["find-and-install", "setup-report"],
                &["source", "helper"],
                &[source_executor(), executor.clone()],
            )
        },
    );
    f.hold_contributor(&source, vec![contributor.clone()]);
    let helper = f.helper_claims_under(bounds(
        &["setup-report", "publish-report"],
        &["helper"],
        std::slice::from_ref(&executor),
    ));
    assert_eq!(
        f.grants
            .resolve_lineage(
                std::slice::from_ref(&contributor),
                &executor,
                helper.represented_subject.as_ref()
            )
            .err(),
        Some(GrantRefusal::Denied),
        "the contributor's lineage is not the helper's"
    );

    let (_, setup) = f.operation_policy(f.helper_operation("setup-report"));
    let allowance = operate(&setup, &helper).expect("the operation is within the intersection");
    assert_eq!(
        allowance.restrictions.actions.bound(),
        &SetBound::Exact([report_action("setup-report")].into())
    );
    assert_eq!(
        allowance.restrictions.resource_domains.bound(),
        &DomainBound::Exact([session_of("helper")].into())
    );
    assert_eq!(
        allowance.restrictions.processors.bound(),
        &SetBound::Exact([processor_of(&executor)].into())
    );
    assert_eq!(
        allowance.expires_at_ms, 5_000,
        "the contributor's expiry bounds the operation"
    );

    let (_, publish) = f.operation_policy(f.helper_operation("publish-report"));
    assert_eq!(
        operation_refusal(operate(&publish, &helper)),
        Some(OperationRefusalKind::Denied),
        "the helper's own grant allows it, the contributor's ceiling does not"
    );
}

/// The current helper-report shape: the source's ceiling is find-and-install
/// on the source's session by the source's executor, and the helper's
/// operation is setup-report on its own session by its own executor. The
/// ceilings are disjoint, so the operation is refused locally although the
/// helper's own grant allows it.
#[test]
fn disjoint_contributor_and_helper_ceilings_refuse_the_operation_locally() {
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let executor = f.claims.logical_executor.clone();
    let contributor = f.issue_source_grant(
        "source-leaf",
        bounds(&["find-and-install"], &["source"], &[source_executor()]),
    );
    f.hold_contributor(&source, vec![contributor]);
    let helper = f.helper_claims_under(bounds(&["setup-report"], &["helper"], &[executor]));
    let (_, setup) = f.operation_policy(f.helper_operation("setup-report"));
    assert_eq!(
        operation_refusal(operate(&setup, &helper)),
        Some(OperationRefusalKind::Denied)
    );
}

/// A contributor's ceiling never supplies the helper's own authority: with
/// a ceiling that covers the operation, a helper with no operation grant of
/// its own, or naming the contributor's lineage as its own, is refused.
#[test]
fn a_contributor_ceiling_never_supplies_the_helpers_own_authority() {
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let executor = f.claims.logical_executor.clone();
    let covering = bounds(&["setup-report"], &["helper"], &[executor]);
    let contributor = f.issue_source_grant("source-leaf", covering.clone());
    f.hold_contributor(&source, vec![contributor.clone()]);
    let (_, setup) = f.operation_policy(f.helper_operation("setup-report"));
    let mut helper = f.helper_claims_under(covering);
    assert!(
        operate(&setup, &helper).is_ok(),
        "with its own grant the operation proceeds"
    );

    helper.authority_basis = WorkAuthorityBasis::TargetedGrantLineages {
        base: None,
        targets: Vec::new(),
    };
    assert_eq!(
        operation_refusal(operate(&setup, &helper)),
        Some(OperationRefusalKind::Denied),
        "no operation grant of its own"
    );
    helper.authority_basis = WorkAuthorityBasis::GrantLineage {
        lineage: vec![contributor],
    };
    assert_eq!(
        operation_refusal(operate(&setup, &helper)),
        Some(OperationRefusalKind::Denied),
        "the contributor's lineage named as its own"
    );
}

/// An operation refuses on a contributor that is no longer current: a
/// revoked or expired lineage, another grant owner's or a replaced
/// reference, a cancelled or no longer retained original are denied, and a
/// source owner that cannot say now stays unavailable.
#[test]
fn an_operation_refuses_on_a_contributor_that_is_no_longer_current() {
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let executor = f.claims.logical_executor.clone();
    let covering = || {
        bounds(
            &["setup-report"],
            &["helper"],
            std::slice::from_ref(&executor),
        )
    };
    let denied = Some(OperationRefusalKind::Denied);
    let live = f.issue_source_grant("source-live", covering());
    let revoked = f.issue_source_grant("source-revoked", covering());
    f.grants
        .revoke(&f.root, &revoked, &mut AnyCustody)
        .expect("revoked");
    // The grant owner's time is 100: this window has ended.
    let expired = f.issue_source_grant(
        "source-expired",
        ExecutionRestrictions {
            lifetime: LifetimeRestriction::window(0, 100),
            ..covering()
        },
    );
    let other = LocalGrantAuthority::new(
        LocalGrantConfiguration {
            root: f.root.clone(),
            namespace: id("work"),
            generation: 1,
        },
        LocalAuthorizationPublication::new(),
        Arc::new(Clock),
    )
    .expect("another grant owner");
    let foreign = other
        .issue_root(
            &f.root,
            id("source-live"),
            source_executor(),
            Some(source_subject()),
            covering(),
        )
        .expect("the other owner's grant");
    let replaced = GrantLineageRef {
        issued_revision: live.issued_revision + 1,
        ..live.clone()
    };
    let helper = f.helper_claims_under(covering());
    let (_, setup) = f.operation_policy(f.helper_operation("setup-report"));

    f.hold_contributor(&source, vec![live.clone()]);
    assert!(operate(&setup, &helper).is_ok(), "the live contributor");
    for (label, reference) in [
        ("revoked", revoked),
        ("expired", expired),
        ("another grant owner", foreign),
        ("a replaced reference", replaced),
    ] {
        f.hold_contributor(&source, vec![reference]);
        assert_eq!(
            operation_refusal(operate(&setup, &helper)),
            denied,
            "{label}"
        );
    }

    f.hold_contributor(&source, vec![live]);
    f.originals
        .cancelled
        .lock()
        .expect("fixture")
        .insert(source.work.clone(), fixture_evidence(&f, "cancellation"));
    assert_eq!(
        operation_refusal(operate(&setup, &helper)),
        denied,
        "cancelled"
    );
    f.originals.cancelled.lock().expect("fixture").clear();
    f.originals
        .released
        .lock()
        .expect("fixture")
        .insert(source.work.clone());
    assert_eq!(
        operation_refusal(operate(&setup, &helper)),
        denied,
        "no longer retained"
    );
    f.originals.released.lock().expect("fixture").clear();
    f.originals
        .unreadable
        .lock()
        .expect("fixture")
        .insert(source.work.clone());
    assert!(
        matches!(
            operate(&setup, &helper),
            Err(meerkat_core::OperationAuthorizationError::Unavailable)
        ),
        "an owner that cannot say now is unavailable, not denied"
    );
    f.originals.unreadable.lock().expect("fixture").clear();
    assert!(operate(&setup, &helper).is_ok(), "current again");
}

/// For a contributor of the work's own executor and subject, its ceiling is
/// exactly its resolution for that executor, so the operation is narrowed as
/// before.
#[test]
fn a_same_executor_contributor_ceiling_is_its_executor_resolution() {
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let executor = f.claims.logical_executor.clone();
    let subject = principal("helper-person");
    let same = f
        .grants
        .issue_root(
            &f.root,
            id("same-executor-leaf"),
            executor.clone(),
            Some(subject.clone()),
            bounds(
                &["find-and-install"],
                &["helper"],
                std::slice::from_ref(&executor),
            ),
        )
        .expect("a grant to the work's own executor");
    let ceiling = f
        .grants
        .resolve_lineage_ceiling(std::slice::from_ref(&same))
        .expect("its ceiling");
    let resolved = f
        .grants
        .resolve_lineage(std::slice::from_ref(&same), &executor, Some(&subject))
        .expect("its resolution for the executor");
    assert_eq!(ceiling.restrictions(), resolved.restrictions());
    assert_eq!(ceiling.expires_at_ms(), resolved.expires_at_ms());

    f.hold_contributor(&source, vec![same]);
    let helper = f.helper_claims();
    let (_, install) = f.operation_policy(f.helper_operation("find-and-install"));
    assert!(operate(&install, &helper).is_ok(), "within its ceiling");
    let (_, setup) = f.operation_policy(f.helper_operation("setup-report"));
    assert_eq!(
        operation_refusal(operate(&setup, &helper)),
        Some(OperationRefusalKind::Denied),
        "outside its ceiling"
    );
}

/// A disjoint contributor narrows only the Operation purpose of the work
/// that names it: the controller purpose is not narrowed by contributors,
/// and a sibling work of the same helper that does not name it is still
/// permitted.
#[test]
fn a_disjoint_contributor_leaves_the_controller_purpose_and_a_sibling_unaffected() {
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let executor = f.claims.logical_executor.clone();
    let contributor = f.issue_source_grant(
        "source-leaf",
        bounds(&["find-and-install"], &["source"], &[source_executor()]),
    );
    f.hold_contributor(&source, vec![contributor]);
    let helper = f.helper_claims_under(bounds(&["setup-report"], &["helper"], &[executor]));
    let (configuration, setup) = f.operation_policy(f.helper_operation("setup-report"));
    assert_eq!(
        operation_refusal(operate(&setup, &helper)),
        Some(OperationRefusalKind::Denied),
        "the operation of the work naming it"
    );

    let association = InputAuthorityAssociation::new(helper.clone()).expect("claims");
    let controller = configuration
        .invocation_owner
        .authorize_admitted_work(
            &association,
            &read_binding(),
            LocalPolicyPurpose::Controller,
            100,
        )
        .expect("the controller purpose is the host policy's answer");
    assert_eq!(
        controller.restrictions,
        ExecutionRestrictions::unrestricted(),
        "not narrowed by contributors"
    );
    assert_eq!(controller.expires_at_ms, u64::MAX);

    let mut sibling = helper;
    sibling.original_work.work = id("sibling-work");
    sibling.contributing_work = vec![f.claims.contributing_work[0].clone()];
    assert!(
        operate(&setup, &sibling).is_ok(),
        "a sibling work that does not name it is permitted"
    );
}

/// The installed work owner forwards the host invocation policy's review
/// mandate under the same checks as an operation of the work, followed by
/// its contributor's current ceiling: the grant-backed review context
/// carries it while the work's own original and its contributor are
/// current; a cancelled original and a contributor its owner no longer
/// retains are denied, and an owner that cannot say now is unavailable.
#[test]
fn the_installed_work_owner_forwards_the_policy_mandate_under_the_contributor_checks() {
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let executor = f.claims.logical_executor.clone();
    let covering = bounds(
        &["setup-report"],
        &["helper"],
        std::slice::from_ref(&executor),
    );
    let contributor = f.issue_source_grant("source-leaf", covering.clone());
    f.hold_contributor(&source, vec![contributor]);
    let helper = f.helper_claims_under(covering);
    let association = InputAuthorityAssociation::new(helper.clone()).expect("claims");
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&f.grants),
        ingress_domain: f.claims.ingress_namespace.ingress.clone(),
        originals: f.originals.clone(),
        invocation: ScopedInvocation::from_policy(Arc::new(MandateWork)),
        operation_owner: Arc::new(ExactOperation(f.helper_operation("setup-report"))),
        retained_resume: None,
    }
    .into_configuration();
    let policy = GrantBackedWorkPolicy::new(
        Arc::clone(&f.grants),
        Arc::clone(&configuration.invocation_owner),
        Arc::clone(&configuration.operation_owner),
    );
    let mandate = || {
        configuration
            .invocation_owner
            .review_mandate(&association, &read_binding(), 100)
    };
    let narrowed = format!(
        "{POLICY_MANDATE} Contributing originals narrow it to: actions {}; resources {}; \
         processors {}; audiences any; effective until no expiry.",
        json(&[report_action("setup-report")]),
        json(&[session_of("helper")]),
        json(&[processor_of(&executor)]),
    );
    assert_eq!(
        mandate().expect("the policy's mandate is forwarded"),
        Some(narrowed.clone())
    );
    assert_eq!(
        policy
            .review_context(&association, &read_binding(), 100)
            .expect("the review material"),
        meerkat_core::authorization::ReviewOperationContextResolution::Material(
            meerkat_core::authorization::ReviewOperationContext::new(None, Some(narrowed.clone()))
                .expect("bounded material")
        )
    );

    let denied = |label: &str| {
        assert!(
            matches!(
                mandate(),
                Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                    if refusal.kind() == OperationRefusalKind::Denied
            ),
            "{label}"
        );
    };
    f.originals.cancelled.lock().expect("fixture").insert(
        helper.original_work.work.clone(),
        fixture_evidence(&f, "cancellation"),
    );
    denied("the work's own original is cancelled");
    f.originals.cancelled.lock().expect("fixture").clear();
    f.originals
        .released
        .lock()
        .expect("fixture")
        .insert(source.work.clone());
    denied("its contributor is no longer retained");
    f.originals.released.lock().expect("fixture").clear();
    f.originals
        .unreadable
        .lock()
        .expect("fixture")
        .insert(source.work.clone());
    assert!(
        matches!(
            mandate(),
            Err(meerkat_core::OperationAuthorizationError::Unavailable)
        ),
        "an owner that cannot say now is unavailable, not denied"
    );
    f.originals.unreadable.lock().expect("fixture").clear();
    assert_eq!(mandate().expect("current again"), Some(narrowed));
}

/// A mandate its contributor's narrowing takes past 4 KiB, the former bound
/// of each review context field, is the work's review material in full: a
/// contributor whose ceiling names many sessions is stated with every one of
/// them. A narrowed mandate past the field bound itself is unavailable, never
/// truncated.
#[test]
fn a_contributor_narrowed_mandate_past_four_kib_is_material_and_past_the_bound_unavailable() {
    use meerkat_core::authorization::{
        MAX_REVIEW_OPERATION_CONTEXT_BYTES, ReviewOperationContextResolution,
    };
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let association = InputAuthorityAssociation::new(f.helper_claims()).expect("claims");
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&f.grants),
        ingress_domain: f.claims.ingress_namespace.ingress.clone(),
        originals: f.originals.clone(),
        invocation: ScopedInvocation::from_policy(Arc::new(MandateWork)),
        operation_owner: Arc::new(ExactOperation(f.helper_operation("setup-report"))),
        retained_resume: None,
    }
    .into_configuration();
    let policy = GrantBackedWorkPolicy::new(
        Arc::clone(&f.grants),
        Arc::clone(&configuration.invocation_owner),
        Arc::clone(&configuration.operation_owner),
    );
    // The contributor is held under a grant whose ceiling names `sessions`
    // sessions; the work's review mandate is the policy's, narrowed by it.
    let narrowed_over = |grant: &str, sessions: usize| {
        let ceiling = ExecutionRestrictions {
            resource_domains: DomainRestriction::exact(
                (0..sessions).map(|index| session_of(&format!("helper-{index:05}"))),
            ),
            ..ExecutionRestrictions::unrestricted()
        };
        f.hold_contributor(&source, vec![f.issue_source_grant(grant, ceiling)]);
        configuration
            .invocation_owner
            .review_mandate(&association, &read_binding(), 100)
            .expect("the narrowed mandate")
            .expect("the policy states a mandate")
    };

    let past_four_kib = narrowed_over("source-leaf", 128);
    assert!(
        past_four_kib.starts_with(POLICY_MANDATE) && past_four_kib.len() > 4 * 1024,
        "the narrowed mandate is past 4 KiB: {} bytes",
        past_four_kib.len()
    );
    let material = policy.review_context(&association, &read_binding(), 100);
    assert!(
        matches!(
            &material,
            Ok(ReviewOperationContextResolution::Material(context))
                if context.mandate() == Some(past_four_kib.as_str())
                    && context.action_account().is_none()
        ),
        "a {}-byte narrowed mandate is review material, in full: {material:?}",
        past_four_kib.len()
    );

    // Every serialized session is longer than 64 bytes.
    let past_the_bound = narrowed_over(
        "source-oversized-leaf",
        MAX_REVIEW_OPERATION_CONTEXT_BYTES / 64,
    );
    assert!(
        past_the_bound.len() > MAX_REVIEW_OPERATION_CONTEXT_BYTES,
        "the narrowed mandate is past the field bound: {} bytes",
        past_the_bound.len()
    );
    let unavailable = policy.review_context(&association, &read_binding(), 100);
    assert!(
        matches!(
            &unavailable,
            Err(meerkat_core::OperationAuthorizationError::Unavailable)
        ),
        "a mandate past the field bound is unavailable, never truncated: {unavailable:?}"
    );
}

/// The review material states what the contributors' current ceilings
/// leave of the work: the policy's mandate is followed by the conjunction
/// of every contributor's ceiling and the earliest of their expiries, so a
/// work naming other contributors is presented with other bounds, and a
/// work naming none with the mandate alone.
#[test]
fn the_contributor_ceilings_narrow_the_review_material() {
    let f = fixture();
    let executor = f.claims.logical_executor.clone();
    let first = f.claims.contributing_work[0].clone();
    let second = f.claims.contributing_work[1].clone();
    let wide = ExecutionRestrictions {
        lifetime: LifetimeRestriction::window(0, 200),
        ..bounds(
            &["setup-report", "find-and-install"],
            &["helper", "source"],
            std::slice::from_ref(&executor),
        )
    };
    let narrow = ExecutionRestrictions {
        lifetime: LifetimeRestriction::window(50, 150),
        ..bounds(
            &["setup-report"],
            &["helper"],
            std::slice::from_ref(&executor),
        )
    };
    f.hold_contributor(&first, vec![f.issue_source_grant("source-a-leaf", wide)]);
    f.hold_contributor(&second, vec![f.issue_source_grant("source-b-leaf", narrow)]);
    let helper = f.helper_claims();
    let configuration = ScopedGrantComposition {
        grants: Arc::clone(&f.grants),
        ingress_domain: f.claims.ingress_namespace.ingress.clone(),
        originals: f.originals.clone(),
        invocation: ScopedInvocation::from_policy(Arc::new(MandateWork)),
        operation_owner: Arc::new(ExactOperation(f.helper_operation("setup-report"))),
        retained_resume: None,
    }
    .into_configuration();
    let mandate = |claims: &InputAuthorityAssociationCandidate| {
        configuration.invocation_owner.review_mandate(
            &InputAuthorityAssociation::new(claims.clone()).expect("claims"),
            &read_binding(),
            100,
        )
    };
    let processors = json(&[processor_of(&executor)]);
    assert_eq!(
        mandate(&helper),
        Ok(Some(format!(
            "{POLICY_MANDATE} Contributing originals narrow it to: actions {}; resources {}; \
             processors {processors}; audiences any; effective until \
             1970-01-01T00:00:00.150Z.",
            json(&[report_action("setup-report")]),
            json(&[session_of("helper")]),
        ))),
        "both contributors: their conjunction and earliest expiry"
    );
    let mut only_wide = helper.clone();
    only_wide.contributing_work = vec![first];
    assert_eq!(
        mandate(&only_wide),
        Ok(Some(format!(
            "{POLICY_MANDATE} Contributing originals narrow it to: actions {}; resources {}; \
             processors {processors}; audiences any; effective until \
             1970-01-01T00:00:00.200Z.",
            json(&std::collections::BTreeSet::from([
                report_action("find-and-install"),
                report_action("setup-report"),
            ])),
            json(&std::collections::BTreeSet::from([
                session_of("helper"),
                session_of("source"),
            ])),
        ))),
        "the wider contributor alone"
    );
    let mut none = helper;
    none.contributing_work = Vec::new();
    assert_eq!(
        mandate(&none),
        Ok(Some(POLICY_MANDATE.to_owned())),
        "no contributor"
    );
}

/// An owner-scoped resource bound is stated as its owners, each in its
/// serialized form, never as "any". It never permits nothing by itself; met
/// with another authority's exact domain it does, and is refused as at
/// authorization.
#[test]
fn the_review_bounds_text_states_an_owner_scoped_resource_bound() {
    let owner = principal("owner");
    let owned = ExecutionRestrictions {
        resource_domains: DomainRestriction::owned_by([owner.clone()]),
        ..ExecutionRestrictions::unrestricted()
    };
    assert_eq!(
        review_bounds_text(&owned, u64::MAX, 100),
        Ok(format!(
            "actions any; resources owned by {}; processors any; audiences any; effective \
             until no expiry.",
            json(&[owner])
        ))
    );
    let foreign = ExecutionRestrictions {
        resource_domains: owned
            .resource_domains
            .conjoin(&DomainRestriction::exact([session_of("someone-else")])),
        ..ExecutionRestrictions::unrestricted()
    };
    assert!(matches!(
        review_bounds_text(&foreign, u64::MAX, 100),
        Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == OperationRefusalKind::Denied
    ));
}

/// The bounds text states every bounded value exactly, in its serialized
/// form, and the effective expiry, the earlier of the given expiry and the
/// lifetime window's end; bounds that permit nothing now, or carry an
/// unresolved fact, are refused as at authorization. A principal is stated
/// completely (kind, id and qualification): the same id in another trust
/// domain is another principal and reads differently.
#[test]
fn the_review_bounds_text_states_exact_bounds_and_refuses_bounds_that_permit_nothing() {
    let owner = principal("owner");
    assert_eq!(
        review_bounds_text(&ExecutionRestrictions::unrestricted(), u64::MAX, 100).as_deref(),
        Ok(
            "actions any; resources any; processors any; audiences any; effective until no \
             expiry."
        )
    );
    let bounded = ExecutionRestrictions {
        actions: ExactRestriction::exact([report_action("write"), report_action("read")]),
        resource_domains: DomainRestriction::exact([session_of("owner")]),
        processors: ExactRestriction::exact([ProcessorRef::Route {
            authority: owner.clone(),
            route_id: "model-a".into(),
        }]),
        audiences: ExactRestriction::exact([
            AudienceRef::Destination {
                authority: owner,
                destination_id: "inbox".into(),
            },
            AudienceRef::Principal {
                principal: principal("reader"),
            },
        ]),
        lifetime: LifetimeRestriction::window(0, 1_000),
        ..ExecutionRestrictions::unrestricted()
    };
    let stated = |until: &str| {
        format!(
            "actions {}; resources {}; processors {}; audiences {}; effective until {until}.",
            json(&std::collections::BTreeSet::from([
                report_action("read"),
                report_action("write"),
            ])),
            json(&[session_of("owner")]),
            json(&[ProcessorRef::Route {
                authority: principal("owner"),
                route_id: "model-a".into(),
            }]),
            json(&std::collections::BTreeSet::from([
                AudienceRef::Principal {
                    principal: principal("reader"),
                },
                AudienceRef::Destination {
                    authority: principal("owner"),
                    destination_id: "inbox".into(),
                },
            ])),
        )
    };
    assert_eq!(
        review_bounds_text(&bounded, 2_000, 100),
        Ok(stated("1970-01-01T00:00:01.000Z")),
        "the lifetime window ends first"
    );
    assert_eq!(
        review_bounds_text(&bounded, 500, 100),
        Ok(stated("1970-01-01T00:00:00.500Z")),
        "the given expiry comes first"
    );

    let agent_in = |domain: &str| {
        PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            "agent",
            TrustDomainId::new(domain).expect("domain"),
        )
        .expect("qualified")
    };
    let processing = |principals: &[PrincipalRef]| ExecutionRestrictions {
        processors: ExactRestriction::exact(principals.iter().map(processor_of)),
        ..ExecutionRestrictions::unrestricted()
    };
    let in_a =
        review_bounds_text(&processing(&[agent_in("domain-a")]), u64::MAX, 100).expect("stated");
    assert_eq!(
        in_a,
        r#"actions any; resources any; processors [{"processor":"principal","principal":{"kind":"service_account","id":"agent","qualification":{"kind":"qualified","trust_domain_id":"domain-a"}}}]; audiences any; effective until no expiry."#
    );
    let in_b =
        review_bounds_text(&processing(&[agent_in("domain-b")]), u64::MAX, 100).expect("stated");
    assert_ne!(
        in_a, in_b,
        "the same id in another domain is another principal"
    );
    assert_eq!(
        review_bounds_text(
            &processing(&[agent_in("domain-a"), agent_in("domain-b")]),
            u64::MAX,
            100
        ),
        Ok(format!(
            "actions any; resources any; processors {}; audiences any; effective until no \
             expiry.",
            json(&std::collections::BTreeSet::from([
                processor_of(&agent_in("domain-a")),
                processor_of(&agent_in("domain-b")),
            ])),
        )),
        "both are stated, each with its own domain"
    );

    let denied = |label: &str, restrictions: &ExecutionRestrictions, expires_at_ms, now_ms| {
        assert!(
            matches!(
                review_bounds_text(restrictions, expires_at_ms, now_ms),
                Err(meerkat_core::OperationAuthorizationError::Refused(refusal))
                    if refusal.kind() == OperationRefusalKind::Denied
            ),
            "{label}"
        );
    };
    denied("an ended window", &bounded, 2_000, 1_000);
    denied("a passed expiry", &bounded, 500, 500);
    for (label, change) in [
        (
            "a window not yet open",
            Box::new(|r: &mut ExecutionRestrictions| {
                r.lifetime = LifetimeRestriction::window(200, 1_000);
            }) as Box<dyn Fn(&mut ExecutionRestrictions)>,
        ),
        (
            "an empty lifetime",
            Box::new(|r: &mut ExecutionRestrictions| {
                r.lifetime = LifetimeRestriction::window(5, 5);
            }),
        ),
        (
            "an empty action set",
            Box::new(|r: &mut ExecutionRestrictions| {
                r.actions = ExactRestriction::exact([]);
            }),
        ),
        (
            "an unresolved processor",
            Box::new(|r: &mut ExecutionRestrictions| {
                r.processors = ExactRestriction::unresolved(UnresolvedConstraint::Unknown);
            }),
        ),
        (
            "an unresolved lifetime",
            Box::new(|r: &mut ExecutionRestrictions| {
                r.lifetime = LifetimeRestriction::unresolved(UnresolvedConstraint::Absent);
            }),
        ),
        (
            "an unresolved delegation depth",
            Box::new(|r: &mut ExecutionRestrictions| {
                r.delegation_depth = DelegationDepth::unresolved(UnresolvedConstraint::Unavailable);
            }),
        ),
    ] {
        let mut restrictions = ExecutionRestrictions::unrestricted();
        change(&mut restrictions);
        denied(label, &restrictions, u64::MAX, 100);
    }
}

/// The runtime's replay lookup verdict on the ingress context decides
/// first versus retry: a first authentication must be the original's own
/// authentication observation, an exact retained replay may carry a fresh
/// transport authentication, and a check without a verdict applies neither
/// rule.
#[test]
fn the_runtime_replay_verdict_decides_first_authentication_versus_retry() {
    let f = fixture();
    let association = InputAuthorityAssociation::new(f.claims.clone()).expect("claims");
    let fresh = f.ingress.as_ref();
    assert!(
        fresh.authentication() != &f.claims.original_authentication,
        "the fixture's transport authentication is a fresh observation"
    );
    let check =
        |ingress: &NativeIngressContext| f.check.check(&f.runtime, &f.input, ingress, &association);

    let first = fresh.with_replay_lookup(Some(NativeReplayLookup::FirstAuthentication));
    assert_eq!(
        refusal(check(&first)),
        Some(OperationRefusalKind::MalformedFacts),
        "a first authentication must be the original's own observation"
    );
    let retained = fresh.with_replay_lookup(Some(NativeReplayLookup::RetainedReplay {
        existing_id: meerkat_core::InputId::new(),
    }));
    assert!(
        check(&retained).is_ok(),
        "an exact retained replay may carry a fresh transport authentication"
    );
    assert!(
        check(&fresh.with_replay_lookup(None)).is_ok(),
        "a pre-admission check without a verdict applies neither rule"
    );

    let mut own = f.claims.clone();
    own.original_authentication = fresh.authentication().clone();
    f.set_authentication(&own.original_authentication);
    assert!(
        f.admit_with(own, &first).is_ok(),
        "a first authentication that is the original's own observation admits"
    );
}

/// The current-source conjuncts read the source owner's custody: retained,
/// active and a resolving mandate. Owner changes can be excluded around a
/// host commit without publishing and without waiting.
#[test]
fn a_current_source_original_is_retained_and_its_mandate_resolves() {
    use meerkat_authorization::publication::PublicationError;
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let lineage = f
        .grants
        .issue_root(
            &f.root,
            id("source-leaf"),
            f.claims.logical_executor.clone(),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("the source original's grant");
    f.originals.contributors.lock().expect("fixture").insert(
        source.work.clone(),
        contributor_custody(
            &f.claims,
            &source,
            WorkAuthorityBasis::GrantLineage {
                lineage: vec![lineage.clone()],
            },
        ),
    );
    let current = || {
        require_source_original_live(
            &f.grants,
            f.originals.as_ref(),
            &f.claims.ingress_namespace,
            &source,
            &f.claims.logical_executor,
            None,
        )
    };
    assert!(current().is_ok());
    assert!(
        f.grants
            .try_with_owner_changes_excluded(current)
            .expect("uncontended")
            .is_ok(),
        "the conjuncts resolve while owner changes are excluded"
    );
    assert_eq!(
        f.grants.try_with_owner_changes_excluded(|| {
            f.grants.try_with_owner_changes_excluded(|| ())
        }),
        Ok(Err(PublicationError::Unavailable)),
        "a held exclusion refuses another without waiting"
    );
    assert!(current().is_ok(), "the exclusion published nothing");

    f.grants
        .revoke(&f.root, &lineage, &mut AnyCustody)
        .expect("revoked");
    assert_eq!(refusal(current()), Some(OperationRefusalKind::Denied));
    f.originals
        .contributors
        .lock()
        .expect("fixture")
        .remove(&source.work);
    assert_eq!(
        refusal(current()),
        Some(OperationRefusalKind::Denied),
        "an original the source owner no longer retains is not current"
    );
}

/// Outside an exclusion of owner changes, an owner change in progress while
/// the current-source conjuncts resolve the original's own mandate is a
/// changed policy (`PolicyChanged`), as at admission, never an owner that
/// cannot say now. The original is current again once the change completes.
#[test]
fn a_source_original_read_during_an_owner_change_is_a_changed_policy() {
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let lineage = f
        .grants
        .issue_root(
            &f.root,
            id("source-leaf"),
            f.claims.logical_executor.clone(),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("the source original's grant");
    f.originals.contributors.lock().expect("fixture").insert(
        source.work.clone(),
        contributor_custody(
            &f.claims,
            &source,
            WorkAuthorityBasis::GrantLineage {
                lineage: vec![lineage],
            },
        ),
    );
    let current = || {
        require_source_original_live(
            &f.grants,
            f.originals.as_ref(),
            &f.claims.ingress_namespace,
            &source,
            &f.claims.logical_executor,
            None,
        )
    };
    assert!(current().is_ok(), "no change in progress");
    let change = f
        .publication
        .begin_owner_change()
        .expect("an owner change in progress");
    let during = current();
    drop(change);
    assert_eq!(
        readiness(during),
        Some(ControllerReadinessFailure::PolicyChanged),
        "a change met outside an exclusion"
    );
    assert!(current().is_ok(), "current once the change completes");
}

/// The fixture's source owner, interfering once when a contributor's
/// retained custody is read: inside the preparation's observation of the
/// grant publication, just before contributor lineages are resolved.
struct InterferingOriginals {
    inner: Arc<Originals>,
    pending: Mutex<Option<ContributorInterference>>,
}

enum ContributorInterference {
    /// A helper thread begins an owner change on the grant publication and
    /// holds it until the test releases it.
    HoldOwnerChange {
        begin: std::sync::mpsc::SyncSender<()>,
        held: std::sync::mpsc::Receiver<()>,
    },
    Poison(LocalAuthorizationPublication),
}

impl AuthenticatedOriginalSource for InterferingOriginals {
    fn authenticated_original(
        &self,
        ingress: &NativeIngressContext,
        namespace: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<Option<AuthenticatedOriginal>, ControllerReadinessFailure> {
        self.inner
            .authenticated_original(ingress, namespace, original)
    }

    fn retained_custody(
        &self,
        namespace: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<RetainedCustody, ControllerReadinessFailure> {
        match self.pending.lock().expect("interference").take() {
            Some(ContributorInterference::HoldOwnerChange { begin, held }) => {
                begin.send(()).expect("start the owner change");
                held.recv_timeout(std::time::Duration::from_secs(5))
                    .expect("the owner change is in progress");
            }
            Some(ContributorInterference::Poison(publication)) => publication.poison(),
            None => {}
        }
        self.inner.retained_custody(namespace, original)
    }

    fn original_disposition(
        &self,
        namespace: &meerkat_authorization_contracts::work_association::QualifiedIngressNamespace,
        original: &OriginalWorkRef,
    ) -> Result<OriginalDisposition, ControllerReadinessFailure> {
        self.inner.original_disposition(namespace, original)
    }
}

impl Fixture {
    /// [`Self::operation_policy`] over a source owner that interferes once.
    fn interfering_policy(
        &self,
        operation: LocalOperationValues,
        interference: ContributorInterference,
    ) -> Arc<GrantBackedWorkPolicy> {
        let configuration = ScopedGrantComposition {
            grants: Arc::clone(&self.grants),
            ingress_domain: self.claims.ingress_namespace.ingress.clone(),
            originals: Arc::new(InterferingOriginals {
                inner: self.originals.clone(),
                pending: Mutex::new(Some(interference)),
            }),
            invocation: ScopedInvocation::from_policy(Arc::new(AllowWork)),
            operation_owner: Arc::new(ExactOperation(operation)),
            retained_resume: None,
        }
        .into_configuration();
        Arc::new(GrantBackedWorkPolicy::new(
            Arc::clone(&self.grants),
            Arc::clone(&configuration.invocation_owner),
            Arc::clone(&configuration.operation_owner),
        ))
    }
}

/// A helper names its source's admitted original. While the helper's
/// operation is prepared, an owner change is in progress when the
/// contributor's lineage is resolved: the preparation's own observation is
/// stale too, so the preparation refuses it as a stale read it may rebuild,
/// never as an unavailable owner, and a fresh preparation succeeds once the
/// change completes. A poison at the same point stays unavailable.
#[test]
fn a_contributor_resolved_during_an_owner_change_is_a_stale_refusal() {
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let executor = f.claims.logical_executor.clone();
    let covering = || {
        bounds(
            &["setup-report"],
            &["helper"],
            std::slice::from_ref(&executor),
        )
    };
    let contributor = f.issue_source_grant("source-live", covering());
    f.hold_contributor(&source, vec![contributor]);
    let helper = InputAuthorityAssociation::new(f.helper_claims_under(covering()))
        .expect("well-formed claims");
    let prepare = |policy: &Arc<GrantBackedWorkPolicy>| {
        policy
            .work_context(
                vec![helper.clone()].into(),
                meerkat_core::exact_operation::OperationExecutionScope::Domain,
            )
            .expect("the helper's work")
            .authorization()
            .prepare(&read_binding())
            .err()
    };

    let (begin_tx, begin_rx) = std::sync::mpsc::sync_channel(1);
    let (held_tx, held_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let publication = f.publication.clone();
    let changer = std::thread::spawn(move || {
        begin_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("asked to change an owner");
        let change = publication
            .begin_owner_change()
            .expect("an owner change in progress");
        held_tx.send(()).expect("the change is held");
        release_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("released by the test");
        drop(change);
    });
    let stale = prepare(&f.interfering_policy(
        f.helper_operation("setup-report"),
        ContributorInterference::HoldOwnerChange {
            begin: begin_tx,
            held: held_rx,
        },
    ));
    release_tx.send(()).expect("release the owner change");
    changer.join().expect("changer thread");
    assert!(
        matches!(
            &stale,
            Some(meerkat_core::OperationAuthorizationError::Refused(refusal))
                if refusal.kind() == OperationRefusalKind::ReprepareRequired
        ),
        "a contributor resolved during a change is a stale read, never unavailable: {stale:?}"
    );
    let (_, current) = f.operation_policy(f.helper_operation("setup-report"));
    let fresh = prepare(&Arc::new(current));
    assert!(
        fresh.is_none(),
        "a fresh preparation succeeds once the change completes: {fresh:?}"
    );

    let poisoned = prepare(&f.interfering_policy(
        f.helper_operation("setup-report"),
        ContributorInterference::Poison(f.publication.clone()),
    ));
    assert!(
        matches!(
            poisoned,
            Some(meerkat_core::OperationAuthorizationError::Unavailable)
        ),
        "a poison stays unavailable: {poisoned:?}"
    );
}

/// An owner change is in progress on the grant publication while admission
/// resolves a grant lineage: the admitted original's own mandate, or the
/// liveness of a contributor it names. Admission reads each lineage in an
/// observation of its own, and what it met there is a changed policy
/// (`PolicyChanged`, which the runtime driver reports as
/// `ControllerReadinessUnavailable`), never an owner that cannot say now.
/// The same admission admits once the change completes.
#[test]
fn an_owner_change_during_an_admission_lineage_read_is_a_changed_policy() {
    let changed = Some(ControllerReadinessFailure::PolicyChanged);

    // The admitted original's own grant-lineage mandate.
    let f = fixture();
    let helper = f.helper_claims();
    assert!(f.admit(helper.clone()).is_ok(), "no change in progress");
    let change = f
        .publication
        .begin_owner_change()
        .expect("an owner change in progress");
    let during = f.admit(helper.clone());
    drop(change);
    assert!(
        matches!(
            during.clone().map_err(RuntimeDriverError::from),
            Err(RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::PolicyChanged,
            })
        ),
        "the runtime driver reports a changed policy: {during:?}"
    );
    assert_eq!(readiness(during), changed, "the original's own mandate");
    assert!(f.admit(helper).is_ok(), "admits once the change completes");

    // A named contributor's lineage, under the fixture's own host-policy
    // mandate, so that no other lineage is read first.
    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let lineage = f.issue_source_grant("source-leaf", ExecutionRestrictions::unrestricted());
    f.hold_contributor(&source, vec![lineage]);
    assert!(f.admit(f.claims.clone()).is_ok(), "no change in progress");
    let change = f
        .publication
        .begin_owner_change()
        .expect("an owner change in progress");
    let during = f.admit(f.claims.clone());
    drop(change);
    assert_eq!(readiness(during), changed, "a contributor's liveness");
    assert!(
        f.admit(f.claims.clone()).is_ok(),
        "admits once the change completes"
    );
}

/// A poisoned grant publication at the same admission reads stays
/// unavailable (`PolicyUnavailable`), never a changed policy: no attempt
/// succeeds until its owners are reconstructed.
#[test]
fn a_poisoned_publication_at_an_admission_lineage_read_stays_unavailable() {
    let unavailable = Some(ControllerReadinessFailure::PolicyUnavailable);

    let f = fixture();
    let helper = f.helper_claims();
    f.publication.poison();
    assert_eq!(
        readiness(f.admit(helper)),
        unavailable,
        "the original's own mandate"
    );

    let f = fixture();
    let source = f.claims.contributing_work[1].clone();
    let lineage = f.issue_source_grant("source-leaf", ExecutionRestrictions::unrestricted());
    f.hold_contributor(&source, vec![lineage]);
    f.publication.poison();
    assert_eq!(
        readiness(f.admit(f.claims.clone())),
        unavailable,
        "a contributor's liveness"
    );
}
