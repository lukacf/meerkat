#![allow(clippy::unwrap_used)]

use meerkat::project_job_description;
use meerkat_jobs::{
    JobDescription, JobFailureCode, JobId, JobPhase, JobProgress, JobTerminalResult, RestartClass,
    RunnerIdentity,
};

#[test]
fn domain_job_description_maps_to_safe_wire_projection() {
    let wire = project_job_description(JobDescription {
        job_id: JobId::new("job_01").unwrap(),
        runner: RunnerIdentity::new("mobkit_callback", "1").unwrap(),
        phase: JobPhase::NeedsAttention,
        restart_class: RestartClass::NonResumable,
        attempt_count: 1,
        progress: Some(JobProgress::new(2, "worker lost").unwrap()),
        cancel_requested: false,
        terminal_result: Some(JobTerminalResult::NeedsAttention {
            reason: JobFailureCode::new("host_disconnected").unwrap(),
        }),
        subscription_count: 1,
        delivery_backlog: 1,
    });

    let value = serde_json::to_value(wire).unwrap();
    assert_eq!(value["phase"], "needs_attention");
    assert_eq!(
        value["terminal_result"],
        serde_json::json!({
            "kind": "needs_attention",
            "reason": "host_disconnected",
        })
    );
    let encoded = value.to_string();
    assert!(!encoded.contains("fence"));
    assert!(!encoded.contains("attempt_id"));
}
