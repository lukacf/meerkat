//! Carrier/issuer controls through real prepared owner and native dispatch.
//! Meta owns the actual factory offer and machine-before-session installation;
//! these tests do not simulate GatewayFactoryConstruction or a Console login.
use super::*;
use toolkit_mobkit::installed_host::{
    PreparedInstallationOwner, operator_setup::OperatorOwnerStartup,
};

fn prepared(root: &Path) -> PreparedInstallationOwner {
    let (_, mut owner) = crate::application_setup::compilation::tests::helperless_input(root, true);
    owner.native_host.remote_operator = Some(
        PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            "operation-review-operator",
            TrustDomainId::new("application-setup-fixture").unwrap(),
        )
        .unwrap(),
    );
    fs::create_dir_all(root.join("shared/data")).unwrap();
    PreparedInstallationOwner::open(
        root.to_owned(),
        owner,
        OperatorOwnerStartup::fresh("operation-review-carrier", 1).unwrap(),
    )
    .unwrap()
}
fn selection(owner: Arc<ApplicationConsentOwner>) -> ApplicationOperationReview {
    ApplicationOperationReview::new(owner, Arc::new(NoModelReview), Duration::from_secs(30))
}
fn with_review(review: Arc<BoundOperationReview>, claim: &HostingClaim) -> ToolDispatchContext {
    ToolDispatchContext::default()
        .with_work_authorization(Some(
            WorkAuthorizationContext::new(
                Arc::new(ProbePolicy),
                meerkat_core::OperationExecutionScope::Domain,
            )
            .with_consent_hosting(claim.clone()),
        ))
        .with_operation_review(Some(review))
}

#[test]
fn application_review_binding_refuses_wrong_owner_changed_history_and_zero_bounds() {
    let temp = private_root();
    let root = temp.path().canonicalize().unwrap();
    let prepared = prepared(&root);
    let identity = ConsentApplicationIdentity::from_preparation(&prepared).unwrap();
    let owner = ApplicationConsentOwner::create(&root, identity.clone()).unwrap();
    let mut wrong = identity;
    wrong.deployment = Id::new("other-deployment").unwrap();
    let foreign = ApplicationConsentOwner::create(&root, wrong).unwrap();
    assert!(matches!(
        selection(foreign).bind(&prepared),
        Err(ConsentOwnerError::WrongApplication)
    ));
    assert!(matches!(
        ApplicationOperationReview::new(owner.clone(), Arc::new(NoModelReview), Duration::ZERO)
            .bind(&prepared),
        Err(ConsentOwnerError::InvalidSelection)
    ));
    assert!(matches!(
        selection(owner.clone())
            .with_human_consent(Arc::new(LocalHumanRoute), Duration::ZERO)
            .bind(&prepared),
        Err(ConsentOwnerError::InvalidSelection)
    ));
    let manifest = owner.selection().artifact().path.clone();
    let bytes = fs::read(&manifest).unwrap();
    fs::write(&manifest, b"changed").unwrap();
    assert!(selection(owner.clone()).bind(&prepared).is_err());
    fs::write(&manifest, &bytes).unwrap();
    let retained = selection(owner.clone()).bind(&prepared).unwrap();
    assert!(
        Arc::ptr_eq(&retained.owner().unwrap(), &owner),
        "the prepared carrier returns its actual issuer"
    );
    let first = retained.current().unwrap();
    assert!(Arc::ptr_eq(&first, &retained.current().unwrap()));
    fs::write(&manifest, b"changed after selection").unwrap();
    assert!(retained.current().is_err());
    fs::write(&manifest, bytes).unwrap();
    assert!(Arc::ptr_eq(&first, &retained.current().unwrap()));
    assert!(
        owner.store.load_all().unwrap().is_empty(),
        "binding issues no approval or grant"
    );
}

