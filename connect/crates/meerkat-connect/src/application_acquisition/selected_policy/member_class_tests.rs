use super::*;
use meerkat_core::{
    CompiledMemberToolAction, CompiledMemberToolGrant, CompiledMemberToolGrants,
    CompiledPolicySourceProvenance, CompiledToolConsequence, MobMemberBinding, PolicyId,
    PolicyProviderId, PolicyRevision, PrincipalKind, TrustDomainId,
};
use meerkat_toolkit::host_lifetime::policy::{CompiledMemberClassInstance, PreparedNativeMember};
use toolkit_mobkit::installed_host::dynamic_members::{
    ApplicationMemberPolicyRequest, MemberClassAdmission, MemberClassAdmissionPhase,
};

fn fixture() -> (
    Arc<dyn ApplicationServicePolicy>,
    PrincipalRef,
    RealmId,
    MemberClassAdmission,
) {
    let owner = PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        "application",
        TrustDomainId::new("classes").unwrap(),
    )
    .unwrap();
    let realm = meerkat_core::mob_realm_id("application").unwrap();
    let class = CompiledMemberClass {
        purpose: Default::default(),
        id: Id::new("project").unwrap(),
        profile: "project".into(),
        factory: Id::new("application-callbacks").unwrap(),
        callback_account: None,
        callbacks: vec![],
        grants: vec![CompiledMemberToolGrant {
            tool_name: "project_read".into(),
            action: CompiledMemberToolAction::Invoke,
            consequence: CompiledToolConsequence::R0,
        }],
    };
    // The default human-review service policy still honors explicit packaged
    // classes. A standing service policy is not prerequisite class authority.
    let participant = SelectedPolicyBinding::human_review_participant_with_member_classes(
        owner.clone(),
        Id::new("deployment").unwrap(),
        realm.clone(),
        BTreeSet::from(["coordinator".into()]),
        vec![class.clone()],
    );
    let admission = MemberClassAdmission {
        workflow: None,
        workflow_resource: None,
        id: Id::new("new-project").unwrap(),
        instance: CompiledMemberClassInstance {
            class: class.id.clone(),
            member: PreparedNativeMember {
                member: MobMemberBinding {
                    mob_id: "application".into(),
                    member: "project-one".into(),
                    role: "project".into(),
                },
                logical_owner: owner.clone(),
                context_generation: 1,
                audience: meerkat_authorization_contracts::constraints::AudienceRef::Principal {
                    principal: owner.clone(),
                },
                setup_capability: None,
            },
        },
        class,
        destination: None,
        installed_surface: None,
        native_surface: None,
        route: None,
        standing_operations: None,
        phase: MemberClassAdmissionPhase::Reserved,
    };
    (participant, owner, realm, admission)
}

fn canonical(addition: Option<CompiledMemberToolGrants>) -> Vec<u8> {
    let mut members = vec![CompiledMemberToolGrants {
        member_identity: "coordinator".into(),
        grants: vec![],
    }];
    members.extend(addition);
    CompiledApplicationToolPolicy::new(
        PolicyProviderId::new("application").unwrap(),
        PolicyId::new("selected").unwrap(),
        PolicyRevision(1),
        CompiledPolicySourceProvenance {
            source_id: "selected".into(),
            source_digest: PolicyDigest::from_canonical_bytes(b"selected"),
        },
        members,
    )
    .unwrap()
    .canonical_json()
    .unwrap()
}

#[test]
fn zero_instance_reservation_and_exact_class_successor_are_separate() {
    let (policy, owner, realm, mut admission) = fixture();
    let current = canonical(None);
    let evaluate = |admission: &MemberClassAdmission, bytes: &[u8]| {
        policy.reserve_member_policy(&ApplicationMemberPolicyRequest {
            owner: &owner,
            realm: &realm,
            deployment: "deployment",
            admission,
            canonical_policy: bytes,
        })
    };
    assert!(matches!(
        evaluate(&admission, &current),
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(_))
    ));
    let expected = admission
        .class
        .materialize(&realm, &admission.instance)
        .unwrap();
    let candidate = canonical(Some(expected.clone()));
    assert!(
        evaluate(&admission, &candidate).is_err(),
        "reservation must not pretend native grants already landed"
    );
    admission.phase = MemberClassAdmissionPhase::Prepared {
        context: meerkat_authorization_contracts::evidence::EvidenceId::new("actual-context")
            .unwrap(),
        context_generation: admission.instance.member.context_generation,
    };
    assert!(matches!(
        evaluate(&admission, &candidate),
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(_))
    ));
    let mut widened = expected;
    widened.grants[0].tool_name = "delete_project".into();
    assert!(evaluate(&admission, &canonical(Some(widened))).is_err());
}

