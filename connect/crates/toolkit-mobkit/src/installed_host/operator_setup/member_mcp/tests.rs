//! Actual SQLite decisions and native grant authority, without account network
//! I/O or a claimed native member construction/ServiceCall. No witness is forged.
use super::*;
use crate::installed_host::{
    dynamic_members::ApplicationMemberPolicyRequest,
    tool_policy::{PreparedInstalledToolPolicy, service_policy::ServicePolicyOwner},
};
use crate::private_originals::{
    ApplicationServicePolicy, ApplicationServicePolicyDecision, ApplicationServicePolicyRequest,
    ApplicationServicePolicyReservation, ApplicationToolPolicyRecoveryDecision,
};
use meerkat_core::{
    ApprovalPrincipalId, ApprovalService, CompiledApplicationToolPolicy, CompiledMemberToolGrants,
    CompiledPolicySourceProvenance, MobMemberBinding, PolicyDigest, PolicyId, PolicyProviderId,
    PolicyRevision,
};
use meerkat_toolkit::host_lifetime::policy::PreparedNativeMember;
use std::sync::atomic::{AtomicBool, Ordering};
use toolkit_store::StoreScope;

struct Rule {
    owner: PrincipalRef,
    current: Arc<AtomicBool>,
    provenance: serde_json::Map<String, serde_json::Value>,
}
impl ApplicationServicePolicyReservation for Rule {
    fn owner(&self) -> &PrincipalRef {
        &self.owner
    }
    fn provenance(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.provenance
    }
    fn check_current(&self) -> Result<(), HostLifetimeError> {
        if self.current.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(HostLifetimeError::Denied)
        }
    }
}
impl ApplicationServicePolicy for Rule {
    fn reserve(
        &self,
        _: &ApplicationServicePolicyRequest<'_>,
    ) -> Result<ApplicationServicePolicyDecision, HostLifetimeError> {
        Ok(ApplicationServicePolicyDecision::RequiresHuman)
    }
    fn reserve_member_policy(
        &self,
        request: &ApplicationMemberPolicyRequest<'_>,
    ) -> Result<ApplicationToolPolicyRecoveryDecision, HostLifetimeError> {
        if request.owner != &self.owner {
            return Err(HostLifetimeError::Denied);
        }
        self.check_current()?;
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(Box::new(
            Self {
                owner: self.owner.clone(),
                current: self.current.clone(),
                provenance: Default::default(),
            },
        )))
    }
}
fn class() -> CompiledMemberClass {
    CompiledMemberClass {
        purpose: Default::default(),
        id: Id::new("project").unwrap(),
        profile: "project".into(),
        factory: Id::new("project-factory").unwrap(),
        callbacks: vec![],
        callback_account: None,
        grants: vec![],
    }
}
fn instance(owner: &OperatorSetup, name: &str) -> CompiledMemberClassInstance {
    let principal = crate::installed_host::tests::principal(name);
    CompiledMemberClassInstance {
        class: class().id,
        member: PreparedNativeMember {
            member: MobMemberBinding {
                mob_id: owner.native.members[0].member.mob_id.clone(),
                member: name.into(),
                role: "project".into(),
            },
            logical_owner: principal.clone(),
            context_generation: 1,
            audience: AudienceRef::Principal { principal },
            setup_capability: None,
        },
    }
}
async fn bind(
    owner: &OperatorSetup,
) -> (
    Arc<PrivateOriginals>,
    Arc<ServicePolicyOwner>,
    Arc<AtomicBool>,
) {
    let scope = StoreScope {
        realm: owner.native.realm.to_string(),
        owner: owner.lifetime.selected_policy().deployment.to_string(),
    };
    let originals = Arc::new(
        PrivateOriginals::prepare(scope.clone(), owner.originals.clone(), owner.source.clone())
            .await
            .unwrap(),
    );
    owner
        .source
        .select_installed_pair_recovery(owner.originals.as_ref(), &scope)
        .unwrap();
    let bytes = CompiledApplicationToolPolicy::new(
        PolicyProviderId::new("member-mcp-fixture").unwrap(),
        PolicyId::new("selected").unwrap(),
        PolicyRevision(1),
        CompiledPolicySourceProvenance {
            source_id: "fixture".into(),
            source_digest: PolicyDigest::from_canonical_bytes(b"fixture"),
        },
        vec![CompiledMemberToolGrants {
            member_identity: "static".into(),
            grants: vec![],
        }],
    )
    .unwrap()
    .canonical_json()
    .unwrap();
    let policy = PreparedInstalledToolPolicy::from_canonical(&bytes).unwrap();
    let current = Arc::new(AtomicBool::new(true));
    let application = crate::installed_host::tests::principal("application");
    let canonical = policy
        .bind_service_owner(
            owner.originals.clone(),
            scope,
            application.clone(),
            Some(Arc::new(Rule {
                owner: application,
                current: current.clone(),
                provenance: Default::default(),
            })),
            vec![class()],
        )
        .await
        .unwrap();
    originals
        .bind_service_tool_policy(canonical.clone())
        .unwrap();
    (originals, canonical, current)
}
async fn fresh() -> (
    tempfile::TempDir,
    HostBootstrapSelection,
    OperatorSetup,
    Arc<PrivateOriginals>,
    Arc<ServicePolicyOwner>,
    Arc<AtomicBool>,
) {
    let (directory, selection) = crate::installed_host::tests::fixture();
    let owner = OperatorSetup::open(
        selection.clone(),
        OperatorOwnerStartup::fresh("member-access-owner", 1).unwrap(),
    )
    .unwrap();
    let (originals, canonical, current) = bind(&owner).await;
    (directory, selection, owner, originals, canonical, current)
}
fn config(server: &str) -> McpServerConfig {
    let mut config = McpServerConfig::stdio(server, "not-executed", vec![], Default::default());
    config.access = Some(meerkat_core::mcp_config::McpServerAccess {
        allowed_operations: ["read".into(), "write".into()].into(),
    });
    config
}
async fn review(
    owner: &OperatorSetup,
    originals: &PrivateOriginals,
    canonical: &Arc<ServicePolicyOwner>,
    member: &str,
    server: &str,
) -> MemberMcpReview {
    let admitted = canonical
        .admit_member_declaration(
            Id::new(format!("admission-{member}")).unwrap(),
            instance(owner, member),
        )
        .await
        .unwrap();
    let target = originals.member_mcp_target(&admitted).await.unwrap();
    owner
        .retain_member_mcp_review(
            originals,
            target,
            MemberMcpOrigin::OperatorSelf,
            Installation::capture(
                &meerkat_core::RealmId::parse("work").unwrap(),
                &config(server),
                None,
                ["read".into()].into(),
            )
            .unwrap(),
            LifetimeRestriction::unrestricted(),
            super::super::tests::request(),
        )
        .await
        .unwrap()
}
fn approve(review: &MemberMcpReview) -> ApprovalRecord {
    let approvals = ApprovalService::new();
    let pending = approvals.request(review.request.clone()).unwrap();
    approvals
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("protected-fixture-presenter").unwrap(),
            None,
            None,
        )
        .unwrap()
}
fn native_id(owner: &OperatorSetup, review: &MemberMcpReview) -> EvidenceId {
    EvidenceId::new(key(&owner.member_mcp_scope(), review.binding()).unwrap().id).unwrap()
}
async fn issue(owner: &OperatorSetup, originals: &PrivateOriginals, review: &MemberMcpReview) {
    owner
        .approve_and_issue_member_mcp(originals, review, approve(review), &|| true)
        .await
        .unwrap();
}