#[tokio::test]
async fn application_review_without_human_route_refuses_r3_without_consent_or_effect() {
    let temp = private_root();
    let root = temp.path().canonicalize().unwrap();
    let prepared = prepared(&root);
    let owner = ApplicationConsentOwner::create(
        &root,
        ConsentApplicationIdentity::from_preparation(&prepared).unwrap(),
    )
    .unwrap();
    let retained = selection(owner.clone()).bind(&prepared).unwrap();
    let review = retained.current().unwrap();
    assert!(review.consent_owner_loss_settlement().is_none());
    let session = meerkat_core::SessionId::new();
    let ordinary = ordinary(&owner, &session);
    let claim = grant_session_hosting(&hosting(&root), &HostingOwner::mint(), &session).unwrap();
    let context = with_review(review, &claim);
    let probe = Arc::new(Probe::default());
    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    let call = ToolCallView {
        id: "no-route",
        name: "consent_probe",
        args: &args,
    };
    let resolution = meerkat_core::ToolExecutionResolutionContext::new(
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            Duration::from_secs(30),
        )])
        .unwrap(),
    );
    let plan =
        meerkat_core::resolve_tool_execution_plan_fenced(&probe, call, &context, &resolution)
            .unwrap();
    assert!(matches!(
        meerkat_core::dispatch_tool_execution_plan_fenced(&probe, call, &context, &plan).await,
        Err(ToolError::ReviewUnsatisfied {
            kind: ReviewUnsatisfiedKind::HumanConsentRequired,
            approval_id: None
        })
    ));
    assert_eq!(probe.0.load(Ordering::SeqCst), 0);
    assert_eq!(owner.store.load_all().unwrap(), vec![ordinary.clone()]);
    assert!(
        ordinary.consent.is_none(),
        "ordinary history is not upgraded to consent"
    );
}

#[tokio::test]
async fn application_review_uses_exact_persistent_issuer_and_retains_lease_through_settlement() {
    let temp = private_root();
    let root = temp.path().canonicalize().unwrap();
    let prepared = prepared(&root);
    let identity = ConsentApplicationIdentity::from_preparation(&prepared).unwrap();
    let owner = ApplicationConsentOwner::create(&root, identity.clone()).unwrap();
    let issuer = owner.selection().clone();
    let retained = selection(owner.clone())
        .with_human_consent(Arc::new(LocalHumanRoute), Duration::from_secs(3600))
        .bind(&prepared)
        .unwrap();
    let review = retained.current().unwrap();
    let settlement = review.consent_owner_loss_settlement().unwrap();
    let session = meerkat_core::SessionId::new();
    let claim = grant_session_hosting(&hosting(&root), &HostingOwner::mint(), &session).unwrap();
    let context = with_review(review.clone(), &claim);
    let probe = Arc::new(Probe::default());
    let id = request(&probe, &context, "carrier-request").await;
    assert_eq!(
        owner.approvals().get(&id).unwrap().status,
        ApprovalStatus::Pending
    );
    approve(&owner, &id);
    meerkat_core::dispatch_retained_consent_attempt(&probe, &context, &id)
        .await
        .unwrap();
    assert_eq!(probe.0.load(Ordering::SeqCst), 1);
    let spent = owner.approvals().get(&id).unwrap();
    assert!(spent.consent.as_ref().unwrap().consumed_at.is_some());
    drop(context);
    drop(probe);
    drop(retained);
    drop(owner);
    assert!(
        matches!(
            ApplicationConsentOwner::reopen(issuer.clone(), &identity),
            Err(ConsentOwnerError::OwnerHeld)
        ),
        "the returned review still retains the actual issuer lease"
    );
    drop(review);
    assert!(
        matches!(
            ApplicationConsentOwner::reopen(issuer.clone(), &identity),
            Err(ConsentOwnerError::OwnerHeld)
        ),
        "the exact review's settlement still retains the same lease"
    );
    drop(settlement);
    let reopened = ApplicationConsentOwner::reopen(issuer, &identity).unwrap();
    assert_eq!(reopened.approvals().get(&id).unwrap(), spent);
}
