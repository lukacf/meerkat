# Meerkat Platform Roadmap

## Summary

Baseline: `origin/main` at PR #12 (`1fa2215`). Session lifecycle centralized
via `SessionService` + `AgentFactory`. Wire types still per-surface with
real duplication. Prompt assembly has gaps. No skill system. No external SDKs.

Goal: build a unified platform in five phases — clean foundation, protocol
alignment, skill system, external SDKs, and the SDK Builder. Every phase
builds on the previous. The SDK Builder is the destination: given a feature
manifest, produce a matched runtime binary + SDK package where the SDK surface
exactly reflects the runtime's compiled capabilities, including which skills
are available.

---

## Phase 0 — Prompt Assembly Unification

Standalone prerequisite. Can land immediately as its own PR. Fixes real bugs
and establishes the single-path prompt assembly that both contracts and skills
need.

### Current state (verified)

In `meerkat/src/factory.rs:610-618`, `build_agent()` composes the system prompt:

```rust
let mut system_prompt = match build_config.system_prompt {
    Some(prompt) => prompt,
    None => SystemPromptConfig::new().compose().await,
};
if !tool_usage_instructions.is_empty() {
    system_prompt.push_str("\n\n");
    system_prompt.push_str(&tool_usage_instructions);
}
```

Three config fields exist in `AgentConfig` but are **never read** by the factory:

| Field | Exists in config | Merged in `Config::merge()` | Read in `build_agent()` |
|-------|------------------|-----------------------------|------------------------|
| `agent.system_prompt` | Yes (`config.rs:607`) | Yes (`config.rs:219`) | **No** |
| `agent.system_prompt_file` | Yes (`config.rs:609`) | Yes (`config.rs:220`) | **No** |
| `agent.tool_instructions` | Yes (`config.rs:611`) | Yes (`config.rs:222`) | **No** |

`SystemPromptConfig::new()` creates a default config that loads `AGENTS.md`
files (global `~/.rkat/AGENTS.md` + project `./AGENTS.md`/`.rkat/AGENTS.md`,
capped at 32 KiB each). But it never checks `config.agent.*` fields.

Tool instructions come from `CompositeDispatcher::usage_instructions()`, not
from `config.agent.tool_instructions`.

### Fix

Consolidate prompt assembly into a single function in the facade with clear
precedence:

```rust
/// Assemble the final system prompt. Single canonical path.
pub async fn assemble_system_prompt(
    config: &Config,
    build_config: &AgentBuildConfig,
    tool_usage_instructions: &str,
) -> String {
    // 1. Per-request override wins outright (skips AGENTS.md and config).
    if let Some(ref prompt) = build_config.system_prompt {
        return append_sections(prompt, &[], tool_usage_instructions);
    }

    // 2-4. Use SystemPromptConfig to compose the base prompt.
    //
    // IMPORTANT: Wire config overrides INTO SystemPromptConfig rather than
    // bypassing it. This preserves AGENTS.md loading when config-level
    // overrides are set — a user setting `agent.system_prompt` in config
    // wants to override the DEFAULT prompt, not lose AGENTS.md content.
    let mut spc = SystemPromptConfig::new();

    // 2. Config-level file override → feeds into SystemPromptConfig.
    if let Some(ref path) = config.agent.system_prompt_file {
        match tokio::fs::read_to_string(path).await {
            Ok(content) => spc.system_prompt = Some(content),
            Err(_) => tracing::warn!("system_prompt_file not readable: {}", path.display()),
        }
    }

    // 3. Config-level inline override (lower precedence than file).
    if spc.system_prompt.is_none() {
        if let Some(ref prompt) = config.agent.system_prompt {
            spc.system_prompt = Some(prompt.clone());
        }
    }

    // 4. compose() uses the override if set, otherwise DEFAULT_SYSTEM_PROMPT.
    //    Either way, AGENTS.md files are appended (global + project).
    let base = spc.compose().await;

    // 5. Append config-level tool instructions (if any) before dispatcher instructions.
    let config_tool_instructions = config.agent.tool_instructions.as_deref().unwrap_or("");
    append_sections(&base, &[config_tool_instructions], tool_usage_instructions)
}

fn append_sections(base: &str, extra_sections: &[&str], tool_instructions: &str) -> String {
    let mut prompt = base.to_string();
    for section in extra_sections {
        if !section.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(section);
        }
    }
    if !tool_instructions.is_empty() {
        prompt.push_str("\n\n");
        prompt.push_str(tool_instructions);
    }
    prompt
}
```

The `extra_sections` parameter is a forward-compatible slot — Phase 3 (skills)
will pass the skill inventory section here. For now it's empty.

### Phase 0 acceptance

- Prompt assembly is a single function with documented precedence.
- All config prompt fields that exist are wired (verified by research).
- Existing tests pass (no behavior change when overrides are absent).
- New tests cover each precedence level.

---

## Phase 1 — Contracts Foundation

