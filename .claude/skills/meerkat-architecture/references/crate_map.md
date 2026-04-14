# Meerkat Crate Map

## Dependency Order (bottom to top)

```
meerkat-models        (leaf crate — model catalog, provider profiles, parameter schemas)

meerkat-core          (depends on meerkat-models — pure types, traits, agent loop, session-store contract)
  ├── meerkat-contracts   (wire types, capability registry, error codes)
  ├── meerkat-client      (LLM providers: Anthropic, OpenAI, Gemini)
  ├── meerkat-store       (session persistence: SQLite, Jsonl, Memory)
  ├── meerkat-tools       (tool registry, builtins, shell, session-scoped task store)
  ├── meerkat-session     (session service: Ephemeral, Persistent)
  ├── meerkat-runtime     (runtime control plane, input lifecycle, policy engine, detached wake)
  ├── meerkat-comms       (inter-agent: inproc, TCP, UDS, Ed25519)
  ├── meerkat-hooks       (hook engine: in-process, command, HTTP)
  ├── meerkat-skills      (skill loading: filesystem, git, HTTP, embedded)
  ├── meerkat-memory      (semantic memory: HNSW, simple)
  └── meerkat-mcp         (MCP protocol client)

meerkat-machine-schema    (formal machine/composition catalog + seam/handoff protocol metadata)
meerkat-machine-kernels   (generated kernel interpreter — centralized, no owner-crate re-exports)
meerkat-machine-codegen   (TLA+ generation, TLC verification, drift detection)

meerkat (facade)          (AgentFactory, wiring, re-exports)
  ├── meerkat-mob          (multi-agent: MobBuilder, MobActor, FlowEngine, FlowFrameEngine, member provisioning)
  ├── meerkat-mob-pack     (mobpack archive: signing, trust, validation)
  ├── meerkat-mob-mcp      (mob tools as MCP dispatcher + agent delegation surface)
  ├── meerkat-schedule     (scheduler: cron/interval triggers, occurrence lifecycle, delivery, schedule tools)
  └── meerkat-web-runtime  (WASM embedded runtime)

Surface binaries:
  ├── meerkat-cli (rkat)
  ├── meerkat-rpc (rkat-rpc)
  ├── meerkat-rest (rkat-rest)
  └── meerkat-mcp-server (rkat-mcp)
```

## Key Traits

### Core traits (defined in meerkat-core)

| Trait | Purpose | Implementors |
|-------|---------|-------------|
| `AgentLlmClient` | LLM provider abstraction | `LlmClientAdapter` (meerkat-client) |
| `AgentToolDispatcher` | Tool routing and dispatch | `CompositeDispatcher` (meerkat-tools), `DynamicToolComposite` / `ToolGateway` (meerkat-core), `AgentMobToolSurface` (meerkat-mob-mcp), `EmptyToolDispatcher` (meerkat-tools) |
| `AgentSessionStore` | Agent-loop-facing persistence adapter | `StoreAdapter<S>` (meerkat-store) |
| `SessionStore` | Canonical session persistence contract for backend authors | `MemoryStore`, `JsonlStore`, `SqliteSessionStore` (meerkat-store) |
| `SessionService` | Full substrate lifecycle; runtime-backed surfaces layer canonical runtime semantics on top | `EphemeralSessionService<B>` (meerkat-session), `PersistentSessionService<B>` (meerkat-session) |
| `CommsRuntime` | Inter-agent communication | `meerkat_comms::CommsRuntime` |
| `HookEngine` | Hook execution | `DefaultHookEngine` (meerkat-hooks) |
| `SkillEngine` | Skill resolution + rendering | `DefaultSkillEngine` (meerkat-skills) |
| `Compactor` | Context compaction | `DefaultCompactor` (meerkat-session) |
| `MemoryStore` | Semantic memory | `HnswMemoryStore`, `SimpleMemoryStore` (meerkat-memory) |
| `OpsLifecycleRegistry` | Async operation tracking (wait_all, collect_completed, bounded retention, timestamps, concurrency, detached wake) | `RuntimeOpsLifecycleRegistry` (meerkat-runtime) |
| `MobToolsFactory` | Late-binding session-scoped mob tool construction | `AgentMobToolSurfaceFactory` (meerkat-mob-mcp) |

