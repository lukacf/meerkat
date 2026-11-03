//! Canonical policy transitions on SQLite and the actual native provider.
//! Pending records below model named durable checkpoints, not a fabricated
//! native build identity, held activation, or a claimed callback execution.
use super::*;
use crate::installed_host::dynamic_members::PreparedMemberClassTools;
use meerkat_authorization_contracts::{constraints::AudienceRef, evidence::EvidenceId};
use meerkat_core::{
    AgentToolDispatcher, CompiledMemberToolGrants, PrincipalKind, ToolDef, TrustDomainId,
};
use meerkat_toolkit::host_lifetime::policy::{
    CompiledClassCallback, CompiledMemberClass, CompiledMemberClassInstance, PreparedNativeMember,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use toolkit_package::Id;

type MemberCheckProbe = Arc<dyn Fn(&MemberClassAdmission, usize) + Send + Sync>;

struct Rule {
    owner: PrincipalRef,
    class: CompiledMemberClass,
    member_current: Arc<AtomicBool>,
    available: AtomicBool,
    service_current: Arc<AtomicBool>,
    approved: Mutex<Vec<String>>,
    member_probe: Arc<Mutex<Option<MemberCheckProbe>>>,
    recovery_inputs: Mutex<Vec<Vec<MemberClassAdmission>>>,
}
struct Reservation {
    owner: PrincipalRef,
    current: Arc<AtomicBool>,
    provenance: serde_json::Map<String, serde_json::Value>,
    member_probe: Option<(MemberClassAdmission, Arc<Mutex<Option<MemberCheckProbe>>>)>,
    checks: AtomicUsize,
}
impl ApplicationServicePolicyReservation for Reservation {
    fn owner(&self) -> &PrincipalRef {
        &self.owner
    }
    fn provenance(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.provenance
    }
    fn check_current(&self) -> Result<(), HostLifetimeError> {
        if let Some((admission, probe)) = &self.member_probe {
            let probe = probe.lock().unwrap().clone();
            if let Some(probe) = probe {
                probe(admission, self.checks.fetch_add(1, Ordering::SeqCst));
            }
        }
        if self.current.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(HostLifetimeError::Denied)
        }
    }
}
impl Rule {
    fn reservation(&self, current: Arc<AtomicBool>) -> Reservation {
        Reservation {
            owner: self.owner.clone(),
            current,
            provenance: Default::default(),
            member_probe: None,
            checks: AtomicUsize::new(0),
        }
    }
}
impl ApplicationServicePolicy for Rule {
    fn reserve(
        &self,
        _: &crate::private_originals::ApplicationServicePolicyRequest<'_>,
    ) -> Result<crate::private_originals::ApplicationServicePolicyDecision, HostLifetimeError> {
        Ok(crate::private_originals::ApplicationServicePolicyDecision::RequiresHuman)
    }
    fn reserve_member_policy(
        &self,
        request: &ApplicationMemberPolicyRequest<'_>,
    ) -> Result<ApplicationToolPolicyRecoveryDecision, HostLifetimeError> {
        if !self.available.load(Ordering::SeqCst) {
            return Err(HostLifetimeError::Unavailable);
        }
        if request.owner != &self.owner
            || request.admission.class != self.class
            || request.admission.instance.member.context_generation != 1
            || !self.member_current.load(Ordering::SeqCst)
        {
            return Err(HostLifetimeError::Denied);
        }
        let mut reservation = self.reservation(self.member_current.clone());
        reservation.member_probe = Some((request.admission.clone(), self.member_probe.clone()));
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(Box::new(
            reservation,
        )))
    }
    fn reserve_tool_policy_recovery(
        &self,
        request: &ApplicationToolPolicyRecoveryRequest<'_>,
    ) -> Result<ApplicationToolPolicyRecoveryDecision, HostLifetimeError> {
        self.recovery_inputs
            .lock()
            .unwrap()
            .push(request.member_admissions.to_vec());
        if request.decision.owner != self.owner
            || request.canonical_json
                != request
                    .decision
                    .proposal
                    .candidate_canonical_json
                    .as_bytes()
            || !self
                .approved
                .lock()
                .unwrap()
                .contains(&request.decision.approval_id)
            || !self.service_current.load(Ordering::SeqCst)
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(Box::new(
            self.reservation(self.service_current.clone()),
        )))
    }
}
struct Tools {
    defs: Arc<[Arc<ToolDef>]>,
}
#[async_trait::async_trait]
impl AgentToolDispatcher for Tools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.defs.clone()
    }
    async fn dispatch(
        &self,
        _: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        panic!("policy checkpoint control does not dispatch")
    }
}
fn principal() -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        "application",
        TrustDomainId::new("classes").unwrap(),
    )
    .unwrap()
}
fn class() -> CompiledMemberClass {
    CompiledMemberClass {
        purpose: Default::default(),
        id: Id::new("project").unwrap(),
        profile: "project".into(),
        factory: Id::new("trusted-project").unwrap(),
        callback_account: None,
        callbacks: vec![CompiledClassCallback {
            name: "project_read".into(),
            description: "Read selected project".into(),
            input_schema: serde_json::json!({"type":"object"}),
        }],
        grants: vec![CompiledMemberToolGrant {
            tool_name: "project_read".into(),
            action: CompiledMemberToolAction::Invoke,
            consequence: CompiledToolConsequence::R0,
        }],
    }
}
fn instance(class: &CompiledMemberClass) -> CompiledMemberClassInstance {
    CompiledMemberClassInstance {
        class: class.id.clone(),
        member: PreparedNativeMember {
            member: MobMemberBinding {
                mob_id: "fixture".into(),
                member: "project-one".into(),
                role: class.profile.clone(),
            },
            logical_owner: principal(),
            context_generation: 1,
            audience: AudienceRef::Principal {
                principal: principal(),
            },
            setup_capability: None,
        },
    }
}
fn request(class: &CompiledMemberClass) -> MemberClassAdmissionRequest {
    MemberClassAdmissionRequest {
        id: Id::new("request-one").unwrap(),
        instance: instance(class),
        tools: PreparedMemberClassTools::from_factory(
            class,
            &class.factory,
            Arc::new(Tools {
                defs: vec![Arc::new(class.callbacks[0].definition().unwrap())].into(),
            }),
        )
        .unwrap(),
    }
}
async fn fixture() -> (
    tempfile::TempDir,
    PreparedInstalledToolPolicy,
    Arc<ServicePolicyOwner>,
    Arc<Rule>,
) {
    fixture_with_class(class()).await
}
async fn fixture_with_class(
    selected: CompiledMemberClass,
) -> (
    tempfile::TempDir,
    PreparedInstalledToolPolicy,
    Arc<ServicePolicyOwner>,
    Arc<Rule>,
) {
    let directory = tempfile::tempdir().unwrap();
    let store: Arc<dyn ToolkitStore> =
        Arc::new(toolkit_store::SqliteStore::open(directory.path().join("state.sqlite3")).unwrap());
    let bytes = CompiledApplicationToolPolicy::new(
        PolicyProviderId::new("application").unwrap(),
        PolicyId::new("selected").unwrap(),
        PolicyRevision(1),
        CompiledPolicySourceProvenance {
            source_id: "fixture".into(),
            source_digest: PolicyDigest::from_canonical_bytes(b"fixture"),
        },
        vec![CompiledMemberToolGrants {
            member_identity: "coordinator".into(),
            grants: vec![],
        }],
    )
    .unwrap()
    .canonical_json()
    .unwrap();
    let policy = PreparedInstalledToolPolicy::from_canonical(&bytes).unwrap();
    let rule = Arc::new(Rule {
        owner: principal(),
        class: selected,
        member_current: Arc::new(AtomicBool::new(true)),
        available: AtomicBool::new(true),
        service_current: Arc::new(AtomicBool::new(true)),
        approved: Mutex::new(vec![]),
        member_probe: Arc::new(Mutex::new(None)),
        recovery_inputs: Mutex::new(Vec::new()),
    });
    let owner = policy
        .bind_service_owner(
            store,
            StoreScope {
                realm: meerkat_core::mob_realm_id("fixture").unwrap().to_string(),
                owner: "deployment".into(),
            },
            principal(),
            Some(rule.clone()),
            vec![rule.class.clone()],
        )
        .await
        .unwrap();
    (directory, policy, owner, rule)
}
async fn pending(owner: &Arc<ServicePolicyOwner>, accepted: bool) {
    let _guard = owner.exclusion.lock().await;
    let (stored, mut slot) = owner.read().await.unwrap();
    let admission = &mut slot.admissions[0];
    admission.native_surface = admission.installed_surface.clone();
    admission.phase = MemberClassAdmissionPhase::Prepared {
        context_generation: 1,
        context: EvidenceId::new("checkpoint-context").unwrap(),
    };
    admission.destination = Some(MemberClassDestination {
        context: EvidenceId::new("checkpoint-context").unwrap(),
        context_generation: 1,
    });
    let admission = admission.clone();
    let candidate = owner.member_candidate(&slot.current, &admission).unwrap();
    slot.member_pending = Some(MemberPolicyDecision {
        admission,
        prior: slot.current.clone(),
        candidate: candidate.clone(),
    });
    owner
        .change(&stored, &slot, "member-policy-pending")
        .await
        .unwrap();
    *owner.observed_current.lock().unwrap() = None;
    if accepted {
        owner
            .provider
            .accept_canonical_json(candidate.as_bytes())
            .unwrap();
    }
}
#[tokio::test]
async fn denied_pending_is_narrowed_after_either_possible_native_outcome() {
    for accepted in [false, true] {
        let (_dir, policy, owner, rule) = fixture().await;
        policy
            .admit_member_class(request(&rule.class))
            .await
            .unwrap();
        pending(&owner, accepted).await;
        rule.member_current.store(false, Ordering::SeqCst);
        owner.recover().await.unwrap();
        let (_, slot) = owner.read().await.unwrap();
        assert!(slot.member_pending.is_none());
        assert_eq!(slot.admissions[0].phase, MemberClassAdmissionPhase::Retired);
        let current =
            CompiledApplicationToolPolicy::parse_canonical_json(slot.current.as_bytes()).unwrap();
        assert_eq!(current.members, policy.compiled().members);
        assert_eq!(current.revision.0, 3);
        owner.check_current().unwrap();
    }
}
#[tokio::test]
async fn unavailable_pending_retains_exact_retryable_intent_then_explicit_retire_removes_native_row()
 {
    let (_dir, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    pending(&owner, true).await;
    let (_, before) = owner.read().await.unwrap();
    rule.available.store(false, Ordering::SeqCst);
    assert_eq!(
        owner.recover().await.unwrap_err(),
        HostLifetimeError::Unavailable
    );
    let (_, after) = owner.read().await.unwrap();
    assert_eq!(
        after.member_pending.unwrap().candidate,
        before.member_pending.unwrap().candidate
    );
    assert!(matches!(
        after.admissions[0].phase,
        MemberClassAdmissionPhase::Prepared { .. }
    ));
    rule.available.store(true, Ordering::SeqCst);
    owner.recover().await.unwrap();
    admitted.retire().await.unwrap();
    let (_, slot) = owner.read().await.unwrap();
    let current =
        CompiledApplicationToolPolicy::parse_canonical_json(slot.current.as_bytes()).unwrap();
    assert_eq!(current.members, policy.compiled().members);
}
#[tokio::test]
async fn retirement_preserves_existing_service_decision_and_its_live_reservation() {
    let (_dir, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let original = PrivateOriginalRef {
        route: Id::new("route").unwrap(),
        original: EvidenceId::new("original").unwrap(),
        event_sha256: sha(b"original"),
    };
    let acquisition = StoreKey {
        realm: owner.key.realm.clone(),
        owner: "toolkit/acquisition/deployment".into(),
        id: "request-service".into(),
    };
    let prepared = owner
        .prepare(
            "coordinator",
            ServiceToolPolicySelection {
                tools: vec![ServiceToolInvocation {
                    name: "service_read".into(),
                    consequence: CompiledToolConsequence::R0,
                }],
                catalog: None,
            },
            None,
            original,
            acquisition,
        )
        .await
        .unwrap();
    rule.approved
        .lock()
        .unwrap()
        .push(prepared.proposal.sha256().unwrap());
    let approval = ServiceToolPolicyApproval::until_superseded(&prepared.proposal).unwrap();
    let mut transaction = prepared.begin(&approval, &|| Ok(())).await.unwrap();
    transaction.publish(&|| Ok(())).await.unwrap();
    drop(transaction);
    admitted.retire().await.unwrap();
    assert!(owner.read().await.unwrap().1.decision.is_some());
    rule.service_current.store(false, Ordering::SeqCst);
    assert!(owner.check_current().is_err());
    assert!(owner.recover().await.is_err());
}

#[derive(Clone, Copy, Debug)]
enum OtherRetirementOrder {
    BeforeA,
    BeforeReadmission,
    AfterReadmission,
}

#[tokio::test]
async fn service_snapshot_preserves_class_rows_through_retirement_and_conditional_readmission() {
    for order in [
        OtherRetirementOrder::BeforeA,
        OtherRetirementOrder::BeforeReadmission,
        OtherRetirementOrder::AfterReadmission,
    ] {
        for native_accepted in [false, true] {
            for revoke in [false, true] {
                service_snapshot_interleaving(order, native_accepted, revoke).await;
            }
        }
    }
}
async fn service_snapshot_interleaving(
    order: OtherRetirementOrder,
    native_accepted: bool,
    revoke: bool,
) {
    eprintln!("other_retirement={order:?}, native_accepted={native_accepted}, revoke={revoke}");
    let (_dir, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    // Existing named checkpoint fixture, then actual canonical recovery and
    // provider publication. This does not claim native member construction.
    pending(&owner, true).await;
    owner.recover().await.unwrap();
    let member = admitted.instance().member.member.clone();
    let (prior, before) = owner.read().await.unwrap();
    let original = PrivateOriginalRef {
        route: Id::new("route").unwrap(),
        original: EvidenceId::new("service-original").unwrap(),
        event_sha256: sha(b"service-original"),
    };
    let acquisition = StoreKey {
        realm: owner.key.realm.clone(),
        owner: "toolkit/acquisition/deployment".into(),
        id: "service-request".into(),
    };
    let select = |name: &str| ServiceToolPolicySelection {
        tools: vec![ServiceToolInvocation {
            name: name.into(),
            consequence: CompiledToolConsequence::R0,
        }],
        catalog: None,
    };
    let prepared = owner
        .prepare(
            "coordinator",
            select("service_read"),
            None,
            original.clone(),
            acquisition.clone(),
        )
        .await
        .unwrap();
    assert_eq!(prepared.prior, prior);
    assert_eq!(prepared.member_admissions, before.admissions);
    let candidate = CompiledApplicationToolPolicy::parse_canonical_json(
        prepared.proposal.candidate_canonical_json.as_bytes(),
    )
    .unwrap();
    let dynamic = rule
        .class
        .materialize(&owner.member_realm().unwrap(), admitted.instance())
        .unwrap();
    assert!(candidate.members.contains(&dynamic));
    rule.approved
        .lock()
        .unwrap()
        .push(prepared.proposal.sha256().unwrap());
    let approval = ServiceToolPolicyApproval::until_superseded(&prepared.proposal).unwrap();
    let mut transaction = prepared.begin(&approval, &|| Ok(())).await.unwrap();
    transaction.publish(&|| Ok(())).await.unwrap();
    assert_eq!(
        transaction.observation().native,
        Some(ServiceToolPolicyNativeOutcome::Accepted)
    );
    drop(transaction);
    assert!(
        rule.recovery_inputs
            .lock()
            .unwrap()
            .iter()
            .any(|admissions| admissions == &before.admissions)
    );

    let mut second_original = original;
    second_original.original = EvidenceId::new("second-service-original").unwrap();
    second_original.event_sha256 = sha(b"second-service-original");
    let mut second_acquisition = acquisition;
    second_acquisition.id = "second-service-request".into();
    let stale = owner
        .prepare(
            "coordinator",
            select("second_read"),
            None,
            second_original,
            second_acquisition,
        )
        .await
        .unwrap();
    let stale_approval = ServiceToolPolicyApproval::until_superseded(&stale.proposal).unwrap();
    let stale_prior = stale.prior.clone();
    let mut unrelated = request(&rule.class);
    unrelated.id = Id::new("unrelated-admission").unwrap();
    unrelated.instance.member.member.member = "project-two".into();
    let unrelated = policy.admit_member_class(unrelated).await.unwrap();
    let (advanced, slot) = owner.read().await.unwrap();
    assert_ne!(advanced.revision, stale_prior.revision);
    assert_eq!(
        version(slot.current.as_bytes()).unwrap(),
        stale.proposal.prior,
        "admission metadata changed while the compiled policy did not"
    );
    let native = owner.provider.snapshot(&candidate.policy_id).unwrap();
    let events = owner.store.read_events(&owner.key, 0, 128).await.unwrap();
    assert!(matches!(
        stale.begin(&stale_approval, &|| Ok(())).await,
        Err(HostLifetimeError::Denied)
    ));
    assert_eq!(owner.read().await.unwrap().0, advanced);
    assert_eq!(
        owner.store.read_events(&owner.key, 0, 128).await.unwrap(),
        events
    );
    assert!(Arc::ptr_eq(
        &native,
        &owner.provider.snapshot(&candidate.policy_id).unwrap()
    ));
    if matches!(order, OtherRetirementOrder::BeforeA) {
        unrelated.retire().await.unwrap();
    }
    admitted.retire().await.unwrap();
    owner.recover().await.unwrap();
    let (_, retired_slot) = owner.read().await.unwrap();
    let retired = retired_slot
        .admissions
        .iter()
        .find(|a| a.id == *admitted.admission_id())
        .unwrap()
        .clone();
    assert_eq!(retired.phase, MemberClassAdmissionPhase::Retired);
    let saved = retired_slot.decision.as_ref().unwrap().clone();
    assert!(
        CompiledApplicationToolPolicy::parse_canonical_json(
            saved.proposal.candidate_canonical_json.as_bytes()
        )
        .unwrap()
        .members
        .contains(&dynamic),
        "historical approval retains the exact row it originally covered"
    );
    let current =
        CompiledApplicationToolPolicy::parse_canonical_json(retired_slot.current.as_bytes())
            .unwrap();
    assert_eq!(current.members.len(), 1);
    assert_eq!(current.members[0].member_identity, "coordinator");
    assert_eq!(
        current.members[0].grants,
        candidate
            .members
            .iter()
            .find(|row| row.member_identity == "coordinator")
            .unwrap()
            .grants
    );
    assert!(
        rule.recovery_inputs
            .lock()
            .unwrap()
            .iter()
            .any(|admissions| admissions.contains(&retired))
    );

    if matches!(order, OtherRetirementOrder::BeforeReadmission) {
        unrelated.retire().await.unwrap();
    }
    let before_readmission = owner.read().await.unwrap().1.current;
    let checkpoint = policy.member_class_checkpoint(&member).await.unwrap();
    let next_id = Id::new("explicit-readmission").unwrap();
    let mut next_request = request(&rule.class);
    next_request.id = next_id.clone();
    let successor = policy
        .readmit_member_class(checkpoint, next_request)
        .await
        .unwrap();
    assert_eq!(
        owner.read().await.unwrap().1.current,
        before_readmission,
        "readmission does not republish the retired native row"
    );
    if matches!(order, OtherRetirementOrder::AfterReadmission) {
        unrelated.retire().await.unwrap();
    }
    owner.recover().await.unwrap();
    let (_, after) = owner.read().await.unwrap();
    let reserved = after.admissions.iter().find(|a| a.id == next_id).unwrap();
    assert_ne!(reserved.id, retired.id);
    assert_eq!(reserved.phase, MemberClassAdmissionPhase::Reserved);
    assert_eq!(
        CompiledApplicationToolPolicy::parse_canonical_json(after.current.as_bytes())
            .unwrap()
            .members,
        current.members,
        "unrelated retirement changes no preserved service grants"
    );
    assert_eq!(
        canonical_json(after.decision.as_ref().unwrap()).unwrap(),
        canonical_json(&saved).unwrap()
    );
    let last_retired = if matches!(order, OtherRetirementOrder::BeforeA) {
        admitted.admission_id()
    } else {
        unrelated.admission_id()
    };
    assert_eq!(
        &after.member_decision.as_ref().unwrap().admission.id,
        last_retired,
        "historical approval survives both A and unrelated B as the latest member decision"
    );
    assert!(
        rule.recovery_inputs
            .lock()
            .unwrap()
            .iter()
            .any(|admissions| admissions.contains(reserved))
    );
    assert!(owner.is_observed_current(after.current.as_bytes()).unwrap());
    assert_eq!(
        owner
            .provider
            .snapshot(&candidate.policy_id)
            .unwrap()
            .provenance()
            .revision,
        version(after.current.as_bytes()).unwrap().revision
    );
    assert!(
        owner
            .with_current_member(
                admitted.admission_id(),
                admitted.class(),
                admitted.instance(),
                || -> Result<(), HostLifetimeError> {
                    panic!("stale A reached an effect after re-admission")
                }
            )
            .await
            .is_err()
    );

    // Retain the actual canonical checkpoint around native acceptance. A cold
    // recovery must decode it before it can reserve and publish the candidate.
    pending(&owner, native_accepted).await;
    let (stored_pending, retained_pending) = owner.read().await.unwrap();
    assert!(retained_pending.member_pending.is_some());
    assert_eq!(retained_pending.current, after.current);
    assert_eq!(
        canonical_json(retained_pending.decision.as_ref().unwrap()).unwrap(),
        canonical_json(&saved).unwrap()
    );
    for mutation in 0..8 {
        let mut changed = retained_pending.clone();
        match mutation {
            0 => changed.member_pending = None,
            1 => {
                changed.admissions[0].phase = MemberClassAdmissionPhase::Active {
                    context: EvidenceId::new("checkpoint-context").unwrap(),
                    context_generation: 1,
                }
            }
            2 => {
                changed
                    .member_pending
                    .as_mut()
                    .unwrap()
                    .admission
                    .instance
                    .member
                    .context_generation += 1
            }
            3 => {
                changed
                    .member_pending
                    .as_mut()
                    .unwrap()
                    .admission
                    .installed_surface = Some(vec![])
            }
            4 => changed.member_pending.as_mut().unwrap().prior.push(' '),
            5 => changed.member_pending.as_mut().unwrap().candidate.push(' '),
            6 => changed.admissions[0].class.factory = Id::new("unselected").unwrap(),
            7 => {
                let prior =
                    CompiledApplicationToolPolicy::parse_canonical_json(changed.current.as_bytes())
                        .unwrap();
                let narrowed = CompiledApplicationToolPolicy::new(
                    prior.provider_id,
                    prior.policy_id,
                    prior.revision,
                    prior.source,
                    vec![],
                )
                .unwrap();
                changed.current = String::from_utf8(narrowed.canonical_json().unwrap()).unwrap();
            }
            _ => unreachable!(),
        }
        let mut tampered = stored_pending.clone();
        tampered.data = encode(&changed).unwrap();
        assert!(
            owner.decode(&tampered).is_err(),
            "invalid historical narrowing mutation {mutation}"
        );
    }
    rule.member_current.store(!revoke, Ordering::SeqCst);
    owner.recover().await.unwrap();
    let (_, recovered) = owner.read().await.unwrap();
    assert!(recovered.member_pending.is_none());
    assert_eq!(
        canonical_json(recovered.decision.as_ref().unwrap()).unwrap(),
        canonical_json(&saved).unwrap()
    );
    let recovered_policy =
        CompiledApplicationToolPolicy::parse_canonical_json(recovered.current.as_bytes()).unwrap();
    assert_eq!(
        recovered_policy
            .members
            .iter()
            .find(|row| row.member_identity == "coordinator"),
        candidate
            .members
            .iter()
            .find(|row| row.member_identity == "coordinator")
    );
    assert_eq!(recovered_policy.members.contains(&dynamic), !revoke);
    assert!(if revoke {
        recovered.admissions[0].phase == MemberClassAdmissionPhase::Retired
    } else {
        matches!(
            recovered.admissions[0].phase,
            MemberClassAdmissionPhase::Prepared { .. }
        )
    });
    assert!(
        owner
            .with_current_member(
                admitted.admission_id(),
                admitted.class(),
                admitted.instance(),
                || -> Result<(), HostLifetimeError> {
                    panic!("old A reached an effect after recovery")
                }
            )
            .await
            .is_err()
    );
    assert!(
        owner
            .is_observed_current(recovered.current.as_bytes())
            .unwrap()
    );
    assert_eq!(
        owner
            .provider
            .snapshot(&candidate.policy_id)
            .unwrap()
            .provenance()
            .revision,
        version(recovered.current.as_bytes()).unwrap().revision
    );
    successor.retire().await.unwrap();
}

// These exercise the real customizer method and its executable overlay, not
// native held activation or an ordinary authorized model turn.
struct BuildCustomizer {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    revoke: Option<Arc<AtomicBool>>,
}
#[async_trait::async_trait]
impl meerkat_mobkit::identity_first::AgentCustomizer for BuildCustomizer {
    async fn customize_build(
        &self,
        _: &meerkat_mobkit::identity_first::AgentBuildContext,
        _: &meerkat_mobkit::identity_first::DurableAgentSpec,
        _: &mut meerkat_mobkit::identity_first::AgentBuildDraft,
    ) -> Result<(), meerkat_mobkit::identity_first::CustomizerError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(current) = &self.revoke {
            current.store(false, Ordering::SeqCst);
        }
        Ok(())
    }
}
struct BuildFactory {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    executed: Arc<Mutex<Vec<usize>>>,
    changed_schema: bool,
}
struct BuildTools {
    defs: Arc<[Arc<ToolDef>]>,
    construction: usize,
    executed: Arc<Mutex<Vec<usize>>>,
}
#[async_trait::async_trait]
impl AgentToolDispatcher for BuildTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.defs.clone()
    }
    async fn dispatch(
        &self,
        call: meerkat_core::ToolCallView<'_>,
    ) -> Result<meerkat_core::ToolDispatchOutcome, meerkat_core::ToolError> {
        self.executed.lock().unwrap().push(self.construction);
        Ok(meerkat_core::ToolResult::new(
            call.id.into(),
            "reconstructed callback executed".into(),
            false,
        )
        .into())
    }
}
impl crate::installed_host::dynamic_members::MemberClassFactory for BuildFactory {
    fn prepare(
        &self,
        class: &CompiledMemberClass,
        _: &CompiledMemberClassInstance,
    ) -> Result<PreparedMemberClassTools, HostLifetimeError> {
        let construction = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let mut definition = class.callbacks[0].definition()?;
        if self.changed_schema {
            definition.input_schema = serde_json::json!({"type":"string"});
        }
        PreparedMemberClassTools::from_factory(
            class,
            &class.factory,
            Arc::new(BuildTools {
                defs: vec![Arc::new(definition)].into(),
                construction,
                executed: self.executed.clone(),
            }),
        )
    }
}
// The customizer validates the actual selected mob definition. No member is
// spawned here; this owner supplies only the real runtime-services handle.
async fn build_runtime() -> meerkat_mob::MobHandle {
    use meerkat::{AgentFactory, Config, EphemeralSessionService, FactoryAgentBuilder};
    use meerkat_mob::{
        MobBuilder, MobDefinition, MobId, MobRuntimeMode, MobStorage, Profile, ProfileBinding,
        ProfileName, ToolConfig,
    };
    let mut definition = MobDefinition::explicit(MobId::from("fixture"));
    definition.profiles.insert(
        ProfileName::from("project"),
        ProfileBinding::Inline(Box::new(Profile {
            model_fallback: None,
            model: "claude-sonnet-4-5".into(),
            provider: None,
            self_hosted_server_id: None,
            image_generation_provider: None,
            auto_compact_threshold: None,
            resume_overrides: vec![],
            skills: vec![],
            tools: ToolConfig {
                comms: true,
                ..Default::default()
            },
            peer_description: "unspawned class customizer control".into(),
            external_addressable: true,
            backend: None,
            runtime_mode: MobRuntimeMode::TurnDriven,
            max_inline_peer_notifications: None,
            output_schema: None,
            provider_params: None,
            durable: false,
        })),
    );
    let service = Arc::new(EphemeralSessionService::new(
        FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default()),
        1,
    ));
    MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service)
        .with_runtime_adapter(Arc::new(meerkat_runtime::MeerkatMachine::ephemeral()))
        .allow_ephemeral_sessions(true)
        .create()
        .await
        .unwrap()
}

