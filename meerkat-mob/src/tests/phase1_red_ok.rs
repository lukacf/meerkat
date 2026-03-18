#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use crate::{FlowRunConfig, MobBuilder, MobError, MobStorage, Prefab, SpawnMemberSpec};
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::ops_lifecycle::{
    OperationKind, OperationPeerHandle, OperationProgressUpdate, OperationResult, OperationSpec,
    OperationStatus, OperationTerminalOutcome, OpsLifecycleRegistry,
};
use meerkat_core::types::SessionId;
use meerkat_runtime::RuntimeOpsLifecycleRegistry;

fn mob_spec(name: &str) -> OperationSpec {
    OperationSpec {
        id: meerkat_core::ops_lifecycle::OperationId::new(),
        kind: OperationKind::MobMemberChild,
        owner_session_id: SessionId::new(),
        display_name: name.into(),
        source_label: "phase1-mob".into(),
        child_session_id: Some(SessionId::new()),
        expect_peer_channel: true,
    }
}

fn background_spec(name: &str) -> OperationSpec {
    OperationSpec {
        id: meerkat_core::ops_lifecycle::OperationId::new(),
        kind: OperationKind::BackgroundToolOp,
        owner_session_id: SessionId::new(),
        display_name: name.into(),
        source_label: "phase1-tool".into(),
        child_session_id: None,
        expect_peer_channel: false,
    }
}

#[tokio::test]
#[ignore = "Phase 1 red-ok mob decomposition suite"]
async fn mob_decomposition_red_ok_builder_mode_guards_keep_create_and_resume_paths_distinct() {
    let definition = Prefab::Pipeline.definition();

    let resume_on_create = match MobBuilder::new(definition.clone(), MobStorage::in_memory())
        .resume()
        .await
    {
        Ok(_) => panic!("create-mode builders should reject resume()"),
        Err(err) => err,
    };
    assert!(
        matches!(resume_on_create, MobError::Internal(message) if message.contains("requires MobBuilder::for_resume")),
        "builder mode boundary should keep create and resume paths distinct"
    );

    let create_on_resume = match MobBuilder::for_resume(MobStorage::in_memory())
        .create()
        .await
    {
        Ok(_) => panic!("resume-mode builders should reject create()"),
        Err(err) => err,
    };
    assert!(
        matches!(create_on_resume, MobError::Internal(message) if message.contains("cannot be used with for_resume")),
        "resume builders should not become a second create path"
    );
}

#[test]
#[ignore = "Phase 1 red-ok mob decomposition suite"]
fn mob_decomposition_red_ok_flow_run_config_carries_orchestrator_and_flow_truth() {
    let definition = Prefab::Pipeline.definition();
    let flow_id = definition
        .flows
        .keys()
        .next()
        .cloned()
        .expect("pipeline prefab should define at least one flow");
    let config = FlowRunConfig::from_definition(flow_id.clone(), &definition)
        .expect("flow config should be derivable from durable mob definition");

    assert_eq!(config.flow_id, flow_id);
    assert!(
        !config.flow_spec.steps.is_empty(),
        "durable flow truth should include concrete step ownership"
    );
    assert!(
        config.orchestrator_role.is_some(),
        "flow truth should preserve orchestrator ownership when present"
    );
    assert_eq!(config.topology, definition.topology);
    assert_eq!(config.supervisor, definition.supervisor);
    assert_eq!(config.limits, definition.limits);
}

#[tokio::test]
#[ignore = "Phase 1 red-ok shared lifecycle integration suite"]
async fn ops_registry_integration_red_ok_tracks_mob_member_peer_ready_and_completion() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let spec = mob_spec("member-alpha");
    let op_id = spec.id.clone();

    registry
        .register_operation(spec.clone())
        .expect("register operation");
    let watch = registry.register_watcher(&op_id).expect("register watcher");
    registry
        .provisioning_succeeded(&op_id)
        .expect("provision success");
    registry
        .report_progress(
            &op_id,
            OperationProgressUpdate {
                message: "member booted".into(),
                percent: Some(0.5),
            },
        )
        .expect("report progress");
    registry
        .peer_ready(
            &op_id,
            OperationPeerHandle {
                peer_name: "member-alpha".into(),
                trusted_peer: TrustedPeerSpec::new(
                    "member-alpha",
                    "member-alpha-id",
                    "inproc://member-alpha",
                )
                .expect("trusted peer"),
            },
        )
        .expect("peer ready");
    registry
        .complete_operation(
            &op_id,
            OperationResult {
                id: op_id.clone(),
                content: "member complete".into(),
                is_error: false,
                duration_ms: 42,
                tokens_used: 8,
            },
        )
        .expect("complete operation");

    assert_eq!(
        watch.wait().await,
        OperationTerminalOutcome::Completed(OperationResult {
            id: op_id.clone(),
            content: "member complete".into(),
            is_error: false,
            duration_ms: 42,
            tokens_used: 8,
        })
    );

    let snapshot = registry.snapshot(&op_id).expect("snapshot");
    assert_eq!(snapshot.status, OperationStatus::Completed);
    assert!(snapshot.peer_ready);
    assert_eq!(snapshot.child_session_id, spec.child_session_id);
}

#[tokio::test]
#[ignore = "Phase 1 red-ok shared lifecycle integration suite"]
async fn ops_registry_integration_red_ok_background_ops_retire_without_peer_handoff() {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let spec = background_spec("shell-job");
    let op_id = spec.id.clone();

    registry
        .register_operation(spec)
        .expect("register background op");
    let watch = registry.register_watcher(&op_id).expect("register watcher");
    registry
        .provisioning_succeeded(&op_id)
        .expect("provision success");
    registry
        .report_progress(
            &op_id,
            OperationProgressUpdate {
                message: "job draining".into(),
                percent: Some(0.9),
            },
        )
        .expect("report progress");
    registry.request_retire(&op_id).expect("request retire");
    registry.mark_retired(&op_id).expect("mark retired");

    assert_eq!(watch.wait().await, OperationTerminalOutcome::Retired);
    assert_eq!(
        registry.snapshot(&op_id).expect("snapshot").status,
        OperationStatus::Retired
    );
}

#[test]
#[ignore = "Phase 1 red-ok shared lifecycle integration suite"]
fn ops_registry_integration_red_ok_existing_session_attach_reuses_spawn_control_plane() {
    let session_id = SessionId::new();
    let spec = SpawnMemberSpec::new("orchestrator", "member-alpha")
        .with_resume_session_id(session_id.clone());

    assert_eq!(spec.resume_session_id, Some(session_id));
    assert_eq!(spec.profile_name.as_str(), "orchestrator");
}