### Runtime traits (defined in meerkat-runtime)

| Trait | Purpose | Implementors |
|-------|---------|-------------|
| `RuntimeControlPlane` | Multi-session runtime control (ingest, retire, respawn, reset, recover, destroy) | `MeerkatMachine` |
| `RuntimeDriver` | Per-session input lifecycle (accept, run events, control, recover, retire, destroy) | `EphemeralRuntimeDriver`, `PersistentRuntimeDriver` |

### Session traits (defined in meerkat-session)

| Trait | Purpose | Implementors |
|-------|---------|-------------|
| `SessionAgentBuilder` | Agent construction from request | `FactoryAgentBuilder` (meerkat facade) |
| `SessionAgent` | Running agent with session access | `FactoryAgent` (meerkat facade) |

### Mob traits (defined in meerkat-mob)

| Trait | Purpose | Implementors |
|-------|---------|-------------|
| `MobSessionService` | Session service + comms access for mobs | `EphemeralSessionService<B>`, `PersistentSessionService<B>` |
| `MobProvisioner` | Member spawn/retire/turn | `MultiBackendProvisioner`, session-backed provisioner |
| `MobEventStore` | Mob structural events | `InMemoryMobEventStore`, `SqliteMobEventStore` |
| `MobRunStore` | Flow run persistence, kernel state, frame/loop snapshots | `InMemoryMobRunStore`, `SqliteMobRunStore` |
| `MobSpecStore` | Mob definition persistence + revision CAS | `InMemoryMobSpecStore`, `SqliteMobSpecStore` |

### Mob / flow types (defined in meerkat-mob)

| Type | Purpose |
|------|---------|
| `MemberLaunchMode` | Fresh / Resume / Fork (how to start a member) |
| `ForkContext` | FullHistory (CoW) / LastMessages(n) (how much history to fork) |
| `BudgetSplitPolicy` | Equal / Proportional / Remaining / Fixed(u64) |
| `MobMemberSnapshot` | status, agent_runtime_id, fence_token, output_preview, error, tokens_used, is_final, peer_connectivity, kickoff |
| `FrameSpec` / `FlowNodeSpec` / `RepeatUntilSpec` | Frame-based flow graphs and repeat-until loop nodes |
| `MobDefinition.owner_bridge_session_id` / `MobDefinition.is_implicit` | Session-scoped mob ownership, access control, implicit cleanup |

### Multimodal types (defined in meerkat-core)

| Type | Purpose |
|------|---------|
| `ContentBlock` | Text or Image content unit |
| `ContentInput` | Prompt type for session requests |
| `ToolOutput` | Return type for `BuiltinTool::call()` (defined in meerkat-tools) |

### Post-0.5.0 load-bearing types

| Type | Purpose |
|------|---------|
| `ToolCategoryOverride` | Tri-state tooling intent across save/resume (`Inherit` / `Enable` / `Disable`) |
| `DetachedWakeState` | Runtime-owned idle keep-alive wake for detached background ops |
| `PeerInput.handling_mode` | Typed per-input policy override for actionable peer traffic only |
| `RuntimeBuildMode` | Explicit runtime ownership mode for builds (`SessionOwned` / `StandaloneEphemeral`) |
| `SessionRuntimeBindings` | Runtime-backed session bindings carrying `session_id`, `epoch_id`, ops lifecycle, and cursor state |
| `RuntimeEpochId` / `EpochCursorState` | Epoch-local runtime continuity identity and consumer cursor state |
| `RuntimeCompletionFeed` | Read handle to the completion feed (implements `CompletionFeed`) — meerkat-runtime |
| `PersistedOpsSnapshot` | Serializable snapshot for durable epoch recovery — meerkat-runtime |
| `RuntimeBindingsError` | Error type for `prepare_bindings()` — meerkat-runtime |
| `CompletionFeed` | Trait for monotonic completion event log — meerkat-core |
| `CompletionEntry` | Single completion event in the feed — meerkat-core |

