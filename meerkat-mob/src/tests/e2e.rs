//! E2E tests (Phase 2).
//!
//! These tests exercise the full MobRuntime with mock meerkats:
//! spec apply -> activate -> DAG dispatch -> comms collection -> run completion.
//! No API keys needed -- mock meerkats auto-respond to PeerRequests.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone
)]

use std::sync::Arc;
use uuid::Uuid;

use crate::run::{RunStatus, StepEntryStatus};
use crate::runtime::{MobRuntime, MobRuntimeConfig};
use crate::service::{ActivateRequest, ApplySpecRequest, MobService};
use crate::spec::parse_mob_specs_from_toml;
use crate::store::{InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore};
use crate::validate::ValidateOptions;

/// Helper: create a MobRuntime with in-memory stores.
fn create_test_runtime(mob_id: &str) -> (MobRuntime, String) {
    let suffix = Uuid::new_v4().simple().to_string();
    let realm_id = format!("test-realm-{suffix}");
    let spec_store = Arc::new(InMemoryMobSpecStore::new());
    let run_store = Arc::new(InMemoryMobRunStore::new());
    let event_store = Arc::new(InMemoryMobEventStore::new());

    let config = MobRuntimeConfig {
        realm_id: realm_id.clone(),
        mob_id: mob_id.to_string(),
        validate_opts: ValidateOptions::default(),
    };

    let runtime = MobRuntime::new(config, spec_store, run_store, event_store)
        .expect("create MobRuntime");
    (runtime, realm_id)
}

