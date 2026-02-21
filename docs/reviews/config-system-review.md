# Configuration System Review

**Date:** 2026-02-21
**Base revision:** `1f8a2dd` (after skills v2.1, session streams, persistent storage, realm infrastructure)
**Scope:** Cross-surface consistency, storage overlap, SDK suitability

---

## Executive Summary

The configuration system has evolved substantially since the initial review. Three of the most critical issues from prior reviews have been resolved:

- **Config propagation** (was Issue 4) -- `FactoryAgentBuilder::new_with_config_store()` now re-reads config from the store on each `build_agent()` call. REST, RPC, and MCP surfaces all use this path.
- **`build_config_slot` concurrency hazard** (was Issue 5) -- the shared mutable slot pattern has been entirely removed. `AgentBuildConfig` is now constructed from `CreateSessionRequest` inside the builder.
- **CLI storage backend divergence** (was Issue 13) -- session management commands (`list`, `show`, `delete`) now go through `PersistentSessionService` with realm-scoped storage. The direct `JsonlStore` bypass is gone.

Additionally, a `ConfigRuntime` layer with generation-based optimistic concurrency has been added across all three server surfaces, and a `ConfigEnvelope` wire type provides policy-aware path exposure.

The remaining issues center on structural duplication in the config type, Rust-specific serialization leaking through the wire format, and the CLI `run` command still using `EphemeralSessionService`.

---

## Resolved Issues

### R1: Config Changes Now Propagate (formerly Issue 4)

**Files:** `meerkat/src/service_factory.rs:141-186`, `meerkat-rest/src/lib.rs:212`, `meerkat-mcp-server/src/lib.rs:386`, `meerkat-rpc/src/session_runtime.rs:130`

`FactoryAgentBuilder` now holds an `Option<Arc<dyn ConfigStore>>` and calls `resolve_config()` on each `build_agent()`:

```rust
async fn resolve_config(&self) -> Config {
    if let Some(store) = &self.config_store {
        match store.get().await {
            Ok(config) => return config,
            Err(err) => {
                tracing::warn!("Failed to read latest config from store: {err}");
            }
        }
    }
    self.config_snapshot.clone()
}
```

REST, MCP, and RPC all use `new_with_config_store()`. Fallback to snapshot on store errors is sensible.

### R2: `build_config_slot` Removed (formerly Issue 5)

The `Arc<Mutex<Option<AgentBuildConfig>>>` shared slot pattern and the `promote_lock` workaround have been completely removed. `AgentBuildConfig` is now constructed from `CreateSessionRequest` inside `SessionAgentBuilder::build_agent()`, making the data flow explicit and safe under concurrency.

### R3: CLI Session Commands Use Persistent Service (formerly Issue 13)

Session management commands now use `build_cli_persistent_service()` which creates a `PersistentSessionService` with realm-scoped redb storage, matching the server surfaces.

### R4: REST Config Store Now Realm-Scoped (formerly Issue 8, partially Issue 14)

REST no longer reads from an isolated `~/.local/share/meerkat/rest/config.toml`. It uses `TaggedConfigStore` backed by `realm_paths.config_path`, with `ConfigResolvedPaths` surfacing the actual paths through `ConfigEnvelope`.

### R5: MCP Server Config Loading Improved (formerly Issue 7)

MCP server now loads config from realm-scoped `FileConfigStore` via `realm_config_store()` with proper metadata tagging, matching the REST/RPC pattern. All three surfaces read from the same realm config path when pointed at the same realm.

---

## Remaining Issues

### Issue 1: Dual Provider Configuration Paths

**Severity:** Medium
**Files:** `meerkat-core/src/config.rs:28-38`, `meerkat/src/factory.rs:627-654`

The `Config` struct still has two mechanisms:

```rust
pub struct Config {
    pub provider: ProviderConfig,   // Tagged enum: Anthropic { api_key, base_url }
    pub providers: ProviderSettings, // base_urls: HashMap
}
```

`build_agent()` ignores `ProviderConfig` entirely -- it resolves the provider from the model name, reads API keys from env vars via `ProviderResolver::api_key_for()`, and reads base URLs from `config.providers.base_urls`.

Meanwhile, `apply_env_overrides()` still writes API keys into `ProviderConfig`, which is never consumed by the factory.

**Recommendation:** Deprecate `ProviderConfig`. Consolidate into `ProviderSettings`.

