//! Integration chokepoint test scaffolding (Phase 1 + Phase 2 + Phase 3).
//!
//! These tests compile and run. Phase 1 green tests remain green.
//! Phase 2 tests now exercise real runtime behavior.
//! Phase 3 tests cover topology enforcement, reconcile engine, tool composition,
//! event emission coverage, and supervisor integration.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone
)]

use chrono::Utc;
use std::collections::HashSet;
use std::sync::Arc;
use uuid::Uuid;

use crate::event::{MobEventKind, RetentionCategory};
use crate::resolver::{MeerkatIdentity, MeerkatResolver, ResolverContext};
use crate::run::{MobRun, RunStatus, StepEntryStatus};
use crate::runtime::{compile_role_config, DagScheduler, MobRuntime, MobRuntimeConfig};
use crate::service::{
    ActivateRequest, ApplySpecRequest, ListMeerkatsRequest, MobService, PollEventsRequest,
    ReconcileMode, ReconcileRequest,
};
use crate::spec::parse_mob_specs_from_toml;
use crate::store::{
    InMemoryMobEventStore, InMemoryMobRunStore, InMemoryMobSpecStore, MobRunStore, MobSpecStore,
};
use crate::validate::{ValidateOptions, validate_spec};

// ---------------------------------------------------------------------------
// CHOKE-MOB-001: Spec parse -> MobSpecStore round-trip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn choke_mob_001_spec_store_roundtrip() {
    // Parse TOML to MobSpec
    let toml = r#"
[mob.specs.review_mob]
roles = [
    { role = "coordinator", prompt_inline = "You coordinate reviews." },
    { role = "reviewer", prompt_ref = "config://prompts/review", cardinality = { kind = "singleton" } },
]

[mob.specs.review_mob.topology]
mode = "advisory"

[[mob.specs.review_mob.flows]]
flow_id = "triage"
steps = [
    { step_id = "dispatch", targets = { role = "reviewer" }, dispatch_mode = "fan_out" },
    { step_id = "collect", depends_on = ["dispatch"], targets = { role = "coordinator" }, dispatch_mode = "one_to_one" },
]

[mob.specs.review_mob.prompts]
review = "Review the code carefully."

[mob.specs.review_mob.limits]
max_steps_per_flow = 50
"#;

    let mut specs = parse_mob_specs_from_toml(toml).unwrap();
    assert_eq!(specs.len(), 1);
    let original = specs.remove(0);

    // Validate
    let diags = validate_spec(&original, &ValidateOptions::default());
    assert!(diags.is_empty(), "Spec should be valid: {diags:?}");

    // Store
    let store = InMemoryMobSpecStore::new();
    let stored = store.put(original.clone()).await.unwrap();
    assert_eq!(stored.spec_revision, 1);

    // Retrieve and compare
    let retrieved = store.get("review_mob").await.unwrap().unwrap();

    // All fields must survive the round-trip
    assert_eq!(retrieved.mob_id, original.mob_id);
    assert_eq!(retrieved.roles.len(), original.roles.len());
    assert_eq!(retrieved.flows.len(), original.flows.len());
    assert_eq!(retrieved.topology, original.topology);
    assert_eq!(retrieved.prompts, original.prompts);
    assert_eq!(retrieved.limits, original.limits);
    assert_eq!(retrieved.supervisor, original.supervisor);
    assert_eq!(retrieved.retention, original.retention);

    // Deep field check on roles
    for (r, o) in retrieved.roles.iter().zip(original.roles.iter()) {
        assert_eq!(r.role, o.role);
        assert_eq!(r.prompt_ref, o.prompt_ref);
        assert_eq!(r.prompt_inline, o.prompt_inline);
        assert_eq!(r.cardinality, o.cardinality);
        assert_eq!(r.spawn_strategy, o.spawn_strategy);
        assert_eq!(r.model, o.model);
    }

    // Deep field check on flow steps
    for (rf, of) in retrieved.flows.iter().zip(original.flows.iter()) {
        assert_eq!(rf.flow_id, of.flow_id);
        for (rs, os) in rf.steps.iter().zip(of.steps.iter()) {
            assert_eq!(rs.step_id, os.step_id);
            assert_eq!(rs.depends_on, os.depends_on);
            assert_eq!(rs.targets, os.targets);
            assert_eq!(rs.dispatch_mode, os.dispatch_mode);
            assert_eq!(rs.collection_policy, os.collection_policy);
            assert_eq!(rs.timeout_ms, os.timeout_ms);
            assert_eq!(rs.retry, os.retry);
        }
    }
}

