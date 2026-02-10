# Skills

Meerkat's skill system provides domain-specific knowledge injection into the agent's context. Skills are markdown documents with YAML frontmatter that can be discovered from multiple sources, gated by required capabilities, and injected into the system prompt or per-turn context.

## Core Types

All core types are defined in `meerkat-core/src/skills/mod.rs`:

### `SkillId`

Newtype wrapper for type-safe skill identification:

```rust
pub struct SkillId(pub String);
```

### `SkillScope`

Where a skill was discovered:

| SkillScope | Source |
|------------|--------|
| `Builtin` | Embedded in a component crate via `inventory::submit!` |
| `Project` | `.rkat/skills/` directory in the project root |
| `User` | `~/.rkat/skills/` directory in the user's home |

### `SkillDescriptor`

Metadata describing a skill:

```rust
pub struct SkillDescriptor {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub scope: SkillScope,
    pub requires_capabilities: Vec<String>,
}
```

### `SkillDocument`

A loaded skill with full content:

```rust
pub struct SkillDocument {
    pub descriptor: SkillDescriptor,
    pub body: String,
    pub extensions: IndexMap<String, String>,
}
```

### `SkillError`

Error variants for skill operations:

| Variant | When |
|---------|------|
| `NotFound { id }` | Skill ID not found in any source |
| `CapabilityUnavailable { id, capability }` | Skill requires a capability that is not available |
| `Ambiguous { reference, matches }` | Reference matches multiple skills |
| `Load(Cow<'static, str>)` | I/O or loading failure |
| `Parse(Cow<'static, str>)` | Frontmatter or content parsing failure |

## Skill Sources

Skill sources implement the `SkillSource` trait (from `meerkat-core/src/skills/mod.rs`):

```rust
#[async_trait]
pub trait SkillSource: Send + Sync {
    async fn list(&self) -> Result<Vec<SkillDescriptor>, SkillError>;
    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError>;
}
```

### `EmbeddedSkillSource`

Collects skills registered at compile time via `inventory::submit!`. Uses `collect_registered_skills()` to iterate over static `SkillRegistration` entries.

### `FilesystemSkillSource`

Reads from a directory where each skill is a subdirectory containing a `SKILL.md` file:

```
.rkat/skills/
  my-skill/
    SKILL.md
  another-skill/
    SKILL.md
```

The subdirectory name becomes the `SkillId`.

### `CompositeSkillSource`

Wraps multiple sources with deduplication. **First source wins** for duplicate skill IDs -- if two sources provide a skill with the same ID, the source added first takes precedence.

### `InMemorySkillSource`

Test-only source backed by a `Vec<SkillDocument>`. Used in unit tests.

## Source Precedence

The `AgentFactory::build_agent()` method (in `meerkat/src/factory.rs`) constructs the `CompositeSkillSource` with sources in this order:

1. **Project** (`.rkat/skills/`) -- highest precedence
2. **User** (`~/.rkat/skills/`)
3. **Embedded** (builtin skills from component crates)

Because `CompositeSkillSource` uses first-source-wins deduplication, a project skill with the same ID as a builtin skill overrides the builtin.

## SKILL.md Format

Each skill is a markdown file with YAML frontmatter parsed by `parse_skill_md()` in `meerkat-skills/src/parser.rs`:

```markdown
---
name: Shell Patterns
description: "Background job workflows: patterns and tips"
requires_capabilities: [builtins, shell]
---

# Shell Patterns

When running background jobs...
```

### Frontmatter fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | `String` | Yes | Display name for the skill |
| `description` | `String` | Yes | One-line description |
| `requires_capabilities` | `Vec<String>` | No (default `[]`) | Capability IDs that must be available |

The frontmatter is delimited by `---` markers. The body (everything after the closing `---`) is stored as the skill's `body` field.

## Capability Gating

Skills are filtered by required capabilities before being shown or injected. The `DefaultSkillEngine` (in `meerkat-skills/src/engine.rs`) filters the inventory using `filter_by_capabilities()`:

- A skill's `requires_capabilities` field lists capability ID strings.
- The engine holds a set of `available_capabilities` (provided at construction).
- A skill is included only if **all** its required capabilities are present in the available set.
- A skill with an empty `requires_capabilities` list is always available.

## Skill Resolution

The resolver (in `meerkat-skills/src/resolver.rs`) supports two reference formats:

1. **Slash-prefix ID**: `/skill-id` -- matches against `SkillDescriptor.id`.
2. **Exact name match** (case-insensitive): `"Shell Patterns"` or `"shell patterns"` -- matches against `SkillDescriptor.name`.

Resolution returns `SkillError::NotFound` if no match, or `SkillError::Ambiguous` if multiple skills match.

The `SkillResolutionMode` enum (in `meerkat-skills/src/config.rs`) currently has one variant:

| Mode | Behavior |
|------|----------|
| `Explicit` | Only explicit `/skill-id` or exact name matches |

## Rendering

The renderer (in `meerkat-skills/src/renderer.rs`) produces two output formats:

### Inventory section (`render_inventory`)

Generates a `## Available Skills` section listing all available skills for the system prompt:

```
## Available Skills

- `/task-workflow` -- How to use task_create/task_update/task_list for structured work tracking
- `/shell-patterns` -- Background job patterns with shell and job management tools
```

