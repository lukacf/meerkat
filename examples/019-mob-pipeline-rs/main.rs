//! # 019 — Mob: Pipeline (Rust)
//!
//! A pipeline mob processes work in sequential stages. Each stage is handled
//! by a specialized worker, with explicit handoffs between stages.
//!
//! ## What you'll learn
//! - Sequential stage processing with mobs
//! - Stage handoff via directed peer requests
//! - The pipeline prefab pattern
//! - Flow definitions for deterministic execution order
//!
//! ## Run
//! ```bash
//! This is a reference implementation. For runnable examples, see meerkat/examples/.
//! ```

use meerkat_mob::{MobDefinition, Prefab, validate_definition};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── Pipeline Prefab ────────────────────────────────────────────────────

    println!("=== Mob: Pipeline ===\n");
    println!("{}\n", Prefab::Pipeline.toml_template());

    // ── Custom CI/CD Pipeline ──────────────────────────────────────────────

    println!("=== Custom CI/CD Pipeline ===\n");

    let pipeline_toml = r#"
[mob]
id = "cicd-pipeline"
orchestrator = "coordinator"

[profiles.coordinator]
model = "claude-opus-4-6"
skills = ["pipeline-coordinator"]
peer_description = "Pipeline coordinator — drives sequential stage execution"
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
shell = true
comms = true
mob_tasks = true

[profiles.tester]
model = "claude-sonnet-4-5"
skills = ["test-stage"]
peer_description = "Stage 2: Test execution and coverage analysis"

[profiles.tester.tools]
builtins = true
shell = true
comms = true
mob_tasks = true

[profiles.security]
model = "claude-sonnet-4-5"
skills = ["security-stage"]
peer_description = "Stage 3: Security audit and vulnerability scanning"

[profiles.security.tools]
builtins = true
shell = true
comms = true
mob_tasks = true

[profiles.deployer]
model = "claude-sonnet-4-5"
skills = ["deploy-stage"]
peer_description = "Stage 4: Deployment and smoke testing"

[profiles.deployer.tools]
builtins = true
shell = true
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
3. Security: Vulnerability scanning
4. Deploy: Release and smoke tests

## Rules
- Stages execute sequentially
- A stage must PASS before the next starts
- On failure: report to user, do not proceed
- On success: pass artifacts to next stage
- Call mob.complete() after all stages pass
"""

[skills.lint-stage]
source = "inline"
content = "Run code linting: cargo clippy, rustfmt check. Report warnings and errors. Pass/fail."

[skills.test-stage]
source = "inline"
content = "Run test suite: cargo test --workspace. Report coverage and failures. Pass/fail."

[skills.security-stage]
source = "inline"
content = "Run security audit: cargo deny, check for known vulnerabilities. Pass/fail."

[skills.deploy-stage]
source = "inline"
content = "Execute deployment: build release binary, run smoke tests. Pass/fail."
"#;

    let definition = MobDefinition::from_toml(pipeline_toml)?;
    println!("Pipeline: {}", definition.id);
    println!("Stages:");
    for (i, (name, profile)) in definition.profiles.iter().enumerate() {
        if name.as_str() != "coordinator" {
            println!("  Stage {}: {} — {}", i, name, profile.peer_description);
        }
    }

    let diagnostics = validate_definition(&definition);
    println!(
        "\nValidation: {}",
        if diagnostics.is_empty() {
            "PASSED"
        } else {
            "ISSUES FOUND"
        }
    );

    // ── Pipeline execution pattern ─────────────────────────────────────────

    println!("\n\n=== Pipeline Execution Pattern ===\n");
    println!(
        r#"Sequential stage processing with explicit handoffs:

Coordinator
    │
    ├──→ [Stage 1: Lint]
    │         ↓ PASS
    │         ├──→ artifacts: lint_report.json
    │
    ├──→ [Stage 2: Test] ← receives lint artifacts
    │         ↓ PASS
    │         ├──→ artifacts: test_report.json, coverage.json
    │
    ├──→ [Stage 3: Security] ← receives test artifacts
    │         ↓ PASS
    │         ├──→ artifacts: audit_report.json
    │
    └──→ [Stage 4: Deploy] ← receives all prior artifacts
              ↓ PASS
              └──→ artifacts: deploy_receipt.json

If any stage FAILS:
    Coordinator reports failure and stops pipeline.
    No subsequent stages execute.
"#
    );

    Ok(())
}