// ---------------------------------------------------------------------------
// CHOKE-MOB-002: Role config -> AgentBuildConfig compilation
// ---------------------------------------------------------------------------

#[test]
fn choke_mob_002_role_to_agent_build_config() {
    let toml = r#"
[mob.specs.build_test]
roles = [
    { role = "analyzer", prompt_inline = "Analyze code.", model = "claude-sonnet-4-5" },
]

[[mob.specs.build_test.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "analyzer" } }]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let role = &spec.roles[0];

    // Compile role to build config
    let config = compile_role_config(role, &spec, "test-realm", "meerkat-001");

    // Verify model
    assert_eq!(config.model.as_deref(), Some("claude-sonnet-4-5"));

    // Verify host_mode
    assert!(config.host_mode);

    // Verify comms_name format: {realm_id}/{mob_id}/{role}/{meerkat_id}
    assert_eq!(
        config.comms_name,
        "test-realm/build_test/analyzer/meerkat-001"
    );

    // Verify peer_meta labels
    assert_eq!(
        config.peer_meta.labels.get("mob_id").map(|s| s.as_str()),
        Some("build_test")
    );
    assert_eq!(
        config.peer_meta.labels.get("role").map(|s| s.as_str()),
        Some("analyzer")
    );
    assert_eq!(
        config.peer_meta.labels.get("meerkat_id").map(|s| s.as_str()),
        Some("meerkat-001")
    );

    // Verify resolved system prompt
    assert_eq!(config.system_prompt.as_deref(), Some("Analyze code."));
}

#[test]
fn choke_mob_002_role_with_prompt_ref() {
    let toml = r#"
[mob.specs.ref_test]
roles = [
    { role = "reviewer", prompt_ref = "config://prompts/review_prompt" },
]

[mob.specs.ref_test.prompts]
review_prompt = "Review the code carefully."

[[mob.specs.ref_test.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "reviewer" } }]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let role = &spec.roles[0];

    let config = compile_role_config(role, &spec, "realm", "mk-1");

    // Prompt ref should be resolved to the actual prompt content
    assert_eq!(
        config.system_prompt.as_deref(),
        Some("Review the code carefully.")
    );
}

// ---------------------------------------------------------------------------
// CHOKE-MOB-005: DAG scheduler step readiness
// ---------------------------------------------------------------------------

#[test]
fn choke_mob_005_dag_scheduler_step_readiness() {
    let toml = r#"
[mob.specs.dag_test]
roles = [{ role = "worker", prompt_inline = "work" }]

[[mob.specs.dag_test.flows]]
flow_id = "parallel"
steps = [
    { step_id = "a", targets = { role = "worker" } },
    { step_id = "b", targets = { role = "worker" } },
    { step_id = "c", depends_on = ["a", "b"], targets = { role = "worker" } },
]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let flow = &spec.flows[0];

    // Verify the flow structure
    assert_eq!(flow.steps.len(), 3);
    assert!(flow.steps[0].depends_on.is_empty()); // a: entry step
    assert!(flow.steps[1].depends_on.is_empty()); // b: entry step
    assert_eq!(flow.steps[2].depends_on, vec!["a", "b"]); // c: depends on both

    let scheduler = DagScheduler::new(flow);

    // No completed steps: a and b are ready (entry steps)
    let ready = scheduler.ready_steps(&HashSet::new(), &HashSet::new());
    assert_eq!(ready, vec!["a", "b"]);

    // Only a completed: b still ready, c not ready yet
    let completed_a = HashSet::from(["a".to_string()]);
    let ready2 = scheduler.ready_steps(&completed_a, &HashSet::new());
    assert_eq!(ready2, vec!["b"]);

    // Both a and b completed: c now ready
    let completed_ab = HashSet::from(["a".to_string(), "b".to_string()]);
    let ready3 = scheduler.ready_steps(&completed_ab, &HashSet::new());
    assert_eq!(ready3, vec!["c"]);

    // All completed: nothing ready
    let completed_all = HashSet::from(["a".to_string(), "b".to_string(), "c".to_string()]);
    let ready4 = scheduler.ready_steps(&completed_all, &HashSet::new());
    assert!(ready4.is_empty());

    // a in progress (not completed): only b is ready
    let in_progress_a = HashSet::from(["a".to_string()]);
    let ready5 = scheduler.ready_steps(&HashSet::new(), &in_progress_a);
    assert_eq!(ready5, vec!["b"]);
}

