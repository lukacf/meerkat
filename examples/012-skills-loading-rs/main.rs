//! # 012 — Skills Loading (Rust)
//!
//! Skills inject domain-specific knowledge and behavioral instructions into
//! agents at runtime. They can be loaded from files, inline content, git repos,
//! or HTTP endpoints.
//!
//! ## What you'll learn
//! - What skills are and when to use them
//! - Loading skills from different sources
//! - Referencing skills in session creation
//! - How skills compose with the system prompt
//!
//! ## Run
//! ```bash
//! ANTHROPIC_API_KEY=your-key cargo run --example 012_skills_loading
//! ```

use std::sync::Arc;

use meerkat::{
    AgentBuilder, AgentFactory, AnthropicClient,
};
use meerkat_store::{JsonlStore, StoreAdapter};
use meerkat_tools::EmptyToolDispatcher;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .map_err(|_| "Set ANTHROPIC_API_KEY to run this example")?;

    let store_dir = tempfile::tempdir()?.into_path().join("sessions");
    std::fs::create_dir_all(&store_dir)?;

    let factory = AgentFactory::new(store_dir.clone());
    let client = Arc::new(AnthropicClient::new(api_key)?);
    let llm = factory.build_llm_adapter(client, "claude-sonnet-4-5").await;

    let store = Arc::new(JsonlStore::new(store_dir));
    store.init().await?;
    let store = Arc::new(StoreAdapter::new(store));

    // Skills are injected into the system prompt before the agent runs.
    // They provide domain-specific instructions, behavioral patterns,
    // and tool usage guidance.

    let code_review_skill = r#"
## Role
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
Be direct and specific. No fluff. Focus on correctness and idiomatic Rust.
"#;

    // Build agent with the skill injected as system prompt content
    let system_prompt = format!(
        "You are a code review assistant.\n\n<skill>\n{}\n</skill>",
        code_review_skill.trim()
    );

    let mut agent = AgentBuilder::new()
        .model("claude-sonnet-4-5")
        .system_prompt(&system_prompt)
        .max_tokens_per_turn(2048)
        .build(Arc::new(llm), Arc::new(EmptyToolDispatcher), store)
        .await;

    println!("=== Code Review Agent with Loaded Skill ===\n");

    let result = agent
        .run(
            r#"Review this Rust function:

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
"#
            .to_string(),
        )
        .await?;

    println!("{}", result.text);

    // ── Skill configuration reference ──────────────────────────────────────

    println!("\n\n=== Skill Configuration Reference ===\n");
    println!(
        r#"Skills can be loaded from multiple sources:

# .rkat/config.toml

# 1. Inline skill
[[skills]]
id = "code-reviewer"
source = "inline"
content = """
## Role
You are a code reviewer...
"""

# 2. File-based skill
[[skills]]
id = "api-designer"
source = "path"
path = ".rkat/skills/api-designer.md"

# 3. Git-based skill (fetched at runtime)
[[skills]]
id = "security-auditor"
source = "git"
url = "https://github.com/org/skills.git"
path = "security/auditor.md"

# 4. HTTP-based skill (fetched at runtime)
[[skills]]
id = "compliance-checker"
source = "http"
url = "https://skills.example.com/compliance.md"

# Reference skills in CLI:
#   rkat run --skill code-reviewer "Review this file..."
#   rkat run --skill code-reviewer --skill security-auditor "Audit this..."

# Reference skills in Python/TypeScript SDK:
#   client.create_session("...", skill_references=["code-reviewer"])
"#
    );

    Ok(())
}
