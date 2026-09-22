//! # 017 — Mob: Coding Swarm (Rust)
//!
//! A "mob" is a coordinated group of Meerkat agents with defined roles and
//! wiring. This example parses and validates coding-team definitions, spawns
//! one lead and one worker, wires them, and submits a planning prompt to the
//! lead. It does not delegate work to the worker or merge worker results.
//!
//! ## What you'll learn
//! - `MobDefinition` — declaring agents, profiles, and wiring
//! - `MobBuilder` — creating and starting a mob runtime
//! - `MobHandle` — spawning agents and running turns
//! - Parsing mob definitions from TOML
//! - A lead + worker topology
//!
//! Note: Uses `build_ephemeral_service` (in-memory substrate) for simplicity.
//! Production mob deployments use the runtime-backed path.
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... ./scripts/repo-cargo run -p meerkat-mob --example 017-mob-coding-swarm
//! ```

use std::sync::Arc;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_mob::{
    AgentIdentity, MobBuilder, MobDefinition, MobEventKind, MobRuntimeMode, MobStorage,
    SpawnMemberSpec, validate_definition,
};

mod tracked_turn;
use tracked_turn::DemoError;

const CODING_SWARM_TOML: &str = r#"
[mob]
id = "coding_swarm"
orchestrator = "lead"

[profiles.lead]
model = "claude-opus-4-8"
skills = ["orchestrator"]
peer_description = "Orchestrator"
external_addressable = true

[profiles.lead.tools]
builtins = true
comms = true
mob = true

[profiles.worker]
model = "claude-sonnet-4-6"
skills = ["worker"]
peer_description = "Worker"
external_addressable = false

[profiles.worker.tools]
builtins = true
shell = true
comms = true

[wiring]
auto_wire_orchestrator = true

[skills.orchestrator]
source = "inline"
content = "Lead a coding swarm: coordinate workers, assign tasks, synthesize results."

[skills.worker]
source = "inline"
content = "Implement assigned code tasks and report concise diffs."
"#;

/// Format a mob event kind into a short human-readable label.
fn event_label(kind: &MobEventKind) -> &'static str {
    match kind {
        MobEventKind::MobCreated { .. } => "MobCreated",
        MobEventKind::MobCompleted => "MobCompleted",
        MobEventKind::MobReset => "MobReset",
        MobEventKind::MemberSpawned(..) => "MemberSpawned",
        MobEventKind::MemberRetired { .. } => "MemberRetired",
        MobEventKind::FlowStarted { .. } => "FlowStarted",
        MobEventKind::FlowCompleted { .. } => "FlowCompleted",
        MobEventKind::FlowFailed { .. } => "FlowFailed",
        MobEventKind::FlowCanceled { .. } => "FlowCanceled",
        MobEventKind::StepCompleted { .. } => "StepCompleted",
        _ => "Other",
    }
}

fn main() -> Result<(), DemoError> {
    meerkat_runtime::host_stack::run_host("017-mob-coding-swarm", async_main)?
}

