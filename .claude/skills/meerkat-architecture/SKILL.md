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
| `meerkat-models` | Model catalog, provider profiles, parameter schemas (leaf crate, no meerkat deps) | — |
| `meerkat-core` | Agent loop, types, budget, retry, state machine, ALL trait contracts | `AgentLlmClient`, `AgentToolDispatcher` (incl. `bind_wait_interrupt`, `supports_wait_interrupt`), `AgentSessionStore`, `SessionService` (incl. `set_session_client`), `SessionAgent` (incl. `replace_client`), `CommsRuntime`, `HookEngine` |
| `meerkat-client` | LLM providers (Anthropic, OpenAI, Gemini) | Implements `AgentLlmClient` (via `LlmClientAdapter`) |
| `meerkat-store` | Session persistence (SQLite, Jsonl, Memory, Redb) | Implements `SessionStore` |
| `meerkat-tools` | Tool registry, dispatch, builtins (task tools, utility helpers like `apply_patch`, shell/comms/delegated-work compatibility surfaces) | Implements `AgentToolDispatcher` |
| `meerkat-session` | Session orchestration (Ephemeral, Persistent) | Implements `SessionService` |
| `meerkat-comms` | Inter-agent messaging (inproc, TCP, UDS) | Implements `CommsRuntime` |
| `meerkat-mob` | Multi-agent orchestration (MobBuilder, FlowEngine) | `MobSessionService`, `MobProvisioner` (mob-local traits) |
| `meerkat-mob-pack` | Mobpack archive format, signing, trust policies, validation | — |
| `meerkat-mob-mcp` | Expose mob tools as MCP interface | `MobMcpState`, `MobMcpDispatcher` |
| `meerkat-web-runtime` | WASM browser deployment (wasm_bindgen exports) | — |
| `meerkat-hooks` | Hook runtimes (in-process, command, HTTP) | Implements `HookEngine` |
| `meerkat-skills` | Skill loading (filesystem, git, HTTP, embedded) | Implements `SkillEngine` |
| `meerkat` (facade) | AgentFactory, FactoryAgentBuilder, build_ephemeral_service, re-exports | Wires everything together |

**Rule: meerkat-core has zero I/O dependencies.** All I/O happens in satellite crates.

## Agent Construction Pipeline

`AgentFactory::build_agent()` is the single entry point for ALL surfaces. Key steps:

1. Validate host_mode
2. Resolve provider (infer from model or explicit)
3. Create LLM client (override > factory credentials > config)
4. Create LLM adapter (with event channel and event tap)
5. Resolve max_tokens
6a. Build skill engine (override > factory > config > filesystem)
6b. Build tool dispatcher (override > factory builtin builder); create comms runtime (if comms_name set; inproc on wasm32); wire delegated child-session comms inheritance for legacy compatibility only
7. Create session store (override > factory custom_store > feature-flag default)
9. Compose tools with comms (add send/list_peers tools)
10. Resolve hooks (override > filesystem layered config)
11. Generate skill inventory
12. Build system prompt + AgentBuilder + wire memory/compactor/skill-engine/event-tap/checkpointer
13. Build agent
14. Set SessionMetadata

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
- `PersistentSessionService<B>` — event-sourced persistent orchestration (CLI, RPC, REST, MCP; typically backed by sqlite or redb through `PersistenceBundle`)

`FactoryAgentBuilder` bridges `AgentFactory` into `SessionAgentBuilder`.

## Persistence pairing

Persistent realm opening is backend-owned in the `meerkat` facade through `PersistenceBundle`.

- Surfaces open a realm bundle, not a raw session store plus ad hoc runtime companion.
- The bundle carries the paired `SessionStore`, optional `RuntimeStore`, and matching `RuntimeSessionAdapter`.
- SQLite is now the default persistent realm backend when compiled; redb remains explicit.
- Backend-specific pairing logic should stay in the persistence seam, not in `SessionStore` and not in `meerkat-runtime`.

## Mob Orchestration

```
MobBuilder::new(definition, storage)
  .with_session_service(service)
  .allow_ephemeral_sessions(true)  // for non-persistent services
  .create() → MobHandle
```

