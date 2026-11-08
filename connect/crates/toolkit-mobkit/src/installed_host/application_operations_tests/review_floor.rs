//! Real source/grant-backed preparation, not a mapping-only tier test.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
struct Floor {
    calls: AtomicUsize,
    refusal: bool,
}
impl InstalledOperationReviewPolicy for Floor {
    fn review_floor(
        &self,
        c: InstalledOperationReviewContext<'_>,
    ) -> Result<OperationReviewTier, OperationAuthorizationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(c.selected().native_host.as_ref(), Some(c.native()));
        assert_eq!(
            c.association().candidate().ingress_namespace.realm,
            c.native().realm
        );
        assert!(
            matches!(c.target(), meerkat_core::ToolOperationTarget::RegisteredToolBundleOperation {
            bundle, operation, .. } if bundle == crate::FIND_AND_INSTALL_BUNDLE && operation == crate::FIND_AND_INSTALL)
        );
        assert_eq!(c.allowance().review_tier, OperationReviewTier::R1);
        if self.refusal {
            Err(OperationAuthorizationError::Unavailable)
        } else {
            Ok(OperationReviewTier::R3)
        }
    }
}
fn floor(refusal: bool) -> Arc<Floor> {
    Arc::new(Floor {
        calls: AtomicUsize::new(0),
        refusal,
    })
}

#[tokio::test]
async fn installed_review_floor_reaches_native_preparation_without_widening_permission() {
    let ordinary = Fixture::with_targeted_setup().await;
    let call = SetupBundleCall::new(crate::FIND_AND_INSTALL_BUNDLE, crate::FIND_AND_INSTALL);
    let binding = call.binding(&ordinary);
    assert_eq!(
        ordinary
            .context
            .authorization()
            .prepare(&binding)
            .unwrap()
            .review_tier(),
        OperationReviewTier::R1
    );
    let selected = floor(false);
    let f = Fixture::with_review_policy(
        setup_request_bounds,
        true,
        false,
        true,
        Some(selected.clone()),
    )
    .await;
    let binding = call.binding(&f);
    let authorized = f.context.authorization().prepare(&binding).unwrap();
    authorized.check_current(&binding).unwrap();
    assert_eq!(authorized.review_tier(), OperationReviewTier::R3);
    assert!(selected.calls.load(Ordering::SeqCst) > 0);
    // A review requirement does not invoke the effect or broaden a sibling's
    // missing native target grant, even when its source and owner are current.
    let report = SetupBundleCall::new(
        meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE,
        meerkat_toolkit::setup::SETUP_REPORT,
    );
    let before = selected.calls.load(Ordering::SeqCst);
    assert!(
        f.context
            .authorization()
            .prepare(&report.binding(&f))
            .is_err()
    );
    assert_eq!(
        selected.calls.load(Ordering::SeqCst),
        before,
        "local refusal precedes floor selection"
    );
    let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } =
        &f.association.candidate().authority_basis
    else {
        panic!("targeted")
    };
    let machine = meerkat_runtime::MeerkatMachine::ephemeral()
        .with_local_grant_authorization(f.source.configuration(
            ScopedInvocation::from_policy(f.application.clone()),
            f.application.clone(),
            None,
        ))
        .unwrap();
    {
        let mut custody = machine.try_controller_grant_mutation().unwrap();
        f.source
            .grants()
            .revoke(
                &f.application.native.grant_administrator,
                targets[0].lineage.last().unwrap(),
                &mut custody,
            )
            .unwrap();
    }
    denied(f.context.authorization().prepare(&binding));
    assert_eq!(call.calls.load(Ordering::SeqCst), 0);
    assert_eq!(report.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn installed_review_floor_preserves_source_and_selected_policy_refusal() {
    let selected = floor(true);
    let f = Fixture::with_review_policy(
        setup_request_bounds,
        true,
        false,
        true,
        Some(selected.clone()),
    )
    .await;
    let call = SetupBundleCall::new(crate::FIND_AND_INSTALL_BUNDLE, crate::FIND_AND_INSTALL);
    operation_unavailable(f.context.authorization().prepare(&call.binding(&f)));
    assert!(selected.calls.load(Ordering::SeqCst) > 0);
    let before = selected.calls.load(Ordering::SeqCst);
    let mut candidate = f.association.candidate().clone();
    candidate.original_work.work = id("unretained-review-original");
    let changed = InputAuthorityAssociation::new(candidate).unwrap();
    let context = f
        .policy
        .work_context(vec![changed].into(), scope(&f.session))
        .unwrap();
    operation_unavailable(context.authorization().prepare(&call.binding(&f)));
    assert_eq!(selected.calls.load(Ordering::SeqCst), before);
    f.lifetime
        .begin_close_activation(HostCloseReason::Shutdown)
        .unwrap();
    denied(f.context.authorization().prepare(&call.binding(&f)));
    assert_eq!(selected.calls.load(Ordering::SeqCst), before);
    assert_eq!(call.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn prepared_review_floor_is_unavailable_until_same_owner_adoption_and_survives_it() {
    let selected = floor(false);
    let f = Fixture::with_targeted_setup().await;
    let prepared = Arc::new(PreparedApplicationPolicy::new(
        f.application.originals.clone(),
    ));
    let configuration = f.source.configuration(
        ScopedInvocation::from_policy(prepared.clone()),
        prepared.clone(),
        None,
    );
    let policy = Arc::new(GrantBackedWorkPolicy::new(
        configuration.grants,
        configuration.invocation_owner,
        configuration.operation_owner,
    ));
    let context = policy
        .work_context(vec![f.association.clone()].into(), scope(&f.session))
        .unwrap();
    let call = SetupBundleCall::new(crate::FIND_AND_INSTALL_BUNDLE, crate::FIND_AND_INSTALL);
    let binding = call.binding(&f);
    operation_unavailable(context.authorization().prepare(&binding));
    assert_eq!(selected.calls.load(Ordering::SeqCst), 0);
    let installed = || InstalledApplicationPolicy {
        operation_review_policy: Some(selected.clone()),
        originals: f.application.originals.clone(),
        lifetime: f.lifetime.clone(),
        native: f.application.native.clone(),
        selected: f.application.selected.clone(),
    };
    prepared.adopt(installed()).unwrap();
    assert_eq!(
        context
            .authorization()
            .prepare(&binding)
            .unwrap()
            .review_tier(),
        OperationReviewTier::R3
    );
    assert!(
        prepared.adopt(installed()).is_err(),
        "the same native policy cannot be replaced"
    );
    assert_eq!(
        context
            .authorization()
            .prepare(&binding)
            .unwrap()
            .review_tier(),
        OperationReviewTier::R3
    );
    assert_eq!(call.calls.load(Ordering::SeqCst), 0);
}