fn build_arguments_for(
    runtime: &meerkat_mob::MobHandle,
    identity: &str,
) -> (
    meerkat_mobkit::identity_first::AgentBuildContext,
    meerkat_mobkit::identity_first::DurableAgentSpec,
    meerkat_mobkit::identity_first::AgentBuildDraft,
) {
    let (mut context, spec, draft) = build_arguments(identity);
    context.runtime_services =
        meerkat_mobkit::identity_first::AgentRuntimeServices::new(runtime.clone());
    (context, spec, draft)
}

fn build_arguments(
    identity: &str,
) -> (
    meerkat_mobkit::identity_first::AgentBuildContext,
    meerkat_mobkit::identity_first::DurableAgentSpec,
    meerkat_mobkit::identity_first::AgentBuildDraft,
) {
    use meerkat_mobkit::identity_first::*;
    let identity = AgentIdentity::parse(identity).unwrap();
    (
        AgentBuildContext {
            identity: identity.clone(),
            active_peers: vec![],
            managed_edges: vec![],
            runtime_services: Default::default(),
        },
        DurableAgentSpec {
            identity,
            profile: meerkat_mob::ProfileName::from("project"),
            addressability: Default::default(),
            display_name: None,
            labels: Default::default(),
            context: None,
            additional_instructions: vec![],
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        },
        serde_json::from_value(
            serde_json::json!({"model":null,"system_prompt":null,"app_context":null}),
        )
        .unwrap(),
    )
}
#[tokio::test]
async fn customizer_reconstructs_only_selected_instance_on_each_build() {
    let (_directory, policy, owner, rule) = fixture().await;
    let runtime = build_runtime().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let factory_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executed = Arc::new(Mutex::new(Vec::new()));
    let customizer = policy.member_class_customizer(
        Arc::new(BuildCustomizer {
            calls: calls.clone(),
            revoke: None,
        }),
        Arc::new(BuildFactory {
            calls: factory_calls.clone(),
            executed: executed.clone(),
            changed_schema: false,
        }),
    );
    let (context, spec, mut draft) = build_arguments_for(&runtime, "project-one");
    customizer
        .customize_build(&context, &spec, &mut draft)
        .await
        .unwrap();
    assert_eq!(
        draft.local_external_tools.dispatcher().unwrap().tools()[0].identity(),
        admitted.tools.as_ref().unwrap().identities()[0]
    );
    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    draft
        .local_external_tools
        .dispatcher()
        .unwrap()
        .dispatch(meerkat_core::ToolCallView {
            id: "first",
            name: "project_read",
            args: &args,
        })
        .await
        .unwrap();
    let (context, spec, mut draft) = build_arguments_for(&runtime, "project-one");
    customizer
        .customize_build(&context, &spec, &mut draft)
        .await
        .unwrap();
    draft
        .local_external_tools
        .dispatcher()
        .unwrap()
        .dispatch(meerkat_core::ToolCallView {
            id: "second",
            name: "project_read",
            args: &args,
        })
        .await
        .unwrap();
    assert_eq!(*executed.lock().unwrap(), vec![1, 2]);
    assert_eq!(factory_calls.load(Ordering::SeqCst), 2);
    assert_eq!(owner.read().await.unwrap().1.admissions.len(), 1);
    let (context, spec, mut static_draft) = build_arguments_for(&runtime, "coordinator");
    customizer
        .customize_build(&context, &spec, &mut static_draft)
        .await
        .unwrap();
    assert!(!static_draft.local_external_tools.is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(factory_calls.load(Ordering::SeqCst), 2);
    admitted.retire().await.unwrap();
    let (context, spec, mut draft) = build_arguments_for(&runtime, "project-one");
    assert!(
        customizer
            .customize_build(&context, &spec, &mut draft)
            .await
            .is_err()
    );
    assert_eq!(factory_calls.load(Ordering::SeqCst), 2);
    assert!(
        runtime.list_members().await.is_empty(),
        "customizer control spawned no member"
    );
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn customizer_refuses_revocation_during_inner_await_and_changed_callback_schema() {
    let (_directory, policy, _owner, rule) = fixture().await;
    let _admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let factory_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let executed = Arc::new(Mutex::new(Vec::new()));
    let customizer = policy.member_class_customizer(
        Arc::new(BuildCustomizer {
            calls: Default::default(),
            revoke: Some(rule.member_current.clone()),
        }),
        Arc::new(BuildFactory {
            calls: factory_calls.clone(),
            executed: executed.clone(),
            changed_schema: false,
        }),
    );
    let (context, spec, mut draft) = build_arguments("project-one");
    assert!(
        customizer
            .customize_build(&context, &spec, &mut draft)
            .await
            .is_err()
    );
    assert_eq!(factory_calls.load(Ordering::SeqCst), 0);
    assert!(!draft.local_external_tools.is_some());
    rule.member_current.store(true, Ordering::SeqCst);
    let customizer = policy.member_class_customizer(
        Arc::new(BuildCustomizer {
            calls: Default::default(),
            revoke: None,
        }),
        Arc::new(BuildFactory {
            calls: factory_calls.clone(),
            executed: executed.clone(),
            changed_schema: true,
        }),
    );
    assert!(
        customizer
            .customize_build(&context, &spec, &mut draft)
            .await
            .is_err()
    );
    assert!(!draft.local_external_tools.is_some());
}
#[test]
fn native_surface_observation_keeps_native_tools_without_adding_class_grants() {
    use meerkat_core::{PreparedToolCatalog, PreparedToolSurface};
    let selected = class();
    let callback = selected.callbacks[0].definition().unwrap().identity();
    let native = ToolDef::new(
        "actual_native_control",
        "Actual native fixture output",
        serde_json::json!({"type":"object"}),
    )
    .identity();
    let surface = PreparedToolSurface {
        tools: vec![callback.clone(), native.clone()],
        projected: vec![callback.clone()],
        catalog: PreparedToolCatalog::VisibleOnly,
    };
    let captured = native_surface_identities(&surface).unwrap();
    assert!(captured.contains(&callback) && captured.contains(&native));
    assert_eq!(selected.grants.len(), 1);
    assert_eq!(selected.grants[0].tool_name, "project_read");
    let empty = PreparedToolSurface {
        tools: vec![],
        projected: vec![],
        catalog: PreparedToolCatalog::VisibleOnly,
    };
    assert!(native_surface_identities(&empty).unwrap().is_empty());
    let invalid = PreparedToolSurface {
        tools: vec![callback],
        projected: vec![native],
        catalog: PreparedToolCatalog::VisibleOnly,
    };
    assert!(native_surface_identities(&invalid).is_err());
}

#[path = "gateway_declarations.rs"]
mod gateway_declarations;
#[path = "registration_generation.rs"]
mod registration_generation;

// These controls use genuine Core registration witnesses and the actual
// canonical policy store. They do not claim held activation or actor recovery;
// the native fixture exercises those production paths separately.
async fn register(
    machine: &meerkat_runtime::MeerkatMachine,
    session: &meerkat_core::SessionId,
) -> RuntimeSessionRegistrationWitness {
    machine.register_session(session.clone()).await.unwrap();
    machine
        .current_session_registration_witness(session)
        .await
        .unwrap()
}

#[tokio::test]
async fn static_registration_reads_do_not_reenter_shared_policy_recovery() {
    let (_directory, _policy, owner, _rule) = fixture().await;
    let realm = owner.member_realm().unwrap();
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let first_session = meerkat_core::SessionId::new();
    let second_session = meerkat_core::SessionId::new();
    let first = register(&machine, &first_session).await;
    let second = register(&machine, &second_session).await;
    let member = MobMemberBinding {
        mob_id: "fixture".into(),
        member: "coordinator".into(),
        role: "coordinator".into(),
    };
    let peer = MobMemberBinding {
        mob_id: "fixture".into(),
        member: "helper".into(),
        role: "helper".into(),
    };
    let observed = owner.observed_current.lock().unwrap().clone().unwrap();
    // Native boot can restore several static members at once. Neither read
    // may enter the policy mutation lane or invalidate its peer's observation.
    let exclusion = owner.exclusion.lock().await;
    let results = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        tokio::join!(
            owner.select_registration(&realm, &member, &first),
            owner.select_registration(&realm, &peer, &second),
        )
    })
    .await
    .expect("static registration must not enter policy recovery");
    assert_eq!(results.0.unwrap(), None);
    assert_eq!(results.1.unwrap(), None);
    assert_eq!(
        owner
            .expected_registration(&realm, &member, &first)
            .unwrap(),
        None
    );
    assert_eq!(
        owner.expected_registration(&realm, &peer, &second).unwrap(),
        None
    );
    assert!(Arc::ptr_eq(
        owner.observed_current.lock().unwrap().as_ref().unwrap(),
        &observed
    ));
    // An unavailable owner remains unavailable; the destination read cannot
    // manufacture a fresh policy observation to admit this registration.
    *owner.observed_current.lock().unwrap() = None;
    assert_eq!(
        owner
            .select_registration(&realm, &member, &first)
            .await
            .unwrap_err(),
        HostLifetimeError::Unavailable
    );
    drop(exclusion);
    machine.unregister_session(&first_session).await.unwrap();
    machine.unregister_session(&second_session).await.unwrap();
}

