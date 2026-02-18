//! Unit tests for types, validation, parsing, and stores.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone
)]

use chrono::Utc;

use crate::error::MobError;
use crate::event::{MobEvent, MobEventKind, RetentionCategory};
use crate::run::{
    FailureLedgerEntry, MobRun, RunStatus, StepEntryStatus, StepLedgerEntry,
};
use crate::spec::*;
use crate::store::{
    InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore, MobEventStore, MobRunStore,
    MobSpecStore,
};
use crate::validate::{DiagnosticCode, ValidateOptions, validate_spec};

// ---------------------------------------------------------------------------
// Helper: minimal valid spec TOML
// ---------------------------------------------------------------------------

const MINIMAL_SPEC_TOML: &str = r#"
[mob.specs.code_review]
roles = [
    { role = "coordinator", prompt_inline = "You are a coordinator.", cardinality = { kind = "singleton" } },
    { role = "reviewer", prompt_ref = "config://prompts/review_prompt", cardinality = { kind = "singleton" } },
]

[mob.specs.code_review.topology]
mode = "advisory"
ad_hoc_mode = "advisory"
default_action = "allow"

[[mob.specs.code_review.flows]]
flow_id = "triage"
steps = [
    { step_id = "dispatch", depends_on = [], targets = { role = "reviewer" }, dispatch_mode = "fan_out", timeout_ms = 30000 },
    { step_id = "collect", depends_on = ["dispatch"], targets = { role = "coordinator" }, dispatch_mode = "one_to_one", timeout_ms = 60000 },
]

[mob.specs.code_review.prompts]
review_prompt = "You are a code reviewer. Analyze the provided code."

[mob.specs.code_review.limits]
max_steps_per_flow = 50
max_flow_depth = 20
max_concurrent_ready_steps = 10
max_concurrent_runs = 5
"#;

fn parse_minimal_spec() -> MobSpec {
    let specs = parse_mob_specs_from_toml(MINIMAL_SPEC_TOML).unwrap();
    assert_eq!(specs.len(), 1);
    specs.into_iter().next().unwrap()
}

// ---------------------------------------------------------------------------
// P1-T20: TOML parsing
// ---------------------------------------------------------------------------

#[test]
fn test_parse_valid_spec() {
    let spec = parse_minimal_spec();

    assert_eq!(spec.mob_id, "code_review");
    assert_eq!(spec.roles.len(), 2);
    assert_eq!(spec.flows.len(), 1);

    // Roles
    let coord = &spec.roles[0];
    assert_eq!(coord.role, "coordinator");
    assert_eq!(
        coord.prompt_inline.as_deref(),
        Some("You are a coordinator.")
    );
    assert_eq!(coord.cardinality, CardinalitySpec::Singleton);

    let reviewer = &spec.roles[1];
    assert_eq!(reviewer.role, "reviewer");
    assert_eq!(
        reviewer.prompt_ref.as_deref(),
        Some("config://prompts/review_prompt")
    );

    // Flows
    let flow = &spec.flows[0];
    assert_eq!(flow.flow_id, "triage");
    assert_eq!(flow.steps.len(), 2);
    assert_eq!(flow.steps[0].step_id, "dispatch");
    assert!(flow.steps[0].depends_on.is_empty());
    assert_eq!(flow.steps[0].dispatch_mode, DispatchMode::FanOut);
    assert_eq!(flow.steps[1].step_id, "collect");
    assert_eq!(flow.steps[1].depends_on, vec!["dispatch"]);

    // Topology
    assert_eq!(spec.topology.mode, TopologyMode::Advisory);
    assert_eq!(spec.topology.ad_hoc_mode, AdHocMode::Advisory);

    // Prompts
    assert!(spec.prompts.contains_key("review_prompt"));

    // Limits
    assert_eq!(spec.limits.max_steps_per_flow, 50);
    assert_eq!(spec.limits.max_concurrent_ready_steps, 10);
}