#[test]
fn choke_mob_005_dag_scheduler_linear_chain() {
    let toml = r#"
[mob.specs.chain_test]
roles = [{ role = "worker", prompt_inline = "work" }]

[[mob.specs.chain_test.flows]]
flow_id = "chain"
steps = [
    { step_id = "s1", targets = { role = "worker" } },
    { step_id = "s2", depends_on = ["s1"], targets = { role = "worker" } },
    { step_id = "s3", depends_on = ["s2"], targets = { role = "worker" } },
]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let flow = &spec.flows[0];
    let scheduler = DagScheduler::new(flow);

    // Only s1 is ready initially
    let ready = scheduler.ready_steps(&HashSet::new(), &HashSet::new());
    assert_eq!(ready, vec!["s1"]);

    // After s1: only s2
    let ready2 = scheduler.ready_steps(&HashSet::from(["s1".to_string()]), &HashSet::new());
    assert_eq!(ready2, vec!["s2"]);

    // After s1+s2: only s3
    let ready3 = scheduler.ready_steps(
        &HashSet::from(["s1".to_string(), "s2".to_string()]),
        &HashSet::new(),
    );
    assert_eq!(ready3, vec!["s3"]);
}

// ---------------------------------------------------------------------------
// CHOKE-MOB-007: Run store CAS transitions (should be green)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn choke_mob_007_run_store_cas_transitions() {
    let store = InMemoryMobRunStore::new();
    let now = Utc::now();

    // Create a run in Pending state
    let run = MobRun {
        run_id: "choke-007".to_string(),
        mob_id: "test".to_string(),
        flow_id: "flow-1".to_string(),
        spec_revision: 1,
        status: RunStatus::Pending,
        step_ledger: Vec::new(),
        failure_ledger: Vec::new(),
        created_at: now,
        updated_at: now,
    };
    store.create(run).await.unwrap();

    // CAS: Pending -> Running (valid)
    store
        .cas_status("choke-007", RunStatus::Pending, RunStatus::Running)
        .await
        .unwrap();
    let r = store.get("choke-007").await.unwrap().unwrap();
    assert_eq!(r.status, RunStatus::Running);

    // CAS: wrong expected (Pending, but actual is Running)
    let err = store
        .cas_status("choke-007", RunStatus::Pending, RunStatus::Completed)
        .await;
    assert!(err.is_err(), "CAS with wrong expected should fail");

    // CAS: Running -> Completed (valid)
    store
        .cas_status("choke-007", RunStatus::Running, RunStatus::Completed)
        .await
        .unwrap();
    let r = store.get("choke-007").await.unwrap().unwrap();
    assert_eq!(r.status, RunStatus::Completed);

    // CAS: terminal state cannot transition
    let err = store
        .cas_status("choke-007", RunStatus::Completed, RunStatus::Running)
        .await;
    assert!(
        err.is_err(),
        "CAS from terminal state should fail"
    );

    // Concurrent CAS simulation: create second run, two transitions race
    let run2 = MobRun {
        run_id: "choke-007-race".to_string(),
        mob_id: "test".to_string(),
        flow_id: "flow-1".to_string(),
        spec_revision: 1,
        status: RunStatus::Running,
        step_ledger: Vec::new(),
        failure_ledger: Vec::new(),
        created_at: now,
        updated_at: now,
    };
    store.create(run2).await.unwrap();

    // Both try Running -> Completed
    let result1 = store
        .cas_status("choke-007-race", RunStatus::Running, RunStatus::Completed)
        .await;
    let result2 = store
        .cas_status("choke-007-race", RunStatus::Running, RunStatus::Failed)
        .await;

    // One should succeed, one should fail (not concurrent here, but sequential
    // CAS proves the semantics)
    assert!(result1.is_ok(), "First CAS should succeed");
    assert!(result2.is_err(), "Second CAS should fail (status already changed)");
}

// ===========================================================================
// Phase 3: Topology, Reconcile, Supervisor, Events, Tool Composition
// ===========================================================================

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
// CHOKE-MOB-003: Topology enforcement during flow execution
// ---------------------------------------------------------------------------

