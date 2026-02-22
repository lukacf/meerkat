# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Meerkat (`rkat`) is a minimal, high-performance agent harness for LLM-powered applications written in Rust. It provides the core execution loop for agents without opinions about prompts, tools, or output formatting.

**Naming convention:**
- Project/branding: **Meerkat**
- CLI binary: **rkat**
- Crate names: `meerkat`, `meerkat-core`, `meerkat-client`, etc.
- Config directory: `.rkat/`
- Environment variables: API keys only (RKAT_* secrets and provider-native keys)

## Build and Test Commands

```bash
# Build everything
cargo build --workspace

# Run fast tests (unit + integration-fast; skips doctests)
cargo test --workspace --lib --bins --tests

# Run all tests including doc-tests (SLOW due to doc-test compilation)
cargo test --workspace

# Run integration-real tests (ignored by default; spawns processes / requires binaries)
cargo test --workspace integration_real -- --ignored --test-threads=1

# Run E2E tests (ignored by default; live APIs / full-system resources)
cargo test --workspace e2e_ -- --ignored --test-threads=1

# Cargo aliases (defined in .cargo/config.toml)
cargo rct       # Fast tests (unit + integration-fast)
cargo unit      # Unit tests only
cargo int       # Integration-fast tests only
cargo int-real  # Integration-real tests (ignored)
cargo e2e       # E2E tests (ignored)

# Run the CLI
cargo run -p meerkat-cli -- run "prompt"
./target/debug/rkat run "prompt"

# Run a specific example
ANTHROPIC_API_KEY=... cargo run --example simple
```

## Architecture

```
meerkat-core      → Agent loop, types, budget, retry, state machine (no I/O deps)
                     Also: SessionService trait, Compactor trait, MemoryStore trait, SessionError
meerkat-client    → LLM providers (Anthropic, OpenAI, Gemini) implementing AgentLlmClient
meerkat-store     → Session persistence (JsonlStore, MemoryStore, RedbSessionStore) implementing SessionStore
meerkat-tools     → Tool registry and validation implementing AgentToolDispatcher
meerkat-session   → Session service orchestration (EphemeralSessionService, DefaultCompactor)
                     Features: session-store (PersistentSessionService, RedbEventStore),
                               session-compaction (DefaultCompactor)
meerkat-memory    → Semantic memory (HnswMemoryStore via hnsw_rs + redb, SimpleMemoryStore for tests)
meerkat-mcp       → MCP protocol client, McpRouter for tool routing
meerkat-mcp-server → Expose Meerkat as MCP tools (meerkat_run, meerkat_resume, meerkat_config, meerkat_capabilities)
meerkat-rpc       → JSON-RPC stdio server (stateful SessionRuntime, IDE/desktop integration)
meerkat-rest      → Optional REST API server
meerkat-comms     → Inter-agent communication (Ed25519-signed messaging, transports, trust model)
meerkat-contracts → Wire types, capability registry, error codes (canonical over all surfaces)
meerkat-skills    → Skill loading, resolution, rendering (filesystem, git, HTTP, embedded sources)
meerkat-hooks     → Hook infrastructure (in-process, command, HTTP runtimes)
meerkat-mob       → Multi-agent mob orchestration (spawn, provision, finalize)
meerkat-mob-mcp   → Expose mob tools as MCP interface (MobMcpState, MobMcpDispatcher)
meerkat-cli       → CLI binary (produces `rkat`)
meerkat           → Facade crate, re-exports, AgentFactory, SDK helpers
```

**Key traits** (all in meerkat-core):
- `AgentLlmClient` - LLM provider abstraction
- `AgentToolDispatcher` - Tool routing abstraction
- `AgentSessionStore` - Session persistence abstraction
- `SessionService` - Canonical session lifecycle (create/turn/interrupt/read/list/archive)
- `Compactor` - Context compaction strategy
- `MemoryStore` - Semantic memory indexing

