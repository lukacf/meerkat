//! Installed-host SDK consumption of the existing native/canonical fixture.
//! No second runtime is opened. The facade attachment is test-only; production
//! gateway bootstrap and a standalone realm CLI are not qualified here.
use super::*;
use crate::host_gateway::{LocalOperatorRecoveryPolicy, LocalRecoveryPageRead, ToolkitGatewayHost};
use crate::private_host::PrivateHostClosed;

fn host<S>(
    fixture: &ContinuationFixture<S>,
    selected: bool,
) -> (tokio::sync::watch::Sender<bool>, ToolkitGatewayHost) {
    let (send, receive) = tokio::sync::watch::channel(false);
    let host = ToolkitGatewayHost::fixture_local_recovery(
        fixture.lifetime().clone(),
        fixture.source.clone(),
        fixture.machine.clone(),
        PrivateHostClosed(receive),
        selected,
    );
    (send, host)
}
fn first_id(page: &crate::host_gateway::LocalServiceRecoveryPage) -> String {
    let projection = page.projection();
    assert_eq!(projection["rows"].as_array().unwrap().len(), 1);
    assert_eq!(projection["rows"][0]["can_recover"], true);
    projection["rows"][0]["id"].as_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_local_recovery_requires_opt_in_and_exact_owner_then_revokes_once() {
    // Both actual fixtures use the selected factory mob identity. Retain a
    // genuine foreign page, then fully retire its runtime before opening the
    // current owner; concurrent identical supervisor names are not isolated.
    let other = ContinuationFixture::for_service_input().await;
    let foreign_page = AssertUnwindSafe(Box::pin(async {
        let task = persistence_task(&other).await;
        issue_before_publication(&other, task).await;
        let (pipe, selected) = host(&other, true);
        let view = selected.local_service_recovery().unwrap().unwrap();
        let page = view.page(None).await.unwrap();
        let id = first_id(&page);
        drop(pipe);
        (page, id, journal(&other))
    })).catch_unwind().await;
    let cleanup = other.shutdown().await;
    assert!(cleanup.is_ok(), "foreign fixture join: {cleanup:?}");
    let (foreign_page, foreign_id, foreign_journal) = match foreign_page {
        Ok(page) => page,
        Err(panic) => std::panic::resume_unwind(panic),
    };
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let task = persistence_task(&fixture).await;
        let (grant, parent, _) = issue_before_publication(&fixture, task).await;
        let (_, child) = fixture
            .source
            .service_compensation_child(&grant, &parent)
            .await
            .unwrap()
            .unwrap();
        let before = journal(&fixture);
        let (disabled_pipe, disabled) = host(&fixture, false);
        assert!(!disabled.local_operator_recovery_selected());
        assert!(disabled.local_service_recovery().unwrap().is_none());
        assert!(
            disabled
                .with_local_operator_recovery(
                    LocalOperatorRecoveryPolicy::UnpublishedServiceApprovals
                )
                .is_err(),
            "late selection cannot change an attached host"
        );
        let (pipe, selected) = host(&fixture, true);
        assert!(selected.local_operator_recovery_selected());
        let view = selected.local_service_recovery().unwrap().unwrap();
        let page = view.page(None).await.unwrap();
        let id = first_id(&page);
        assert!(!page.has_next());
        assert!(matches!(
            view.page(Some((&foreign_page, LocalRecoveryPageRead::Refresh))).await,
            Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::WrongOwner)
        ));
        assert!(matches!(
            view.revoke(&foreign_page, &foreign_id).await,
            Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::WrongOwner)
        ));
        assert_eq!(journal(&other), foreign_journal, "foreign grants are unchanged");
        assert!(view.revoke(&page, "unretained-row").await.is_err());
        assert_eq!(
            journal(&fixture),
            before,
            "selection/foreign handles changed no grant"
        );
        let result = view.revoke(&page, &id).await.unwrap();
        assert!(!result.uncertain());
        let projection = result.projection();
        assert_eq!(projection["parent"], "revoked");
        assert_eq!(projection["child"], "revoked");
        assert_eq!(projection["policy_changed"], false);
        assert_eq!(projection["policy_observed_at_offer"], "not_requested");
        assert_eq!(
            revoked(&journal(&fixture)[before.len()..]),
            vec![
                parent.grant_id.as_str().to_owned(),
                child.grant_id.as_str().to_owned()
            ]
        );
        assert!(issued(&journal(&fixture)[before.len()..]).is_empty());
        assert!(
            view.revoke(&page, &id).await.is_err(),
            "an offer is consumed once"
        );
        let refreshed = view
            .page(Some((&page, LocalRecoveryPageRead::Refresh)))
            .await
            .unwrap();
        let observed = journal(&fixture);
        pipe.send(true).unwrap();
        assert!(view.page(None).await.is_err());
        assert!(view.revoke(&refreshed, &id).await.is_err());
        assert!(selected.local_service_recovery().is_err());
        assert_eq!(journal(&fixture), observed, "closed host changed no grant");
        drop(disabled_pipe);
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "current fixture join: {cleanup:?}");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_local_recovery_rechecks_publication_before_native_entry() {
    let fixture = ContinuationFixture::for_service_input().await;
    let result = AssertUnwindSafe(Box::pin(async {
        let task = persistence_task(&fixture).await;
        let (grant, _, evidence) = issue_before_publication(&fixture, task.clone()).await;
        let (pipe, selected) = host(&fixture, true);
        let view = selected.local_service_recovery().unwrap().unwrap();
        let page = view.page(None).await.unwrap();
        let id = first_id(&page);
        task.retain_approved(grant.clone(), evidence, &|| Ok(()))
            .await
            .unwrap();
        let before = journal(&fixture);
        assert!(view.revoke(&page, &id).await.is_err());
        assert_eq!(journal(&fixture), before);
        fixture
            .source
            .require_service_grant_issued(&grant)
            .await
            .unwrap();
        let refreshed = view
            .page(Some((&page, LocalRecoveryPageRead::Refresh)))
            .await
            .unwrap();
        let projection = refreshed.projection();
        assert_eq!(projection["rows"][0]["state"], "publication_present");
        assert_eq!(projection["rows"][0]["can_recover"], false);
        assert!(view.revoke(&refreshed, &id).await.is_err());
        drop(pipe);
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installed_local_recovery_refuses_a_configured_remote_operator() {
    use crate::private_originals::console_ingress::auth_fixture::selected_human;
    let fixture = ContinuationFixture::for_console_operator_input(selected_human()).await;
    let result = AssertUnwindSafe(Box::pin(async {
        assert_eq!(
            fixture
                .lifetime()
                .selected_policy()
                .native_host
                .as_ref()
                .unwrap()
                .remote_operator
                .as_ref(),
            Some(&selected_human())
        );
        let before = journal(&fixture);
        let (pipe, selected) = host(&fixture, true);
        assert!(selected.local_operator_recovery_selected());
        assert!(
            selected.local_service_recovery().is_err(),
            "local OS authority cannot select a remote issuer"
        );
        assert_eq!(journal(&fixture), before);
        drop(pipe);
    }))
    .catch_unwind()
    .await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
