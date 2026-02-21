//! # 017 — Mob: Coding Swarm (Rust)
//!
//! A "mob" is a coordinated group of Meerkat agents with defined roles,
//! wiring rules, and shared task boards. The `coding_swarm` prefab creates
//! a lead orchestrator that spawns and manages worker agents for coding tasks.
//!
//! ## What you'll learn
//! - `MobDefinition` — declaring agents, profiles, and wiring
//! - `MobBuilder` — creating and starting a mob
//! - `MobHandle` — interacting with a running mob
//! - Prefab templates — ready-made mob configurations
//! - The coding swarm pattern (lead + workers)
//!
//! ## Run
//! ```bash
//! This is a reference implementation. For runnable examples, see meerkat/examples/.
//! ```

use meerkat_mob::{MobDefinition, Prefab};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── Load the coding swarm prefab ───────────────────────────────────────

    println!("=== Mob: Coding Swarm ===\n");

    // Prefabs are built-in mob templates
    let definition = Prefab::CodingSwarm.definition();

    println!("Mob ID: {}", definition.id);
    println!("Profiles:");
    for (name, profile) in &definition.profiles {
        println!(
            "  {} — model: {}, skills: {:?}, tools: {:?}",
            name, profile.model, profile.skills, profile.tools,
        );
    }

    if let Some(ref orchestrator) = definition.orchestrator {
        println!("Orchestrator: {}", orchestrator.profile);
    }

    println!("Wiring:");
    println!(
        "  auto_wire_orchestrator: {}",
        definition.wiring.auto_wire_orchestrator
    );

    // ── Show the TOML template ─────────────────────────────────────────────

    println!("\n=== TOML Template (editable) ===\n");
    println!("{}", Prefab::CodingSwarm.toml_template());

    // ── Custom mob definition ──────────────────────────────────────────────

    println!("\n=== Custom Mob Definition ===\n");

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
    println!("Profiles: {:?}", custom_def.profiles.keys().collect::<Vec<_>>());

    // Validate the definition
    let diagnostics = meerkat_mob::validate_definition(&custom_def);
    if diagnostics.is_empty() {
        println!("Validation: PASSED");
    } else {
        for d in &diagnostics {
            println!("  {:?}: {}", d.severity, d.message);
        }
    }

    // ── Mob lifecycle reference ────────────────────────────────────────────

    println!("\n\n=== Mob Lifecycle ===\n");
    println!(
        r#"1. Define: Write mob.toml or use a Prefab
2. Create: MobBuilder::new(definition, storage).create()
3. Run:    MobHandle drives the orchestrator
4. Scale:  Orchestrator spawns/retires workers via mob tools
5. Done:   Orchestrator calls mob.complete()

Mob tools available to the orchestrator:
  mob.spawn   — Spawn a new worker with a profile
  mob.retire  — Retire an idle worker
  mob.wire    — Connect two workers as comms peers
  mob.status  — Get mob state (roster, tasks)

Task tools available to all agents:
  task_create — Create a shared task on the task board
  task_update — Update task status
  task_list   — List tasks
  task_get    — Get task details

Communication:
  peers()       — Discover available peers
  send()        — Send message to a peer
  PeerRequest   — Request/response with a peer

All wiring is automatic when auto_wire_orchestrator = true.
Workers are wired to the orchestrator on spawn.
"#
    );

    Ok(())
}
