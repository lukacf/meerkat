//! Tests of the real compiler with an explicit test-only admitted-work owner.
//! The fixture policy resolves its retained association and exact native input;
//! constructing a wire association is never the fixture's admission step.

#![allow(clippy::expect_used)]

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, DelegationDepth, ExactRestriction, ExecutionRestrictions,
    LifetimeRestriction, ProcessorRef, ResourceDomain, UnresolvedConstraint,
};
use meerkat_authorization_contracts::evidence::{
    EvidenceDigest, EvidenceId, HistoricalEvidenceRef,
};
use meerkat_authorization_contracts::protocol::ContractRequirements;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociationCandidate, NativeWorkTarget, OriginalWorkRef,
    QualifiedIngressNamespace, WorkAuthorityBasis,
};
use meerkat_core::authorization::{
    AuthorizationOperation, OperationAuthorizationFacts, OwnerQualifiedTarget,
    SourceAuthorizationFacts, SourceAuthorizationTarget, SourceAuthorizationUse,
};
use meerkat_core::connection::RealmId;
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::types::SessionId;
use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};

use super::*;
use crate::policy::{ExactOperationRelation, LocalOperationValues};

fn principal(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        TrustDomainId::new("fixture-domain").expect("fixture domain"),
    )
    .expect("fixture principal")
}

fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).expect("fixture evidence id")
}

fn domain(namespace: &str) -> ResourceDomain {
    ResourceDomain {
        authority: principal("resource-owner"),
        namespace: namespace.into(),
    }
}

fn resource(name: &str) -> ResourceRef {
    ResourceRef {
        domain: domain("records"),
        resource_id: name.into(),
    }
}

fn evidence(name: &str) -> HistoricalEvidenceRef {
    HistoricalEvidenceRef {
        resource: resource(name),
        revision: id("revision-1"),
        digest: EvidenceDigest::from_array([13; 32]),
    }
}

fn controller_selection(model: &str) -> meerkat_core::ControllerModelSelection {
    meerkat_core::ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: model.into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(
            serde_json::json!({"realm":"fixture-realm", "account":"controller"}),
        )
        .expect("credential identity"),
        "profile".into(),
        "fixture".into(),
    )
}

fn association(ceiling: ExecutionRestrictions) -> InputAuthorityAssociation {
    InputAuthorityAssociation::new(InputAuthorityAssociationCandidate {
        requester: principal("requester"),
        ingress_actor: principal("ingress"),
        represented_subject: None,
        original_authentication: evidence("authentication"),
        logical_executor: principal("executor"),
        target: NativeWorkTarget {
            logical_owner: principal("session-owner"),
            logical_runtime: id("runtime"),
            context: id("context"),
            context_generation: 1,
            audience: audience(),
        },
        original_work: OriginalWorkRef {
            authority: principal("ingress"),
            work: id("original-work"),
        },
        root_event: evidence("event"),
        contributing_work: vec![],
        authority_basis: WorkAuthorityBasis::HostPolicy {
            policy: evidence("actual-fixture-host-policy"),
        },
        controller_grant_lineage: Vec::new(),
        controller_model: Some(controller_selection("controller-model")),
        controller_ceiling: ExecutionRestrictions::unrestricted(),
        admitted_ceiling: ceiling,
        source_observations: vec![evidence("observed-source")],
        ingress_namespace: QualifiedIngressNamespace {
            realm: RealmId::parse("fixture-realm").expect("fixture realm"),
            ingress: domain("input"),
            occurrence_scope: id("occurrence"),
        },
        contract: ContractRequirements::local_governed_v1(),
    })
    .expect("association data")
}

fn scope() -> OperationExecutionScope {
    OperationExecutionScope::RuntimeInput {
        owner_session_id: SessionId::parse("00000000-0000-0000-0000-000000000001")
            .expect("session id"),
        runtime_epoch_id: serde_json::from_str(r#""00000000-0000-0000-0000-000000000002""#)
            .expect("epoch id"),
        submitted_input_id: serde_json::from_str(r#""00000000-0000-0000-0000-000000000003""#)
            .expect("input id"),
        canonical_input_id: serde_json::from_str(r#""00000000-0000-0000-0000-000000000003""#)
            .expect("input id"),
    }
}

fn processor() -> ProcessorRef {
    ProcessorRef::Principal {
        principal: principal("executor"),
    }
}

fn audience() -> AudienceRef {
    AudienceRef::Principal {
        principal: principal("requester"),
    }
}

fn tuple(action: &str, namespace: &str) -> LocalOperationValues {
    LocalOperationValues {
        action: ActionRef {
            feature: "fixture".into(),
            action: action.into(),
        },
        resource_domain: domain(namespace),
        processor: processor(),
        audience: audience(),
    }
}

fn binding(usage: SourceAuthorizationUse, namespace: &str) -> PreparedAuthorizationBinding {
    PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
        operation_id: serde_json::from_str(r#""00000000-0000-0000-0000-000000000004""#)
            .expect("operation id"),
        execution_scope: scope(),
        run_id: Some(
            serde_json::from_str(r#""00000000-0000-0000-0000-000000000005""#).expect("run id"),
        ),
        context_revision: None,
        operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                authority: principal("resource-owner"),
                namespace: Arc::from(namespace),
                id: Arc::from("record"),
            }),
            usage,
        }),
    })
}