**Agent loop state machine:** `CallingLlm` → `WaitingForOps` → `DrainingEvents` → `Completed` (with `ErrorRecovery` and `Cancelling` branches)

**Crate ownership:** `meerkat-core` owns trait contracts. `meerkat-store` owns `SessionStore` implementations. `meerkat-session` owns session orchestration (`EphemeralSessionService`, `PersistentSessionService`) and `EventStore`. `meerkat-memory` owns `HnswMemoryStore`. The facade (`meerkat`) wires features, re-exports, and provides `FactoryAgentBuilder`/`FactoryAgent`/`build_ephemeral_service`.

**Agent construction:** All surfaces use `AgentFactory::build_agent()` for centralized prompt assembly, provider resolution, tool dispatcher setup, comms wiring, and hook resolution. Zero `AgentBuilder::new()` calls in surface crates.

**Session lifecycle:** All four surfaces (CLI, REST, MCP Server, JSON-RPC) route through `SessionService` for the full session lifecycle (create/turn/interrupt/read/list/archive). `FactoryAgentBuilder` bridges `AgentFactory` into the `SessionAgentBuilder` trait. Per-request build data is passed in-band via `CreateSessionRequest.build` / `SessionBuildOptions`.

**Capability matrix:** See `docs/reference/capability-matrix.mdx` for build profiles, error codes, and feature behavior. See `docs/reference/session-contracts.mdx` for concurrency, durability, and compaction semantics.

**`.rkat/sessions/` files** are derived projection output (materialized by `SessionProjector`), NOT canonical state. Deleting them and replaying from the event store produces identical content.

## MCP Server Management

```bash
# Add MCP server (stdio)
rkat mcp add <name> -- <command> [args...]

# Add MCP server (HTTP/SSE)
rkat mcp add <name> --url <url>

# List/remove servers
rkat mcp list
rkat mcp remove <name>
```

Config stored in `.rkat/mcp.toml` (project) or `~/.rkat/mcp.toml` (user).

## JSON-RPC Stdio Server

```bash
# Start the JSON-RPC stdio server (for IDE/desktop integration)
rkat-rpc
```

The RPC server speaks JSON-RPC 2.0 over newline-delimited JSON (JSONL) on stdin/stdout. Unlike REST/MCP, it keeps agents alive between turns via `SessionRuntime` -- enabling fast multi-turn conversations, mid-turn cancellation, and event streaming without agent reconstruction overhead.

**Methods:**

| Method | Purpose |
|--------|---------|
| `initialize` | Handshake, returns server capabilities |
| `session/create` | Create session + run first turn |
| `session/list` | List active sessions |
| `session/read` | Get session state |
| `session/archive` | Remove session |
| `turn/start` | Start a new turn on existing session |
| `turn/interrupt` | Cancel in-flight turn |
| `comms/send` | Push external event into session (comms feature) |
| `comms/peers` | List discoverable peers (comms feature) |
| `comms/stream_open` | Open scoped comms event stream (comms feature) |
| `comms/stream_close` | Close scoped comms stream (comms feature) |
| `capabilities/get` | List runtime capabilities |
| `config/get` | Read config |
| `config/set` | Replace config |
| `config/patch` | Merge-patch config |

**Notifications** (server -> client): `session/event` with `AgentEvent` payload, emitted during turns. `comms/stream_event` for scoped comms event streams (comms feature).

**Architecture:** Each session gets a dedicated tokio task that exclusively owns the `Agent` (no mutex needed for `cancel(&mut self)`). The `SessionRuntime` dispatches commands via channels. `AgentFactory.build_agent()` consolidates the agent construction pipeline shared across all surfaces.

## Key Files

