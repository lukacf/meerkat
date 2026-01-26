# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Meerkat (`rkat`) is a minimal, high-performance agent harness for LLM-powered applications written in Rust. It provides the core execution loop for agents without opinions about prompts, tools, or output formatting.

**Naming convention:**
- Project/branding: **Meerkat**
- CLI binary: **rkat**
- Crate names: `meerkat`, `meerkat-core`, `meerkat-client`, etc.
- Config directory: `.rkat/`
- Environment variables: `RKAT_MODEL`, `RKAT_MAX_TOKENS`, etc.

## Build and Test Commands

```bash
# Build everything
cargo build --workspace

# Run all unit/integration tests (FAST - ~10s, skips slow doc-test compilation)
cargo test --workspace --lib --bins --tests

# Run all tests including doc-tests (SLOW - ~1min due to doc-test compilation)
cargo test --workspace

# Run E2E tests (tests marked #[ignore = "e2e: ..."], spawns real processes)
cargo test --workspace -- --ignored

# Cargo aliases (defined in .cargo/config.toml)
cargo rct    # Run all unit tests
cargo int    # Run integration tests
cargo e2e    # Run E2E tests

# Run the CLI
cargo run -p meerkat-cli -- run "prompt"
./target/debug/rkat run "prompt"

# Run a specific example
ANTHROPIC_API_KEY=... cargo run --example simple
```

## Architecture

```
meerkat-core      → Agent loop, types, budget, retry, state machine (no I/O deps)
meerkat-client    → LLM providers (Anthropic, OpenAI, Gemini) implementing AgentLlmClient
meerkat-store     → Session persistence (JsonlStore, MemoryStore) implementing AgentSessionStore
meerkat-tools     → Tool registry and validation implementing AgentToolDispatcher
meerkat-mcp-client → MCP protocol client, McpRouter for tool routing
meerkat-mcp-server → Expose Meerkat as MCP tools (meerkat_run, meerkat_resume)
meerkat-rest      → Optional REST API server
meerkat-cli       → CLI binary (produces `rkat`)
meerkat           → Facade crate, re-exports and SDK helpers
```

**Key traits** (all in meerkat-core):
- `AgentLlmClient` - LLM provider abstraction
- `AgentToolDispatcher` - Tool routing abstraction
- `AgentSessionStore` - Session persistence abstraction

**Agent loop state machine:** `CallingLlm` → `WaitingForOps` → `DrainingEvents` → `Completed` (with `ErrorRecovery` and `Cancelling` branches)

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

## Key Files

- `meerkat-core/src/agent.rs` - Main agent execution loop
- `meerkat-core/src/state.rs` - LoopState state machine
- `meerkat-core/src/types.rs` - Core types (Message, Session, ToolCall, etc.)
- `meerkat-client/src/anthropic.rs` - Anthropic streaming implementation
- `meerkat-mcp-client/src/router.rs` - MCP tool routing
- `meerkat-cli/src/main.rs` - CLI entry point

## Testing with Multiple Providers

When running tests or demos that involve multiple LLM providers/models, use these model names:

| Provider | Model Name |
|----------|------------|
| OpenAI | `gpt-5.2` |
| Gemini | `gemini-3-flash-preview` or `gemini-3-pro-preview` |
| Anthropic | `claude-opus-4-5` or `claude-sonnet-4-5` |

Do NOT use older model names like `gpt-4o-mini` or `gemini-2.0-flash`.
