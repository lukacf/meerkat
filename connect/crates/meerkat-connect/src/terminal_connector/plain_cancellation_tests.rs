//! Inert controls for the production finalization sequence. The barriers replace
//! native effects, not the sequencing under test; no account or grant is changed.
use super::*;
use std::cell::Cell;
use meerkat::governed_realm::stock::issuance::StockGrantOutcome;
use meerkat_core::mcp_config::McpRealmPersistError;

fn selected() -> StockGrantOutcome {
    let realm = meerkat_core::RealmId::parse("stop-fixture").unwrap();
    let principal = stock_agent_principal(&realm, "reader").unwrap();
    StockGrantOutcome {
        issued:serde_json::from_value(json!({
            "root_authority":principal,"authority_namespace":"fixture-authority",
            "authority_generation":1,"authority_incarnation":"6f75f457-4232-4e12-8426-508d49ece197",
            "grant_id":"fixture-grant","issued_revision":1,
        })).unwrap(),
        superseded:None, cleanup:Vec::new(),
    }
}

fn observed(outcome: FinalizationOutcome, stopped: Option<secrets::Stop>) -> FinalizedInstallation {
    let root = tempfile::tempdir().unwrap();
    let storage = crate::RealmOptions { context_root:root.path().to_owned(),
        state_root:Some(root.path().join("state")), user_config_root:Some(root.path().join("home")),
    }.resolve("stop-fixture").unwrap();
    FinalizedInstallation::new_with_stop(FinalizationTarget {
        realm:storage.locator, server:"news".into(), agent:"reader".into(),
        job_id:"fixture-job".into(), session:SessionId::new(),
    }, outcome, stopped)
}

fn check_observation(mut result: FinalizedInstallation, persistence: &str, grant: &str,
    stop: Option<&str>, uncertain: bool) {
    let facts = result.notice();
    assert_eq!(facts["facts"]["persistence"], persistence);
    assert_eq!(facts["facts"]["grant"]["state"], grant);
    assert_eq!(facts["stop"], json!(stop));
    assert_eq!(result.stopped_during_finalization(), stop.is_some());
    // The one continuation must already contain the same stop and native facts.
    let input = result.take_continuation_notice().unwrap().into_input();
    let meerkat_runtime::Input::ExternalEvent(event) = input else { panic!("expected observation"); };
    let input_facts: Value = serde_json::from_str(event.payload["body"].as_str().unwrap()).unwrap();
    assert_eq!(input_facts, facts);
    let directory = tempfile::tempdir().unwrap();
    result.retain(directory.path(), 1).unwrap();
    let retained: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("standalone-finalization-1.txt")).unwrap()).unwrap();
    assert_eq!(retained["observation"], facts);
    let mut output = Vec::new();
    let displayed = result.display_for_continuation(&mut output);
    assert_eq!(displayed.is_err(), stop == Some("listener_failed"));
    let text = String::from_utf8(output).unwrap();
    assert!(!text.contains("PRIVATE_DIAGNOSTIC"));
    assert!(!retained.to_string().contains("PRIVATE_DIAGNOSTIC"));
    let action = FinalizedAction { outcome:ActionOutcome::Installation(Arc::new(result)), continuation:None };
    assert_eq!(FinalizedAction::finish_invocation(&[action], displayed).is_err(),
        uncertain || stop == Some("listener_failed"));
}

#[tokio::test]
async fn ready_stop_prevents_public_configuration_and_reused_grant_entry() {
    for existing in [None, Some(7)] {
        for listener_failed in [false, true] {
            let interrupt = std::future::ready(if listener_failed {
                Err(io::Error::other("PRIVATE_DIAGNOSTIC"))
            } else { Ok(()) });
            tokio::pin!(interrupt);
            let mut stopped = None;
            let writes = Cell::new(0);
            let grants = Cell::new(0);
            let outcome = settle_public_effects(interrupt.as_mut(), &mut stopped, existing,
                || async { writes.set(writes.get() + 1); Ok(7) },
                |_| async { grants.set(grants.get() + 1); Ok(selected()) },
            ).await;
            assert_eq!((writes.get(), grants.get()), (0, 0));
            assert!(matches!((&outcome, existing),
                (FinalizationOutcome::BeforeAdministrationStopped, None)
                | (FinalizationOutcome::PublicStoppedBeforeGrant { generation:7, reused:true }, Some(7))));
            check_observation(observed(outcome, stopped),
                if existing.is_some() { "already_saved" } else { "not_entered" },
                "not_requested", Some(if listener_failed { "listener_failed" } else { "cancelled" }), false);
        }
    }
}

