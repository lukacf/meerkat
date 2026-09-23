#![allow(clippy::expect_used)]

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
//! - Explicitly activates the review skill through a typed per-turn key
//! - Demonstrates inventory rendering separately for the host terminal
//!
//! ## What you'll learn
//! - The skill resolution pipeline: Source -> Engine -> Runtime -> AgentBuilder
//! - Creating skills from different sources (in-memory, filesystem)
//! - Registration, inventory visibility, and body activation are separate steps
//! - How `SkillRuntime` type-erases the engine for the agent loop
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=... ./scripts/repo-cargo run -p meerkat --example 012-skills-loading --features jsonl-store,skills
//! ```

use std::{path::Path, sync::Arc};

use indexmap::IndexMap;
use meerkat::{AgentBuilder, AgentFactory, AnthropicClient, SkillRuntime, SkillScope};
use meerkat_core::skills::SkillEngine as _;
use meerkat_core::skills::{
    SkillDescriptor, SkillDocument, SkillKey, SkillName, SourceIdentityRecord,
    SourceIdentityRegistry, SourceIdentityStatus, SourceTransportKind, SourceUuid,
};
use meerkat_skills::source::SourceNode;
use meerkat_skills::{
    CompositeSkillSource, DefaultSkillEngine, FilesystemSkillSource, InMemorySkillSource,
    NamedSource,
};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;

type ExampleError = Box<dyn std::error::Error + Send + Sync>;

fn main() -> Result<(), ExampleError> {
    meerkat_runtime::host_stack::run_host("skills-loading", run)?
}

fn review_key() -> SkillKey {
    SkillKey::builtin(SkillName::parse("review-rust-reviewer").expect("valid skill name"))
}