#[tokio::test]
async fn choke_mob_003_topology_strict_blocks_dispatch() {
    // Scenario: Strict topology with deny rule supervisor->worker.
    // Activating a flow that dispatches to worker should fail because
    // the topology blocks the dispatch.

    let toml = r#"
[mob.specs.strict_topo]
roles = [
    { role = "worker", prompt_inline = "You work." },
]

[mob.specs.strict_topo.topology]
mode = "strict"
default_action = "deny"

[[mob.specs.strict_topo.flows]]
flow_id = "blocked"
steps = [
    { step_id = "s1", targets = { role = "worker" }, dispatch_mode = "fan_out", timeout_ms = 2000 },
]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);

    let (runtime, realm_id) = create_test_runtime("strict_topo");
    let namespace = format!("{realm_id}/strict_topo");

    // Apply spec
    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // Spawn a mock worker (it must exist so targets resolve)
    let name = format!("worker-1-{}", Uuid::new_v4().simple());
    let (mock_rt, mock_handle) = crate::runtime::spawn_mock_meerkat(
        &namespace,
        &name,
        "worker",
        "worker-1",
        runtime.supervisor(),
    );
    crate::runtime::establish_trust(runtime.supervisor(), &mock_rt).await;

    // Activate flow
    let result = runtime
        .activate(ActivateRequest {
            mob_id: "strict_topo".to_string(),
            flow_id: "blocked".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();

    // Wait for terminal state - should fail because topology blocks
    let run = wait_for_run_terminal(&runtime, &result.run_id, 10_000).await;
    assert_eq!(
        run.status,
        RunStatus::Failed,
        "Run should fail when topology blocks all targets, got: {:?}",
        run.status
    );

    // Verify TopologyBlocked event was emitted
    let events = runtime
        .poll_events(PollEventsRequest {
            mob_id: "strict_topo".to_string(),
            after_cursor: None,
            limit: None,
        })
        .await
        .unwrap();

    let blocked_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.kind, MobEventKind::TopologyBlocked { .. }))
        .collect();
    assert!(
        !blocked_events.is_empty(),
        "Should have emitted TopologyBlocked event(s), events: {:?}",
        events.iter().map(|e| &e.kind).collect::<Vec<_>>()
    );

    // Verify failure ledger has topology blocked entry
    assert!(
        !run.failure_ledger.is_empty(),
        "Failure ledger should have topology blocked entries"
    );
    assert!(
        run.failure_ledger[0].error.contains("topology blocked"),
        "Failure entry should mention topology blocked: {}",
        run.failure_ledger[0].error
    );

    mock_handle.abort();
}

#[tokio::test]
async fn choke_mob_003_topology_advisory_warns_but_proceeds() {
    // Scenario: Advisory topology with deny rule supervisor->worker.
    // Activating a flow that dispatches to worker should succeed but
    // emit TopologyViolation warning events.

    let toml = r#"
[mob.specs.advisory_topo]
roles = [
    { role = "worker", prompt_inline = "You work." },
]

[mob.specs.advisory_topo.topology]
mode = "advisory"
default_action = "deny"

[[mob.specs.advisory_topo.flows]]
flow_id = "warned"
steps = [
    { step_id = "s1", targets = { role = "worker" }, dispatch_mode = "fan_out", timeout_ms = 5000 },
]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);

    let (runtime, realm_id) = create_test_runtime("advisory_topo");
    let namespace = format!("{realm_id}/advisory_topo");

    // Apply spec
    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

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

    // Activate flow
    let result = runtime
        .activate(ActivateRequest {
            mob_id: "advisory_topo".to_string(),
            flow_id: "warned".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();

    // Wait for completion - should succeed in advisory mode
    let run = wait_for_run_terminal(&runtime, &result.run_id, 10_000).await;
    assert_eq!(
        run.status,
        RunStatus::Completed,
        "Run should complete in advisory mode, got: {:?}",
        run.status
    );

    // Verify TopologyViolation event was emitted (advisory warn)
    let events = runtime
        .poll_events(PollEventsRequest {
            mob_id: "advisory_topo".to_string(),
            after_cursor: None,
            limit: None,
        })
        .await
        .unwrap();

    let violation_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.kind, MobEventKind::TopologyViolation { .. }))
        .collect();
    assert!(
        !violation_events.is_empty(),
        "Should have emitted TopologyViolation event(s) in advisory mode"
    );

    mock_handle.abort();
}

// ---------------------------------------------------------------------------
// CHOKE-MOB-004: Reconcile engine (report_only and apply modes)
// ---------------------------------------------------------------------------

/// A simple resolver that returns a static list of meerkat identities.
struct StaticTestResolver {
    identities: Vec<MeerkatIdentity>,
}

#[async_trait::async_trait]
impl MeerkatResolver for StaticTestResolver {
    async fn list_meerkats(
        &self,
        _ctx: &ResolverContext,
    ) -> Result<Vec<MeerkatIdentity>, crate::error::MobError> {
        Ok(self.identities.clone())
    }
}