fn read_binding() -> PreparedAuthorizationBinding {
    binding(SourceAuthorizationUse::Read, "public")
}

#[cfg(not(target_arch = "wasm32"))]
fn malformed() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::MalformedFacts)
}

struct FixtureClock {
    start: Instant,
    unix_ms: AtomicU64,
    elapsed_ms: AtomicU64,
    unavailable: AtomicBool,
    reads: AtomicUsize,
}

impl FixtureClock {
    fn new() -> Self {
        Self {
            start: Instant::now(),
            unix_ms: AtomicU64::new(1_000),
            elapsed_ms: AtomicU64::new(0),
            unavailable: AtomicBool::new(false),
            reads: AtomicUsize::new(0),
        }
    }

    fn set(&self, unix_ms: u64, elapsed_ms: u64) {
        self.unix_ms.store(unix_ms, Ordering::Relaxed);
        self.elapsed_ms.store(elapsed_ms, Ordering::Relaxed);
    }
}

impl LocalAuthorizationClock for FixtureClock {
    fn now(&self) -> Result<LocalAuthorizationTime, crate::clock::LocalClockError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if self.unavailable.load(Ordering::Relaxed) {
            return Err(crate::clock::LocalClockError::Unavailable);
        }
        Ok(LocalAuthorizationTime {
            unix_ms: self.unix_ms.load(Ordering::Relaxed),
            monotonic: self.start + Duration::from_millis(self.elapsed_ms.load(Ordering::Relaxed)),
        })
    }
}

struct PolicyState {
    restrictions: ExecutionRestrictions,
    expires_at_ms: u64,
    extra_values: Vec<LocalOperationValues>,
    refuse: bool,
}

struct FixturePolicy {
    // This exact retained row is the fake owner's authority. The evaluate input
    // must match it and the separately fixed native runtime scope in full.
    admitted: Arc<InputAuthorityAssociation>,
    relation: ExactOperationRelation,
    state: Mutex<PolicyState>,
    publication: LocalAuthorizationPublication,
    calls: AtomicUsize,
    changes_during_evaluate: AtomicUsize,
}

impl LocalWorkPolicy for FixturePolicy {
    fn evaluate(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if association != self.admitted.as_ref() || binding.facts().execution_scope != scope() {
            return Err(denied().into());
        }
        let AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::External(target),
            usage,
        }) = &binding.facts().operation
        else {
            return Err(denied().into());
        };
        if target.authority != principal("resource-owner") || target.id.as_ref() != "record" {
            return Err(denied().into());
        }
        let action = match usage {
            SourceAuthorizationUse::Read => "read",
            SourceAuthorizationUse::Retain => "write",
            SourceAuthorizationUse::Hydrate => "hydrate",
        };
        let actual = tuple(action, &target.namespace);
        if !self.relation.contains(&actual) {
            return Err(denied().into());
        }
        let allowance = {
            let state = self.state.lock().expect("fixture owner state");
            if state.refuse {
                return Err(denied().into());
            }
            let mut operation_values = vec![actual];
            operation_values.extend(state.extra_values.iter().cloned());
            LocalPolicyAllowance {
                operation_values,
                restrictions: state.restrictions.clone(),
                expires_at_ms: state.expires_at_ms,
                review_tier: meerkat_core::authorization::OperationReviewTier::R1,
            }
        };
        if self
            .changes_during_evaluate
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            // Deliberately mutate publication after reading owner facts. The
            // real compiler must discard this view rather than retagging it.
            drop(
                self.publication
                    .begin_owner_change()
                    .expect("test publication"),
            );
        }
        Ok(allowance)
    }
}

struct Fixture {
    authorization: LocalWorkAuthorization,
    policy: Arc<FixturePolicy>,
    clock: Arc<FixtureClock>,
    publication: LocalAuthorizationPublication,
}

impl Fixture {
    fn new(admitted_ceiling: ExecutionRestrictions) -> Self {
        let admitted = Arc::new(association(admitted_ceiling));
        let publication = LocalAuthorizationPublication::new();
        let clock = Arc::new(FixtureClock::new());
        let policy = Arc::new(FixturePolicy {
            admitted: Arc::clone(&admitted),
            relation: ExactOperationRelation::new(vec![
                tuple("read", "public"),
                tuple("write", "private"),
            ]),
            state: Mutex::new(PolicyState {
                restrictions: ExecutionRestrictions::unrestricted(),
                expires_at_ms: 2_000,
                extra_values: vec![],
                refuse: false,
            }),
            publication: publication.clone(),
            calls: AtomicUsize::new(0),
            changes_during_evaluate: AtomicUsize::new(0),
        });
        let authorization = LocalWorkAuthorization::new(
            admitted,
            policy.clone(),
            publication.clone(),
            clock.clone(),
        );
        Self {
            authorization,
            policy,
            clock,
            publication,
        }
    }

