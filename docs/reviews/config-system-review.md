# Configuration System Review

**Date:** 2026-02-15
**Base revision:** `5577c14` (after persistent session storage PR #26)
**Scope:** Cross-surface consistency, storage overlap, SDK suitability

---

## Executive Summary

The configuration system has solid foundations -- a single `Config` struct, a `ConfigStore` abstraction, and a consistent config/get/set/patch API across REST, RPC, and CLI surfaces. However, it has accumulated structural debt that creates real problems for non-Rust consumers and introduces subtle inconsistencies between surfaces.

PR #26 (persistent session storage) resolved several previous concerns -- server surfaces now use `PersistentSessionService` + `RedbSessionStore` and there's a new `database_dir` config field. But it also introduced new inconsistencies: the CLI `run` command still uses `EphemeralSessionService` while `rkat rpc` uses persistent storage, and session management commands (`list`, `show`, `delete`) still bypass `SessionService` to access `JsonlStore` directly.

The remaining issues fall into four categories:

1. **Structural duplication** -- overlapping config fields that serve the same purpose
2. **Config-as-snapshot semantics** -- config changes don't propagate to running agents
3. **Rust-native assumptions** -- the `Config` type is too coupled to Rust details for SDK use
4. **Storage divergence** -- three storage backends (JSONL, redb, ephemeral) coexist with inconsistent config and feature wiring

---

## Issue 1: Dual Provider Configuration Paths

**Files:** `meerkat-core/src/config.rs:28-38`, `meerkat/src/factory.rs:640-654`

The `Config` struct has two separate mechanisms for provider configuration:

```rust
pub struct Config {
    pub provider: ProviderConfig,   // Tagged enum: Anthropic { api_key, base_url }
    // ...
    pub providers: ProviderSettings, // base_urls: HashMap, api_keys: HashMap
}
```

`ProviderConfig` is a tagged enum that locks the config to a single provider and carries API keys inline. `ProviderSettings` is a flat map that supports multiple providers simultaneously.

**In practice, `build_agent()` ignores `ProviderConfig` entirely** -- it resolves the provider from the model name (step 2, `factory.rs:627-637`), reads API keys from environment variables via `ProviderResolver::api_key_for()` (step 3, `factory.rs:644`), and reads base URLs from `config.providers.base_urls` (step 3, `factory.rs:649-653`).

Meanwhile, `apply_env_overrides()` (`config.rs:306-338`) writes API keys into `ProviderConfig`, which is never read by the factory.

**Impact:** The `ProviderConfig` enum is vestigial. It creates confusion for users who set `[provider]` in their config expecting it to work, and for SDK consumers who need to understand which of two provider config mechanisms actually takes effect.

**Recommendation:** Deprecate `ProviderConfig`. Consolidate provider configuration into `ProviderSettings` (the map-based approach), which naturally supports multi-provider setups. Route `apply_env_overrides()` to write into `providers.api_keys` instead.

---

## Issue 2: Three-Way Storage Path Configuration

**Files:** `meerkat-core/src/config.rs:29,37,836`, `meerkat-store/src/lib.rs:66-98`

After PR #26, there are now **three** config sections related to storage paths:

```rust
pub struct Config {
    pub storage: StorageConfig,  // { directory: Option<PathBuf> }         -- generic fallback
    pub store: StoreConfig,      // { sessions_path, tasks_path, database_dir }  -- specific paths
}
```

With **two** resolution functions in `meerkat-store`:

1. `resolve_store_path()` -- sessions JSONL: `store.sessions_path > storage.directory > platform`
2. `resolve_database_dir()` -- redb database: `store.database_dir > storage.directory/db > platform`

Both fall back to `storage.directory` as a middle tier, but branch to different subdirectories.

Additionally, REST inlines the `resolve_store_path()` logic (`meerkat-rest/src/lib.rs:102-112`) instead of calling the function, and then patches `store.database_dir` from `instance_root` before calling `resolve_database_dir()` (`meerkat-rest/src/lib.rs:129-132`).

**Impact:** Users now face three config fields (`storage.directory`, `store.sessions_path`, `store.database_dir`) that interact in non-obvious ways. The REST surface mutates config before resolving, adding a fourth implicit path source.

**Recommendation:** Merge `StorageConfig` into `StoreConfig`. Establish a single `store.root` field as the base directory, with `sessions_path` and `database_dir` as optional overrides relative to that root. Remove the inline path resolution in REST.

---

## Issue 3: `max_tokens` Exists in Three Places

**Files:** `meerkat-core/src/config.rs:35,617,1019`

Token limits are configured via:

1. `config.max_tokens: u32` (line 35) -- top-level field
2. `config.agent.max_tokens_per_turn: u32` (line 617) -- agent sub-config
3. `config.budget.max_tokens: Option<u64>` (line 1021) -- budget sub-config (note: `u64`, not `u32`)

The factory resolves `max_tokens` from `build_config.max_tokens.unwrap_or(config.max_tokens)` (`factory.rs:672`), ignoring `agent.max_tokens_per_turn`. But `AppState` in REST reads `config.agent.max_tokens_per_turn` (`meerkat-rest/src/lib.rs:118`).

The type mismatch (`u32` vs `u64`) between `max_tokens` and `budget.max_tokens` adds to the confusion.

**Impact:** Different surfaces read different fields for the same concept. REST uses `agent.max_tokens_per_turn`, the factory uses `config.max_tokens`. A user setting one may not affect the other.

**Recommendation:** Unify to a single `max_tokens` field with consistent type. If per-turn and per-session budgets are genuinely different concepts, name them unambiguously (e.g., `max_tokens_per_turn` vs `max_tokens_budget`).

---

## Issue 4: Config Changes Don't Propagate to Running Agents

**Files:** `meerkat-rest/src/lib.rs:597-631`, `meerkat-rpc/src/handlers/config.rs`, `meerkat/src/service_factory.rs:148-155`

The config/set and config/patch endpoints update the `ConfigStore` (file or memory), but the `FactoryAgentBuilder` holds a frozen `Config` snapshot captured at construction:

```rust
pub struct FactoryAgentBuilder {
    factory: AgentFactory,
    config: Config,              // <-- snapshot, never updated
    // ...
}
```

When `config/set` writes a new config, subsequent `build_agent()` calls still use the stale snapshot because `FactoryAgentBuilder` doesn't re-read from the store.

**Impact:** For REST and RPC surfaces, config changes via the API are persisted but silently ignored until the server process restarts. This is a confusing UX -- the API returns success, the file on disk changes, but behavior doesn't.

**Recommendation:** Either:
- (a) Make `FactoryAgentBuilder` read config from the `ConfigStore` on each `build_agent()` call, or
- (b) Document that config changes require a server restart, and return a response indicating this, or
- (c) Remove set/patch from REST/RPC if hot-reload isn't supported.

Option (a) is the cleanest. It would require `FactoryAgentBuilder` to hold an `Arc<dyn ConfigStore>` instead of a `Config` value, and read from it in `build_agent()`.

---

## Issue 5: `build_config_slot` Mutex Is a Concurrency Hazard

**Files:** `meerkat/src/service_factory.rs:154`, `meerkat-rpc/src/session_runtime.rs:80-82`

The `build_config_slot: Arc<Mutex<Option<AgentBuildConfig>>>` pattern requires surfaces to stage config into a shared slot before calling `create_session()`. This works for single-threaded CLI use, but for REST and RPC with concurrent requests, there's a window between staging and consumption where another request could overwrite the slot.

The RPC surface mitigates this with a `promote_lock: Mutex<()>` (`session_runtime.rs:80`) that serializes the stage-then-create sequence. The REST surface uses `builder_slot: Arc<Mutex<Option<AgentBuildConfig>>>` (`meerkat-rest/src/lib.rs:73`) but the staging+create is not wrapped in a single lock scope in all handler paths.

**Impact:** Under concurrent REST requests, one request could stage a config that gets consumed by a different request's `create_session()` call.

**Recommendation:** Replace the shared mutable slot with a per-request pattern. Pass `AgentBuildConfig` directly through `CreateSessionRequest` (or a new builder parameter) instead of relying on side-channel staging. This would eliminate the concurrency issue and make the data flow explicit.

---

## Issue 6: `Config` Type Is Not SDK-Friendly

**Files:** `meerkat-core/src/config.rs` (full file)

The `Config` struct contains Rust-specific details that don't translate to SDK consumers:

1. **`ProviderConfig` is a tagged enum** with `#[serde(tag = "type")]` -- serializes as `{"type": "anthropic", "api_key": ...}`. This is idiomatic in Rust but awkward in Python/TypeScript where a simple `{"provider": "anthropic", "api_key": "..."}` flat object would be more natural.

2. **`Duration` fields** use `humantime_serde` (e.g., `"30s"`, `"1m"`) for TOML friendliness, but the JSON serialization produces `{"secs": 30, "nanos": 0}` -- an internal Rust Duration representation that SDK consumers would need to manually construct.

3. **`HookRuntimeConfig`** uses custom serialize/deserialize that flattens `config` fields into the parent object when they're an object, but nests them under `"config"` when they're not. This polymorphic shape is difficult to model in typed languages.

4. **`Box<RawValue>`** in `HookRuntimeConfig` -- opaque pass-through JSON that can't be represented in generated SDK types.

5. **`PathBuf`** fields (`sessions_path`, `storage.directory`, `database_dir`) serialize as platform-specific path strings. A REST API consumer on a different OS would produce paths the server can't use.

**Impact:** The config/get and config/set APIs expose the raw Rust `Config` struct over the wire. SDK consumers receiving or constructing this type must deal with Rust serialization artifacts.

**Recommendation:** Define a separate wire-format `ConfigResponse`/`ConfigRequest` in `meerkat-contracts` that:
- Uses ISO 8601 duration strings (or plain seconds) instead of Rust Duration
- Flattens provider config into a simple object
- Omits implementation-specific fields like `PathBuf` storage paths
- Has stable, documented JSON schema

The internal `Config` struct stays as-is for the Rust runtime; the contracts layer handles translation.

---

## Issue 7: MCP Server Bypasses Config Layering

**File:** `meerkat-mcp-server/src/lib.rs:151-157`

The MCP server loads config via:
```rust
async fn load_config_async() -> Config {
    let project_root = resolve_project_root();
    let store = FileConfigStore::project(&project_root);
    let mut config = store.get().await.unwrap_or_else(|_| Config::default());
    let _ = config.apply_env_overrides();
    config
}
```

This creates a `FileConfigStore::project()` which reads from `{project_root}/.rkat/config.toml` directly. It does **not** use `Config::load()`, which implements the full layering: defaults -> project config -> global config -> env overrides.

If there's no `.rkat/config.toml` in the project, `FileConfigStore::get()` returns `Config::default()` -- it never falls back to `~/.rkat/config.toml`.

**Impact:** The MCP server ignores global user config (`~/.rkat/config.toml`). A user who sets their API keys or model preferences in global config will find they work in CLI and RPC but not through the MCP server.

**Recommendation:** Use `Config::load()` in the MCP server, consistent with CLI and RPC.

---

## Issue 8: REST Config Store Location Is Inconsistent

**File:** `meerkat-rest/src/lib.rs:89-92`

The REST server creates its own config store at `{instance_root}/config.toml`:

```rust
let config_store: Arc<dyn ConfigStore> =
    Arc::new(FileConfigStore::new(instance_root.join("config.toml")));
```

Where `instance_root` is `~/.local/share/meerkat/rest/` (from `rest_instance_root()`). This is separate from both the global config (`~/.rkat/config.toml`) and any project config (`.rkat/config.toml`).

On initial load (`AppState::load_from()`), the store reads from its own `config.toml`, which is typically empty/missing, so it falls back to `Config::default()` and then applies env overrides. It never reads from `~/.rkat/config.toml` or any project config.

**Impact:** The REST server has its own isolated config universe. Settings in `~/.rkat/config.toml` don't apply. Config changes via the REST API don't affect CLI or RPC. This is intentional isolation for a daemon, but it's undocumented and surprising.

**Recommendation:** Document this behavior explicitly. Consider adding an `--config` flag or `RKAT_CONFIG` env var to let users point the REST server at their preferred config file.

---

## Issue 9: Compaction Config Is Not User-Configurable

**File:** `meerkat/src/factory.rs:948-956`, `meerkat-core/src/compact.rs`

The factory always uses `CompactionConfig::default()` when the `session-compaction` feature is enabled:

```rust
#[cfg(feature = "session-compaction")]
let compactor = {
    let config = meerkat_core::CompactionConfig::default();
    Some(Arc::new(meerkat_session::DefaultCompactor::new(config))
        as Arc<dyn meerkat_core::Compactor>)
};
```

The `CompactionConfig` has meaningful tunables (threshold, budget, max_summary_tokens, min_turn_gap), but there's no path from `Config` -> `CompactionConfig`. Users cannot adjust compaction behavior without recompiling.

**Impact:** Compaction thresholds are locked at compile-time defaults. Users with large context windows or different use patterns can't tune compaction.

**Recommendation:** Add a `compaction: CompactionConfig` section to `Config` and wire it through the factory.

---

## Issue 10: Inconsistent Config Merge Semantics

**File:** `meerkat-core/src/config.rs:219-294`

The `Config::merge()` method uses two different strategies:

1. **Field-by-field comparison against defaults** for most fields (e.g., `if other.agent.model != AgentConfig::default().model`). This means if a user explicitly sets a field to its default value, the merge treats it as "not set" and skips it.

2. **Wholesale replacement** for some fields (e.g., `self.provider = other.provider` on line 235 -- always replaces, no default check).

3. **Append semantics** for hooks entries (line 292 -- `self.hooks.entries.extend(other.hooks.entries)`).

This creates surprising behavior: if the config template sets `model = "claude-opus-4-6"` and a user's config file also sets `model = "claude-opus-4-6"`, the merge skips it because it equals the default. If they then change the template default, the user's config silently changes too.

**Impact:** Merge behavior is unpredictable. "Setting a value to its default" and "not setting a value" are indistinguishable.

**Recommendation:** Use Option-based merging: parse the file config with all-optional fields, then overlay present values onto defaults. This is what `TemplateDefaults` already does -- the same pattern should apply to `Config::merge()`.

---

## Issue 11: `ToolsConfig` Flags vs. `AgentFactory` Flags vs. `AgentBuildConfig` Overrides

**Files:** `meerkat-core/src/config.rs:1071-1091`, `meerkat/src/factory.rs:271-287`, `meerkat/src/factory.rs:150-161`

Tool enablement is controlled at three levels:

1. `config.tools.builtins_enabled` / `shell_enabled` / `comms_enabled` / `subagents_enabled` (config level)
2. `factory.enable_builtins` / `enable_shell` / `enable_subagents` / `enable_memory` (factory level)
3. `build_config.override_builtins` / `override_shell` / `override_subagents` / `override_memory` (per-request level)

The factory flags are set from the config flags during surface initialization. The build config overrides can then override the factory flags per-request.

The MCP server and `rkat rpc` set factory flags to max-permissive (`builtins: true, shell: true`) and rely on per-request overrides. The CLI `run` and REST read config flags literally.

**Impact:** The three-level override system works but is hard to reason about. The "always-permissive factory" pattern in MCP/RPC vs. the "config-literal" pattern in CLI/REST is a design divergence.

**Recommendation:** Consider collapsing factory flags into `AgentBuildConfig` overrides only, populated from config at the surface level. This reduces the override levels from three to two (config -> per-request) and makes the data flow clearer.

---

## Issue 12: Config Validation Is Narrow

**File:** `meerkat-core/src/config.rs:460-523`

`Config::validate()` only validates `sub_agents` configuration (allowlists, provider names). It does not validate:

- Model names exist or are recognized
- API keys are present for the selected provider
- Duration values are non-negative
- `max_tokens` values are within provider limits
- Storage paths are writable
- REST host/port are valid
- Comms addresses are syntactically valid

Validation is called by `ConfigStore::set()` and `ConfigStore::patch()`, so invalid config can be persisted if it passes the narrow sub-agents check.

**Recommendation:** Extend `validate()` to cover structural correctness of all sections. Provider-specific validation (API key presence, model compatibility) can be deferred to agent build time, but structural validation (valid host:port, non-zero timeouts, sensible duration ranges) should happen at persist time.

---

## Issue 13 (NEW): CLI Storage Backend Divergence

**File:** `meerkat-cli/src/main.rs:943-969,992-995,1101-1113`

After PR #26, the CLI has a split personality:

- **`rkat rpc`** uses `PersistentSessionService` + `RedbSessionStore` (lines 958-969) -- sessions survive restarts.
- **`rkat run`** uses `EphemeralSessionService` (line 1101-1112) -- sessions are memory-only for the process lifetime. JSONL store is configured in the factory but only used for internal `AgentBuilder` plumbing.
- **`rkat session list/show/delete`** bypasses `SessionService` entirely and reads from `JsonlStore` directly (lines 992-994, 1517-1518, 1566, 1664).

This means:
- Sessions created via `rkat run` are saved to JSONL but **not** to the redb database.
- Sessions created via `rkat rpc` are saved to redb but the session management commands read from JSONL.
- `rkat session list` after using `rkat rpc` will **not** show those sessions.

The code itself acknowledges this with TODO comments:
```
/// TODO: Migrate to `PersistentSessionService::list()` when the `session-store`
/// feature is wired into the CLI.
```

**Impact:** The CLI has three parallel storage paths that don't see each other's data. A user switching between `rkat run` and `rkat rpc` (or using an IDE that connects via RPC) will get inconsistent session lists.

**Recommendation:** Wire the CLI `run` command through `PersistentSessionService` and migrate the session management commands to use `SessionService` instead of direct `JsonlStore` access. This aligns CLI with the server surfaces.

---

## Issue 14 (NEW): REST Mutates Config Before Path Resolution

**File:** `meerkat-rest/src/lib.rs:129-132`

The REST surface patches `database_dir` into the config before resolving:

```rust
if config.store.database_dir.is_none() {
    config.store.database_dir = Some(instance_root.join("db"));
}
let db_dir = meerkat_store::resolve_database_dir(&config);
```

This mutation happens after config loading but before the config is passed to `FactoryAgentBuilder`. The mutated config is then frozen in the builder. However, the `ConfigStore` still returns the **original** unmutated config via `config/get`, so a client reading config would not see the actual `database_dir` being used.

The MCP server does **not** do this patching -- it calls `resolve_database_dir()` directly and lets the standard fallback logic apply. So the two server surfaces have different `database_dir` resolution behavior for the same config.

**Impact:** The REST surface's effective database location is invisible through the config API. A client calling `config/get` would see `database_dir: null` even though the actual path is `~/.local/share/meerkat/rest/db`. This makes debugging storage issues harder.

**Recommendation:** Either:
- (a) Persist the resolved `database_dir` back to the config store so `config/get` reflects reality, or
- (b) Add a separate `runtime/info` endpoint that reports resolved paths, or
- (c) Treat `instance_root` as a first-class concept -- like MCP Server's `resolve_project_root()` -- and document how each surface determines its data directory.

---

## Summary of Recommendations

| # | Issue | Severity | Recommendation |
|---|-------|----------|----------------|
| 1 | Dual provider config | Medium | Deprecate `ProviderConfig` enum |
| 2 | Three-way storage path config | Medium | Merge `StorageConfig` into `StoreConfig` with single root |
| 3 | `max_tokens` in three places | Medium | Unify to single field with clear semantics |
| 4 | Config changes don't propagate | High | Read from ConfigStore on each build |
| 5 | `build_config_slot` mutex | High | Pass config through request, not side-channel |
| 6 | Config not SDK-friendly | High | Wire-format types in meerkat-contracts |
| 7 | MCP bypasses config layering | Medium | Use `Config::load()` |
| 8 | REST isolated config store | Low | Document; add --config flag |
| 9 | Compaction not configurable | Low | Add compaction section to Config |
| 10 | Inconsistent merge semantics | Medium | Option-based merging |
| 11 | Three-level tool flag override | Low | Collapse to two levels |
| 12 | Narrow validation | Medium | Extend to all config sections |
| 13 | CLI storage backend divergence | High | Wire CLI through PersistentSessionService |
| 14 | REST mutates config silently | Medium | Surface resolved paths via API or config |
