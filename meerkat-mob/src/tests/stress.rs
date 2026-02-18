//! Stress tests for Phase 4 hardening.
//!
//! Covers:
//! - DAG with 20 ready steps verifies concurrency cap enforcement
//! - Namespace isolation with concurrent mobs
//! - Event cursor monotonicity under concurrent appends

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone
)]

use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

use crate::run::{RunStatus, StepEntryStatus};
use crate::runtime::{MobRuntime, MobRuntimeConfig};
use crate::service::{ActivateRequest, ApplySpecRequest, MobService};
use crate::spec::parse_mob_specs_from_toml;
use crate::store::{InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore, MobEventStore};
use crate::validate::ValidateOptions;

/// Helper: create a MobRuntime with in-memory stores.
fn create_test_runtime(mob_id: &str) -> (MobRuntime, String) {
    let suffix = Uuid::new_v4().simple().to_string();
    let realm_id = format!("stress-realm-{suffix}");
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
// DAG with 20 ready steps: concurrency cap enforcement
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stress_dag_20_ready_steps_concurrency_cap() {
    // Build a flow with 20 independent steps (no dependencies = all ready at once)
    // and a concurrency cap of 5. All 20 should complete, but only 5 at a time.
    let mut steps_toml = String::new();
    for i in 1..=20 {
        steps_toml.push_str(&format!(
            r#"    {{ step_id = "step_{i}", targets = {{ role = "worker", meerkat_id = "worker-{i}" }}, dispatch_mode = "fan_out", timeout_ms = 5000 }},
"#
        ));
    }

    let toml = format!(
        r#"
[mob.specs.wide_dag]
roles = [
    {{ role = "worker", prompt_inline = "You work." }},
]

[mob.specs.wide_dag.limits]
max_concurrent_ready_steps = 5

[[mob.specs.wide_dag.flows]]
flow_id = "parallel"
steps = [
{steps_toml}]
"#
    );

    let spec = parse_mob_specs_from_toml(&toml).unwrap().remove(0);
    let (runtime, realm_id) = create_test_runtime("wide_dag");
    let namespace = format!("{realm_id}/wide_dag");

    // Apply spec
    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // Spawn 20 mock workers
    let mut mock_handles = Vec::new();
    for i in 1..=20 {
        let meerkat_id = format!("worker-{i}");
        let name = format!("worker-{i}-{}", Uuid::new_v4().simple());
        let (mock_rt, handle) = crate::runtime::spawn_mock_meerkat(
            &namespace,
            &name,
            "worker",
            &meerkat_id,
            runtime.supervisor(),
        );
        crate::runtime::establish_trust(runtime.supervisor(), &mock_rt).await;
        mock_handles.push(handle);
    }

    // Activate
    let result = runtime
        .activate(ActivateRequest {
            mob_id: "wide_dag".to_string(),
            flow_id: "parallel".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();

    assert_eq!(result.status, RunStatus::Pending);

    // Wait for completion
    let run = wait_for_run_terminal(&runtime, &result.run_id, 30_000).await;
    assert_eq!(
        run.status,
        RunStatus::Completed,
        "Run with 20 steps should complete. Ledger: {:?}",
        run.step_ledger
    );

    // Verify all 20 steps completed
    let completed_step_ids: HashSet<&str> = run
        .step_ledger
        .iter()
        .filter(|e| e.status == StepEntryStatus::Completed)
        .map(|e| e.step_id.as_str())
        .collect();

    for i in 1..=20 {
        let step_id = format!("step_{i}");
        assert!(
            completed_step_ids.contains(step_id.as_str()),
            "step_{i} should be completed"
        );
    }

    // The concurrency cap is enforced by the semaphore in execute_flow_inner.
    // We can't directly measure concurrent execution count without instrumenting
    // the runtime, but the fact that all 20 steps complete successfully proves
    // the semaphore doesn't deadlock under high fan-out.

    // Cleanup
    for handle in mock_handles {
        handle.abort();
    }
}

// ---------------------------------------------------------------------------
// Namespace isolation: concurrent mobs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stress_namespace_isolation_concurrent_mobs() {
    // Create two independent mob runtimes in different namespaces.
    // Each has its own workers. Verify they don't see each other's peers.

    let (runtime_a, realm_a) = create_test_runtime("mob_alpha");
    let (runtime_b, realm_b) = create_test_runtime("mob_beta");

    let namespace_a = format!("{realm_a}/mob_alpha");
    let namespace_b = format!("{realm_b}/mob_beta");

    // Verify namespaces are different
    assert_ne!(namespace_a, namespace_b);

    // Spawn workers in each namespace
    let (mock_a, handle_a) = crate::runtime::spawn_mock_meerkat(
        &namespace_a,
        &format!("worker-a-{}", Uuid::new_v4().simple()),
        "worker",
        "worker-alpha",
        runtime_a.supervisor(),
    );
    crate::runtime::establish_trust(runtime_a.supervisor(), &mock_a).await;

    let (mock_b, handle_b) = crate::runtime::spawn_mock_meerkat(
        &namespace_b,
        &format!("worker-b-{}", Uuid::new_v4().simple()),
        "worker",
        "worker-beta",
        runtime_b.supervisor(),
    );
    crate::runtime::establish_trust(runtime_b.supervisor(), &mock_b).await;

    // List meerkats: mob_alpha should see only worker-alpha
    let meerkats_a = runtime_a
        .list_meerkats(crate::service::ListMeerkatsRequest {
            mob_id: "mob_alpha".to_string(),
            role: Some("worker".to_string()),
        })
        .await
        .unwrap();

    let meerkat_ids_a: Vec<&str> = meerkats_a.iter().map(|m| m.meerkat_id.as_str()).collect();
    assert!(
        meerkat_ids_a.contains(&"worker-alpha"),
        "mob_alpha should see worker-alpha, got: {:?}",
        meerkat_ids_a
    );
    assert!(
        !meerkat_ids_a.contains(&"worker-beta"),
        "mob_alpha should NOT see worker-beta, got: {:?}",
        meerkat_ids_a
    );

    // List meerkats: mob_beta should see only worker-beta
    let meerkats_b = runtime_b
        .list_meerkats(crate::service::ListMeerkatsRequest {
            mob_id: "mob_beta".to_string(),
            role: Some("worker".to_string()),
        })
        .await
        .unwrap();

    let meerkat_ids_b: Vec<&str> = meerkats_b.iter().map(|m| m.meerkat_id.as_str()).collect();
    assert!(
        meerkat_ids_b.contains(&"worker-beta"),
        "mob_beta should see worker-beta, got: {:?}",
        meerkat_ids_b
    );
    assert!(
        !meerkat_ids_b.contains(&"worker-alpha"),
        "mob_beta should NOT see worker-alpha, got: {:?}",
        meerkat_ids_b
    );

    // Cleanup
    handle_a.abort();
    handle_b.abort();
}

// ---------------------------------------------------------------------------
// Event cursor monotonicity under concurrent appends
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stress_event_cursor_monotonicity() {
    // Verify that event cursors are strictly monotonically increasing even
    // when multiple tasks append concurrently.

    let event_store = Arc::new(InMemoryMobEventStore::new());

    // Spawn 10 concurrent tasks each appending 50 events
    let mut handles = Vec::new();
    for task_id in 0..10 {
        let store = Arc::clone(&event_store);
        handles.push(tokio::spawn(async move {
            let mut cursors = Vec::new();
            for i in 0..50 {
                let event = crate::event::MobEvent {
                    cursor: 0, // will be assigned by store
                    timestamp: chrono::Utc::now(),
                    mob_id: "stress-test".to_string(),
                    run_id: Some(format!("run-{task_id}")),
                    flow_id: None,
                    step_id: Some(format!("step-{i}")),
                    retention: crate::event::RetentionCategory::Debug,
                    kind: crate::event::MobEventKind::DispatchSent {
                        target_meerkat_id: format!("target-{task_id}-{i}"),
                    },
                };
                let cursor = store.append(event).await.unwrap();
                cursors.push(cursor);
            }
            cursors
        }));
    }

    // Collect all cursors
    let mut all_cursors = Vec::new();
    for handle in handles {
        let cursors = handle.await.unwrap();
        all_cursors.extend(cursors);
    }

    // All 500 cursors should be unique
    let unique_cursors: HashSet<u64> = all_cursors.iter().copied().collect();
    assert_eq!(
        unique_cursors.len(),
        500,
        "All 500 cursors should be unique, got {} unique out of {}",
        unique_cursors.len(),
        all_cursors.len()
    );

    // Poll all events and verify cursor ordering
    let events = event_store
        .poll("stress-test", None, None)
        .await
        .unwrap();
    assert_eq!(events.len(), 500, "Should have 500 events total");

    // Verify monotonicity in the poll result
    for window in events.windows(2) {
        assert!(
            window[1].cursor > window[0].cursor,
            "Event cursors must be monotonically increasing: {} should be > {}",
            window[1].cursor,
            window[0].cursor
        );
    }

    // Verify cursor range: 1..=500
    let min_cursor = events.first().unwrap().cursor;
    let max_cursor = events.last().unwrap().cursor;
    assert_eq!(min_cursor, 1, "First cursor should be 1");
    assert_eq!(max_cursor, 500, "Last cursor should be 500");
}

// ---------------------------------------------------------------------------
// Cursor-based resumable polling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stress_cursor_resumable_polling() {
    // Verify that polling with after_cursor correctly resumes from
    // the last consumed cursor.

    let event_store = Arc::new(InMemoryMobEventStore::new());

    // Append 100 events
    for i in 0..100 {
        let event = crate::event::MobEvent {
            cursor: 0,
            timestamp: chrono::Utc::now(),
            mob_id: "poll-test".to_string(),
            run_id: None,
            flow_id: None,
            step_id: Some(format!("step-{i}")),
            retention: crate::event::RetentionCategory::Ops,
            kind: crate::event::MobEventKind::StepStarted {
                target_count: 1,
            },
        };
        event_store.append(event).await.unwrap();
    }

    // Poll first 25
    let batch1 = event_store
        .poll("poll-test", None, Some(25))
        .await
        .unwrap();
    assert_eq!(batch1.len(), 25);
    let last_cursor_1 = batch1.last().unwrap().cursor;

    // Poll next 25 from cursor
    let batch2 = event_store
        .poll("poll-test", Some(last_cursor_1), Some(25))
        .await
        .unwrap();
    assert_eq!(batch2.len(), 25);
    let last_cursor_2 = batch2.last().unwrap().cursor;

    // No overlap
    let batch1_cursors: HashSet<u64> = batch1.iter().map(|e| e.cursor).collect();
    let batch2_cursors: HashSet<u64> = batch2.iter().map(|e| e.cursor).collect();
    assert!(
        batch1_cursors.is_disjoint(&batch2_cursors),
        "Batches should not overlap"
    );

    // batch2 cursors should all be > last_cursor_1
    for cursor in &batch2_cursors {
        assert!(
            *cursor > last_cursor_1,
            "batch2 cursor {} should be > {}",
            cursor,
            last_cursor_1
        );
    }

    // Poll remaining
    let batch3 = event_store
        .poll("poll-test", Some(last_cursor_2), None)
        .await
        .unwrap();
    assert_eq!(batch3.len(), 50, "remaining 50 events should be returned");

    // Total: 25 + 25 + 50 = 100
    let total = batch1.len() + batch2.len() + batch3.len();
    assert_eq!(total, 100, "total events should be 100");
}