    fn change(&self, mutate: impl FnOnce(&mut PolicyState)) {
        let publication = self
            .publication
            .begin_owner_change()
            .expect("owner publication");
        mutate(&mut self.policy.state.lock().expect("fixture owner state"));
        drop(publication);
    }
}

fn assert_refused<T, E: Into<meerkat_core::OperationAuthorizationError>>(
    result: Result<T, E>,
    kind: OperationRefusalKind,
) {
    assert!(
        matches!(result.map_err(Into::into), Err(meerkat_core::OperationAuthorizationError::Refused(refusal)) if refusal.kind() == kind)
    );
}

#[test]
fn context_compilation_retains_only_its_coherent_policy_observation() {
    let publication = LocalAuthorizationPublication::new();
    let clock: Arc<dyn LocalAuthorizationClock> = Arc::new(FixtureClock::new());
    let binding = read_binding();
    let ((), stamp) = publication.observe(|| ()).expect("initial publication");
    let expected = Some(stamp.policy_observation());
    let allowed = compile_context_control(&publication, &clock, &binding, |_| {
        Ok(vec![LocalPolicyAllowance {
            operation_values: vec![tuple("read", "public")],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 2_000,
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        }])
    });
    assert_eq!(allowed.policy, expected);
    let decision = allowed.result.expect("allowed control");
    assert_eq!(decision.policy_observation(), expected);
    let refused = compile_context_control(&publication, &clock, &binding, |_| Err(denied().into()));
    assert_refused(refused.result, OperationRefusalKind::Denied);
    assert_eq!(refused.policy, expected);
    let changed = compile_context_control(&publication, &clock, &binding, |_| {
        drop(
            publication
                .begin_owner_change()
                .expect("concurrent owner change"),
        );
        Err(denied().into())
    });
    assert_refused(changed.result, OperationRefusalKind::ReprepareRequired);
    assert_eq!(changed.policy, None);
    // An infrastructure failure must not be reclassified as a stale refusal.
    let unavailable = compile_context_control(&publication, &clock, &binding, |_| {
        drop(
            publication
                .begin_owner_change()
                .expect("concurrent owner change"),
        );
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    });
    assert!(matches!(
        unavailable.result,
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    ));
    assert_eq!(unavailable.policy, None);
}

#[test]
fn context_clock_failure_never_claims_a_policy_read() {
    let publication = LocalAuthorizationPublication::new();
    let clock = Arc::new(FixtureClock::new());
    clock.unavailable.store(true, Ordering::Relaxed);
    let clock: Arc<dyn LocalAuthorizationClock> = clock;
    let calls = AtomicUsize::new(0);
    let observed = compile_context_control(&publication, &clock, &read_binding(), |_| {
        calls.fetch_add(1, Ordering::Relaxed);
        Err(denied().into())
    });
    assert!(matches!(
        observed.result,
        Err(meerkat_core::OperationAuthorizationError::Unavailable)
    ));
    assert_eq!(observed.policy, None);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
}

#[test]
fn compiled_operation_and_retained_clone_check_without_policy_reevaluation() {
    let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
    let binding = read_binding();
    let prepared = fixture
        .authorization
        .prepare(&binding)
        .expect("fixture allows");
    let retained = binding.clone();
    for _ in 0..32 {
        prepared
            .check_current(&retained)
            .expect("same exact operation");
    }
    assert_eq!(fixture.policy.calls.load(Ordering::Relaxed), 1);
    assert_eq!(fixture.clock.reads.load(Ordering::Relaxed), 34);
}

#[test]
fn equal_or_modified_facts_cannot_retarget_a_prepared_decision() {
    let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
    let binding = read_binding();
    let prepared = fixture
        .authorization
        .prepare(&binding)
        .expect("fixture allows");
    for change_scope in [false, true] {
        let mut facts = binding.facts().clone();
        if change_scope {
            facts.execution_scope = OperationExecutionScope::Domain;
        }
        let reconstructed = PreparedAuthorizationBinding::new(facts);
        assert_refused(
            prepared.check_current(&reconstructed),
            OperationRefusalKind::ReprepareRequired,
        );
    }
    prepared
        .check_current(&binding)
        .expect("original remains valid");
}

