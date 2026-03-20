---
name: meerkat-architecture
description: "Internal architecture guide for the Meerkat agent platform. This skill should be used when understanding crate ownership, trait contracts, the agent construction pipeline, session service lifecycle, runtime control plane, mob orchestration internals, machine authority, comms wiring, or making cross-cutting architectural changes. Oriented toward AI agents and developers working on meerkat internals, not end users."
---

# Meerkat Internal Architecture

Meerkat is a library-first agent runtime. The execution pipeline is shared across all surfaces. Understanding crate ownership, trait contracts, the runtime control plane, and the agent construction pipeline is essential for making changes that don't break architectural invariants.

## Core Principles

1. **Infrastructure, not application** — the agent loop is a composable primitive with no opinions about prompts, tools, or output.
2. **Trait contracts own the architecture** — `meerkat-core` defines contracts; implementations live in satellite crates.
3. **Surfaces are interchangeable skins** — CLI, REST, RPC, MCP, WASM all route through `SessionService` → `AgentFactory::build_agent()`.
4. **Composition over configuration** — optional components are `Option<Arc<dyn Trait>>`, not feature-flagged defaults.
5. **Runtime conforms to machines** — the runtime must follow the verified machine model, not the other way around.
6. **Mob is the only multi-agent path** — there is no sub-agent system. All delegated/forked/helper work routes through mob orchestration.
7. **Override-first resource injection** — `AgentBuildConfig` overrides take precedence over factory/config/filesystem resolution.
8. **Seams are formal too** — async owner handoffs, wait barriers, and surfaced terminal classes are part of the architecture and should be modeled/protocolized, not left to shell convention.

## Crate Ownership

| Crate | Owns | Key Trait |
|-------|------|-----------|
| `meerkat-models` | Model catalog, provider profiles, parameter schemas (leaf crate, no meerkat deps) | — |
| `meerkat-core` | Agent loop, types, budget, retry, state machine, ALL trait contracts | `AgentLlmClient`, `AgentToolDispatcher`, `AgentSessionStore`, `SessionService`, `CommsRuntime`, `HookEngine`, `OpsLifecycleRegistry` |
| `meerkat-client` | LLM providers (Anthropic, OpenAI, Gemini) | Implements `AgentLlmClient` |
| `meerkat-store` | Session persistence (SQLite, Jsonl, Memory, Redb) | Implements `SessionStore` |
| `meerkat-tools` | Tool registry, dispatch, builtins (task tools, utility helpers, shell) | Implements `AgentToolDispatcher` |
| `meerkat-session` | Session orchestration (Ephemeral, Persistent) | Implements `SessionService` |
| `meerkat-runtime` | Runtime control plane, input lifecycle, policy engine, drivers (ephemeral/persistent), silent intent override | `RuntimeControlPlane`, `RuntimeDriver`, `RuntimeSessionAdapter` |
| `meerkat-comms` | Inter-agent messaging (inproc, TCP, UDS, Ed25519) | Implements `CommsRuntime` |
| `meerkat-mob` | Multi-agent orchestration (MobBuilder, MobActor, FlowEngine, member provisioning) | `MobSessionService`, `MobProvisioner` |
| `meerkat-mob-pack` | Mobpack archive format, signing, trust policies, validation | — |
| `meerkat-mob-mcp` | Expose mob tools as MCP interface | `MobMcpState`, `MobMcpDispatcher` |
| `meerkat-web-runtime` | WASM browser deployment (wasm_bindgen exports) | — |
| `meerkat-hooks` | Hook runtimes (in-process, command, HTTP) | Implements `HookEngine` |
| `meerkat-skills` | Skill loading (filesystem, git, HTTP, embedded) | Implements `SkillEngine` |
| `meerkat-machine-schema` | Rust-native machine/composition catalog plus seam/handoff protocol metadata — the formal authority | — |
| `meerkat-machine-kernels` | Generated kernel interpreter for all machines | `GeneratedMachineKernel` |
| `meerkat-machine-codegen` | TLA+ model generation, TLC verification, drift detection | — |
| `meerkat` (facade) | AgentFactory, FactoryAgentBuilder, build_ephemeral_service, re-exports | Wires everything together |

**Rule: meerkat-core has zero I/O dependencies.** All I/O happens in satellite crates.

## Machine Authority

The system is formally modeled as canonical machines plus closed-world compositions, and is being extended with explicit seam/handoff protocols for async owner boundaries.

**Design rule:** centralized `meerkat-machine-kernels` is the enforced layout. Owner crates do NOT have `machines/mod.rs` re-export modules. The xtask verification explicitly rejects parallel owner modules.