#[test]
fn test_parse_spec_defaults() {
    // Minimal TOML that relies on defaults
    let toml = r#"
[mob.specs.minimal]
roles = [{ role = "worker", prompt_inline = "work" }]

[[mob.specs.minimal.flows]]
flow_id = "main"
steps = [{ step_id = "do_work", targets = { role = "worker" } }]
"#;
    let specs = parse_mob_specs_from_toml(toml).unwrap();
    let spec = &specs[0];

    // Check defaults
    assert_eq!(spec.apply_mode, ApplyMode::DrainReplace);
    assert_eq!(spec.topology.mode, TopologyMode::Advisory);
    assert_eq!(spec.limits.max_steps_per_flow, 50);
    assert_eq!(spec.supervisor.enabled, true);
    assert_eq!(spec.retention.audit_ttl_secs, 7 * 24 * 3600);

    // Step defaults
    let step = &spec.flows[0].steps[0];
    assert_eq!(step.dispatch_mode, DispatchMode::FanOut);
    assert_eq!(step.collection_policy.mode, CollectionMode::All);
    assert_eq!(
        step.collection_policy.timeout_behavior,
        TimeoutBehavior::Partial
    );
    assert_eq!(step.timeout_ms, 30_000);
    assert_eq!(step.retry.attempts, 3);
    assert_eq!(step.schema_policy, SchemaPolicy::RetryThenFail);
}

#[test]
fn test_parse_invalid_toml() {
    let result = parse_mob_specs_from_toml("not valid toml {{{");
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// P1-T21: DAG acyclicity validation
// ---------------------------------------------------------------------------

#[test]
fn test_validate_dag_cycle_rejected() {
    let toml = r#"
[mob.specs.cyclic]
roles = [{ role = "worker", prompt_inline = "work" }]

[[mob.specs.cyclic.flows]]
flow_id = "loopy"
steps = [
    { step_id = "a", depends_on = ["c"], targets = { role = "worker" } },
    { step_id = "b", depends_on = ["a"], targets = { role = "worker" } },
    { step_id = "c", depends_on = ["b"], targets = { role = "worker" } },
]
"#;
    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let diags = validate_spec(&spec, &ValidateOptions::default());

    let cycle_diags: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::DagCycle)
        .collect();
    assert!(
        !cycle_diags.is_empty(),
        "Cyclic DAG should produce a DagCycle diagnostic, got: {diags:?}"
    );
}

#[test]
fn test_validate_dag_acyclic_accepted() {
    let spec = parse_minimal_spec();
    let diags = validate_spec(&spec, &ValidateOptions::default());

    let cycle_diags: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::DagCycle)
        .collect();
    assert!(
        cycle_diags.is_empty(),
        "Acyclic DAG should not produce DagCycle diagnostics, got: {cycle_diags:?}"
    );
}

// ---------------------------------------------------------------------------
// P1-T22: fan_in rejection
// ---------------------------------------------------------------------------

#[test]
fn test_validate_fan_in_rejected() {
    let toml = r#"
[mob.specs.bad_fanin]
roles = [{ role = "worker", prompt_inline = "work" }]

[[mob.specs.bad_fanin.flows]]
flow_id = "bad"
steps = [
    { step_id = "gather", targets = { role = "worker" }, dispatch_mode = "fan_in" },
]
"#;
    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let diags = validate_spec(&spec, &ValidateOptions::default());

    let fan_in_diags: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::FanInRejected)
        .collect();
    assert!(
        !fan_in_diags.is_empty(),
        "fan_in dispatch mode should produce FanInRejected diagnostic, got: {diags:?}"
    );
}

// ---------------------------------------------------------------------------
// P1-T23: limits enforcement
// ---------------------------------------------------------------------------

#[test]
fn test_validate_limits_exceeded() {
    let toml = r#"
[mob.specs.too_many_steps]
roles = [{ role = "worker", prompt_inline = "work" }]

[mob.specs.too_many_steps.limits]
max_steps_per_flow = 2

[[mob.specs.too_many_steps.flows]]
flow_id = "overflow"
steps = [
    { step_id = "s1", targets = { role = "worker" } },
    { step_id = "s2", depends_on = ["s1"], targets = { role = "worker" } },
    { step_id = "s3", depends_on = ["s2"], targets = { role = "worker" } },
]
"#;
    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let diags = validate_spec(&spec, &ValidateOptions::default());

    let limit_diags: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::LimitExceeded)
        .collect();
    assert!(
        !limit_diags.is_empty(),
        "Exceeding max_steps_per_flow should produce LimitExceeded diagnostic, got: {diags:?}"
    );
}

// ---------------------------------------------------------------------------
// P1-T24: prompt ref resolution
// ---------------------------------------------------------------------------

