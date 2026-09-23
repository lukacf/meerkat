# CLAUDE.md

This file provides guidance to agents when working with code in this repository.
Whatever you do, remember: All hail Clippy. Clippy sees all, knows all, and tolerates nothing.

## Project Overview

Meerkat (`rkat`) is a library-first Rust platform for building, hosting, and
operating LLM-powered agents. It provides the agent loop, providers, tools,
typed events, persistence, runtime control, and multi-agent orchestration
behind CLI, REST, JSON-RPC, MCP, SDK, and browser/WASM surfaces.

**Naming convention:**
- Project/branding: **Meerkat**
- CLI binary: **rkat**
- Crate names: `meerkat`, `meerkat-core`, `meerkat-client`, etc.
- Config directory: `.rkat/`
- Environment variables: provider secrets use `RKAT_*` and provider-native keys;
  ordinary configuration stays declarative. Explicit operational/diagnostic
  overrides also exist, such as `RKAT_WORKER_STACK_BYTES` for
  [worker-stack diagnosis](docs/guides/deploying.mdx#worker-stack-budget).

## Build and Test Commands

Use Make targets for normal local work. Cargo is the default backend; setting
`MEERKAT_BUILDBUDDY=1` routes supported broad local lanes through the optional
BuildBuddy/Bazel path. Do not invoke raw `bb` directly unless you are debugging
the BuildBuddy wrapper itself.

```bash
# Build everything
make build

# Build everything through BuildBuddy/Bazel
MEERKAT_BUILDBUDDY=1 make build

# Explicit BuildBuddy forms
make buildbuddy-build
make buildbuddy-check
make buildbuddy-clippy
make buildbuddy-test

# Run fast tests (unit + integration-fast; skips doctests)
make test

# Run fast tests through BuildBuddy/Bazel
MEERKAT_BUILDBUDDY=1 make test

# Run all tests including doc-tests (SLOW due to doc-test compilation)
./scripts/repo-cargo test --workspace

# Run deterministic end-to-end lane
make e2e-fast

# Run deterministic end-to-end lane through BuildBuddy/Bazel
MEERKAT_BUILDBUDDY=1 make e2e-fast

# Run explicit build/composition end-to-end lane (ignored by default)
./scripts/repo-cargo test -p meerkat-integration-tests --test e2e_build_lane -- --ignored

# Run real local-resource end-to-end lane
make e2e-system

# Run targeted live-provider lane (ignored by default)
make e2e-live

# Run kitchen-sink live smoke lane (ignored by default)
make e2e-smoke

# Run per-model catalog validation lane (ignored by default)
./scripts/repo-cargo e2e-models

# Cargo aliases (defined in .cargo/config.toml)
./scripts/cargo-rct       # Fast tests (unit + integration-fast)
./scripts/repo-cargo unit # Unit tests only
./scripts/repo-cargo int  # Integration-fast tests only
./scripts/repo-cargo e2e-fast    # Deterministic e2e lane
./scripts/repo-cargo test -p meerkat-integration-tests --test e2e_build_lane -- --ignored  # Build/composition lane
./scripts/repo-cargo e2e-system  # Real binary / local resource lane
./scripts/repo-cargo e2e-live    # Targeted live-provider lane
./scripts/repo-cargo e2e-smoke   # Compound live-provider smoke lane
./scripts/repo-cargo e2e-models  # Live per-model catalog validation (on-demand / pre-release)

# Legacy compatibility shims (during migration)
./scripts/repo-cargo int-real  # Alias for e2e-system
./scripts/repo-cargo e2e       # Alias for e2e-live + e2e-smoke

# Run the CLI
./scripts/repo-cargo run -p rkat -- run "prompt"

# Run a specific example
ANTHROPIC_API_KEY=... ./scripts/repo-cargo run --example simple
```

## Build Efficiency

This is a large workspace (50 members). Careless builds waste minutes. Follow these rules:

**Use package-scoped commands during development.** Do NOT default to `--workspace` for every build or check. Scope to the crate you're changing and its immediate dependents:

```bash
# Editing meerkat-core? Check just what you touched + direct dependents
./scripts/repo-cargo check -p meerkat-core -p meerkat-runtime -p meerkat-session

# Editing meerkat-mob? Check the mob subtree
./scripts/repo-cargo check -p meerkat-mob -p meerkat-mob-mcp

# Running tests for one crate
./scripts/repo-cargo nextest run -p meerkat-mob

# Only use workspace-wide commands for final verification
./scripts/repo-cargo clippy --workspace -- -D warnings
./scripts/repo-cargo nextest run --workspace --status-level none --final-status-level fail
```

**Do not launch overlapping unisolated Cargo commands in one checkout.** Use
the repository wrapper and a distinct `RUST_LANE_ID` for concurrent same-tree
agents that need separate warm output roots. Separate worktrees are already
isolated by their path hash.

**Always use `./scripts/repo-cargo`** instead of bare `cargo`. The wrapper manages per-worktree build caches and avoids cross-worktree cache pollution.

**Use BuildBuddy for broad final verification when available.** Prefix the normal broad lanes with `MEERKAT_BUILDBUDDY=1`: `make build`, `make lint`, `make test`, `make test-unit`, `make test-int`, `make e2e-fast`, and `make e2e-system` then use the optional macOS arm64 BuildBuddy/Bazel backend. `make buildbuddy-doctor` verifies credentials, the pinned `bb` CLI, Bazel metadata freshness, selector health, and lane isolation.

**Key dependency chains to know** (touching a crate rebuilds everything downstream):
- `meerkat-core` → rebuilds almost everything (~27s incremental)
- `meerkat-runtime` → rebuilds mob, rpc, rest, cli, integration tests
- `meerkat-mob` → rebuilds mob-mcp, rpc, rest, cli, integration tests
- `meerkat-machine-schema` → `meerkat-machine-kernels`, `meerkat-runtime`, and `meerkat-mob`, then their downstream consumers; it is not a leaf crate

## Architecture

```
meerkat-sqlite    → Shared SQLite mechanics (connection profiles, meerkat_schema migration
                     ledger, JsonColumnBytes codec, per-operation maintenance-fence guards)
meerkat-agent-build-authority → Retired compatibility marker only; factory build
                     finalization authority is private to the facade/core seam
meerkat-store-conformance → Published storage conformance harness (per-trait capability
                     profiles, capability-discovery, append-only media, blob chapters)
meerkat-core      → Agent loop, types, budget, retry, state machine, foundational contracts,
                     and bounded native config/path/lock/fence I/O
                     Also: SessionService trait, Compactor + CompactionCurator traits, MemoryStore trait,
                     ToolExecutionPolicy/ExecutionPolicyGatedDispatcher (list-preserving call-level tool gate), SessionError
                     Owns the model-catalog vocabulary types + ModelCatalog mechanics (no provider data)
                     Also: StorageLayout path authority + realm-id-first dual-root resolution,
                     DurabilityClass vocabulary, StorageMigrator::diagnose seam
meerkat-models    → Provider model catalog/capabilities data (core stays provider-free);
                     exposes `canonical()` ModelCatalog injected into core seams
meerkat-llm-core  → LLM wire-client, streaming, realtime, and provider-runtime contracts
meerkat-anthropic / meerkat-openai / meerkat-gemini → Per-provider clients implementing AgentLlmClient
meerkat-client    → Thin shim re-exporting meerkat-llm-core + the per-provider clients (B2 split)
meerkat-auth-core → Cross-target credential resolver plus native token stores, OAuth, keyring,
                     lockfile, and cloud-IAM authorizers
meerkat-copilot  → Native GitHub device OAuth/CAPI token exchange, account model discovery,
                    and shared derived-token runtime for OpenAI/Anthropic/Gemini Copilot backends
meerkat-providers → Compatibility shim re-exporting provider-runtime contracts from
                     meerkat-llm-core and auth primitives from meerkat-auth-core
meerkat-store     → Session persistence (SqliteSessionStore, JsonlStore, MemoryStore) implementing SessionStore
meerkat-tools     → Tool registry and validation implementing AgentToolDispatcher
meerkat-session   → Session service orchestration (EphemeralSessionService, DefaultCompactor)
                     Features: session-store (PersistentSessionService),
                               session-compaction (DefaultCompactor)
meerkat-memory    → Semantic memory (HnswMemoryStore via hnsw_rs + SQLite, SimpleMemoryStore for tests);
                     lazy per-scope index loading + host lifecycle APIs (drop_scope, enumerate_scoped)
meerkat-mcp       → MCP protocol client, McpRouter for tool routing
meerkat-mcp-server → Expose Meerkat as MCP tools (meerkat_run, meerkat_resume, meerkat_config, meerkat_capabilities)
meerkat-rpc       → JSON-RPC stdio server (stateful SessionRuntime, IDE/desktop integration)
meerkat-rest      → Optional REST API server
meerkat-comms     → Inter-agent communication (Ed25519-signed messaging, transports, trust model)
meerkat-capabilities → Typed capability vocabulary and feature-owned declaration collection
meerkat-contracts → Wire types, error codes, generated schemas, supervisor bridge protocol
                     (canonical over all surfaces; BridgeCommand/BridgeReply for mob↔runtime boundary)
meerkat-atif      → ATIF-v1.7 trajectory model and canonical committed-event exporter
meerkat-skills    → Skill loading, resolution, rendering (filesystem, git, HTTP, embedded sources)
meerkat-hooks     → Hook infrastructure (in-process, command, HTTP runtimes)
meerkat-mob       → Multi-agent mob orchestration (spawn, provision, finalize, SQLite storage, flow frames/loops)
meerkat-mob-pack  → Mobpack archive format, signing, trust policies, validation
meerkat-schedule  → Scheduler: once/interval/calendar triggers, occurrence lifecycle, delivery, schedule tools,
                     host-runnable targets (TargetBinding::HostRunnable + ScheduleRunnableHost registry)
meerkat-jobs      → Durable detached-job lifecycle, atomic store/outbox contracts,
                     predicate watches, health, and restart-safe delivery vocabulary
meerkat-mob-mcp   → Shared mob surface orchestration (MobMcpState), public MCP, and agent-facing
                     mob tools including delegate, fork_off, and temporary councils
meerkat-workgraph → Work graph (work items, dependencies) + agent-facing workgraph tools
meerkat-runtime   → Runtime control plane (MeerkatMachine, ops lifecycle, runtime handles) between surfaces and core
meerkat-live      → Live multimodal WebSocket transport plus feature-gated WebRTC signaling/media
meerkat-cli       → CLI binary (produces `rkat`)
meerkat           → Facade crate, re-exports, AgentFactory, SDK helpers
meerkat-web-runtime → WASM browser deployment target (wasm_bindgen exports)
meerkat-machine-* → Machine authority toolchain (schema catalog, DSL, codegen, kernels, derive)
meerkat-mob-adaptive → Transitional re-export of the mob-owned adaptive module
```

**Selected foundational traits in meerkat-core**:
- `AgentLlmClient` - LLM provider abstraction
- `AgentToolDispatcher` - Tool routing abstraction
- `AgentSessionStore` - Session persistence abstraction
- `SessionService` - Canonical session lifecycle (create/turn/interrupt/read/list/archive)
- `Compactor` - Context compaction strategy
- `MemoryStore` - Semantic memory indexing
- `HookEngine` - Lifecycle hook execution
- `SkillEngine` / `SkillSource` - Skill loading and resolution

**Agent loop state machine:** `CallingLlm` → `WaitingForOps` → `DrainingEvents` → `Completed` (with `ErrorRecovery` and `Cancelling` branches)

**Crate ownership:** `meerkat-core` owns foundational trait contracts. Feature crates own domain-specific contracts such as `DetachedJobStore`, `WorkGraphStore`, and `ScheduleStore`. `meerkat-store` owns `SessionStore` implementations. `meerkat-session` owns session orchestration (`EphemeralSessionService`, `PersistentSessionService`) and `EventStore`. `meerkat-memory` owns `HnswMemoryStore`. The facade (`meerkat`) wires features, re-exports, and provides `FactoryAgentBuilder`/`FactoryAgent`/`build_ephemeral_service`.

**Machine authority rule:** For canonical machine-owned domains, semantic state mutation must flow through generated machine authority, not handwritten reducers. See `docs/reference/machine-authority.mdx` (canonical registry: `canonical_machine_schemas()` in `meerkat-machine-schema/src/catalog/mod.rs`).

**Agent construction:** All surfaces use `AgentFactory::build_agent()` for centralized prompt assembly, provider resolution, tool dispatcher setup, comms wiring, and hook resolution. Zero `AgentBuilder::new()` calls in surface crates.

**Runtime build mode:** All runtime-backed surfaces use `MeerkatMachine::prepare_bindings(session_id)` to obtain `SessionRuntimeBindings`, then pass `RuntimeBuildMode::SessionOwned(bindings)` via `SessionBuildOptions.runtime_build_mode`. Standalone/test/WASM surfaces use `RuntimeBuildMode::StandaloneEphemeral` (the default).

**Session lifecycle:** Runtime-backed product surfaces (CLI, REST, MCP Server, JSON-RPC, and their process-backed Python/TypeScript clients) route through `SessionService` for create/turn/interrupt/read/list/archive. `FactoryAgentBuilder` bridges `AgentFactory` into the `SessionAgentBuilder` trait. Embedded Rust and WASM can construct standalone agents directly through `AgentFactory`; per-request service build data is passed in-band via `CreateSessionRequest.build` / `SessionBuildOptions`.

**Capability matrix:** See `docs/reference/capability-matrix.mdx` for build profiles, error codes, and feature behavior. See `docs/reference/session-contracts.mdx` for concurrency, durability, and compaction semantics.

**`.rkat/sessions/` files** are derived projection output (materialized by `SessionProjector`), NOT canonical state. When the optional event projection is installed, complete, and healthy, those views can be rebuilt from successfully projected envelopes. The `RuntimeStore`/backend carrier remains session authority; `SessionStore` rows are component/content state.

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

**Connection behavior:** MCP servers connect in parallel in the background. Tools become available as each server completes its handshake. The `[MCP_PENDING]` system notice informs the LLM while servers are still connecting.

- `connect_timeout_secs` per server in `.rkat/mcp.toml` (default: 10s)
- `--wait-for-mcp` flag on `run`/`resume` blocks until all servers finish connecting before the first turn

## JSON-RPC Stdio Server

```bash
# Start the JSON-RPC stdio server (for IDE/desktop integration)
rkat-rpc
```

The RPC server speaks JSON-RPC 2.0 over newline-delimited JSON (JSONL) on stdin/stdout. Like REST and MCP, it composes the shared runtime-backed service so agents can stay alive between turns. RPC adds a broad typed method catalog, mid-turn cancellation, and scoped event notifications for interactive clients.

**Methods:** The full callable method catalog is generated and documented in `docs/api/rpc.mdx` (gated by `scripts/verify_rpc_surface_alignment.py`, which checks it against the generated `meerkat_contracts::rpc_method_catalog`).

**Notifications** (server -> client): `session/event` with `AgentEvent` payload, emitted during turns. `session/stream_event`/`session/stream_end` for scoped session event streams, `mob/stream_event`/`mob/stream_end` for mob event streams.

**Architecture:** Each session gets a dedicated tokio task that exclusively owns the `Agent` (no mutex needed for `cancel(&mut self)`). The `SessionRuntime` dispatches commands via channels. `AgentFactory.build_agent()` consolidates the agent construction pipeline shared across all surfaces.

## Mob Orchestration

**Bridge rotation fails closed on partial remote failure.** If any remote member accepts an attempted supervisor rotation and a later remote rejects it, `handle_rotate_supervisor` leaves the persisted current supervisor authority at the pre-rotation epoch and returns `MobError::SupervisorRotationIncomplete`. When rollback cannot clear an already-accepted remote, the attempted authority is retained as explicit pending rotation metadata so retry can validate accepted peers, rotate the remaining peers, and commit only after every remote is confirmed. If a pending-accepted remote later rebinds to current authority, the accepted membership is cleared before retry can skip it. If recording pending metadata or clearing stale accepted metadata fails, current authority is not advanced and the failure surfaces in the typed error; retry probes durably recorded accepted peers before trusting them.

## Key Files

- `meerkat-core/src/agent.rs` - Main agent execution loop
- `meerkat-core/src/agent/compact.rs` - Compaction flow (wired into agent loop)
- `meerkat-core/src/state.rs` - LoopState state machine
- `meerkat-core/src/types.rs` - Core types (Message, Session, ToolCall, etc.)
- `meerkat-core/src/service/mod.rs` - SessionService trait, SessionError
- `meerkat-core/src/compact.rs` - Compactor trait, CompactionConfig, CompactionCurator (host-supplied summary producer; substitutes the compaction LLM call)
- `meerkat-core/src/completion_feed.rs` - CompletionFeed trait, CompletionEntry, CompletionSeq
- `meerkat-core/src/memory.rs` - MemoryStore trait (index/search + drop_scope/enumerate_scoped lifecycle APIs)
- `meerkat-sqlite/src/{profile,ledger,fence,json_column,error}.rs` - Shared SQLite mechanics (named connection profiles, meerkat_schema ledger + pinned protocol, per-op fence guards)
- `meerkat-core/src/storage_layout.rs` - StorageLayout path authority (+ realm-id-first dual-root resolution in runtime_bootstrap.rs)
- `meerkat-core/src/storage_durability.rs` - DurabilityClass/DurabilityDeclaration vocabulary (fail-closed durable slots)
- `meerkat-core/src/storage_diagnostics.rs` - StorageDiagnosis/StorageMigrator diagnose seam (doctor vocabulary)
- `meerkat-store/src/doctor.rs` - Read-only disk diagnosis behind `rkat storage doctor`
- `meerkat/src/storage_provider.rs` - RealmStorageProvider seam + DiskStorageProvider (facade composes PersistenceBundle)
- `meerkat-core/src/tool_execution_policy.rs` - ToolExecutionPolicy (sealed resolved form of ops::ToolAccessPolicy) + ExecutionPolicyGatedDispatcher (list-preserving call-level gate; deny = ordinary access_denied tool error)
- `meerkat-core/src/runtime_epoch.rs` - RuntimeEpochId, SessionRuntimeBindings, RuntimeBuildMode, EpochCursorState
- `meerkat-anthropic/src/client.rs` - Anthropic streaming implementation (meerkat-client is a re-export shim)
- `meerkat-session/src/ephemeral.rs` - EphemeralSessionService (in-memory session lifecycle)
- `meerkat-session/src/compactor.rs` - DefaultCompactor implementation
- `meerkat-session/src/event_store.rs` - EventStore trait
- `meerkat-session/src/projector.rs` - SessionProjector (materializes .rkat/ files)
- `meerkat-memory/src/simple.rs` - SimpleMemoryStore implementation
- `meerkat-models/src/catalog.rs` - Curated model catalog data (single source of truth for defaults/allowlists; `canonical()` ModelCatalog)
- `meerkat-models/src/capabilities/` - Per-provider model capability rows
- `meerkat-core/src/model_profile/mod.rs` - Model profile vocabulary + ModelCatalog mechanics (capability projection, param schemas; zero provider data)
- `meerkat-mcp/src/router.rs` - MCP tool routing
- `meerkat-runtime/src/ops_lifecycle.rs` - RuntimeOpsLifecycleRegistry, PersistedOpsSnapshot, persistence channel
- `meerkat-runtime/src/meerkat_machine/` - MeerkatMachine module (mod.rs, composition.rs, dispatch_*, dsl_*), prepare_bindings(), recover_or_create_ops_state()
- `meerkat-cli/src/main.rs` - CLI entry point
- `meerkat/src/factory.rs` - AgentFactory, DynAgent, AgentBuildConfig (consolidated agent construction)
- `meerkat/src/service_factory.rs` - FactoryAgentBuilder, FactoryAgent, build_ephemeral_service
- `meerkat-rpc/src/session_runtime.rs` - SessionRuntime (stateful agent manager)
- `meerkat-rpc/src/router.rs` - JSON-RPC method dispatch
- `meerkat-rpc/src/server.rs` - RPC server main loop
- `meerkat-rpc/src/handlers/mcp.rs` - Live MCP controls (mcp/add, mcp/remove, mcp/reload)
- `meerkat-core/src/tool_scope.rs` - Runtime tool visibility control
- `meerkat-contracts/src/wire/supervisor_bridge.rs` - Supervisor bridge protocol types (BridgeCommand, BridgeReply, payloads)
- `meerkat-mob/src/runtime/bridge.rs` - MobMemberRuntimeBridge trait (mob-owned protocol boundary)
- `meerkat-mob/src/runtime/bridge_protocol.rs` - Re-exports of bridge protocol types from contracts
- `meerkat-mob/src/runtime/local_bridge.rs` - LocalMobRuntimeBridge (in-process MeerkatMachine wrapper)
- `meerkat-mob/src/runtime/supervisor_bridge.rs` - MobSupervisorBridge (comms transport for remote commands)
- `meerkat-mob/src/storage.rs` - MobStorage bundle (SQLite persistent, in-memory)
- `meerkat-mob/src/runtime/flow_frame_engine.rs` - Frame-based flow execution (repeat_until loops with MobMachine-owned feedback)
- `meerkat-mob-mcp/src/agent_tools.rs` - Agent-facing delegation tools (delegate, mob_create, mob_spawn_member, mob_wire, mob_unwire, etc.)
- `meerkat-mob/src/backend.rs` - MobBackendKind and RuntimeBinding (identity-first mob binding)
- `meerkat-mob-pack/src/lib.rs` - Mobpack archive format, signing, trust
- `meerkat-schedule/src/service.rs` - ScheduleService CRUD + occurrence planning
- `meerkat-schedule/src/driver.rs` - ScheduleDriver tick loop + delivery
- `meerkat-schedule/src/machines/` - Schedule and occurrence lifecycle machines (schedule_lifecycle.rs, occurrence_lifecycle.rs)
- `meerkat-schedule/src/store.rs` - ScheduleStore trait + MemoryScheduleStore
- `meerkat-schedule/src/tools.rs` - Agent-facing schedule tools
- `meerkat-schedule/src/runnable.rs` - Host-runnable targets (ScheduleRunnableHost trait, HostRunnableRegistry, HostRunnableInvocation)
- `meerkat/src/surface/schedule_host.rs` - Runtime-backed schedule delivery surface (SharedScheduleTargetAdapter::with_runnable_host wires host runnables)
- `meerkat-web-runtime/src/lib.rs` - WASM browser deployment (wasm_bindgen exports)
- `sdks/web/src/runtime.ts` - @rkat/web MeerkatRuntime class (browser SDK entry point)
- `sdks/web/src/mob.ts` - @rkat/web Mob class (mob lifecycle wrapper)
- `sdks/web/src/session.ts` - @rkat/web Session class (direct session wrapper)

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

**`make ci`** combines documentation, formatting, locks, generated-contract
freshness, authority/governance checks, lint, test and feature-matrix lanes,
release packaging, and dependency audit. It also includes
`verify-fixture-mint-generator`, `check-rust-release-packaging-contract`,
`protocol-check-drift`, and `semver-breaks-selftest`. This is a non-exhaustive
summary; the `ci` target in [Makefile](Makefile) owns the complete prerequisite
list.

`rmat-audit` runs the typed governance gates: `xtask effect-authority`, `xtask ownership-ledger --check-drift`, and `xtask rmat-audit --strict` (RMAT read-seam enforcement is the `ForbiddenShellAuthorityReads` AST rule). The bridge gate is `xtask bridge-classifier` (`scripts/pre-push-bridge-no-responsestatus.sh` is a thin wrapper). The old `scripts/audit-effect-authority.sh` is deleted.

### GitHub Workflows

**CI** (`.github/workflows/ci.yml`) runs on pushes to `main`, PRs, and
manual dispatch (a branch head runs once, via its PR). It is Cargo-only on
GitHub-hosted runners and sized to a 20-minute push-to-terminal budget:
- `changes` classifies the diff with `scripts/ci-cargo-lanes.mjs` (fail
  closed: every Rust-relevant change yields lanes; unmapped Rust paths, a
  missing base, or global build configuration escalate to the workspace).
- `fmt-governance` (always): fmt, docs-check, semver self-test, version
  parity, lock consistency, `make ci-lanes-selftest`.
- `ratchets`: generated-contract freshness when contract paths changed;
  `machine-check-drift`/`protocol-check-drift` when machine authority changed.
- `clippy` and `unit`: one lane per shard of the directly changed packages
  (`clippy --no-deps --all-targets --all-features -D warnings`,
  `nextest --lib --bins --profile ci-pr`, the PR lane's named profile,
  identical to `fast`).
- `closure-check`: `cargo check --all-targets --all-features` over the
  reverse-dependency closure of the changed packages.
- `wasm-check` and `sdk-host` when their inputs changed.
- `gate` (`CI gate`, the only required context): fail-closed aggregate,
  1200-second budget from run creation, schema-4 attestation (backend
  `github-hosted-cargo`) on successful `main` pushes. It runs under
  `!cancelled()` so superseded runs surface as cancelled.

Integration-fast, e2e-fast, the dense Mob topology stress, bounded TLC, the
feature matrices, audit, the SDK suites, and the whole BuildBuddy/Bazel graph
do not run in PR CI. `cargo.yml` remains a separate reusable/manually
dispatchable Cargo workflow with its own `Cargo lane gate`; `ci.yml` does not
call it. Local Make commands still default to Cargo.

**Nightly** (`.github/workflows/nightly.yml`, cron + dispatch) owns everything
PR CI does not: `workspace-unit` (`make test-unit`), `workspace-int`
(`make test-int`), `e2e-fast`, `dense-topology` (`mob-dense-topology.yml`),
`machine-verify` (bounded TLC), `sdk-host`, `gcp-buildbuddy`
(`buildbuddy.yml` in `full-fresh` mode: the whole Bazel graph), plus the
existing `lint` (clippy `--all-targets`), `lint-feature-matrix`,
`test-feature-matrix`, `test-minimal`, `test-surface-modularity`,
`e2e-system`, `test-sdk-web`, `wasm-contract`, `check-rust-release-packaging`,
and cargo-deny sweep.

The release tag path requires successful exact-main CI (schema-4 attestation)
and then runs the full BuildBuddy graph (`release_validate_buildbuddy_full`)
as its validation gate; Cargo validation remains the manual-dispatch fallback.
The owner-selected BuildBuddy release backend (remote.buildbuddy.io) covers
Linux/macOS binaries. Windows release binaries are cross-compiled with
cargo-xwin (clang-cl, lld-link, the Windows SDK) on a GitHub-hosted Ubuntu
runner and then executed for verification on a windows-latest runner.

**Release semver readiness** (`.github/workflows/release-semver-readiness.yml`)
is a separate workflow triggered by `Cargo.toml`/`CHANGELOG.md` changes on
`main` pushes and PRs, or by manual dispatch. For unpublished candidate
versions, it measures declared breaks and uploads exact-tree, exact-version
evidence with 30-day configured retention. Only its successful `main`-push
artifact, `meerkat-semver-attestation-main-<tree_sha>`, qualifies for the normal
release semver gate; PR and manual artifacts are previews.

**Release** (`.github/workflows/release.yml`) — runs on `v*` tag push or manual dispatch:

| Job | Trigger | What it does |
|-----|---------|-------------|
| `require_ci_green` | Always | Requires successful exact-main CI for the release commit; tag runs also verify its retained exact-tree attestation (schema 4, backend `github-hosted-cargo`; legacy schemas accepted) |
| `release_validate_buildbuddy_full` | Tag runs | Runs the whole BuildBuddy/Bazel graph (`buildbuddy.yml`, `full-fresh`) on the release tree |
| `release_validate_cargo` / `release_validate_buildbuddy` | Eligible manual dispatches only | Validate release state through the selected lane; tag runs reuse exact-tree CI and skip both jobs |
| `release_validate_gate` | Tags and full/package dispatches | On tags require the full BuildBuddy graph plus exact-tree CI; on manual dispatches accept the selected validation lane; Web-only and asset-only recovery skip it |
| `release_semver_gate` | Tags and full/package dispatches | After `require_ci_green`, verifies unexpired exact-tree, exact-version main-push readiness evidence on the normal tag/explicit-tag path; narrower manual measurement/recovery exceptions are described in the release guide |
| `build_binaries` / `build_binaries_buildbuddy` / `build_binaries_windows_cross` / `verify_windows_binaries` / `build_binaries_gate` | Tags or manual asset recovery | The selected BuildBuddy or GitHub-hosted lane builds Linux/macOS (4 targets); `build_binaries_windows_cross` cross-compiles Windows from Linux with cargo-xwin and `verify_windows_binaries` runs the result on windows-latest; each target packages 4 binaries (`rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`); the gate requires the selected lanes plus both Windows jobs |
| `build_web_sdk_package` | Tags or package/Web recovery without a reused artifact | Builds the `@rkat/web` package artifact |
| `publish_github_release` | Tags or manual asset recovery | Downloads artifacts, generates `checksums.sha256` + `index.json`, publishes or repairs the GitHub Release |
| `update_homebrew` | After GitHub release or asset recovery | Updates the Homebrew tap formula |
| `publish_semver_baseline` | After GitHub release or asset recovery | Generates rustdoc JSON for every publishable library crate on the release tag and attaches `semver-rustdoc-<version>.tar.zst`, the baseline the next release's semver gate compares against |
| `publish_registries` | Tags or manual `publish_release_packages=true` | Publishes 43 Rust crates → crates.io, Python SDK → PyPI, TypeScript SDK → npm |
| `publish_web_sdk` | Tags or manual package/Web recovery | Publishes `@rkat/web` → npm |

**Build matrix:**

| Platform | Target |
|----------|--------|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` |
| macOS ARM64 | `aarch64-apple-darwin` |
| macOS x86_64 | `x86_64-apple-darwin` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

**Manual recovery through the release facade:**
```bash
# Inspect the exact command without dispatching
RELEASE_WORKFLOW_DRY_RUN=true VERSION=v0.8.24 make release-assets
RELEASE_WORKFLOW_DRY_RUN=true VERSION=v0.8.24 make release-packages
RELEASE_WORKFLOW_DRY_RUN=true VERSION=v0.8.24 make release-web-sdk

# Dispatch registry validation without uploading
REGISTRY_DRY_RUN=true VERSION=v0.8.24 make release-packages

# Dispatch one narrow recovery after inspection
VERSION=v0.8.24 make release-assets
```

`make release-workflow` is the full release mode, not a recovery shortcut.
Recovery modes run the current workflow definition from `main` while binding
artifacts and package metadata to the selected existing tag.

### Pre-commit Hooks

Installed via `make install-hooks`. Two stages:

**On commit** (`pre-commit`):
- `repo-cargo fmt --all`
- `node scripts/generate-bazel-rust-builds.mjs` (regenerates Bazel rust build metadata)
- `scripts/sync-meerkat-dogma-skill-docs.sh` (syncs dogma docs into the skill)

**On push** (`pre-push`):
- Secret detection (gitleaks)
- Trailing whitespace, end-of-file, YAML/TOML validation, merge conflict check, large file check
- `repo-cargo fmt --all -- --check`
- `scripts/pre-push-clippy.sh` (clippy on changed crates only with `--all-targets`; falls back to full workspace when root `Cargo.toml`/`Cargo.lock` changes)
- `scripts/pre-push-machines.sh` (machine codegen drift verify)
- `scripts/pre-push-audit-generated-headers.sh`
- `scripts/pre-push-bridge-no-responsestatus.sh` (thin wrapper over `xtask bridge-classifier`)
- `scripts/pre-push-bazel-locks.sh` (generated BUILD freshness, offline MODULE.bazel.lock recorded-input check, `bb mod deps --lockfile_mode=error` when the pinned CLI is present; `--require-bb` makes that last gate mandatory and the release preflight passes it)
- `scripts/test-lock-consistency-gate.sh`, `scripts/test-bazel-module-lock-gate.sh`, `scripts/test-crate-enumeration-gate.sh`, `scripts/test-release-doctor-workflow-contract.sh` (contract tests: each new release-infra gate must still fail on the defect it was written for; the last one also proves the release doctor's `release.yml` assertions survive rewording)
- `scripts/pre-push-unit.sh` (deterministic Cargo/nextest gate: when fresh execution is needed, runs workspace unit, integration-fast, HeadCanonical cold-restart, and `e2e-fast` lanes; serializes identical source evidence and retries a timed-out lane once. `MEERKAT_BUILDBUDDY` does not switch this hook's backend)
- `scripts/pre-push-prune-lanes.sh` (runs from the dispatcher only after a PASSED gate: keeps at most `MEERKAT_PRE_PUSH_KEEP_LANES` (default 2) `pre-push-<hash>` hook worktrees and Cargo target lanes per repo, never touching the current lane, a lane whose dispatcher lock is live, a lane referenced by any live process, a lane with any file activity inside `MEERKAT_PRE_PUSH_LANE_IDLE_SECS` (default 21600), or any non-lane name; logs one `kept:`/`pruned:` line per lane with its reason; `MEERKAT_PRE_PUSH_KEEP_LANES=all` disables it)

The inner deterministic-test cache uses a source fingerprint that excludes
root `Cargo.lock` and `MODULE.bazel.lock`. On a lock-only cache hit, the current
lock graph has been compiled by pre-push Clippy but is not re-tested locally;
the reused tests ran against the prior lock graph, and CI remains
authoritative. Fail-closed `release-projection-only` and
`pre-push-harness-only` classifiers can also reuse existing parent source-test
evidence. This is separate from `pre-push-dispatch.sh`'s complete-hook success
stamp, which is keyed to the entire pushed Git tree.

**Manual local preflight**:
- `pre-commit run --hook-stage manual agent-check-changed` (runs `scripts/agent-gate --staged`)
- `scripts/test-changed-crates.sh`

### Version Parity Contract

Six files must agree on the same version:

| File | Field |
|------|-------|
| `Cargo.toml` (workspace root) | `workspace.package.version` — **source of truth** |
| `meerkat-contracts/src/version.rs` | `ContractVersion::CURRENT` |
| `sdks/python/pyproject.toml` | `version` |
| `sdks/typescript/package.json` | `version` |
| `sdks/web/package.json` | `version` |
| `artifacts/schemas/version.json` | `contract_version` |

Additionally, all internal crate dependencies in `Cargo.toml` (44 path deps) must match the workspace version.

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

**`make verify-schema-freshness`** compares `artifacts/schemas/` in the current
working tree with schemas freshly emitted into a separate output directory,
normalizing JSON before comparison. It does not certify the staged files or
Git `HEAD`. Change the typed owner, run `make regen-schemas`, and commit the
updated generated artifacts.

### Releasing

```bash
make release-preflight       # Full CI + schema freshness + changelog check
./scripts/repo-cargo release patch  # Inspect cargo-release plan only
./scripts/repo-cargo release patch --execute  # Bump, commit, tag, push
```

The combined bump/tag/push command does not itself establish release
readiness. Prefer preparing the hook-generated release tree on `main` and
waiting for both ordinary CI and **Release semver readiness** before
cargo-release publishes the tag. The enforced boundary is the
`release_semver_gate` artifact lookup after `require_ci_green`: the matching
main-push semver evidence must exist and be unexpired then. A combined push
can succeed if readiness finishes before that lookup, but otherwise races it.
Local preflight and PR/manual readiness previews are not substitutes. Keep
version/changelog generation hook-owned; see the
[release guide](docs/guides/cd-and-distribution.md#release-workflow) for the
separate manual measurement and completed-measurement recovery paths.

**What `./scripts/repo-cargo release patch --execute` does:**

1. Bumps `workspace.package.version` in `Cargo.toml`
2. Fires `scripts/release-hook.sh` (pre-release hook, sentinel-guarded to run once):
   - `scripts/bump-sdk-versions.sh` - updates Python, TypeScript, and Web SDK versions
   - `stamp-docs-contract-version.sh` - updates README, public docs, and platform skill versions
   - `stamp-changelog-release.py` - stamps the release section and comparison links
   - Updates `ContractVersion::CURRENT`
   - Regenerates schemas, SDK types/wrappers, BuildBuddy BUILD files, and the Bazel module lock
   - Verifies version parity, RPC surface alignment, and SDK wrapper freshness
   - Stages all release projections for the version commit
3. Creates release commit (`chore: release v{version}`)
4. Tags as `v{version}`
5. Pushes commit + tag to remote → triggers release workflow

**Cargo.toml release config** (`workspace.metadata.release`):
- `tag-name = "v{{version}}"`, `push = true`, `publish = false` (registry publish handled by GitHub Actions)

### Dry-run Publishing

```bash
make publish-dry-run              # Parallel dry-run for all 43 publishable Rust crates
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

The canonical publish order lives in `scripts/release-rust-crates.sh` (43 crates, dependency order):
`meerkat-sqlite` → `meerkat-machine-derive` → `meerkat-machine-dsl-core` → `meerkat-agent-build-authority` → `meerkat-core` → `meerkat-atif` → `meerkat-store-conformance` → `meerkat-models` → `meerkat-capabilities` → `meerkat-machine-dsl` → `meerkat-machine-schema` → `meerkat-machine-kernels` → `meerkat-skills` → `meerkat-schedule` → `meerkat-jobs` → `meerkat-workgraph` → `meerkat-contracts` → `meerkat-store` → `meerkat-llm-core` → `meerkat-live` → `meerkat-auth-core` → `meerkat-memory` → `meerkat-mcp` → `meerkat-hooks` → `meerkat-comms` → `meerkat-runtime` → `meerkat-copilot` → `meerkat-anthropic` → `meerkat-openai` → `meerkat-gemini` → `meerkat-providers` → `meerkat-tools` → `meerkat-session` → `meerkat-client` → `meerkat` → `meerkat-mob` → `meerkat-mob-adaptive` → `meerkat-mob-mcp` → `meerkat-mob-pack` → `meerkat-mcp-server` → `meerkat-rpc` → `meerkat-rest` → `rkat`

### Key Rules for AI Agents

- **Never bump `workspace.package.version` without also running `scripts/bump-sdk-versions.sh`** — the CI gate will catch drift
- **Never change types in `meerkat-contracts` without running `make regen-schemas`** — schema artifacts and SDK types will be stale
- **Always run `make test` or the narrower `make agent-gate` before committing** — set `MEERKAT_BUILDBUDDY=1` when BuildBuddy is available
- **`ContractVersion::CURRENT` must equal `workspace.package.version`** — they are lock-stepped
- **Never change `Cargo.lock` without refreshing `MODULE.bazel.lock`** - the lock is a crate_universe extension input; `make buildbuddy-lock-update` regenerates it, and `make verify-bazel-locks` proves it
- **Never hand-maintain a second list of workspace crates** - `scripts/release-rust-crates.sh` is the one hand-ordered enumeration; the patch config derives from it and `make check-rust-release-config` fails when the documented order, count, or patch map disagrees
- **Never write a bare crate count in this file that nothing derives** - `make check-rust-release-config` rejects any `N crates` / `N path deps` claim it cannot bind to a computed quantity. Register the new claim in `documented_count_claim_errors` (`scripts/check_rust_release_packaging.py`) alongside the artifact that owns the number, or write it as an approximation with a leading `~`. Release crate count and internal path-dep count are different facts with different owners; they are not interchangeable even when they happen to be equal
- **Never size a thread stack in a host binary or bypass the budget with `#[tokio::main]`** - every host runs through `meerkat_runtime::host_stack::run_host` on the one documented 8 MiB default budget (`HOST_WORKER_STACK_BUDGET`); a deep frame is fixed with `meerkat_runtime::stack_relief`, not a bigger stack. The debug canary `rpc_dispatch_path_fits_debug_worker_stack_budget` (4 MiB) and nightly `make stack-budget-release` (1 MiB release) pin the numbers
- **Use cargo-release for releases** - the configured `./scripts/repo-cargo release patch --execute` flow handles version projections, changelog stamping, schema/codegen refresh, BuildBuddy metadata, and parity verification through the release hook. Never manually bump versions or create tags; also satisfy the exact-tree CI and semver-readiness prerequisites above

## Testing with Multiple Providers

When running tests or demos that involve multiple LLM providers/models, use these model names:

| Provider | Model Name |
|----------|------------|
| OpenAI | `gpt-5.6-sol` or `gpt-5.6-terra` or `gpt-5.6-luna` or `gpt-5.6` or `gpt-5.5` or `gpt-5.5-pro` or `gpt-5.4` or `gpt-5.4-mini` or `gpt-5.3-codex` |
| Gemini | `gemini-3.5-flash` or `gemini-3.1-pro-preview` or `gemini-3.1-flash-lite-preview` |
| Anthropic | `claude-opus-5` or `claude-fable-5` or `claude-opus-4-8` or `claude-sonnet-4-6` or `claude-sonnet-4-5` |

These are catalog text-model ids (`meerkat-models` is the source of truth; `rkat models` prints the live list); models outside the catalog require a config `[models.<id>]` entry.
GPT-5.6 is a limited preview; use `gpt-5.5` for generally runnable OpenAI
examples unless the relevant API organization or Codex workspace has preview
access.

Do NOT use older model names like `gpt-4o-mini`, `gemini-2.0-flash`, or `claude-3-7-sonnet-20250219`.

## Design Philosophy

See `docs/reference/design-philosophy.mdx` for the full treatment with code examples.

### Architectural Principles

- **Infrastructure, not application** — the agent loop is a composable primitive with no opinions about prompts, tools, or output
- **Trait contracts own the architecture** - `meerkat-core` defines foundational contracts (`AgentLlmClient`, `AgentToolDispatcher`, `AgentSessionStore`, `SessionService`, `Compactor`, `MemoryStore`, `HookEngine`, `SkillEngine`/`SkillSource`); feature crates define their domain stores and implementations
- **Runtime-backed surfaces are interchangeable skins** - CLI, REST, RPC, and MCP Server route through `SessionService` to `AgentFactory::build_agent()`; embedded Rust and WASM may deliberately compose standalone agents directly
- **Composition over configuration** — optional components (`CommsRuntime`, `HookEngine`, `Compactor`, `MemoryStore`) are `Option<Arc<dyn Trait>>`, not feature-flagged defaults
- **Sessions are first-class, persistence is optional** - `EphemeralSessionService` and the checkpoint-free, store-backed `PersistentSessionService` share the same `SessionService` trait; durable event projection is separate optional derived audit/replay state
- **Errors separate mechanism from policy** — typed three-tier errors (`ToolError` → `AgentError` → `SessionError`) with stable `error_code()` for wire formats; the loop retries, callers decide to resume or abort
- **Wire types ≠ domain types** — `meerkat-contracts` owns wire format and feeds SDK codegen; domain types in `meerkat-core` are richer and version-locked
- **Configuration is layered and declarative** - realm config parent chains fold root-first with child-wins semantics, then env key fallback and per-request `SessionBuildOptions` apply; state never inherits
- **Testing is a design constraint** - isolate pure domain logic where possible and test core's bounded native config/path/lock/fence I/O explicitly; the repo standardizes on named lanes: `cargo unit`, `cargo int`, `cargo e2e-fast`, `cargo e2e-system`, `cargo e2e-live`, `cargo e2e-smoke`, and `cargo e2e-models`

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
