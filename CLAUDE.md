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
  [worker-stack diagnosis](docs/guides/deploying.mdx#worker-stack-budget) and
  `RKAT_WORKER_THREADS` for the
  [worker count](docs/guides/deploying.mdx#worker-threads).

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

**Machine authority rule:** For canonical machine-owned domains, semantic state mutation must flow through generated machine authority, not handwritten reducers. See `docs/reference/machine-authority.mdx` (canonical registry: `canonical_machine_schemas()` in `crates/meerkat-machine-schema/src/catalog/mod.rs`).

**Agent construction:** All surfaces use `AgentFactory::build_agent()` for centralized prompt assembly, provider resolution, tool dispatcher setup, comms wiring, and hook resolution. Zero `AgentBuilder::new()` calls in surface crates.

**Runtime build mode:** All runtime-backed surfaces, including Web/WASM, use `MeerkatMachine::prepare_bindings(session_id)` to obtain `SessionRuntimeBindings`, then pass `RuntimeBuildMode::SessionOwned(bindings)` via `SessionBuildOptions.runtime_build_mode`. Explicit standalone Rust embeddings and tests may use `RuntimeBuildMode::StandaloneEphemeral` (the default).

**Session lifecycle:** Runtime-backed product surfaces (CLI, REST, MCP Server, JSON-RPC, Web/WASM, and their process-backed Python/TypeScript clients) route through `SessionService` for create/turn/interrupt/read/list/archive. `FactoryAgentBuilder` bridges `AgentFactory` into the `SessionAgentBuilder` trait. Explicit standalone Rust embeddings can construct agents directly through `AgentFactory`; per-request service build data is passed in-band via `CreateSessionRequest.build` / `SessionBuildOptions`. Browser limits are typed capability exclusions on this shared runtime path.

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

Config stored in `.rkat/mcp.toml` (project), `~/.rkat/mcp.toml` (user), or the
selected realm's own config (`--scope realm`, `[[tools.mcp_servers]]`, inherited
by child realms). A realm server and a file server with the same name must be
defined identically. Realm servers are literal: `${VAR}` expands only in
`mcp.toml`, and realm writes refuse it.

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

- `crates/meerkat-core/src/agent.rs` - Main agent execution loop
- `crates/meerkat-core/src/agent/compact.rs` - Compaction flow (wired into agent loop)
- `crates/meerkat-core/src/state.rs` - LoopState state machine
- `crates/meerkat-core/src/types.rs` - Core types (Message, Session, ToolCall, etc.)
- `crates/meerkat-core/src/service/mod.rs` - SessionService trait, SessionError
- `crates/meerkat-core/src/compact.rs` - Compactor trait, CompactionConfig, CompactionCurator (host-supplied summary producer; substitutes the compaction LLM call)
- `crates/meerkat-core/src/completion_feed.rs` - CompletionFeed trait, CompletionEntry, CompletionSeq
- `crates/meerkat-core/src/memory.rs` - MemoryStore trait (index/search + drop_scope/enumerate_scoped lifecycle APIs)
- `crates/meerkat-sqlite/src/{profile,ledger,fence,json_column,error}.rs` - Shared SQLite mechanics (named connection profiles, meerkat_schema ledger + pinned protocol, per-op fence guards)
- `crates/meerkat-core/src/storage_layout.rs` - StorageLayout path authority (+ realm-id-first dual-root resolution in runtime_bootstrap.rs)
- `crates/meerkat-core/src/storage_durability.rs` - DurabilityClass/DurabilityDeclaration vocabulary (fail-closed durable slots)
- `crates/meerkat-core/src/storage_diagnostics.rs` - StorageDiagnosis/StorageMigrator diagnose seam (doctor vocabulary)
- `crates/meerkat-store/src/doctor.rs` - Read-only disk diagnosis behind `rkat storage doctor`
- `crates/meerkat/src/storage_provider.rs` - RealmStorageProvider seam + DiskStorageProvider (facade composes PersistenceBundle)
- `crates/meerkat-core/src/tool_execution_policy.rs` - ToolExecutionPolicy (sealed resolved form of ops::ToolAccessPolicy) + ExecutionPolicyGatedDispatcher (list-preserving call-level gate; deny = ordinary access_denied tool error)
- `crates/meerkat-core/src/runtime_epoch.rs` - RuntimeEpochId, SessionRuntimeBindings, RuntimeBuildMode, EpochCursorState
- `crates/meerkat-anthropic/src/client.rs` - Anthropic streaming implementation (meerkat-client is a re-export shim)
- `crates/meerkat-session/src/ephemeral.rs` - EphemeralSessionService (in-memory session lifecycle)
- `crates/meerkat-session/src/compactor.rs` - DefaultCompactor implementation
- `crates/meerkat-session/src/event_store.rs` - EventStore trait
- `crates/meerkat-session/src/projector.rs` - SessionProjector (materializes .rkat/ files)
- `crates/meerkat-memory/src/simple.rs` - SimpleMemoryStore implementation
- `crates/meerkat-models/src/catalog.rs` - Curated model catalog data (single source of truth for defaults/allowlists; `canonical()` ModelCatalog)
- `crates/meerkat-models/src/capabilities/` - Per-provider model capability rows
- `crates/meerkat-core/src/model_profile/mod.rs` - Model profile vocabulary + ModelCatalog mechanics (capability projection, param schemas; zero provider data)
- `crates/meerkat-mcp/src/router.rs` - MCP tool routing
- `crates/meerkat-runtime/src/ops_lifecycle.rs` - RuntimeOpsLifecycleRegistry, PersistedOpsSnapshot, persistence channel
- `crates/meerkat-runtime/src/meerkat_machine/` - MeerkatMachine module (mod.rs, composition.rs, dispatch_*, dsl_*), prepare_bindings(), recover_or_create_ops_state()
- `crates/meerkat-cli/src/main.rs` - CLI entry point
- `crates/meerkat/src/factory.rs` - AgentFactory, DynAgent, AgentBuildConfig (consolidated agent construction)
- `crates/meerkat/src/service_factory.rs` - FactoryAgentBuilder, FactoryAgent, build_ephemeral_service
- `crates/meerkat-rpc/src/session_runtime.rs` - SessionRuntime (stateful agent manager)
- `crates/meerkat-rpc/src/router.rs` - JSON-RPC method dispatch
- `crates/meerkat-rpc/src/server.rs` - RPC server main loop
- `crates/meerkat-rpc/src/handlers/mcp.rs` - Live MCP controls (mcp/add, mcp/remove, mcp/reload)
- `crates/meerkat-core/src/tool_scope.rs` - Runtime tool visibility control
- `crates/meerkat-contracts/src/wire/supervisor_bridge.rs` - Supervisor bridge protocol types (BridgeCommand, BridgeReply, payloads)
- `crates/meerkat-mob/src/runtime/bridge.rs` - MobMemberRuntimeBridge trait (mob-owned protocol boundary)
- `crates/meerkat-mob/src/runtime/bridge_protocol.rs` - Re-exports of bridge protocol types from contracts
- `crates/meerkat-mob/src/runtime/local_bridge.rs` - LocalMobRuntimeBridge (in-process MeerkatMachine wrapper)
- `crates/meerkat-mob/src/runtime/supervisor_bridge.rs` - MobSupervisorBridge (comms transport for remote commands)
- `crates/meerkat-mob/src/storage.rs` - MobStorage bundle (SQLite persistent, in-memory)
- `crates/meerkat-mob/src/runtime/flow_frame_engine.rs` - Frame-based flow execution (repeat_until loops with MobMachine-owned feedback)
- `crates/meerkat-mob-mcp/src/agent_tools.rs` - Agent-facing delegation tools (delegate, mob_create, mob_spawn_member, mob_wire, mob_unwire, etc.)
- `crates/meerkat-mob/src/backend.rs` - MobBackendKind and RuntimeBinding (identity-first mob binding)
- `crates/meerkat-mob-pack/src/lib.rs` - Mobpack archive format, signing, trust
- `crates/meerkat-schedule/src/service.rs` - ScheduleService CRUD + occurrence planning
- `crates/meerkat-schedule/src/driver.rs` - ScheduleDriver tick loop + delivery
- `crates/meerkat-schedule/src/machines/` - Schedule and occurrence lifecycle machines (schedule_lifecycle.rs, occurrence_lifecycle.rs)
- `crates/meerkat-schedule/src/store.rs` - ScheduleStore trait + MemoryScheduleStore
- `crates/meerkat-schedule/src/tools.rs` - Agent-facing schedule tools
- `crates/meerkat-schedule/src/runnable.rs` - Host-runnable targets (ScheduleRunnableHost trait, HostRunnableRegistry, HostRunnableInvocation)
- `crates/meerkat/src/surface/schedule_host.rs` - Runtime-backed schedule delivery surface (SharedScheduleTargetAdapter::with_runnable_host wires host runnables)
- `crates/meerkat-web-runtime/src/lib.rs` - WASM browser deployment (wasm_bindgen exports)
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

**CI** (`.github/workflows/ci.yml`) runs on pushes to `main` and to
`release/**` integration branches, PRs, and manual dispatch (any other branch
head runs once, via its PR; a release-branch push runs the same push-only
lanes as `main` and writes the exact-tree attestation too, which a tag trusts
only from `main` or from the release branch of its own version, a
[maintenance release](docs/guides/cd-and-distribution.md#maintenance-releases)).
It is Cargo-only on
GitHub-hosted runners. A pull request runs what fits the 20-minute cap from
the start of the pre-push hook to a green `CI gate`; the push runs every lane,
and its CI must be green before a release tag (`require_ci_green`):
- `changes` classifies the diff with `scripts/ci-cargo-lanes.mjs --event
  <event>` (fail closed: every Rust-relevant change yields lanes; unmapped
  Rust paths, a missing base, or global build configuration escalate to the
  workspace). A push to `main` diffs against the newest commit before HEAD
  whose main CI run completed green with an attestation
  (`scripts/ci-push-base.mjs`), not the payload base, so a superseded,
  cancelled or failed main run's changes are planned again.
  `scripts/ci-push-plan-base.mjs`, the payload base's own copy (so a pushed
  commit cannot edit the decider for its own push; without that copy the
  push plans the whole workspace), runs the finder from HEAD's first parent
  only when that parent is the payload base (`github.event.before`), and
  accepts only a full sha of an ancestor of HEAD other than HEAD (the
  checkout's history, or GitHub's compare API where it is shallow); a push
  of several commits, a refused answer or no such commit plans the whole
  workspace. A `Cargo.lock` change that is the only global path is scoped:
  the package entries it adds, removes or changes are walked up the head
  lock's dependency lists to the members that resolve them, which become the
  changed packages; a header or other-table change, an unresolvable entry, or
  an unreadable lock plans the workspace. On a pull request the plan leaves
  every lane that compiles the meerkat-mob chain or sweeps the
  reverse-dependency closure to the push and names it (`deferred_to_push`,
  printed by the gate): the `closure-tests` rows, `closure-check`,
  `rustdoc`,
  `feature-check`, `governed-jsonl` and `mcp-confinement-positive` (unless
  it touches their own inputs or changes their definition), the
  integration suites whose package compiles meerkat-mob, the feature suites
  marked `pullRequest: false`, the mob-chain packages' test-target Clippy,
  and the whole Clippy of the chain heads rkat, meerkat-integration-tests and
  meerkat-rpc (`MOB_CHAIN_CLIPPY_HEADS`) whenever the change invalidates
  meerkat-mob's build (workspace mode, or a change to meerkat-mob or a crate
  its library compiles), listed as "mob chain invalidated": their lanes then
  compile the whole chain uncached. A pull request that leaves meerkat-mob's
  build alone lints them. The gate refuses a push plan that left anything to
  a later push, and the push lints every target of every changed package.
  A release projection is the one exception: when the diff from HEAD's
  first parent is only the forward patch projection `scripts/release-hook.sh`
  writes (`scripts/release-projection-only.mjs`, the pre-push hook's own
  fail-closed classifier: regular files only, strict UTF-8, stamped docs and
  CHANGELOG exactly the base's stampers' output, x.y.z to x.y.(z+1)) and that
  parent has a completed, successful exact-main CI run that planned the whole
  workspace with every lane `success` (`scripts/ci-release-evidence.mjs
  find`, run from the parent's own tree; a main push waits up to 40 minutes
  for a running parent run, a pull request does not), the plan is mode
  `release-projection`: `fmt-governance` (version parity, lock consistency,
  release packaging/config, docs-check, the semver self-test), the
  generation ratchets (schema/SDK freshness), the Bazel graph check and the
  readers of the changed files (their `closure-tests` rows, path-selected
  integration suites, and a `--lib --bins` unit row per reader package plus
  `meerkat-contracts`) run, and the Clippy, other unit, check, feature, wasm,
  SDK and example lanes reuse the parent's run. The finder takes the newest
  run of the parent only, and refuses reuse when any test, snapshot, fixture,
  SDK or Rust code line outside the projection pins the old or new version
  (`scripts/release-version-pins.mjs`). Without such evidence, or for any
  other byte change, the plan is the ordinary one. A commit whose message, or
  whose merged branch head's message, carries the trailer `CI-Plan: workspace`
  is planned as the whole workspace (without looking for evidence): the
  explicit way to give a release's parent that evidence.
- `fmt-governance` (always): fmt, docs-check, semver self-test, version
  parity, lock consistency, generated BUILD freshness
  (`node scripts/generate-bazel-rust-builds.mjs --check`),
  `make ci-lanes-selftest`,
  `make path-classifier-selftest`, the ripgrep tombstone scans
  (`legacy-surface-gate`, `session-control-gate`, `deprecated-backend-gate`;
  ripgrep is installed first, and each scan refuses to run without it), and
  the published crates' static standalone guard
  (`make published-crates-standalone-selftest
  check-published-crates-standalone-guard`: no packaged test reads outside
  its package or skips itself when a binary is missing, every workspace-only
  test file is left out in both the crate's `[package] exclude` and
  `ci/standalone-workspace-only.toml`, and a crate whose tests need features
  or a stack prints the exact `cargo test` command in its README).
- `ratchets`: generated-contract freshness when contract paths changed;
  `machine-check-drift`/`protocol-check-drift` when machine authority changed;
  `verify-machine-poster-coverage` when machine authority or a poster input
  (`scripts/machine-posters-changed`, a separate classifier so poster-only
  changes never start the machine lanes) changed.
- `governance-gates`: the typed governance gates (`make rmat-audit
  seam-inventory runtime-authority-bypass machine-authority-docs-gate`) in a
  job of their own, on the agent gate's governance paths: a machine-authority
  change or a dogma document (`governance_gates` in the plan,
  `GOVERNANCE_DOC_PATTERNS` in `scripts/ci-cargo-lanes.mjs`); the `CI gate`
  requires them on such a change.
- `tlc-audits`: on a machine-authority change, the seven hand-written TLC
  audit shards of the canonical TLC lane
  (`machine_verify_all_tlc_test.sh --part audits-a` through `audits-g`) in parallel,
  needing only a JDK and the TLC jar (`.github/actions/setup-tlc-ci`, the one
  TLC install for every lane: an immutable repository mirror of the tlaplus
  v1.8.0 jar pinned by sha256); the `CI gate` requires them on such a change. The `machine-verify --all` sweep is not in PR CI.
- `clippy`: one lane per shard of every directly changed package
  (`clippy --no-deps <targets> --all-features -D warnings`, every target
  except a pull request's meerkat-mob chain shards, which lint their lib and
  bin targets because their tests run on the push; a pull request that
  invalidates meerkat-mob's build leaves the Clippy of
  `MOB_CHAIN_CLIPPY_HEADS` to the push); a
  whole-workspace plan gives the heads of the long check chains fixed
  shards of their own (`CLIPPY_HEAD_SHARDS`) and packs the rest. Its
  `RUSTC_WRAPPER` is `scripts/ci-clippy-sccache-wrapper`, which sends the
  crates Clippy does not lint to sccache as plain rustc so they are cached;
  the lane's own packages still go through clippy-driver.
- `unit`: `nextest --lib --bins --profile ci-pr` (`fast` plus a 4-minute hung-test kill; a
  row names only the target kinds its packages have, so the binary-only
  rkat and surface-build-fixtures run `--bins`) for
  the changed packages outside the meerkat-mob compile chain; crates that
  compile `meerkat-mob` (mob, mob-adaptive, mob-mcp, mob-pack, rpc, rest,
  mcp-server, rkat, web-runtime, integration-tests, machine-codegen;
  computed from each unit lane's real build graph: the package's own deps
  of every kind, then normal and build deps only) defer their unit tests to
  `push: main` because their lanes need 17-22 min on 4 vCPU, meerkat-rpc's
  default-feature row (its MCP restart controls) included. xtask and
  machine-dsl-tests do not build mob and run in the pull request; xtask, whose
  tests run 5-7 minutes on 4 vCPU, gets a unit lane of its own
  (`UNIT_SOLO_PACKAGES`) so they do not stack on other crates' builds.
  meerkat-runtime's lane of its own runs as two rows
  (`PARTITIONED_UNIT_PACKAGES`): each builds the crate (10-11 minutes) and
  runs one `--partition hash:k/2` of its tests (7-8 minutes in one job).
- Feature-gated unit suites (`FEATURE_UNIT_SUITES` in
  `scripts/ci-cargo-lanes.mjs`): the unit lanes build default features only,
  so each suite adds a `nextest -p <package> --features <list>` row to
  `main-unit` on every Rust-relevant main push, and to a pull request's
  `unit` lane when that package changed outside the meerkat-mob chain
  (unless the suite is marked `pullRequest: false`: the facade's two GPT
  Live suites). A new test behind a non-default feature needs a suite, or no
  lane runs it. A push runs no `unit` lane: the unit rows it would run (the
  same shards and suites) run verbatim in `main-unit` (`unit_rows_on_main`).
- `integration`: the `tests/*.rs` binaries (`nextest -p <package> --test
  '*'`) of each `INTEGRATION_SUITES` entry in `scripts/ci-cargo-lanes.mjs`
  whose trigger packages or paths changed (meerkat-runtime;
  meerkat-machine-codegen, whose parity tests also trigger on meerkat-mob;
  xtask with machine-authority, also on any `.github/workflows/` or `sdks/`
  edit, because its tests pin the workflows and read the SDK manifests and
  generated types), on main pushes, and on PRs for the suites whose package
  does not compile meerkat-mob. The lane also runs the reader rows: the
  exact target that reads a changed non-Rust input
  (`scripts/ci-non-rust-readers.json`: docs, skills, CHANGELOG, specs,
  fixtures), with the inventory's features and a filter on the reading
  tests, where no other lane of the plan runs it (`reader_rows`; a reader
  in library code runs its lib and bins there, its test targets in the
  push's `closure-tests`, which alone run the nested-Cargo canaries). A pull
  request leaves the readers that compile meerkat-mob and the readers
  keyed only by the whole checkout to the push; the push's `closure-tests`
  run test targets with the features a workspace build resolves, so a test
  target that needs others (xtask's `machine-authority` contracts) gets a
  reader row there, and a push with no `main-unit` gets reader rows for
  lib, bin and example targets. A reader that needs live-provider
  credentials (`integration-real-tests`, `openai-live-e2e`) runs in no CI
  lane (`reader_not_run`, printed by the gate). In-crate documentation the
  crate compiles in (`include_str!` of
  its `skills/*/SKILL.md`) is a build input of that crate.
  The unit lanes run `--lib --bins` only; `closure-tests` runs the
  integration tests of every package in its test scope.
- `closure-tests` (push only): the deterministic workspace lanes of
  `scripts/pre-push-unit.sh` (the hook's cargo-test body, now a manual hook)
  as required rows
  for the test scope: the reverse-dependency closure of the changed packages
  (every member on a global change) plus the packages whose tests read a
  changed non-Rust input (`scripts/ci-non-rust-readers.json`, kept complete
  by `scripts/ci-non-rust-readers-guard.mjs` in `make ci-lanes-selftest`,
  which also fails a reader whose named test its features compile out).
  Rows (`closure_test_rows` in `scripts/ci-cargo-lanes.mjs`) select the whole
  workspace and name only their targets, so they compile the hook's own
  units: every `tests/*.rs` target the hook builds (by name, `ci-integration`
  profile, the fast selection), `e2e-fast`, and HeadCanonical with the
  hook's own package-scoped command, all on the hook's 8 MiB test-thread
  stack (the workspace floor of `.cargo/config.toml`). The lib tests get no
  row: one
  `--workspace --lib` row (filtered to the scope's packages) is planned only
  with `--lib-row-runner LABEL`, for a runner that builds it in time (a
  hosted 4-vCPU runner needs about 64 min), and the plan otherwise records
  the lane it leaves to `main-unit` (package-scoped) and nightly's workspace
  unit lane (the workspace feature set) (`hook_retained_lanes`). Each row lists and
  runs one shared selection and first checks that nextest selects exactly
  its planned binaries (`scripts/ci-verify-row-binaries.mjs`). The binary
  holding the nested-Cargo canaries (the `downstream-cargo-canary` test
  group in `.config/nextest.toml`) has rows of its own, one
  `--partition count:K/N` per canary, so the canaries run on separate
  runners; the coverage check credits it only when every partition is
  planned. The plan
  fails when `scripts/ci-hook-coverage.mjs` finds a hook binary in scope
  without a row (other than a lib binary the plan leaves to those unit
  lanes; a pull-request plan is checked against its push's rows): it
  reads the hook's lanes from `scripts/pre-push-unit.sh`, binds every line
  of the executing step that names a row input, the exported test-thread
  stack, the selection, nextest or the binary check (so none can reassign
  an input or the stack), and credits only whole-workspace rows.
- `closure-check` (push only): `cargo check --all-features` (lib and bin
  targets) over the reverse-dependency closure of the changed packages.
- `rustdoc` (push only): `cargo doc --workspace --no-deps --keep-going`
  with `RUSTDOCFLAGS=-Dwarnings`, in two parallel legs (default features
  and `--all-features`), so a broken or private intra-doc link anywhere in
  the workspace fails the push. It documents the whole workspace, so an
  error that only `cargo doc -p <crate>` shows is outside it.
- `governed-jsonl` and `mcp-confinement-positive` (push; a pull request
  runs a lane when it touches the lane's inputs, `GUARD_LANE_INPUTS` in
  `scripts/ci-cargo-lanes.mjs` (the files that define its selected cases,
  and their crates' manifests), when it changes the lane's definition
  between its base and head, `GUARD_LANE_DEFINITIONS` (its ci.yml job
  without comment and blank lines, with the governed step's expected list
  and command; the Makefile's CONFINEMENT_* assignments and the
  test-mcp-confinement-positive recipe with its cargo flags; the selection
  of the default nextest profile that recipe runs, `.config/nextest.toml`'s
  `[profile.default]` and any default override that sets a `default-filter`;
  the attributes and `mod` line declaring each module that holds selected
  cases), and when its diff is unknown.
  `scripts/ci-guard-lane-inputs-selftest.mjs` resolves every selected case
  to the files that define it and fails when such a file or its crate's
  manifest is not an input, when one of those declarations differs from
  its pin, or when a second module name ends in governed_jsonl. Neither
  watches the parent modules' declarations, the scripts the recipe calls or
  other global build configuration): exactly the non-MCP governed JSONL
  tests of meerkat-rpc that the step's expected list names, and exactly the
  Linux positive confinement cases of `make test-mcp-confinement-positive`
  (its CONFINEMENT_* filters and counts), on every Rust change.
- `feature-check` (push only; two jobs per clippy shard, Bazel set and matrix): compile-only checks of the
  shard's packages under the feature sets the Bazel graph builds them with
  (`--no-default-features` plus exactly the set their generated
  `BUILD.bazel` gives their tests, `--lib --bins --tests`; then each other
  Bazel library/binary set, such as production variants without
  `test-support` and surface variants) and the `$(CARGO) check` rows of
  `make test-minimal` and `make test-feature-matrix-lib`. Feature names are
  read from `BUILD.bazel` and the Makefile and validated against cargo
  metadata, so a renamed feature fails the plan.
- `wasm-check` (`make wasm-check`: wasm32 check and clippy of the web
  runtime and the crates it links): every Rust-relevant push, and pull
  requests that change the web runtime or a workspace crate it compiles
  (its dependency closure in the cargo graph, the provider crates
  included), plan the whole workspace, or change the CLI web template.
  `example-web` and `wasm-timers` run on a pull request only for their
  own inputs (`sdks/web/`, the runtime, the contracts, examples 031-033),
  and on a push also for every change whose closure reaches
  `meerkat-web-runtime`; `example-web` also runs the Web SDK's `npm test` (type
  tests over the generated events, node unit suites). A pull request that
  changes a crate the web runtime builds on, or plans the whole workspace,
  runs only that `npm test` and leaves the wasm build, the browser suites
  and `wasm-timers` to the push (`example_web_wasm` false, named in
  `deferred_to_push`): the cold wasm build of the runtime's closure alone
  took about 20 minutes. On a push or dispatch `example-web` gets 35 minutes,
  `integration` and `mcp-confinement-positive` 50 (a pull request keeps 25,
  30 and 40), and `feature-check` 60: caps sized from cold runs after a
  meerkat-core change.
- `push: main` only (no budget): `main-unit` over the whole workspace (the
  unit lane's rows verbatim, then the rest of the workspace in packed
  shards), `sdk-host`, and the push-only lanes above.
- Long rows first: `clippy`, `main-unit`, `closure-tests` and
  `feature-check` run in two waves (`<lane>` and `<lane>-rest`, a copy the
  lanes self-test checks); rows measured at 8 minutes or more
  (`scripts/ci-row-durations.json`, keyed by a row's name without a packed
  row's member count, so a repack keeps the measurement while the bin's
  heaviest member stays the same), or never measured, start first, and the
  rest and the short lanes wait for the `stagger` job. A red main run is
  a failed `CI gate` on the main commit and blocks `require_ci_green`.
- `gate` (`CI gate`, the only required context): fail-closed aggregate; on
  pull requests a 1080-second lane execution budget (classification, the
  stagger job for the lanes that wait for it, and each lane's own run time,
  runner queue excluded) and a 1200-second
  push-to-terminal runaway ceiling (the owner's cap is 20 minutes from the
  start of the pre-push hook, which takes about a minute) (each lane timed from the start of the
  run attempt it ran in, runner queue on its path excluded); an
  exact-profile lane the plan ran only because the change touched its
  inputs or changed its definition (`guard_lane_reasons`) is outside both
  budgets, still required, bounded by its own timeout-minutes and named in
  a `Guard lane outside the budget` warning; runner queue
  wait of 300 s or more on the critical path is a `CI runner queue wait`
  warning with a typed verdict, never a
  failure (the account's 40 concurrent hosted jobs are shared by every PR,
  main push and nightly); the lane table is reported on every non-cancelled
  run, failed runs included; a push or a dispatch is a full validation run
  that only reports the budget (a failure to measure only warns there);
  schema-4 attestation (backend `github-hosted-cargo`) on successful pushes
  to `main` and to `release/**` branches. For a `release-projection` plan
  it re-verifies the reused parent
  attempt with the parent's copies of the scripts (`ci-release-evidence.mjs
  verify`: a completed, successful push run of `ci.yml` on `main` at exactly
  HEAD's first parent, still the run's latest attempt, whose attestation
  names that commit and tree, planned the whole workspace with every lane
  `success` and is not itself a reuse)
  and records the run and attempt in the attestation (`plan.mode:
  release-projection`, `reused_test_evidence`). It runs under
  `!cancelled()` so superseded runs surface as cancelled.
  `scripts/ci-gate-consistency-selftest.mjs` (in `make ci-lanes-selftest`)
  runs its shell on the planner's plans with each job's `if:` evaluated:
  the gate must pass a run in which every selected lane passed and fail one
  in which a lane that ran failed or did not run (an allowlist names the
  dispatch's archived main-unit lanes).

The dense Mob topology stress, the bounded TLC `machine-verify --all` sweep,
the feature-matrix test runs, audit, the SDK suites, and the whole
BuildBuddy/Bazel graph do not run in PR CI; integration-fast and e2e-fast run
on the push for the test scope only (`closure-tests`). `cargo.yml`
remains a separate reusable/manually dispatchable Cargo workflow with its own
`Cargo lane gate`; `ci.yml` does not call it. Local Make commands still
default to Cargo.

**Nightly** (`.github/workflows/nightly.yml`, cron + dispatch) owns everything
PR CI does not. A dispatch with `graph_only=true` runs only `gcp-buildbuddy`:
the release final gate runs it on the release branch tip before the tag
(`gh workflow run nightly.yml --ref release/X -f graph_only=true`), which
also warms the remote cache for the tag run. The self-hosted graph is
default off behind the repository variable `MEERKAT_GCP_BUILDBUDDY_ENABLED`:
unless it is `true`, scheduled runs skip `gcp-buildbuddy` (nothing in GCP
starts; the other nightly jobs still run) and a `graph_only` dispatch fails
in `buildbuddy.yml`'s gate instead of finishing green. After the variable is
set again, the first `full-fresh` run (a nightly, or a release final gate's
`graph_only` dispatch) starts with a cold action cache for the
`linux_x86_64_gcp_ci` platform, whose action keys changed when it stopped
naming an image. Accept the re-enabled lane once that run is green with its
actions in the digest-pinned `MEERKAT_BUILDBUDDY_CI_IMAGE` and the next run is
warm. Every change of `MEERKAT_BUILDBUDDY_CI_IMAGE` starts that cache cold
again: the image digest is part of the platform's action keys. The full
nightly set is:
`workspace-unit` (`make test-unit`), `workspace-int` (`make test-int`),
`e2e-fast`, `dense-topology` (`mob-dense-topology.yml`),
`machine-verify` (bounded TLC, including every non-skipped composition
witness with its completion proof), `machine-verify-deep` (`make
machine-verify-deep-compositions`: Deep TLC over the compositions that fit
the 900 s per-run cap; a capped run fails as TLC INCOMPLETE), `sdk-host`,
`gcp-buildbuddy`
(`buildbuddy.yml` in `full-fresh` mode: the whole Bazel graph), plus the
existing `lint` (clippy `--all-targets`), `lint-feature-matrix`,
`test-feature-matrix`, `test-minimal`, `test-surface-modularity`,
`e2e-system`, `test-sdk-web`, `wasm-contract`, `check-rust-release-packaging`,
and cargo-deny sweep.

**Published crates standalone** (`.github/workflows/published-crates-standalone.yml`,
daily after nightly's chains and on dispatch) runs
`make published-crates-standalone-census` in shards: every release crate is
packaged and unpacked into a synthetic workspace, and per crate, without
sibling feature unification, its tests run there with plain `cargo test` and
the feature set its `[package.metadata.meerkat.standalone-tests]` publishes,
the naive `cargo test --no-run` compiles with default features, and the test
names per target must equal the workspace's minus the files and tests
`ci/standalone-workspace-only.toml` declares workspace-only. A unit test that
needs a crate the package cannot depend on (a cyclic or unpublished
dev-dependency) is gated on its crate's internal `workspace-tests` feature,
which only the crate's path-only self dev-dependency enables (`cargo package`
strips it), and the Bazel generator fails a crate that declares the feature
without that edge. Publication does not wait for the census yet: the release
gate on a passing run at the release commit returns once a nightly run has
passed and its duration is measured.

The release tag path requires successful exact-main CI (schema-4 attestation;
for a maintenance release, the same push CI on `release/<version>`, the
release branch of the tag's version)
and then, while `MEERKAT_GCP_BUILDBUDDY_ENABLED` is `true`, runs the full
BuildBuddy graph (`release_validate_buildbuddy_full`) as its validation gate;
while it is off (the default) the tag skips `release_validate_cargo` only when
its verified attestation proves that run ran every lane the job
repeats: the plan's `generated_contract` flag with a green `ratchets` job
(main CI runs those ratchets only when its plan selects them) and a green
`sdk-host` job (or a release-projection plan that reused a verified
workspace-mode parent run for it). Any other attestation, legacy schemas
included, keeps Cargo validation on the tag. Cargo validation is also the
manual-dispatch fallback.
The owner-selected BuildBuddy release backend (remote.buildbuddy.io) covers
the critical Linux (x86_64, aarch64) and macOS arm64 binaries. The trailing
platforms build in their own jobs on either backend: Windows release
binaries are cross-compiled with cargo-xwin (clang-cl, lld-link, the Windows
SDK) on a GitHub-hosted Ubuntu runner and then executed for verification on a
windows-2025-vs2026 runner, and Intel macOS binaries are cross-compiled on an
Apple silicon runner (macos-15). The critical archives publish as soon as
their lane passes; the trailing ones are added afterwards, add-only, with the
complete manifest and the Homebrew formula.

**Release semver readiness** (`.github/workflows/release-semver-readiness.yml`)
is a separate workflow triggered by `Cargo.toml`/`CHANGELOG.md` changes on
pushes to `main` and `release/**` branches and on PRs, or by manual dispatch. For unpublished candidate
versions, it measures declared breaks and uploads exact-tree, exact-version
evidence with 30-day configured retention. A post-release tree, whose notes
sit under `## [Unreleased]` above the stamped workspace version, is measured
against that version's own tag and gets no attestation. Only its successful push
artifact, `meerkat-semver-attestation-main-<tree_sha>` (named so on a release-branch
push too), qualifies for the normal release semver gate, and only from `main`
or from the release branch of the tag's version; PR and manual artifacts are previews.

**Release** (`.github/workflows/release.yml`) — runs on `v*` tag push or manual dispatch:

| Job | Trigger | What it does |
|-----|---------|-------------|
| `require_ci_green` | Always | Requires successful exact-main CI for the release commit (a successful push run of `ci.yml` at exactly that commit on `main`, or on `release/<version>` for a maintenance release, `<version>` being the checked-out workspace version; no other branch, pull-request or dispatched run); tag runs also verify its retained exact-tree attestation (schema 4, backend `github-hosted-cargo`; legacy schemas accepted; its ref `main` or that release branch) and, for a `release-projection` attestation, re-verify the reused first-parent run and its attestation with the parent's copies of the scripts; a prebuild dispatch instead verifies its request (on main, no publishing or recovery input, tag not yet created) and waits for nothing |
| `release_validate_buildbuddy_full` | Tag runs while `MEERKAT_GCP_BUILDBUDDY_ENABLED` is `true` | Runs the whole BuildBuddy/Bazel graph (`buildbuddy.yml`, `full-fresh`) on the release tree |
| `release_validate_cargo` / `release_validate_buildbuddy` | Eligible manual dispatches; `release_validate_cargo` also on tags whose attestation does not prove its lanes ran, while the self-hosted graph is off | Validate release state through the selected lane; tag runs whose attestation covers those lanes (or with the graph on) reuse exact-tree CI and skip both jobs |
| `release_validate_gate` | Tags and full/package dispatches | On tags require exact-tree CI plus the full BuildBuddy graph (graph on), or with the graph off pass on a verified attestation that covers the Cargo validation lanes (`release_validate_cargo` otherwise); on manual dispatches accept the selected validation lane; Web-only and asset-only recovery skip it |
| `release_semver_gate` | Tags and full/package dispatches | After `require_ci_green`, verifies unexpired exact-tree, exact-version push readiness evidence (from `main`, or from the release branch of the tag's version for a maintenance release) on the normal tag/explicit-tag path; narrower manual measurement/recovery exceptions are described in the release guide |
| `release_prebuild` | Tag runs | Finds a usable pre-tag prebuild of exactly the tag commit (`scripts/release-find-prebuild`: a `release.yml` dispatch on main at that commit whose `Release prebuild gate` passed, so its critical binaries and Web SDK package are ready while its trailing lanes may still run, and whose critical artifacts have not expired; a gate that has not concluded is waited for, unless a job it needs already failed, timed out or was cancelled); when found, the build jobs below are skipped and the publishers use that run's artifacts |
| `build_binaries` / `build_binaries_buildbuddy` / `build_binaries_critical_gate` | Tags without a prebuild, manual asset recovery, or a prebuild dispatch | The selected BuildBuddy or GitHub-hosted lane builds the critical platforms (Linux x86_64 and aarch64, macOS arm64); each target packages 4 binaries (`rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`); the critical gate requires the selected lane (or, when the archives come from a prebuild or `assets_run_id`, that every build job was skipped) |
| `build_binaries_windows_cross` / `verify_windows_binaries` / `build_binaries_macos_x86_cross` / `build_binaries_gate` | Same | The trailing platforms on either backend: Windows cross-compiled from Linux with cargo-xwin and run on windows-2025-vs2026, Intel macOS cross-compiled on Apple silicon (`macos-15`); the full gate requires the selected critical lane plus every trailing job |
| `build_web_sdk_package` | Tags without a prebuild, package/Web recovery without a reused artifact, or a prebuild dispatch | After `require_ci_green`, beside the release validation and the semver gate, builds the `@rkat/web` package artifact, stamps it with its commit, package and tarball digest, and attests it; Cargo runs four jobs while the tree's `scripts/release-web-sdk-rustc-wrapper` keeps the memory-heavy wasm32 crates off the catalog's rustc, or one job for a tree without a matching wrapper |
| `release_prebuild_gate` | Prebuild dispatches | Passes once the critical binary build gate and the Web SDK package succeeded (the run's trailing lanes continue); the marker `release_prebuild` requires of a prebuild run |
| `publish_critical_release_assets` | Tags | As soon as the critical gate passes, verifies and publishes exactly the Linux and macOS arm64 archives (add-only, no manifest); waits for no trailing job |
| `publish_github_release` | Tags or manual asset recovery | The trailing publisher: after exact-commit CI (`require_ci_green`) and the full gate, downloads artifacts (a prebuild's once that run's full binary build gate passed, verified exactly like an `assets_run_id` recovery), adds the archives not yet on the release (never replacing one), then writes the complete `checksums.sha256` + `index.json` once (refusing to change a published one) |
| `update_homebrew` | After the trailing publisher | Updates the Homebrew tap formula from the complete release's published checksums |
| `publish_semver_baseline` / `attach_semver_baseline` | Tags or asset recovery, alongside publication | `publish_semver_baseline` generates rustdoc JSON for every publishable library crate on the release tag as soon as `require_ci_green` passes; `attach_semver_baseline` attaches it as `semver-rustdoc-<version>.tar.zst` once the GitHub release is published (add-only: a baseline already attached stays). It is the baseline the next release's semver gate compares against; no job waits for either |
| `publish_registries` | Tags or manual `publish_release_packages=true` | Publishes 46 Rust crates → crates.io by dependency level (concurrently within a level), Python SDK → PyPI, TypeScript SDK → npm; before the SDK packages build, it waits for the release's complete manifest (`scripts/release-await-binary-manifest`), verifies each `rkat-rpc` archive's build attestation and stamps the archive and binary digests into both (`scripts/release-stamp-sdk-binary-digests`), and each publish step refuses a package that does not pin them for its version |
| `publish_web_sdk` | Tags or manual package/Web recovery | After `require_ci_green`, `release_validate_gate` and `release_semver_gate` (both skipped only on Web-only recovery), the registry credential preflight and the build (or a found prebuild), checks the artifact's stamp against the release commit (and a reused artifact's source run), verifies a prebuild's package attestation and version, then publishes `@rkat/web` → npm |

**Build matrix:**

| Platform | Target | Publication |
|----------|--------|-------------|
| Linux x86_64 | `x86_64-unknown-linux-gnu` | critical |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | critical |
| macOS ARM64 | `aarch64-apple-darwin` | critical |
| macOS x86_64 | `x86_64-apple-darwin` | trailing (cross-compiled on Apple silicon) |
| Windows x86_64 | `x86_64-pc-windows-msvc` | trailing (cross-compiled on Linux) |

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
artifacts and package metadata to the selected existing tag; a maintenance
release's tag runs its own definition instead (`RELEASE_REF=vX.Y.Z`, see
Releasing below).

**Pre-tag prebuild** (`make release-prebuild VERSION=vX.Y.Z`, dry run with
`RELEASE_WORKFLOW_DRY_RUN=true`): once the release commit is on `main`,
dispatch `release.yml` with `prebuild_release_artifacts=true` on `main`, on
the backend its tag run will select (the request refuses another). It
builds, stamps, attests and uploads every release binary and the Web SDK
package of that commit while CI and semver readiness run, and publishes
nothing. The tag run of exactly that commit then skips every build and
publishes those artifacts after the `assets_run_id` verification (each
archive and its source stamp attested by `release.yml` at that commit on
`refs/heads/main`, the run's binary build gate green, every binary embedding
the release version); without a complete prebuild it builds as before.

### Pre-commit Hooks

Installed via `make install-hooks`. Two stages:

**On commit** (`pre-commit`):
- `repo-cargo fmt --all`
- `node scripts/precommit-build-hook.mjs` (runs `scripts/generate-bazel-rust-builds.mjs` to regenerate Bazel rust build metadata when the staged changes, deletions included, touch Cargo metadata, a Rust source under `crates/` or `tests/`, or a generator input; about 5 s. It refuses the commit while a Rust file under `crates/` or `tests/` is not in the index, since the generator would read it from disk)
- `scripts/sync-meerkat-dogma-skill-docs.sh` (syncs dogma docs into the skill)

**On push** (`pre-push`), sized to finish in a few minutes warm (the CI it
duplicated is the authority; heavy lanes are CI's, below):
- Secret detection (gitleaks)
- Trailing whitespace, end-of-file, YAML/TOML validation, merge conflict check, large file check
- `repo-cargo fmt --all -- --check`
- `scripts/pre-push-audit-generated-headers.sh`
- `scripts/pre-push-bridge-no-responsestatus.sh` (thin wrapper over `xtask bridge-classifier`)
- `scripts/pre-push-bazel-locks.sh` (generated BUILD freshness, offline MODULE.bazel.lock recorded-input check, the release test-code gate `scripts/check-bazel-release-features.py` (every Bazel release build passes `--define=meerkat_release_build=1`, under which no target the shipped binaries link enables a feature from `scripts/non-shipping-features.json`, and no crate_universe repository of an external crate listed there under `external_crate_features` enables one of its listed features: tokio's `test-util`, which `tools/third-party/patches/rules_rust-release-stripped-crate-features.patch` moves behind the release select), `bb mod deps --lockfile_mode=error` when the pinned CLI is present; `--require-bb` makes that last gate mandatory and the release preflight passes it)
- `scripts/test-lock-consistency-gate.sh`, `scripts/test-bazel-module-lock-gate.sh`, `scripts/test-crate-enumeration-gate.sh`, `scripts/test-crate-license-files-gate.sh` (contract tests: each new release-infra gate must still fail on the defect it was written for)
- `scripts/test_release_stamp_sdk_binary_digests.py` (synthetic releases and a stubbed `gh`, no network; runs when the stamp, wait or SDK client files change: the SDK binary digest stamp writes nothing unless the release manifest is complete for the tag, every `rkat-rpc` archive is within the SDKs' 256 MiB cap, re-hashes to its entry, carries a build attestation from the release workflow and holds exactly one root binary, and both SDK packages carry the tag's version; `verify`, the publish steps' gate, refuses an unstamped, wrong-version, incomplete or different table; the committed digest modules are the unstamped rendering; `scripts/release-await-binary-manifest` fails at once on a failed publisher and counts a failed API call as not yet)
- `scripts/test-gcp-buildbuddy-workflow-gate.sh` (plans nightly, release and `buildbuddy.yml` jobs with `MEERKAT_GCP_BUILDBUDDY_ENABLED` unset and `true`: unset starts nothing in GCP and tags validate on an exact-tree CI attestation that covers the Cargo validation lanes, with Cargo otherwise; `true` keeps the full-graph route)
- `scripts/test-gcp-buildbuddy-hardening.sh` (stubbed `gcloud`/`bb`, no network: the self-hosted control plane renders authenticated, TLS-only, executor-authorized config and installs exactly it; `ensure` creates only IAP/executor allows behind an explicit deny, never edits or re-enables a firewall rule, and refuses open, legacy, disabled or drifted rules, VMs, disks and digest-less images; executors get per-secret IAM and gRPCS only; the wrapper never sends an API key to a non-TLS endpoint, and the self-hosted path never uses the BuildBuddy Cloud key)
- The hook harness contracts: `scripts/test-pre-push-lane-retention.sh`, `scripts/test-pre-push-machines.sh`, `scripts/test-release-packaging-isolation.sh`, `scripts/test-ci-nextest-archive.sh` on every push, and the two slow ones only when a push changes what they exercise: `scripts/test-pre-push-dispatch.sh` (the dispatcher and the hook config) and `scripts/test-pre-push-unit-status.sh` (the deterministic gate, the release-projection and harness-only classifiers, and `scripts/test-pre-push-ratchets.sh`, which reads the CI planner, `ci.yml` and the agent gate)
- `scripts/pre-push-prune-lanes.sh` (runs from the dispatcher only after a PASSED gate: keeps at most `MEERKAT_PRE_PUSH_KEEP_LANES` (default 2) `pre-push-<hash>` hook worktrees and Cargo target lanes per repo, never touching the current lane, a lane whose dispatcher lock is live, a lane referenced by any live process, a lane with any file activity inside `MEERKAT_PRE_PUSH_LANE_IDLE_SECS` (default 21600), or any non-lane name; logs one `kept:`/`pruned:` line per lane with its reason; `MEERKAT_PRE_PUSH_KEEP_LANES=all` disables it)

**Manual** (`pre-commit run --hook-stage manual <id> --from-ref <base> --to-ref HEAD`):
the push stage ran these before 0.9.3, 68-116 minutes of every push on 2026-10-14..16,
and CI runs each of them before merge or on the push a release tag requires
green, except the deterministic gate's lib tests under the workspace's unified
features, which only nightly's workspace unit lane runs:
- `changed-path-ratchets`: `scripts/pre-push-ratchets.sh` (the agent gate's changed-path ratchets: formal/governance targets, generated contract freshness, Web SDK tests, wasm check; no Clippy). CI: the `ratchets` job (generated contracts; drift on a machine-authority change), the `governance-gates` job (the typed governance gates on a machine-authority or dogma change), `example-web` and `wasm-check`, selected by the same paths; `scripts/test-pre-push-ratchets.sh` pins it (each ratchet's CI step and trigger included) and runs in `make ci-lanes-selftest`
- `machine-codegen-drift` / `machine-codegen-verify`: `scripts/pre-push-machines.sh --codegen-only|--verify-only` (machine and protocol codegen under the clean-tree contract; the canonical TLC lane). CI: the `ratchets` job's `machine-check-drift`/`protocol-check-drift` and the `tlc-audits` shards on a machine-authority change; the `machine-verify --all` sweep nightly and on the release gate
- `cargo-test`: `scripts/pre-push-unit.sh` (deterministic Cargo/nextest gate: when fresh execution is needed, runs workspace unit, integration-fast, HeadCanonical cold-restart, and `e2e-fast` lanes; serializes identical source evidence and retries a timed-out lane once. `MEERKAT_BUILDBUDDY` does not switch this hook's backend). CI: the pull request's unit lanes for the changed packages, the push's `main-unit` and `closure-tests` rows, nightly's workspace unit lane
- `release-test-code-guards-contract`, `release-doctor-workflow-contract`, `release-find-prebuild-contract`, `release-standalone-gate-contract`, `macos-x86-release-binary-check-contract`, `release-sqlite-profile-check-contract`, `release-asset-provenance-contract`: CI's `Format + governance` job runs these on every pull request

The manual deterministic gate's source-test cache uses a source fingerprint that excludes
root `Cargo.lock` and `MODULE.bazel.lock`. On a lock-only cache hit, the current
lock graph is compiled by the required CI Clippy lanes but is not re-tested locally;
the reused tests ran against the prior lock graph, and CI remains
authoritative. Fail-closed `release-projection-only` and
`pre-push-harness-only` classifiers can also reuse existing parent source-test
evidence; CI's `release-projection` plan uses the same
`scripts/release-projection-only.mjs`. This is separate from `pre-push-dispatch.sh`'s complete-hook success
stamp, which is keyed to the entire pushed Git tree and written only when no
hook was skipped. A push with `SKIP=<hook ids>` records partial evidence labeled
with the skipped set, reusable only by a later push of that tree that skips the
same hooks; a push without that `SKIP` re-runs them.

After the gate, passed or failed, the dispatcher prints each hook's wall time
and appends it to `.git/meerkat-hook-cache/timings.tsv`.

**Manual local preflight**:
- `pre-commit run --hook-stage manual agent-check-changed` (runs `scripts/agent-gate --staged`)
- `scripts/test-changed-crates.sh`

**Avoid duplicate final qualification**:

Run the change-specific regression and required stress checks during development.
For a candidate proceeding through normal commit hooks, one normal push and its
pull request, use that pull request's CI for overlapping final lint, governance
and deterministic tests instead of prepending another standalone `make test` or
`make agent-gate`. If no pull request will run, retain an appropriate standalone
gate before declaring the work qualified. This changes scheduling, not required
coverage or hook behavior.

Only deduplicate an inventory actually covered by the selected push backend,
paths, test targets and features. The push hook runs no Clippy, no ratchets
and no tests: CI runs them before merge (the pull request's Clippy, unit,
integration, ratchet and TLC lanes) or on the push to `main` (the
`closure-tests` rows, `main-unit`, the closure and feature checks). The manual
`changed-path-ratchets` hook runs
`agent-gate --committed --ratchets-only --machine-drift-by-hook`; its path base is
`CARGO_AGENT_BASE` or `origin/main` (then `main`/HEAD), which can differ from the
dispatcher's remote-ref range. The manual deterministic lane covers workspace
default-feature `kind(lib)` and `kind(test)` inventories,
plus HeadCanonical `test-support` and `e2e-fast`. It does not prove package-only
feature isolation, binary-target unit tests, required-feature targets absent
from that inventory, doctests, ignored/live/system lanes, platform checks or the
feature matrix. Retain each distinct requirement; all-feature Clippy is not an
all-feature test result. BuildBuddy-specific test requirements also stay distinct.

`make agent-gate` additionally runs `docs-check` and the selected lane doctor;
the push does not call that Make wrapper. Keep `docs-check` for changed docs or
validator inputs and whenever applicable docs evidence is missing. Run the
selected doctor for new or changed environments/lanes, or uncertain setup. Keep
explicit task, CI and release validation requirements unchanged.

Use the repository-installed hook and push one named destination branch from
the checked-out branch, for example
`git push origin refs/heads/my-branch:refs/heads/my-branch` or
`git push origin HEAD:refs/heads/my-branch`. The dispatcher picks the diff base
from the destination ref: a new branch is compared with the remote default
branch's merge-base, an existing branch with its previous remote commit, and a
tag stays on the all-files empty tree. Keep normal hooks enabled; this guidance
does not authorize `SKIP` or `--no-verify`.
Record the exact commit/tree, push outcome and distinct acceptance receipts.
Existing full-tree or source-test cache reuse is reported as reuse, not fresh
execution; lock-only source-test reuse does not re-test the new dependency graph.
A hook failure or interrupted push is incomplete qualification, not a waiver.

### Version Parity Contract

Six files must agree on the same version:

| File | Field |
|------|-------|
| `Cargo.toml` (workspace root) | `workspace.package.version` — **source of truth** |
| `crates/meerkat-contracts/src/version.rs` | `ContractVersion::CURRENT` |
| `sdks/python/pyproject.toml` | `version` |
| `sdks/typescript/package.json` | `version` |
| `sdks/web/package.json` | `version` |
| `artifacts/schemas/version.json` | `contract_version` |

Additionally, all internal crate dependencies in `Cargo.toml` (47 path deps) must require the workspace version exactly (`version = "=X.Y.Z"`; cargo-release keeps the `=` when it bumps). 0.x patch releases may break API, so every requirement a release crate publishes on a workspace crate is exact; `make check-rust-release-config` checks the manifests and `make check-rust-release-packaging` the packaged archives.

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
Local preflight and PR/manual readiness previews are not substitutes. To
let CI reuse evidence for the bump, merge the release notes and fixes first
with the trailer `CI-Plan: workspace` (in the merge commit's message or on
the merged branch's head commit), wait for that main run to succeed, then
land the bump as its own commit on top (a bump-only PR or push): CI plans
only the release checks and the readers of the changed files and reuses the
rest of that run. A bump merged together with source changes, as v0.9.1's
release branch merge was, or on a parent whose main run was not a
whole-workspace run, gets the full plan. For the fast tag path, also dispatch
`make release-prebuild VERSION=vX.Y.Z` as soon as the release commit is on
`main` and push the tag once CI, readiness and the prebuild are green: the
tag run then validates on the CI attestation and builds nothing. Keep
version/changelog generation hook-owned; see the
[release guide](docs/guides/cd-and-distribution.md#release-workflow) for the
separate manual measurement and completed-measurement recovery paths.

**Maintenance releases** ship a fix on top of an earlier release without
`main`'s unreleased changes: branch `release/<version>` from that release's
tag, merge the fix and the cargo-release bump there, and tag that branch's
head, which never reaches `main` first. `require_ci_green` and
`release_semver_gate` accept CI and readiness evidence from a push to
`release/<version>` of the tag's own version next to `main`'s, and from no
other branch. Give the release commit `CI-Plan: workspace`: a
release-branch push diffs against its payload base and reuses no
projection. It has no prebuild (`make release-prebuild` is `main`-only), so
the tag run builds every binary, and its recovery dispatches run the tag's
own workflow (`RELEASE_REF=vX.Y.Z VERSION=vX.Y.Z make release-assets`, and
likewise for packages and the Web SDK) until `main` has the same rules.
After publication merge the tag into `main` by pull request; never set
`main`'s version to the released one without that merge. See the
[release guide](docs/guides/cd-and-distribution.md#maintenance-releases).

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
make publish-dry-run              # Parallel dry-run for all 46 publishable Rust crates
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

The canonical publish order lives in `scripts/release-rust-crates.sh` (46 crates, dependency order):
`meerkat-sqlite` → `meerkat-machine-derive` → `meerkat-machine-dsl-core` → `meerkat-agent-build-authority` → `meerkat-core` → `meerkat-sandbox` → `meerkat-authorization-contracts` → `meerkat-atif` → `meerkat-store-conformance` → `meerkat-models` → `meerkat-capabilities` → `meerkat-machine-dsl` → `meerkat-machine-schema` → `meerkat-authorization` → `meerkat-machine-kernels` → `meerkat-skills` → `meerkat-schedule` → `meerkat-jobs` → `meerkat-workgraph` → `meerkat-contracts` → `meerkat-store` → `meerkat-llm-core` → `meerkat-live` → `meerkat-auth-core` → `meerkat-memory` → `meerkat-mcp` → `meerkat-hooks` → `meerkat-comms` → `meerkat-runtime` → `meerkat-copilot` → `meerkat-anthropic` → `meerkat-openai` → `meerkat-gemini` → `meerkat-providers` → `meerkat-tools` → `meerkat-session` → `meerkat-client` → `meerkat` → `meerkat-mob` → `meerkat-mob-adaptive` → `meerkat-mob-mcp` → `meerkat-mob-pack` → `meerkat-mcp-server` → `meerkat-rpc` → `meerkat-rest` → `rkat`

Publication walks the dependency levels derived from this order and
`cargo metadata` (`scripts/release-rust-crates.sh --levels`, computed by
`scripts/release_crate_levels.py`; `--format summary` prints them): every
crate of a level publishes concurrently (`MEERKAT_CRATE_PUBLISH_JOBS`,
default 6 for uploads, 1 for dry runs), and the next level starts only once
the whole level is visible in the crates.io sparse index. The order stays the
only hand-written enumeration; `make check-rust-release-config` fails when a
release crate depends on one listed after it, and
`scripts/test-release-publish-rust-crates.sh` (part of
`make check-rust-release-packaging-contract`) proves the level order, the
bounded overlap, the skip of a published version, the stop at the first
failure and the server-error retry.
crates.io's own publish rate limit for new versions (a burst, then about one
per minute) still bounds a full release. A crates.io server error (5xx, or
cargo's "Request timed out after 30 seconds", its report of a 503 that
arrives late in an upload) or a network error (curl's code and text, such
as "[28] Timeout was reached": an unresolved host, a refused, reset or
dropped connection, a failed TLS handshake) is retried after each delay of
`MEERKAT_CRATE_PUBLISH_SERVER_ERROR_BACKOFF` (default `30 60 120 240`
seconds), on a budget separate from the rate limit's; an upload that landed
despite the error is skipped as published.

### Key Rules for AI Agents

- **Never bump `workspace.package.version` without also running `scripts/bump-sdk-versions.sh`** — the CI gate will catch drift
- **Never change types in `meerkat-contracts` without running `make regen-schemas`** — schema artifacts and SDK types will be stale
- **Use the final qualification sequence above** - run change-specific required checks, then normal commit hooks and one normal push for the overlapping inventory; retain standalone and explicit checks for coverage the push does not supply
- **`ContractVersion::CURRENT` must equal `workspace.package.version`** — they are lock-stepped
- **Never change `Cargo.lock` without refreshing `MODULE.bazel.lock`** - the lock is a crate_universe extension input; `make buildbuddy-lock-update` regenerates it, and `make verify-bazel-locks` proves it
- **Never add or change a fenced `rkat` example in `docs/` without refreshing its fixture** - the published `rkat` package parses every one against the CLI from `crates/meerkat-cli/tests/fixtures/doc_examples.txt`; `UPDATE_DOC_EXAMPLES_FIXTURE=1 cargo test -p rkat --test doc_examples_fixture` regenerates it, and that test fails until it matches
- **Never hand-maintain a second list of workspace crates** - `scripts/release-rust-crates.sh` is the one hand-ordered enumeration; the patch config derives from it and `make check-rust-release-config` fails when the documented order, count, or patch map disagrees
- **Never write a bare crate count in this file that nothing derives** - `make check-rust-release-config` rejects any `N crates` / `N path deps` claim it cannot bind to a computed quantity. Register the new claim in `documented_count_claim_errors` (`scripts/check_rust_release_packaging.py`) alongside the artifact that owns the number, or write it as an approximation with a leading `~`. Release crate count and internal path-dep count are different facts with different owners; they are not interchangeable even when they happen to be equal
- **Never size a thread stack in a host binary or bypass the budget with `#[tokio::main]`** - every host runs through `meerkat_runtime::host_stack::run_host` on the one documented 8 MiB default budget (`HOST_WORKER_STACK_BUDGET`); a deep frame is fixed with `meerkat_runtime::stack_relief`, not a bigger stack. The debug canary `rpc_dispatch_path_fits_debug_worker_stack_budget` (4 MiB) and nightly `make stack-budget-release` (1 MiB release) pin the numbers
- **Use cargo-release for releases** - the configured `./scripts/repo-cargo release patch --execute` flow handles version projections, changelog stamping, schema/codegen refresh, BuildBuddy metadata, and parity verification through the release hook. Never manually bump versions or create tags; also satisfy the exact-tree CI and semver-readiness prerequisites above

## Testing with Multiple Providers

When running tests or demos that involve multiple LLM providers/models, use these model names:

| Provider | Model Name |
|----------|------------|
| OpenAI | `gpt-5.6-sol` or `gpt-5.6-terra` or `gpt-5.6-luna` or `gpt-5.6` or `gpt-5.5` or `gpt-5.5-pro` or `gpt-5.4` or `gpt-5.4-mini` or `gpt-5.3-codex` |
| Gemini | `gemini-4-argon` (paid Gemini API tier) or `gemini-3.8-flash` or `gemini-3.6-flash` or `gemini-3.5-flash-lite` or `gemini-3.1-flash-lite` or `gemini-3.5-flash` or `gemini-3.1-pro-preview` or `gemini-3.1-flash-lite-preview` |
| Anthropic | `claude-opus-5-5` or `claude-opus-5` or `claude-fable-5` or `claude-opus-4-8` or `claude-sonnet-5-5` or `claude-sonnet-4-6` or `claude-sonnet-4-5` or `claude-haiku-5-5` |

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
- **Runtime-backed surfaces are interchangeable skins** - CLI, REST, RPC, MCP Server, and Web/WASM route through `SessionService` to `AgentFactory::build_agent()`; explicit standalone Rust embeddings may compose agents directly
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