Create `meerkat-contracts` as the single source of truth for all wire-facing
types, capability model, error contracts, and schema emission.

### 1.1 Create `meerkat-contracts` crate

```
meerkat-contracts/
  src/
    lib.rs
    wire/
      mod.rs
      usage.rs       — WireUsage
      result.rs      — WireRunResult
      event.rs       — WireEvent envelope
      session.rs     — WireSessionInfo, WireSessionSummary
      params.rs      — Composable request building blocks
    error/
      mod.rs         — WireError, ErrorCode (enum), ErrorCategory (enum)
    capability/
      mod.rs         — CapabilityId (enum), CapabilityScope, CapabilityStatus
      registry.rs    — Distributed registration via `inventory`, resolution layers
      query.rs       — CapabilitiesResponse
    protocol.rs      — Protocol enum (Rpc, Rest, Mcp, Cli)
    version.rs       — ContractVersion (Ord, Display, FromStr, Copy)
    emit.rs          — Schema emission (bin target, feature-gated)
```

Dependency: `meerkat-contracts` depends on `meerkat-core`. Surface crates
depend on `meerkat-contracts`. The facade re-exports contracts types.

#### Rust conventions

- All identifier types are **enums, not strings** (`CapabilityId`, `ErrorCode`,
  `ErrorCategory`, `Protocol`). Exhaustive matching prevents silent omissions.
- `JsonSchema` derives are **feature-gated**: `#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]`.
  The `emit-schemas` binary enables `--features schema`. Normal builds don't
  pay for `schemars`.
- `Cow<'static, str>` for error messages to avoid allocation for static strings.

### 1.2 Typed capability model

```rust
/// Every capability known to Meerkat. Adding a variant forces updates to
/// the registry, error mappings, and codegen templates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[derive(strum::EnumIter, strum::EnumString, strum::Display)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum CapabilityId {
    Sessions,
    Streaming,
    StructuredOutput,
    Hooks,
    Builtins,
    Shell,
    Comms,
    SubAgents,
    MemoryStore,
    SessionStore,
    SessionCompaction,
    Skills,        // ← Phase 3 adds this variant
}
```

The `Skills` variant is added in Phase 1 as a placeholder (registered as a
capability in Phase 3 when `meerkat-skills` exists). This avoids a contract
version bump later.

#### Distributed registration via `inventory`

```rust
pub struct CapabilityRegistration {
    pub id: CapabilityId,
    pub description: &'static str,
    pub scope: CapabilityScope,
    pub requires_feature: Option<&'static str>,
    pub prerequisites: &'static [CapabilityId],
}

inventory::collect!(CapabilityRegistration);

pub fn build_capabilities() -> Vec<&'static CapabilityRegistration> {
    inventory::iter::<CapabilityRegistration>.into_iter().collect()
}
```

Feature-gated crates self-register. Adding a capability = add enum variant +
`inventory::submit!` in the providing crate.

`CapabilityId` derives `Ord`. `build_capabilities()` sorts collected
registrations by `CapabilityId` ordinal before returning, ensuring
deterministic ordering regardless of `inventory` collection order. This is
required for deterministic schema emission — the emitted `capabilities.json`
must be identical across builds.

