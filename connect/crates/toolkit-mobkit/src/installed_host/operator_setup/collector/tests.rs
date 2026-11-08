//! Real SQLite and native grant-owner checks. Account transport and complete
//! ServiceCall execution are deliberately not claimed by these owner tests.
use super::*;
use meerkat_core::{ApprovalPrincipalId, ApprovalService};
use toolkit_store::StoreScope;

async fn selected(
    selection: HostBootstrapSelection,
    startup: OperatorOwnerStartup,
) -> (OperatorSetup, PrivateOriginals) {
    let owner = OperatorSetup::open(selection, startup).unwrap();
    let scope = StoreScope {
        realm: owner.native.realm.to_string(),
        owner: owner.lifetime.selected_policy().deployment.to_string(),
    };
    let originals =
        PrivateOriginals::prepare(scope.clone(), owner.originals.clone(), owner.source.clone())
            .await
            .unwrap();
    owner
        .source
        .select_installed_pair_recovery(owner.originals.as_ref(), &scope)
        .unwrap();
    (owner, originals)
}
fn review(owner: &OperatorSetup) -> CollectorEnrollmentReview {
    let (config, status, _) = crate::private_originals::collector_setup::tests::fixture();
    // Real installation credentials are outside the canonical mob/source realm.
    assert_ne!(status.slot.realm, owner.native.realm);
    let installation =
        CollectorInstallation::from_native(&config, &status, ["history_page".into()].into())
            .unwrap();
    let collector = owner
        .bind_collector_principal(
            crate::installed_host::tests::principal("dedicated-source-collector"),
            AudienceRef::Principal {
                principal: owner.operator.clone(),
            },
        )
        .unwrap();
    // This collector is deliberately absent from the application's agent roster.
    assert!(
        !owner
            .native
            .members
            .iter()
            .any(|member| member.logical_owner == collector.principal)
    );
    owner
        .prepare_collector_enrollment(
            &collector,
            installation,
            CollectorSource {
                mob_id: EvidenceId::new("announcement-mob").unwrap(),
                source_id: EvidenceId::new("inbox-source").unwrap(),
                generation: 1,
            },
            LifetimeRestriction::unrestricted(),
            super::super::tests::request(),
        )
        .unwrap()
}
fn decision(review: &CollectorEnrollmentReview) -> CollectorDecision {
    let approvals = ApprovalService::new();
    let pending = approvals.request(review.request.clone()).unwrap();
    let approval = approvals
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("fixture-protected-presenter").unwrap(),
            None,
            None,
        )
        .unwrap();
    assert!(same_request(&review.request, &approval));
    CollectorDecision {
        cancelled: false,
        cleanup: None,
        intent: review.intent.clone(),
        approval,
        issued: ParentIssuance::Pending,
    }
}
async fn fresh() -> (
    tempfile::TempDir,
    HostBootstrapSelection,
    OperatorSetup,
    PrivateOriginals,
) {
    let (directory, selection) = crate::installed_host::tests::fixture();
    let (owner, originals) = selected(
        selection.clone(),
        OperatorOwnerStartup::fresh("collector-owner", 1).unwrap(),
    )
    .await;
    (directory, selection, owner, originals)
}