**Machines:** InputLifecycleMachine, RuntimeControlMachine, RuntimeIngressMachine, OpsLifecycleMachine, PeerCommsMachine, ExternalToolSurfaceMachine, TurnExecutionMachine, MobLifecycleMachine, FlowRunMachine, MobOrchestratorMachine, plus seam-focused machines such as CommsDrainLifecycleMachine where an async boundary has its own lifecycle truth.

**Verification:** `cargo xtask machine-codegen --all` (regenerate), `cargo xtask machine-check-drift --all` (detect stale artifacts), `cargo xtask machine-verify --all` (TLC model checking + owner tests).

### Seam closure model

Meerkat does not solve composition correctness by collapsing everything into one giant machine. Instead it:

- reuses the existing composition actor model
- extends effect-disposition rules with handoff protocol metadata
- treats semantically relevant owner handoffs as outstanding obligations
- generates helper APIs so owner feedback is correlation-safe and not ad hoc shell code
- derives surfaced terminal classification from machine truth

Use this mental model when reviewing changes:

- single-machine mutators protect local truth
- compositions protect routed truth
- seam protocols protect owner-realized async truth

## Agent Construction Pipeline

`AgentFactory::build_agent()` is the single entry point for ALL surfaces. Key steps:

1. Validate host_mode
2. Resolve provider (infer from model or explicit)
3. Create LLM client (override > factory credentials > config)
4. Create LLM adapter (with event channel and event tap)
5. Resolve max_tokens
6. Build skill engine and tool dispatcher
7. Create session store (override > factory custom_store > feature-flag default)
8. Compose tools with comms (add send/list_peers tools)
9. Resolve hooks (override > filesystem layered config)
10. Build system prompt + AgentBuilder + wire memory/compactor/skill-engine/ops-lifecycle/event-tap/checkpointer
11. Build agent, set SessionMetadata

**Precedence at every step:** `build_config override > factory field > config resolution > default`

## Agent Loop State Machine

`CallingLlm` → `WaitingForOps` → `DrainingEvents` → `Completed`

With branches: `ErrorRecovery`, `Cancelling`

**WaitingForOps is real, but the important truth is the barrier membership.** The machine must own which async operations are barriers for the active run. Raw operation IDs are no longer enough on their own; the architecture is moving toward typed async-op references and explicit wait policy (`Barrier` vs `Detached`) so long-lived background work cannot accidentally block the turn forever.

## Runtime Control Plane

`RuntimeSessionAdapter` implements the `RuntimeControlPlane` trait with per-session `RuntimeDriver` instances (ephemeral or persistent).

**Key operations:**
- `ingest` — admit an input through policy resolution
- `retire` — graceful drain (process queue, reject new input)
- `respawn` — helper convenience: retire old session, spawn fresh with same identity/spec/wiring, new session ID (not machine-owned)
- `reset` — abandon pending, return to Idle
- `recover` — replay from store for crash recovery
- `destroy` — terminal state, no recovery
- `runtime_state` — query current state (Initializing/Idle/Running/Recovering/Retired/Stopped/Destroyed)

**Policy engine:** `DefaultPolicyTable` resolves `PolicyDecision` per input kind × runtime state. 9 input kinds (prompt, peer_message, peer_request, peer_response_progress, peer_response_terminal, flow_step, external_event, continuation, operation) × 2 states (idle, running). Each cell specifies ApplyMode, WakeMode, QueueMode, ConsumePoint, InterruptPolicy, DrainPolicy, RoutingDisposition.

**Silent intent override:** If an incoming peer intent matches the session's `silent_comms_intents` list, the policy is overridden to `ApplyMode::Ignore`, `WakeMode::None` — no LLM turn triggered. This is canonical runtime-owned behavior.

**Host-mode drain ownership:** host mode is not "just a loop that drains at turn boundaries." Runtime-backed and direct session-service paths must both follow canonical drain lifecycle truth. Suppression of turn-boundary drain is a projection of drain lifecycle state, not a free shell boolean.

**Respawn semantics:** Helper convenience (not a machine-owned primitive). Same member identity (MeerkatId), spec, and peer wiring — **new session ID**. Old session archived. Used for "agent is confused, restart it" recovery.

## OpsLifecycleRegistry

Trait in `meerkat-core/src/ops_lifecycle.rs`, concrete impl (`RuntimeOpsLifecycleRegistry`) in `meerkat-runtime/src/ops_lifecycle.rs`.