/// Helper: wait for a run to reach a terminal state, with timeout.
async fn wait_for_run_terminal(
    service: &dyn MobService,
    run_id: &str,
    timeout_ms: u64,
) -> crate::run::MobRun {
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_millis(timeout_ms);

    loop {
        let run = service.get_run(run_id).await.unwrap();
        if run.status.is_terminal() {
            return run;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!(
                "Timed out waiting for run {} to reach terminal state (current: {:?})",
                run_id, run.status
            );
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

// ---------------------------------------------------------------------------
// E2E-MOB-001: Single-step fan-out flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_mob_001_single_step_fan_out() {
    // Scenario: Create mob spec with 3 reviewers. Activate triage flow.
    // Dispatch to all 3 reviewers via fan_out. All 3 respond.
    // Collection policy all completes. Run status becomes completed.

    let toml = r#"
[mob.specs.review_squad]
roles = [
    { role = "coordinator", prompt_inline = "You coordinate code reviews." },
    { role = "reviewer", prompt_inline = "You review code." },
]

[mob.specs.review_squad.topology]
mode = "advisory"

[[mob.specs.review_squad.flows]]
flow_id = "triage"
steps = [
    { step_id = "fan_out_review", targets = { role = "reviewer", meerkat_id = "*" }, dispatch_mode = "fan_out", collection_policy = { mode = "all", timeout_behavior = "partial" }, timeout_ms = 5000 },
]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);

    let (runtime, realm_id) = create_test_runtime("review_squad");
    let namespace = format!("{realm_id}/review_squad");

    // Apply spec
    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // Spawn 3 mock reviewers
    let mut mock_handles = Vec::new();
    let mut mock_runtimes = Vec::new();

    for i in 1..=3 {
        let meerkat_id = format!("reviewer-{i}");
        let name = format!("reviewer-{i}-{}", Uuid::new_v4().simple());
        let (mock_rt, handle) = crate::runtime::spawn_mock_meerkat(
            &namespace,
            &name,
            "reviewer",
            &meerkat_id,
            runtime.supervisor(),
        );
        // Establish mutual trust
        crate::runtime::establish_trust(runtime.supervisor(), &mock_rt).await;
        mock_runtimes.push(mock_rt);
        mock_handles.push(handle);
    }

    // Activate
    let result = runtime
        .activate(ActivateRequest {
            mob_id: "review_squad".to_string(),
            flow_id: "triage".to_string(),
            params: serde_json::json!({"pr_number": 42}),
        })
        .await
        .unwrap();

    assert_eq!(result.status, RunStatus::Pending);
    assert_eq!(result.spec_revision, 1);

    // Wait for completion
    let run = wait_for_run_terminal(&runtime, &result.run_id, 10_000).await;
    assert_eq!(
        run.status,
        RunStatus::Completed,
        "Run should complete, got: {:?}. Ledger: {:?}",
        run.status,
        run.step_ledger
    );

    // Verify step ledger has entries for all 3 reviewers
    let completed_entries: Vec<_> = run
        .step_ledger
        .iter()
        .filter(|e| e.status == StepEntryStatus::Completed)
        .collect();
    assert_eq!(
        completed_entries.len(),
        3,
        "Should have 3 completed entries, got: {:?}",
        run.step_ledger
    );

    // Verify each completed entry has a result
    for entry in &completed_entries {
        assert!(
            entry.result.is_some(),
            "Completed entry should have a result"
        );
        let result = entry.result.as_ref().unwrap();
        assert_eq!(result["mock"], true);
    }

    // Cleanup mock tasks
    for handle in mock_handles {
        handle.abort();
    }
}

// ---------------------------------------------------------------------------
// E2E-MOB-002: Multi-step DAG with parallel branches
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_mob_002_multi_step_dag_parallel_branches() {
    // Scenario: Flow with 3 steps: A (no deps), B (no deps), C (depends on A and B).
    // A and B run concurrently. C runs only after both complete.

    let toml = r#"
[mob.specs.dag_mob]
roles = [
    { role = "analyzer", prompt_inline = "You analyze code." },
    { role = "tester", prompt_inline = "You write tests." },
    { role = "integrator", prompt_inline = "You integrate results." },
]

[[mob.specs.dag_mob.flows]]
flow_id = "parallel_dag"
steps = [
    { step_id = "analyze", targets = { role = "analyzer" }, dispatch_mode = "fan_out", timeout_ms = 5000 },
    { step_id = "test", targets = { role = "tester" }, dispatch_mode = "fan_out", timeout_ms = 5000 },
    { step_id = "integrate", depends_on = ["analyze", "test"], targets = { role = "integrator" }, dispatch_mode = "fan_out", timeout_ms = 5000 },
]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);

    let (runtime, realm_id) = create_test_runtime("dag_mob");
    let namespace = format!("{realm_id}/dag_mob");

    // Apply spec
    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // Spawn mock meerkats for each role
    let mut mock_handles = Vec::new();

    for (role, meerkat_id) in [
        ("analyzer", "analyzer-1"),
        ("tester", "tester-1"),
        ("integrator", "integrator-1"),
    ] {
        let name = format!("{meerkat_id}-{}", Uuid::new_v4().simple());
        let (mock_rt, handle) = crate::runtime::spawn_mock_meerkat(
            &namespace,
            &name,
            role,
            meerkat_id,
            runtime.supervisor(),
        );
        crate::runtime::establish_trust(runtime.supervisor(), &mock_rt).await;
        mock_handles.push(handle);
    }

    // Activate
    let result = runtime
        .activate(ActivateRequest {
            mob_id: "dag_mob".to_string(),
            flow_id: "parallel_dag".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();

    assert_eq!(result.status, RunStatus::Pending);

    // Wait for completion
    let run = wait_for_run_terminal(&runtime, &result.run_id, 15_000).await;
    assert_eq!(
        run.status,
        RunStatus::Completed,
        "Run should complete, got: {:?}. Ledger: {:?}",
        run.status,
        run.step_ledger
    );

    // Verify all 3 steps completed
    let completed_steps: std::collections::HashSet<&str> = run
        .step_ledger
        .iter()
        .filter(|e| e.status == StepEntryStatus::Completed)
        .map(|e| e.step_id.as_str())
        .collect();
    assert!(
        completed_steps.contains("analyze"),
        "analyze step should complete"
    );
    assert!(
        completed_steps.contains("test"),
        "test step should complete"
    );
    assert!(
        completed_steps.contains("integrate"),
        "integrate step should complete"
    );

    // Verify timing: analyze and test dispatched before integrate.
    // Both a and b should have dispatched_at before c's dispatched_at.
    let dispatched_entries: Vec<_> = run
        .step_ledger
        .iter()
        .filter(|e| e.status == StepEntryStatus::Dispatched)
        .collect();

    let analyze_dispatched = dispatched_entries
        .iter()
        .find(|e| e.step_id == "analyze")
        .and_then(|e| e.dispatched_at);
    let test_dispatched = dispatched_entries
        .iter()
        .find(|e| e.step_id == "test")
        .and_then(|e| e.dispatched_at);
    let integrate_dispatched = dispatched_entries
        .iter()
        .find(|e| e.step_id == "integrate")
        .and_then(|e| e.dispatched_at);

    if let (Some(a_time), Some(b_time), Some(c_time)) =
        (analyze_dispatched, test_dispatched, integrate_dispatched)
    {
        assert!(
            c_time >= a_time,
            "integrate should dispatch after analyze"
        );
        assert!(
            c_time >= b_time,
            "integrate should dispatch after test"
        );
    }

    // Cleanup
    for handle in mock_handles {
        handle.abort();
    }
}

// ---------------------------------------------------------------------------
// E2E-MOB-003: Quorum collection with partial timeout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_mob_003_quorum_collection_with_partial_timeout() {
    // Scenario: Fan-out to 5 reviewers, quorum(3), 3 respond (mock), 2 time out
    // (silent mock). Step completes with 3 results. Failure ledger has 2 timeout
    // entries.

    let toml = r#"
[mob.specs.quorum_mob]
roles = [
    { role = "coordinator", prompt_inline = "You coordinate code reviews." },
    { role = "reviewer", prompt_inline = "You review code." },
]

[mob.specs.quorum_mob.topology]
mode = "advisory"

[[mob.specs.quorum_mob.flows]]
flow_id = "quorum_review"
steps = [
    { step_id = "fan_review", targets = { role = "reviewer", meerkat_id = "*" }, dispatch_mode = "fan_out", collection_policy = { mode = { "quorum" = 3 }, timeout_behavior = "partial" }, timeout_ms = 1000 },
]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);

    let (runtime, realm_id) = create_test_runtime("quorum_mob");
    let namespace = format!("{realm_id}/quorum_mob");

    // Apply spec
    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // Spawn 3 responding mock reviewers
    let mut mock_handles = Vec::new();

    for i in 1..=3 {
        let meerkat_id = format!("reviewer-{i}");
        let name = format!("reviewer-{i}-{}", Uuid::new_v4().simple());
        let (mock_rt, handle) = crate::runtime::spawn_mock_meerkat(
            &namespace,
            &name,
            "reviewer",
            &meerkat_id,
            runtime.supervisor(),
        );
        crate::runtime::establish_trust(runtime.supervisor(), &mock_rt).await;
        mock_handles.push(handle);
    }

    // Spawn 2 silent (non-responding) mock reviewers
    for i in 4..=5 {
        let meerkat_id = format!("reviewer-{i}");
        let name = format!("reviewer-{i}-{}", Uuid::new_v4().simple());
        let (mock_rt, handle) = crate::runtime::spawn_silent_mock_meerkat(
            &namespace,
            &name,
            "reviewer",
            &meerkat_id,
            runtime.supervisor(),
        );
        crate::runtime::establish_trust(runtime.supervisor(), &mock_rt).await;
        mock_handles.push(handle);
    }

    // Activate
    let result = runtime
        .activate(ActivateRequest {
            mob_id: "quorum_mob".to_string(),
            flow_id: "quorum_review".to_string(),
            params: serde_json::json!({"pr_number": 99}),
        })
        .await
        .unwrap();

    assert_eq!(result.status, RunStatus::Pending);

    // Wait for completion (step should succeed with quorum met)
    let run = wait_for_run_terminal(&runtime, &result.run_id, 10_000).await;
    assert_eq!(
        run.status,
        RunStatus::Completed,
        "Run should complete with quorum(3) met from 3 responders, got: {:?}. Ledger: {:?}",
        run.status,
        run.step_ledger
    );

    // Verify: at least 3 completed step entries
    let completed_entries: Vec<_> = run
        .step_ledger
        .iter()
        .filter(|e| e.status == StepEntryStatus::Completed)
        .collect();
    assert!(
        completed_entries.len() >= 3,
        "Should have at least 3 completed entries (quorum met), got: {}",
        completed_entries.len()
    );

    // Verify: 2 timed-out entries in step ledger
    let timed_out_entries: Vec<_> = run
        .step_ledger
        .iter()
        .filter(|e| e.status == StepEntryStatus::TimedOut)
        .collect();
    assert_eq!(
        timed_out_entries.len(),
        2,
        "Should have 2 timed out entries, got: {:?}",
        timed_out_entries
    );

    // Verify: 2 failure ledger entries for timeouts
    assert_eq!(
        run.failure_ledger.len(),
        2,
        "Should have 2 failure ledger entries for timeouts, got: {:?}",
        run.failure_ledger
    );
    for entry in &run.failure_ledger {
        assert!(
            entry.error.contains("timeout"),
            "Failure entry should mention timeout: {}",
            entry.error
        );
    }

    // Cleanup
    for handle in mock_handles {
        handle.abort();
    }
}