#[tokio::test]
async fn cancellation_during_public_write_joins_result_without_starting_grant() {
    for result in [Ok(7), Err(McpRealmPersistError::ServerExists("PRIVATE_DIAGNOSTIC".into())),
        Err(McpRealmPersistError::OutcomeUnknown(meerkat_core::config_runtime::ConfigRuntimeError::Io(
            io::Error::other("PRIVATE_DIAGNOSTIC"))))] {
        let uncertain = result.as_ref().is_err_and(public_persistence_may_have_changed);
        let expected = if result.is_ok() { "saved" } else if uncertain { "uncertain" } else { "refused" };
        let (cancel, signal) = oneshot::channel();
        let interrupt = async { signal.await.unwrap(); Ok(()) };
        tokio::pin!(interrupt);
        let mut stopped = None;
        let (entered, started) = oneshot::channel();
        let (release, finish) = oneshot::channel();
        let grants = Cell::new(0);
        let mut sequence = Box::pin(settle_public_effects(interrupt.as_mut(), &mut stopped, None,
            || async { entered.send(()).unwrap(); finish.await.unwrap() },
            |_| async { grants.set(grants.get() + 1); Ok(selected()) }));
        tokio::select! {
            _ = started => {},
            _ = sequence.as_mut() => panic!("configuration did not reach its barrier"),
        }
        cancel.send(()).unwrap();
        assert!(sequence.as_mut().now_or_never().is_none(), "entered write must stay owned");
        release.send(result).unwrap();
        let outcome = sequence.await;
        assert_eq!(grants.get(), 0);
        check_observation(observed(outcome, stopped), expected, "not_requested", Some("cancelled"), uncertain);
    }
}

#[tokio::test]
async fn ready_stop_after_public_write_prevents_grant_without_repolling_signal() {
    let (cancel, signal) = oneshot::channel();
    let interrupt = async { signal.await.unwrap(); Ok(()) };
    tokio::pin!(interrupt);
    let mut stopped = None;
    let grants = Cell::new(0);
    let outcome = settle_public_effects(interrupt.as_mut(), &mut stopped, None,
        || async { cancel.send(()).unwrap(); Ok(7) },
        |_| async { grants.set(grants.get() + 1); Ok(selected()) },
    ).await;
    assert_eq!(grants.get(), 0);
    check_observation(observed(outcome, stopped), "saved", "not_requested", Some("cancelled"), false);
}

#[tokio::test]
async fn public_listener_failure_joins_entered_write_and_exits_nonzero() {
    let (fail, signal) = oneshot::channel();
    let interrupt = async { signal.await.unwrap(); Err(io::Error::other("PRIVATE_DIAGNOSTIC")) };
    tokio::pin!(interrupt);
    let mut stopped = None;
    let (entered, started) = oneshot::channel();
    let (release, finish) = oneshot::channel();
    let grants = Cell::new(0);
    let mut sequence = Box::pin(settle_public_effects(interrupt.as_mut(), &mut stopped, None,
        || async { entered.send(()).unwrap(); finish.await.unwrap() },
        |_| async { grants.set(grants.get() + 1); Ok(selected()) }));
    tokio::select! {
        _ = started => {},
        _ = sequence.as_mut() => panic!("configuration did not reach its barrier"),
    }
    fail.send(()).unwrap();
    assert!(sequence.as_mut().now_or_never().is_none(), "listener failure cannot abandon entered write");
    release.send(Ok(7)).unwrap();
    let outcome = sequence.await;
    assert_eq!(grants.get(), 0);
    check_observation(observed(outcome, stopped), "saved", "not_requested", Some("listener_failed"), false);
}