This section is injected into the system prompt via the `extra_sections` parameter of `assemble_system_prompt()` in `meerkat/src/prompt_assembly.rs`.

### Injection block (`render_injection`)

Wraps the skill body in XML-style tags for per-turn injection:

```xml
<skill name="Shell Patterns">
# Shell Patterns

When running background jobs...
</skill>
```

### Size limits

`MAX_INJECTION_BYTES` is set to 32 KiB (32 * 1024 bytes). If an injection block exceeds this limit, it is truncated with a warning log.

## Skill Engine

The `SkillEngine` trait (from `meerkat-core/src/skills/mod.rs`):

```rust
#[async_trait]
pub trait SkillEngine: Send + Sync {
    async fn inventory_section(&self) -> Result<String, SkillError>;
    async fn resolve_and_render(
        &self,
        references: &[String],
        available_capabilities: &[String],
    ) -> Result<String, SkillError>;
}
```

The `DefaultSkillEngine` implementation:
- `inventory_section()`: Lists all skills filtered by available capabilities, renders with `render_inventory()`.
- `resolve_and_render()`: Resolves each reference, loads the skill document, renders injection blocks, joins with `\n\n`.

## Embedded Skills Registration

Component crates register skills at compile time using the `inventory` crate. The `SkillRegistration` struct (in `meerkat-skills/src/registration.rs`):

```rust
pub struct SkillRegistration {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub scope: SkillScope,
    pub requires_capabilities: &'static [&'static str],
    pub body: &'static str,
    pub extensions: &'static [(&'static str, &'static str)],
}
```

Registration example (from `meerkat-hooks/src/lib.rs`):

```rust
inventory::submit! {
    meerkat_skills::SkillRegistration {
        id: "hook-authoring",
        name: "Hook Authoring",
        description: "Writing hooks for the 7 hook points, execution modes, and decision semantics",
        scope: meerkat_core::skills::SkillScope::Builtin,
        requires_capabilities: &["hooks"],
        body: include_str!("../skills/hook-authoring/SKILL.md"),
        extensions: &[],
    }
}
```

All registered skills are collected at runtime via `collect_registered_skills()`.

## Embedded Skills

The following skills are embedded in component crates:

| ID | Name | Crate | Required capabilities |
|----|------|-------|----------------------|
| `hook-authoring` | Hook Authoring | `meerkat-hooks` | `hooks` |
| `shell-patterns` | Shell Patterns | `meerkat-tools` | `builtins`, `shell` |
| `task-workflow` | Task Workflow | `meerkat-tools` | `builtins` |
| `sub-agent-orchestration` | Sub-Agent Orchestration | `meerkat-tools` | `sub_agents` |
| `mcp-server-setup` | MCP Server Setup | `meerkat-mcp` | (none) |
| `memory-retrieval` | Memory Retrieval | `meerkat-memory` | `memory_store` |
| `session-management` | Session Management | `meerkat-session` | `session_store` |
| `multi-agent-comms` | Multi-Agent Comms | `meerkat-comms` | `comms` |

## Configuration

### `SkillsConfig`

From `meerkat-skills/src/config.rs`:

```rust
pub struct SkillsConfig {
    pub enabled: bool,              // Default: true
    pub max_injection_bytes: usize, // Default: 32 * 1024 (32 KiB)
}
```

## Custom Skills

### Project-level skills

Create a subdirectory under `.rkat/skills/` with a `SKILL.md` file:

```
.rkat/skills/
  my-deployment/
    SKILL.md
```

Example `.rkat/skills/my-deployment/SKILL.md`:

```markdown
---
name: Deployment Guide
description: Project-specific deployment procedures
requires_capabilities: []
---

# Deployment Guide

1. Run `make build` to create the release binary.
2. Deploy via `./deploy.sh production`.
```

### User-level skills

Same structure under `~/.rkat/skills/`. These are available across all projects but have lower precedence than project-level skills.

## Wire Parameters

Skills are controlled per-request via `SkillsParams` (from `meerkat-contracts/src/wire/params.rs`):

```rust
pub struct SkillsParams {
    pub skills_enabled: bool,
    pub skill_references: Vec<String>,
}
```

## SDK Usage

### Building a skill engine

```rust
use meerkat_skills::{
    DefaultSkillEngine, CompositeSkillSource, EmbeddedSkillSource,
    FilesystemSkillSource,
};
use meerkat_core::skills::SkillScope;
use std::path::PathBuf;

let embedded = EmbeddedSkillSource::new();
let project = FilesystemSkillSource::new(
    PathBuf::from(".rkat/skills"),
    SkillScope::Project,
);

let composite = CompositeSkillSource::new(vec![
    Box::new(project),   // Project first (highest precedence)
    Box::new(embedded),  // Embedded last
]);

let engine = DefaultSkillEngine::new(
    Box::new(composite),
    vec!["builtins".to_string(), "shell".to_string()],
);

// Get inventory section for system prompt
let inventory = engine.inventory_section().await?;

// Resolve and render specific skills
let injection = engine
    .resolve_and_render(
        &["/shell-patterns".to_string()],
        &["builtins".to_string(), "shell".to_string()],
    )
    .await?;
```
