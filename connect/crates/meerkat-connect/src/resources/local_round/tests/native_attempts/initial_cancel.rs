//! Real router finish receipts qualify preparation reuse only. These
//! controls do not attribute an installed reset failure or retire a live router
//! merely because its registration has been replaced.
use super::*;
use meerkat_mcp::{McpAttemptCancelCause, McpServerCloseOutcome, McpStdioLeaderOutcome};

fn initial_cancel(case: &NativeCase, report: usize) -> &meerkat_mcp::McpAttemptDisposition {
    let report = &case.reports[report];
    assert!(report.is_settled() && report.guards_completed(), "{report:?}");
    let record = report.guards().iter().find(|record| record.stage == McpGuardStage::Cancel)
        .expect("the actual initial guard has a native cancellation completion");
    assert_eq!(record.completion, McpGuardCompletion::Completed, "{record:?}");
    let disposition = record.disposition.as_ref().unwrap();
    assert_eq!(disposition.key, case.probe.requests.lock().unwrap()[0].key);
    assert!(matches!(disposition.open, McpAttemptOpenOutcome::Cancelled { .. }));
    disposition
}

#[tokio::test]
async fn native_unspawned_initial_cancellation_returns_unused_preparation() {
    run("ready", "normal", false, |case| Box::pin(async move {
        let session = case.fixture.register().await;
        case.probe.hold_first.send_replace(true);
        let first = case.route(&session);
        case.add(first, ExternalToolDeltaPhase::Failed).await;
        assert!(case.probe.held.load(Ordering::SeqCst));
        assert!(case.native_pids().is_empty() && case.protocol.pids().is_empty());

        // A new actual registration cannot take the outstanding old guard,
        // even though its native begin has stopped waiting for the late return.
        case.fixture.retire(&session).await;
        let next_session = case.fixture.register().await;
        assert_ne!(session, next_session);
        let blocked = case.route(&next_session);
        case.add(blocked, ExternalToolDeltaPhase::Failed).await;
        let closed = case.close(blocked).await;
        let report = &case.reports[closed];
        assert!(report.is_settled() && report.guards_completed(), "{report:?}");
        assert!(report.guards().iter().any(|record| {
            record.stage == McpGuardStage::Begin
                && record.completion == McpGuardCompletion::Cancelled
                && record.disposition.as_ref().is_some_and(|disposition| {
                    matches!(disposition.open, McpAttemptOpenOutcome::Refused {
                        refusal: meerkat_mcp::McpAttemptRefusal::PriorGuardUnsettled,
                    }) && matches!(disposition.process, McpAttemptProcessEnd::NotSpawned)
                })
        }), "{report:?}");
        // Core groups this member's guards across sessions and refuses before
        // calling the factory while the first begin remains held.
        assert_eq!(case.probe.requests.lock().unwrap().len(), 1);
        assert!(case.native_pids().is_empty());

        case.probe.hold_first.send_replace(false);
        let closed = case.close(first).await;
        let disposition = initial_cancel(case, closed);
        assert!(matches!(disposition.open, McpAttemptOpenOutcome::Cancelled {
            cause: McpAttemptCancelCause::BeginBound,
        }));
        assert!(matches!(disposition.process, McpAttemptProcessEnd::NotSpawned));

        let stale = case.route(&session);
        case.add(stale, ExternalToolDeltaPhase::Failed).await;
        case.close(stale).await;
        assert!(case.native_pids().is_empty(), "restoring preparation does not restore the old session");
        let next = case.route(&next_session);
        case.add(next, ExternalToolDeltaPhase::Applied).await;
        let requests = case.probe.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1].owner.session_id, session);
        assert_eq!(requests[2].owner.session_id, next_session);
        assert_ne!(requests[0].key, requests[2].key);
        drop(requests);
        assert_eq!(case.native_pids().len(), 1);
        assert!(case.protocol.pids().is_empty(), "unused initial descriptor needs no successor offer");
        let closed = case.close(next).await;
        case.assert_settled(closed);
    })).await;
}