#[derive(Default)]
struct RequirementApplication {
    contract: MemberMcpRequirementContract,
    row: std::sync::Mutex<Option<AcceptedMemberMcpRequirement>>,
    retained: std::sync::Mutex<
        Option<crate::installed_host::member_mcp_consumer::RetainedMemberMcpBindings>,
    >,
}
#[async_trait::async_trait]
impl crate::installed_host::member_mcp_consumer::MemberMcpApplication for RequirementApplication {
    fn requirement_contract(&self) -> MemberMcpRequirementContract {
        self.contract
    }
    async fn retain(
        &self,
        value: crate::installed_host::member_mcp_consumer::RetainedMemberMcpBindings,
    ) -> Result<(), HostLifetimeError> {
        let row = self.row.lock().unwrap();
        if value.accepted_requirement.as_ref() != row.as_ref() {
            return Err(HostLifetimeError::Denied);
        }
        value.check(&value.selection)?;
        let mut retained = self.retained.lock().unwrap();
        if retained.as_ref().is_some_and(|old| old != &value) {
            return Err(HostLifetimeError::Denied);
        }
        *retained = Some(value);
        Ok(())
    }
    async fn lookup(
        &self,
        selection: MemberMcpAdmissionSelection,
    ) -> Result<
        crate::installed_host::member_mcp_consumer::RetainedMemberMcpBindings,
        HostLifetimeError,
    > {
        let value = self
            .retained
            .lock()
            .unwrap()
            .clone()
            .ok_or(HostLifetimeError::Unavailable)?;
        value.check(&selection)?;
        Ok(value)
    }
    async fn accepted_requirement(
        &self,
        _: MemberMcpAdmissionSelection,
        _: PrivateSourceEvidence,
    ) -> Result<AcceptedMemberMcpRequirement, HostLifetimeError> {
        self.row
            .lock()
            .unwrap()
            .clone()
            .ok_or(HostLifetimeError::Unavailable)
    }
    fn check_accepted_requirement(
        &self,
        value: &AcceptedMemberMcpRequirement,
    ) -> Result<(), HostLifetimeError> {
        assert_eq!(
            self.contract,
            MemberMcpRequirementContract::CurrentOwner,
            "remote observation must not claim a synchronous owner predicate"
        );
        if self.row.lock().unwrap().as_ref() == Some(value) {
            Ok(())
        } else {
            Err(HostLifetimeError::Denied)
        }
    }
}

async fn requirement_target(
    owner: &OperatorSetup,
    originals: &PrivateOriginals,
    canonical: &Arc<ServicePolicyOwner>,
) -> (MemberMcpTarget, AcceptedMemberMcpRequirement) {
    let admitted = canonical
        .admit_member_declaration(
            Id::new("accepted-admission").unwrap(),
            instance(owner, "accepted"),
        )
        .await
        .unwrap();
    let target = originals.member_mcp_target(&admitted).await.unwrap();
    let value = AcceptedMemberMcpRequirement {
        contract: MemberMcpRequirementContract::CurrentOwner,
        selection: MemberMcpAdmissionSelection::from_target(&target),
        source: PrivateSourceEvidence {
            resource_id: EvidenceId::new("accepted-requirement").unwrap(),
            revision: EvidenceId::new("revision-one").unwrap(),
            sha256: "a".repeat(64),
        },
        services: ["hn", "other"]
            .into_iter()
            .map(|server| MemberMcpIntent {
                server: server.into(),
                operations: ["read".into()].into(),
                lifetime: LifetimeRestriction::unrestricted(),
            })
            .collect(),
    };
    (target, value)
}

async fn retain_requirement(
    owner: &OperatorSetup,
    originals: &PrivateOriginals,
    target: MemberMcpTarget,
    source: PrivateSourceEvidence,
) -> Result<MemberMcpReview, MemberMcpError> {
    owner
        .retain_member_mcp_review(
            originals,
            target,
            MemberMcpOrigin::AcceptedApplicationRequirement { source },
            Installation::capture(
                &meerkat_core::RealmId::parse("work").unwrap(),
                &config("hn"),
                None,
                ["read".into()].into(),
            )
            .unwrap(),
            LifetimeRestriction::unrestricted(),
            super::super::tests::request(),
        )
        .await
}

