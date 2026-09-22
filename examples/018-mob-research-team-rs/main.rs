//! # 018 — Mob: Research Team (Rust)
//!
//! Builds and validates a research-team mob definition, spawns a lead plus two
//! specialists, wires the team, and sends one question-generation prompt to
//! the lead. It does not execute specialist research or synthesis turns.
//!
//! ## What you'll learn
//! - Defining a research team from TOML
//! - Custom profiles with specialized skills
//! - Spawning and wiring multiple agents from a definition
//!
//! Note: Uses `build_ephemeral_service` (in-memory substrate) for simplicity.
//! Production mob deployments use the runtime-backed path.
//! - Running a turn on the lead and reading mob events
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... ./scripts/repo-cargo run -p meerkat-mob --example 018-mob-research-team
//! ```

use std::sync::Arc;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_mob::{
    AgentIdentity, MobBuilder, MobDefinition, MobEventKind, MobRuntimeMode, MobStorage,
    SpawnMemberSpec, validate_definition,
};

#[path = "../017-mob-coding-swarm-rs/tracked_turn.rs"]
mod tracked_turn;
use tracked_turn::DemoError;

const RESEARCH_TEAM_TOML: &str = r#"
[mob]
id = "research_team"
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
content = "Run structured research: coordinate workers, synthesize findings into a cohesive report."

[skills.worker]
source = "inline"
content = "Gather evidence and return sourced summaries for assigned research questions."
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
    meerkat_runtime::host_stack::run_host("018-mob-research-team", async_main)?
}