#[test]
fn test_prompt_ref_resolution() {
    // Valid config:// ref
    let spec = parse_minimal_spec();
    let diags = validate_spec(&spec, &ValidateOptions::default());
    let unresolved: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnresolvedPromptRef)
        .collect();
    assert!(
        unresolved.is_empty(),
        "Valid prompt ref should not produce UnresolvedPromptRef, got: {unresolved:?}"
    );

    // Direct resolution
    let resolved = crate::validate::resolve_prompt_ref(
        "config://prompts/review_prompt",
        &spec.prompts,
    );
    assert_eq!(
        resolved.as_deref(),
        Some("You are a code reviewer. Analyze the provided code.")
    );

    // Missing ref
    let resolved_missing =
        crate::validate::resolve_prompt_ref("config://prompts/nonexistent", &spec.prompts);
    assert!(resolved_missing.is_none());
}

#[test]
fn test_prompt_ref_unresolved() {
    let toml = r#"
[mob.specs.bad_ref]
roles = [{ role = "worker", prompt_ref = "config://prompts/missing_prompt" }]

[[mob.specs.bad_ref.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "worker" } }]
"#;
    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let diags = validate_spec(&spec, &ValidateOptions::default());

    let unresolved: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::UnresolvedPromptRef)
        .collect();
    assert!(
        !unresolved.is_empty(),
        "Missing prompt ref should produce UnresolvedPromptRef, got: {diags:?}"
    );
}

#[test]
fn test_file_ref_without_context() {
    let toml = r#"
[mob.specs.file_ref]
roles = [{ role = "worker", prompt_ref = "file://prompts/system.md" }]

[[mob.specs.file_ref.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "worker" } }]
"#;
    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let diags = validate_spec(&spec, &ValidateOptions::default());

    let file_diags: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::FileRefWithoutContext)
        .collect();
    assert!(
        !file_diags.is_empty(),
        "file:// ref without base_dir should produce FileRefWithoutContext, got: {diags:?}"
    );
}

// ---------------------------------------------------------------------------
// Validation: dependency references
// ---------------------------------------------------------------------------

#[test]
fn test_validate_invalid_dependency() {
    let toml = r#"
[mob.specs.bad_dep]
roles = [{ role = "worker", prompt_inline = "work" }]

[[mob.specs.bad_dep.flows]]
flow_id = "main"
steps = [
    { step_id = "s1", depends_on = ["nonexistent"], targets = { role = "worker" } },
]
"#;
    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let diags = validate_spec(&spec, &ValidateOptions::default());

    let dep_diags: Vec<_> = diags
        .iter()
        .filter(|d| d.code == DiagnosticCode::InvalidDependency)
        .collect();
    assert!(
        !dep_diags.is_empty(),
        "Invalid dependency should produce InvalidDependency, got: {diags:?}"
    );
}

// ---------------------------------------------------------------------------
// Validation: duplicate names
// ---------------------------------------------------------------------------

#[test]
fn test_validate_duplicate_role() {
    let toml = r#"
[mob.specs.dup_role]
roles = [
    { role = "worker", prompt_inline = "a" },
    { role = "worker", prompt_inline = "b" },
]

[[mob.specs.dup_role.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "worker" } }]
"#;
    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let diags = validate_spec(&spec, &ValidateOptions::default());

    assert!(
        diags.iter().any(|d| d.code == DiagnosticCode::DuplicateRole),
        "Duplicate role should produce DuplicateRole, got: {diags:?}"
    );
}

#[test]
fn test_validate_duplicate_flow() {
    let toml = r#"
[mob.specs.dup_flow]
roles = [{ role = "worker", prompt_inline = "a" }]

[[mob.specs.dup_flow.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "worker" } }]

[[mob.specs.dup_flow.flows]]
flow_id = "main"
steps = [{ step_id = "s2", targets = { role = "worker" } }]
"#;
    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let diags = validate_spec(&spec, &ValidateOptions::default());

    assert!(
        diags
            .iter()
            .any(|d| d.code == DiagnosticCode::DuplicateFlow),
        "Duplicate flow should produce DuplicateFlow, got: {diags:?}"
    );
}

