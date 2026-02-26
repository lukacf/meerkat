---
name: meerkat-wasm
description: "Developer guide for the Meerkat WASM embedded runtime (meerkat-web-runtime). This skill should be used when working on wasm32 compilation, building/deploying WASM bundles, writing wasm_bindgen exports, debugging browser runtime issues, or making crates wasm32-compatible. Covers the override-first resource injection pattern, tokio_with_wasm aliasing, cfg-gating patterns, mobpack bootstrap, and common wasm32 gotchas."
---

# Meerkat WASM Embedded Runtime

The `meerkat-web-runtime` crate compiles the full meerkat agent stack to `wasm32-unknown-unknown`. It is an embedded deployment target for mobpacks — the JavaScript equivalent of the Rust SDK. Not a protocol server like RPC/REST.

## Crate Compatibility Pattern

To make a meerkat crate compile for wasm32, apply this pattern:

**Cargo.toml** — gate platform-specific deps:
```toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = { workspace = true }
redb = { workspace = true }

[target.'cfg(target_arch = "wasm32")'.dependencies]
tokio_with_wasm = { workspace = true }
```

**lib.rs** — tokio alias + module gating:
```rust
#[cfg(target_arch = "wasm32")]
pub mod tokio { pub use tokio_with_wasm::alias::*; }

#[cfg(not(target_arch = "wasm32"))]
mod filesystem_module;
```

**async_trait** — dual cfg_attr:
```rust
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
```

**Time types** — use `meerkat_core::time_compat::{SystemTime, Instant, Duration}`, never `std::time::*`.

**Tokio imports** in wasm32-visible code — add `#[cfg(target_arch = "wasm32")] use crate::tokio;` then existing `use tokio::...` paths resolve through the alias.

## Override-First Resource Injection

`AgentBuildConfig` has 4 override fields. When set, `build_agent()` uses them directly, skipping filesystem resolution. On wasm32, these are always set.

| Field | Skips | wasm32 default |
|-------|-------|---------------|
| `tool_dispatcher_override` | Shell/file/project tool resolution | `CompositeDispatcher::new_wasm()` |
| `session_store_override` | Feature-flag store creation | No-op store |
| `hook_engine_override` | Filesystem hook config | `None` |
| `skill_engine_override` | Filesystem/git skill resolution | `None` |

`FactoryAgentBuilder` has `default_tool_dispatcher` and `default_session_store` — injected into ALL `build_agent()` calls including mob-spawned sessions.

## Building

```bash
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' \
  wasm-pack build meerkat-web-runtime --target web --out-dir <dir>
```

**CRITICAL: `--out-dir` is relative to CRATE root (`meerkat-web-runtime/`), not workspace root.**

Verify: `cargo check -p <crate> --target wasm32-unknown-unknown`

## Common Gotchas

1. **Stale WASM binary**: Browser caches WASM aggressively. Nuke Playwright cache between rebuilds.
2. **cargo clean scope**: `cargo clean -p <crate>` only cleans native. Use `rm -rf target/wasm32-unknown-unknown`.
3. **cfg inside async_trait**: May not propagate. Move cfg-gated logic to standalone functions outside the impl.
4. **Feature unioning**: Workspace `tokio = { features = ["full"] }` pulls mio which fails on wasm32. Each crate needs target-specific deps.
5. **MobBuilder**: Requires `.allow_ephemeral_sessions(true)` for EphemeralSessionService.
6. **Flow JSON**: Flow engine parses step output as JSON. Prompts must match or use `expected_schema_ref`.

## Subsystem Availability on wasm32

Full: agent loop, LLM providers (browser fetch), sessions (ephemeral), comms (inproc), mob orchestration (in-memory), tools (non-shell), tool scoping (`ToolScope` + per-turn overlays), skills (embedded), hooks (in-process), config (in-memory), compaction.

Excluded: shell tools, filesystem persistence, MCP client (rmcp), network comms (TCP/UDS), sub-agent spawning.

## Key Files

For detailed API surface and architecture, load: `references/api_surface.md`.

- `meerkat-web-runtime/src/lib.rs` — 25 wasm_bindgen exports
- `meerkat/src/factory.rs` — `build_agent()` with overrides, `AgentFactory::minimal()`
- `meerkat/src/service_factory.rs` — `FactoryAgentBuilder` with default injection