#### Capability scope and status

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum CapabilityScope {
    Universal,
    Extension { protocols: Vec<Protocol> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub enum CapabilityStatus {
    /// Compiled in, config-enabled, protocol supports it.
    Available,
    /// Compiled in but disabled by policy.
    ///
    /// `description` summarizes why — this is intentionally a human-readable
    /// string, not a config path, because the reality is multi-layered:
    /// always-compiled features like `Builtins` and `Shell` are controlled by
    /// `AgentFactory` flags → per-build `AgentBuildConfig` overrides →
    /// `BuiltinToolConfig` policy layers (soft + enforced). A single
    /// "config_path" can't represent that resolution chain.
    DisabledByPolicy { description: Cow<'static, str> },
    /// Not compiled into this build (feature flag absent).
    NotCompiled { feature: Cow<'static, str> },
    /// This protocol surface doesn't support it.
    NotSupportedByProtocol { reason: Cow<'static, str> },
}
```

Resolution layers: build (`inventory`) → config/policy → surface.

Note on always-compiled features: `Builtins`, `Shell`, and `Hooks` don't have
Cargo feature flags — they're always linked. Their capability status is
resolved at the config/policy layer via the three-tier tool policy system
(`ToolPolicyLayer` soft policies → `EnforcedToolPolicy` hard constraints →
per-tool `default_enabled()`). The `DisabledByPolicy` variant covers this
case with a descriptive string rather than pretending there's a single config
knob.

### 1.3 Typed error envelope

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[derive(strum::EnumString, strum::Display)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    SessionNotFound,
    SessionBusy,
    SessionNotRunning,
    ProviderError,
    BudgetExhausted,
    HookDenied,
    AgentError,
    CapabilityUnavailable,
    SkillNotFound,         // ← Phase 3 uses this
    SkillResolutionFailed, // ← Phase 3 uses this
    InvalidParams,
    InternalError,
}

impl ErrorCode {
    pub fn jsonrpc_code(self) -> i32 { ... }
    pub fn http_status(self) -> u16 { ... }
    pub fn cli_exit_code(self) -> i32 { ... }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireError {
    pub code: ErrorCode,
    pub category: ErrorCategory,
    pub message: Cow<'static, str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_hint: Option<CapabilityHint>,
}
```

Replaces per-surface error construction. Surfaces map `WireError` to their
native format (RPC code, HTTP status, CLI exit code).

### 1.4 Composable request fragments

Contracts defines canonical field sets. Protocol crates inline the fields they
support and provide accessor methods returning the fragment type.

```rust
pub struct CoreCreateParams { prompt, model, provider, max_tokens, system_prompt }
pub struct StructuredOutputParams { output_schema, structured_output_retries }
pub struct CommsParams { host_mode, comms_name }
pub struct HookParams { hooks_override }
pub struct SkillsParams { skills_enabled, skill_references }  // ← Phase 3
```

No `#[serde(flatten)]` — explicit delegation to avoid known serde/schemars issues.

### 1.5 Canonical wire response types

```rust
pub struct WireUsage { input_tokens, output_tokens, total_tokens, cache_creation_tokens?, cache_read_tokens? }
pub struct WireRunResult { session_id, text, turns, tool_calls, usage, structured_output?, schema_warnings? }
pub struct WireEvent { session_id, sequence, event: AgentEvent, contract_version: ContractVersion }
```

One `From<RunResult>` impl. All surfaces use these directly.

### 1.6 Contract versioning

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ContractVersion { major: u32, minor: u32, patch: u32 }

impl ContractVersion {
    /// Start at 0.1.0 — allow breaking changes during Phase 1-3 development.
    /// Bump to 1.0.0 when the SDK Builder ships (Phase 5) and external
    /// consumers exist. Before 1.0.0, minor bumps can be breaking.
    pub const CURRENT: Self = Self { major: 0, minor: 1, patch: 0 };
    pub fn is_compatible_with(&self, other: &Self) -> bool { ... }
}
// + Display, FromStr, Copy
```

Embedded in events, capabilities response, SDK packages, RPC handshake.

### 1.7 Schema emission

```
cargo run -p meerkat-contracts --features schema --bin emit-schemas
```

Outputs to `artifacts/schemas/`:
- `version.json`, `wire-types.json`, `params.json`, `events.json`
- `errors.json`, `capabilities.json`
- `rpc-methods.json`, `rest-openapi.json`

CI: run emitter, diff against committed artifacts.

### Phase 1 acceptance

- `meerkat-contracts` crate exists with all wire types, capability model,
  error envelope, versioning, schema emission.
- Typed enums for all identifiers. Feature-gated `JsonSchema`.
- `inventory`-based capability registration.
- Schema artifacts committed and CI-verified.
- `CapabilityId::Skills` variant exists (registered in Phase 3).
- `ErrorCode` includes skill-related codes (used in Phase 3).

---

## Phase 2 — Protocol Alignment

Migrate all four surfaces to consume contracts. Deduplicate helpers. Add
`capabilities/get` to every surface.

### 2.1 Surface migration

For each surface crate (RPC, REST, MCP Server, CLI):

1. Replace local wire response types with contracts imports.
2. Replace local error construction with `WireError` + protocol projection.
3. Compose request types from contract fragments via delegation.
4. Add `capabilities/get` (method / endpoint / tool / command).
5. Remove duplicated helpers.

Specific deduplication targets and their destinations:

- `resolve_host_mode()` → `meerkat-comms` as `meerkat_comms::validate_host_mode()`.
  THREE copies today (CLI `main.rs:980`, REST `lib.rs:200`, MCP Server `lib.rs:156`).
  This is comms validation logic — the comms crate owns it.
- `resolve_store_path()` → `meerkat-store` as `meerkat_store::resolve_store_path()`.
  Near-identical in REST and MCP Server. Storage path resolution belongs in the
  storage crate.
- `spawn_event_forwarder()` → `meerkat` (facade) as a generic helper in
  `meerkat/src/surface.rs`. Three variants across RPC, REST, MCP Server. This
  is cross-cutting surface infrastructure — the facade wires surfaces, so it
  owns shared surface helpers.
- Usage mapping → eliminated. All surfaces use `WireUsage` and `WireRunResult`
  from contracts directly. The three `RunResult` → wire conversion functions
  are replaced by one `From<RunResult> for WireRunResult` impl in contracts.
- `McpRouterAdapter` → `meerkat-mcp` as `meerkat_mcp::McpRouterAdapter`.
  Currently CLI-specific (`meerkat-cli/src/adapters.rs`). The MCP crate owns
  the `McpRouter`, it owns the adapter that bridges it to `AgentToolDispatcher`.
  All surfaces import from `meerkat-mcp`.

### 2.2 Capability registration for existing features

Each feature-gated crate adds its `inventory::submit!`:

Each feature-gated crate registers its capabilities via `inventory::submit!`:

| Crate | Capability | Feature flag | What it provides |
|-------|-----------|-------------|-----------------|
| (core, always) | `Sessions`, `Streaming` | None | Agent loop, session lifecycle, event streaming |
| meerkat-tools | `Builtins` | — | `task_list`, `task_create`, `task_get`, `task_update`, `wait`, `datetime` |
| meerkat-tools | `Shell` | — | `shell` (Nushell backend), `shell_jobs`, `shell_job_status`, `shell_job_cancel` |
| meerkat-tools | `SubAgents` | `sub-agents` | `agent_spawn`, `agent_fork`, `agent_status`, `agent_cancel`, `agent_list` |
| meerkat-comms | `Comms` | `comms` | `comms_send`, `comms_request`, `comms_response`, `comms_list_peers` + host mode |
| meerkat-memory | `MemoryStore` | `memory-store` | HNSW semantic search + redb persistence (indexes compaction discards) |
| meerkat-session | `SessionStore` | `session-store` | PersistentSessionService, RedbEventStore, SessionProjector |
| meerkat-session | `SessionCompaction` | `session-compaction` | DefaultCompactor (auto-compact at token threshold, LLM summary, history rebuild) |
| meerkat-hooks | `Hooks` | — | 7 hook points, 3 runtimes (in-process/command/HTTP), deny/allow/rewrite semantics |
| meerkat-skills | `Skills` | `skills` | Skill loading, resolution, injection (Phase 3) |

Notes on capability registration nuances:

- `Builtins`, `Shell`, and `Hooks` don't have feature flags — they're always
  compiled in but controlled by config (`tools.builtins_enabled`,
  `tools.shell_enabled`) and the three-tier tool policy system. Their capability
  status resolves via the config/policy layer as `DisabledByPolicy`, not the
  build layer.

- `Comms` tools are mounted differently from builtins — they're NOT part of
  `CompositeDispatcher`. Instead they're overlaid via `wrap_with_comms()` /
  `ToolGateway` to avoid circular dependencies (comms is infrastructure, not
  a domain tool). The capability registration and `DisabledByPolicy` handling
  must account for this separate mounting path.

### Phase 2 acceptance

- No wire response types defined outside contracts.
- All four surfaces expose `capabilities/get` with `CapabilitiesResponse`.
- Error mapping consistent: `WireError` → protocol format.
- Zero duplicated helpers across surface crates (`resolve_host_mode`, `resolve_store_path`, `spawn_event_forwarder` — all extracted).
- `McpRouterAdapter` relocated from `meerkat-cli/src/adapters.rs` to `meerkat-mcp`.
- All feature-gated crates self-register capabilities via `inventory`.
- Existing tests pass.

---

## Phase 3 — Skills System

Core contracts, `meerkat-skills` crate, factory integration, and embedded
skills. Built on top of the contracts foundation from Phases 1-2.

### 3.1 Core contracts (`meerkat-core/src/skills/`)

```rust
/// Skill identifier — newtype for type safety.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SkillId(pub String);

/// Where a skill was discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[derive(strum::EnumString, strum::Display)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    Builtin,   // embedded in a component crate
    Project,   // .rkat/skills/
    User,      // ~/.rkat/skills/ (consistent with ~/.rkat/AGENTS.md convention)
}

pub struct SkillDescriptor {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub scope: SkillScope,
    pub requires_capabilities: Vec<CapabilityId>,
}

pub struct SkillDocument {
    pub descriptor: SkillDescriptor,
    pub body: String,
    pub extensions: IndexMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill not found: {id}")]
    NotFound { id: SkillId },

    #[error("skill requires unavailable capability: {capability}")]
    CapabilityUnavailable { id: SkillId, capability: CapabilityId },

    #[error("ambiguous skill reference '{reference}' matches: {matches:?}")]
    Ambiguous { reference: String, matches: Vec<SkillId> },

    #[error("skill loading failed: {0}")]
    Load(Cow<'static, str>),

    #[error("skill parse failed: {0}")]
    Parse(Cow<'static, str>),
}

#[async_trait]
pub trait SkillSource: Send + Sync {
    async fn list(&self) -> Result<Vec<SkillDescriptor>, SkillError>;
    async fn load(&self, id: &SkillId) -> Result<SkillDocument, SkillError>;
}

#[async_trait]
pub trait SkillEngine: Send + Sync {
    async fn inventory_section(&self) -> Result<String, SkillError>;
    async fn resolve_and_render(
        &self,
        references: &[String],
        available_capabilities: &[CapabilityId],
    ) -> Result<String, SkillError>;
}
```

Agent integration: `AgentBuilder::skill_engine()` setter. Emits
`SkillsResolved` and `SkillResolutionFailed` events through existing
`WireEvent` envelope.

**Injection model**: The skill inventory (metadata listing) is part of the
system prompt — assembled during prompt construction (Phase 0's
`assemble_system_prompt` gains an `extra_sections` slot for this). Per-turn
skill injection (resolved skill bodies) is prepended to the user message as a
`<skill>` tagged block. Both are visible to hooks at `PreLlmRequest`, which
sees the full message list including system prompt and user message. No
ordering ambiguity — hooks always see skill content and can inspect, modify,
or deny it.

**Skill events** (added to `AgentEvent` in `meerkat-core/src/event.rs`):

```rust
/// Skills resolved for this turn.
SkillsResolved {
    skills: Vec<SkillId>,
    injection_bytes: usize,
},

/// A skill reference could not be resolved.
SkillResolutionFailed {
    reference: String,
    error: String,
},
```

### 3.2 `meerkat-skills` crate

```
meerkat-skills/
  src/
    lib.rs
    source/
      filesystem.rs    — FilesystemSkillSource
      embedded.rs      — EmbeddedSkillSource (from inventory)
      memory.rs        — InMemorySkillSource
      composite.rs     — CompositeSkillSource
    parser.rs          — SKILL.md frontmatter parser
    resolver.rs        — Explicit + optional implicit resolution
    renderer.rs        — Inventory section + injection blocks
    engine.rs          — DefaultSkillEngine
    config.rs          — SkillsConfig, SkillResolutionMode (enum)
    registration.rs    — SkillRegistration via inventory
```

Depends on `meerkat-core` and `meerkat-contracts`. No dependency on component
crates. Same `inventory` pattern as capabilities:

```rust
pub struct SkillRegistration {
    pub descriptor: SkillDescriptor,
    pub body: &'static str,
    pub extensions: &'static [(&'static str, &'static str)],
}
inventory::collect!(SkillRegistration);
```

#### SKILL.md frontmatter schema

```markdown
---
name: Shell Patterns                    # REQUIRED
description: Background job workflows   # REQUIRED
requires_capabilities: [builtins, shell] # OPTIONAL, list of CapabilityId string forms
---

# Shell Patterns

When running background jobs...
```

The `id` is derived from the directory name (filesystem source) or set
explicitly in the `SkillRegistration` (embedded source). The `scope` is set
by the source type, not frontmatter.

#### Resolver — mention syntax

Explicit skill references are detected by:
1. `/skill-id` syntax (slash prefix, e.g., `/shell-patterns`).
2. Exact name match against `SkillDescriptor.name` (case-insensitive).

Ambiguous matches (multiple skills matching a reference) produce a
`SkillResolutionFailed` event with the `Ambiguous` error variant and the list
of matching IDs. The resolver does NOT guess — it fails visibly.

#### Renderer — output formats

**Inventory section** (appended to system prompt via `extra_sections`):

```
## Available Skills

- `/task-workflow` — How to use task_create/task_update/task_list for structured work tracking.
- `/shell-patterns` — Background job patterns with shell and job management tools.
- `/mcp-server-setup` — How to configure MCP servers in .rkat/mcp.toml.
```

**Per-turn injection block** (prepended to user message):

```
<skill name="shell-patterns">
# Shell Patterns

When running background jobs...
</skill>
```

Injection content is truncated to `max_injection_bytes` (default 32 KiB).
Truncation emits a warning log but does not error.

### 3.3 Factory integration

In `build_agent()`:
1. If `skills` feature enabled: collect embedded skills via `inventory`,
   create filesystem sources, merge with any SDK-provided sources.
2. Filter skills by available capabilities.
3. Wire `DefaultSkillEngine` into `AgentBuilder`.
4. Extend prompt assembly (from Phase 0) to include skill inventory section.

Prompt section order: base → skill inventory → tool instructions.

Skills registers as a capability:
```rust
inventory::submit! {
    CapabilityRegistration {
        id: CapabilityId::Skills,
        description: "Skill loading, resolution, and injection",
        scope: CapabilityScope::Universal,
        requires_feature: Some("skills"),
        prerequisites: &[],
    }
}
```

### 3.4 Embedded skills in component crates

Each component crate stores skills in `<crate>/skills/<name>/SKILL.md` and
registers via `inventory::submit!` in a `skills.rs` module using `include_str!`.

Skills declare `requires_capabilities` so capability-gated skills are
automatically excluded from builds that lack the required features.

Size budget: per-skill 16 KiB, per-crate 64 KiB, CI-enforced. The 16 KiB
per-skill limit accommodates complex topics like hook authoring that cover
multiple interacting concepts. The per-crate budget is the real constraint
for binary size. Sidecar loading available for reference material that exceeds
the per-skill limit.

Precedence for duplicate IDs: project > user > builtin (with debug-level
shadow logging).

Filesystem scope paths are aligned with the existing AGENTS.md convention:
- Project: `.rkat/skills/` (consistent with `.rkat/AGENTS.md`, `.rkat/mcp.toml`)
- User: `~/.rkat/skills/` (consistent with `~/.rkat/AGENTS.md`, `~/.rkat/mcp.toml`)
- NOT `~/.config/rkat/` — that would diverge from the established `.rkat` convention.

#### Embedded skill inventory (verified against crate capabilities)

| Crate | Skill | Requires | What it teaches |
|-------|-------|----------|----------------|
| meerkat-tools | `task-workflow` | `Builtins` | How to use `task_create`/`task_update`/`task_list` for structured work tracking — status transitions, priority management, blocking relationships, label conventions |
| meerkat-tools | `shell-patterns` | `Builtins`, `Shell` | Background job patterns (`shell` with `background=true` → `shell_job_status` polling → `shell_job_cancel`), output truncation awareness, working directory conventions, timeout management |
| meerkat-tools | `sub-agent-orchestration` | `SubAgents` | When to `agent_spawn` vs `agent_fork`, model selection for sub-agents, result aggregation, concurrency limit awareness, parent-child context patterns |
| meerkat-comms | `multi-agent-comms` | `Comms` | Setting up host mode, peer trust configuration, `comms_send` vs `comms_request`/`comms_response` patterns, inbox message handling, transport selection (UDS vs TCP vs inproc) |
| meerkat-mcp | `mcp-server-setup` | — | How to configure MCP servers in `.rkat/mcp.toml`, stdio vs HTTP transport, environment variable expansion, project vs user scope security implications, troubleshooting connection failures. Owned by `meerkat-mcp` (not CLI) because the skill teaches the MCP ecosystem — config format, transport semantics, connection lifecycle — even though CLI commands are mentioned. |
| meerkat-hooks | `hook-authoring` | `Hooks` | Writing hooks for the 7 hook points, choosing execution mode (foreground/background/observe), deny/allow/rewrite decision semantics, patch format for `PreToolExecution` arg rewriting, failure policy (fail-open vs fail-closed) |
| meerkat-memory | `memory-retrieval` | `MemoryStore` | How semantic memory works with compaction, the `MemoryStore` trait (`index`/`search`), understanding similarity scores, session-scoped vs cross-session memory |
| meerkat-session | `session-management` | `SessionStore` | Session persistence, resume patterns, event store replay, compaction threshold tuning, `.rkat/sessions/` file structure (derived output, not canonical) |

Crates that do NOT have embedded skills:
- `meerkat-core` — contract-only, no user-facing behavior to teach.
- `meerkat-client` — internal provider plumbing (Anthropic/OpenAI/Gemini adapters).
- `meerkat-store` — storage backend internals (JsonlStore, RedbSessionStore, MemoryStore).
- `meerkat-contracts` — wire types and schema emission only.
- Surface/facade crates (`meerkat`, `meerkat-cli`, `meerkat-rest`, `meerkat-rpc`, `meerkat-mcp-server`) — wire and compose, don't own procedural content.

### 3.5 Wire memory indexing of compaction discards

`CompactionOutcome.discarded` exists. `MemoryStore.index()` exists.
`HnswMemoryStore` works. The only missing piece is the wiring in the agent
loop that feeds discarded messages into the memory store after compaction.

In `meerkat-core/src/agent/compact.rs`, after `run_compaction()` returns a
`CompactionOutcome`, the agent loop indexes each discarded message:

```rust
// After compaction succeeds, index discarded messages into memory.
if let Some(ref memory_store) = self.memory_store {
    for message in &outcome.discarded {
        let content = message.as_indexable_text();
        if !content.is_empty() {
            let metadata = MemoryMetadata {
                session_id: self.session.id().clone(),
                turn: Some(outcome.current_turn),
                indexed_at: SystemTime::now(),
            };
            if let Err(e) = memory_store.index(&content, metadata).await {
                tracing::warn!("failed to index compaction discard into memory: {e}");
            }
        }
    }
}
```

This requires:
1. `AgentBuilder::memory_store()` setter (new, mirrors `skill_engine()` pattern).
2. Factory wires `HnswMemoryStore` when `memory-store` feature is enabled.
3. `Message` gets an `as_indexable_text() -> String` method that extracts
   user and assistant text content suitable for indexing.

The `memory-retrieval` skill then teaches a fully working feature, not
a half-built one.

### 3.6 Session metadata extension

`SessionMetadata` gains `active_skills: Option<Vec<SkillId>>` for
deterministic resume. Missing skills on resume emit warning events but
don't fail.

### Phase 3 acceptance

- Skills is a registered capability (`CapabilityId::Skills`).
- `meerkat-skills` crate exists with sources, parser, resolver, renderer, engine.
- Skill registration uses `inventory` — same mechanism as capabilities.
- Factory wires skill engine when `skills` feature enabled.
- Prompt assembly includes skill inventory section.
- All 8 embedded skills listed above are implemented in their respective crates.
- Skills with `requires_capabilities` filtered per build profile.
- Resume preserves skill set with degraded-mode handling.
- Memory indexing of compaction discards is wired and functional.
- `McpRouterAdapter` lives in `meerkat-mcp`, used by all surfaces.

---

## Phase 4 — Python + TypeScript SDKs

Generated from contract artifacts. Communicate with Rust runtime via
`rkat rpc` (JSON-RPC stdio). No FFI. Capability-aware, including skills.

### 4.1 Codegen pipeline (`tools/sdk-codegen/`)

Reads `artifacts/schemas/*.json`, produces typed client code for both languages.

Inputs: `wire-types.json`, `params.json`, `events.json`, `errors.json`,
`capabilities.json`, `rpc-methods.json`.

Outputs:
- `sdks/python/meerkat/generated/`
- `sdks/typescript/src/generated/`

Generated code committed and CI-verified (golden diff test).

### 4.2 Python SDK (`sdks/python/`)

```
sdks/python/
  meerkat/
    __init__.py
    client.py          — MeerkatClient: async + sync, process lifecycle
    streaming.py       — Async iterator over WireEvent
    capabilities.py    — CapabilityChecker, method gating
    skills.py          — Skill listing, invocation helpers
    types.py           — Re-exports from generated/
    errors.py          — MeerkatError with capability hints
    generated/         — Pydantic models, stubs, error types
  tests/
  pyproject.toml
```

- Async-first with sync wrapper.
- Spawns `rkat rpc` subprocess, JSONL stdio.
- `initialize` handshake checks `ContractVersion` compatibility.
- `capabilities/get` on connect → gates methods by availability.
- Unavailable methods raise `CapabilityUnavailableError` with `CapabilityHint`.
- Skills methods gated by `CapabilityId::Skills` presence.

### 4.3 TypeScript SDK (`sdks/typescript/`)

Same architecture as Python. ESM + CJS dual package.

### 4.4 Conformance tests

- Schema round-trip (Rust → Python/TS → Rust).
- Golden codegen diff.
- Capability gating against minimal/standard/full builds.
- E2E lifecycle: create → turn → stream → interrupt → archive.
- Version compatibility: same major/different minor, major mismatch.
- Cross-SDK parity: same scenario in Python and TS, normalized output match.
- Skills: SDK correctly lists skills, invokes by reference, handles
  `SkillNotFound` and `CapabilityUnavailable` errors.

### Phase 4 acceptance

- Python and TypeScript SDKs exist.
- Both communicate via `rkat rpc` stdio.
- Method signatures derived from same schema artifacts.
- Capability gating works including skills.
- E2E tests pass against minimal, standard, full profiles.
- Publishable on PyPI / npm.

---

## Phase 5 — SDK Builder

The destination. Everything before exists to make this work.

### 5.1 Builder tool (`tools/sdk-builder/`)

```toml
# meerkat-sdk.toml
[profile]
name = "my-agent-runtime"

[features]
include = ["comms", "sub-agents", "session-store", "skills"]
exclude = ["memory-store"]

[sdk]
languages = ["python", "typescript"]

[sdk.python]
package_name = "my-agent"

[sdk.typescript]
package_name = "@myorg/agent"
```

### 5.2 Builder pipeline

1. **Resolve features** → `--features` flag set. Validate prerequisites via
   `CapabilityId` exhaustive match.
2. **Build runtime** → custom `rkat` binary.
3. **Emit schemas** → `capabilities.json` reflects exactly what was compiled.
   Skills capability present/absent. Skill registrations present/absent.
4. **Run codegen** → omit methods for absent capabilities. No `CommsParams`
   if no comms. No skill methods if no skills. No dead surface.
5. **Package SDKs** → wheel / npm tarball with embedded contract version +
   capability manifest.
6. **Emit bundle manifest** → source commit, features, contract version,
   hashes, timestamp.

### 5.3 Toolchain requirement

The builder requires a Rust toolchain — it runs `cargo build` from source.
This is a developer tool for teams that build custom runtimes, not an end-user
tool. End users who want a pre-built binary pick from the preset profiles
shipped as CI artifacts.

### 5.4 Profile presets

CI builds the three preset profiles and publishes runtime binaries + SDK
packages as release artifacts. Users without a Rust toolchain use these
directly.

- `profiles/minimal.toml` — core loop only. No builtins, no comms, no skills.
- `profiles/standard.toml` — builtins, session store, shell, skills.
- `profiles/full.toml` — everything. Comms, sub-agents, memory, skills, compaction.

CI builds all three + runs SDK conformance against each.

### 5.5 Testing

- Manifest resolution with prerequisite validation.
- Each preset produces working binary + SDK.
- SDK surface verification: minimal < standard < full method count.
- Capability consistency: runtime `capabilities/get` matches SDK manifest.
- Skill surface: full profile SDK has skill methods, minimal doesn't.
- Reproducibility: same manifest + commit = same output.

### Phase 5 acceptance

- Builder produces working runtime + SDK bundles.
- Generated SDKs expose only compiled capabilities.
- Skills included/excluded per profile.
- Preset profiles pass conformance.
- Reproducible, version-locked bundles.

---

## Cross-Cutting: Rust Design Conventions

Applied throughout all phases:

1. **Typed enums over strings.** `CapabilityId`, `ErrorCode`, `ErrorCategory`,
   `Protocol`, `SkillScope`, `SkillResolutionMode` — all enums with `strum`.
2. **Feature-gated `JsonSchema`.** `#[cfg_attr(feature = "schema", derive(...))]`
   on all wire types.
3. **Distributed registration via `inventory`.** Capabilities and skills
   self-register from their providing crates.
4. **`Cow<'static, str>`** for error messages and resolution hints.
5. **`Ord + Display + FromStr + Copy`** on `ContractVersion`.
6. **Explicit delegation over `#[serde(flatten)]`** for request composition.
7. **Newtype identifiers** — `SkillId`, not bare `String`.
8. **`Result` over silent failures** — capability and skill resolution return
   typed status values.

## Architecture Diagram

```
  Component crates                meerkat-contracts         meerkat-skills
  ┌────────────────┐             ┌──────────────────┐     ┌──────────────┐
  │ meerkat-tools  │──submit!──► │  Wire types      │     │  Sources     │
  │ meerkat-comms  │  caps +     │  Error envelope   │     │  Parser      │
  │ meerkat-mcp    │  skills     │  Capability model │     │  Resolver    │
  │ meerkat-memory │             │  Contract version │     │  Renderer    │
  │ meerkat-hooks  │             │  Schema emission  │     │  Engine      │
  └────────────────┘             └────────┬─────────┘     └──────┬───────┘
                                          │                      │
                           ┌──────────────┼──────────────┐       │
                           │              │              │       │
                           ▼              ▼              ▼       │
                 ┌─────────────┐  ┌────────────┐  ┌──────────┐  │
                 │ meerkat-rpc │  │meerkat-rest│  │meerkat-  │  │
                 │             │  │            │  │mcp-server│  │
                 └──────┬──────┘  └─────┬──────┘  └────┬─────┘  │
                        │               │              │        │
                        └───────────────┼──────────────┘        │
                                        │                       │
                                 ┌──────▼──────┐                │
                                 │   meerkat   │◄───────────────┘
                                 │   (facade)  │
                                 │             │
                                 │  Factory    │
                                 │  Prompt asm │
                                 │  Wiring     │
                                 └──────┬──────┘
                                        │
                                 ┌──────▼──────┐
                                 │  artifacts/ │
                                 │  schemas/   │ ◄── emit-schemas
                                 └──────┬──────┘
                                        │
                                 ┌──────▼──────┐
                                 │ sdk-codegen │
                                 └──────┬──────┘
                                  ┌─────┴─────┐
                                  ▼           ▼
                           ┌──────────┐ ┌──────────┐
                           │ Python   │ │TypeScript│
                           │ SDK      │ │ SDK      │
                           └──────────┘ └──────────┘
                                  ▲           ▲
                                  └─────┬─────┘
                                 ┌──────▼──────┐
                                 │ SDK Builder │
                                 └─────────────┘
```

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Schema drift between Rust types and JSON schemas | Medium | High | CI determinism check; round-trip conformance tests |
| `rkat rpc` protocol changes break SDKs | Medium | High | Contract version handshake; semver discipline |
| Codegen produces incorrect Python/TS types | Medium | Medium | Round-trip tests; golden diff tests |
| `inventory` registration order non-determinism | Low | Medium | Sort by enum variant ordinal before emission |
| Embedded skills bloat binary size | Medium | Medium | Per-skill/per-crate budgets; CI enforcement; sidecar fallback |
| Skill implicit resolution false positives | Medium | Medium | Explicit mode default; ambiguity detection |
| Skills interact poorly with context compaction | Medium | Medium | Skill injection is per-turn ephemeral |
| Forgetting `CapabilityId` variant for new feature | Low | Low | `inventory::submit!` with non-existent variant = compile error |
| Fragment delegation boilerplate | High | Low | Manual initially; `#[derive(ComposeContract)]` if needed |
| `schemars` diverges from serde | Low | High | Pin version; validate schema against actual serde output |
| Builder feature resolution subtle interactions | Medium | Medium | Prerequisite graph via exhaustive match; presets in CI |

## Sequencing Summary

```
Phase 0: Prompt assembly unification ──────────────────────► (standalone PR)
Phase 1: Contracts foundation ─────────────────────────────► (new crate)
Phase 2: Protocol alignment ───────────────────────────────► (surface migration)
Phase 3: Skills system ────────────────────────────────────► (new crate + content)
Phase 4: Python + TypeScript SDKs ─────────────────────────► (codegen + SDKs)
Phase 5: SDK Builder ──────────────────────────────────────► (the destination)
```

Each phase builds on the previous. No phase requires rework of an earlier phase.
Skills (Phase 3) composes with contracts (Phases 1-2) from day one. SDKs
(Phase 4) include skill support naturally. Builder (Phase 5) handles skills
as a capability like any other.