#[tokio::test]
async fn single_approval_issues_nonmember_collector_root_and_enrolls_exact_source() {
    let (_directory, _, owner, originals) = fresh().await;
    let review = review(&owner);
    assert_ne!(review.intent.installation.realm(), &owner.native.realm);
    assert!(
        review
            .intent
            .installation
            .check_credential_realm(&meerkat_core::RealmId::parse("work").unwrap())
            .is_ok()
    );
    assert_eq!(
        review
            .intent
            .installation
            .check_credential_realm(&owner.native.realm),
        Err(CollectorSetupError::Denied)
    );
    let decision = decision(&review);
    let key = owner.collector_decision_key(review.id()).unwrap();
    assert_eq!(key.realm, owner.native.realm.to_string());
    let native_id = EvidenceId::new(&key.id).unwrap();
    assert!(
        owner
            .source
            .grants()
            .issued_reference(&native_id)
            .unwrap()
            .is_none()
    );
    let saved = owner.retain_collector_decision(&decision).await.unwrap();
    assert!(
        owner
            .source
            .grants()
            .issued_reference(&native_id)
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        owner.decode_collector_decision(&saved).unwrap().issued,
        ParentIssuance::Pending
    ));
    let issued = owner
        .issue_collector_decision(&originals, saved, &|| true)
        .await
        .unwrap();
    let decision = owner.decode_collector_decision(&issued).unwrap();
    let record = owner.collector_record(&issued, &decision).unwrap();
    assert_eq!(
        record.association.ingress_namespace.realm,
        owner.native.realm
    );
    assert_ne!(
        record.installation.realm(),
        &record.association.ingress_namespace.realm
    );
    assert_eq!(record.association.logical_executor, review.intent.collector);
    assert_eq!(record.lineage.len(), 1);
    assert_eq!(record.lineage[0].root_authority, owner.operator);
    assert_eq!(
        record.review.as_ref().unwrap().proposed_action.body,
        Some(review.intent.review_body())
    );
    assert_eq!(
        originals
            .persist_collector_setup(record.clone())
            .await
            .unwrap(),
        *review.id()
    );
    assert_eq!(
        originals
            .persist_collector_setup(record.clone())
            .await
            .unwrap(),
        *review.id()
    );
    use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, RetainedCustody};
    assert!(matches!(
        owner
            .source
            .retained_custody(
                &record.association.ingress_namespace,
                &record.association.original_work
            )
            .unwrap(),
        RetainedCustody::NotRetained
    ));
    drop(originals);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn changed_intent_cannot_reuse_review_or_native_issuance_id() {
    let (_directory, _, owner, originals) = fresh().await;
    let review = review(&owner);
    let original = decision(&review);
    owner.retain_collector_decision(&original).await.unwrap();
    let mut changed = original.clone();
    changed.intent.collector = crate::installed_host::tests::principal("other-collector");
    assert!(matches!(
        owner.retain_collector_decision(&changed).await,
        Err(CollectorSetupError::Denied)
    ));
    let key = owner.collector_decision_key(review.id()).unwrap();
    assert!(
        owner
            .source
            .grants()
            .issued_reference(&EvidenceId::new(key.id).unwrap())
            .unwrap()
            .is_none()
    );
    drop(originals);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn presenter_revocation_after_intent_commit_cannot_issue_but_exact_retry_can_reconcile() {
    let (_directory, _, owner, originals) = fresh().await;
    let review = review(&owner);
    let saved = owner
        .retain_collector_decision(&decision(&review))
        .await
        .unwrap();
    let id = EvidenceId::new(&saved.key.id).unwrap();
    assert!(matches!(
        owner
            .issue_collector_decision(&originals, saved.clone(), &|| false)
            .await,
        Err(CollectorSetupError::Denied)
    ));
    assert!(
        owner
            .source
            .grants()
            .issued_reference(&id)
            .unwrap()
            .is_none()
    );
    // The caller's exact durable locator survives an acknowledgement gap. No
    // newly minted review, source or issuance identity is needed for recovery.
    let retained = owner.originals.get(&saved.key).await.unwrap().unwrap();
    let issued = owner
        .issue_collector_decision(&originals, retained, &|| true)
        .await
        .unwrap();
    let first = owner
        .source
        .grants()
        .issued_reference(&id)
        .unwrap()
        .unwrap();
    let again = owner
        .issue_collector_decision(&originals, issued, &|| true)
        .await
        .unwrap();
    assert_eq!(
        owner.source.grants().issued_reference(&id).unwrap(),
        Some(first)
    );
    assert!(matches!(
        owner.decode_collector_decision(&again).unwrap().issued,
        ParentIssuance::Issued { .. }
    ));
    drop(originals);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn native_issuance_ack_gap_reconciles_existing_exact_reference_after_cold_reopen() {
    let (_directory, selection, owner, originals) = fresh().await;
    let review = review(&owner);
    let decision = decision(&review);
    let saved = owner.retain_collector_decision(&decision).await.unwrap();
    let native_id = EvidenceId::new(&saved.key.id).unwrap();
    // Actual native commit followed by a deliberately absent Toolkit outcome:
    // the persisted decision still says Pending, as after an acknowledgement gap.
    let first = owner
        .source
        .grants()
        .issue_root(
            &owner.operator,
            native_id.clone(),
            decision.intent.collector.clone(),
            None,
            decision.intent.restrictions.clone(),
        )
        .unwrap();
    let id = review.id().clone();
    drop(review);
    drop(originals);
    owner.finish().await.unwrap();
    let (owner, originals) = selected(selection, OperatorOwnerStartup::Restore).await;
    let saved = owner
        .originals
        .get(&owner.collector_decision_key(&id).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        owner.decode_collector_decision(&saved).unwrap().issued,
        ParentIssuance::Pending
    ));
    let issued = owner
        .issue_collector_decision(&originals, saved, &|| true)
        .await
        .unwrap();
    assert_eq!(
        owner.source.grants().issued_reference(&native_id).unwrap(),
        Some(first)
    );
    let record = owner
        .collector_record(&issued, &owner.decode_collector_decision(&issued).unwrap())
        .unwrap();
    assert_eq!(originals.persist_collector_setup(record).await.unwrap(), id);
    drop(originals);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn same_id_with_another_native_grantee_is_not_reinterpreted() {
    let (_directory, _, owner, originals) = fresh().await;
    let review = review(&owner);
    let decision = decision(&review);
    let saved = owner.retain_collector_decision(&decision).await.unwrap();
    owner
        .source
        .grants()
        .issue_root(
            &owner.operator,
            EvidenceId::new(&saved.key.id).unwrap(),
            crate::installed_host::tests::principal("unrelated-agent"),
            None,
            decision.intent.restrictions.clone(),
        )
        .unwrap();
    assert!(matches!(
        owner
            .issue_collector_decision(&originals, saved, &|| true)
            .await,
        Err(CollectorSetupError::Denied)
    ));
    drop(originals);
    owner.finish().await.unwrap();
}

#[tokio::test]
async fn principal_binding_from_another_installed_owner_is_refused_before_review() {
    let (_a, _, a, a_originals) = fresh().await;
    let (_b, _, b, b_originals) = fresh().await;
    let selected = a
        .bind_collector_principal(
            crate::installed_host::tests::principal("source-collector"),
            AudienceRef::Principal {
                principal: a.operator.clone(),
            },
        )
        .unwrap();
    assert!(matches!(
        b.check_binding(&selected.source, &selected.lifetime),
        Err(CollectorSetupError::Denied)
    ));
    drop(a_originals);
    drop(b_originals);
    a.finish().await.unwrap();
    b.finish().await.unwrap();
}

mod pause_store;

#[tokio::test]
async fn cancel_before_enrollment_survives_cold_reopen_for_pending_and_issued_intents() {
    for issue in [false, true] {
        let (_directory, selection, owner, originals) = fresh().await;
        let machine = Arc::new(crate::original_ingress::tests::machine(&owner.source));
        originals.bind_machine(machine.clone()).unwrap();
        let review = review(&owner);
        let mut saved = owner
            .retain_collector_decision(&decision(&review))
            .await
            .unwrap();
        if issue {
            saved = owner
                .issue_collector_decision(&originals, saved, &|| true)
                .await
                .unwrap();
        }
        let id = review.id().clone();
        let native_id = EvidenceId::new(&saved.key.id).unwrap();
        owner
            .revoke_collector_enrollment(&originals, &id)
            .await
            .unwrap();
        let current = owner.originals.get(&saved.key).await.unwrap().unwrap();
        let cancelled = owner.decode_collector_decision(&current).unwrap();
        assert!(cancelled.cancelled && cancelled.cleanup.is_some());
        assert!(matches!(
            owner
                .issue_collector_decision(&originals, saved, &|| true)
                .await,
            Err(CollectorSetupError::SetupRevoked)
        ));
        if !issue {
            assert!(
                owner
                    .source
                    .grants()
                    .issued_reference(&native_id)
                    .unwrap()
                    .is_none()
            );
        }
        drop(review);
        drop(originals);
        drop(machine);
        owner.finish().await.unwrap();
        let (owner, originals) = selected(selection, OperatorOwnerStartup::Restore).await;
        assert!(matches!(
            originals.collector_setup_not_revoked(&id).await,
            Err(CollectorSetupError::SetupRevoked)
        ));
        let saved = owner
            .originals
            .get(&owner.collector_decision_key(&id).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            owner
                .issue_collector_decision(&originals, saved, &|| true)
                .await,
            Err(CollectorSetupError::SetupRevoked)
        ));
        drop(originals);
        owner.finish().await.unwrap();
    }
}

#[tokio::test]
async fn cancel_prepared_review_before_approval_persistence_survives_cold_reopen() {
    let (_directory, selection, owner, originals) = fresh().await;
    let review = review(&owner);
    let approved = decision(&review);
    let id = review.id().clone();
    let key = owner.collector_decision_key(&id).unwrap();
    let native_id = EvidenceId::new(&key.id).unwrap();
    assert!(owner.originals.get(&key).await.unwrap().is_none());
    owner
        .revoke_collector_enrollment(&originals, &id)
        .await
        .unwrap();
    // No fabricated approved intent or native grant is created by cancellation.
    assert!(owner.originals.get(&key).await.unwrap().is_none());
    assert!(
        owner
            .source
            .grants()
            .issued_reference(&native_id)
            .unwrap()
            .is_none()
    );
    // A delayed approval persistence cannot issue after the tombstone exists.
    let saved = owner.retain_collector_decision(&approved).await.unwrap();
    assert!(matches!(
        owner
            .issue_collector_decision(&originals, saved, &|| true)
            .await,
        Err(CollectorSetupError::SetupRevoked)
    ));
    drop(review);
    drop(originals);
    owner.finish().await.unwrap();
    let (owner, originals) = selected(selection, OperatorOwnerStartup::Restore).await;
    let saved = owner.originals.get(&key).await.unwrap().unwrap();
    assert!(matches!(
        owner
            .issue_collector_decision(&originals, saved, &|| true)
            .await,
        Err(CollectorSetupError::SetupRevoked)
    ));
    owner
        .revoke_collector_enrollment(&originals, &id)
        .await
        .unwrap();
    assert!(
        owner
            .source
            .grants()
            .issued_reference(&native_id)
            .unwrap()
            .is_none()
    );
    drop(originals);
    owner.finish().await.unwrap();
}