**Capabilities:** register/complete/fail/cancel/retire operations, `wait_all`, `collect_completed`, bounded completed-operation retention (FIFO eviction, default 256), multi-listener completion observation, peer info in snapshots, wall-clock timestamps (`created_at_ms`, `completed_at_ms`, `elapsed_ms` from SystemTime anchor), per-parent concurrency enforcement (`max_concurrent`).

Important current boundary:

- `OpsLifecycleRegistry` is still the authority-backed source of async operation truth
- tranche 1 barrier fixes stay inside `TurnExecution`
- tranche 2 is where barrier satisfaction gets reconciled end-to-end with authority truth instead of shell `wait_all` folklore

## Session Service

All surfaces route through `SessionService` for the full lifecycle:

```
CreateSessionRequest → SessionService::create_session() → RunResult
  └── SessionAgentBuilder::build_agent() → SessionAgent
      └── AgentFactory::build_agent() → DynAgent
```

Two implementations:
- `EphemeralSessionService<B>` — in-memory, no persistence (WASM, testing)
- `PersistentSessionService<B>` — event-sourced persistent orchestration (CLI, RPC, REST, MCP; typically backed by sqlite or redb through `PersistenceBundle`)

`FactoryAgentBuilder` bridges `AgentFactory` into `SessionAgentBuilder`.

## Persistence Pairing

Persistent realm opening is backend-owned in the `meerkat` facade through `PersistenceBundle`.

- Surfaces open a realm bundle, not a raw session store plus ad hoc runtime companion.
- The bundle carries the paired `SessionStore`, optional `RuntimeStore`, and matching `RuntimeSessionAdapter`.
- SQLite is now the default persistent realm backend when compiled; redb remains explicit.

## Mob Orchestration

**There is no sub-agent system.** All multi-agent work routes through mobs.

```
MobBuilder::new(definition, storage)
  .with_session_service(service)
  .allow_ephemeral_sessions(true)
  .create() → MobHandle
```

`MobHandle` is clone-cheap (Arc-shared state). Sends commands to `MobActor` via channel.

### Member Launch Modes

`MemberLaunchMode` (in `meerkat-mob/src/launch.rs`):
- `Fresh` — new session (default)
- `Resume { session_id }` — resume existing session
- `Fork { source_member_id, fork_context }` — fork from another member's history

`ForkContext`:
- `FullHistory` — `Session::fork()` (O(1) CoW)
- `LastMessages { count }` — `source.last_n(n).to_vec()` (shallow copy)

### Spawn Policies

`SpawnMemberSpec` carries: `launch_mode`, `tool_access_policy` (inherit/allow-list/deny-list), `budget_split_policy` (Equal/Proportional/Remaining/Fixed), `auto_wire_parent` (bool).

Budget splitting: orchestrator reads remaining budget, computes share per policy, decrements own budget, seeds child `BudgetLimits`.

### Helper Convenience

- `MobHandle::spawn_helper(prompt, opts)` — synthesize ephemeral mob, spawn, wait, return result, teardown
- `MobHandle::fork_helper(source_id, prompt, opts)` — same but with Fork launch mode

Profile source rule: agent-internal surfaces inherit from caller config. Non-agent surfaces (REST/RPC/CLI/MCP) require explicit config source — never silent defaults.

### Lifecycle Control

- `retire_member(id)` — archive session, remove from roster
- `force_cancel_member(id)` — cancel in-flight turn (distinct from retire)
- `respawn(id, initial_message)` — helper convenience: retire old session → enqueue spawn with same identity/profile/wiring/labels → new session ID (not machine-owned)
- `member_status(id)` → `MobMemberSnapshot` (status, output, error, timestamps, tokens, is_final, peer_metadata)
- `wait_one(id)`, `wait_all(ids)`, `collect_completed()`

### Provisioning

`MobActor` → `MobProvisioner` → session-backed provisioner → `session_service.create_session(req)`. Members are real sessions.

### Wiring

Definition has `WiringRules` with `role_wiring: [{a, b}]`. At spawn time, `MobActor` computes wiring targets and establishes bidirectional trust via comms.

### Flows

DAG of steps. Each step has a role, message, depends_on. `FlowEngine` dispatches turns to members via provisioner. Turn-driven mode: explicit `start_turn()`. Autonomous mode: inject via `event_injector`.

### Actor Decomposition