---

### Issue 2: Three-Way Storage Path Configuration

**Severity:** Medium (reduced from prior review -- realm infrastructure mitigates some confusion)
**Files:** `meerkat-core/src/config.rs:29,37,836`, `meerkat-store/src/lib.rs:66-98`

Three config sections relate to storage: `storage.directory`, `store.sessions_path`, `store.database_dir`. Two resolution functions (`resolve_store_path()`, `resolve_database_dir()`) use different fallback chains.

In practice, the new realm infrastructure (`realm_paths_in()`, `open_realm_session_store_in()`) now determines paths for server surfaces, making these config fields less relevant for REST/RPC/MCP. They still matter for CLI `run` (which doesn't use realms for its own ephemeral sessions).

**Recommendation:** Merge `StorageConfig` into `StoreConfig`. Document that realm-scoped surfaces derive paths from realm infrastructure rather than these fields.

---

### Issue 3: `max_tokens` in Three Places

**Severity:** Medium
**Files:** `meerkat-core/src/config.rs:35,617,1019`

Still three locations:
1. `config.max_tokens: u32` -- top-level
2. `config.agent.max_tokens_per_turn: u32` -- agent sub-config
3. `config.budget.max_tokens: Option<u64>` -- budget (note: `u64`)

REST reads `config.agent.max_tokens_per_turn` (`meerkat-rest/src/lib.rs:190`). The factory reads `config.max_tokens` (`factory.rs:672`).

**Recommendation:** Unify to single field with consistent type.

---

### Issue 4: `Config` Type Is Not SDK-Friendly

**Severity:** High
**Files:** `meerkat-core/src/config.rs`, `meerkat-core/src/config_runtime.rs:29-68`

The new `ConfigEnvelope` is a good step -- it adds `generation`, `realm_id`, `instance_id`, `backend`, and `ConfigEnvelopePolicy` for controlling path exposure. But the `config` field inside the envelope is still the raw Rust `Config` struct:

```rust
pub struct ConfigEnvelope {
    pub config: Config,   // <-- still the raw Rust type
    pub generation: u64,
    pub realm_id: Option<String>,
    // ...
}
```

This means the same Rust serialization artifacts leak through: `Duration` as `{"secs": 30, "nanos": 0}`, tagged enums, `Box<RawValue>`, `PathBuf` fields.

**Recommendation:** Define a wire-format `WireConfig` in `meerkat-contracts` that normalizes these for SDK consumers. `ConfigEnvelope` should contain `WireConfig` instead of `Config`.

---

### Issue 5: CLI `run` Command Still Uses EphemeralSessionService

**Severity:** Medium
**Files:** `meerkat-cli/src/main.rs:1849-1856,2286`

The CLI `run` command builds via `build_cli_service()` which creates an `EphemeralSessionService`:

```rust
fn build_cli_service(factory: AgentFactory, config: Config) -> EphemeralSessionService<FactoryAgentBuilder> {
    let builder = FactoryAgentBuilder::new(factory, config);
    EphemeralSessionService::new(builder, 64)
}
```

This uses `FactoryAgentBuilder::new()` (snapshot-based), not `new_with_config_store()`. Sessions from `rkat run` are not persisted to redb, and the builder doesn't pick up config changes.

Meanwhile, `rkat rpc` (line 2407) also uses `FactoryAgentBuilder::new()` (not `new_with_config_store()`), though the RPC server has its own `ConfigRuntime` layer.

The session management commands (`list`, `show`, `delete`) now use `PersistentSessionService`, so they see redb-persisted sessions but not ephemeral `rkat run` sessions.

**Recommendation:** Wire `rkat run` through `PersistentSessionService` or at minimum use `new_with_config_store()` for config propagation.

---

### Issue 6: Inconsistent Config Merge Semantics

**Severity:** Medium
**Files:** `meerkat-core/src/config.rs:219-294`

`Config::merge()` still uses three different strategies:
1. Field-by-field default comparison (setting to default = not set)
2. Wholesale replacement (always replaces)
3. Append semantics (hooks entries)

**Recommendation:** Option-based merging.

---

### Issue 7: Three-Level Tool Flag Override

**Severity:** Low
**Files:** `meerkat-core/src/config.rs:1071-1091`, `meerkat/src/factory.rs:271-287`

Three levels: config -> factory -> per-request. MCP/RPC use max-permissive factory, CLI/REST use config-literal.

**Recommendation:** Collapse factory flags into per-request overrides.

---

### Issue 8: Compaction Config Not User-Configurable

**Severity:** Low
**Files:** `meerkat/src/factory.rs:948-956`

`CompactionConfig::default()` is still hardcoded in the factory.

**Recommendation:** Add `compaction` section to `Config`.

---

### Issue 9: Config Validation Is Narrow

**Severity:** Medium
**Files:** `meerkat-core/src/config.rs:460-523`

`Config::validate()` only validates `sub_agents`. The skills system now validates its own config via `build_source_identity_registry()` during config/set in RPC handlers (`handlers/config.rs:93-104`), but this is handler-level validation, not `Config::validate()`. Other surfaces don't get this validation on set/patch.

**Recommendation:** Move skills config validation into `Config::validate()` so it applies uniformly. Extend to other sections.

---

### Issue 10 (NEW): RPC Skills Registry Update Only Happens for RPC Surface

**Severity:** Medium
**Files:** `meerkat-rpc/src/handlers/config.rs:160-184,247-273`, `meerkat-rest/src/lib.rs:688-740`

When config is set/patched through the RPC surface, the handler validates the skills config and updates `SessionRuntime`'s `SourceIdentityRegistry`:

```rust
let registry = match build_registry_or_invalid_params(id.clone(), &config) {
    Ok(registry) => registry,
    Err(response) => return response,
};
// ...
runtime.set_skill_identity_registry_for_generation(snapshot.generation, committed_registry);
```

The REST and MCP surfaces do **not** perform this validation or registry update on config/set/patch. Their handlers delegate directly to `ConfigRuntime::set()` / `ConfigRuntime::patch()` without skill-aware validation.

This means:
- A skills config change via RPC is validated and takes effect immediately
- The same change via REST or MCP is persisted but not validated against the identity registry, and running agents don't pick up the new registry until next `build_agent()`

**Recommendation:** Move the `build_source_identity_registry()` validation into `ConfigRuntime::set()`/`patch()` or `Config::validate()` so it applies uniformly. For registry propagation to running sessions, consider a shared registry similar to the RPC pattern.

---

### Issue 11 (NEW): Generation Counter Not Wired for CLI

**Severity:** Low
**Files:** `meerkat-cli/src/main.rs:1542-1607`

The CLI `config set` and `config patch` commands use `FileConfigStore` directly without `ConfigRuntime`. They don't participate in generation tracking:

```rust
async fn handle_config_set(...) -> anyhow::Result<()> {
    store.set(config).await?;
    // No ConfigRuntime, no generation, no ConfigEnvelope
}
```

This means:
- CLI config writes don't increment the generation counter
- A concurrent `config/set` from an RPC or REST client could silently overwrite CLI changes (or vice versa) without a generation conflict

**Recommendation:** Wire `ConfigRuntime` into CLI config commands, or at minimum document that CLI config changes don't participate in CAS.

---

## Summary

| # | Issue | Severity | Status |
|---|-------|----------|--------|
| R1 | Config changes don't propagate | -- | **Resolved** |
| R2 | `build_config_slot` concurrency hazard | -- | **Resolved** |
| R3 | CLI session commands bypass SessionService | -- | **Resolved** |
| R4 | REST isolated config store | -- | **Resolved** (realm-scoped) |
| R5 | MCP bypasses config layering | -- | **Resolved** (realm-scoped) |
| 1 | Dual provider config | Medium | Open |
| 2 | Three-way storage path config | Medium | Open (mitigated by realms) |
| 3 | `max_tokens` in three places | Medium | Open |
| 4 | Config not SDK-friendly | High | Open (ConfigEnvelope is good, but inner Config leaks) |
| 5 | CLI `run` still ephemeral | Medium | Open |
| 6 | Inconsistent merge semantics | Medium | Open |
| 7 | Three-level tool flag override | Low | Open |
| 8 | Compaction not configurable | Low | Open |
| 9 | Config validation narrow | Medium | Open (skills validation is surface-specific) |
| 10 | Skills registry update RPC-only | Medium | **New** |
| 11 | CLI config has no generation tracking | Low | **New** |

**High:** 1 (Issue 4)
**Medium:** 6 (Issues 1, 2, 3, 5, 6, 9, 10)
**Low:** 3 (Issues 7, 8, 11)