- `meerkat-core/src/agent.rs` - Main agent execution loop
- `meerkat-core/src/agent/compact.rs` - Compaction flow (wired into agent loop)
- `meerkat-core/src/state.rs` - LoopState state machine
- `meerkat-core/src/types.rs` - Core types (Message, Session, ToolCall, etc.)
- `meerkat-core/src/service/mod.rs` - SessionService trait, SessionError
- `meerkat-core/src/compact.rs` - Compactor trait, CompactionConfig
- `meerkat-core/src/memory.rs` - MemoryStore trait
- `meerkat-client/src/anthropic.rs` - Anthropic streaming implementation
- `meerkat-session/src/ephemeral.rs` - EphemeralSessionService (in-memory session lifecycle)
- `meerkat-session/src/compactor.rs` - DefaultCompactor implementation
- `meerkat-session/src/event_store.rs` - EventStore trait
- `meerkat-session/src/redb_events.rs` - RedbEventStore implementation
- `meerkat-session/src/projector.rs` - SessionProjector (materializes .rkat/ files)
- `meerkat-store/src/redb_store.rs` - RedbSessionStore implementation
- `meerkat-memory/src/simple.rs` - SimpleMemoryStore implementation
- `meerkat-mcp/src/router.rs` - MCP tool routing
- `meerkat-cli/src/main.rs` - CLI entry point
- `meerkat/src/factory.rs` - AgentFactory, DynAgent, AgentBuildConfig (consolidated agent construction)
- `meerkat/src/service_factory.rs` - FactoryAgentBuilder, FactoryAgent, build_ephemeral_service
- `meerkat-rpc/src/session_runtime.rs` - SessionRuntime (stateful agent manager)
- `meerkat-rpc/src/router.rs` - JSON-RPC method dispatch
- `meerkat-rpc/src/server.rs` - RPC server main loop

## CI/CD and Versioning

### Running CI

```bash
make ci          # Full CI: fmt, lint, feature matrix, tests, audit, version parity
make ci-smoke    # Faster CI: skips full feature matrix expansion
make test        # Fast tests only (unit + integration-fast)
make lint        # Clippy with all features
make fmt         # Auto-fix formatting
make audit       # Security audit via cargo-deny
```

**`make ci` runs** (in order): `fmt-check` → `legacy-surface-gate` → `verify-version-parity` → `lint` → `lint-feature-matrix` → `test-all` → `test-minimal` → `test-feature-matrix` → `test-surface-modularity` → `audit`

### GitHub Workflows

**CI** (`.github/workflows/ci.yml`) — runs on push to main, PRs, and manual dispatch:
- Single `quality` job: runs `make ci` (full pipeline)

**Release** (`.github/workflows/release.yml`) — runs on `v*` tag push or manual dispatch:

| Job | Trigger | What it does |
|-----|---------|-------------|
| `release_validate` | Always | `make release-preflight-smoke` + tag-version check (tags only) |
| `build_binaries` | Always | Matrix build for 4 targets, packages 4 binaries each (`rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`) |
| `publish_github_release` | Tags only | Downloads artifacts, generates `checksums.sha256` + `index.json`, publishes GitHub Release |
| `publish_registries` | Tags or manual `publish_release_packages=true` | Publishes 15 Rust crates → crates.io, Python SDK → PyPI, TypeScript SDK → npm |

**Build matrix:**

| Platform | Target |
|----------|--------|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` |
| macOS ARM64 | `aarch64-apple-darwin` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

**Manual dispatch options:**
```bash
# Build-only validation (no publish)
gh workflow run release.yml --ref main

# Dry-run publish (validate all registries without uploading)
gh workflow run release.yml --ref main -f publish_release_packages=true -f registry_dry_run=true