#[tokio::test]
async fn cancellation_during_public_grant_joins_and_preserves_native_outcome() {
    for existing in [None, Some(7)] {
        for uncertain in [false, true] {
            let (cancel, signal) = oneshot::channel();
            let interrupt = async { signal.await.unwrap(); Ok(()) };
            tokio::pin!(interrupt);
            let mut stopped = None;
            let (entered, started) = oneshot::channel();
            let (release, finish) = oneshot::channel();
            let writes = Cell::new(0);
            let mut sequence = Box::pin(settle_public_effects(interrupt.as_mut(), &mut stopped, existing,
                || async { writes.set(writes.get() + 1); Ok(7) },
                |generation| async move {
                    assert_eq!(generation, 7);
                    entered.send(()).unwrap();
                    finish.await.unwrap()
                }));
            tokio::select! {
                _ = started => {},
                _ = sequence.as_mut() => panic!("grant did not reach its barrier"),
            }
            cancel.send(()).unwrap();
            assert!(sequence.as_mut().now_or_never().is_none(), "entered grant must stay owned");
            release.send(if uncertain { Err(StockGrantError::CommandUncertain) } else { Ok(selected()) }).unwrap();
            let outcome = sequence.await;
            assert_eq!(writes.get(), usize::from(existing.is_none()));
            check_observation(observed(outcome, stopped),
                if existing.is_some() { "already_saved" } else { "saved" },
                if uncertain { "command_uncertain" } else { "selected" }, Some("cancelled"), uncertain);
        }
    }
}

#[tokio::test]
async fn uninterrupted_public_sequence_preserves_create_and_reuse_outcomes() {
    for existing in [None, Some(7)] {
        let interrupt = std::future::pending::<io::Result<()>>();
        tokio::pin!(interrupt);
        let mut stopped = None;
        let writes = Cell::new(0);
        let grants = Cell::new(0);
        let outcome = settle_public_effects(interrupt.as_mut(), &mut stopped, existing,
            || async { writes.set(writes.get() + 1); Ok(7) },
            |generation| { assert_eq!(generation, 7); async { grants.set(grants.get() + 1); Ok(selected()) } },
        ).await;
        assert_eq!((writes.get(), grants.get()), (usize::from(existing.is_none()), 1));
        check_observation(observed(outcome, stopped),
            if existing.is_some() { "already_saved" } else { "saved" }, "selected", None, false);
    }
}

#[tokio::test]
async fn connector_ready_stop_prevents_native_composite_entry() {
    let interrupt = std::future::ready(Ok(()));
    tokio::pin!(interrupt);
    let mut stopped = None;
    let entered = Cell::new(false);
    let outcome = settle_connector_effect(interrupt.as_mut(), &mut stopped, || async {
        entered.set(true);
        ReviewedConnectorFinalization::Persisted { generation:7, grant:Ok(selected()) }
    }).await;
    assert!(!entered.get());
    assert!(matches!(outcome, FinalizationOutcome::BeforeAdministrationStopped));
    check_observation(observed(outcome, stopped), "not_entered", "not_requested", Some("cancelled"), false);
}

#[tokio::test]
async fn connector_cancellation_joins_the_entered_native_composite() {
    let (cancel, signal) = oneshot::channel();
    let interrupt = async { signal.await.unwrap(); Ok(()) };
    tokio::pin!(interrupt);
    let mut stopped = None;
    let (entered, started) = oneshot::channel();
    let (release, finish) = oneshot::channel();
    let mut sequence = Box::pin(settle_connector_effect(interrupt.as_mut(), &mut stopped, || async {
        entered.send(()).unwrap();
        finish.await.unwrap()
    }));
    tokio::select! {
        _ = started => {},
        _ = sequence.as_mut() => panic!("native composite did not reach its barrier"),
    }
    cancel.send(()).unwrap();
    assert!(sequence.as_mut().now_or_never().is_none(), "entered composite must stay owned");
    release.send(ReviewedConnectorFinalization::PersistUncertain { reason:"PRIVATE_DIAGNOSTIC".into() }).unwrap();
    let outcome = sequence.await;
    check_observation(observed(outcome, stopped), "uncertain", "not_requested", Some("cancelled"), true);
}