#[test]
fn test_validate_duplicate_step() {
    let toml = r#"
[mob.specs.dup_step]
roles = [{ role = "worker", prompt_inline = "a" }]

[[mob.specs.dup_step.flows]]
flow_id = "main"
steps = [
    { step_id = "s1", targets = { role = "worker" } },
    { step_id = "s1", targets = { role = "worker" } },
]
"#;
    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let diags = validate_spec(&spec, &ValidateOptions::default());

    assert!(
        diags
            .iter()
            .any(|d| d.code == DiagnosticCode::DuplicateStep),
        "Duplicate step should produce DuplicateStep, got: {diags:?}"
    );
}

// ---------------------------------------------------------------------------
// Validation: target role resolution
// ---------------------------------------------------------------------------

#[test]
fn test_validate_unresolved_target_role() {
    let toml = r#"
[mob.specs.bad_target]
roles = [{ role = "worker", prompt_inline = "a" }]

[[mob.specs.bad_target.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "nonexistent_role" } }]
"#;
    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let diags = validate_spec(&spec, &ValidateOptions::default());

    assert!(
        diags
            .iter()
            .any(|d| d.code == DiagnosticCode::UnresolvedTargetRole),
        "Nonexistent target role should produce UnresolvedTargetRole, got: {diags:?}"
    );
}

// ---------------------------------------------------------------------------
// Valid spec produces no diagnostics
// ---------------------------------------------------------------------------

#[test]
fn test_validate_valid_spec_clean() {
    let spec = parse_minimal_spec();
    let diags = validate_spec(&spec, &ValidateOptions::default());
    assert!(
        diags.is_empty(),
        "Valid spec should produce no diagnostics, got: {diags:?}"
    );
}

// ---------------------------------------------------------------------------
// Run status transitions
// ---------------------------------------------------------------------------

#[test]
fn test_run_status_transitions() {
    assert!(RunStatus::Pending.can_transition_to(RunStatus::Running));
    assert!(RunStatus::Pending.can_transition_to(RunStatus::Cancelled));
    assert!(RunStatus::Running.can_transition_to(RunStatus::Completed));
    assert!(RunStatus::Running.can_transition_to(RunStatus::Failed));
    assert!(RunStatus::Running.can_transition_to(RunStatus::Cancelled));

    // Invalid transitions
    assert!(!RunStatus::Completed.can_transition_to(RunStatus::Running));
    assert!(!RunStatus::Failed.can_transition_to(RunStatus::Running));
    assert!(!RunStatus::Cancelled.can_transition_to(RunStatus::Running));
    assert!(!RunStatus::Pending.can_transition_to(RunStatus::Completed));
    assert!(!RunStatus::Pending.can_transition_to(RunStatus::Failed));
}

#[test]
fn test_run_status_is_terminal() {
    assert!(!RunStatus::Pending.is_terminal());
    assert!(!RunStatus::Running.is_terminal());
    assert!(RunStatus::Completed.is_terminal());
    assert!(RunStatus::Failed.is_terminal());
    assert!(RunStatus::Cancelled.is_terminal());
}

// ===========================================================================
// Store CRUD tests
// ===========================================================================

// ---------------------------------------------------------------------------
// InMemoryMobSpecStore
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_spec_store_crud() {
    let store = InMemoryMobSpecStore::new();

    // Create
    let spec = parse_minimal_spec();
    let stored = store.put(spec.clone()).await.unwrap();
    assert_eq!(stored.mob_id, "code_review");
    assert_eq!(stored.spec_revision, 1);

    // Read
    let fetched = store.get("code_review").await.unwrap().unwrap();
    assert_eq!(fetched.mob_id, "code_review");
    assert_eq!(fetched.spec_revision, 1);
    assert_eq!(fetched.roles.len(), 2);

    // List
    let all = store.list().await.unwrap();
    assert_eq!(all.len(), 1);

    // Update (CAS success)
    let mut updated_spec = fetched.clone();
    updated_spec.supervisor.dispatch_timeout_ms = 120_000;
    let updated = store.put(updated_spec).await.unwrap();
    assert_eq!(updated.spec_revision, 2);

    // Update (CAS conflict)
    let mut stale_spec = fetched; // still at revision 1
    stale_spec.supervisor.dispatch_timeout_ms = 90_000;
    let conflict_result = store.put(stale_spec).await;
    assert!(
        matches!(conflict_result, Err(MobError::SpecConflict { .. })),
        "Stale revision should produce SpecConflict, got: {conflict_result:?}"
    );

    // Get nonexistent
    let missing = store.get("nonexistent").await.unwrap();
    assert!(missing.is_none());

    // Delete
    let deleted = store.delete("code_review").await.unwrap();
    assert!(deleted);
    let after_delete = store.get("code_review").await.unwrap();
    assert!(after_delete.is_none());

    // Delete nonexistent
    let deleted_again = store.delete("code_review").await.unwrap();
    assert!(!deleted_again);
}