#[test]
fn decoded_host_policy_claim_and_wrong_native_input_do_not_establish_admission() {
    let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
    let binding = read_binding();
    let mut claim = fixture.policy.admitted.candidate().clone();
    claim.requester = principal("other-requester");
    let impostor = LocalWorkAuthorization::new(
        Arc::new(InputAuthorityAssociation::new(claim).expect("well-formed claim")),
        fixture.policy.clone(),
        fixture.publication.clone(),
        fixture.clock.clone(),
    );
    assert_refused(impostor.prepare(&binding), OperationRefusalKind::Denied);
    let mut facts = binding.facts().clone();
    if let OperationExecutionScope::RuntimeInput {
        canonical_input_id, ..
    } = &mut facts.execution_scope
    {
        *canonical_input_id = serde_json::from_str(r#""00000000-0000-0000-0000-000000000099""#)
            .expect("other input id");
    }
    let wrong_input = PreparedAuthorizationBinding::new(facts);
    assert_refused(
        fixture.authorization.prepare(&wrong_input),
        OperationRefusalKind::Denied,
    );
}

#[test]
fn owner_revocation_invalidates_old_projection_before_and_after_publication() {
    let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
    let binding = read_binding();
    let prepared = fixture
        .authorization
        .prepare(&binding)
        .expect("fixture allows");
    let change = fixture
        .publication
        .begin_owner_change()
        .expect("start revocation");
    assert_refused(
        prepared.check_current(&binding),
        OperationRefusalKind::ReprepareRequired,
    );
    fixture.policy.state.lock().expect("fixture owner").refuse = true;
    drop(change);
    assert_refused(
        prepared.check_current(&binding),
        OperationRefusalKind::ReprepareRequired,
    );
    assert_refused(
        fixture.authorization.prepare(&binding),
        OperationRefusalKind::Denied,
    );
    // The ordinary policy refusal is evaluated once; it is not retried.
    assert_eq!(fixture.policy.calls.load(Ordering::Relaxed), 2);
}

#[test]
fn concurrent_publication_rebuilds_once_and_never_spins_or_retags() {
    for (changes, succeeds) in [(1, true), (2, false)] {
        let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
        let binding = read_binding();
        fixture
            .policy
            .changes_during_evaluate
            .store(changes, Ordering::Relaxed);
        let prepared = fixture.authorization.prepare(&binding);
        if succeeds {
            prepared
                .expect("one clean rebuild")
                .check_current(&binding)
                .expect("current");
        } else {
            assert_refused(prepared, OperationRefusalKind::ReprepareRequired);
        }
        assert_eq!(fixture.policy.calls.load(Ordering::Relaxed), 2);
    }
}

#[test]
fn admitted_and_current_ceilings_each_constrain_every_implicit_target() {
    let mut admitted = ExecutionRestrictions::unrestricted();
    admitted.resource_domains = ExactRestriction::exact([domain("public")]);
    let fixture = Fixture::new(admitted);
    let binding = read_binding();
    fixture
        .authorization
        .prepare(&binding)
        .expect("public source allowed");
    fixture.change(|state| state.extra_values.push(tuple("write", "private")));
    assert_refused(
        fixture.authorization.prepare(&binding),
        OperationRefusalKind::Denied,
    );

    let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
    fixture.change(|state| state.restrictions.actions = ExactRestriction::exact([]));
    assert_refused(
        fixture.authorization.prepare(&binding),
        OperationRefusalKind::Denied,
    );
}

#[test]
fn unresolved_restrictions_cannot_be_dropped_during_compilation() {
    for dimension in 0..6 {
        let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
        fixture.change(|state| match dimension {
            0 => {
                state.restrictions.actions =
                    ExactRestriction::unresolved(UnresolvedConstraint::Absent);
            }
            1 => {
                state.restrictions.resource_domains =
                    ExactRestriction::unresolved(UnresolvedConstraint::Unknown);
            }
            2 => {
                state.restrictions.processors =
                    ExactRestriction::unresolved(UnresolvedConstraint::Unavailable);
            }
            3 => {
                state.restrictions.audiences =
                    ExactRestriction::unresolved(UnresolvedConstraint::Unknown);
            }
            4 => {
                state.restrictions.lifetime =
                    LifetimeRestriction::unresolved(UnresolvedConstraint::Absent);
            }
            _ => {
                state.restrictions.delegation_depth =
                    DelegationDepth::unresolved(UnresolvedConstraint::Unavailable);
            }
        });
        assert_refused(
            fixture.authorization.prepare(&read_binding()),
            OperationRefusalKind::Denied,
        );
    }
}

#[test]
fn exact_time_boundaries_and_earliest_owner_expiry_are_enforced() {
    let mut ceiling = ExecutionRestrictions::unrestricted();
    ceiling.lifetime = LifetimeRestriction::window(1_000, 1_800);
    let fixture = Fixture::new(ceiling);
    fixture.change(|state| {
        state.restrictions.lifetime = LifetimeRestriction::window(900, 1_600);
        state.expires_at_ms = 1_400;
    });
    let binding = read_binding();
    fixture.clock.set(999, 0);
    assert_refused(
        fixture.authorization.prepare(&binding),
        OperationRefusalKind::Denied,
    );
    fixture.clock.set(1_000, 0);
    let prepared = fixture
        .authorization
        .prepare(&binding)
        .expect("inclusive start");
    fixture.clock.set(1_399, 399);
    prepared
        .check_current(&binding)
        .expect("before earliest expiry");
    fixture.clock.set(1_400, 399);
    assert_refused(
        prepared.check_current(&binding),
        OperationRefusalKind::Denied,
    );
}

#[test]
fn wall_rollback_does_not_extend_monotonic_deadline_or_cross_not_before() {
    let mut ceiling = ExecutionRestrictions::unrestricted();
    ceiling.lifetime = LifetimeRestriction::window(1_000, 2_000);
    let fixture = Fixture::new(ceiling);
    let binding = read_binding();
    let prepared = fixture
        .authorization
        .prepare(&binding)
        .expect("initial allowance");
    fixture.clock.set(999, 10);
    assert_refused(
        prepared.check_current(&binding),
        OperationRefusalKind::Denied,
    );
    fixture.clock.set(1_100, 999);
    prepared
        .check_current(&binding)
        .expect("both clocks before end");
    fixture.clock.set(1_100, 1_000);
    assert_refused(
        prepared.check_current(&binding),
        OperationRefusalKind::Denied,
    );
    fixture.clock.unavailable.store(true, Ordering::Relaxed);
    let error: meerkat_core::OperationAuthorizationError = prepared
        .check_current(&binding)
        .expect_err("unavailable clock cannot validate entry");
    assert_eq!(
        meerkat_core::ToolError::from(error).error_code(),
        "operation_authorization_unavailable"
    );
}

#[test]
fn independent_policy_relations_do_not_authorize_crossed_action_resource_pairs() {
    let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
    for (usage, namespace, expected) in [
        (SourceAuthorizationUse::Read, "public", true),
        (SourceAuthorizationUse::Retain, "private", true),
        (SourceAuthorizationUse::Read, "private", false),
        (SourceAuthorizationUse::Retain, "public", false),
    ] {
        let binding = binding(usage, namespace);
        let result = fixture.authorization.prepare(&binding);
        if expected {
            result
                .expect("exact relation member")
                .check_current(&binding)
                .expect("current");
        } else {
            assert_refused(result, OperationRefusalKind::Denied);
        }
    }
}

#[test]
fn admitted_and_current_processor_and_audience_limits_remain_permissions() {
    for admitted_limit in [false, true] {
        for processors in [false, true] {
            let mut restriction = ExecutionRestrictions::unrestricted();
            if processors {
                restriction.processors = ExactRestriction::exact([]);
            } else {
                restriction.audiences = ExactRestriction::exact([]);
            }
            let fixture = if admitted_limit {
                Fixture::new(restriction)
            } else {
                let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
                fixture.change(|state| state.restrictions = restriction);
                fixture
            };
            assert_refused(
                fixture.authorization.prepare(&read_binding()),
                OperationRefusalKind::Denied,
            );
        }
    }
}

#[test]
fn historical_source_reference_does_not_authorize_another_actual_resource() {
    let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
    for change_owner in [false, true] {
        let mut facts = read_binding().facts().clone();
        let AuthorizationOperation::Source(SourceAuthorizationFacts {
            target: SourceAuthorizationTarget::External(target),
            ..
        }) = &mut facts.operation
        else {
            unreachable!("fixture source target");
        };
        if change_owner {
            target.authority = principal("other-resource-owner");
        } else {
            target.id = Arc::from("other-record");
        }
        assert_refused(
            fixture
                .authorization
                .prepare(&PreparedAuthorizationBinding::new(facts)),
            OperationRefusalKind::Denied,
        );
    }
    fixture
        .authorization
        .prepare(&read_binding())
        .expect("actual owned resource");
}

#[cfg(not(target_arch = "wasm32"))]
mod tool_dispatch;

struct BatchFixturePolicy {
    admitted: Vec<InputAuthorityAssociation>,
    refused_index: AtomicUsize,
    calls: AtomicUsize,
}

impl LocalWorkPolicy for BatchFixturePolicy {
    fn evaluate(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let index = self
            .admitted
            .iter()
            .position(|item| item == association)
            .ok_or_else(denied)?;
        if self.refused_index.load(Ordering::Relaxed) == index
            || binding.facts().execution_scope != scope()
        {
            return Err(denied().into());
        }
        // Actual source coordinates, independently checked before returning
        // the correlated tuple. This fixture cannot elect a different target.
        match &binding.facts().operation {
            AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::External(target),
                usage: SourceAuthorizationUse::Read,
            }) if target.authority == principal("resource-owner")
                && target.namespace.as_ref() == "public"
                && target.id.as_ref() == "record" => {}
            _ => return Err(denied().into()),
        }
        Ok(LocalPolicyAllowance {
            operation_values: vec![tuple("read", "public")],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: if index == 0 { 2_000 } else { 1_500 },
            review_tier: meerkat_core::authorization::OperationReviewTier::R1,
        })
    }
}

