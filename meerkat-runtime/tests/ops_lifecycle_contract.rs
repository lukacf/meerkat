#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 0 external-boundary contract tests for the shared ops lifecycle seam.

use std::sync::Arc;

use chrono::Utc;
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::lifecycle::InputId;
use meerkat_core::ops::{OpEvent, OperationId};
use meerkat_core::ops_lifecycle::{
    OperationKind, OperationPeerHandle, OperationProgressUpdate, OperationResult, OperationSpec,
    OperationStatus, OperationTerminalOutcome, OpsLifecycleRegistry,
};
use meerkat_core::types::SessionId;
use meerkat_runtime::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, OperationInput,
    RuntimeOpsLifecycleRegistry, RuntimeSessionAdapter, SessionServiceRuntimeExt,
};

fn background_spec(name: &str) -> OperationSpec {
    OperationSpec {
        id: meerkat_core::ops_lifecycle::OperationId::new(),
        kind: OperationKind::BackgroundToolOp,
        owner_session_id: SessionId::new(),
        display_name: name.into(),
        source_label: "test-background".into(),
        child_session_id: None,
        expect_peer_channel: false,
    }
}

fn mob_member_spec(name: &str) -> OperationSpec {
    OperationSpec {
        id: meerkat_core::ops_lifecycle::OperationId::new(),
        kind: OperationKind::MobMemberChild,
        owner_session_id: SessionId::new(),
        display_name: name.into(),
        source_label: "test-mob".into(),
        child_session_id: Some(SessionId::new()),
        expect_peer_channel: true,
    }
}

fn peer_handle(name: &str) -> OperationPeerHandle {
    OperationPeerHandle {
        peer_name: name.into(),
        trusted_peer: TrustedPeerSpec::new(name, format!("{name}-id"), format!("inproc://{name}"))
            .unwrap(),
    }
}

