---
name: meerkat-architecture
description: "Internal architecture guide for the Meerkat agent platform. This skill should be used when understanding crate ownership, trait contracts, the agent construction pipeline, session service lifecycle, mob orchestration internals, comms wiring, or making cross-cutting architectural changes. Oriented toward AI agents and developers working on meerkat internals, not end users."
---

# Meerkat Internal Architecture

Meerkat is a library-first agent runtime. The execution pipeline is shared across all surfaces. Understanding crate ownership, trait contracts, and the agent construction pipeline is essential for making changes that don't break architectural invariants.

## Core Principles

1. **Infrastructure, not application** — the agent loop is a composable primitive with no opinions about prompts, tools, or output.
2. **Trait contracts own the architecture** — `meerkat-core` defines contracts; implementations live in satellite crates.
3. **Surfaces are interchangeable skins** — CLI, REST, RPC, MCP, WASM all route through `SessionService` → `AgentFactory::build_agent()`.
4. **Composition over configuration** — optional components are `Option<Arc<dyn Trait>>`, not feature-flagged defaults.
5. **Override-first resource injection** — `AgentBuildConfig` overrides take precedence over factory/config/filesystem resolution.

## Crate Ownership

| Crate | Owns | Key Trait |
|-------|------|-----------|
| `meerkat-core` | Agent loop, types, budget, retry, state machine, ALL trait contracts | `AgentLlmClient`, `AgentToolDispatcher`, `AgentSessionStore`, `SessionService`, `CommsRuntime`, `HookEngine` |
| `meerkat-client` | LLM providers (Anthropic, OpenAI, Gemini) | Implements `LlmClient` |
| `meerkat-store` | Session persistence (Jsonl, Memory, Redb) | Implements `SessionStore` |
| `meerkat-tools` | Tool registry, dispatch, builtins | Implements `AgentToolDispatcher` |
| `meerkat-session` | Session orchestration (Ephemeral, Persistent) | Implements `SessionService` |
| `meerkat-comms` | Inter-agent messaging (inproc, TCP, UDS) | Implements `CoreCommsRuntime` |
| `meerkat-mob` | Multi-agent orchestration (MobBuilder, FlowEngine) | `MobSessionService`, `MobProvisioner` |
| `meerkat-hooks` | Hook runtimes (in-process, command, HTTP) | Implements `HookEngine` |
| `meerkat-skills` | Skill loading (filesystem, git, HTTP, embedded) | Implements `SkillEngine` |
| `meerkat` (facade) | AgentFactory, FactoryAgentBuilder, build_ephemeral_service, re-exports | Wires everything together |

**Rule: meerkat-core has zero I/O dependencies.** All I/O happens in satellite crates.

## Agent Construction Pipeline

`AgentFactory::build_agent()` is the single entry point for ALL surfaces. 12 steps:

1. Validate host_mode
2. Resolve provider (infer from model or explicit)
3. Create LLM client (override > factory credentials > config)
4. Create LLM adapter (with event channel)
5. Resolve max_tokens
6a. Build skill engine (override > factory > config > filesystem)
6b. Create comms runtime (if comms_name set; inproc on wasm32)
6b. Build tool dispatcher (override > factory builtin builder)
7. Create session store (override > factory custom_store > feature-flag default)
8. Sub-agent comms inheritance (if enabled)
9. Compose tools with comms (add send/list_peers tools)
10. Resolve hooks (override > filesystem layered config)
11. Generate skill inventory
12. Build system prompt + AgentBuilder + SessionMetadata

**Precedence at every step:** `build_config override > factory field > config resolution > default`

## Session Service

All surfaces route through `SessionService` for the full lifecycle:

```
CreateSessionRequest → SessionService::create_session() → RunResult
  └── SessionAgentBuilder::build_agent() → SessionAgent
      └── AgentFactory::build_agent() → DynAgent
```

Two implementations:
- `EphemeralSessionService<B>` — in-memory, no persistence (WASM, testing)
- `PersistentSessionService<B>` — event-sourced (CLI, RPC, REST with redb)