#[tokio::test]
async fn choke_mob_004_reconcile_report_only() {
    // Scenario: Singleton role with no meerkats registered.
    // Reconcile in report_only mode reports 1 to spawn, 0 to retire.
    // No actual changes are made.

    let toml = r#"
[mob.specs.recon_test]
roles = [
    { role = "worker", prompt_inline = "You work.", cardinality = { kind = "singleton" } },
]

[[mob.specs.recon_test.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "worker" } }]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);

    let (runtime, _realm_id) = create_test_runtime("recon_test");

    // Apply spec
    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // Reconcile in report_only mode
    let result = runtime
        .reconcile(ReconcileRequest {
            mob_id: "recon_test".to_string(),
            mode: ReconcileMode::ReportOnly,
        })
        .await
        .unwrap();

    assert_eq!(result.spawned, 1, "Should report 1 meerkat to spawn");
    assert_eq!(result.retired, 0, "Should report 0 meerkats to retire");
    assert_eq!(result.unchanged, 0, "Should report 0 unchanged");

    // Verify no actual meerkats were registered (report_only)
    let meerkats = runtime
        .list_meerkats(ListMeerkatsRequest {
            mob_id: "recon_test".to_string(),
            role: None,
        })
        .await
        .unwrap();

    // Only the supervisor should be in the namespace
    let worker_meerkats: Vec<_> = meerkats
        .iter()
        .filter(|m| m.role == "worker")
        .collect();
    assert!(
        worker_meerkats.is_empty(),
        "report_only should not spawn meerkats, found: {:?}",
        worker_meerkats
    );
}

#[tokio::test]
async fn choke_mob_004_reconcile_apply_spawns_missing() {
    // Scenario: Singleton role with no meerkats registered.
    // Reconcile in apply mode spawns 1 meerkat.

    let toml = r#"
[mob.specs.recon_apply]
roles = [
    { role = "worker", prompt_inline = "You work.", cardinality = { kind = "singleton" } },
]

[[mob.specs.recon_apply.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "worker" } }]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);

    let (runtime, _realm_id) = create_test_runtime("recon_apply");

    // Apply spec
    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // Reconcile in apply mode
    let result = runtime
        .reconcile(ReconcileRequest {
            mob_id: "recon_apply".to_string(),
            mode: ReconcileMode::Apply,
        })
        .await
        .unwrap();

    assert_eq!(result.spawned, 1, "Should spawn 1 meerkat");
    assert_eq!(result.retired, 0, "Should retire 0 meerkats");
    assert_eq!(result.spawned_ids, vec!["worker"]);

    // Verify the meerkat was actually registered
    let meerkats = runtime
        .list_meerkats(ListMeerkatsRequest {
            mob_id: "recon_apply".to_string(),
            role: Some("worker".to_string()),
        })
        .await
        .unwrap();

    assert_eq!(
        meerkats.len(),
        1,
        "Should have 1 worker meerkat after apply"
    );
    assert_eq!(meerkats[0].role, "worker");
}

#[tokio::test]
async fn choke_mob_004_reconcile_apply_idempotent() {
    // Scenario: Reconcile apply twice. Second call should report 0 spawned, 0 retired.

    let toml = r#"
[mob.specs.recon_idem]
roles = [
    { role = "worker", prompt_inline = "You work.", cardinality = { kind = "singleton" } },
]

[[mob.specs.recon_idem.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "worker" } }]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);

    let (runtime, _realm_id) = create_test_runtime("recon_idem");

    // Apply spec
    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // First reconcile
    let result1 = runtime
        .reconcile(ReconcileRequest {
            mob_id: "recon_idem".to_string(),
            mode: ReconcileMode::Apply,
        })
        .await
        .unwrap();
    assert_eq!(result1.spawned, 1);

    // Second reconcile (idempotent)
    let result2 = runtime
        .reconcile(ReconcileRequest {
            mob_id: "recon_idem".to_string(),
            mode: ReconcileMode::Apply,
        })
        .await
        .unwrap();

    assert_eq!(
        result2.spawned, 0,
        "Second reconcile should spawn 0 (idempotent)"
    );
    assert_eq!(
        result2.retired, 0,
        "Second reconcile should retire 0 (idempotent)"
    );
    assert_eq!(
        result2.unchanged, 1,
        "Second reconcile should report 1 unchanged"
    );
}

