//! # 019 — Mob: Pipeline (Rust)
//!
//! A pipeline mob processes work in sequential stages. Each stage is handled
//! by a specialized worker, with explicit handoffs between stages.
//!
//! ## What you'll learn
//! - Sequential stage processing with mobs
//! - Spawning stage-specific workers
//! - Running turns on individual pipeline stages
//! - The pipeline prefab pattern
//! - Task board for tracking stage results
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... cargo run --example 019-mob-pipeline --features comms
//! ```

use std::sync::Arc;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_mob::{
    MeerkatId, MobBuilder, MobDefinition, MobEventKind, MobStorage, Prefab, ProfileName,
    validate_definition,
};

/// Format a mob event kind into a short human-readable label.
fn event_label(kind: &MobEventKind) -> &'static str {
    match kind {
        MobEventKind::MobCreated { .. } => "MobCreated",
        MobEventKind::MobCompleted => "MobCompleted",
        MobEventKind::MobReset => "MobReset",
        MobEventKind::MeerkatSpawned { .. } => "MeerkatSpawned",
        MobEventKind::MeerkatRetired { .. } => "MeerkatRetired",
        MobEventKind::PeersWired { .. } => "PeersWired",
        MobEventKind::PeersUnwired { .. } => "PeersUnwired",
        MobEventKind::TaskCreated { .. } => "TaskCreated",
        MobEventKind::TaskUpdated { .. } => "TaskUpdated",
        MobEventKind::FlowStarted { .. } => "FlowStarted",
        MobEventKind::FlowCompleted { .. } => "FlowCompleted",
        MobEventKind::FlowFailed { .. } => "FlowFailed",
        MobEventKind::FlowCanceled { .. } => "FlowCanceled",
        MobEventKind::StepCompleted { .. } => "StepCompleted",
        _ => "Other",
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── Check for API key ────────────────────────────────────────────────────
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("ANTHROPIC_API_KEY is required to run this example.");
        eprintln!("Set it and re-run:");
        eprintln!(
            "  ANTHROPIC_API_KEY=sk-... cargo run --example 019-mob-pipeline --features comms"
        );
        std::process::exit(1);
    }

    // ── Part 1: Explore the pipeline prefab ──────────────────────────────────
    println!("=== Mob: Pipeline (Prefab) ===\n");
    println!("{}\n", Prefab::Pipeline.toml_template());

    // ── Part 2: Custom CI/CD pipeline definition (TOML) ──────────────────────

    println!("=== Custom CI/CD Pipeline ===\n");

    let pipeline_toml = r#"
[mob]
id = "cicd-pipeline"
orchestrator = "coordinator"

[profiles.coordinator]
model = "claude-opus-4-6"
skills = ["pipeline-coordinator"]
peer_description = "Pipeline coordinator -- drives sequential stage execution"
external_addressable = true

[profiles.coordinator.tools]
builtins = true
comms = true
mob = true
mob_tasks = true

[profiles.linter]
model = "claude-sonnet-4-5"
skills = ["lint-stage"]
peer_description = "Stage 1: Code linting and style checks"

[profiles.linter.tools]
builtins = true
comms = true
mob_tasks = true

[profiles.tester]
model = "claude-sonnet-4-5"
skills = ["test-stage"]
peer_description = "Stage 2: Test execution and coverage analysis"

[profiles.tester.tools]
builtins = true
comms = true
mob_tasks = true

[profiles.deployer]
model = "claude-sonnet-4-5"
skills = ["deploy-stage"]
peer_description = "Stage 3: Deployment and smoke testing"

[profiles.deployer.tools]
builtins = true
comms = true
mob_tasks = true

[wiring]
auto_wire_orchestrator = true

[skills.pipeline-coordinator]
source = "inline"
content = """
## Role
Drive staged CI/CD pipeline execution.

## Stages
1. Lint: Code quality checks
2. Test: Unit and integration tests
3. Deploy: Release and smoke tests

## Rules
- Stages execute sequentially
- A stage must PASS before the next starts
- On failure: report and stop
"""

[skills.lint-stage]
source = "inline"
content = "Run code linting: check formatting and style. Report pass/fail."

[skills.test-stage]
source = "inline"
content = "Run test suite: check all tests pass. Report coverage and failures."

