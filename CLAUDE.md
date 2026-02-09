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
meerkat-mcp-client → MCP protocol client, McpRouter for tool routing
meerkat-mcp-server → Expose Meerkat as MCP tools (meerkat_run, meerkat_resume)
meerkat-rpc       → JSON-RPC stdio server (stateful SessionRuntime, IDE/desktop integration)
meerkat-rest      → Optional REST API server
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

**Session lifecycle:** `SessionService` trait manages stateful session lifecycle (create/turn/interrupt/read/list/archive). Currently used by RPC (`SessionRuntime` wraps `EphemeralSessionService`). Stateless surfaces (CLI, REST, MCP) use `build_agent()` directly per-request — `SessionService` is optional for stateless patterns.

**Capability matrix:** See `docs/CAPABILITY_MATRIX.md` for build profiles, error codes, and feature behavior. See `docs/SESSION_CONTRACTS.md` for concurrency, durability, and compaction semantics.

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

Config stored in `.rkat/mcp.toml` (project) or `~/.config/rkat/mcp.toml` (user).

## JSON-RPC Stdio Server

```bash
# Start the JSON-RPC stdio server (for IDE/desktop integration)
rkat rpc
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
| `config/get` | Read config |
| `config/set` | Replace config |
| `config/patch` | Merge-patch config |

**Notifications** (server -> client): `session/event` with `AgentEvent` payload, emitted during turns.

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
- `meerkat-mcp-client/src/router.rs` - MCP tool routing
- `meerkat-cli/src/main.rs` - CLI entry point
- `meerkat/src/factory.rs` - AgentFactory, DynAgent, AgentBuildConfig (consolidated agent construction)
- `meerkat-rpc/src/session_runtime.rs` - SessionRuntime (stateful agent manager)
- `meerkat-rpc/src/router.rs` - JSON-RPC method dispatch
- `meerkat-rpc/src/server.rs` - RPC server main loop

## Testing with Multiple Providers

When running tests or demos that involve multiple LLM providers/models, use these model names:

| Provider | Model Name |
|----------|------------|
| OpenAI | `gpt-5.2` |
| Gemini | `gemini-3-flash-preview` or `gemini-3-pro-preview` |
| Anthropic | `claude-opus-4-6` or `claude-sonnet-4-5` |

Do NOT use older model names like `gpt-4o-mini`, `gemini-2.0-flash`, or `claude-3-7-sonnet-20250219`.

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