fn op_result(id: &meerkat_core::ops_lifecycle::OperationId, content: &str) -> OperationResult {
    OperationResult {
        id: id.clone(),
        content: content.into(),
        is_error: false,
        duration_ms: 42,
        tokens_used: 7,
    }
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn ops_lifecycle_contract_register_progress_peer_ready_complete_and_watch() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let spec = mob_member_spec("member-alpha");
    let op_id = spec.id.clone();

    registry.register_operation(spec.clone()).unwrap();

    let initial = registry.snapshot(&op_id).unwrap();
    assert_eq!(initial.status, OperationStatus::Provisioning);
    assert!(!initial.peer_ready);
    assert_eq!(initial.progress_count, 0);
    assert_eq!(initial.watcher_count, 0);
    assert_eq!(initial.child_session_id, spec.child_session_id);

    let watch = registry.register_watcher(&op_id).unwrap();
    registry.provisioning_succeeded(&op_id).unwrap();
    registry
        .report_progress(
            &op_id,
            OperationProgressUpdate {
                message: "booting".into(),
                percent: Some(0.25),
            },
        )
        .unwrap();
    registry
        .report_progress(
            &op_id,
            OperationProgressUpdate {
                message: "readying peer".into(),
                percent: Some(0.75),
            },
        )
        .unwrap();
    registry
        .peer_ready(&op_id, peer_handle("member-alpha"))
        .unwrap();

    let running = registry.snapshot(&op_id).unwrap();
    assert_eq!(running.status, OperationStatus::Running);
    assert!(running.peer_ready);
    assert_eq!(running.progress_count, 2);
    assert_eq!(running.watcher_count, 1);

    let listed = registry.list_operations();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].display_name, "member-alpha");

    let result = op_result(&op_id, "completed");
    registry.complete_operation(&op_id, result.clone()).unwrap();

    assert_eq!(
        watch.wait().await,
        OperationTerminalOutcome::Completed(result.clone())
    );

    let late_watch = registry.register_watcher(&op_id).unwrap();
    assert_eq!(
        late_watch.wait().await,
        OperationTerminalOutcome::Completed(result.clone())
    );

    let completed = registry.snapshot(&op_id).unwrap();
    assert_eq!(completed.status, OperationStatus::Completed);
    assert_eq!(completed.watcher_count, 0);
    assert_eq!(
        completed.terminal_outcome,
        Some(OperationTerminalOutcome::Completed(result))
    );
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn ops_lifecycle_contract_fail_cancel_and_retire_surface_terminal_outcomes() {
    let registry = RuntimeOpsLifecycleRegistry::new();

    let failed = background_spec("background-fail");
    let failed_id = failed.id.clone();
    registry.register_operation(failed).unwrap();
    let failed_watch = registry.register_watcher(&failed_id).unwrap();
    registry.provisioning_succeeded(&failed_id).unwrap();
    registry
        .fail_operation(&failed_id, "tool crashed".into())
        .unwrap();
    assert_eq!(
        failed_watch.wait().await,
        OperationTerminalOutcome::Failed {
            error: "tool crashed".into(),
        }
    );

    let cancelled = background_spec("background-cancel");
    let cancelled_id = cancelled.id.clone();
    registry.register_operation(cancelled).unwrap();
    let cancelled_watch = registry.register_watcher(&cancelled_id).unwrap();
    registry.provisioning_succeeded(&cancelled_id).unwrap();
    registry
        .cancel_operation(&cancelled_id, Some("operator request".into()))
        .unwrap();
    assert_eq!(
        cancelled_watch.wait().await,
        OperationTerminalOutcome::Cancelled {
            reason: Some("operator request".into()),
        }
    );

    let retired = mob_member_spec("member-retire");
    let retired_id = retired.id.clone();
    registry.register_operation(retired).unwrap();
    let retired_watch = registry.register_watcher(&retired_id).unwrap();
    registry.provisioning_succeeded(&retired_id).unwrap();
    registry.request_retire(&retired_id).unwrap();
    registry
        .report_progress(
            &retired_id,
            OperationProgressUpdate {
                message: "finishing".into(),
                percent: Some(0.9),
            },
        )
        .unwrap();
    let retiring = registry.snapshot(&retired_id).unwrap();
    assert_eq!(retiring.status, OperationStatus::Retiring);
    assert_eq!(retiring.progress_count, 1);

    registry.mark_retired(&retired_id).unwrap();
    assert_eq!(
        retired_watch.wait().await,
        OperationTerminalOutcome::Retired
    );
    assert_eq!(
        registry.snapshot(&retired_id).unwrap().status,
        OperationStatus::Retired
    );
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn ops_lifecycle_contract_terminate_owner_resolves_all_pending_watches_once() {
    let registry = RuntimeOpsLifecycleRegistry::new();

    let provisioning = background_spec("background-terminate");
    let provisioning_id = provisioning.id.clone();
    registry.register_operation(provisioning).unwrap();
    let provisioning_watch = registry.register_watcher(&provisioning_id).unwrap();

    let running = mob_member_spec("member-terminate");
    let running_id = running.id.clone();
    registry.register_operation(running).unwrap();
    registry.provisioning_succeeded(&running_id).unwrap();
    let running_watch = registry.register_watcher(&running_id).unwrap();

    registry
        .terminate_owner("runtime shutting down".into())
        .unwrap();

    let expected = OperationTerminalOutcome::Terminated {
        reason: "runtime shutting down".into(),
    };
    assert_eq!(provisioning_watch.wait().await, expected.clone());
    assert_eq!(running_watch.wait().await, expected.clone());

    let late_watch = registry.register_watcher(&running_id).unwrap();
    assert_eq!(late_watch.wait().await, expected);

    assert_eq!(
        registry.snapshot(&provisioning_id).unwrap().status,
        OperationStatus::Terminated
    );
    assert_eq!(
        registry.snapshot(&running_id).unwrap().status,
        OperationStatus::Terminated
    );
    assert_eq!(
        registry.snapshot(&running_id).unwrap().watcher_count,
        0,
        "terminal watch resolution should drain the registry's active watcher set"
    );
}

fn make_operation_input(operation_id: OperationId, event: OpEvent) -> Input {
    Input::Operation(OperationInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::System,
            durability: InputDurability::Derived,
            visibility: InputVisibility {
                transcript_eligible: false,
                operator_eligible: false,
            },
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        operation_id,
        event,
    })
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn ops_lifecycle_contract_runtime_session_entries_get_distinct_registries() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let session_a = SessionId::new();
    let session_b = SessionId::new();

    adapter.register_session(session_a.clone()).await;
    adapter.register_session(session_b.clone()).await;

    let registry_a = adapter
        .ops_lifecycle_registry(&session_a)
        .await
        .expect("session A registry");
    let registry_b = adapter
        .ops_lifecycle_registry(&session_b)
        .await
        .expect("session B registry");

    assert!(
        !Arc::ptr_eq(&registry_a, &registry_b),
        "each runtime session should own a distinct lifecycle registry"
    );

    let member = mob_member_spec("member-owned-by-a");
    let member_id = member.id.clone();
    registry_a.register_operation(member).unwrap();
    registry_a.provisioning_succeeded(&member_id).unwrap();

    let job = background_spec("job-owned-by-b");
    let job_id = job.id.clone();
    registry_b.register_operation(job).unwrap();
    registry_b.provisioning_succeeded(&job_id).unwrap();

    assert!(registry_a.snapshot(&member_id).is_some());
    assert!(registry_a.snapshot(&job_id).is_none());
    assert!(registry_b.snapshot(&job_id).is_some());
    assert!(registry_b.snapshot(&member_id).is_none());
}

#[tokio::test]
#[ignore = "Phase 0 external boundary contract"]
async fn ops_lifecycle_contract_runtime_admits_operation_inputs_for_child_and_background_events() {
    let adapter = RuntimeSessionAdapter::ephemeral();
    let runtime: &dyn SessionServiceRuntimeExt = &adapter;
    let session_id = SessionId::new();

    adapter.register_session(session_id.clone()).await;

    let child_operation_id = OperationId::new();
    let (child_outcome, child_handle) = runtime
        .accept_input_with_completion(
            &session_id,
            make_operation_input(
                child_operation_id.clone(),
                OpEvent::Progress {
                    id: child_operation_id,
                    message: "member provisioning".into(),
                    percent: Some(0.5),
                },
            ),
        )
        .await
        .expect("accept child lifecycle operation input");
    assert!(child_outcome.is_accepted());
    assert!(
        child_handle.is_none(),
        "ignore-on-accept lifecycle inputs should not allocate completion waiters"
    );

    let background_operation_id = OperationId::new();
    let (background_outcome, background_handle) = runtime
        .accept_input_with_completion(
            &session_id,
            make_operation_input(
                background_operation_id.clone(),
                OpEvent::Cancelled {
                    id: background_operation_id,
                },
            ),
        )
        .await
        .expect("accept background lifecycle operation input");
    assert!(background_outcome.is_accepted());
    assert!(background_handle.is_none());
}