// ---------------------------------------------------------------------------
// E2E-MOB-004: drain_replace pins active runs to old spec revision
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_mob_004_drain_replace_pins_old_revision() {
    // Scenario: Apply spec (revision 1), start a run, apply updated spec
    // (revision 2). The running run should remain pinned to revision 1.
    // A new activation should use revision 2.

    let toml_v1 = r#"
[mob.specs.drain_mob]
roles = [
    { role = "worker", prompt_inline = "You work on tasks." },
]

[[mob.specs.drain_mob.flows]]
flow_id = "work"
steps = [
    { step_id = "do_work", targets = { role = "worker" }, dispatch_mode = "fan_out", timeout_ms = 5000 },
]
"#;

    let spec_v1 = parse_mob_specs_from_toml(toml_v1).unwrap().remove(0);

    let (runtime, realm_id) = create_test_runtime("drain_mob");
    let namespace = format!("{realm_id}/drain_mob");

    // Apply spec v1
    let applied_v1 = runtime
        .apply_spec(ApplySpecRequest {
            spec: spec_v1.clone(),
        })
        .await
        .unwrap();
    assert_eq!(applied_v1.spec_revision, 1);

    // Spawn a mock worker
    let name = format!("worker-1-{}", Uuid::new_v4().simple());
    let (mock_rt, mock_handle) = crate::runtime::spawn_mock_meerkat(
        &namespace,
        &name,
        "worker",
        "worker-1",
        runtime.supervisor(),
    );
    crate::runtime::establish_trust(runtime.supervisor(), &mock_rt).await;

    // Activate flow with v1
    let result_v1 = runtime
        .activate(ActivateRequest {
            mob_id: "drain_mob".to_string(),
            flow_id: "work".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();

    assert_eq!(result_v1.spec_revision, 1, "Run should use spec revision 1");

    // Apply spec v2 (update a limit to make it different)
    let toml_v2 = r#"
[mob.specs.drain_mob]
spec_revision = 1
roles = [
    { role = "worker", prompt_inline = "You work harder on tasks." },
]

[mob.specs.drain_mob.limits]
max_steps_per_flow = 100

[[mob.specs.drain_mob.flows]]
flow_id = "work"
steps = [
    { step_id = "do_work", targets = { role = "worker" }, dispatch_mode = "fan_out", timeout_ms = 5000 },
]
"#;

    let spec_v2 = parse_mob_specs_from_toml(toml_v2).unwrap().remove(0);
    let applied_v2 = runtime
        .apply_spec(ApplySpecRequest { spec: spec_v2 })
        .await
        .unwrap();
    assert_eq!(applied_v2.spec_revision, 2);

    // Wait for v1 run to complete
    let run_v1 = wait_for_run_terminal(&runtime, &result_v1.run_id, 10_000).await;
    assert_eq!(
        run_v1.status,
        RunStatus::Completed,
        "v1 run should complete"
    );
    // The run is still pinned to revision 1
    assert_eq!(
        run_v1.spec_revision, 1,
        "Running run should remain pinned to spec revision 1"
    );

    // Activate a new run - should use v2
    let result_v2 = runtime
        .activate(ActivateRequest {
            mob_id: "drain_mob".to_string(),
            flow_id: "work".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();

    assert_eq!(
        result_v2.spec_revision, 2,
        "New run should use latest spec revision 2"
    );

    // Wait for v2 run to complete
    let run_v2 = wait_for_run_terminal(&runtime, &result_v2.run_id, 10_000).await;
    assert_eq!(
        run_v2.status,
        RunStatus::Completed,
        "v2 run should complete"
    );
    assert_eq!(
        run_v2.spec_revision, 2,
        "v2 run should use spec revision 2"
    );

    // Cleanup
    mock_handle.abort();
}

// ---------------------------------------------------------------------------
// REQ-MOB-037: Retry with backoff on failure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_retry_on_failure() {
    // A mock meerkat that fails on first attempt, succeeds on second.
    // RetrySpec.attempts = 3, so the step should succeed on retry.

    let toml = r#"
[mob.specs.retry_mob]
roles = [
    { role = "worker", prompt_inline = "You work." },
]

[[mob.specs.retry_mob.flows]]
flow_id = "retry_flow"
steps = [
    { step_id = "do_work", targets = { role = "worker" }, dispatch_mode = "fan_out", timeout_ms = 3000, retry = { attempts = 3, backoff_ms = 50, multiplier = 2.0, max_backoff_ms = 200 } },
]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let (runtime, realm_id) = create_test_runtime("retry_mob");
    let namespace = format!("{realm_id}/retry_mob");

    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // Spawn a mock that fails first, then succeeds
    let name = format!("worker-1-{}", Uuid::new_v4().simple());
    let (mock_rt, mock_handle) = crate::runtime::spawn_fail_then_succeed_mock(
        &namespace,
        &name,
        "worker",
        "worker-1",
        runtime.supervisor(),
        1, // fail first 1 attempt
    );
    crate::runtime::establish_trust(runtime.supervisor(), &mock_rt).await;

    let result = runtime
        .activate(ActivateRequest {
            mob_id: "retry_mob".to_string(),
            flow_id: "retry_flow".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();

    let run = wait_for_run_terminal(&runtime, &result.run_id, 10_000).await;
    assert_eq!(run.status, RunStatus::Completed, "Should succeed on retry");

    // Failure ledger should have 1 entry for the failed first attempt
    assert!(
        !run.failure_ledger.is_empty(),
        "Should have failure ledger entry for first failed attempt"
    );

    // Step ledger should have a completed entry with attempt > 1
    let completed = run
        .step_ledger
        .iter()
        .find(|e| e.status == StepEntryStatus::Completed);
    assert!(completed.is_some(), "Should have a completed entry");
    assert!(
        completed.unwrap().attempt > 1,
        "Completed entry should be on attempt > 1"
    );

    mock_handle.abort();
}

// ---------------------------------------------------------------------------
// REQ-MOB-038: Condition skips step
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_condition_skips_step() {
    // Step B has condition activation.priority == "low".
    // Activate with priority = "high". Step B should be skipped.

    let toml = r#"
[mob.specs.cond_mob]
roles = [
    { role = "worker", prompt_inline = "You work." },
    { role = "optional", prompt_inline = "Maybe work." },
]

[[mob.specs.cond_mob.flows]]
flow_id = "cond_flow"
steps = [
    { step_id = "always_run", targets = { role = "worker" }, timeout_ms = 3000 },
    { step_id = "conditional", depends_on = ["always_run"], targets = { role = "optional" }, timeout_ms = 3000, condition = { eq = { path = "activation.priority", value = "low" } } },
]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let (runtime, realm_id) = create_test_runtime("cond_mob");
    let namespace = format!("{realm_id}/cond_mob");

    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // Spawn mock for "worker" role only (optional doesn't need one if skipped)
    let name = format!("worker-1-{}", Uuid::new_v4().simple());
    let (mock_rt, handle1) = crate::runtime::spawn_mock_meerkat(
        &namespace, &name, "worker", "worker-1", runtime.supervisor(),
    );
    crate::runtime::establish_trust(runtime.supervisor(), &mock_rt).await;

    // Also spawn optional mock (in case it runs despite condition)
    let name2 = format!("optional-1-{}", Uuid::new_v4().simple());
    let (mock_rt2, handle2) = crate::runtime::spawn_mock_meerkat(
        &namespace, &name2, "optional", "optional-1", runtime.supervisor(),
    );
    crate::runtime::establish_trust(runtime.supervisor(), &mock_rt2).await;

    let result = runtime
        .activate(ActivateRequest {
            mob_id: "cond_mob".to_string(),
            flow_id: "cond_flow".to_string(),
            params: serde_json::json!({"priority": "high"}),
        })
        .await
        .unwrap();

    let run = wait_for_run_terminal(&runtime, &result.run_id, 10_000).await;
    assert_eq!(run.status, RunStatus::Completed);

    // "always_run" should have completed
    let always_completed = run
        .step_ledger
        .iter()
        .any(|e| e.step_id == "always_run" && e.status == StepEntryStatus::Completed);
    assert!(always_completed, "always_run should complete");

    // "conditional" should be skipped
    let conditional_skipped = run
        .step_ledger
        .iter()
        .any(|e| e.step_id == "conditional" && e.status == StepEntryStatus::Skipped);
    assert!(conditional_skipped, "conditional step should be skipped when condition is false");

    handle1.abort();
    handle2.abort();
}

// ---------------------------------------------------------------------------
// REQ-MOB-100: Schema validation emits warning
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_schema_validation_warns_on_invalid() {
    // Step has expected_schema_ref pointing to a schema requiring {"status": "string"}.
    // Mock returns {"status": 123} (invalid). schema_policy = warn_only.
    // Step should complete but a DegradationWarning event should be emitted.

    let toml = r#"
[mob.specs.schema_mob]
roles = [
    { role = "worker", prompt_inline = "You work." },
]

[mob.specs.schema_mob.schemas.result_schema]
type = "object"
required = ["status"]
[mob.specs.schema_mob.schemas.result_schema.properties.status]
type = "string"

[[mob.specs.schema_mob.flows]]
flow_id = "schema_flow"
steps = [
    { step_id = "validated_step", targets = { role = "worker" }, timeout_ms = 3000, expected_schema_ref = "result_schema", schema_policy = "warn_only" },
]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let (runtime, realm_id) = create_test_runtime("schema_mob");
    let namespace = format!("{realm_id}/schema_mob");

    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // Spawn mock that returns invalid schema (status is integer, not string)
    let name = format!("worker-1-{}", Uuid::new_v4().simple());
    let (mock_rt, mock_handle) = crate::runtime::spawn_mock_meerkat(
        &namespace, &name, "worker", "worker-1", runtime.supervisor(),
    );
    crate::runtime::establish_trust(runtime.supervisor(), &mock_rt).await;

    let result = runtime
        .activate(ActivateRequest {
            mob_id: "schema_mob".to_string(),
            flow_id: "schema_flow".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();

    let run = wait_for_run_terminal(&runtime, &result.run_id, 10_000).await;
    // With warn_only, step should still complete
    assert_eq!(run.status, RunStatus::Completed);

    // Check for DegradationWarning event about schema validation
    use crate::service::PollEventsRequest;
    let events = runtime
        .poll_events(PollEventsRequest {
            mob_id: "schema_mob".to_string(),
            after_cursor: None,
            limit: None,
        })
        .await
        .unwrap();

    let schema_warnings: Vec<_> = events
        .iter()
        .filter(|e| matches!(&e.kind, crate::event::MobEventKind::DegradationWarning { message } if message.contains("schema")))
        .collect();
    assert!(
        !schema_warnings.is_empty(),
        "Should emit DegradationWarning for schema mismatch. Events: {:?}",
        events.iter().map(|e| &e.kind).collect::<Vec<_>>()
    );

    mock_handle.abort();
}

// ---------------------------------------------------------------------------
// REQ-MOB-060: Supervisor fires during flow execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_supervisor_fires_during_flow() {
    // Silent mock (never responds). Supervisor dispatch_timeout_ms = 200ms.
    // Step timeout = 1000ms. Supervisor should fire before step times out.

    let toml = r#"
[mob.specs.sup_mob]
roles = [
    { role = "worker", prompt_inline = "You work." },
]

[mob.specs.sup_mob.supervisor]
enabled = true
dispatch_timeout_ms = 200

[[mob.specs.sup_mob.flows]]
flow_id = "sup_flow"
steps = [
    { step_id = "stuck_step", targets = { role = "worker" }, timeout_ms = 500, collection_policy = { mode = "all", timeout_behavior = "partial" }, retry = { attempts = 1, backoff_ms = 10, multiplier = 1.0, max_backoff_ms = 10 } },
]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let (runtime, realm_id) = create_test_runtime("sup_mob");
    let namespace = format!("{realm_id}/sup_mob");

    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // Spawn silent mock (never responds)
    let name = format!("worker-1-{}", Uuid::new_v4().simple());
    let (mock_rt, mock_handle) = crate::runtime::spawn_silent_mock_meerkat(
        &namespace, &name, "worker", "worker-1", runtime.supervisor(),
    );
    crate::runtime::establish_trust(runtime.supervisor(), &mock_rt).await;

    let result = runtime
        .activate(ActivateRequest {
            mob_id: "sup_mob".to_string(),
            flow_id: "sup_flow".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();

    let run = wait_for_run_terminal(&runtime, &result.run_id, 5_000).await;
    // Should complete (partial timeout behavior)
    assert_eq!(run.status, RunStatus::Completed);

    // Check for SupervisorEscalation event
    use crate::service::PollEventsRequest;
    let events = runtime
        .poll_events(PollEventsRequest {
            mob_id: "sup_mob".to_string(),
            after_cursor: None,
            limit: None,
        })
        .await
        .unwrap();

    let escalations: Vec<_> = events
        .iter()
        .filter(|e| matches!(&e.kind, crate::event::MobEventKind::SupervisorEscalation { .. }))
        .collect();
    assert!(
        !escalations.is_empty(),
        "Supervisor should emit escalation event for stuck dispatch. Events: {:?}",
        events.iter().map(|e| &e.kind).collect::<Vec<_>>()
    );

    mock_handle.abort();
}