#[tokio::test]
async fn native_closed_initial_cancellation_uses_the_existing_successor_offer() {
    run("ready", "initialize_hold", false, |case| Box::pin(async move {
        let session = case.fixture.register().await;
        let first = case.route(&session);
        let config = case.fixture.server().clone();
        let router = case.routers[first].as_mut().unwrap();
        router.stage_add(config).unwrap();
        router.apply_staged().await.unwrap();
        tokio::time::timeout(BOUND, async {
            loop {
                if case.native_pids().iter().any(|pid| case.protocol.output
                    .join(format!("{pid}.native-initializing")).exists()) { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("actual child waits inside native initialization");
        assert_eq!(case.native_pids().len(), 1);
        assert!(case.protocol.pids().is_empty());

        let closed = case.close(first).await;
        let disposition = initial_cancel(case, closed);
        assert!(matches!(disposition.open, McpAttemptOpenOutcome::Cancelled {
            cause: McpAttemptCancelCause::RouterClosed,
        }));
        let McpAttemptProcessEnd::Closed(receipt) = &disposition.process else {
            panic!("native custody must supply the actual child close: {disposition:?}");
        };
        assert!(McpServerCloseOutcome::Stdio(receipt.clone()).is_settled());
        assert!(matches!(&receipt.leader, McpStdioLeaderOutcome::Reaped(status) if status.success()));
        assert!(receipt.sigterm.is_none() && receipt.sigkill.is_none());

        case.fixture.retire(&session).await;
        let next_session = case.fixture.register().await;
        assert_ne!(session, next_session);
        fs::write(case.protocol.output.join("native-mode"), "normal").unwrap();
        let next = case.route(&next_session);
        case.add(next, ExternalToolDeltaPhase::Applied).await;
        assert_eq!(case.native_pids().len(), 2);
        assert_eq!(case.protocol.pids().len(), 1, "a spawned initial child requires the real next-round owner");
        assert_eq!(case.probe.requests.lock().unwrap()[1].owner.session_id, next_session);
        let closed = case.close(next).await;
        case.assert_settled(closed);
    })).await;
}

#[tokio::test]
async fn native_live_initial_guard_blocks_replacement_until_its_actual_close() {
    run("ready", "normal", false, |case| Box::pin(async move {
        let session = case.fixture.register().await;
        let first = case.route(&session);
        case.add(first, ExternalToolDeltaPhase::Applied).await;
        let first_pid = case.native_pids()[0];
        case.fixture.retire(&session).await;
        let next_session = case.fixture.register().await;
        assert_ne!(session, next_session);
        let blocked = case.route(&next_session);
        case.add(blocked, ExternalToolDeltaPhase::Failed).await;
        case.close(blocked).await;
        assert_eq!(case.native_pids(), vec![first_pid]);
        assert!(!case.protocol.output.join(format!("{first_pid}.native-ended")).exists(),
            "the old router remains live after registration replacement");
        assert!(case.protocol.pids().is_empty());

        let closed = case.close(first).await;
        case.assert_settled(closed);
        assert!(case.protocol.output.join(format!("{first_pid}.native-ended")).exists());
        let next = case.route(&next_session);
        case.add(next, ExternalToolDeltaPhase::Applied).await;
        assert_eq!(case.native_pids().len(), 2);
        assert_eq!(case.protocol.pids().len(), 1);
        let closed = case.close(next).await;
        case.assert_settled(closed);
    })).await;
}


async fn initial_cancel_with_changed_resources(spawned: bool) {
    let native = if spawned { "initialize_hold" } else { "normal" };
    run("ready", native, false, move |case| Box::pin(async move {
        let session = case.fixture.register().await;
        case.probe.hold_first.send_replace(!spawned);
        let first = case.route(&session);
        if spawned {
            let config = case.fixture.server().clone();
            let router = case.routers[first].as_mut().unwrap();
            router.stage_add(config).unwrap();
            router.apply_staged().await.unwrap();
            tokio::time::timeout(BOUND, async {
                loop {
                    if case.native_pids().iter().any(|pid| case.protocol.output
                        .join(format!("{pid}.native-initializing")).exists()) { break; }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.expect("actual child waits inside native initialization");
            assert_eq!(case.native_pids().len(), 1);
        } else {
            case.add(first, ExternalToolDeltaPhase::Failed).await;
            assert!(case.probe.held.load(Ordering::SeqCst));
            assert!(case.native_pids().is_empty());
        }
        assert!(case.protocol.pids().is_empty());

        // This manifest-listed leaf is never executed by either fixture child.
        // Change it only after the actual Initial guard owns the preparation.
        let resource = case.protocol.selected_root.join("starter/start.py");
        let original = fs::read(&resource).unwrap();
        let mut changed = original.clone();
        changed[0] ^= 1;
        fs::write(&resource, &changed).unwrap();
        let manifest = fs::read(case.protocol.selected_root.join("resources.json")).unwrap();
        assert!(InstalledResources::verify(case.protocol.selected_root.clone(), &digest(&manifest)).is_err());

        case.probe.hold_first.send_replace(false);
        let closed = case.close(first).await;
        let disposition = initial_cancel(case, closed);
        if spawned {
            assert!(matches!(disposition.open, McpAttemptOpenOutcome::Cancelled {
                cause: McpAttemptCancelCause::RouterClosed,
            }));
            let McpAttemptProcessEnd::Closed(receipt) = &disposition.process else {
                panic!("native custody must supply the actual child close: {disposition:?}");
            };
            assert!(McpServerCloseOutcome::Stdio(receipt.clone()).is_settled());
            assert!(matches!(&receipt.leader, McpStdioLeaderOutcome::Reaped(status) if status.success()));
            assert!(receipt.sigterm.is_none() && receipt.sigkill.is_none());
        } else {
            assert!(matches!(disposition.open, McpAttemptOpenOutcome::Cancelled {
                cause: McpAttemptCancelCause::BeginBound,
            }));
            assert!(matches!(disposition.process, McpAttemptProcessEnd::NotSpawned));
        }

        case.fixture.retire(&session).await;
        let next_session = case.fixture.register().await;
        assert_ne!(session, next_session);
        fs::write(case.protocol.output.join("native-mode"), "normal").unwrap();
        let refused = case.route(&next_session);
        case.add(refused, ExternalToolDeltaPhase::Failed).await;
        case.close(refused).await;
        assert_eq!(case.probe.requests.lock().unwrap().len(), 2);
        assert_eq!(case.probe.requests.lock().unwrap()[1].owner.session_id, next_session);
        assert_eq!(case.native_pids().len(), usize::from(spawned),
            "changed resources refuse the next current begin before another native spawn");
        assert!(case.protocol.pids().is_empty(), "changed resources cannot open a successor offer");

        fs::write(&resource, &original).unwrap();
        let next = case.route(&next_session);
        case.add(next, ExternalToolDeltaPhase::Applied).await;
        let requests = case.probe.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[2].owner.session_id, next_session);
        assert_ne!(requests[0].key, requests[2].key);
        drop(requests);
        assert_eq!(case.native_pids().len(), usize::from(spawned) + 1);
        assert_eq!(case.protocol.pids().len(), usize::from(spawned),
            "only a previously spawned initial child needs the existing successor offer");
        let closed = case.close(next).await;
        case.assert_settled(closed);
        let owner = case.attempts.take_closed_owner().expect("the original owner returns after native close");
        case.protocol.owner = Some(owner);
        let owner = case.protocol.owner.as_ref().unwrap();
        assert!(!owner.poisoned && !owner.has_pending_round());
        assert_eq!(owner.resources.root, case.protocol.selected_root);
    })).await;
}

#[tokio::test]
async fn native_unspawned_initial_cancel_returns_custody_before_resource_revalidation() {
    initial_cancel_with_changed_resources(false).await;
}

#[tokio::test]
async fn native_closed_initial_cancel_returns_custody_before_resource_revalidation() {
    initial_cancel_with_changed_resources(true).await;
}

#[tokio::test]
async fn native_healthy_initial_settle_returns_custody_before_resource_revalidation() {
    run("ready", "normal", false, |case| Box::pin(async move {
        let session = case.fixture.register().await;
        let first = case.route(&session);
        case.add(first, ExternalToolDeltaPhase::Applied).await;
        let first_pid = case.native_pids()[0];
        assert_eq!(case.native_pids(), vec![first_pid]);
        assert!(case.protocol.pids().is_empty());

        // Change only an inert manifest-listed leaf after the actual native
        // Initial child is installed. Neither fixture child executes this file.
        let resource = case.protocol.selected_root.join("starter/start.py");
        let original = fs::read(&resource).unwrap();
        let mut changed = original.clone();
        changed[0] ^= 1;
        fs::write(&resource, &changed).unwrap();
        let manifest = fs::read(case.protocol.selected_root.join("resources.json")).unwrap();
        assert!(InstalledResources::verify(case.protocol.selected_root.clone(), &digest(&manifest)).is_err());

        let closed = case.close(first).await;
        case.assert_settled(closed);
        {
            let report = &case.reports[closed];
            let mut settlements = report.guards().iter()
                .filter(|record| record.stage == McpGuardStage::Settle);
            let record = settlements.next()
                .expect("the actual installed Initial has a native Settle record");
            assert!(settlements.next().is_none(), "one installed Initial has one consuming finish");
            assert_eq!(record.completion, McpGuardCompletion::Completed);
            let disposition = record.disposition.as_ref().unwrap();
            assert_eq!(disposition.key, case.probe.requests.lock().unwrap()[0].key);
            assert!(matches!(disposition.open, McpAttemptOpenOutcome::Installed { .. }));
            let McpAttemptProcessEnd::Closed(receipt) = &disposition.process else {
                panic!("native custody must supply the actual child close: {disposition:?}");
            };
            assert!(McpServerCloseOutcome::Stdio(receipt.clone()).is_settled());
            assert!(matches!(&receipt.leader, McpStdioLeaderOutcome::Reaped(status) if status.success()));
            assert!(receipt.sigterm.is_none() && receipt.sigkill.is_none());
        }
        assert!(case.protocol.output.join(format!("{first_pid}.native-ended")).exists());
        assert_eq!(fs::read(&resource).unwrap(), changed);

        case.fixture.retire(&session).await;
        let next_session = case.fixture.register().await;
        assert_ne!(session, next_session);
        let refused = case.route(&next_session);
        case.add(refused, ExternalToolDeltaPhase::Failed).await;
        case.close(refused).await;
        {
            let requests = case.probe.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[1].owner.session_id, next_session);
        }
        assert_eq!(case.native_pids(), vec![first_pid],
            "changed resources refuse the next current begin before native spawn");
        assert!(case.protocol.pids().is_empty(),
            "returning closed custody does not authorize a changed successor offer");

        fs::write(&resource, &original).unwrap();
        let next = case.route(&next_session);
        case.add(next, ExternalToolDeltaPhase::Applied).await;
        {
            let requests = case.probe.requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            assert_eq!(requests[2].owner.session_id, next_session);
            assert_ne!(requests[0].key, requests[2].key);
        }
        assert_eq!(case.native_pids().len(), 2);
        assert_eq!(case.protocol.pids().len(), 1,
            "healthy Initial keeps started custody and needs one actual successor offer");
        let closed = case.close(next).await;
        case.assert_settled(closed);
        let owner = case.attempts.take_closed_owner()
            .expect("the original owner returns after actual Offered close");
        case.protocol.owner = Some(owner);
        let owner = case.protocol.owner.as_ref().unwrap();
        assert!(!owner.poisoned && !owner.has_pending_round());
        assert_eq!(owner.resources.root, case.protocol.selected_root);
    })).await;
}
