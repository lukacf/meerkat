//! Inert native outcome fixtures exercise the same exit fold used by main.
//! These tests do not execute installation, grant or cleanup effects.
use super::*;
use meerkat::governed_realm::stock::issuance::{CleanupAttempt, StockGrantOutcome};

fn action(outcome: FinalizationOutcome) -> FinalizedAction {
    let root = tempfile::tempdir().unwrap();
    let storage = crate::RealmOptions {
        context_root: root.path().to_owned(),
        state_root: Some(root.path().join("state")),
        user_config_root: Some(root.path().join("home")),
    }.resolve("exit-fixture").unwrap();
    FinalizedAction {
        outcome: ActionOutcome::Installation(Arc::new(FinalizedInstallation::new(
            FinalizationTarget { realm:storage.locator, server:"mail".into(),
                agent:"fixture".into(), job_id:"reviewed-job".into(), session:SessionId::new() },
            outcome,
        ))),
        continuation: None,
    }
}

fn reference(id: &str) -> meerkat_authorization_contracts::work_association::GrantLineageRef {
    let realm = meerkat_core::RealmId::parse("exit-fixture").unwrap();
    let principal = stock_agent_principal(&realm, "fixture").unwrap();
    serde_json::from_value(json!({
        "root_authority":principal,"authority_namespace":"fixture-authority",
        "authority_generation":1,"authority_incarnation":"6f75f457-4232-4e12-8426-508d49ece197",
        "grant_id":id,"issued_revision":1,
    })).unwrap()
}

fn selected(cleanup: Vec<CleanupAttempt>) -> FinalizedAction {
    action(FinalizationOutcome::PublicPersisted { generation:7,
        grant:Ok(StockGrantOutcome { issued:reference("current"), superseded:None, cleanup }) })
}

#[test]
fn ordinary_quit_keeps_uncertain_finalization_nonzero_after_later_success() {
    let completed = vec![
        action(FinalizationOutcome::Connector(ReviewedConnectorFinalization::PersistUncertain {
            reason:"PRIVATE_WRITE_DIAGNOSTIC".into(),
        })),
        selected(Vec::new()),
    ];
    // /quit and EOF end the conversation with Ok; the invocation fold must
    // retain the older failure after the next round's clean shutdown.
    let error = FinalizedAction::finish_invocation(&completed, Ok(())).unwrap_err().to_string();
    assert!(error.contains("unresolved or uncertain"));
    assert!(!error.contains("PRIVATE_WRITE_DIAGNOSTIC"));
}

#[test]
fn interrupt_after_entered_finalization_preserves_uncertain_grant() {
    let completed = [action(FinalizationOutcome::PublicPersisted {
        generation:7, grant:Err(StockGrantError::SelectionUncertain {
            issued:reference("current"), previous:None, reason:"PRIVATE_SELECTION_DIAGNOSTIC".into(),
        }),
    })];
    // The retained interrupt returns Ok only after the entered finalizer
    // settles. That clean stop cannot turn its unknown result into success.
    let error = FinalizedAction::finish_invocation(&completed, Ok(())).unwrap_err().to_string();
    assert!(error.contains("do not repeat it automatically"));
    assert!(!error.contains("PRIVATE_SELECTION_DIAGNOSTIC"));
}

#[test]
fn failed_exit_keeps_primary_error_without_claiming_artifact_retention() {
    let completed = [action(FinalizationOutcome::PublicPersisted {
        generation:7, grant:Err(StockGrantError::IssuanceUncertain {
            id:serde_json::from_value(json!("possibly-issued")).unwrap(),
        }),
    })];
    let error = FinalizedAction::finish_invocation(&completed,
        Err(unavailable("The observation could not be retained."))).unwrap_err();
    assert!(error.to_string().contains("observation could not be retained"));
    assert!(!matches!(error, ConnectError::Cleanup { .. }));
    assert!(!error.to_string().contains("is retained"));
}

#[test]
fn exit_distinguishes_unknown_cleanup_and_unselected_grants_from_known_no_effects() {
    for outcome in [RevocationOutcome::Revoked, RevocationOutcome::ControllerInUse,
        RevocationOutcome::Deferred, RevocationOutcome::ReferencesUnknown,
        RevocationOutcome::CustodyUnavailable, RevocationOutcome::Refused, RevocationOutcome::Uncertain] {
        let completed = [selected(vec![CleanupAttempt {
            agent:"fixture".into(), grant:reference("older"), outcome,
        }])];
        assert_eq!(FinalizedAction::finish_invocation(&completed, Ok(())).is_err(),
            outcome == RevocationOutcome::Uncertain, "{outcome:?}");
    }
    let completed = [action(FinalizationOutcome::PublicPersisted {
        generation:7, grant:Err(StockGrantError::NotSelected {
            issued:reference("current"), reason:"PRIVATE_NOT_SELECTED".into(),
        }),
    })];
    assert!(FinalizedAction::finish_invocation(&completed, Ok(())).is_err());
    assert!(FinalizedAction::finish_invocation(&[], Ok(())).is_ok(),
        "cancellation before any finalizer entered remains a clean stop");
    assert!(FinalizedAction::finish_invocation(&[selected(Vec::new())], Ok(())).is_ok());
    let refused = action(FinalizationOutcome::BeforeAdministrationRefused(
        unavailable("No native administration entered.")));
    assert!(FinalizedAction::finish_invocation(&[refused], Ok(())).is_ok());
}