async fn async_main() -> Result<(), DemoError> {
    let _api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    // ── Part 1: Explore the coding swarm definition ──────────────────────────
    println!("=== Mob: Coding Swarm ===\n");

    let definition = MobDefinition::from_toml(CODING_SWARM_TOML)?;

    println!("Mob ID: {}", definition.id);
    println!("Profiles:");
    for (name, binding) in &definition.profiles {
        if let Some(profile) = binding.as_inline() {
            println!(
                "  {} -- model: {}, skills: {:?}",
                name, profile.model, profile.skills,
            );
        } else {
            println!("  {name} -- realm ref");
        }
    }

    if let Some(ref orchestrator) = definition.orchestrator {
        println!("Orchestrator profile: {}", orchestrator.profile);
    }

    println!(
        "Auto-wire orchestrator: {}",
        definition.wiring.auto_wire_orchestrator
    );

    // ── Part 2: Custom mob definition (TOML) ────────────────────────────────

    println!("\n=== Custom Mob Definition (from TOML) ===\n");

    let custom_toml = r#"
[mob]
id = "my-dev-team"
orchestrator = "architect"

[profiles.architect]
model = "claude-opus-4-8"
skills = ["system-design"]
peer_description = "System architect and task coordinator"
external_addressable = true

[profiles.architect.tools]
builtins = true
comms = true
mob = true

[profiles.frontend]
model = "claude-sonnet-4-6"
skills = ["react-specialist"]
peer_description = "React/TypeScript frontend developer"

[profiles.frontend.tools]
builtins = true
shell = true
comms = true

[profiles.backend]
model = "claude-sonnet-4-6"
skills = ["rust-specialist"]
peer_description = "Rust backend developer"

[profiles.backend.tools]
builtins = true
shell = true
comms = true

[wiring]
auto_wire_orchestrator = true

[[wiring.role_wiring]]
a = "frontend"
b = "backend"

[skills.system-design]
source = "inline"
content = "Design systems, break into tasks, coordinate frontend/backend work."

[skills.react-specialist]
source = "inline"
content = "Implement React components and TypeScript interfaces."

[skills.rust-specialist]
source = "inline"
content = "Implement Rust services, APIs, and data models."
"#;

    let custom_def = MobDefinition::from_toml(custom_toml)?;
    println!("Custom mob: {}", custom_def.id);
    println!(
        "Profiles: {:?}",
        custom_def.profiles.keys().collect::<Vec<_>>()
    );

    let diagnostics = validate_definition(&custom_def);
    if diagnostics.is_empty() {
        println!("Validation: PASSED");
    } else {
        for d in &diagnostics {
            println!("  {:?}: {}", d.severity, d.message);
        }
    }

    // ── Part 3: Create and run a real mob ────────────────────────────────────

    println!("\n=== Live Mob Execution ===\n");

    // Set up infrastructure: temp directory, agent factory with comms,
    // ephemeral session service backed by the factory.
    let temp_dir = tempfile::tempdir()?;
    let store_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&store_path)?;

    let factory = AgentFactory::new(&store_path).comms(true);
    let config = Config::default();
    let session_service = Arc::new(build_ephemeral_service(factory, config, 16));

    // Create the mob from the coding swarm definition.
    let definition = MobDefinition::from_toml(CODING_SWARM_TOML)?;
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(session_service)
        .allow_ephemeral_sessions(true)
        .create()
        .await?;

    println!(
        "Mob '{}' created (status: {:?})",
        handle.mob_id(),
        handle.status().await?
    );

    // Spawn an orchestrator (lead profile) and a worker.
    println!("\nSpawning agents...");
    // Exact tracked turns use the turn-driven lane, not autonomous inbox delivery.
    let mut lead_spec =
        SpawnMemberSpec::new("lead", "lead-1").with_runtime_mode(MobRuntimeMode::TurnDriven);
    lead_spec.initial_message = Some("You are the coding swarm orchestrator.".to_string().into());
    let lead_ref = handle.spawn_spec(lead_spec).await?;
    println!("  Spawned lead-1: {lead_ref:?}");

    let mut worker_spec = SpawnMemberSpec::new("worker", "worker-1");
    worker_spec.initial_message = Some("You are a coding worker in the swarm.".to_string().into());
    let worker_ref = handle.spawn_spec(worker_spec).await?;
    println!("  Spawned worker-1: {worker_ref:?}");

    // Wire orchestrator to worker for peer communication.
    handle
        .wire(
            AgentIdentity::from("lead-1"),
            AgentIdentity::from("worker-1"),
        )
        .await?;
    println!("  Wired lead-1 <-> worker-1");

    // Show the roster.
    let members = handle.list_members().await;
    println!("\nRoster ({} members):", members.len());
    for m in &members {
        println!(
            "  {} (profile: {}, wired_to: {:?})",
            m.agent_identity, m.role, m.wired_to
        );
    }

    // Send a task to the orchestrator (external turn -- lead is external_addressable).
    println!("\nSending task to orchestrator (live LLM call)...");
    let lead = handle.member(&AgentIdentity::from("lead-1")).await?;
    let turn_result = tracked_turn::report_turn(
        &lead,
        "Plan a small task: write a function that reverses a string in Rust. \
             Describe the plan in 2-3 sentences. Do NOT spawn workers or use any tools -- \
             just describe the plan in plain text.",
        std::time::Duration::from_secs(60),
        &mut std::io::stdout(),
    )
    .await;
    if let Err(error) = turn_result {
        if let Err(cleanup_error) = handle.retire_all().await {
            eprintln!("Cleanup also failed: {cleanup_error}");
        }
        return Err(error);
    }
    // Lifecycle events are diagnostic only; the exact response is already shown.
    let events = handle.poll_events(0, 50).await?;
    println!("\nMob events ({} total):", events.len());
    for event in &events {
        println!("  cursor={}: {}", event.cursor, event_label(&event.kind));
    }

    // Final status.
    println!("\nFinal mob status: {:?}", handle.status().await?);

    // Clean up.
    handle.retire_all().await?;
    println!("All members retired.");

    Ok(())
}
