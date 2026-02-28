# Meerkat Crate Map

## Dependency Order (bottom to top)

```
meerkat-core          (no I/O deps — pure types, traits, agent loop)
  ├── meerkat-contracts   (wire types, capability registry, error codes)
  ├── meerkat-client      (LLM providers: Anthropic, OpenAI, Gemini)
  ├── meerkat-store       (session persistence: Jsonl, Memory, Redb)
  ├── meerkat-tools       (tool registry, builtins, shell, comms tools)
  ├── meerkat-session     (session service: Ephemeral, Persistent)
  ├── meerkat-comms       (inter-agent: inproc, TCP, UDS, Ed25519)
  ├── meerkat-hooks       (hook engine: in-process, command, HTTP)
  ├── meerkat-skills      (skill loading: filesystem, git, HTTP, embedded)
  ├── meerkat-memory      (semantic memory: HNSW, simple)
  └── meerkat-mcp         (MCP protocol client)

meerkat (facade)          (AgentFactory, wiring, re-exports)
  ├── meerkat-mob          (multi-agent: MobBuilder, MobActor, FlowEngine)
  ├── meerkat-mob-pack     (mobpack archive: signing, trust, validation)
  ├── meerkat-mob-mcp      (mob tools as MCP dispatcher)
  └── meerkat-web-runtime  (WASM embedded runtime)

Surface binaries:
  ├── meerkat-cli (rkat)
  ├── meerkat-rpc (rkat-rpc)
  ├── meerkat-rest (rkat-rest)
  └── meerkat-mcp-server (rkat-mcp)
```

## Key Traits (all in meerkat-core)

| Trait | Purpose | Implementors |
|-------|---------|-------------|
| `AgentLlmClient` | LLM provider abstraction | `LlmClientAdapter` |
| `AgentToolDispatcher` | Tool routing | `CompositeDispatcher`, `ToolGateway`, `EmptyToolDispatcher` |
| `AgentSessionStore` | Session persistence | `StoreAdapter<S>`, no-op stores |
| `SessionService` | Full session lifecycle | `EphemeralSessionService<B>`, `PersistentSessionService<B>` |
| `SessionAgentBuilder` | Agent construction from request | `FactoryAgentBuilder` |
| `SessionAgent` | Running agent with session access | `FactoryAgent` |
| `CommsRuntime` | Inter-agent communication | `meerkat_comms::CommsRuntime` |
| `HookEngine` | Hook execution | `DefaultHookEngine` |
| `SkillEngine` | Skill resolution + rendering | `DefaultSkillEngine` |
| `Compactor` | Context compaction | `DefaultCompactor` |
| `MemoryStore` | Semantic memory | `HnswMemoryStore`, `SimpleMemoryStore` |
| `MobSessionService` | Session service + comms access for mobs | `EphemeralSessionService<B>`, `PersistentSessionService<B>` |
| `MobProvisioner` | Member spawn/retire/turn | `SubagentBackend`, `MultiBackendProvisioner` |
| `MobEventStore` | Mob structural events | `InMemoryMobEventStore`, `RedbMobEventStore` |

## Agent Loop State Machine

`CallingLlm` → `WaitingForOps` → `DrainingEvents` → `Completed`

With branches: `ErrorRecovery`, `Cancelling`

## Session Lifecycle

```
create_session(req) → RunResult
  ├── build_agent(req) → Agent
  ├── spawn session task
  ├── store handle (with comms_runtime, event_injector)
  └── run first turn (or defer)

start_turn(id, prompt) → RunResult
  ├── acquire turn lock
  ├── send command to session task
  └── await result

interrupt(id)
  └── set interrupt flag, notify session task

archive(id)
  └── remove handle, drop session task
```

## Mob Lifecycle

```
MobBuilder::create() → MobHandle
  ├── validate definition
  ├── emit MobCreated event
  ├── create provisioner (SubagentBackend wrapping session_service)
  └── spawn MobActor task

MobHandle::spawn(profile, meerkat_id)
  ├── build AgentBuildConfig from profile
  ├── provisioner.provision_member() → session_service.create_session()
  ├── add to roster
  ├── compute wiring targets from definition.wiring
  └── do_wire() for each target pair

MobHandle::run_flow(flow_id, params)
  ├── create MobRun record
  ├── topological sort steps
  ├── for each step: dispatch turn to matching member
  ├── collect results
  └── emit FlowCompleted/FlowFailed
```