#[tokio::test]
async fn choke_mob_004_reconcile_per_meerkat_cardinality() {
    // Scenario: per_meerkat cardinality with a resolver returning 3 identities.
    // Reconcile apply should spawn 3 meerkats.

    let toml = r#"
[mob.specs.recon_pm]
roles = [
    { role = "scanner", prompt_inline = "You scan.", cardinality = { kind = "per_meerkat", resolver = "team_resolver" } },
]

[mob.specs.recon_pm.resolvers.team_resolver]
kind = { static = { identities = [
    { meerkat_id = "alice" },
    { meerkat_id = "bob" },
    { meerkat_id = "carol" },
] } }

[[mob.specs.recon_pm.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "scanner" } }]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);

    let (runtime, _realm_id) = create_test_runtime("recon_pm");

    // Apply spec
    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

    // Register the resolver
    let resolver = Arc::new(StaticTestResolver {
        identities: vec![
            MeerkatIdentity {
                meerkat_id: "alice".to_string(),
                role: "scanner".to_string(),
                labels: std::collections::BTreeMap::new(),
                attributes: serde_json::Value::Object(serde_json::Map::new()),
            },
            MeerkatIdentity {
                meerkat_id: "bob".to_string(),
                role: "scanner".to_string(),
                labels: std::collections::BTreeMap::new(),
                attributes: serde_json::Value::Object(serde_json::Map::new()),
            },
            MeerkatIdentity {
                meerkat_id: "carol".to_string(),
                role: "scanner".to_string(),
                labels: std::collections::BTreeMap::new(),
                attributes: serde_json::Value::Object(serde_json::Map::new()),
            },
        ],
    });

    runtime
        .register_resolver("team_resolver".to_string(), resolver)
        .await;

    // Reconcile in apply mode
    let result = runtime
        .reconcile(ReconcileRequest {
            mob_id: "recon_pm".to_string(),
            mode: ReconcileMode::Apply,
        })
        .await
        .unwrap();

    assert_eq!(result.spawned, 3, "Should spawn 3 meerkats");
    assert_eq!(result.retired, 0, "Should retire 0 meerkats");

    let mut spawned = result.spawned_ids.clone();
    spawned.sort();
    assert_eq!(spawned, vec!["alice", "bob", "carol"]);
}

// ---------------------------------------------------------------------------
// CHOKE-MOB-006: Tool composition in compile_role_config
// ---------------------------------------------------------------------------

#[test]
fn choke_mob_006_tool_composition_builtin_bundle() {
    let toml = r#"
[mob.specs.tool_test]
roles = [
    { role = "hacker", prompt_inline = "Hack.", tool_bundles = ["core_tools"] },
]

[mob.specs.tool_test.tool_bundles.core_tools]
kind = "builtin"

[[mob.specs.tool_test.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "hacker" } }]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let role = &spec.roles[0];

    let config = compile_role_config(role, &spec, "realm", "mk-1");

    assert!(config.tools.enable_builtins, "Builtin bundle should enable builtins");
    assert!(config.tools.enable_shell, "Builtin bundle should enable shell");
    assert!(config.tools.enable_subagents, "Builtin bundle should enable subagents");
    assert!(config.tools.enable_memory, "Builtin bundle should enable memory");
    assert!(config.tools.enable_comms, "Comms should always be enabled for mob meerkats");
}

#[test]
fn choke_mob_006_tool_composition_mcp_bundle() {
    let toml = r#"
[mob.specs.mcp_test]
roles = [
    { role = "worker", prompt_inline = "Work.", tool_bundles = ["github_mcp"] },
]

[mob.specs.mcp_test.tool_bundles.github_mcp]
kind = { mcp = { server = "github-tools" } }

[[mob.specs.mcp_test.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "worker" } }]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let role = &spec.roles[0];

    let config = compile_role_config(role, &spec, "realm", "mk-1");

    assert_eq!(config.tools.mcp_servers, vec!["github-tools"]);
    assert!(!config.tools.enable_builtins, "MCP bundle should not enable builtins");
}

#[test]
fn choke_mob_006_tool_composition_rust_bundle() {
    let toml = r#"
[mob.specs.rust_test]
roles = [
    { role = "worker", prompt_inline = "Work.", tool_bundles = ["custom_tools"] },
]

[mob.specs.rust_test.tool_bundles.custom_tools]
kind = { rust_bundle = { bundle_id = "my_custom_toolkit" } }

[[mob.specs.rust_test.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "worker" } }]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let role = &spec.roles[0];

    let config = compile_role_config(role, &spec, "realm", "mk-1");

    assert_eq!(config.tools.rust_bundles, vec!["my_custom_toolkit"]);
}