fn batch_fixture() -> (
    Vec<InputAuthorityAssociation>,
    Arc<BatchFixturePolicy>,
    LocalAuthorizationPublication,
    Arc<FixtureClock>,
) {
    let first = association(ExecutionRestrictions::unrestricted());
    let mut second = first.candidate().clone();
    second.original_work.work = id("second-original");
    second.root_event = evidence("second-event");
    let associations = vec![
        first,
        InputAuthorityAssociation::new(second).expect("second original"),
    ];
    let policy = Arc::new(BatchFixturePolicy {
        admitted: associations.clone(),
        refused_index: AtomicUsize::new(usize::MAX),
        calls: AtomicUsize::new(0),
    });
    (
        associations,
        policy,
        LocalAuthorizationPublication::new(),
        Arc::new(FixtureClock::new()),
    )
}

#[test]
fn batch_checks_every_original_once_then_one_stamp_and_earliest_deadline() {
    let (originals, policy, publication, clock) = batch_fixture();
    let compiler = LocalWorkAuthorization::new_batch(
        originals.into(),
        policy.clone(),
        publication,
        clock.clone(),
    )
    .expect("homogeneous originals");
    let binding = read_binding();
    let prepared = compiler.prepare(&binding).expect("both owners allow");
    assert_eq!(policy.calls.load(Ordering::Relaxed), 2);
    for _ in 0..10 {
        prepared.check_current(&binding).expect("same stamp");
    }
    assert_eq!(
        policy.calls.load(Ordering::Relaxed),
        2,
        "warm checks never revisit contributors"
    );
    clock.set(1_500, 500);
    assert!(
        prepared.check_current(&binding).is_err(),
        "second contributor deadline is binding"
    );
}