// Probe only the existing application reservation. Every mutation below still
// uses the actual canonical store and a Core-minted registration witness.
#[tokio::test]
async fn registration_metadata_keeps_unrelated_reads_and_withholds_failed_destination() {
    for revoke_after_write in [false, true] {
        let (_directory, policy, owner, rule) = fixture().await;
        let admitted = policy
            .admit_member_class(request(&rule.class))
            .await
            .unwrap();
        let member = admitted.instance().member.member.clone();
        let realm = owner.member_realm().unwrap();
        let machine = meerkat_runtime::MeerkatMachine::ephemeral();
        let session = meerkat_core::SessionId::new();
        let witness = register(&machine, &session).await;
        let static_session = meerkat_core::SessionId::new();
        let static_witness = register(&machine, &static_session).await;
        let static_member = MobMemberBinding {
            mob_id: "fixture".into(),
            member: "coordinator".into(),
            role: "coordinator".into(),
        };
        let before = owner.read().await.unwrap().1;
        let authority = owner
            .observed_current
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .authority
            .clone();
        let observations = Arc::new(Mutex::new(Vec::new()));
        let recorded = observations.clone();
        let weak = Arc::downgrade(&owner);
        let context = EvidenceId::new(session.to_string()).unwrap();
        let expected_context = context.clone();
        let selected_member = member.clone();
        let selected_realm = realm.clone();
        let selected_static_witness = static_witness.clone();
        let current = rule.member_current.clone();
        *rule.member_probe.lock().unwrap() = Some(Arc::new(move |admission, check| {
            // The new reservation is first checked before CAS. Its second
            // check is the real post-write boundary in select_registration.
            if admission
                .destination
                .as_ref()
                .is_some_and(|row| row.context == expected_context)
                && check == 1
            {
                let owner = weak.upgrade().unwrap();
                recorded.lock().unwrap().push((
                    owner.check_current(),
                    owner.contains_current_member("coordinator"),
                    owner.observed_admission(&static_member),
                    owner.expected_registration(
                        &selected_realm,
                        &static_member,
                        &selected_static_witness,
                    ),
                    owner.observed_admission(&selected_member),
                ));
                if revoke_after_write {
                    current.store(false, Ordering::SeqCst);
                }
            }
        }));
        let selected = owner.select_registration(&realm, &member, &witness).await;
        *rule.member_probe.lock().unwrap() = None;
        let after = owner.read().await.unwrap().1;
        let current_destination = owner.expected_registration(&realm, &member, &witness);
        let same_authority = owner
            .observed_current
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|observed| Arc::ptr_eq(&authority, &observed.authority));
        machine.unregister_session(&session).await.unwrap();
        machine.unregister_session(&static_session).await.unwrap();
        let observations = observations.lock().unwrap();
        assert_eq!(
            observations.len(),
            1,
            "actual post-CAS reservation check was observed"
        );
        let (policy, member_present, static_admission, static_destination, changing_admission) =
            &observations[0];
        assert_eq!(*policy, Ok(()));
        assert_eq!(*member_present, Ok(true));
        assert_eq!(*static_admission, Ok(None));
        assert_eq!(*static_destination, Ok(None));
        assert_eq!(*changing_admission, Err(HostLifetimeError::Unavailable));
        assert!(same_authority);
        assert_eq!(
            after.current, before.current,
            "destination selection changes no grants"
        );
        assert_eq!(
            after.admissions[0].destination.as_ref().unwrap().context,
            context
        );
        let mut expected = before.admissions;
        expected[0].destination = after.admissions[0].destination.clone();
        assert_eq!(after.admissions, expected);
        if revoke_after_write {
            assert_eq!(selected.unwrap_err(), HostLifetimeError::Denied);
            assert!(
                current_destination.is_err(),
                "committed destination stays withheld on revocation"
            );
            assert_eq!(
                owner.observed_admission(&member).unwrap_err(),
                HostLifetimeError::Unavailable
            );
        } else {
            assert_eq!(current_destination.unwrap(), selected.unwrap());
            assert!(owner.observed_admission(&member).unwrap().is_some());
        }
        assert!(
            owner
                .expected_registration(&realm, &member, &witness)
                .is_err(),
            "ending the actual native witness still refuses registration"
        );
    }
}