fn skill_engine(
    skill_dir: &Path,
) -> Result<DefaultSkillEngine<CompositeSkillSource>, ExampleError> {
    // ── Step 1: Create in-memory skills ──────────────────────────────────────
    //
    // InMemorySkillSource holds skill documents directly in memory.
    // This is useful for embedded/SDK scenarios where skills are bundled
    // with the application rather than loaded from the filesystem.

    let code_review_skill = SkillDocument {
        descriptor: {
            let mut d = SkillDescriptor::new(
                review_key(),
                "Rust Code Reviewer",
                "Reviews Rust code for idiomatic patterns, \
                          safety issues, and performance.",
            );
            d.scope = SkillScope::Builtin;
            d
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
        descriptor: {
            let mut d = SkillDescriptor::new(
                SkillKey::builtin(
                    SkillName::parse("review-api-designer").expect("valid skill name"),
                ),
                "API Designer",
                "Designs RESTful APIs following best practices.",
            );
            d.scope = SkillScope::Builtin;
            d
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
    // Use a direct child whose directory and frontmatter share a lowercase slug.
    // Canonical identity is (source UUID, skill name), not a slash-delimited path.

    let security_skill_dir = skill_dir.join("security-auditor");
    std::fs::create_dir_all(&security_skill_dir)?;
    std::fs::write(
        security_skill_dir.join("SKILL.md"),
        r"---
name: security-auditor
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

    let fs_source = FilesystemSkillSource::new_with_identity(
        skill_dir.to_path_buf(),
        SkillScope::Project,
        SourceUuid::project_local(),
        Default::default(),
    );

    // ── Step 3: Compose skill sources ────────────────────────────────────────
    //
    // Composition preserves each source UUID. Equal names in different sources
    // are distinct skills; only identical full SkillKeys can shadow.

    let inline_identity = SourceIdentityRecord {
        source_uuid: SourceUuid::builtin(),
        display_name: "inline".to_string(),
        transport_kind: SourceTransportKind::Embedded,
        fingerprint: "example:inline".to_string(),
        status: SourceIdentityStatus::Active,
    };
    let filesystem_identity = SourceIdentityRecord {
        source_uuid: SourceUuid::project_local(),
        display_name: "filesystem".to_string(),
        transport_kind: SourceTransportKind::Filesystem,
        fingerprint: format!("example:filesystem:{}", skill_dir.display()),
        status: SourceIdentityStatus::Active,
    };
    let registry = Arc::new(SourceIdentityRegistry::build(
        vec![inline_identity.clone(), filesystem_identity.clone()],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    )?);
    let composite_source = CompositeSkillSource::from_named_with_registry(
        vec![
            NamedSource {
                identity: inline_identity,
                source: SourceNode::Memory(inline_source),
            },
            NamedSource {
                identity: filesystem_identity,
                source: SourceNode::Filesystem(fs_source),
            },
        ],
        registry,
    );

    // ── Step 4: Build the skill engine ───────────────────────────────────────
    //
    // DefaultSkillEngine wraps a SkillSource and adds:
    //   - Capability filtering (skills requiring unavailable capabilities are hidden)
    //   - Inventory rendering (XML skill list for the system prompt)
    //   - Skill resolution and rendering (load + wrap in XML tags)

    Ok(DefaultSkillEngine::new(composite_source, vec![]))
}

async fn run() -> Result<(), ExampleError> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;
    let scratch = tempfile::tempdir_in(std::env::current_dir()?)?;
    let store_dir = scratch.path().join("sessions");
    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-6").await;
    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));
    let engine = skill_engine(&scratch.path().join("skills"))?;
    // ── Step 5: Demonstrate the engine directly ──────────────────────────────
    //
    // Before wiring into an agent, let's see what the skill engine produces.

    println!("=== Available Skills (Inventory) ===\n");
    let inventory = engine.inventory_section().await?;
    println!("{inventory}\n");

    println!("=== Resolved Skill: review-rust-reviewer ===\n");
    let resolved = engine.resolve_and_render(&[review_key()]).await?;
    for skill in &resolved {
        println!("Name: {}", skill.name);
        println!("Size: {} bytes", skill.byte_size);
        println!("Rendered:\n{}\n", skill.rendered_body);
    }

    // ── Step 6: Create SkillRuntime and wire into agent ──────────────────────
    //
    // SkillRuntime type-erases the engine (which is generic over SkillSource)
    // into a Send + Sync + Clone runtime that the agent loop can use.
    // Registration alone does not expose inventory or activate bodies. This
    // example retains EmptyToolDispatcher: no on-demand skill tools are exposed.

    let skill_runtime = Arc::new(SkillRuntime::new(Arc::new(engine)));

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-6")
        .system_prompt(
            "You are a code review assistant. Apply the explicitly activated review skill.",
        )
        .max_tokens_per_turn(2048)
        .with_skill_engine(skill_runtime)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await?;
    agent.pending_skill_references = Some(vec![review_key()]);

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
            .into(),
        )
        .await?;

    println!("{}", result.text);

    // ── Skill system architecture reference ──────────────────────────────────

    println!("\n=== Skill System Architecture ===\n");
    println!(
        r"Skill resolution pipeline:

  SkillSource (where skills live)
    |-- InMemorySkillSource    (inline / SDK embedding)
    |-- FilesystemSkillSource  (SKILL.md files in directories)
    |-- GitSkillSource         (cloned git repositories)
    |-- HttpSkillSource        (fetched from URLs)
    |-- EmbeddedSkillSource    (inventory-registered builtins)
    |-- ExternalSkillSource    (stdio protocol with external process)
    |
    v
  CompositeSkillSource (canonical keys retain source identity)
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
    - pending_skill_references -> typed per-turn body activation
    - emits SkillsResolved and appends canonical SkillContext blocks

Printing inventory on the host is not model visibility. A host may explicitly
compose inventory and discovery tools; this example activates only the review
body, without granting any tools.

For local filesystem skills, resolve the typed source UUID and SkillKey through
the RPC or SDK skill APIs, then preload that typed reference on session create.
"
    );
    println!("SKILL.md format (in a shell-patterns/ directory):\n{SHELL_SKILL}");
    println!("Config (.rkat/config.toml):\n{SKILLS_CONFIG}");
    println!(
        "CLI preload (builtins only; compatible with the default Safe tools):\n  {PRELOAD_COMMAND}"
    );

    Ok(())
}

const SHELL_SKILL: &str = r#"---
name: shell-patterns
description: "Background job workflows"
requires_capabilities: [builtins, shell]
---
# Shell Patterns
Use explicit working directories and inspect command results.
"#;

const SKILLS_CONFIG: &str = r#"[skills]
[[skills.repositories]]
name = "project-examples"
source_uuid = "11111111-1111-4111-8111-111111111111"
type = "filesystem"
path = ".rkat/skills/"

[[skills.repositories]]
name = "team-examples"
source_uuid = "22222222-2222-4222-8222-222222222222"
type = "git"
url = "https://github.com/org/skills.git"
"#;

const PRELOAD_COMMAND: &str =
    "rkat run --skill builtin-utilities-workflow \"Explain the builtin utility workflow.\"";

#[cfg(test)]
mod tests;