#[test]
fn choke_mob_006_tool_composition_tool_policy() {
    let toml = r#"
[mob.specs.policy_test]
roles = [
    { role = "worker", prompt_inline = "Work.", tool_policy = { allow = ["read_file", "search"], deny = ["execute_command"] } },
]

[[mob.specs.policy_test.flows]]
flow_id = "main"
steps = [{ step_id = "s1", targets = { role = "worker" } }]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);
    let role = &spec.roles[0];

    let config = compile_role_config(role, &spec, "realm", "mk-1");

    assert_eq!(config.tools.tool_allow, vec!["read_file", "search"]);
    assert_eq!(config.tools.tool_deny, vec!["execute_command"]);
}

// ---------------------------------------------------------------------------
// CHOKE-MOB-008: Event emission coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn choke_mob_008_event_emission_coverage() {
    // Scenario: Run a simple flow end-to-end and verify all expected event
    // kinds are emitted with correct retention categories.

    let toml = r#"
[mob.specs.event_mob]
roles = [
    { role = "worker", prompt_inline = "You work." },
]

[[mob.specs.event_mob.flows]]
flow_id = "event_flow"
steps = [
    { step_id = "work_step", targets = { role = "worker" }, dispatch_mode = "fan_out", timeout_ms = 5000 },
]
"#;

    let spec = parse_mob_specs_from_toml(toml).unwrap().remove(0);

    let (runtime, realm_id) = create_test_runtime("event_mob");
    let namespace = format!("{realm_id}/event_mob");

    // Apply spec
    runtime
        .apply_spec(ApplySpecRequest { spec })
        .await
        .unwrap();

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

    // Activate flow
    let result = runtime
        .activate(ActivateRequest {
            mob_id: "event_mob".to_string(),
            flow_id: "event_flow".to_string(),
            params: serde_json::json!({}),
        })
        .await
        .unwrap();

    // Wait for completion
    let run = wait_for_run_terminal(&runtime, &result.run_id, 10_000).await;
    assert_eq!(run.status, RunStatus::Completed);

    // Poll all events
    let events = runtime
        .poll_events(PollEventsRequest {
            mob_id: "event_mob".to_string(),
            after_cursor: None,
            limit: None,
        })
        .await
        .unwrap();

    // Verify cursors are monotonically increasing
    for window in events.windows(2) {
        assert!(
            window[1].cursor > window[0].cursor,
            "Event cursors must be monotonically increasing"
        );
    }

    // Extract event kinds for assertion
    let event_kinds: Vec<&MobEventKind> = events.iter().map(|e| &e.kind).collect();

    // Must have RunStarted
    assert!(
        event_kinds.iter().any(|k| matches!(k, MobEventKind::RunStarted)),
        "Should have RunStarted event. Kinds: {:?}",
        event_kinds
    );

    // Must have StepStarted
    assert!(
        event_kinds.iter().any(|k| matches!(k, MobEventKind::StepStarted { .. })),
        "Should have StepStarted event. Kinds: {:?}",
        event_kinds
    );

    // Must have DispatchSent
    assert!(
        event_kinds.iter().any(|k| matches!(k, MobEventKind::DispatchSent { .. })),
        "Should have DispatchSent event. Kinds: {:?}",
        event_kinds
    );

    // Must have ResponseCollected
    assert!(
        event_kinds.iter().any(|k| matches!(k, MobEventKind::ResponseCollected { .. })),
        "Should have ResponseCollected event. Kinds: {:?}",
        event_kinds
    );

    // Must have StepCompleted
    assert!(
        event_kinds.iter().any(|k| matches!(k, MobEventKind::StepCompleted { .. })),
        "Should have StepCompleted event. Kinds: {:?}",
        event_kinds
    );

    // Must have RunCompleted
    assert!(
        event_kinds.iter().any(|k| matches!(k, MobEventKind::RunCompleted)),
        "Should have RunCompleted event. Kinds: {:?}",
        event_kinds
    );

    // Verify retention categories
    let run_started_event = events
        .iter()
        .find(|e| matches!(e.kind, MobEventKind::RunStarted))
        .unwrap();
    assert_eq!(
        run_started_event.retention,
        RetentionCategory::Audit,
        "RunStarted should have Audit retention"
    );

    let dispatch_event = events
        .iter()
        .find(|e| matches!(e.kind, MobEventKind::DispatchSent { .. }))
        .unwrap();
    assert_eq!(
        dispatch_event.retention,
        RetentionCategory::Debug,
        "DispatchSent should have Debug retention"
    );

    let step_started_event = events
        .iter()
        .find(|e| matches!(e.kind, MobEventKind::StepStarted { .. }))
        .unwrap();
    assert_eq!(
        step_started_event.retention,
        RetentionCategory::Ops,
        "StepStarted should have Ops retention"
    );

    // Verify correlation fields
    for event in &events {
        assert_eq!(event.mob_id, "event_mob", "All events should have mob_id set");
        if matches!(
            event.kind,
            MobEventKind::StepStarted { .. }
                | MobEventKind::DispatchSent { .. }
                | MobEventKind::ResponseCollected { .. }
                | MobEventKind::StepCompleted { .. }
        ) {
            assert!(event.step_id.is_some(), "Step-level events should have step_id");
            assert!(event.flow_id.is_some(), "Step-level events should have flow_id");
        }
        assert!(event.run_id.is_some(), "All run events should have run_id");
    }

    // Verify reconcile emits ReconcileReport event
    let _ = runtime
        .reconcile(ReconcileRequest {
            mob_id: "event_mob".to_string(),
            mode: ReconcileMode::ReportOnly,
        })
        .await
        .unwrap();

    let all_events = runtime
        .poll_events(PollEventsRequest {
            mob_id: "event_mob".to_string(),
            after_cursor: None,
            limit: None,
        })
        .await
        .unwrap();

    let reconcile_events: Vec<_> = all_events
        .iter()
        .filter(|e| matches!(e.kind, MobEventKind::ReconcileReport { .. }))
        .collect();
    assert!(
        !reconcile_events.is_empty(),
        "Reconcile should emit ReconcileReport event"
    );

    mock_handle.abort();
}