[skills.deploy-stage]
source = "inline"
content = "Execute deployment: build release, run smoke tests. Report pass/fail."
"#;

    let definition = MobDefinition::from_toml(pipeline_toml)?;
    println!("Pipeline: {}", definition.id);
    println!("Stages:");
    for (name, profile) in &definition.profiles {
        if name.as_str() != "coordinator" {
            println!("  {} -- {}", name, profile.peer_description);
        }
    }

    let diagnostics = validate_definition(&definition);
    println!(
        "Validation: {}\n",
        if diagnostics.is_empty() {
            "PASSED"
        } else {
            "ISSUES FOUND"
        }
    );

    // ── Part 3: Create and run a real pipeline mob ───────────────────────────

    println!("=== Live Pipeline Execution ===\n");

    // Set up infrastructure.
    let temp_dir = tempfile::tempdir()?;
    let store_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&store_path)?;

    let factory = AgentFactory::new(&store_path).comms(true);
    let config = Config::default();
    let session_service = Arc::new(build_ephemeral_service(factory, config, 16));

    // Create the pipeline mob.
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(session_service)
        .allow_ephemeral_sessions(true)
        .create()
        .await?;

    println!(
        "Pipeline '{}' created (status: {:?})",
        handle.mob_id(),
        handle.status()
    );

    // Spawn the coordinator.
    println!("\nSpawning coordinator...");
    let coord_ref = handle
        .spawn(
            ProfileName::from("coordinator"),
            MeerkatId::from("coordinator-1"),
            Some("You are the CI/CD pipeline coordinator.".to_string()),
        )
        .await?;
    println!("  Spawned coordinator-1: {coord_ref:?}");

    // Spawn pipeline stage workers sequentially.
    let stages = [
        ("linter", "lint-1", "You are the linting stage worker."),
        ("tester", "test-1", "You are the testing stage worker."),
        (
            "deployer",
            "deploy-1",
            "You are the deployment stage worker.",
        ),
    ];

    for (profile, id, msg) in &stages {
        let member_ref = handle
            .spawn(
                ProfileName::from(*profile),
                MeerkatId::from(*id),
                Some(msg.to_string()),
            )
            .await?;
        println!("  Spawned {id} ({profile}): {member_ref:?}");
    }

    // Wire coordinator to all stages, and chain stages sequentially.
    for (_, id, _) in &stages {
        handle
            .wire(MeerkatId::from("coordinator-1"), MeerkatId::from(*id))
            .await?;
    }
    // Chain: lint -> test -> deploy
    handle
        .wire(MeerkatId::from("lint-1"), MeerkatId::from("test-1"))
        .await?;
    handle
        .wire(MeerkatId::from("test-1"), MeerkatId::from("deploy-1"))
        .await?;
    println!("  Wired pipeline topology");

    // Show the roster.
    let members = handle.list_members().await;
    println!("\nRoster ({} members):", members.len());
    for m in &members {
        println!(
            "  {} (profile: {}, wired_to: {:?})",
            m.meerkat_id, m.profile, m.wired_to
        );
    }

    // Run the first pipeline stage: send a lint request to the linter.
    println!("\n--- Stage 1: Lint ---");
    println!("Sending lint request (live LLM call)...");
    handle
        .internal_turn(
            MeerkatId::from("lint-1"),
            "Analyze this Rust function for style issues. Report PASS or FAIL with a one-line reason. \
             Do NOT use any tools -- respond in plain text only.\n\n\
             ```rust\n\
             fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\
             ```"
                .to_string(),
        )
        .await?;

    println!("Waiting for lint result...");
    tokio::time::sleep(std::time::Duration::from_secs(8)).await;

    // Run the second stage: send a test request to the tester.
    println!("\n--- Stage 2: Test ---");
    println!("Sending test request (live LLM call)...");
    handle
        .internal_turn(
            MeerkatId::from("test-1"),
            "The lint stage passed. Now evaluate the test coverage for this function. \
             Report PASS or FAIL with a one-line summary. \
             Do NOT use any tools -- respond in plain text only.\n\n\
             ```rust\n\
             fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\n\
             #[test]\n\
             fn test_add() {\n    assert_eq!(add(2, 3), 5);\n}\n\
             ```"
            .to_string(),
        )
        .await?;

    println!("Waiting for test result...");
    tokio::time::sleep(std::time::Duration::from_secs(8)).await;

    // Poll mob events.
    let events = handle.poll_events(0, 50).await?;
    println!("\nMob events ({} total):", events.len());
    for event in &events {
        println!("  cursor={}: {}", event.cursor, event_label(&event.kind));
    }

    // Final status.
    println!("\nPipeline status: {:?}", handle.status());
    println!("Members: {}", handle.list_members().await.len());

    // Clean up.
    handle.retire_all().await?;
    println!("All stage workers retired. Pipeline complete.");

    Ok(())
}