#[tokio::test]
async fn member_read_crosses_sibling_metadata_but_not_same_bytes_recovery() {
    for recover in [false, true] {
        let (_directory, policy, owner, rule) = fixture().await;
        let first = policy
            .admit_member_class(request(&rule.class))
            .await
            .unwrap();
        let mut second_request = request(&rule.class);
        second_request.id = Id::new("request-two").unwrap();
        second_request.instance.member.member.member = "project-two".into();
        let second = policy.admit_member_class(second_request).await.unwrap();
        let first_member = first.instance().member.member.clone();
        let before = owner.observed_admission(&first_member).unwrap().unwrap();
        let canonical = owner.read().await.unwrap().1.current;
        let authority = owner
            .observed_current
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .authority
            .clone();
        let realm = owner.member_realm().unwrap();
        let machine = meerkat_runtime::MeerkatMachine::ephemeral();
        let session = meerkat_core::SessionId::new();
        let witness = register(&machine, &session).await;
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let entered = Mutex::new(Some(entered));
        let (release, released) = std::sync::mpsc::channel();
        let released = Mutex::new(released);
        let selected = first.admission_id().clone();
        *rule.member_probe.lock().unwrap() = Some(Arc::new(move |admission, _| {
            if admission.id == selected {
                let entered = entered.lock().unwrap().take();
                if let Some(entered) = entered {
                    let _ = entered.send(());
                    released
                        .lock()
                        .unwrap()
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .expect("selected read must be released");
                }
            }
        }));
        let reading = owner.clone();
        let pending =
            tokio::task::spawn_blocking(move || reading.observed_admission(&first_member));
        let entered = tokio::time::timeout(std::time::Duration::from_secs(5), waiting).await;
        let changed = if recover {
            owner.recover().await
        } else {
            owner
                .select_registration(&realm, &second.instance().member.member, &witness)
                .await
                .map(|_| ())
        };
        let _ = release.send(());
        let read = pending.await;
        *rule.member_probe.lock().unwrap() = None;
        let after = owner.read().await.unwrap().1.current;
        let same_authority = owner
            .observed_current
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|observed| Arc::ptr_eq(&authority, &observed.authority));
        machine.unregister_session(&session).await.unwrap();
        entered.unwrap().unwrap();
        changed.unwrap();
        assert_eq!(after, canonical);
        assert_eq!(same_authority, !recover);
        if recover {
            assert_eq!(
                read.unwrap().unwrap_err(),
                HostLifetimeError::Unavailable,
                "a fresh accepted observation cannot satisfy a prior read even with identical bytes"
            );
        } else {
            assert_eq!(
                read.unwrap().unwrap(),
                Some(before),
                "a sibling's real destination write does not replace this member's authority"
            );
        }
    }
}

