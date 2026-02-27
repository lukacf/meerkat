//! # 017 — Mob: Coding Swarm (Rust)
//!
//! A "mob" is a coordinated group of Meerkat agents with defined roles,
//! wiring rules, and shared task boards. The `coding_swarm` prefab creates
//! a lead orchestrator that spawns and manages worker agents for coding tasks.
//!
//! ## What you'll learn
//! - `MobDefinition` — declaring agents, profiles, and wiring
//! - `MobBuilder` — creating and starting a mob runtime
//! - `MobHandle` — spawning agents and running turns
//! - Prefab templates — ready-made mob configurations
//! - The coding swarm pattern (lead + workers)
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... cargo run --example 017-mob-coding-swarm --features comms
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
    let _api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    // ── Part 1: Explore the coding swarm prefab ──────────────────────────────
    println!("=== Mob: Coding Swarm ===\n");

    let definition = Prefab::CodingSwarm.definition();

    println!("Mob ID: {}", definition.id);
    println!("Profiles:");
    for (name, profile) in &definition.profiles {
        println!(
            "  {} -- model: {}, skills: {:?}",
            name, profile.model, profile.skills,
        );
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
model = "claude-opus-4-6"
skills = ["system-design"]
peer_description = "System architect and task coordinator"
external_addressable = true

[profiles.architect.tools]
builtins = true
comms = true
mob = true
mob_tasks = true

[profiles.frontend]
model = "claude-sonnet-4-5"
skills = ["react-specialist"]
peer_description = "React/TypeScript frontend developer"

[profiles.frontend.tools]
builtins = true
shell = true
comms = true
mob_tasks = true

[profiles.backend]
model = "claude-sonnet-4-5"
skills = ["rust-specialist"]
peer_description = "Rust backend developer"

[profiles.backend.tools]
builtins = true
shell = true
comms = true
mob_tasks = true

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

    // Create the mob from the CodingSwarm prefab.
    let prefab_def = Prefab::CodingSwarm.definition();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(prefab_def, storage)
        .with_session_service(session_service)
        .allow_ephemeral_sessions(true)
        .create()
        .await?;

    println!(
        "Mob '{}' created (status: {:?})",
        handle.mob_id(),
        handle.status()
    );

    // Spawn an orchestrator (lead profile) and a worker.
    println!("\nSpawning agents...");
    let lead_ref = handle
        .spawn(
            ProfileName::from("lead"),
            MeerkatId::from("lead-1"),
            Some("You are the coding swarm orchestrator.".to_string()),
        )
        .await?;
    println!("  Spawned lead-1: {lead_ref:?}");

    let worker_ref = handle
        .spawn(
            ProfileName::from("worker"),
            MeerkatId::from("worker-1"),
            Some("You are a coding worker in the swarm.".to_string()),
        )
        .await?;
    println!("  Spawned worker-1: {worker_ref:?}");

    // Wire orchestrator to worker for peer communication.
    handle
        .wire(MeerkatId::from("lead-1"), MeerkatId::from("worker-1"))
        .await?;
    println!("  Wired lead-1 <-> worker-1");

    // Show the roster.
    let members = handle.list_members().await;
    println!("\nRoster ({} members):", members.len());
    for m in &members {
        println!(
            "  {} (profile: {}, wired_to: {:?})",
            m.meerkat_id, m.profile, m.wired_to
        );
    }

    // Send a task to the orchestrator (external turn -- lead is external_addressable).
    println!("\nSending task to orchestrator (live LLM call)...");
    handle
        .send_message(
            MeerkatId::from("lead-1"),
            "Plan a small task: write a function that reverses a string in Rust. \
             Describe the plan in 2-3 sentences. Do NOT spawn workers or use any tools -- \
             just describe the plan in plain text."
                .to_string(),
        )
        .await?;

    // Poll for mob events until we see activity (with timeout).
    println!("Waiting for response...");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let ev = handle.poll_events(0, 1).await?;
        if !ev.is_empty() || tokio::time::Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    let events = handle.poll_events(0, 50).await?;
    println!("\nMob events ({} total):", events.len());
    for event in &events {
        println!("  cursor={}: {}", event.cursor, event_label(&event.kind));
    }

    // Final status.
    println!("\nFinal mob status: {:?}", handle.status());

    // Clean up.
    handle.retire_all().await?;
    println!("All members retired.");

    Ok(())
}