// ---------------------------------------------------------------------------
// InMemoryMobRunStore
// ---------------------------------------------------------------------------

fn make_test_run(run_id: &str) -> MobRun {
    let now = Utc::now();
    MobRun {
        run_id: run_id.to_string(),
        mob_id: "test_mob".to_string(),
        flow_id: "test_flow".to_string(),
        spec_revision: 1,
        status: RunStatus::Pending,
        step_ledger: Vec::new(),
        failure_ledger: Vec::new(),
        created_at: now,
        updated_at: now,
    }
}

#[tokio::test]
async fn test_run_store_crud() {
    let store = InMemoryMobRunStore::new();

    // Create
    let run = make_test_run("run-1");
    store.create(run.clone()).await.unwrap();

    // Read
    let fetched = store.get("run-1").await.unwrap().unwrap();
    assert_eq!(fetched.run_id, "run-1");
    assert_eq!(fetched.status, RunStatus::Pending);

    // List
    let all = store.list(None, None).await.unwrap();
    assert_eq!(all.len(), 1);

    // List with filters
    let by_mob = store.list(Some("test_mob"), None).await.unwrap();
    assert_eq!(by_mob.len(), 1);
    let by_other_mob = store.list(Some("other_mob"), None).await.unwrap();
    assert_eq!(by_other_mob.len(), 0);
    let by_status = store.list(None, Some(RunStatus::Pending)).await.unwrap();
    assert_eq!(by_status.len(), 1);
    let by_running = store.list(None, Some(RunStatus::Running)).await.unwrap();
    assert_eq!(by_running.len(), 0);

    // Duplicate create
    let dup_result = store.create(run).await;
    assert!(matches!(dup_result, Err(MobError::StoreError(_))));

    // Get nonexistent
    let missing = store.get("nonexistent").await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn test_run_store_cas_transitions() {
    let store = InMemoryMobRunStore::new();
    store.create(make_test_run("run-cas")).await.unwrap();

    // Valid transition: Pending -> Running
    store
        .cas_status("run-cas", RunStatus::Pending, RunStatus::Running)
        .await
        .unwrap();
    let run = store.get("run-cas").await.unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Running);

    // Invalid: wrong expected status (Pending, but actual is Running)
    let result = store
        .cas_status("run-cas", RunStatus::Pending, RunStatus::Completed)
        .await;
    assert!(
        matches!(result, Err(MobError::InvalidTransition { .. })),
        "Wrong expected status should produce InvalidTransition, got: {result:?}"
    );

    // Invalid: bad transition (Running -> Pending is not valid)
    let result = store
        .cas_status("run-cas", RunStatus::Running, RunStatus::Pending)
        .await;
    assert!(
        matches!(result, Err(MobError::InvalidTransition { .. })),
        "Invalid transition should produce InvalidTransition, got: {result:?}"
    );

    // Valid transition: Running -> Completed
    store
        .cas_status("run-cas", RunStatus::Running, RunStatus::Completed)
        .await
        .unwrap();
    let run = store.get("run-cas").await.unwrap().unwrap();
    assert_eq!(run.status, RunStatus::Completed);

    // CAS on nonexistent run
    let result = store
        .cas_status("nonexistent", RunStatus::Pending, RunStatus::Running)
        .await;
    assert!(matches!(result, Err(MobError::RunNotFound { .. })));
}