#[tokio::test]
async fn immutable_member_effect_is_excluded_and_survives_real_destination_allocation() {
    let (_directory, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let effect = || {
        assert!(
            owner.exclusion.try_lock().is_err(),
            "canonical exclusion covers the effect"
        );
        Ok(())
    };
    owner
        .with_current_member(
            admitted.admission_id(),
            admitted.class(),
            admitted.instance(),
            effect,
        )
        .await
        .unwrap();
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let session = meerkat_core::SessionId::new();
    let registration = register(&machine, &session).await;
    owner
        .select_registration(
            &owner.member_realm().unwrap(),
            &admitted.instance().member.member,
            &registration,
        )
        .await
        .unwrap();
    owner
        .with_current_member(
            admitted.admission_id(),
            admitted.class(),
            admitted.instance(),
            effect,
        )
        .await
        .unwrap();
    let mut wrong = admitted.instance().clone();
    wrong.member.logical_owner = PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        "another",
        TrustDomainId::new("classes").unwrap(),
    )
    .unwrap();
    assert!(
        owner
            .with_current_member(
                admitted.admission_id(),
                admitted.class(),
                &wrong,
                || -> Result<(), HostLifetimeError> {
                    panic!("wrong immutable subject reached effect")
                }
            )
            .await
            .is_err()
    );
    admitted.retire().await.unwrap();
    assert!(
        owner
            .with_current_member(
                admitted.admission_id(),
                admitted.class(),
                admitted.instance(),
                || -> Result<(), HostLifetimeError> { panic!("retired admission reached effect") }
            )
            .await
            .is_err()
    );
    machine.unregister_session(&session).await.unwrap();
}

#[tokio::test]
async fn same_registration_retry_advances_canonical_generation_without_changing_grants() {
    let (_directory, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    pending(&owner, false).await;
    owner.recover().await.unwrap();
    let member = admitted.instance().member.member.clone();
    let realm = owner.member_realm().unwrap();
    let before = owner.read().await.unwrap().1.current;
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let session = meerkat_core::SessionId::new();
    let witness = register(&machine, &session).await;
    let first = owner
        .select_registration(&realm, &member, &witness)
        .await
        .unwrap()
        .unwrap();
    let selected_revision = owner.read().await.unwrap().0.revision;
    assert_eq!(
        owner
            .expected_registration(&realm, &member, &witness)
            .unwrap(),
        Some(first.clone())
    );
    assert_eq!(
        owner.read().await.unwrap().0.revision,
        selected_revision,
        "read-only expectation never allocates another generation"
    );
    // The earlier native provision can fail after its destination was bound.
    // A new selection must advance even though Core retains the same witness.
    let retry = owner
        .select_registration(&realm, &member, &witness)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.context, first.context);
    assert_eq!(retry.context_generation, first.context_generation + 1);
    let (_, stored) = owner.read().await.unwrap();
    assert_eq!(stored.current, before);
    assert_eq!(stored.admissions[0].instance, *admitted.instance());
    assert_eq!(
        stored.admissions[0]
            .selected_member()
            .unwrap()
            .context_generation,
        retry.context_generation
    );
    assert_eq!(
        owner
            .expected_registration(&realm, &member, &witness)
            .unwrap(),
        Some(retry)
    );
    machine.unregister_session(&session).await.unwrap();
    assert!(
        owner
            .expected_registration(&realm, &member, &witness)
            .is_err()
    );
}

#[tokio::test]
async fn replacement_registration_and_new_session_require_exact_selection() {
    let (_directory, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let member = admitted.instance().member.member.clone();
    let realm = owner.member_realm().unwrap();
    let first_machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let session = meerkat_core::SessionId::new();
    let first = register(&first_machine, &session).await;
    let selected = owner
        .select_registration(&realm, &member, &first)
        .await
        .unwrap()
        .unwrap();
    // Even the same session ID on another actual machine is a different
    // registration. No manufactured witness or serialized epoch can pass.
    let second_machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let second = register(&second_machine, &session).await;
    assert_ne!(first, second);
    assert!(
        owner
            .expected_registration(&realm, &member, &second)
            .is_err()
    );
    first_machine.unregister_session(&session).await.unwrap();
    let recovered = owner
        .select_registration(&realm, &member, &second)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.context, selected.context);
    assert_eq!(
        recovered.context_generation, selected.context_generation,
        "a replacement registration keeps the same logical destination"
    );
    assert!(
        owner
            .expected_registration(&realm, &member, &first)
            .is_err()
    );
    second_machine.unregister_session(&session).await.unwrap();
    let fresh_session = meerkat_core::SessionId::new();
    let fresh = register(&second_machine, &fresh_session).await;
    let fresh_selection = owner
        .select_registration(&realm, &member, &fresh)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(fresh_selection.context, recovered.context);
    assert_eq!(
        fresh_selection.context_generation,
        recovered.context_generation + 1
    );
    assert_eq!(fresh_selection.logical_owner, selected.logical_owner);
    assert_eq!(fresh_selection.audience, selected.audience);
    assert!(
        owner
            .expected_registration(&realm, &member, &second)
            .is_err()
    );
    second_machine
        .unregister_session(&fresh_session)
        .await
        .unwrap();
}