# Recovery: re-publish after registry outage during a tag-triggered release
gh workflow run release.yml --ref v0.3.4 -f publish_release_packages=true
```

### Pre-commit Hooks

Installed via `make install-hooks`. Two stages:

**On commit** (`pre-commit`):
- Runs tests only on changed crates (`scripts/test-changed-crates.sh`)

**On push** (`pre-push`):
- Secret detection (gitleaks)
- Trailing whitespace, YAML/TOML validation, merge conflict check, large file check
- `make test` (fast tests: unit + integration-fast)
- `cargo clippy` (workspace, warnings as errors)
- `cargo doc` (workspace docs build)
- `make audit` (cargo-deny security audit)

### Version Parity Contract

Five files must agree on the same version:

| File | Field |
|------|-------|
| `Cargo.toml` (workspace root) | `workspace.package.version` — **source of truth** |
| `meerkat-contracts/src/version.rs` | `ContractVersion::CURRENT` |
| `sdks/python/pyproject.toml` | `version` |
| `sdks/typescript/package.json` | `version` |
| `artifacts/schemas/version.json` | `contract_version` |

Additionally, all 17 internal crate dependencies in `Cargo.toml` must match the workspace version.

**`make verify-version-parity`** runs in CI and fails on any drift. After changing versions or wire types:

```bash
make regen-schemas           # Re-emit schemas + regenerate SDK types
make verify-version-parity   # Confirm everything is in sync
```

### Schema Generation and SDK Codegen

When wire types in `meerkat-contracts` change:

```bash
make regen-schemas
# Runs:
#   cargo run -p meerkat-contracts --features schema --bin emit-schemas
#   python3 tools/sdk-codegen/generate.py
```

This updates:
- `artifacts/schemas/` — JSON schema artifacts
- `sdks/python/meerkat/generated/` — Python generated types
- `sdks/typescript/src/generated/` — TypeScript generated types

**`make verify-schema-freshness`** detects stale committed schemas by comparing git HEAD against freshly emitted output.

### Releasing

```bash
make release-preflight       # Full CI + schema freshness + changelog check
cargo release patch          # Bump, tag, push (uses cargo-release)
```

**What `cargo release patch` does:**

1. Bumps `workspace.package.version` in `Cargo.toml`
2. Fires `scripts/release-hook.sh` (pre-release hook, sentinel-guarded to run once):
   - `scripts/bump-sdk-versions.sh` — updates Python and TypeScript SDK versions
   - `emit-schemas` + `generate.py` — regenerates schema artifacts and SDK types
   - `verify-version-parity.sh` — sanity check before commit
   - Stages all modified files (SDK configs, generated types, schema artifacts)
3. Creates release commit (`chore: release v{version}`)
4. Tags as `v{version}`
5. Pushes commit + tag to remote → triggers release workflow

**Cargo.toml release config** (`workspace.metadata.release`):
- `tag-name = "v{{version}}"`, `push = true`, `publish = false` (registry publish handled by GitHub Actions)

### Dry-run Publishing

```bash
make publish-dry-run              # Parallel dry-run for all 15 Rust crates
make publish-dry-run-python       # Build + twine check (no upload)
make publish-dry-run-typescript   # npm publish --dry-run
make release-dry-run              # Full preflight + all registry dry-runs
make release-dry-run-smoke        # Smoke preflight + all registry dry-runs
```

### Registry Secrets

Required GitHub Actions secrets for full release:
- `CARGO_REGISTRY_TOKEN` — crates.io API token
- `PYPI_API_TOKEN` — PyPI API token
- `NPM_TOKEN` — npm access token

### Crate Publish Order

The 15 crates are published in dependency order:
`meerkat-core` → `meerkat-contracts` → `meerkat-client` → `meerkat-store` → `meerkat-tools` → `meerkat-session` → `meerkat-memory` → `meerkat-mcp` → `meerkat-mcp-server` → `meerkat-hooks` → `meerkat-skills` → `meerkat-comms` → `meerkat-rpc` → `meerkat-rest` → `meerkat`

### Key Rules for AI Agents

- **Never bump `workspace.package.version` without also running `scripts/bump-sdk-versions.sh`** — the CI gate will catch drift
- **Never change types in `meerkat-contracts` without running `make regen-schemas`** — schema artifacts and SDK types will be stale
- **Always run `make test` (or `cargo rct`) before committing** — pre-commit hooks enforce this
- **`ContractVersion::CURRENT` must equal `workspace.package.version`** — they are lock-stepped
- **Use `cargo release patch` for releases** — never manually bump versions or create tags; the release hook handles SDK sync, schema regen, and parity verification automatically

## Testing with Multiple Providers

When running tests or demos that involve multiple LLM providers/models, use these model names:

| Provider | Model Name |
|----------|------------|
| OpenAI | `gpt-5.2` |
| Gemini | `gemini-3-flash-preview` or `gemini-3-pro-preview` |
| Anthropic | `claude-opus-4-6` or `claude-sonnet-4-5` |

Do NOT use older model names like `gpt-4o-mini`, `gemini-2.0-flash`, or `claude-3-7-sonnet-20250219`.

## Design Philosophy

See `docs/reference/design-philosophy.mdx` for the full treatment with code examples.

### Architectural Principles

- **Infrastructure, not application** — the agent loop is a composable primitive with no opinions about prompts, tools, or output
- **Trait contracts own the architecture** — `meerkat-core` defines the core trait contracts (`AgentLlmClient`, `AgentToolDispatcher`, `AgentSessionStore`, `SessionService`, `Compactor`, `MemoryStore`, `HookEngine`, `SkillEngine`/`SkillSource`) with zero I/O dependencies; implementations live in satellite crates
- **Surfaces are interchangeable skins** — CLI, REST, RPC, MCP Server all route through `SessionService` → `AgentFactory::build_agent()`; no surface constructs agents directly
- **Composition over configuration** — optional components (`CommsRuntime`, `HookEngine`, `Compactor`, `MemoryStore`) are `Option<Arc<dyn Trait>>`, not feature-flagged defaults
- **Sessions are first-class, persistence is optional** — `EphemeralSessionService` (always available) and `PersistentSessionService` (event-sourced) share the same `SessionService` trait
- **Errors separate mechanism from policy** — typed three-tier errors (`ToolError` → `AgentError` → `SessionError`) with stable `error_code()` for wire formats; the loop retries, callers decide to resume or abort
- **Wire types ≠ domain types** — `meerkat-contracts` owns wire format and feeds SDK codegen; domain types in `meerkat-core` are richer and version-locked
- **Configuration is layered and declarative** — `Config::default()` → file → env (keys only) → per-request `SessionBuildOptions`; no cascading merges, no global mutable state
- **Testing is a design constraint** — core has no I/O so unit tests need no mocks; three named tiers (`cargo rct`, `cargo int-real`, `cargo e2e`)

### Rust Implementation Principles

- **Ownership topology** — shared immutable infrastructure in `Arc`, exclusively-owned mutable state (session, budget); `Agent::run(&mut self)` needs no mutex
- **Copy-on-write sessions** — `Arc<Vec<Message>>` with `Arc::make_mut` on mutation; `Session::fork()` is O(1)
- **Zero-allocation iteration** — `ToolCallView<'a>` is `Copy` and borrowed; `ToolCallIter` filters a slice iterator; no `Vec<ToolCall>` materialized
- **Deferred parsing** — tool args are `Box<RawValue>` from provider to dispatcher; parsed at most once, only if the tool executes
- **Typed enums over `Value`** — `ProviderMeta`, `Message`, `AssistantBlock` are typed with `#[non_exhaustive]`; compiler enforces exhaustive matching
- **Newtype discipline** — `BlockKey(usize)`, `OperationId(Uuid)`, `SessionId`, `SourceUuid`, `SkillName` prevent index/ID confusion at compile time
- **Serde as a design tool** — internally/adjacently/externally tagged enums chosen per data shape; custom deserializers for edge cases; `skip_serializing_if` for minimal payloads
- **Streaming block assembly** — append-only `Vec<BlockSlot>` with `Pending` → `Finalized` transitions; `IndexMap` for deterministic order; tool ID is map key only
- **Generic type erasure at boundaries** — `Agent<C, T, S>` monomorphized in tests, boxed to `DynAgent` at surface boundaries via `?Sized` bounds
- **Async without interior mutability** — dedicated tokio task per session owns the `Agent` exclusively; channels for commands, notifications for events
- **Feature gating at the type level** — `#[cfg(feature)]` gates types, not logic; facade re-exports only what features enable; fallbacks always available
- **Trait composition with graceful degradation** — optional methods default to `Err(Unsupported(...))`; required methods define the minimal contract
- **Error propagation** — `thiserror` enums with `From` impls for `?` chaining; each tier captures minimal context; stable `error_code()` for SDKs