#[tokio::test]
async fn accepted_requirement_binds_full_scope_and_still_requires_protected_approval() {
    let (_directory, _, owner, originals, canonical, _) = fresh().await;
    let (target, expected) = requirement_target(&owner, &originals, &canonical).await;
    assert!(matches!(
        retain_requirement(&owner, &originals, target.clone(), expected.source.clone()).await,
        Err(MemberMcpError::Unavailable)
    ));
    let application = Arc::new(RequirementApplication::default());
    *application.row.lock().unwrap() = Some(expected.clone());
    originals
        .bind_member_mcp_application(application.clone())
        .unwrap();
    let selected = retain_requirement(&owner, &originals, target, expected.source.clone())
        .await
        .unwrap();
    assert_eq!(selected.accepted_requirement(), Some(&expected));
    assert!(
        serde_json::to_value(&expected)
            .unwrap()
            .get("contract")
            .is_none()
    );
    let mut wrong_contract = expected.clone();
    wrong_contract.contract = MemberMcpRequirementContract::ObservedRequirement;
    assert!(matches!(
        wrong_contract.check_owner(application.as_ref()),
        Err(HostLifetimeError::Denied)
    ));
    let mut missing = selected.intent.clone();
    missing.accepted_requirement = None;
    assert!(missing.check_requirement_shape().is_err());
    assert_eq!(
        selected.request().proposed_action.body.as_ref().unwrap()["accepted_requirement"],
        serde_json::json!(expected)
    );
    let approval = approve(&selected);
    assert!(matches!(
        owner
            .approve_and_issue_member_mcp(&originals, &selected, approval.clone(), &|| false)
            .await,
        Err(MemberMcpError::Denied)
    ));
    assert!(
        owner
            .source
            .grants()
            .issued_reference(&native_id(&owner, &selected))
            .unwrap()
            .is_none()
    );
    owner
        .approve_and_issue_member_mcp(&originals, &selected, approval, &|| true)
        .await
        .unwrap();
    let resumed = owner
        .resume_member_mcp_review(&originals, selected.binding())
        .await
        .unwrap();
    assert_eq!(resumed.request(), selected.request());
    assert!(
        owner
            .read_member_mcp(selected.binding())
            .await
            .unwrap()
            .1
            .lineage(&owner.source)
            .is_ok()
    );
    let machine = Arc::new(crate::original_ingress::tests::machine(&owner.source));
    originals.bind_machine(machine.clone()).unwrap();
    owner
        .revoke_member_mcp_binding(&originals, selected.binding())
        .await
        .unwrap();
    drop(machine);
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn accepted_requirement_reobserves_drift_and_final_native_currentness() {
    let (_directory, _, owner, originals, canonical, _) = fresh().await;
    let (target, expected) = requirement_target(&owner, &originals, &canonical).await;
    let application = Arc::new(RequirementApplication::default());
    *application.row.lock().unwrap() = Some(expected.clone());
    originals
        .bind_member_mcp_application(application.clone())
        .unwrap();
    let selected = retain_requirement(&owner, &originals, target.clone(), expected.source.clone())
        .await
        .unwrap();
    let approval = approve(&selected);
    for change in 0..7 {
        let mut row = expected.clone();
        match change {
            0 => row.selection.identity.push_str("-other"),
            1 => row.selection.admission_id = Id::new("other-admission").unwrap(),
            2 => row.selection.class = Id::new("other-class").unwrap(),
            3 => row.source.sha256 = "b".repeat(64),
            4 => row.services.reverse(),
            5 => {
                row.services[0].operations.insert("write".into());
            }
            _ => row.services.clear(),
        }
        *application.row.lock().unwrap() = Some(row);
        assert!(
            owner
                .resume_member_mcp_review(&originals, selected.binding())
                .await
                .is_err(),
            "change {change}"
        );
        assert!(
            matches!(
                owner
                    .approve_and_issue_member_mcp(&originals, &selected, approval.clone(), &|| true)
                    .await,
                Err(MemberMcpError::Denied)
            ),
            "change {change}"
        );
        assert!(
            owner
                .source
                .grants()
                .issued_reference(&native_id(&owner, &selected))
                .unwrap()
                .is_none()
        );
    }
    *application.row.lock().unwrap() = Some(expected);
    // Reuse the already retained exact decision, not a replacement approval.
    assert_eq!(
        owner
            .read_member_mcp(selected.binding())
            .await
            .unwrap()
            .1
            .approval
            .as_ref(),
        Some(&approval)
    );
    owner
        .approve_and_issue_member_mcp(&originals, &selected, approval, &|| true)
        .await
        .unwrap();
    let finished = {
        let writer = owner.source.source_writer().await;
        let finishing = owner.finish_member_mcp_review(&originals, &selected);
        tokio::pin!(finishing);
        let early = std::future::poll_fn(|cx| {
            std::task::Poll::Ready(match std::future::Future::poll(finishing.as_mut(), cx) {
                std::task::Poll::Pending => None,
                std::task::Poll::Ready(value) => Some(value),
            })
        })
        .await;
        *application.row.lock().unwrap() = None;
        drop(writer);
        let pending = early.is_none();
        let value = match early {
            Some(value) => value,
            None => finishing.await,
        };
        assert!(
            pending,
            "the finalizer must cross the held source-writer boundary"
        );
        value
    };
    assert!(matches!(finished, Err(MemberMcpError::Denied)));
    let machine = Arc::new(crate::original_ingress::tests::machine(&owner.source));
    originals.bind_machine(machine.clone()).unwrap();
    owner
        .revoke_member_mcp_binding(&originals, selected.binding())
        .await
        .unwrap();
    drop(machine);
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn accepted_empty_requirement_is_retained_without_borrowing_changed_intent() {
    use crate::installed_host::member_mcp_consumer::{
        MemberMcpApplication, RetainedMemberMcpBindings,
    };
    let (_directory, _, owner, originals, canonical, _) = fresh().await;
    let (target, mut expected) = requirement_target(&owner, &originals, &canonical).await;
    let application = Arc::new(RequirementApplication::default());
    originals
        .bind_member_mcp_application(application.clone())
        .unwrap();
    assert!(matches!(
        application
            .accepted_requirement(expected.selection.clone(), expected.source.clone())
            .await,
        Err(HostLifetimeError::Unavailable)
    ));
    let nonempty = expected.clone();
    expected.services.clear();
    *application.row.lock().unwrap() = Some(expected.clone());
    let retained = RetainedMemberMcpBindings {
        selection: expected.selection.clone(),
        bindings: vec![],
        accepted_requirement: Some(expected.clone()),
    };
    application.retain(retained.clone()).await.unwrap();
    retained
        .reobserve_requirement(application.as_ref())
        .await
        .unwrap();
    assert!(
        retain_requirement(&owner, &originals, target, expected.source.clone())
            .await
            .is_err()
    );
    *application.row.lock().unwrap() = Some(nonempty);
    let prior = application
        .lookup(expected.selection.clone())
        .await
        .unwrap();
    assert_eq!(prior, retained);
    assert!(
        prior
            .reobserve_requirement(application.as_ref())
            .await
            .is_err()
    );
    assert!(application.retain(retained).await.is_err());
    let operator = review(&owner, &originals, &canonical, "operator", "hn").await;
    let bytes = serde_json::to_value(&operator.intent).unwrap();
    assert!(bytes.get("accepted_requirement").is_none());
    let legacy: Intent = serde_json::from_value(bytes).unwrap();
    assert_eq!(legacy.body(), operator.intent.body());
    let mut mismatched = legacy;
    mismatched.accepted_requirement = Some(expected);
    assert!(mismatched.check_requirement_shape().is_err());
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

// These controls use the real retained row, native issuer and registration
// mandate combiner. They do not claim a Python Add or provider invocation.
async fn observed_requirement_review(
    owner: &OperatorSetup,
    originals: &PrivateOriginals,
    canonical: &Arc<ServicePolicyOwner>,
) -> (Arc<RequirementApplication>, MemberMcpReview) {
    use crate::installed_host::member_mcp_consumer::{
        MemberMcpApplication, RetainedMemberMcpBindings,
    };
    let (target, mut expected) = requirement_target(owner, originals, canonical).await;
    expected.contract = MemberMcpRequirementContract::ObservedRequirement;
    expected.services.truncate(1);
    let application = Arc::new(RequirementApplication {
        contract: MemberMcpRequirementContract::ObservedRequirement,
        ..Default::default()
    });
    *application.row.lock().unwrap() = Some(expected.clone());
    originals
        .bind_member_mcp_application(application.clone())
        .unwrap();
    let review = retain_requirement(owner, originals, target, expected.source.clone())
        .await
        .unwrap();
    application
        .retain(RetainedMemberMcpBindings {
            selection: expected.selection.clone(),
            bindings: vec![review.binding().clone()],
            accepted_requirement: Some(expected.clone()),
        })
        .await
        .unwrap();
    assert!(
        owner
            .source
            .grants()
            .issued_reference(&native_id(owner, &review))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        application
            .lookup(expected.selection.clone())
            .await
            .unwrap()
            .accepted_requirement,
        Some(expected)
    );
    let body = review.request().proposed_action.body.as_ref().unwrap();
    assert_eq!(body["requirement_contract"], "observed_requirement");
    assert_eq!(
        body["accepted_requirement"]["contract"],
        "observed_requirement"
    );
    (application, review)
}

#[tokio::test]
async fn observed_requirement_void_before_issue_issues_nothing() {
    let (_directory, _, owner, originals, canonical, _) = fresh().await;
    let (application, review) = observed_requirement_review(&owner, &originals, &canonical).await;
    *application.row.lock().unwrap() = None;
    assert!(matches!(
        owner
            .approve_and_issue_member_mcp(&originals, &review, approve(&review), &|| true)
            .await,
        Err(MemberMcpError::Unavailable)
    ));
    assert!(
        owner
            .source
            .grants()
            .issued_reference(&native_id(&owner, &review))
            .unwrap()
            .is_none()
    );
    let record = owner.read_member_mcp(review.binding()).await.unwrap().1;
    assert!(!record.observed_requirement_published);
    assert!(record.approval.is_none());
    assert!(matches!(record.issued, ParentIssuance::Pending));
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn observed_requirement_void_during_issue_revokes_without_publication() {
    let (_directory, _, owner, originals, canonical, _) = fresh().await;
    let (application, review) = observed_requirement_review(&owner, &originals, &canonical).await;
    let machine = Arc::new(crate::original_ingress::tests::machine(&owner.source));
    originals.bind_machine(machine.clone()).unwrap();
    // The real presenter's final synchronous check sits after the remote read
    // and before issue_root. The owner changes here; no grant result is faked.
    owner
        .approve_and_issue_member_mcp(&originals, &review, approve(&review), &|| {
            *application.row.lock().unwrap() = None;
            true
        })
        .await
        .unwrap();
    let reference = owner
        .source
        .grants()
        .issued_reference(&native_id(&owner, &review))
        .unwrap()
        .unwrap();
    assert!(
        owner
            .settle_member_mcp_review(&originals, &review)
            .await
            .is_err()
    );
    let (saved, record) = owner.read_member_mcp(review.binding()).await.unwrap();
    assert!(record.cancelled && !record.observed_requirement_published);
    assert!(matches!(record.cleanup, Some(Cleanup::Revoked)));
    assert!(
        owner
            .source
            .grants()
            .resolve_lineage(&[reference], &record.intent.executor, None)
            .is_err()
    );
    assert!(
        crate::private_originals::member_mcp::combine_member_mcp_records(
            &owner.source,
            &[(saved, record)]
        )
        .is_err()
    );
    assert_eq!(
        application
            .retained
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .bindings,
        vec![review.binding().clone()]
    );
    drop(machine);
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn observed_requirement_unpublished_grant_cannot_select_a_mandate() {
    let (_directory, _, owner, originals, canonical, _) = fresh().await;
    let (_application, review) = observed_requirement_review(&owner, &originals, &canonical).await;
    issue(&owner, &originals, &review).await;
    let (saved, record) = owner.read_member_mcp(review.binding()).await.unwrap();
    assert!(
        record.issued_lineage(&owner.source).is_ok(),
        "the native grant really was issued"
    );
    assert!(!record.observed_requirement_published);
    assert!(matches!(
        record.lineage(&owner.source),
        Err(MemberMcpError::PendingReconciliation)
    ));
    assert!(matches!(
        record.retained_lineage(&owner.source),
        Err(MemberMcpError::PendingReconciliation)
    ));
    assert!(
        crate::private_originals::member_mcp::combine_member_mcp_records(
            &owner.source,
            &[(saved, record)]
        )
        .is_err()
    );
    assert_eq!(
        owner
            .settle_member_mcp_review(&originals, &review)
            .await
            .unwrap(),
        *review.binding()
    );
    let (saved, record) = owner.read_member_mcp(review.binding()).await.unwrap();
    assert!(record.observed_requirement_published);
    assert!(
        crate::private_originals::member_mcp::combine_member_mcp_records(
            &owner.source,
            &[(saved, record)]
        )
        .is_ok()
    );
    let machine = Arc::new(crate::original_ingress::tests::machine(&owner.source));
    originals.bind_machine(machine.clone()).unwrap();
    owner
        .revoke_member_mcp_binding(&originals, review.binding())
        .await
        .unwrap();
    drop(machine);
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn observed_requirement_failed_revoke_is_retried_before_cold_publication() {
    let (_directory, selection, owner, originals, canonical, _) = fresh().await;
    let (application, review) = observed_requirement_review(&owner, &originals, &canonical).await;
    issue(&owner, &originals, &review).await;
    let binding = review.binding().clone();
    let reference = owner
        .source
        .grants()
        .issued_reference(&native_id(&owner, &review))
        .unwrap()
        .unwrap();
    *application.row.lock().unwrap() = None;
    // No machine is bound yet, so the actual native revoke cannot acquire its
    // controller mutation custody. Cancellation and locators still persist.
    assert!(matches!(
        owner.settle_member_mcp_review(&originals, &review).await,
        Err(MemberMcpError::PendingReconciliation)
    ));
    let (saved, record) = owner.read_member_mcp(&binding).await.unwrap();
    assert!(record.cancelled && record.cleanup.is_none() && !record.observed_requirement_published);
    assert!(
        crate::private_originals::member_mcp::combine_member_mcp_records(
            &owner.source,
            &[(saved, record)]
        )
        .is_err()
    );
    drop(review);
    drop(application);
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();

    let owner = OperatorSetup::open(selection, OperatorOwnerStartup::Restore).unwrap();
    let (originals, canonical, _) = bind(&owner).await;
    // Same startup recovery called by prepare_bootstrap. It must not need a
    // surviving nonvoided application row or claim cleanup before it succeeds.
    assert!(matches!(
        originals
            .reconcile_observed_member_mcp(owner.lifetime.clone())
            .await,
        Err(MemberMcpError::PendingReconciliation)
    ));
    let pending = owner.read_member_mcp(&binding).await.unwrap().1;
    assert!(
        pending.cancelled && pending.cleanup.is_none() && !pending.observed_requirement_published
    );
    let machine = Arc::new(crate::original_ingress::tests::machine(&owner.source));
    originals.bind_machine(machine.clone()).unwrap();
    originals
        .reconcile_observed_member_mcp(owner.lifetime.clone())
        .await
        .unwrap();
    let (saved, record) = owner.read_member_mcp(&binding).await.unwrap();
    assert!(record.cancelled && !record.observed_requirement_published);
    assert!(matches!(record.cleanup, Some(Cleanup::Revoked)));
    assert_eq!(
        owner.source.grants().issued_reference(&binding.0).unwrap(),
        Some(reference.clone())
    );
    assert!(
        owner
            .source
            .grants()
            .resolve_lineage(&[reference], &record.intent.executor, None)
            .is_err()
    );
    assert!(
        crate::private_originals::member_mcp::combine_member_mcp_records(
            &owner.source,
            &[(saved.clone(), record)]
        )
        .is_err()
    );
    originals
        .reconcile_observed_member_mcp(owner.lifetime.clone())
        .await
        .unwrap();
    assert_eq!(owner.read_member_mcp(&binding).await.unwrap().0, saved);
    drop(machine);
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

#[test]
fn current_public_server_needs_explicit_ceiling_but_not_an_account_or_read_only_limit() {
    let selected = Installation::capture(
        &meerkat_core::RealmId::parse("work").unwrap(),
        &config("hn"),
        None,
        ["write".into()].into(),
    )
    .unwrap();
    assert!(selected.account.is_none());
    assert!(
        selected
            .config(&[config("unrelated"), config("hn")])
            .is_ok()
    );
    for configs in [vec![], vec![config("hn"), config("hn")]] {
        assert!(matches!(
            selected.config(&configs),
            Err(MemberMcpError::InstallationChanged)
        ));
    }
    let mut changed = config("hn");
    changed.connect_timeout_secs = Some(99);
    assert!(matches!(
        selected.config(&[changed]),
        Err(MemberMcpError::InstallationChanged)
    ));
    for operations in [
        BTreeSet::new(),
        ["undeclared".into()].into(),
        [" read".into()].into(),
    ] {
        assert!(matches!(
            Installation::capture(
                &meerkat_core::RealmId::parse("work").unwrap(),
                &config("hn"),
                None,
                operations
            ),
            Err(MemberMcpError::Denied)
        ));
    }
    let mut absent = config("hn");
    absent.access = None;
    assert!(matches!(
        Installation::capture(
            &meerkat_core::RealmId::parse("work").unwrap(),
            &absent,
            None,
            ["read".into()].into()
        ),
        Err(MemberMcpError::Denied)
    ));
}

#[test]
fn declared_account_requires_exact_current_native_identity_without_collector_limits() {
    // Reuse only the synthetic config/status values, never collector authority.
    let (mut config, status, _) = crate::private_originals::collector_setup::tests::fixture();
    config.access = Some(meerkat_core::mcp_config::McpServerAccess {
        allowed_operations: ["history_page".into()].into(),
    });
    config
        .connector
        .as_mut()
        .unwrap()
        .operations
        .get_mut("history_page")
        .unwrap()
        .read_only = false;
    let operations = ["history_page".into()].into();
    let selected = Installation::capture(
        &meerkat_core::RealmId::parse("work").unwrap(),
        &config,
        Some(&status),
        operations,
    )
    .unwrap();
    assert!(selected.account.is_some());
    assert!(matches!(
        Installation::capture(
            &meerkat_core::RealmId::parse("other-credentials").unwrap(),
            &config,
            Some(&status),
            ["history_page".into()].into()
        ),
        Err(MemberMcpError::Denied)
    ));

    assert!(matches!(
        Installation::capture(
            &meerkat_core::RealmId::parse("work").unwrap(),
            &config,
            None,
            ["history_page".into()].into()
        ),
        Err(MemberMcpError::ReauthRequired)
    ));
    for field in ["issuer", "subject", "strategy", "slot", "phase"] {
        let mut changed = status.clone();
        match field {
            "issuer" => changed.verified_account.as_mut().unwrap().issuer = "other".into(),
            "subject" => changed.verified_account.as_mut().unwrap().subject = "other".into(),
            "strategy" => changed.verified_account.as_mut().unwrap().strategy_id = "other".into(),
            "slot" => {
                changed.slot.account = meerkat_core::CredentialAccountId::parse("other").unwrap()
            }
            _ => changed.phase = ConnectorAuthPhase::AuthorizationRequired,
        }
        assert!(
            Installation::capture(
                &meerkat_core::RealmId::parse("work").unwrap(),
                &config,
                Some(&changed),
                ["history_page".into()].into()
            )
            .is_err()
        );
    }
    let encoded = serde_json::to_value(&selected).unwrap();
    assert!(encoded.get("command").is_none());
    assert!(encoded.get("env").is_none());
}

#[tokio::test]
async fn credential_realm_is_separate_from_member_admission_and_grant_scope() {
    let (_directory, _, owner, originals, canonical, _) = fresh().await;
    let admitted = canonical
        .admit_member_declaration(
            Id::new("account-member").unwrap(),
            instance(&owner, "account-member"),
        )
        .await
        .unwrap();
    let (mut config, status, _) = crate::private_originals::collector_setup::tests::fixture();
    config.access = Some(meerkat_core::mcp_config::McpServerAccess {
        allowed_operations: ["history_page".into()].into(),
    });
    let credential_realm = status.slot.realm.clone();
    assert_ne!(credential_realm, owner.native.realm);
    let installation = Installation::capture(
        &credential_realm,
        &config,
        Some(&status),
        ["history_page".into()].into(),
    )
    .unwrap();
    assert!(matches!(
        Installation::capture(
            &owner.native.realm,
            &config,
            Some(&status),
            ["history_page".into()].into()
        ),
        Err(MemberMcpError::Denied)
    ));
    let review = owner
        .retain_member_mcp_review(
            &originals,
            originals.member_mcp_target(&admitted).await.unwrap(),
            MemberMcpOrigin::OperatorSelf,
            installation,
            LifetimeRestriction::unrestricted(),
            super::super::tests::request(),
        )
        .await
        .unwrap();
    issue(&owner, &originals, &review).await;
    let (saved, record) = owner.read_member_mcp(review.binding()).await.unwrap();
    assert_eq!(saved.key.realm, owner.native.realm.to_string());
    assert_eq!(
        record
            .intent
            .installation
            .account
            .as_ref()
            .unwrap()
            .slot
            .realm,
        credential_realm
    );
    assert_eq!(
        record.lineage(&owner.source).unwrap()[0].root_authority,
        owner.operator
    );
    assert_eq!(record.intent.target.id, *admitted.admission_id());
    drop(originals);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn pre_session_review_replay_and_complete_multi_server_mandate_use_actual_grants() {
    let (_directory, _, owner, originals, canonical, _) = fresh().await;
    let a = review(&owner, &originals, &canonical, "one", "hn").await;
    let b = review(&owner, &originals, &canonical, "one", "calendar").await;
    assert!(
        owner
            .source
            .grants()
            .issued_reference(&native_id(&owner, &a))
            .unwrap()
            .is_none()
    );
    assert!(
        canonical
            .observe_member(&a.intent.target.instance.member.member)
            .await
            .unwrap()
            .admission
            .destination
            .is_none()
    );
    let approval = approve(&a);
    owner
        .approve_and_issue_member_mcp(&originals, &a, approval.clone(), &|| true)
        .await
        .unwrap();
    let first = owner
        .source
        .grants()
        .issued_reference(&native_id(&owner, &a))
        .unwrap()
        .unwrap();
    let resumed = owner
        .resume_member_mcp_review(&originals, a.binding())
        .await
        .unwrap();
    owner
        .approve_and_issue_member_mcp(&originals, &resumed, approval, &|| true)
        .await
        .unwrap();
    assert_eq!(
        owner
            .source
            .grants()
            .issued_reference(&native_id(&owner, &a))
            .unwrap(),
        Some(first)
    );
    issue(&owner, &originals, &b).await;
    let records = vec![
        owner.read_member_mcp(a.binding()).await.unwrap(),
        owner.read_member_mcp(b.binding()).await.unwrap(),
    ];
    let mandate =
        crate::private_originals::member_mcp::combine_member_mcp_records(&owner.source, &records)
            .unwrap();
    assert_eq!(
        mandate.executor(),
        &a.intent.target.instance.member.logical_owner
    );
    assert_eq!(mandate.represented_subject(), None);
    assert_eq!(
        mandate
            .server_lineages()
            .map(|(name, _)| name)
            .collect::<Vec<_>>(),
        vec!["calendar", "hn"]
    );
    assert_eq!(
        mandate.server_lineage("hn").unwrap(),
        records[0].1.lineage(&owner.source).unwrap()
    );
    assert!(
        crate::private_originals::member_mcp::combine_member_mcp_records(
            &owner.source,
            &[records[0].clone(), records[0].clone()]
        )
        .is_err()
    );
    let other = review(&owner, &originals, &canonical, "two", "other").await;
    issue(&owner, &originals, &other).await;
    assert!(
        crate::private_originals::member_mcp::combine_member_mcp_records(
            &owner.source,
            &[
                records[0].clone(),
                owner.read_member_mcp(other.binding()).await.unwrap()
            ]
        )
        .is_err()
    );
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn exact_member_drift_presenter_loss_and_substituted_approval_refuse_before_issue() {
    let (_directory, _, owner, originals, canonical, current) = fresh().await;
    let a = review(&owner, &originals, &canonical, "one", "hn").await;
    let b = review(&owner, &originals, &canonical, "two", "hn").await;
    assert!(matches!(
        owner
            .approve_and_issue_member_mcp(&originals, &a, approve(&b), &|| true)
            .await,
        Err(MemberMcpError::Denied)
    ));
    let approval = approve(&a);
    assert!(matches!(
        owner
            .approve_and_issue_member_mcp(&originals, &a, approval.clone(), &|| false)
            .await,
        Err(MemberMcpError::Denied)
    ));
    current.store(false, Ordering::SeqCst);
    assert!(matches!(
        owner
            .approve_and_issue_member_mcp(&originals, &a, approval, &|| true)
            .await,
        Err(MemberMcpError::Denied)
    ));
    assert!(
        owner
            .source
            .grants()
            .issued_reference(&native_id(&owner, &a))
            .unwrap()
            .is_none()
    );
    current.store(true, Ordering::SeqCst);
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn cancellation_before_issue_is_sticky_and_other_member_survives() {
    let (_directory, _, owner, originals, canonical, _) = fresh().await;
    let a = review(&owner, &originals, &canonical, "one", "hn").await;
    let b = review(&owner, &originals, &canonical, "two", "hn").await;
    owner
        .revoke_member_mcp_binding(&originals, a.binding())
        .await
        .unwrap();
    assert!(matches!(
        owner
            .approve_and_issue_member_mcp(&originals, &a, approve(&a), &|| true)
            .await,
        Err(MemberMcpError::Cancelled)
    ));
    assert!(
        owner
            .source
            .grants()
            .issued_reference(&native_id(&owner, &a))
            .unwrap()
            .is_none()
    );
    issue(&owner, &originals, &b).await;
    assert!(
        owner
            .read_member_mcp(b.binding())
            .await
            .unwrap()
            .1
            .lineage(&owner.source)
            .is_ok()
    );
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn closing_host_cleans_retained_member_binding_but_cannot_issue() {
    use crate::host_gateway::ToolkitGatewayHost;
    use crate::installed_host::member_mcp_consumer::PreparedMemberMcp;
    use meerkat_mobkit::rpc_gateway::{GatewayHostCloseReason, GatewayHostComposition};

    let (_directory, _, owner, originals, canonical, _) = fresh().await;
    let a = review(&owner, &originals, &canonical, "closing", "hn").await;
    let b = review(&owner, &originals, &canonical, "unrelated", "other").await;
    let c = review(&owner, &originals, &canonical, "unissued", "hn").await;
    issue(&owner, &originals, &a).await;
    issue(&owner, &originals, &b).await;
    let reference = owner
        .source
        .grants()
        .issued_reference(&native_id(&owner, &a))
        .unwrap()
        .unwrap();
    let executor = a.intent.executor.clone();
    let (_open, closed) = tokio::sync::watch::channel(false);
    let host = ToolkitGatewayHost::new(
        owner.lifetime.clone(),
        crate::private_host::PrivateHostClosed(closed),
    );
    // Exercise the actual host close and cleanup entries with the owners
    // retained by an accepted operation, without claiming member construction.
    let prepared = PreparedMemberMcp {
        operator: owner,
        originals: originals.clone(),
        bindings: vec![a.binding().clone(), c.binding().clone()],
    };
    host.begin_close(GatewayHostCloseReason::Shutdown).unwrap();
    assert!(
        prepared
            .operator
            .lifetime
            .validate_current_selection()
            .is_err()
    );
    assert!(matches!(
        prepared
            .operator
            .approve_and_issue_member_mcp(&originals, &c, approve(&c), &|| true)
            .await,
        Err(MemberMcpError::Denied)
    ));
    assert!(
        prepared
            .operator
            .source
            .grants()
            .issued_reference(&native_id(&prepared.operator, &c))
            .unwrap()
            .is_none()
    );

    // The missing native mutation owner keeps cancellation durable and cleanup
    // pending. Closure is never reported as successful revocation by itself.
    assert_eq!(
        host.cleanup_member_mcp_bindings(Some(&prepared)).await,
        Err(HostLifetimeError::NotSettled)
    );
    let pending = prepared
        .operator
        .read_member_mcp(a.binding())
        .await
        .unwrap()
        .1;
    assert!(pending.cancelled && pending.cleanup.is_none());
    let not_issued = prepared
        .operator
        .read_member_mcp(c.binding())
        .await
        .unwrap()
        .1;
    assert!(not_issued.cancelled && matches!(not_issued.cleanup, Some(Cleanup::NotIssued)));

    let machine = Arc::new(crate::original_ingress::tests::machine(
        &prepared.operator.source,
    ));
    originals.bind_machine(machine.clone()).unwrap();
    host.cleanup_member_mcp_bindings(Some(&prepared))
        .await
        .unwrap();
    host.cleanup_member_mcp_bindings(Some(&prepared))
        .await
        .unwrap();
    let revoked = prepared
        .operator
        .read_member_mcp(a.binding())
        .await
        .unwrap()
        .1;
    assert!(revoked.cancelled && matches!(revoked.cleanup, Some(Cleanup::Revoked)));
    assert_eq!(
        prepared
            .operator
            .source
            .grants()
            .issued_reference(&native_id(&prepared.operator, &a))
            .unwrap(),
        Some(reference.clone())
    );
    assert!(
        prepared
            .operator
            .source
            .grants()
            .resolve_lineage(&[reference], &executor, None)
            .is_err()
    );
    assert!(
        prepared
            .operator
            .read_member_mcp(b.binding())
            .await
            .unwrap()
            .1
            .lineage(&prepared.operator.source)
            .is_ok()
    );
    drop(host);
    drop(machine);
    drop(originals);
    drop(canonical);
    let PreparedMemberMcp {
        operator,
        originals: retained,
        ..
    } = prepared;
    drop(retained);
    operator.finish().await.unwrap();
}

#[tokio::test]
async fn closed_member_cleanup_rejects_another_source_store_pair_before_cancellation() {
    use meerkat_toolkit::host_lifetime::control::{HostCloseReason, PrivateHostLifetimeControl};
    let (_directory, _, owner, originals, canonical, _) = fresh().await;
    let (_other_directory, _, other, other_originals, other_canonical, _) = fresh().await;
    let selected = review(&owner, &originals, &canonical, "one", "hn").await;
    issue(&owner, &originals, &selected).await;
    let before = owner.read_member_mcp(selected.binding()).await.unwrap().0;
    owner
        .lifetime
        .begin_close_activation(HostCloseReason::Shutdown)
        .unwrap();
    assert!(matches!(
        owner
            .revoke_member_mcp_binding(&other_originals, selected.binding())
            .await,
        Err(MemberMcpError::Denied)
    ));
    assert_eq!(
        owner.read_member_mcp(selected.binding()).await.unwrap().0,
        before
    );
    drop(other_originals);
    drop(other_canonical);
    other.finish().await.unwrap();
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn post_io_finalizer_observes_concurrent_cancel_and_pending_native_cleanup() {
    let (_directory, _, owner, originals, canonical, _) = fresh().await;
    let review = review(&owner, &originals, &canonical, "one", "hn").await;
    issue(&owner, &originals, &review).await;
    let io_started = tokio::sync::Notify::new();
    let io_done = tokio::sync::Notify::new();
    let finishing = async {
        // Hold the same post-issuance I/O boundary, using the real durable
        // decision and revoke operation. No fake host or account is selected.
        io_started.notify_one();
        io_done.notified().await;
        owner.finish_member_mcp_review(&originals, &review).await
    };
    let cancelling = async {
        io_started.notified().await;
        let result = owner
            .revoke_member_mcp_binding(&originals, review.binding())
            .await;
        assert!(matches!(result, Err(MemberMcpError::PendingReconciliation)));
        io_done.notify_one();
    };
    let (finished, ()) = tokio::join!(finishing, cancelling);
    assert!(matches!(
        finished,
        Err(MemberMcpError::PendingReconciliation)
    ));
    let machine = Arc::new(crate::original_ingress::tests::machine(&owner.source));
    originals.bind_machine(machine.clone()).unwrap();
    owner
        .revoke_member_mcp_binding(&originals, review.binding())
        .await
        .unwrap();
    assert!(matches!(
        owner.finish_member_mcp_review(&originals, &review).await,
        Err(MemberMcpError::Cancelled)
    ));
    drop(machine);
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn lost_native_ack_is_reconciled_or_revoked_by_the_same_pre_retained_id() {
    for cancel in [false, true] {
        let (_directory, _, owner, originals, canonical, _) = fresh().await;
        let review = review(&owner, &originals, &canonical, "one", "hn").await;
        let approval = approve(&review);
        let (saved, mut record) = owner.read_member_mcp(review.binding()).await.unwrap();
        record.approval = Some(approval.clone());
        owner.save_member_mcp(&saved, &record).await.unwrap();
        // Actual native commit with the record still Pending models only the
        // acknowledgement gap; no invented grant reference or permission.
        let reference = owner
            .source
            .grants()
            .issue_root(
                &owner.operator,
                native_id(&owner, &review),
                record.intent.executor.clone(),
                None,
                record.intent.restrictions.clone(),
            )
            .unwrap();
        if cancel {
            assert!(matches!(
                owner
                    .revoke_member_mcp_binding(&originals, review.binding())
                    .await,
                Err(MemberMcpError::PendingReconciliation)
            ));
            let pending = owner.read_member_mcp(review.binding()).await.unwrap().1;
            assert!(pending.cancelled && pending.cleanup.is_none());
            assert!(matches!(pending.issued, ParentIssuance::Issued { .. }));
            let machine = Arc::new(crate::original_ingress::tests::machine(&owner.source));
            originals.bind_machine(machine.clone()).unwrap();
            owner
                .revoke_member_mcp_binding(&originals, review.binding())
                .await
                .unwrap();
            assert!(matches!(
                owner
                    .read_member_mcp(review.binding())
                    .await
                    .unwrap()
                    .1
                    .cleanup,
                Some(Cleanup::Revoked)
            ));
            assert!(
                owner
                    .source
                    .grants()
                    .resolve_lineage(&[reference], &record.intent.executor, None)
                    .is_err()
            );
            drop(machine);
        } else {
            owner
                .approve_and_issue_member_mcp(&originals, &review, approval, &|| true)
                .await
                .unwrap();
            assert_eq!(
                owner
                    .source
                    .grants()
                    .issued_reference(&native_id(&owner, &review))
                    .unwrap(),
                Some(reference)
            );
        }
        drop(originals);
        drop(canonical);
        owner.finish().await.unwrap();
    }
}

#[tokio::test]
async fn cold_owner_recovers_exact_record_but_successor_cannot_inherit_it() {
    let (_directory, selection, owner, originals, canonical, _) = fresh().await;
    let review = review(&owner, &originals, &canonical, "one", "hn").await;
    issue(&owner, &originals, &review).await;
    let binding = review.binding().clone();
    let instance = review.intent.target.instance.clone();
    let old_id = review.intent.target.id.clone();
    drop(review);
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
    let owner = OperatorSetup::open(selection, OperatorOwnerStartup::Restore).unwrap();
    let (originals, canonical, _) = bind(&owner).await;
    let resumed = owner
        .resume_member_mcp_review(&originals, &binding)
        .await
        .unwrap();
    assert_eq!(resumed.intent.target.id, old_id);
    assert!(
        owner
            .read_member_mcp(&binding)
            .await
            .unwrap()
            .1
            .lineage(&owner.source)
            .is_ok()
    );
    canonical.retire_member(&old_id).await.unwrap();
    let checkpoint = canonical
        .observe_member(&instance.member.member)
        .await
        .unwrap()
        .checkpoint;
    canonical
        .readmit_member_declaration(checkpoint, Id::new("successor").unwrap(), instance)
        .await
        .unwrap();
    assert!(matches!(
        owner.resume_member_mcp_review(&originals, &binding).await,
        Err(MemberMcpError::Denied)
    ));
    // The old native grant remains a separate revocable fact. It cannot become
    // the successor's mandate just because the native subject is the same.
    assert!(
        owner
            .read_member_mcp(&binding)
            .await
            .unwrap()
            .1
            .lineage(&owner.source)
            .is_ok()
    );
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn input_source_keeps_exact_issued_targets_after_revoke_without_granting_permission() {
    use crate::installed_host::member_mcp_consumer::{
        MemberMcpAdmissionSelection, MemberMcpApplication, RetainedMemberMcpBindings,
    };
    struct Application(std::sync::Mutex<Option<RetainedMemberMcpBindings>>);
    #[async_trait::async_trait]
    impl MemberMcpApplication for Application {
        async fn retain(&self, value: RetainedMemberMcpBindings) -> Result<(), HostLifetimeError> {
            let mut row = self.0.lock().unwrap();
            if row.as_ref().is_some_and(|old| old != &value) {
                return Err(HostLifetimeError::Denied);
            }
            *row = Some(value);
            Ok(())
        }
        async fn lookup(
            &self,
            selection: MemberMcpAdmissionSelection,
        ) -> Result<RetainedMemberMcpBindings, HostLifetimeError> {
            let value = self
                .0
                .lock()
                .unwrap()
                .clone()
                .ok_or(HostLifetimeError::Unavailable)?;
            value.check(&selection)?;
            Ok(value)
        }
    }
    let (_directory, _selection, owner, originals, canonical, _) = fresh().await;
    // Actual revocation needs the same native controller mutation owner as
    // production. Keep its strong owner alive through the retained-source checks.
    let machine = Arc::new(crate::original_ingress::tests::machine(&owner.source));
    originals.bind_machine(machine.clone()).unwrap();
    let review = review(&owner, &originals, &canonical, "one", "hn").await;
    let admission = canonical
        .observed_admission(&review.intent.target.instance.member.member)
        .unwrap()
        .unwrap();
    // Missing application custody never becomes implicit empty authority.
    assert!(matches!(
        originals.select_member_mcp_source(&admission, None).await,
        Err(HostLifetimeError::Unavailable)
    ));
    let app = Arc::new(Application(std::sync::Mutex::new(None)));
    originals.bind_member_mcp_application(app.clone()).unwrap();
    assert!(
        originals
            .select_member_mcp_source(&admission, None)
            .await
            .is_err()
    );
    app.retain(RetainedMemberMcpBindings {
        selection: MemberMcpAdmissionSelection::from_admission(&admission).unwrap(),
        bindings: vec![review.binding().clone()],
        accepted_requirement: None,
    })
    .await
    .unwrap();
    // Pending, unissued refs are not an input authority basis.
    assert!(
        originals
            .select_member_mcp_source(&admission, None)
            .await
            .is_err()
    );
    issue(&owner, &originals, &review).await;
    let selected = originals
        .select_member_mcp_source(&admission, None)
        .await
        .unwrap()
        .unwrap();
    let targets = selected.targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(
        targets[0].target,
        meerkat_authorization_contracts::work_association::GrantTarget::McpServer {
            server: "hn".into()
        }
    );
    owner
        .revoke_member_mcp_binding(&originals, review.binding())
        .await
        .unwrap();
    let retained = originals
        .select_member_mcp_source(&admission, Some(&selected))
        .await
        .unwrap()
        .unwrap();
    assert!(retained == selected);
    assert!(
        originals
            .check_member_mcp_source(&admission, &retained)
            .is_ok()
    );
    assert!(
        owner
            .source
            .grants()
            .resolve_lineage(&targets[0].lineage, &review.intent.executor, None)
            .is_err(),
        "retained identity never revives the selected permission"
    );
    // The existing input record cannot retarget the same issued id to another server.
    let mut changed = serde_json::to_value(&selected).unwrap();
    changed["entries"][0]["server"] = serde_json::json!("different");
    let changed = serde_json::from_value(changed).unwrap();
    assert!(
        originals
            .select_member_mcp_source(&admission, Some(&changed))
            .await
            .is_err()
    );
    canonical.retire_member(&admission.id).await.unwrap();
    assert!(
        originals
            .check_member_mcp_source(&admission, &selected)
            .is_err()
    );
    drop(machine);
    drop(originals);
    drop(canonical);
    owner.finish().await.unwrap();
}