#[tokio::test]
async fn conditional_readmission_preserves_generation_and_rejects_stale_retirement() {
    let (_directory, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let member = admitted.instance().member.member.clone();
    let realm = owner.member_realm().unwrap();
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let session = meerkat_core::SessionId::new();
    let witness = register(&machine, &session).await;
    let before = owner
        .select_registration(&realm, &member, &witness)
        .await
        .unwrap()
        .unwrap();
    admitted.retire().await.unwrap();
    machine.unregister_session(&session).await.unwrap();
    let retired = policy.member_class_checkpoint(&member).await.unwrap();
    let successor_request = || {
        let mut next = request(&rule.class);
        next.id = Id::new("request-two").unwrap();
        next
    };
    assert!(
        policy
            .admit_member_class(successor_request())
            .await
            .is_err(),
        "a random new ID cannot resurrect the logical member"
    );
    assert!(
        policy
            .readmit_member_class(retired.clone(), request(&rule.class))
            .await
            .is_err(),
        "the retired admission ID is never resurrected"
    );
    let mut stale = retired.clone();
    stale.revision -= 1;
    assert!(
        policy
            .readmit_member_class(stale, successor_request())
            .await
            .is_err()
    );
    let mut wrong = retired.clone();
    wrong.admission = Id::new("not-the-prior-admission").unwrap();
    assert!(
        policy
            .readmit_member_class(wrong, successor_request())
            .await
            .is_err()
    );
    rule.member_current.store(false, Ordering::SeqCst);
    assert!(
        policy
            .readmit_member_class(retired.clone(), successor_request())
            .await
            .is_err()
    );
    rule.member_current.store(true, Ordering::SeqCst);
    let next = successor_request();
    let successor = policy
        .readmit_member_class_declaration(retired, next.id, next.instance)
        .await
        .unwrap();
    assert!(
        successor.dispatcher().is_none(),
        "declaration does not fabricate a factory output"
    );
    assert!(
        admitted.retire().await.is_err(),
        "the old handle cannot retire its successor"
    );
    assert_eq!(
        owner.observed_admission(&member).unwrap().unwrap().id,
        *successor.admission_id()
    );
    let fresh = meerkat_core::SessionId::new();
    let fresh_witness = register(&machine, &fresh).await;
    let next = owner
        .select_registration(&realm, &member, &fresh_witness)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.context_generation, before.context_generation + 1);
    assert_ne!(next.context, before.context);
    let (_, stored) = owner.read().await.unwrap();
    assert_eq!(
        stored.admissions.len(),
        1,
        "one canonical logical member row"
    );
    let current =
        CompiledApplicationToolPolicy::parse_canonical_json(stored.current.as_bytes()).unwrap();
    assert_eq!(
        current.members,
        policy.compiled().members,
        "re-admission alone restores no grants and preserves all other members"
    );
    machine.unregister_session(&fresh).await.unwrap();
}

#[tokio::test]
async fn expired_application_selection_cannot_advance_or_expect_a_registration() {
    let (_directory, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let member = admitted.instance().member.member.clone();
    let realm = owner.member_realm().unwrap();
    let machine = meerkat_runtime::MeerkatMachine::ephemeral();
    let session = meerkat_core::SessionId::new();
    let witness = register(&machine, &session).await;
    owner
        .select_registration(&realm, &member, &witness)
        .await
        .unwrap();
    let before = owner.read().await.unwrap().0;
    rule.member_current.store(false, Ordering::SeqCst);
    assert!(
        owner
            .expected_registration(&realm, &member, &witness)
            .is_err()
    );
    assert!(
        owner
            .select_registration(&realm, &member, &witness)
            .await
            .is_err()
    );
    let after = owner.read().await.unwrap().0;
    assert_eq!(after, before);
    machine.unregister_session(&session).await.unwrap();
}

#[tokio::test]
async fn route_projection_requires_verified_generation_and_live_registration() {
    use meerkat_authorization_contracts::constraints::ResourceDomain;
    use meerkat_toolkit::host_lifetime::policy::{
        CompiledNativeSourceOrigin, CompiledNativeSourceRoute,
    };
    for loss in ["ended", "cold", "reselected", "replacement"] {
        let (_directory, policy, owner, rule) = fixture().await;
        let admitted = policy
            .admit_member_class(request(&rule.class))
            .await
            .unwrap();
        pending(&owner, false).await;
        owner.recover().await.unwrap();
        let member = admitted.instance().member.member.clone();
        let realm = owner.member_realm().unwrap();
        let machine = meerkat_runtime::MeerkatMachine::ephemeral();
        let session = meerkat_core::SessionId::new();
        let witness = register(&machine, &session).await;
        let selected = owner
            .select_registration(&realm, &member, &witness)
            .await
            .unwrap()
            .unwrap();
        // Seed the named prepared checkpoint with the actual registration
        // destination. This owner control does not claim a native Created event.
        {
            let _exclusion = owner.exclusion.lock().await;
            let (stored, mut slot) = owner.read().await.unwrap();
            slot.admissions[0].phase = MemberClassAdmissionPhase::Prepared {
                context: selected.context.clone(),
                context_generation: selected.context_generation,
            };
            owner
                .change(&stored, &slot, "fixture-prepared-checkpoint")
                .await
                .unwrap();
            owner.activate_current(&slot).unwrap();
        }
        let route = CompiledNativeSourceRoute {
            id: Id::new("project-console").unwrap(),
            origin: CompiledNativeSourceOrigin::LocalConsole {
                member: member.clone(),
                requester: principal(),
                ingress_actor: principal(),
            },
            application_parent: None,
            controller_parent: None,
            logical_executor: principal(),
            represented_subject: None,
            source_authority: principal(),
            authentication_domain: ResourceDomain {
                authority: principal(),
                namespace: "login".into(),
            },
            event_domain: ResourceDomain {
                authority: principal(),
                namespace: "console".into(),
            },
        };
        owner
            .commit_member_route(
                admitted.admission_id(),
                &selected.context,
                route.clone(),
                None,
            )
            .await
            .unwrap();
        // Retained-format control only: no grants are issued from these cloned
        // values, and the actual legacy canonical row remains unchanged.
        let retained = owner.read().await.unwrap().0;
        let mut checked = owner.read().await.unwrap().1.admissions[0].clone();
        let bundle = crate::installed_host::dynamic_members::callbacks::bundle(
            &owner.key.realm,
            &owner.key.owner,
            &checked,
        )
        .unwrap();
        checked.standing_operations = Some(
            crate::installed_host::dynamic_members::MemberStandingOperations {
                peer_message_parent: EvidenceId::new(format!("{bundle}/peer-message")).unwrap(),
                recorder_write_parent: EvidenceId::new(format!("{bundle}/recorder-write")).unwrap(),
                recorder_scope: meerkat_mobkit::memory::MemoryScope::Identity {
                    realm: realm.to_string(),
                    identity: meerkat_mobkit::member_comms_id::durable_identity_for_roster_member(
                        &member.member,
                    )
                    .to_string(),
                }
                .operation_scope_ref(),
                issued: None,
            },
        );
        owner.validate_admission(&checked).unwrap();
        for field in ["peer", "recorder", "scope"] {
            let mut changed = checked.clone();
            let value = changed.standing_operations.as_mut().unwrap();
            match field {
                "peer" => {
                    value.peer_message_parent =
                        EvidenceId::new("unrelated-operator-parent").unwrap()
                }
                "recorder" => {
                    value.recorder_write_parent =
                        EvidenceId::new("unrelated-operator-parent").unwrap()
                }
                _ => {
                    value.recorder_scope = meerkat_mobkit::memory::MemoryScope::Identity {
                        realm: realm.to_string(),
                        identity: "another-member".into(),
                    }
                    .operation_scope_ref()
                }
            }
            assert_eq!(
                owner.validate_admission(&changed),
                Err(HostLifetimeError::InvalidSelection)
            );
        }
        assert_eq!(owner.read().await.unwrap().0, retained);
        assert!(
            owner.member_route(&route.id).unwrap().is_none(),
            "Prepared is not projected"
        );
        owner
            .activate_member(admitted.admission_id(), &witness)
            .await
            .unwrap();
        assert!(owner.member_route(&route.id).unwrap().is_some());
        assert_eq!(
            admitted.claim_add(),
            Err(HostLifetimeError::Denied),
            "an Active handle cannot transfer ownership of another Add"
        );
        match loss {
            "ended" => machine.unregister_session(&session).await.unwrap(),
            "cold" => owner.member_registrations.lock().unwrap().clear(),
            "replacement" => {
                machine.unregister_session(&session).await.unwrap();
                let next = register(&machine, &session).await;
                let destination = owner
                    .select_registration(&realm, &member, &next)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    destination, selected,
                    "new native witness keeps the logical target"
                );
                assert!(
                    owner.member_route(&route.id).is_err(),
                    "same target is not verification of a replacement registration"
                );
                assert!(owner.member_routes().unwrap().is_empty());
                assert!(
                    owner
                        .verify_registration(&realm, &member, &witness)
                        .await
                        .is_err()
                );
                // Exercise the canonical hook boundary with the real new
                // witness. This owner control does not claim actor recovery;
                // production invokes it only after actual surface validation.
                owner
                    .verify_registration(&realm, &member, &next)
                    .await
                    .unwrap();
                assert!(owner.member_route(&route.id).unwrap().is_some());
                assert!(
                    owner
                        .expected_registration(&realm, &member, &witness)
                        .is_err()
                );
                machine.unregister_session(&session).await.unwrap();
            }
            "reselected" => {
                let next = owner
                    .select_registration(&realm, &member, &witness)
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(next.context, selected.context);
                assert_eq!(next.context_generation, selected.context_generation + 1);
            }
            _ => unreachable!(),
        }
        let before = owner.read().await.unwrap().0;
        assert!(owner.member_route(&route.id).is_err(), "{loss}");
        assert!(owner.member_routes().unwrap().is_empty(), "{loss}");
        assert_eq!(
            owner.read().await.unwrap().0,
            before,
            "projection never repairs lifecycle"
        );
        if !witness.has_ended() {
            machine.unregister_session(&session).await.unwrap();
        }
    }
}