## Rust Design Guidelines

This project follows strict Rust idioms. Code review will reject "JavaScript wearing a struct costume."

### Type Safety

1. **Typed enums over `serde_json::Value`**: If you know the possible shapes of data, define a typed enum. Parse at the boundary, fail fast. Don't ferry `Value` through the system hoping someone else validates it.

   ```rust
   // BAD: runtime "is this an object?" checks
   meta: Option<serde_json::Value>

   // GOOD: compiler-enforced variants
   #[serde(tag = "provider")]
   pub enum ProviderMeta {
       Anthropic { signature: String },
       Gemini { thought_signature: String },
       OpenAi { encrypted_content: String },
   }
   ```

2. **Newtype indices**: If using indices into collections, wrap them in a newtype to prevent mixing up different index spaces.

   ```rust
   struct BlockKey(usize);  // Can't accidentally use a ToolBufferIdx here
   ```

3. **`Box<RawValue>` for pass-through JSON**: If JSON is parsed by another layer (e.g., tool dispatcher), use `RawValue` to avoid parsing twice.

### Error Handling

4. **`Result` over silent failures**: Separate mechanism from policy. Return errors, let the caller decide to skip/count/abort.

   ```rust
   // BAD: swallows the signal
   fn on_delta(&mut self, id: &str) {
       if let Some(buf) = self.buffers.get_mut(id) { ... }
   }

   // GOOD: caller decides policy
   fn on_delta(&mut self, id: &str) -> Result<(), StreamError> {
       let buf = self.buffers.get_mut(id)
           .ok_or_else(|| StreamError::OrphanedDelta(id.into()))?;
       ...
   }
   ```

