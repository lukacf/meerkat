#![allow(clippy::expect_used)]

use meerkat_contracts::{
    CallbackToolDefinition, CallbackToolExecution, JobAttemptAuthority, JobDeliveryKind,
    JobExecutionActivity, JobExecutionActivityKind, JobHealthStatus, JobHealthSummary,
    JobIdempotencyScope, JobPhase, JobProgress, JobRestartClass, JobRunner, JobSummary,
    JobTerminalResult, JobsArtifactsResult, JobsGetResult, JobsHealthResult, JobsProgressResult,
    JobsResultResult, JobsRetryParams, MobkitJobProgressParams, MonitorOutputProtocol,
    MonitorsStartParams,
};

#[test]
fn app_job_projection_never_serializes_attempt_fence_worker_or_credentials() {
    let result = JobsGetResult {
        job: JobSummary {
            job_id: "job_01".into(),
            runner: JobRunner {
                name: "mobkit_callback".into(),
                version: "1".into(),
            },
            phase: JobPhase::Running,
            restart_class: JobRestartClass::NonResumable,
            attempt_count: 1,
            progress: Some(JobProgress {
                cursor: 3,
                detail: "scanning".into(),
            }),
            cancel_requested: false,
            terminal_result: None,
            subscription_count: 1,
            delivery_backlog: 0,
        },
    };

    let encoded = serde_json::to_value(result).expect("encode");
    let text = encoded.to_string();
    assert_eq!(encoded["job"]["phase"], "running");
    assert!(!text.contains("attempt_id"));
    assert!(!text.contains("fence"));
    assert!(!text.contains("worker"));
    assert!(!text.contains("credential"));
}

#[test]
fn monitor_start_is_explicit_idempotent_and_contains_no_write_authority() {
    let params = MonitorsStartParams {
        session_id: "session_01".into(),
        submission_key: "deploy-health-v1".into(),
        command: "health-monitor.nu".into(),
        working_dir: Some("/workspace".into()),
        timeout_secs: 3_600,
        protocol: MonitorOutputProtocol::FramedJsonl,
        restart_class: JobRestartClass::CheckpointResumable,
        delivery: JobDeliveryKind::Notification,
        max_line_bytes: Some(65_536),
        max_notifications_per_window: Some(60),
        notification_window_ms: Some(60_000),
        max_retained_diagnostic_bytes: Some(262_144),
    };

    let encoded = serde_json::to_value(params).expect("encode monitor start");
    assert_eq!(encoded["submission_key"], "deploy-health-v1");
    assert_eq!(encoded["restart_class"], "checkpoint_resumable");
    assert_eq!(encoded["protocol"], "framed_jsonl");
    let text = encoded.to_string();
    assert!(!text.contains("attempt_id"));
    assert!(!text.contains("fence"));
    assert!(!text.contains("worker"));
    assert!(!text.contains("lease"));
}

#[test]
fn callback_registration_carries_private_detached_execution_metadata() {
    let definition: CallbackToolDefinition = serde_json::from_value(serde_json::json!({
        "name": "security_scan",
        "description": "scan",
        "input_schema": {"type": "object"},
        "execution": {
            "mode": "detached",
            "runner": {"name": "mobkit_callback", "version": "1"},
            "restart_class": "non_resumable",
            "idempotency_scope": "interaction_and_arguments",
            "submission_timeout_ms": 30000,
            "credential_scopes": ["unifi.read"]
        }
    }))
    .expect("decode");

    assert!(matches!(
        definition.execution,
        Some(CallbackToolExecution::Detached {
            restart_class: JobRestartClass::NonResumable,
            idempotency_scope: JobIdempotencyScope::InteractionAndArguments,
            ..
        })
    ));
}

#[test]
fn host_progress_mutation_carries_exact_attempt_write_authority() {
    let params = MobkitJobProgressParams {
        authority: JobAttemptAuthority {
            job_id: "job_01".into(),
            attempt_id: "attempt_02".into(),
            fence: 7,
        },
        cursor: 4,
        detail: "halfway".into(),
        observed_at_ms: 123,
    };

    let encoded = serde_json::to_value(params).expect("encode");
    assert_eq!(encoded["authority"]["job_id"], "job_01");
    assert_eq!(encoded["authority"]["attempt_id"], "attempt_02");
    assert_eq!(encoded["authority"]["fence"], 7);
}

#[test]
fn health_and_activity_distinguish_healthy_awaiting_from_faults() {
    let health = JobHealthSummary {
        status: JobHealthStatus::Ok,
        queued: 1,
        running: 2,
        awaiting_members: 1,
        stale_leases: 0,
        needs_attention: 0,
        delivery_backlog: 0,
    };
    let activity = JobExecutionActivity {
        kind: JobExecutionActivityKind::AwaitingDetached,
        since_ms: 123,
        job_ids: vec!["job_01".into()],
    };

    assert_eq!(
        serde_json::to_value(health).expect("health")["status"],
        "ok"
    );
    assert_eq!(
        serde_json::to_value(activity).expect("activity")["kind"],
        "awaiting_detached"
    );
}

#[test]
fn terminal_and_delivery_enums_have_stable_tagged_shapes() {
    assert_eq!(
        serde_json::to_value(JobTerminalResult::NeedsAttention {
            reason: "credential_removed".into(),
        })
        .expect("terminal"),
        serde_json::json!({
            "kind": "needs_attention",
            "reason": "credential_removed",
        })
    );
    assert_eq!(
        serde_json::to_value(JobDeliveryKind::Event {
            handling_mode: meerkat_contracts::WireHandlingMode::Steer,
        })
        .expect("delivery"),
        serde_json::json!({
            "kind": "event",
            "handling_mode": "steer",
        })
    );
}

#[test]
fn public_control_projections_do_not_leak_attempt_authority() {
    let progress = JobsProgressResult {
        job_id: "job_01".into(),
        phase: JobPhase::Running,
        progress: Some(JobProgress {
            cursor: 4,
            detail: "working".into(),
        }),
    };
    let result = JobsResultResult {
        job_id: "job_01".into(),
        phase: JobPhase::Succeeded,
        result: Some(JobTerminalResult::Succeeded {
            result_ref: Some("artifact:result".into()),
        }),
    };
    let artifacts = JobsArtifactsResult {
        job_id: "job_01".into(),
        artifacts: vec![meerkat_contracts::JobArtifactRef {
            reference: "artifact:result".into(),
        }],
    };
    let retry = JobsRetryParams {
        job_id: "job_01".into(),
        retry_due_at_ms: 123,
    };
    let health = JobsHealthResult {
        detached_jobs: JobHealthSummary {
            status: JobHealthStatus::Ok,
            queued: 1,
            running: 0,
            awaiting_members: 0,
            stale_leases: 0,
            needs_attention: 0,
            delivery_backlog: 0,
        },
    };

    let encoded = serde_json::json!({
        "progress": progress,
        "result": result,
        "artifacts": artifacts,
        "retry": retry,
        "health": health,
    })
    .to_string();
    assert!(!encoded.contains("attempt_id"));
    assert!(!encoded.contains("fence"));
    assert!(!encoded.contains("worker"));
    assert!(!encoded.contains("runner_handle"));
}