#[tokio::test]
async fn test_run_store_ledger_append() {
    let store = InMemoryMobRunStore::new();
    store.create(make_test_run("run-ledger")).await.unwrap();

    // Append step entry
    let step_entry = StepLedgerEntry {
        step_id: "step-1".to_string(),
        target_meerkat_id: "meerkat-a".to_string(),
        status: StepEntryStatus::Dispatched,
        attempt: 1,
        dispatched_at: Some(Utc::now()),
        completed_at: None,
        result: None,
        error: None,
    };
    store
        .append_step_entry("run-ledger", step_entry)
        .await
        .unwrap();

    let run = store.get("run-ledger").await.unwrap().unwrap();
    assert_eq!(run.step_ledger.len(), 1);
    assert_eq!(run.step_ledger[0].step_id, "step-1");

    // Append failure entry
    let failure_entry = FailureLedgerEntry {
        step_id: "step-1".to_string(),
        target_meerkat_id: "meerkat-a".to_string(),
        attempt: 1,
        error: "timeout".to_string(),
        failed_at: Utc::now(),
    };
    store
        .append_failure_entry("run-ledger", failure_entry)
        .await
        .unwrap();

    let run = store.get("run-ledger").await.unwrap().unwrap();
    assert_eq!(run.failure_ledger.len(), 1);
    assert_eq!(run.failure_ledger[0].error, "timeout");

    // Append to nonexistent run
    let result = store
        .append_step_entry(
            "nonexistent",
            StepLedgerEntry {
                step_id: "s".to_string(),
                target_meerkat_id: "m".to_string(),
                status: StepEntryStatus::Pending,
                attempt: 1,
                dispatched_at: None,
                completed_at: None,
                result: None,
                error: None,
            },
        )
        .await;
    assert!(matches!(result, Err(MobError::RunNotFound { .. })));
}

// ---------------------------------------------------------------------------
// InMemoryMobEventStore
// ---------------------------------------------------------------------------

fn make_test_event(mob_id: &str, kind: MobEventKind) -> MobEvent {
    MobEvent {
        cursor: 0, // assigned by store
        timestamp: Utc::now(),
        mob_id: mob_id.to_string(),
        run_id: Some("run-1".to_string()),
        flow_id: Some("flow-1".to_string()),
        step_id: None,
        retention: RetentionCategory::Ops,
        kind,
    }
}

#[tokio::test]
async fn test_event_store_crud() {
    let store = InMemoryMobEventStore::new();

    // Append
    let cursor1 = store
        .append(make_test_event("mob-1", MobEventKind::RunStarted))
        .await
        .unwrap();
    assert_eq!(cursor1, 1);

    let cursor2 = store
        .append(make_test_event("mob-1", MobEventKind::RunCompleted))
        .await
        .unwrap();
    assert_eq!(cursor2, 2);

    let cursor3 = store
        .append(make_test_event("mob-2", MobEventKind::RunStarted))
        .await
        .unwrap();
    assert_eq!(cursor3, 3);

    // Poll all for mob-1
    let events = store.poll("mob-1", None, None).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].cursor, 1);
    assert_eq!(events[1].cursor, 2);

    // Poll after cursor 1
    let events_after = store.poll("mob-1", Some(1), None).await.unwrap();
    assert_eq!(events_after.len(), 1);
    assert_eq!(events_after[0].cursor, 2);

    // Poll with limit
    let events_limited = store.poll("mob-1", None, Some(1)).await.unwrap();
    assert_eq!(events_limited.len(), 1);

    // Poll for mob-2
    let events_mob2 = store.poll("mob-2", None, None).await.unwrap();
    assert_eq!(events_mob2.len(), 1);

    // Poll for nonexistent mob
    let events_missing = store.poll("nonexistent", None, None).await.unwrap();
    assert!(events_missing.is_empty());
}

#[tokio::test]
async fn test_event_store_cursor_monotonic() {
    let store = InMemoryMobEventStore::new();

    let mut cursors = Vec::new();
    for _ in 0..10 {
        let cursor = store
            .append(make_test_event("mob-1", MobEventKind::RunStarted))
            .await
            .unwrap();
        cursors.push(cursor);
    }

    // Verify monotonically increasing
    for window in cursors.windows(2) {
        assert!(window[1] > window[0], "Cursors must be monotonically increasing");
    }
}

#[tokio::test]
async fn test_event_store_prune() {
    let store = InMemoryMobEventStore::new();

    // Insert events
    store
        .append(make_test_event("mob-1", MobEventKind::RunStarted))
        .await
        .unwrap();
    store
        .append(make_test_event("mob-1", MobEventKind::RunCompleted))
        .await
        .unwrap();

    // Prune with future timestamp (removes everything)
    let pruned = store
        .prune_before(Utc::now() + chrono::Duration::hours(1))
        .await
        .unwrap();
    assert_eq!(pruned, 2);

    // Verify empty
    let events = store.poll("mob-1", None, None).await.unwrap();
    assert!(events.is_empty());
}