`MobHandle` is clone-cheap (Arc-shared state). Sends commands to `MobActor` via channel.

**Provisioning:** `MobActor` → `MobProvisioner` → session-backed provisioner (`SubagentBackend` in the current code) → `session_service.create_session(req)`. Members are real sessions.

**Wiring:** Definition has `WiringRules` with `role_wiring: [{a, b}]`. At spawn time, `MobActor::spawn_wiring_targets()` computes peers, `do_wire()` establishes bidirectional trust via comms.

**Flows:** DAG of steps. Each step has a role, message, depends_on. `FlowEngine` dispatches turns to members via provisioner. Turn-driven mode: explicit `start_turn()`. Autonomous mode: inject via `event_injector`.

## Multimodal Content

`ContentBlock` (meerkat-core) is the unit of rich content. Two variants:
- `ContentBlock::Text { text }` — plain text.
- `ContentBlock::Image { media_type, data, source_path }` — base64-encoded image. `source_path` is domain-only (stripped from wire).

`ContentInput` (meerkat-core) is the prompt type for `CreateSessionRequest.prompt` and `StartTurnRequest.prompt`:
- `ContentInput::Text(String)` — plain text (implements `From<&str>` and `From<String>`).
- `ContentInput::Blocks(Vec<ContentBlock>)` — multimodal content blocks.

`ToolOutput` (meerkat-tools) is the return type for `BuiltinTool::call()`:
- `ToolOutput::Json(Value)` — standard JSON result, serialized to text.
- `ToolOutput::Blocks(Vec<ContentBlock>)` — rich content blocks injected into `ToolResult.content`.

`ToolResult.content` is `Vec<ContentBlock>` (replacing the former single-string model).
`UserMessage.content` is `Vec<ContentBlock>`.

**Capability gating:** `ModelProfile` has `vision: bool` and `image_tool_results: bool`. The `view_image` builtin tool is hidden via `ToolScope` when either is false. Per-provider: Anthropic (both true), OpenAI (vision true, image_tool_results false), Gemini (both true).

**Wire types:** `WireContentBlock` (no `source_path`), `WireContentInput`, `WireToolResultContent` in meerkat-contracts.

**Comms:** `MessageKind`, `CommsContent`, `InteractionContent` have `blocks: Option<Vec<ContentBlock>>` alongside `body: String`.

## Tool Scoping

`ToolScope` (meerkat-core) manages runtime tool visibility with staged-then-applied semantics:

- **External filters** — staged via `ToolScopeHandle`, applied atomically at `CallingLlm` boundary. Persisted in session metadata.
- **Per-turn overlays** — `TurnToolOverlay` set by mob flow engine for step-scoped restrictions. Ephemeral, not staged.
- **Live MCP mutation** — `McpRouter` has a staging queue (`stage_add/remove/reload`), applied at turn boundary. Servers in removal go through `Active → Removing (draining) → Removed`.
- **Composition rule** — most-restrictive wins: allow-lists intersect, deny-lists union, deny beats allow.
- **Agent loop integration** — `tool_scope.apply_staged()` called at top of each `CallingLlm` iteration before `stream_response()`. Emits `ToolConfigChanged` event and `[SYSTEM NOTICE]` user message on change.

**Key files:** `meerkat-core/src/tool_scope.rs`, `meerkat-core/src/agent/state.rs` (boundary apply), `meerkat-mcp/src/router.rs` (staged MCP ops).

### Non-blocking MCP loading pipeline

MCP servers connect in parallel via `stage_add()` + `apply_staged()` which spawns background tasks per server. Completions flow through a pending channel pattern:

1. **`McpRouter`** — `pending_tx`/`pending_rx` channel, `pending_servers` map, generation-based staleness detection.
2. **`McpRouterAdapter`** — bridges to `AgentToolDispatcher`. `has_pending` `AtomicBool` (Acquire/Release) gates fast-path skip of write lock in `poll_external_updates()`.
3. **Agent loop** — `CallingLlm` calls `tools.poll_external_updates()` before tool capture. Returns `ExternalToolUpdate { notices, pending }`. Emits `ToolConfigChanged` for each notice. Manages `[MCP_PENDING]` synthetic user message lifecycle (strip + re-add on every iteration).
4. **Forwarding** — `poll_external_updates()` forwarded through `CompositeDispatcher` → `ToolGateway` (aggregates + deduplicates by `(server, operation, status)`) → `FilteredToolDispatcher` (passthrough).
5. **`wait_until_ready(timeout)`** — poll loop on `McpRouterAdapter` for surfaces that need blocking (CLI `--wait-for-mcp`, SDK `AgentBuildConfig.wait_for_mcp`).

**Key files:** `meerkat-mcp/src/router.rs`, `meerkat-mcp/src/adapter.rs`, `meerkat-core/src/agent/state.rs` (CallingLlm boundary), `meerkat-core/src/gateway.rs` (ToolGateway dedup).

## Comms Model

- **InprocRegistry** — process-global peer discovery. All sessions in the same process share it.
- **CommsRuntime** — per-session. Created by `AgentFactory::build_agent()` when `comms_name` is set.
- **Wiring** — bidirectional trust. Each peer has a `TrustedPeerSpec` with name, public key, address.
- **Unified trust state** — single `Arc<parking_lot::RwLock<TrustedPeers>>` shared by Router, `IngressClassificationContext`, and `trusted_peers_shared()` callers. Mutations through any handle are immediately visible to classification.
- **Ingress classification** — single-pass classification via `IngressClassificationContext`. Untrusted items dropped at ingress (snapshot semantics). `actionable_input_notify` fires only for `ActionableMessage`/`ActionableRequest`, preventing false wakes from acks, lifecycle traffic, and plain events.
- **Not all mob members are peers.** Wiring rules control which members can communicate.
- **Cross-mob communication** — agents with `external_addressable: true` are visible in InprocRegistry to agents in other mobs.

## Mid-Session Model Hot-Swap

- `Agent::replace_client(&mut self, client)` swaps the LLM client on a live agent without rebuilding.
- `SessionAgent::replace_client()` trait method (default no-op) bridges into the session layer.
- `SessionService::set_session_client(session_id, client)` routes through the session service (default returns `SessionError::Unsupported`).
- `SessionError::Unsupported(String)` — new error variant for capability negotiation. Ephemeral sessions reject hot-swap; persistent sessions support it.
- RPC `turn/start` accepts `model`/`provider`/`provider_params` on materialized sessions, builds a new client via `AgentFactory::build_llm_adapter()`, and hot-swaps before the turn.

## Key Architectural Invariants

1. **Never bypass build_agent()** — all agent construction goes through this pipeline.
2. **Never import implementations in business logic** — use traits from meerkat-core.
3. **Errors separate mechanism from policy** — `ToolError → AgentError → SessionError`.
4. **Wire types ≠ domain types** — `meerkat-contracts` owns wire format; `meerkat-core` owns domain types.
5. **Sessions are first-class, persistence is optional** — Ephemeral and Persistent share the same trait.
6. **Capability negotiation via `Unsupported`** — optional service methods default to `Err(SessionError::Unsupported(...))`. Callers probe before committing.
7. **Platform differences are overrides, not cfg-gates in business logic** — if you need `#[cfg(wasm32)]` inside `build_agent()`, the abstraction is wrong.

## Key Files

For detailed crate-by-crate reference, load: `references/crate_map.md`.

- `meerkat-core/src/agent.rs` — agent loop state machine
- `meerkat/src/factory.rs` — `AgentFactory::build_agent()` (the pipeline)
- `meerkat/src/service_factory.rs` — `FactoryAgentBuilder` (bridges factory to session service)
- `meerkat-session/src/ephemeral.rs` — `EphemeralSessionService`
- `meerkat-mob/src/runtime/actor.rs` — mob actor (spawn, wire, flow dispatch)
- `meerkat-mob/src/definition.rs` — `MobDefinition`, `WiringRules`, `FlowSpec`
- `meerkat-comms/src/runtime/comms_runtime.rs` — `CommsRuntime`