## Agent Loop State Machine

`CallingLlm` → `WaitingForOps` → `DrainingEvents` → `Completed`

With branches: `ErrorRecovery`, `Cancelling`

**WaitingForOps** is a real barrier state, but the load-bearing truth is barrier membership, not a raw bag of operation IDs. The architecture is moving toward typed async-op references and explicit wait policy so long-lived detached work does not accidentally become a turn barrier.

## Session Lifecycle

```
create_session(req) → RunResult
  ├── build_agent(req) → Agent
  ├── spawn session task
  ├── register RuntimeDriver (optional)
  └── run first turn (or defer)

start_turn(id, prompt, handling_mode) → RunResult
  ├── acquire turn lock
  ├── send command to session task
  └── await result

interrupt(id) → set interrupt flag, notify session task
archive(id) → remove handle, drop session task

`keep_alive` is runtime-owned session behavior. Direct substrate usage does not
own runtime drain semantics; runtime-backed surfaces do.

Detached background ops now wake idle keep_alive sessions through the runtime
loop via `ContinuationInput`; surface-local waker tasks are a smell.

Runtime-backed builds should go through:

`prepare_bindings(session_id)` → `SessionRuntimeBindings` →
`SessionBuildOptions.runtime_build_mode = RuntimeBuildMode::SessionOwned(...)`

Standalone/testing/embedded paths should opt into
`RuntimeBuildMode::StandaloneEphemeral` explicitly.
```

## Mob Lifecycle

```
MobBuilder::create() → MobHandle
  ├── validate definition
  ├── emit MobCreated event
  ├── create provisioner
  └── spawn MobActor task

MobHandle::spawn(spec)
  ├── resolve launch mode (Fresh/Resume/Fork)
  ├── build AgentBuildConfig from profile
  ├── provisioner.provision_member() → session_service.create_session()
  ├── add to roster, compute wiring targets
  └── do_wire() for each target pair

MobHandle::respawn(identity: AgentIdentity)
  ├── retire existing member (archive session, remove from roster)
  ├── enqueue spawn with same identity/profile/labels/mode
  └── new FenceToken issued, peer wiring needs re-establishment

MobHandle::run_flow(flow_id, params)
  ├── create MobRun record
  ├── topological sort steps OR dispatch to frame runtime when `flow_spec.root` exists
  ├── canonical step execution via `execute_step_with_all_guards()`
  └── emit FlowCompleted/FlowFailed
```

- `mob/create` is definition-only; prefabs are gone.
- `delegate` / implicit mobs are tracked by canonical `owner_bridge_session_id` + `is_implicit` fields and cleaned up by `destroy_session_mobs()`.

## Post-0.5.0 Deltas

- `SessionStore` moved into `meerkat-core`; `meerkat-store` now re-exports and implements it.
- Agent delegation tools are mob-backed and session-scoped via `owner_bridge_session_id` / `is_implicit`; operator authority is injected, not ambient.
- Tooling persistence is tri-state via `ToolCategoryOverride`; resume paths must preserve `Inherit`.
- Mob persistence switched to SQLite/WAL; the previous exclusive-handle mob store is gone.
- Flow loops are machine-backed through `FlowFrameKernel`, `LoopIterationKernel` (internal sub-machines of MobMachine), and the `FlowFrameEngine` runtime.
- `FlowEngine::execute_step_with_all_guards()` is the single canonical step path for both flat and frame execution.