// ---------------------------------------------------------------------------
// CHOKE-MOB-009: Supervisor integration
// ---------------------------------------------------------------------------

#[test]
fn choke_mob_009_supervisor_check_and_events() {
    use crate::supervisor::MobSupervisor;
    use crate::run::StepLedgerEntry;
    use chrono::Duration;

    let spec = crate::spec::SupervisorSpec {
        enabled: true,
        dispatch_timeout_ms: 500,
        notify_role: Some("escalation_handler".to_string()),
    };

    let supervisor = MobSupervisor::new(&spec);
    let now = Utc::now();

    // Create a run with one dispatched step that's past timeout
    let run = MobRun {
        run_id: "sup-test-1".to_string(),
        mob_id: "test_mob".to_string(),
        flow_id: "test_flow".to_string(),
        spec_revision: 1,
        status: RunStatus::Running,
        step_ledger: vec![
            StepLedgerEntry {
                step_id: "stuck_step".to_string(),
                target_meerkat_id: "meerkat-stuck".to_string(),
                status: StepEntryStatus::Dispatched,
                attempt: 1,
                dispatched_at: Some(now - Duration::milliseconds(1000)),
                completed_at: None,
                result: None,
                error: None,
            },
            StepLedgerEntry {
                step_id: "ok_step".to_string(),
                target_meerkat_id: "meerkat-ok".to_string(),
                status: StepEntryStatus::Completed,
                attempt: 1,
                dispatched_at: Some(now - Duration::milliseconds(1000)),
                completed_at: Some(now - Duration::milliseconds(500)),
                result: Some(serde_json::json!({"done": true})),
                error: None,
            },
        ],
        failure_ledger: Vec::new(),
        created_at: now,
        updated_at: now,
    };

    let escalations = supervisor.check_run(&run, now);

    // Should detect exactly 1 escalation (stuck_step)
    assert_eq!(escalations.len(), 1, "Should detect 1 timeout escalation");
    assert_eq!(escalations[0].step_id, "stuck_step");
    assert_eq!(escalations[0].target_meerkat_id, "meerkat-stuck");

    // Convert to events
    let events = supervisor.escalation_events(
        &escalations,
        "test_mob",
        "sup-test-1",
        Some("test_flow"),
    );
    assert_eq!(events.len(), 1);
    match &events[0].kind {
        MobEventKind::SupervisorEscalation { description } => {
            assert!(description.contains("stuck_step"));
            assert!(description.contains("meerkat-stuck"));
        }
        other => panic!("Expected SupervisorEscalation, got: {other:?}"),
    }

    // Verify notify_role
    assert_eq!(supervisor.notify_role(), Some("escalation_handler"));
}