async fn async_main() -> Result<(), DemoError> {
    let _api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    // ── Part 1: Explore the research team definition ─────────────────────────
    println!("=== Mob: Research Team ===\n");

    let definition = MobDefinition::from_toml(RESEARCH_TEAM_TOML)?;

    println!("Mob ID: {}", definition.id);
    println!("Profiles:");
    for (name, binding) in &definition.profiles {
        if let Some(profile) = binding.as_inline() {
            println!(
                "  {} -- model: {}, peer_description: {}",
                name, profile.model, profile.peer_description,
            );
        } else {
            println!("  {name} -- realm ref");
        }
    }
    println!(
        "Auto-wire orchestrator: {}",
        definition.wiring.auto_wire_orchestrator
    );

    // ── Part 2: Custom research team definition (TOML) ───────────────────────

    println!("\n=== Custom Research Team Definition (from TOML) ===\n");

    let custom = r#"
[mob]
id = "market-research"
orchestrator = "lead-analyst"

[profiles.lead-analyst]
model = "claude-opus-4-8"
skills = ["research-lead"]
peer_description = "Lead analyst -- defines research questions, synthesizes findings"
external_addressable = true

[profiles.lead-analyst.tools]
builtins = true
comms = true
mob = true

[profiles.market-researcher]
model = "claude-sonnet-4-6"
skills = ["market-analysis"]
peer_description = "Market researcher -- competitive analysis, market sizing"

[profiles.market-researcher.tools]
builtins = true
comms = true

[profiles.tech-researcher]
model = "claude-sonnet-4-6"
skills = ["tech-analysis"]
peer_description = "Technology researcher -- technical feasibility, architecture"

[profiles.tech-researcher.tools]
builtins = true
comms = true

[wiring]
auto_wire_orchestrator = true

[[wiring.role_wiring]]
a = "market-researcher"
b = "tech-researcher"

[skills.research-lead]
source = "inline"
content = """
## Role
Run structured market research with synthesis.

## Coordination Pattern
1. Define research questions across domains
2. Spawn domain researchers (market, tech)
3. Monitor progress, unblock researchers
4. Converge: synthesize findings into recommendations
"""

[skills.market-analysis]
source = "inline"
content = "Analyze market dynamics, competitive landscape, TAM/SAM/SOM, and growth trajectories."

[skills.tech-analysis]
source = "inline"
content = "Evaluate technical feasibility, architecture options, scalability constraints."
"#;

    let custom_def = MobDefinition::from_toml(custom)?;
    println!("Mob: {}", custom_def.id);
    println!("Profiles ({}):", custom_def.profiles.len());
    for (name, binding) in &custom_def.profiles {
        if let Some(profile) = binding.as_inline() {
            println!("  {} -- {}", name, profile.peer_description);
        }
    }

    let diagnostics = validate_definition(&custom_def);
    if diagnostics.is_empty() {
        println!("Validation: PASSED");
    } else {
        for d in &diagnostics {
            println!("  {:?}: {}", d.severity, d.message);
        }
    }

    // ── Part 3: Create and run a real research team mob ──────────────────────

    println!("\n=== Live Mob Execution ===\n");

    // Set up infrastructure.
    let temp_dir = tempfile::tempdir()?;
    let store_path = temp_dir.path().join("sessions");
    std::fs::create_dir_all(&store_path)?;

    let factory = AgentFactory::new(&store_path).comms(true);
    let config = Config::default();
    let session_service = Arc::new(build_ephemeral_service(factory, config, 16));

    // Create the mob using the custom definition above.
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(custom_def, storage)
        .with_session_service(session_service)
        .allow_ephemeral_sessions(true)
        .create()
        .await?;

    println!(
        "Mob '{}' created (status: {:?})",
        handle.mob_id(),
        handle.status().await?
    );

    // Spawn the lead analyst and two researchers.
    println!("\nSpawning team...");

    // Exact tracked turns use the turn-driven lane, not autonomous inbox delivery.
    let mut lead_spec = SpawnMemberSpec::new("lead-analyst", "lead-1")
        .with_runtime_mode(MobRuntimeMode::TurnDriven);
    lead_spec.initial_message = Some(
        "You are the lead analyst coordinating this research team."
            .to_string()
            .into(),
    );
    let lead_ref = handle.spawn_spec(lead_spec).await?;
    println!("  Spawned lead-1 (lead-analyst): {lead_ref:?}");

    let mut market_spec = SpawnMemberSpec::new("market-researcher", "market-1");
    market_spec.initial_message = Some(
        "You are a market researcher on this team."
            .to_string()
            .into(),
    );
    let market_ref = handle.spawn_spec(market_spec).await?;
    println!("  Spawned market-1 (market-researcher): {market_ref:?}");

    let mut tech_spec = SpawnMemberSpec::new("tech-researcher", "tech-1");
    tech_spec.initial_message = Some(
        "You are a technology researcher on this team."
            .to_string()
            .into(),
    );
    let tech_ref = handle.spawn_spec(tech_spec).await?;
    println!("  Spawned tech-1 (tech-researcher): {tech_ref:?}");

    // Wire the team: lead <-> market, lead <-> tech, market <-> tech.
    handle
        .wire(
            AgentIdentity::from("lead-1"),
            AgentIdentity::from("market-1"),
        )
        .await?;
    handle
        .wire(AgentIdentity::from("lead-1"), AgentIdentity::from("tech-1"))
        .await?;
    handle
        .wire(
            AgentIdentity::from("market-1"),
            AgentIdentity::from("tech-1"),
        )
        .await?;
    println!("  Wired all team members");

    // Show the roster.
    let members = handle.list_members().await;
    println!("\nRoster ({} members):", members.len());
    for m in &members {
        println!(
            "  {} (profile: {}, wired_to: {:?})",
            m.agent_identity, m.role, m.wired_to
        );
    }

    // Send a research question to the lead analyst (live LLM call).
    println!("\nSending research question to lead analyst (live LLM call)...");
    let lead = handle.member(&AgentIdentity::from("lead-1")).await?;
    let turn_result = tracked_turn::report_turn(
        &lead,
        "Briefly outline 3 key research questions about the market for AI code assistants. \
             Keep your response to 3-4 sentences total. Do NOT use any tools -- \
             just provide the questions in plain text.",
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