#[test]
fn policy_refuses_unknown_class_owner_realm_and_static_member_substitution() {
    let (policy, owner, realm, admission) = fixture();
    let current = canonical(None);
    let other = PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        "other",
        TrustDomainId::new("classes").unwrap(),
    )
    .unwrap();
    for (actor, selected_realm, change) in [
        (&other, realm.clone(), 0),
        (&owner, meerkat_core::mob_realm_id("other").unwrap(), 0),
        (&owner, realm.clone(), 1),
        (&owner, realm.clone(), 2),
    ] {
        let mut selected = admission.clone();
        if change == 1 {
            selected.class.factory = Id::new("unselected").unwrap();
        }
        if change == 2 {
            selected.instance.member.member.member = "coordinator".into();
        }
        assert!(
            policy
                .reserve_member_policy(&ApplicationMemberPolicyRequest {
                    owner: actor,
                    realm: &selected_realm,
                    deployment: "deployment",
                    admission: &selected,
                    canonical_policy: &current
                })
                .is_err()
        );
    }
}

#[test]
fn service_successor_preserves_only_exact_selected_class_rows() {
    use toolkit_mobkit::private_originals::{
        ActiveMemberToolPolicyDecision, PrivateOriginalRef, ServiceToolInvocation,
        ServiceToolPolicySelection, ServiceToolPolicyVersion,
    };
    let (_, owner, realm, mut ordinary) = fixture();
    ordinary.phase = MemberClassAdmissionPhase::Prepared {
        context: meerkat_authorization_contracts::evidence::EvidenceId::new("ordinary-context")
            .unwrap(),
        context_generation: 1,
    };
    let mut planner = ordinary.clone();
    planner.id = Id::new("planning-admission").unwrap();
    planner.class.id = Id::new("planning").unwrap();
    planner.class.purpose =
        meerkat_toolkit::host_lifetime::policy::CompiledMemberClassPurpose::SharedWorkflow;
    planner.class.profile = "planning".into();
    planner.class.grants[0].tool_name = "planning_read".into();
    planner.instance.class = planner.class.id.clone();
    planner.instance.member.member.member = "planning-lead".into();
    planner.instance.member.member.role = planner.class.profile.clone();
    planner.phase = MemberClassAdmissionPhase::Active {
        context: meerkat_authorization_contracts::evidence::EvidenceId::new("planning-context")
            .unwrap(),
        context_generation: planner.instance.member.context_generation,
    };
    let admissions = vec![ordinary, planner];
    let binding = SelectedPolicyBinding::new_with_member_classes(
        StandingPolicySelection {
            installation: InstallationSelection::HumanReview,
            access: AccessSelection::ConfiguredParent,
            tool_policy: Some(MemberToolPolicySelection::UntilSuperseded),
        },
        owner.clone(),
        Id::new("deployment").unwrap(),
        realm.clone(),
        BTreeSet::from(["coordinator".into()]),
        admissions
            .iter()
            .map(|admission| admission.class.clone())
            .collect(),
    );
    let rows = admissions
        .iter()
        .map(|admission| {
            admission
                .class
                .materialize(&realm, &admission.instance)
                .unwrap()
        })
        .collect::<Vec<_>>();
    let base = CompiledApplicationToolPolicy::parse_canonical_json(&canonical(None)).unwrap();
    let proposal_for = |mut rows: Vec<CompiledMemberToolGrants>| {
        rows.sort_by(|left, right| left.member_identity.cmp(&right.member_identity));
        let compiled = CompiledApplicationToolPolicy::new(
            base.provider_id.clone(),
            base.policy_id.clone(),
            PolicyRevision(2),
            base.source.clone(),
            rows,
        )
        .unwrap();
        let bytes = compiled.canonical_json().unwrap();
        ServiceToolPolicyProposal {
            provider_id: compiled.provider_id,
            policy_id: compiled.policy_id,
            member: "coordinator".into(),
            selection: ServiceToolPolicySelection {
                tools: vec![ServiceToolInvocation {
                    name: "service_read".into(),
                    consequence: CompiledToolConsequence::R0,
                }],
                catalog: None,
            },
            catalog_session: None,
            prior: ServiceToolPolicyVersion {
                revision: base.revision,
                digest: PolicyDigest::from_canonical_bytes(&canonical(None)),
            },
            candidate: ServiceToolPolicyVersion {
                revision: compiled.revision,
                digest: PolicyDigest::from_canonical_bytes(&bytes),
            },
            candidate_canonical_json: String::from_utf8(bytes).unwrap(),
            lifetime: MemberPolicyLifetime::UntilSuperseded,
        }
    };
    let mut static_rows = base.members.clone();
    static_rows[0].grants.push(CompiledMemberToolGrant {
        tool_name: "service_read".into(),
        action: CompiledMemberToolAction::Invoke,
        consequence: CompiledToolConsequence::R0,
    });
    let all_rows = static_rows
        .into_iter()
        .chain(rows.clone())
        .collect::<Vec<_>>();
    let proposal = proposal_for(all_rows.clone());
    // Exercise the production binding's fresh preservation validator and its
    // real recovery participant. These class records are component inputs,
    // not native admission or a bound service-task authorization claim.
    let fresh = |proposal: &ServiceToolPolicyProposal, admissions: &[MemberClassAdmission]| {
        binding.facts.scope.proposal(
            proposal,
            proposal.candidate_canonical_json.as_bytes(),
            admissions,
            false,
        )
    };
    fresh(&proposal, &admissions).unwrap();
    assert!(
        fresh(&proposal, &admissions[..1]).is_err(),
        "every extra needs its own retained record"
    );
    let mut duplicate = admissions.clone();
    duplicate.push(admissions[1].clone());
    assert!(fresh(&proposal, &duplicate).is_err());
    for mutation in 0..7 {
        let mut changed = admissions.clone();
        match mutation {
            0 => changed[1].class.factory = Id::new("unselected").unwrap(),
            1 => changed[1].instance.class = Id::new("unselected").unwrap(),
            2 => changed[1].instance.member.member.member = "another-lead".into(),
            3 => changed[1].instance.member.member.mob_id = "another-mob".into(),
            4 => changed[1].id = changed[0].id.clone(),
            5 => changed[1].phase = MemberClassAdmissionPhase::Reserved,
            _ => changed[1].phase = MemberClassAdmissionPhase::Retired,
        }
        assert!(fresh(&proposal, &changed).is_err(), "mutation {mutation}");
    }
    let mut changed_rows = all_rows.clone();
    changed_rows[2].grants[0].tool_name = "unselected_write".into();
    assert!(fresh(&proposal_for(changed_rows), &admissions).is_err());
    let mut missing = all_rows.clone();
    missing.pop();
    assert!(
        fresh(&proposal_for(missing), &admissions).is_err(),
        "fresh successor must preserve the prepared roster"
    );
    assert!(
        fresh(&proposal_for(rows), &admissions).is_err(),
        "static member cannot disappear"
    );
    let mut changed = proposal.clone();
    changed.member = "planning-lead".into();
    assert!(
        fresh(&changed, &admissions).is_err(),
        "preservation does not select a dynamic service target"
    );
    changed = proposal.clone();
    changed.candidate.digest = PolicyDigest::from_canonical_bytes(b"other");
    assert!(fresh(&changed, &admissions).is_err());
    changed = proposal.clone();
    changed.candidate.revision = PolicyRevision(3);
    assert!(fresh(&changed, &admissions).is_err());
    changed = proposal.clone();
    changed.provider_id = PolicyProviderId::new("other").unwrap();
    assert!(fresh(&changed, &admissions).is_err());

    let decision = ActiveMemberToolPolicyDecision {
        approval_id: proposal.sha256().unwrap(),
        owner,
        realm: realm.to_string(),
        deployment: "deployment".into(),
        original: PrivateOriginalRef {
            route: Id::new("route").unwrap(),
            original: meerkat_authorization_contracts::evidence::EvidenceId::new("original")
                .unwrap(),
            event_sha256: "a".repeat(64),
        },
        acquisition: serde_json::from_value(
            json!({"realm":realm.as_str(), "owner":"acquisition", "id":"one"}),
        )
        .unwrap(),
        proposal,
        human_review: None,
    };
    let participant = binding.participant();
    let recover = |admissions: &[MemberClassAdmission]| {
        participant.reserve_tool_policy_recovery(&ApplicationToolPolicyRecoveryRequest {
            decision: &decision,
            canonical_json: decision.proposal.candidate_canonical_json.as_bytes(),
            member_admissions: admissions,
        })
    };
    assert!(matches!(
        recover(&admissions),
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(_))
    ));
    let mut historical = admissions.clone();
    historical[1].phase = MemberClassAdmissionPhase::Retired;
    assert!(matches!(
        recover(&historical),
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(_))
    ));
    historical[1].id = Id::new("explicit-new-admission").unwrap();
    historical[1].phase = MemberClassAdmissionPhase::Reserved;
    assert!(matches!(
        recover(&historical),
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(_))
    ));
    let mut wrong = historical.clone();
    wrong[1].instance.member.member.member = "another-lead".into();
    assert!(recover(&wrong).is_err());
    wrong = historical.clone();
    wrong[1].class.factory = Id::new("unselected").unwrap();
    assert!(recover(&wrong).is_err());
    wrong = historical.clone();
    wrong[1].class.grants.clear();
    assert!(recover(&wrong).is_err());
    wrong = historical.clone();
    wrong.push(wrong[1].clone());
    assert!(recover(&wrong).is_err());
    assert!(
        fresh(&decision.proposal, &historical).is_err(),
        "historical evidence never admits a fresh Reserved row"
    );
    assert!(
        binding.service().is_err(),
        "this component control never binds live service authority"
    );
}