`FactoryAgentBuilder` bridges `AgentFactory` into `SessionAgentBuilder`.

## Mob Orchestration

```
MobBuilder::new(definition, storage)
  .with_session_service(service)
  .allow_ephemeral_sessions(true)  // for non-persistent services
  .create() → MobHandle
```

`MobHandle` is clone-cheap (Arc-shared state). Sends commands to `MobActor` via channel.

**Provisioning:** `MobActor` → `MobProvisioner` → `SubagentBackend` → `session_service.create_session(req)`. Members are real sessions.

**Wiring:** Definition has `WiringRules` with `role_wiring: [{a, b}]`. At spawn time, `MobActor::spawn_wiring_targets()` computes peers, `do_wire()` establishes bidirectional trust via comms.

**Flows:** DAG of steps. Each step has a role, message, depends_on. `FlowEngine` dispatches turns to members via provisioner. Turn-driven mode: explicit `start_turn()`. Autonomous mode: inject via `event_injector`.

## Tool Scoping

`ToolScope` (meerkat-core) manages runtime tool visibility with staged-then-applied semantics:

- **External filters** — staged via `ToolScopeHandle`, applied atomically at `CallingLlm` boundary. Persisted in session metadata.
- **Per-turn overlays** — `TurnToolOverlay` set by mob flow engine for step-scoped restrictions. Ephemeral, not staged.
- **Live MCP mutation** — `McpRouter` has a staging queue (`stage_add/remove/reload`), applied at turn boundary. Servers in removal go through `Active → Removing (draining) → Removed`.
- **Composition rule** — most-restrictive wins: allow-lists intersect, deny-lists union, deny beats allow.
- **Agent loop integration** — `tool_scope.apply_staged()` called at top of each `CallingLlm` iteration before `stream_response()`. Emits `ToolConfigChanged` event and `[SYSTEM NOTICE]` user message on change.

**Key files:** `meerkat-core/src/tool_scope.rs`, `meerkat-core/src/agent/state.rs` (boundary apply), `meerkat-mcp/src/router.rs` (staged MCP ops).

## Comms Model

- **InprocRegistry** — process-global peer discovery. All sessions in the same process share it.
- **CommsRuntime** — per-session. Created by `AgentFactory::build_agent()` when `comms_name` is set.
- **Wiring** — bidirectional trust. Each peer has a `TrustedPeerSpec` with name, public key, address.
- **Not all mob members are peers.** Wiring rules control which members can communicate.
- **Cross-mob communication** — agents with `external_addressable: true` are visible in InprocRegistry to agents in other mobs.

## Key Architectural Invariants

1. **Never bypass build_agent()** — all agent construction goes through this pipeline.
2. **Never import implementations in business logic** — use traits from meerkat-core.
3. **Errors separate mechanism from policy** — `ToolError → AgentError → SessionError`.
4. **Wire types ≠ domain types** — `meerkat-contracts` owns wire format; `meerkat-core` owns domain types.
5. **Sessions are first-class, persistence is optional** — Ephemeral and Persistent share the same trait.
6. **Platform differences are overrides, not cfg-gates in business logic** — if you need `#[cfg(wasm32)]` inside `build_agent()`, the abstraction is wrong.

## Key Files

For detailed crate-by-crate reference, load: `references/crate_map.md`.

- `meerkat-core/src/agent.rs` — agent loop state machine
- `meerkat/src/factory.rs` — `AgentFactory::build_agent()` (the pipeline)
- `meerkat/src/service_factory.rs` — `FactoryAgentBuilder` (bridges factory to session service)
- `meerkat-session/src/ephemeral.rs` — `EphemeralSessionService`
- `meerkat-mob/src/runtime/actor.rs` — mob actor (spawn, wire, flow dispatch)
- `meerkat-mob/src/definition.rs` — `MobDefinition`, `WiringRules`, `FlowSpec`
- `meerkat-comms/src/runtime/comms_runtime.rs` — `CommsRuntime`