#[tokio::test]
async fn atomic_member_observation_separates_retained_record_from_authorization() {
    let (_directory, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let member = admitted.instance().member.member.clone();
    let before = owner.read().await.unwrap();
    let selected = policy.observe_member_class(&member).await.unwrap();
    assert_eq!(selected.checkpoint.revision, before.0.revision);
    assert_eq!(selected.checkpoint.admission, selected.admission.id);
    assert_eq!(selected.admission, before.1.admissions[0]);
    assert_eq!(selected.authorization, Ok(()));
    rule.member_current.store(false, Ordering::SeqCst);
    let expired = policy.observe_member_class(&member).await.unwrap();
    assert_eq!(expired.admission, selected.admission);
    assert_eq!(expired.checkpoint, selected.checkpoint);
    assert_eq!(expired.authorization, Err(HostLifetimeError::Denied));
    assert!(
        owner.observed_admission(&member).is_err(),
        "ordinary enforcement still denies"
    );
    rule.available.store(false, Ordering::SeqCst);
    assert_eq!(
        policy
            .observe_member_class(&member)
            .await
            .unwrap()
            .authorization,
        Err(HostLifetimeError::Unavailable)
    );
    assert_eq!(
        owner.read().await.unwrap().0,
        before.0,
        "observation never writes"
    );
    rule.member_current.store(true, Ordering::SeqCst);
    rule.available.store(true, Ordering::SeqCst);
    admitted.retire().await.unwrap();
    rule.available.store(false, Ordering::SeqCst);
    let retired = policy.observe_member_class(&member).await.unwrap();
    let stored = owner.read().await.unwrap();
    assert_eq!(retired.admission.phase, MemberClassAdmissionPhase::Retired);
    assert_eq!(retired.authorization, Err(HostLifetimeError::Denied));
    assert_eq!(retired.checkpoint.admission, *admitted.admission_id());
    assert_eq!(retired.checkpoint.revision, stored.0.revision);
    assert_eq!(retired.admission, stored.1.admissions[0]);
    let mut other = member;
    other.member = "another-member".into();
    assert!(policy.observe_member_class(&other).await.is_err());
    assert_eq!(owner.read().await.unwrap().0, stored.0);
}

#[tokio::test]
async fn add_ownership_is_one_shot_and_never_recreated_by_replay() {
    let (_directory, policy, owner, rule) = fixture().await;
    let admitted = Arc::new(
        policy
            .admit_member_class(request(&rule.class))
            .await
            .unwrap(),
    );
    let replay = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let before = owner.read().await.unwrap().0;
    assert_eq!(replay.claim_add(), Err(HostLifetimeError::Denied));
    {
        let _busy = owner.exclusion.lock().await;
        assert_eq!(admitted.claim_add(), Err(HostLifetimeError::Unavailable));
    }
    rule.member_current.store(false, Ordering::SeqCst);
    assert_eq!(admitted.claim_add(), Err(HostLifetimeError::Denied));
    rule.member_current.store(true, Ordering::SeqCst);
    assert_eq!(
        admitted.claim_add(),
        Ok(()),
        "unaccepted attempts leave the token intact"
    );
    assert_eq!(admitted.clone().claim_add(), Err(HostLifetimeError::Denied));
    assert_eq!(
        owner.read().await.unwrap().0,
        before,
        "claim never changes canonical policy"
    );
}

#[tokio::test]
async fn reconstruction_cannot_claim_add_after_preparation_or_retirement() {
    let (_directory, policy, owner, rule) = fixture().await;
    let admitted = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    pending(&owner, false).await;
    owner.recover().await.unwrap();
    let before = owner.read().await.unwrap().0;
    assert_eq!(admitted.claim_add(), Err(HostLifetimeError::Denied));
    let reconstructed = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    assert_eq!(reconstructed.claim_add(), Err(HostLifetimeError::Denied));
    assert_eq!(owner.read().await.unwrap().0, before);
    admitted.retire().await.unwrap();
    assert_eq!(admitted.claim_add(), Err(HostLifetimeError::Denied));
    let observation = policy
        .observe_member_class(&admitted.instance().member.member)
        .await
        .unwrap();
    let next = policy
        .readmit_member_class_declaration(
            observation.checkpoint,
            Id::new("next-reservation").unwrap(),
            admitted.instance().clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        next.claim_add(),
        Ok(()),
        "an explicit fresh successor owns its own Add"
    );
    assert_eq!(admitted.claim_add(), Err(HostLifetimeError::Denied));
}

#[tokio::test]
async fn routed_cold_admission_without_native_owner_cannot_claim_callback_cleanup() {
    use meerkat_authorization_contracts::constraints::ResourceDomain;
    use meerkat_toolkit::host_lifetime::policy::{
        CompiledNativeSourceOrigin, CompiledNativeSourceRoute,
    };
    let (_directory, policy, owner, rule) = fixture().await;
    let request = request(&rule.class);
    let admitted = policy.admit_member_class(request).await.unwrap();
    let mut selected = owner
        .observed_admission(&admitted.instance().member.member)
        .unwrap()
        .unwrap();
    assert_eq!(
        owner.revoke_class_callbacks(&selected),
        Ok(()),
        "no retained route means issuance could not enter"
    );
    // Model the retained retired checkpoint, not an invalid Reserved route.
    selected.phase = MemberClassAdmissionPhase::Retired;
    selected.route = Some(CompiledNativeSourceRoute {
        id: Id::new("retained-project-route").unwrap(),
        origin: CompiledNativeSourceOrigin::LocalConsole {
            member: selected.instance.member.member.clone(),
            requester: principal(),
            ingress_actor: principal(),
        },
        application_parent: None,
        controller_parent: None,
        logical_executor: principal(),
        represented_subject: None,
        source_authority: principal(),
        authentication_domain: ResourceDomain {
            authority: principal(),
            namespace: "login".into(),
        },
        event_domain: ResourceDomain {
            authority: principal(),
            namespace: "console".into(),
        },
    });
    assert_eq!(
        owner.revoke_class_callbacks(&selected),
        Err(HostLifetimeError::Unavailable),
        "missing process source does not prove no native grant exists"
    );
}

// These are inert source/workflow correlations. Only the source preparation
// path may supply them in production; this test does not admit native work.
fn workflow_binding(name: &str) -> MemberClassWorkflowBinding {
    let reference = |label: &str| crate::private_originals::PrivateOriginalRef {
        route: Id::new(label).unwrap(),
        original: EvidenceId::new(label).unwrap(),
        event_sha256: "a".repeat(64),
    };
    MemberClassWorkflowBinding {
        workflow: serde_json::from_value(serde_json::json!({
            "key":{"realm":"realm","owner":"application","id":name},
            "terms_digest": toolkit_package::ContentDigest::of_bytes(name.as_bytes())
        }))
        .unwrap(),
        parent: reference("parent"),
        original: reference("lead"),
    }
}

#[tokio::test]
async fn shared_workflow_reservation_is_exact_inert_and_cannot_use_ordinary_admission() {
    let mut selected = class();
    selected.purpose =
        meerkat_toolkit::host_lifetime::policy::CompiledMemberClassPurpose::SharedWorkflow;
    let (_directory, policy, owner, rule) = fixture_with_class(selected.clone()).await;
    assert!(matches!(
        policy
            .reserve_member_class(Id::new("ordinary-door").unwrap(), instance(&selected))
            .await,
        Err(HostLifetimeError::Denied)
    ));
    let binding = workflow_binding("workflow-one");
    let id = Id::new("workflow-admission").unwrap();
    let admitted = owner
        .admit_workflow_member_declaration(id.clone(), instance(&selected), binding.clone())
        .await
        .unwrap();
    let current = policy.current_member_class(&admitted).unwrap();
    assert_eq!(current.workflow.as_ref(), Some(&binding));
    assert_eq!(current.phase, MemberClassAdmissionPhase::Reserved);
    assert!(
        current.route.is_none()
            && current.destination.is_none()
            && current.native_surface.is_none()
    );
    assert!(
        owner.member_routes().unwrap().is_empty(),
        "reservation does not publish a route"
    );
    let replay = owner
        .admit_workflow_member_declaration(id.clone(), instance(&selected), binding.clone())
        .await
        .unwrap();
    assert_eq!(policy.current_member_class(&replay).unwrap(), current);
    assert!(
        matches!(replay.claim_add(), Err(HostLifetimeError::Denied)),
        "replay cannot own the first operation"
    );
    assert!(matches!(
        owner
            .admit_workflow_member_declaration(
                id.clone(),
                instance(&selected),
                workflow_binding("other")
            )
            .await,
        Err(HostLifetimeError::Denied)
    ));
    rule.member_current.store(false, Ordering::SeqCst);
    assert!(matches!(
        policy.current_member_class(&admitted),
        Err(HostLifetimeError::Denied)
    ));
    rule.member_current.store(true, Ordering::SeqCst);
    admitted.retire().await.unwrap();
    assert!(matches!(
        policy.current_member_class(&admitted),
        Err(HostLifetimeError::Denied)
    ));
    assert!(matches!(
        owner
            .admit_workflow_member_declaration(id, instance(&selected), binding)
            .await,
        Err(HostLifetimeError::Denied)
    ));
}

#[tokio::test]
async fn ordinary_class_rejects_workflow_binding_and_foreign_policy_handle() {
    let (_directory, policy, owner, _) = fixture().await;
    assert!(matches!(
        owner
            .admit_workflow_member_declaration(
                Id::new("workflow-door").unwrap(),
                instance(&class()),
                workflow_binding("workflow")
            )
            .await,
        Err(HostLifetimeError::Denied)
    ));
    let admitted = policy
        .reserve_member_class(Id::new("ordinary").unwrap(), instance(&class()))
        .await
        .unwrap();
    let (_other_dir, other, _, _) = fixture().await;
    assert!(matches!(
        other.current_member_class(&admitted),
        Err(HostLifetimeError::WrongOwner)
    ));
    assert!(
        policy
            .current_member_class(&admitted)
            .unwrap()
            .workflow
            .is_none()
    );
}

#[tokio::test]
async fn workflow_resource_cas_preserves_policy_and_rejects_stale_or_unauthorized_selection() {
    use meerkat_authorization_contracts::constraints::ResourceDomain;
    use meerkat_toolkit::host_lifetime::{
        control::SelectedHostArtifact,
        policy::{
            CompiledMemberClassPurpose, CompiledNativeSourceOrigin, CompiledNativeSourceRoute,
        },
    };
    let mut class = class();
    class.purpose = CompiledMemberClassPurpose::SharedWorkflow;
    let (directory, policy, owner, rule) = fixture_with_class(class.clone()).await;
    let binding = workflow_binding("workflow-one");
    let admitted = owner
        .admit_workflow_member_declaration(
            Id::new("workflow-one").unwrap(),
            instance(&class),
            binding.clone(),
        )
        .await
        .unwrap();
    let member = &admitted.instance().member.member;
    let artifact = |name: &str| SelectedHostArtifact {
        path: directory.path().join(name),
        sha256: sha(name.as_bytes()),
    };
    let reserved = policy.observe_member_class(member).await.unwrap();
    assert!(
        serde_json::to_value(&reserved.admission)
            .unwrap()
            .get("workflow_resource")
            .is_none(),
        "unselected admissions keep their existing wire encoding"
    );
    let before = owner.read().await.unwrap().0;
    assert!(
        matches!(
            policy
                .retain_workflow_member_resource(
                    &admitted,
                    &reserved.admission,
                    &reserved.checkpoint,
                    artifact("open.json")
                )
                .await,
            Err(HostLifetimeError::Denied)
        ),
        "a reservation is not held native preparation"
    );
    assert_eq!(owner.read().await.unwrap().0, before);

    let tools = request(&class).tools;
    owner
        .bind_member_surface(&reserved.admission, tools.identities())
        .await
        .unwrap();
    pending(&owner, false).await;
    owner.recover().await.unwrap();
    // Retained named checkpoint only. Actual held finalization and native
    // input settlement belong to the gateway controls, not this CAS test.
    {
        let (stored, mut slot) = owner.read().await.unwrap();
        slot.admissions[0].route = Some(CompiledNativeSourceRoute {
            id: binding.original.route.clone(),
            origin: CompiledNativeSourceOrigin::LocalConsole {
                member: member.clone(),
                requester: principal(),
                ingress_actor: principal(),
            },
            application_parent: None,
            controller_parent: None,
            logical_executor: principal(),
            represented_subject: None,
            source_authority: principal(),
            authentication_domain: ResourceDomain {
                authority: principal(),
                namespace: "login".into(),
            },
            event_domain: ResourceDomain {
                authority: principal(),
                namespace: "console".into(),
            },
        });
        owner
            .change(&stored, &slot, "fixture-held-route-checkpoint")
            .await
            .unwrap();
        owner.activate_current(&slot).unwrap();
    }
    let mut sibling = instance(&class);
    sibling.member.member.member = "project-two".into();
    owner
        .admit_workflow_member_declaration(
            Id::new("workflow-two").unwrap(),
            sibling,
            workflow_binding("workflow-two"),
        )
        .await
        .unwrap();
    let observed = owner.observed_current.lock().unwrap().clone().unwrap();
    let sibling_reservation = observed
        .member_reservations
        .iter()
        .find(|(id, _)| id == "workflow-two")
        .unwrap()
        .1
        .as_ref()
        .unwrap()
        .clone();
    let selected = policy.observe_member_class(member).await.unwrap();
    let (before, before_slot) = owner.read().await.unwrap();
    let before_decision = serde_json::to_value(&before_slot.member_decision).unwrap();
    let callback_bundle = owner.callback_bundle(&selected.admission).unwrap();
    let open = policy
        .retain_workflow_member_resource(
            &admitted,
            &selected.admission,
            &selected.checkpoint,
            artifact("open.json"),
        )
        .await
        .unwrap();
    let (after, after_slot) = owner.read().await.unwrap();
    assert_eq!(after.revision, before.revision + 1);
    assert_eq!(after_slot.current, before_slot.current);
    assert_eq!(
        serde_json::to_value(&after_slot.member_decision).unwrap(),
        before_decision
    );
    assert_eq!(open.phase, selected.admission.phase);
    assert_eq!(open.workflow, selected.admission.workflow);
    assert_eq!(open.destination, selected.admission.destination);
    assert_eq!(owner.callback_bundle(&open).unwrap(), callback_bundle);
    let refreshed = owner.observed_current.lock().unwrap().clone().unwrap();
    assert!(Arc::ptr_eq(&refreshed.authority, &observed.authority));
    assert!(Arc::ptr_eq(
        &sibling_reservation,
        refreshed
            .member_reservations
            .iter()
            .find(|(id, _)| id == "workflow-two")
            .unwrap()
            .1
            .as_ref()
            .unwrap()
    ));
    assert_eq!(refreshed.admissions[1], observed.admissions[1]);

    assert!(matches!(
        policy
            .retain_workflow_member_resource(
                &admitted,
                &selected.admission,
                &selected.checkpoint,
                artifact("closed.json")
            )
            .await,
        Err(HostLifetimeError::Denied)
    ));
    let current = policy.observe_member_class(member).await.unwrap();
    let mut changed = current.admission.clone();
    changed.workflow.as_mut().unwrap().original.event_sha256 = "b".repeat(64);
    assert!(matches!(
        policy
            .retain_workflow_member_resource(
                &admitted,
                &changed,
                &current.checkpoint,
                artifact("closed.json")
            )
            .await,
        Err(HostLifetimeError::Denied)
    ));
    let (_other_dir, other, _, _) = fixture_with_class(class).await;
    assert!(matches!(
        other
            .retain_workflow_member_resource(
                &admitted,
                &current.admission,
                &current.checkpoint,
                artifact("closed.json")
            )
            .await,
        Err(HostLifetimeError::WrongOwner)
    ));
    rule.member_current.store(false, Ordering::SeqCst);
    assert!(matches!(
        policy
            .retain_workflow_member_resource(
                &admitted,
                &current.admission,
                &current.checkpoint,
                artifact("closed.json")
            )
            .await,
        Err(HostLifetimeError::Denied)
    ));
    rule.member_current.store(true, Ordering::SeqCst);
    assert_eq!(
        owner.read().await.unwrap().0,
        after,
        "refusals publish no successor"
    );

    // A newer slot revision still refuses the earlier checkpoint even if
    // another member's change left this whole admission exactly unchanged.
    owner
        .change(&after, &after_slot, "fixture-other-owner-checkpoint")
        .await
        .unwrap();
    let unchanged = owner.read().await.unwrap().0;
    assert!(matches!(
        policy
            .retain_workflow_member_resource(
                &admitted,
                &current.admission,
                &current.checkpoint,
                artifact("closed.json")
            )
            .await,
        Err(HostLifetimeError::Denied)
    ));
    assert_eq!(owner.read().await.unwrap().0, unchanged);
    {
        let (stored, mut slot) = owner.read().await.unwrap();
        let destination = slot.admissions[0].destination.clone().unwrap();
        slot.admissions[0].phase = MemberClassAdmissionPhase::Active {
            context: destination.context,
            context_generation: destination.context_generation,
        };
        owner
            .change(&stored, &slot, "fixture-published-checkpoint")
            .await
            .unwrap();
        owner.activate_current(&slot).unwrap();
    }
    let current = policy.observe_member_class(member).await.unwrap();
    let closed = policy
        .retain_workflow_member_resource(
            &admitted,
            &current.admission,
            &current.checkpoint,
            artifact("closed.json"),
        )
        .await
        .unwrap();
    assert_eq!(closed.workflow_resource, Some(artifact("closed.json")));
    assert_eq!(policy.current_member_class(&admitted).unwrap(), closed);
    let (closed_record, closed_slot) = owner.read().await.unwrap();
    assert_eq!(closed_slot.current, before_slot.current);
    assert_eq!(
        serde_json::to_value(&closed_slot.member_decision).unwrap(),
        before_decision
    );
    let selected_closed = policy.observe_member_class(member).await.unwrap();
    assert!(
        matches!(
            policy
                .retain_workflow_member_resource(
                    &admitted,
                    &closed,
                    &selected_closed.checkpoint,
                    artifact("closed.json")
                )
                .await,
            Err(HostLifetimeError::Denied)
        ),
        "a duplicate publication is refused rather than replayed"
    );
    assert_eq!(owner.read().await.unwrap().0, closed_record);
}

#[tokio::test]
async fn legacy_reservation_reconstruction_keeps_no_standing_mandate_or_add_token() {
    let (_directory, policy, owner, rule) = fixture().await;
    let first = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    let (before, slot) = owner.read().await.unwrap();
    assert!(slot.admissions[0].standing_operations.is_none());
    let bytes = serde_json::to_value(&slot.admissions[0]).unwrap();
    assert!(
        bytes.get("standing_operations").is_none(),
        "legacy canonical shape remains omitted"
    );
    let decoded: MemberClassAdmission = serde_json::from_value(bytes).unwrap();
    assert_eq!(decoded, slot.admissions[0]);
    owner.recover().await.unwrap();
    let replay = policy
        .admit_member_class(request(&rule.class))
        .await
        .unwrap();
    assert!(replay.add_token.is_none());
    assert_eq!(replay.claim_add(), Err(HostLifetimeError::Denied));
    assert_eq!(
        owner.read().await.unwrap().0,
        before,
        "recovery cannot add a standing selection"
    );
    assert_eq!(
        first.claim_add(),
        Ok(()),
        "only the original newly persisted reservation owns Add"
    );
}

mod cold_source_coordinates;