5. **No `.unwrap()` or `.expect()` in library code**: Use `?` propagation or explicit `match`/`if let` with error handling.

### Allocation Discipline

6. **Zero-allocation iterators**: Return `impl Iterator<Item = View<'_>>` instead of `Vec<Owned>` when callers just iterate.

   ```rust
   // BAD: allocates Vec for every call
   pub fn tool_calls(&self) -> Vec<ToolCall> { ... }

   // GOOD: lazy iterator, borrows from self
   pub fn tool_calls(&self) -> impl Iterator<Item = ToolCallView<'_>> { ... }
   ```

7. **`impl Display` over collect+join**: For concatenating strings, implement `Display` to avoid intermediate allocations.

8. **`Slab` for stable keys**: If you need stable indices that survive mutations, use `slab` crate instead of `Vec` + `usize`.

### Data Modeling

9. **Don't duplicate map keys**: If something is a map key, don't store it in the value too.

   ```rust
   // BAD: id stored twice
   tool_buffers: HashMap<String, ToolBuffer>  // where ToolBuffer has `id: String`

   // GOOD: id is only the key
   tool_buffers: IndexMap<String, ToolBuffer>  // ToolBuffer has no id field
   ```

10. **Separate concerns in structs**: Billing metadata (`Usage`) doesn't belong in domain models (`AssistantMessage`). Return them separately.

11. **`IndexMap` for deterministic ordering**: Use `IndexMap` instead of `HashMap` when iteration order matters (e.g., tool calls must appear in emission order).
