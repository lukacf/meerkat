//! # 018 — Mob: Research Team (Rust)
//!
//! A research team mob where a lead coordinates specialized researchers.
//! Each researcher explores a different domain, and the lead synthesizes
//! findings into a cohesive report.
//!
//! ## What you'll learn
//! - The research team prefab pattern
//! - Custom profiles with specialized skills
//! - Diverge/converge coordination (explore then synthesize)
//! - Task board for tracking research hypotheses
//!
//! ## Run
//! ```bash
//! This is a reference implementation. For runnable examples, see meerkat/examples/.
//! ```

use meerkat_mob::{MobDefinition, Prefab, validate_definition};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── Research Team Prefab ───────────────────────────────────────────────

    println!("=== Mob: Research Team ===\n");

    let prefab = Prefab::ResearchTeam;
    let _definition = prefab.definition();

    println!("Template:\n{}\n", prefab.toml_template());

    // ── Custom Research Team ───────────────────────────────────────────────

    println!("=== Custom Research Team Definition ===\n");

    let custom = r#"
[mob]
id = "market-research"
orchestrator = "lead-analyst"

[profiles.lead-analyst]
model = "claude-opus-4-6"
skills = ["research-lead"]
peer_description = "Lead analyst — defines research questions, synthesizes findings"
external_addressable = true

[profiles.lead-analyst.tools]
builtins = true
comms = true
mob = true
mob_tasks = true

[profiles.market-researcher]
model = "claude-sonnet-4-5"
skills = ["market-analysis"]
peer_description = "Market researcher — competitive analysis, market sizing"

[profiles.market-researcher.tools]
builtins = true
comms = true
mob_tasks = true

[profiles.tech-researcher]
model = "claude-sonnet-4-5"
skills = ["tech-analysis"]
peer_description = "Technology researcher — technical feasibility, architecture"

[profiles.tech-researcher.tools]
builtins = true
comms = true
mob_tasks = true

[profiles.user-researcher]
model = "claude-sonnet-4-5"
skills = ["user-research"]
peer_description = "User researcher — personas, pain points, adoption barriers"

[profiles.user-researcher.tools]
builtins = true
comms = true
mob_tasks = true

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
2. Spawn domain researchers (market, tech, user)
3. Create tasks for each research question
4. Monitor progress, unblock researchers
5. Converge: synthesize findings into recommendations
6. Produce final report with evidence and confidence levels
"""

[skills.market-analysis]
source = "inline"
content = "Analyze market dynamics, competitive landscape, TAM/SAM/SOM, and growth trajectories. Provide sourced data points."

[skills.tech-analysis]
source = "inline"
content = "Evaluate technical feasibility, architecture options, scalability constraints, and build-vs-buy decisions."

[skills.user-research]
source = "inline"
content = "Define user personas, map pain points, identify adoption barriers, and recommend user acquisition strategies."
"#;

    let custom_def = MobDefinition::from_toml(custom)?;
    println!("Mob: {}", custom_def.id);
    println!("Profiles ({}):", custom_def.profiles.len());
    for (name, profile) in &custom_def.profiles {
        println!("  {} — {}", name, profile.peer_description);
    }

    let diagnostics = validate_definition(&custom_def);
    if diagnostics.is_empty() {
        println!("\nValidation: PASSED");
    } else {
        for d in &diagnostics {
            println!("  {:?}: {}", d.severity, d.message);
        }
    }

    // ── Research coordination pattern ──────────────────────────────────────

    println!("\n\n=== Research Team Coordination Pattern ===\n");
    println!(
        r#"The diverge/converge pattern:

Phase 1: DIVERGE (parallel exploration)
┌─────────────┐
│ Lead Analyst │──→ "Research Question: Is there a market for X?"
└──────┬──────┘
       │ mob.spawn()
       ├───→ Market Researcher  ──→ task: "Analyze TAM/SAM for X"
       ├───→ Tech Researcher    ──→ task: "Evaluate technical feasibility of X"
       └───→ User Researcher    ──→ task: "Identify target personas for X"

Phase 2: CONVERGE (synthesis)
       ┌─── Market findings ────┐
       ├─── Tech findings ──────┤
       └─── User findings ──────┘
                    ↓
         ┌─────────────────┐
         │   Lead Analyst   │ ──→ "Final Report: Market Analysis for X"
         │   Synthesizes    │     (confidence: HIGH, evidence: 12 sources)
         └─────────────────┘

Role wiring allows researchers to cross-reference:
  market-researcher ←→ tech-researcher (share findings)
  All researchers ←→ lead-analyst (report progress)
"#
    );

    Ok(())
}