#[test]
fn batch_cannot_union_ceilings_or_ignore_a_revoked_contributor() {
    let (originals, policy, publication, clock) = batch_fixture();
    let compiler = LocalWorkAuthorization::new_batch(
        originals.clone().into(),
        policy.clone(),
        publication.clone(),
        clock.clone(),
    )
    .expect("batch");
    let binding = read_binding();
    let prepared = compiler.prepare(&binding).expect("allowed");
    let change = publication.begin_owner_change().expect("owner mutation");
    policy.refused_index.store(1, Ordering::Relaxed);
    drop(change);
    assert!(prepared.check_current(&binding).is_err());
    assert!(
        compiler.prepare(&binding).is_err(),
        "second original must still allow"
    );

    let mut changed = originals[1].candidate().clone();
    changed.admitted_ceiling.actions = ExactRestriction::exact([]);
    let restricted = InputAuthorityAssociation::new(changed).expect("empty exact ceiling");
    let mut originals = originals;
    originals[1] = restricted;
    let policy = Arc::new(BatchFixturePolicy {
        admitted: originals.clone(),
        refused_index: AtomicUsize::new(usize::MAX),
        calls: AtomicUsize::new(0),
    });
    let compiler = LocalWorkAuthorization::new_batch(originals.into(), policy, publication, clock)
        .expect("compatible participants");
    assert!(
        compiler.prepare(&binding).is_err(),
        "first original cannot widen the second"
    );
}

#[test]
fn batch_rejects_empty_or_mixed_requester_subject_executor_realm_and_target() {
    let (originals, policy, publication, clock) = batch_fixture();
    assert!(
        LocalWorkAuthorization::new_batch(
            Vec::new().into(),
            policy.clone(),
            publication.clone(),
            clock.clone()
        )
        .is_err()
    );
    for variant in 0..5 {
        let mut changed = originals[1].candidate().clone();
        match variant {
            0 => changed.requester = principal("other-caller"),
            1 => changed.represented_subject = Some(principal("another-person")),
            2 => changed.logical_executor = principal("other-agent"),
            3 => changed.ingress_namespace.realm = RealmId::parse("other-realm").expect("realm"),
            _ => changed.target.context_generation += 1,
        }
        let changed = InputAuthorityAssociation::new(changed).expect("valid different association");
        assert!(
            LocalWorkAuthorization::new_batch(
                vec![originals[0].clone(), changed].into(),
                policy.clone(),
                publication.clone(),
                clock.clone()
            )
            .is_err()
        );
    }
}

