# Meerkat Crate Map

## Dependency Order (bottom to top)

```
meerkat-models        (leaf crate — model catalog, provider profiles, parameter schemas)

meerkat-core          (depends on meerkat-models — pure types, traits, agent loop)
  ├── meerkat-contracts   (wire types, capability registry, error codes)
  ├── meerkat-client      (LLM providers: Anthropic, OpenAI, Gemini)
  ├── meerkat-store       (session persistence: SQLite, Jsonl, Memory, Redb)
  ├── meerkat-tools       (tool registry, builtins, shell)
  ├── meerkat-session     (session service: Ephemeral, Persistent)
  ├── meerkat-runtime     (runtime control plane, input lifecycle, policy engine)
  ├── meerkat-comms       (inter-agent: inproc, TCP, UDS, Ed25519)
  ├── meerkat-hooks       (hook engine: in-process, command, HTTP)
  ├── meerkat-skills      (skill loading: filesystem, git, HTTP, embedded)
  ├── meerkat-memory      (semantic memory: HNSW, simple)
  └── meerkat-mcp         (MCP protocol client)

meerkat-machine-schema    (formal machine/composition catalog + seam/handoff protocol metadata)
meerkat-machine-kernels   (generated kernel interpreter — centralized, no owner-crate re-exports)
meerkat-machine-codegen   (TLA+ generation, TLC verification, drift detection)

meerkat (facade)          (AgentFactory, wiring, re-exports)
  ├── meerkat-mob          (multi-agent: MobBuilder, MobActor, FlowEngine, member provisioning)
  ├── meerkat-mob-pack     (mobpack archive: signing, trust, validation)
  ├── meerkat-mob-mcp      (mob tools as MCP dispatcher)
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
| `AgentToolDispatcher` | Tool routing and dispatch | `CompositeDispatcher` (meerkat-tools), `ToolGateway` (meerkat-core), `EmptyToolDispatcher` (meerkat-tools) |
| `AgentSessionStore` | Session persistence | `StoreAdapter<S>` (meerkat-store) |
| `SessionService` | Full session lifecycle | `EphemeralSessionService<B>` (meerkat-session), `PersistentSessionService<B>` (meerkat-session) |
| `CommsRuntime` | Inter-agent communication | `meerkat_comms::CommsRuntime` |
| `HookEngine` | Hook execution | `DefaultHookEngine` (meerkat-hooks) |
| `SkillEngine` | Skill resolution + rendering | `DefaultSkillEngine` (meerkat-skills) |
| `Compactor` | Context compaction | `DefaultCompactor` (meerkat-session) |
| `MemoryStore` | Semantic memory | `HnswMemoryStore`, `SimpleMemoryStore` (meerkat-memory) |
| `OpsLifecycleRegistry` | Async operation tracking (wait_all, collect_completed, bounded retention, timestamps, concurrency) | `RuntimeOpsLifecycleRegistry` (meerkat-runtime) |

### Runtime traits (defined in meerkat-runtime)

| Trait | Purpose | Implementors |
|-------|---------|-------------|
| `RuntimeControlPlane` | Multi-session runtime control (ingest, retire, respawn, reset, recover, destroy) | `RuntimeSessionAdapter` |
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
| `MobEventStore` | Mob structural events | `InMemoryMobEventStore`, `RedbMobEventStore` |

### Mob types (defined in meerkat-mob/src/launch.rs)

| Type | Purpose |
|------|---------|
| `MemberLaunchMode` | Fresh / Resume / Fork (how to start a member) |
| `ForkContext` | FullHistory (CoW) / LastMessages(n) (how much history to fork) |
| `BudgetSplitPolicy` | Equal / Proportional / Remaining / Fixed(u64) |
| `MobMemberSnapshot` | Status, output, error, timestamps, tokens, is_final, peer_metadata |

### Multimodal types (defined in meerkat-core)

| Type | Purpose |
|------|---------|
| `ContentBlock` | Text or Image content unit |
| `ContentInput` | Prompt type for session requests |
| `ToolOutput` | Return type for `BuiltinTool::call()` (defined in meerkat-tools) |

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

Host mode is part of the session lifecycle contract. Runtime-backed and direct
session-service paths may realize it differently internally, but they are
expected to preserve the same drain lifecycle truth.
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

MobHandle::respawn(meerkat_id)
  ├── retire existing member (archive session, remove from roster)
  ├── enqueue spawn with same identity/profile/labels/mode
  └── new session ID, peer wiring needs re-establishment

MobHandle::run_flow(flow_id, params)
  ├── create MobRun record
  ├── topological sort steps
  ├── dispatch turns to matching members
  └── emit FlowCompleted/FlowFailed
```
