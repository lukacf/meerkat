//! # 012 -- Skills Loading (Rust)
//!
//! Skills inject domain-specific knowledge and behavioral instructions into
//! agents at runtime. They can be loaded from files, inline content, git repos,
//! or HTTP endpoints.
//!
//! ## What this example actually does
//! - Creates skills using `InMemorySkillSource` (inline skill documents)
//! - Creates a skill from a temporary file using `FilesystemSkillSource`
//! - Composes both sources via `CompositeSkillSource`
//! - Builds a `DefaultSkillEngine` and wraps it in a `SkillRuntime`
//! - Wires the `SkillRuntime` into the agent via `AgentBuilder::with_skill_engine()`
//! - The agent's system prompt is augmented with the skill inventory
//! - Demonstrates resolving and rendering a specific skill by ID
//!
//! ## What you'll learn
//! - The skill resolution pipeline: Source -> Engine -> Runtime -> AgentBuilder
//! - Creating skills from different sources (in-memory, filesystem)
//! - How skills compose with the system prompt via the inventory section
//! - How `SkillRuntime` type-erases the engine for the agent loop
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... cargo run --example 012-skills-loading --features jsonl-store,skills
//! ```

use std::sync::Arc;

use indexmap::IndexMap;
use meerkat::{AgentBuilder, AgentFactory, AnthropicClient, SkillId, SkillRuntime, SkillScope};
use meerkat_core::skills::SkillEngine as _;
use meerkat_core::skills::{SkillDescriptor, SkillDocument};
use meerkat_skills::source::SourceNode;
use meerkat_skills::{
    CompositeSkillSource, DefaultSkillEngine, FilesystemSkillSource, InMemorySkillSource,
    NamedSource,
};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    let store_dir = tempfile::tempdir()?.keep().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-5").await;

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    // ── Step 1: Create in-memory skills ──────────────────────────────────────
    //
    // InMemorySkillSource holds skill documents directly in memory.
    // This is useful for embedded/SDK scenarios where skills are bundled
    // with the application rather than loaded from the filesystem.

    let code_review_skill = SkillDocument {
        descriptor: SkillDescriptor {
            id: SkillId("review/rust-reviewer".to_string()),
            name: "Rust Code Reviewer".to_string(),
            description: "Reviews Rust code for idiomatic patterns, \
                          safety issues, and performance."
                .to_string(),
            scope: SkillScope::Builtin,
            ..Default::default()
        },
        body: r"## Role
You are a senior code reviewer specializing in Rust.

## Review Checklist
1. Check for `unwrap()` and `expect()` in library code
2. Verify error types implement `std::error::Error`
3. Look for unnecessary allocations (Vec where iter suffices)
4. Check ownership patterns (unnecessary clones, missing borrows)
5. Verify async boundaries are correct

## Output Format
For each finding, report:
- **File**: path/to/file.rs:line
- **Severity**: error | warning | suggestion
- **Issue**: Description of the problem
- **Fix**: Suggested correction

## Tone
Be direct and specific. No fluff. Focus on correctness and idiomatic Rust."
            .to_string(),
        extensions: IndexMap::new(),
    };

    let api_design_skill = SkillDocument {
        descriptor: SkillDescriptor {
            id: SkillId("review/api-designer".to_string()),
            name: "API Designer".to_string(),
            description: "Designs RESTful APIs following best practices.".to_string(),
            scope: SkillScope::Builtin,
            ..Default::default()
        },
        body: r"## Role
You are an API design consultant.

## Principles
1. Use resource-oriented URLs
2. HTTP methods map to CRUD operations
3. Return appropriate status codes
4. Use consistent error response format
5. Version via URL path prefix (/v1/, /v2/)"
            .to_string(),
        extensions: IndexMap::new(),
    };

    let inline_source = InMemorySkillSource::new(vec![code_review_skill, api_design_skill]);

    // ── Step 2: Create a filesystem-based skill ──────────────────────────────
    //
    // FilesystemSkillSource scans a directory tree for SKILL.md files.
    // The skill ID is derived from the relative path within the root.
    // Example: root/.rkat/skills/security/auditor/SKILL.md -> id "security/auditor"

    let skill_dir = tempfile::tempdir()?.keep();
    let security_skill_dir = skill_dir.join("security").join("auditor");
    std::fs::create_dir_all(&security_skill_dir)?;
    std::fs::write(
        security_skill_dir.join("SKILL.md"),
        r"---
name: Security Auditor
description: Audits code for common security vulnerabilities
---

## Role
You are a security auditor reviewing code for vulnerabilities.

## Checks
1. SQL injection via string concatenation
2. XSS in template rendering
3. Path traversal in file operations
4. Hardcoded secrets or API keys
5. Missing input validation

## Severity Levels
- CRITICAL: Exploitable in production
- HIGH: Likely exploitable with effort
- MEDIUM: Potential risk under specific conditions
- LOW: Best practice violation
",
    )?;

    let fs_source = FilesystemSkillSource::new(skill_dir.clone(), SkillScope::Project);

    // ── Step 3: Compose skill sources ────────────────────────────────────────
    //
    // CompositeSkillSource merges multiple named sources. When skills have
    // the same ID across sources, the first source wins (shadowing).
    // This enables layering: project skills override user skills override builtins.

    let composite_source = CompositeSkillSource::from_named(vec![
        NamedSource {
            name: "inline".to_string(),
            source: SourceNode::Memory(inline_source),
        },
        NamedSource {
            name: "filesystem".to_string(),
            source: SourceNode::Filesystem(fs_source),
        },
    ]);

    // ── Step 4: Build the skill engine ───────────────────────────────────────
    //
    // DefaultSkillEngine wraps a SkillSource and adds:
    //   - Capability filtering (skills requiring unavailable capabilities are hidden)
    //   - Inventory rendering (XML skill list for the system prompt)
    //   - Skill resolution and rendering (load + wrap in XML tags)

    let engine = DefaultSkillEngine::new(composite_source, vec![]);

    // ── Step 5: Demonstrate the engine directly ──────────────────────────────
    //
    // Before wiring into an agent, let's see what the skill engine produces.

    println!("=== Available Skills (Inventory) ===\n");
    let inventory = engine.inventory_section().await?;
    println!("{inventory}\n");

    println!("=== Resolved Skill: review/rust-reviewer ===\n");
    let resolved = engine
        .resolve_and_render(&[SkillId("review/rust-reviewer".to_string())])
        .await?;
    for skill in &resolved {
        println!("Name: {}", skill.name);
        println!("Size: {} bytes", skill.byte_size);
        println!("Rendered:\n{}\n", skill.rendered_body);
    }

    // ── Step 6: Create SkillRuntime and wire into agent ──────────────────────
    //
    // SkillRuntime type-erases the engine (which is generic over SkillSource)
    // into a Send + Sync + Clone runtime that the agent loop can use.
    // The agent loop calls:
    //   - inventory_section() to build the system prompt skill inventory
    //   - resolve_and_render() when /skill-ref directives are encountered

    let skill_runtime = Arc::new(SkillRuntime::new(Arc::new(engine)));

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt(
            "You are a code review assistant. Use the skills available to you \
             when reviewing code. Apply the appropriate skill based on the review type.",
        )
        .max_tokens_per_turn(2048)
        .with_skill_engine(skill_runtime)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await;

    // ── Step 7: Run the agent with skills ────────────────────────────────────

    println!("=== Running agent with skill engine wired in ===\n");

    let result = agent
        .run(
            r"Review this Rust function:

```rust
fn process_items(items: Vec<String>) -> Vec<String> {
    let mut results = Vec::new();
    for item in items.clone() {
        let processed = item.to_uppercase();
        if processed.len() > 0 {
            results.push(processed);
        }
    }
    results
}
```
"
            .to_string(),
        )
        .await?;

    println!("{}", result.text);

    // ── Skill system architecture reference ──────────────────────────────────

    println!("\n=== Skill System Architecture ===\n");
    println!(
        r#"Skill resolution pipeline:

  SkillSource (where skills live)
    |-- InMemorySkillSource    (inline / SDK embedding)
    |-- FilesystemSkillSource  (SKILL.md files in directories)
    |-- GitSkillSource         (cloned git repositories)
    |-- HttpSkillSource        (fetched from URLs)
    |-- EmbeddedSkillSource    (inventory-registered builtins)
    |-- ExternalSkillSource    (stdio protocol with external process)
    |
    v
  CompositeSkillSource (merges named sources with shadowing)
    |
    v
  DefaultSkillEngine (capability filtering + rendering)
    |
    v
  SkillRuntime (type-erased, Send + Sync + Clone)
    |
    v
  AgentBuilder::with_skill_engine()
    |
    v
  Agent loop:
    - inventory_section() -> system prompt augmentation
    - resolve_and_render() -> on-demand skill activation

SKILL.md file format:
  ---
  name: Shell Patterns
  description: "Background job workflows"
  requires_capabilities: [builtins, shell]
  ---
  # Shell Patterns
  ... markdown body ...

Config (.rkat/config.toml):
  [[skills]]
  source = "path"
  path = ".rkat/skills/"

  [[skills]]
  source = "git"
  url = "https://github.com/org/skills.git"

CLI usage:
  rkat run --skill review/rust-reviewer "Review this code..."
"#
    );

    Ok(())
}