MobActor is composed of narrowly-scoped service objects:
- `MobLifecycleOwner` — state transitions (lock-free AtomicU8)
- `MobOrchestratorKernel` — coordinator binding, spawn/flow tracking
- `FlowRunKernel` — flow run creation and terminalization
- `SpawnPolicyService` — runtime policy swap (RwLock)
- `MobOpsAdapter` — bridges to `OpsLifecycleRegistry`
- `MobTaskBoardService` — task board validation and persistence

## Multimodal Content

`ContentBlock` (meerkat-core): `Text { text }` or `Image { media_type, data, source_path }`.
`ContentInput` (meerkat-core): `Text(String)` or `Blocks(Vec<ContentBlock>)`.
`ToolOutput` (meerkat-tools): `Json(Value)` or `Blocks(Vec<ContentBlock>)`.

## Tool Scoping

`ToolScope` manages runtime tool visibility with staged-then-applied semantics:
- **External filters** — staged via `ToolScopeHandle`, applied atomically at `CallingLlm` boundary.
- **Per-turn overlays** — `TurnToolOverlay` for mob flow step-scoped restrictions.
- **Live MCP mutation** — `McpRouter` staging queue, applied at turn boundary.
- **Composition rule** — most-restrictive wins.

## Comms Model

- **InprocRegistry** — process-global peer discovery.
- **CommsRuntime** — per-session, created by `AgentFactory::build_agent()` when `comms_name` is set.
- **Wiring** — bidirectional trust. Each peer has `TrustedPeerSpec`.
- **Unified trust state** — single `Arc<parking_lot::RwLock<TrustedPeers>>`.
- **Not all mob members are peers.** Wiring rules control which members can communicate.
- **Comms drain lifecycle** — background inbox consumption is a real lifecycle seam. Spawn/abort/exit feedback must return through the formal lifecycle path; turn-boundary suppression is a local projection of that truth.

## Key Architectural Invariants

1. **Never bypass build_agent()** — all agent construction goes through this pipeline.
2. **Never import implementations in business logic** — use traits from meerkat-core.
3. **No sub-agent system** — all multi-agent work goes through mobs. No SubAgentManager, no agent_spawn/agent_fork.
4. **Runtime conforms to machines** — runtime behavior must match verified machine schemas. No owner-crate `machines/mod.rs` re-exports; centralized `meerkat-machine-kernels` is the enforced design.
5. **Shell may execute mechanics; it may not invent seam semantics** — evidence capture is allowed, but feedback mapping, barrier membership, and terminal class must be protocol-constrained or machine-derived.
6. **Errors separate mechanism from policy** — `ToolError → AgentError → SessionError`.
7. **Wire types ≠ domain types** — `meerkat-contracts` owns wire format; `meerkat-core` owns domain types.
8. **Sessions are first-class, persistence is optional** — Ephemeral and Persistent share the same trait.
9. **No backward compatibility aliases** — 0.5 is a clean cut. No serde aliases for old names.
10. **No `.unwrap()`/`.expect()`/`panic!()` in library code** — use `?` propagation or explicit error handling.

## Key Files

- `meerkat-core/src/agent.rs` — agent loop state machine
- `meerkat-core/src/agent/state.rs` — run_loop, WaitingForOps dispatch
- `meerkat-core/src/ops_lifecycle.rs` — OpsLifecycleRegistry trait
- `meerkat/src/factory.rs` — `AgentFactory::build_agent()` (the pipeline)
- `meerkat/src/service_factory.rs` — `FactoryAgentBuilder`
- `meerkat-runtime/src/session_adapter.rs` — RuntimeSessionAdapter, RuntimeControlPlane impl
- `meerkat-runtime/src/policy_table.rs` — DefaultPolicyTable (policy matrix)
- `meerkat-runtime/src/ops_lifecycle.rs` — RuntimeOpsLifecycleRegistry
- `meerkat-runtime/src/silent_intent.rs` — silent intent override
- `meerkat-session/src/ephemeral.rs` — EphemeralSessionService
- `meerkat-mob/src/launch.rs` — MemberLaunchMode, ForkContext, BudgetSplitPolicy
- `meerkat-mob/src/runtime/handle.rs` — MobHandle, SpawnMemberSpec, MobMemberSnapshot
- `meerkat-mob/src/runtime/actor.rs` — MobActor (spawn, wire, flow)
- `meerkat-mob/src/runtime/tools.rs` — mob tool definitions
- `meerkat-machine-schema/src/catalog/` — machine definitions (10 machines)
- `meerkat-machine-kernels/src/runtime.rs` — GeneratedMachineKernel interpreter
- `meerkat-comms/src/runtime/comms_runtime.rs` — CommsRuntime

For detailed crate-by-crate reference, load: `references/crate_map.md`.