fn model_binding(usage: ModelAuthorizationUse, hosted: bool) -> PreparedAuthorizationBinding {
    let mut facts = read_binding().facts().clone();
    facts.operation =
        AuthorizationOperation::Model(meerkat_core::authorization::ModelAuthorizationFacts {
            identity: Arc::new(meerkat_core::SessionLlmIdentity {
                model: "controller-model".into(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: None,
            }),
            wire_model: Arc::from("controller-model"),
            hosted_capabilities: if hosted {
                vec![meerkat_core::ServerToolKind::WebSearch].into()
            } else {
                Vec::new().into()
            },
            backend_profile_id: Some(Arc::from("profile")),
            backend_kind: Arc::from("fixture"),
            endpoint: Arc::from("https://provider.invalid"),
            credential: Some(
                controller_selection("controller-model")
                    .credential()
                    .clone(),
            ),
            usage,
            live_channel: None,
        });
    PreparedAuthorizationBinding::new(facts)
}

#[test]
fn controller_ceiling_is_independent_but_never_bypasses_other_operation_duties() {
    let mut ordinary = ExecutionRestrictions::unrestricted();
    ordinary.lifetime = LifetimeRestriction::window(0, 999);
    let fixture = Fixture::new(ordinary);
    let admitted = fixture.policy.admitted.as_ref();
    let now = fixture.clock.now().expect("clock");
    for (usage, purpose, succeeds) in [
        (
            ModelAuthorizationUse::ControllerInference,
            LocalPolicyPurpose::Controller,
            true,
        ),
        (
            ModelAuthorizationUse::ControllerInference,
            LocalPolicyPurpose::Operation,
            false,
        ),
        (
            ModelAuthorizationUse::Inference,
            LocalPolicyPurpose::Controller,
            false,
        ),
        (
            ModelAuthorizationUse::Inference,
            LocalPolicyPurpose::Operation,
            false,
        ),
        (
            ModelAuthorizationUse::Compaction,
            LocalPolicyPurpose::Operation,
            false,
        ),
        (
            ModelAuthorizationUse::Live,
            LocalPolicyPurpose::Operation,
            false,
        ),
    ] {
        let binding = model_binding(usage, false);
        let result = fixture.authorization.validate_allowance(
            admitted,
            &binding,
            purpose,
            LocalPolicyAllowance {
                operation_values: vec![tuple("controller", "public")],
                restrictions: ExecutionRestrictions::unrestricted(),
                expires_at_ms: 2_000,
                review_tier: meerkat_core::authorization::OperationReviewTier::R1,
            },
            now,
        );
        assert_eq!(result.is_ok(), succeeds);
    }
}

#[test]
fn controller_label_cannot_substitute_model_account_profile_or_backend() {
    let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
    let admitted = fixture.policy.admitted.as_ref();
    let now = fixture.clock.now().expect("clock");
    let check = |binding: &PreparedAuthorizationBinding| {
        fixture.authorization.validate_allowance(
            admitted,
            binding,
            LocalPolicyPurpose::Controller,
            LocalPolicyAllowance {
                operation_values: vec![tuple("controller", "public")],
                restrictions: ExecutionRestrictions::unrestricted(),
                expires_at_ms: 2_000,
                review_tier: meerkat_core::authorization::OperationReviewTier::R1,
            },
            now,
        )
    };
    let original = model_binding(ModelAuthorizationUse::ControllerInference, false);
    assert!(
        check(&original).is_ok(),
        "exact current route baseline must pass"
    );
    for mutation in 0..5 {
        let mut facts = original.facts().clone();
        let AuthorizationOperation::Model(model) = &mut facts.operation else {
            unreachable!("model fixture");
        };
        match mutation {
            0 => Arc::make_mut(&mut model.identity).model = "alternate".into(),
            1 => model.credential = None,
            2 => model.backend_profile_id = Some(Arc::from("other-profile")),
            3 => model.backend_kind = Arc::from("other-backend"),
            _ => {
                Arc::make_mut(&mut model.identity).self_hosted_server_id =
                    Some("other-server".into());
            }
        }
        assert!(check(&PreparedAuthorizationBinding::new(facts)).is_err());
    }
}

struct RoleFixturePolicy {
    admitted: InputAuthorityAssociation,
    calls: Mutex<Vec<LocalPolicyPurpose>>,
    refuse_operation: AtomicBool,
    /// Owner tiers for the controller and the hosted-operation duty.
    tiers: Mutex<(
        meerkat_core::authorization::OperationReviewTier,
        meerkat_core::authorization::OperationReviewTier,
    )>,
}

impl LocalWorkPolicy for RoleFixturePolicy {
    fn evaluate(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        Err(denied().into())
    }
    fn evaluate_for(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        _now_ms: u64,
    ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
        if association != &self.admitted || binding.facts().execution_scope != scope() {
            return Err(denied().into());
        }
        let AuthorizationOperation::Model(facts) = &binding.facts().operation else {
            return Err(denied().into());
        };
        if facts.usage != ModelAuthorizationUse::ControllerInference
            || !association
                .candidate()
                .controller_model
                .as_ref()
                .is_some_and(|selected| selected.matches_model_facts(facts))
        {
            return Err(denied().into());
        }
        self.calls.lock().expect("calls").push(purpose);
        let action = match purpose {
            LocalPolicyPurpose::Controller => "infer",
            LocalPolicyPurpose::Operation => {
                if facts.hosted_capabilities.as_ref() != [meerkat_core::ServerToolKind::WebSearch]
                    || self.refuse_operation.load(Ordering::Relaxed)
                {
                    return Err(denied().into());
                }
                "web_search"
            }
        };
        let (controller_tier, operation_tier) = *self.tiers.lock().expect("tiers");
        Ok(LocalPolicyAllowance {
            operation_values: vec![tuple(action, "public")],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 2_000,
            review_tier: match purpose {
                LocalPolicyPurpose::Controller => controller_tier,
                LocalPolicyPurpose::Operation => operation_tier,
            },
        })
    }
}

#[test]
fn hosted_controller_conjoins_distinct_duties_without_cross_applying_action_ceilings() {
    let mut retained = association(ExecutionRestrictions::unrestricted())
        .candidate()
        .clone();
    retained.controller_ceiling.actions =
        ExactRestriction::exact([tuple("infer", "public").action]);
    retained.admitted_ceiling.actions =
        ExactRestriction::exact([tuple("web_search", "public").action]);
    let admitted = InputAuthorityAssociation::new(retained).expect("separate ceilings");
    let policy = Arc::new(RoleFixturePolicy {
        admitted: admitted.clone(),
        calls: Mutex::new(Vec::new()),
        refuse_operation: AtomicBool::new(false),
        tiers: Mutex::new((
            meerkat_core::authorization::OperationReviewTier::R1,
            meerkat_core::authorization::OperationReviewTier::R1,
        )),
    });
    let publication = LocalAuthorizationPublication::new();
    let compiler = LocalWorkAuthorization::new(
        Arc::new(admitted),
        policy.clone(),
        publication.clone(),
        Arc::new(FixtureClock::new()),
    );
    let binding = model_binding(ModelAuthorizationUse::ControllerInference, true);
    compiler
        .prepare(&binding)
        .expect("both distinct duties allow");
    assert_eq!(
        *policy.calls.lock().expect("calls"),
        [
            LocalPolicyPurpose::Controller,
            LocalPolicyPurpose::Operation
        ]
    );
    let change = publication.begin_owner_change().expect("policy mutation");
    policy.refuse_operation.store(true, Ordering::Relaxed);
    drop(change);
    assert!(
        compiler.prepare(&binding).is_err(),
        "controller may not replace denied hosted authority"
    );
    compiler
        .prepare(&model_binding(
            ModelAuthorizationUse::ControllerInference,
            false,
        ))
        .expect("bare controller remains available");
}

/// The compiled decision keeps the STRICTEST tier over every contributing
/// duty and every context source/audience rule, in either order. A min (or
/// first/last-wins) combination fails this test.
#[test]
fn review_tier_is_the_strictest_over_duties_and_context_rules_in_either_order() {
    use meerkat_core::authorization::OperationReviewTier::{R1, R3};
    for (first, second) in [(R1, R3), (R3, R1)] {
        let mut retained = association(ExecutionRestrictions::unrestricted())
            .candidate()
            .clone();
        retained.controller_ceiling.actions =
            ExactRestriction::exact([tuple("infer", "public").action]);
        retained.admitted_ceiling.actions =
            ExactRestriction::exact([tuple("web_search", "public").action]);
        let admitted = InputAuthorityAssociation::new(retained).expect("separate ceilings");
        let policy = Arc::new(RoleFixturePolicy {
            admitted: admitted.clone(),
            calls: Mutex::new(Vec::new()),
            refuse_operation: AtomicBool::new(false),
            tiers: Mutex::new((first, second)),
        });
        let compiler = LocalWorkAuthorization::new(
            Arc::new(admitted),
            policy,
            LocalAuthorizationPublication::new(),
            Arc::new(FixtureClock::new()),
        );
        let prepared = compiler
            .prepare(&model_binding(
                ModelAuthorizationUse::ControllerInference,
                true,
            ))
            .expect("both duties allow");
        assert_eq!(
            prepared.review_tier(),
            R3,
            "duties ({first:?}, {second:?}) keep the strictest tier"
        );

        let clock: Arc<dyn LocalAuthorizationClock> = Arc::new(FixtureClock::new());
        let allowance = |review_tier| LocalPolicyAllowance {
            operation_values: vec![tuple("read", "public")],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: 2_000,
            review_tier,
        };
        let observed = super::compile_context_control(
            &LocalAuthorizationPublication::new(),
            &clock,
            &read_binding(),
            |_now| Ok(vec![allowance(first), allowance(second)]),
        );
        assert_eq!(
            observed
                .result
                .expect("source and audience rules allow")
                .review_tier(),
            R3,
            "context rules ({first:?}, {second:?}) keep the strictest tier"
        );
    }
}

#[test]
fn unavailable_clock_at_current_check_is_not_a_policy_denial() {
    let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
    let binding = read_binding();
    let prepared = fixture
        .authorization
        .prepare(&binding)
        .expect("healthy initial preparation");
    prepared
        .check_current(&binding)
        .expect("healthy current check");
    fixture.clock.unavailable.store(true, Ordering::Relaxed);
    let error: meerkat_core::OperationAuthorizationError = prepared
        .check_current(&binding)
        .expect_err("unavailable clock cannot validate entry");
    fixture.clock.unavailable.store(false, Ordering::Relaxed);
    prepared
        .check_current(&binding)
        .expect("same retained check recovers with unchanged policy");
    assert_eq!(
        meerkat_core::ToolError::from(error).error_code(),
        "operation_authorization_unavailable"
    );
}

#[test]
#[allow(clippy::panic)] // The spawned thread deliberately poisons the actual owner lock.
fn poisoned_publication_is_unavailable_at_prepare_and_current_check() {
    let fixture = Fixture::new(ExecutionRestrictions::unrestricted());
    let binding = read_binding();
    let prepared = fixture
        .authorization
        .prepare(&binding)
        .expect("healthy initial preparation");
    prepared
        .check_current(&binding)
        .expect("healthy current check");
    let publication = fixture.publication.clone();
    assert!(
        std::thread::spawn(move || {
            let _change = publication
                .begin_owner_change()
                .expect("real owner publication");
            panic!("test panic while holding actual publication custody");
        })
        .join()
        .is_err()
    );
    let current_error: meerkat_core::OperationAuthorizationError = prepared
        .check_current(&binding)
        .expect_err("poisoned publication cannot validate old entry");
    let prepare_error: meerkat_core::OperationAuthorizationError = fixture
        .authorization
        .prepare(&binding)
        .err()
        .expect("poisoned publication cannot issue replacement authority");
    assert_eq!(
        meerkat_core::ToolError::from(current_error).error_code(),
        "operation_authorization_unavailable"
    );
    assert_eq!(
        meerkat_core::ToolError::from(prepare_error).error_code(),
        "operation_authorization_unavailable"
    );
}
