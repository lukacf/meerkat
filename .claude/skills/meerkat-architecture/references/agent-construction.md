# Agent Construction, Content, and Tool Scoping

Load this reference when working on `AgentFactory::build_agent()`, agent builder wiring, multimodal content, or runtime tool visibility.

## AgentFactory::build_agent()

The single entry point for ALL surfaces. Key steps:

1. Validate `keep_alive`
2. Resolve provider (infer from model or explicit)
3. Create LLM client (override > factory credentials > config)
4. Create LLM adapter (with event channel and event tap)
5. Resolve `max_tokens`
6. Build skill engine, tool dispatcher, and any session-scoped task store
7. Create session store (contract lives in `meerkat-core`; impls live in `meerkat-store`)
8. Compose tools with comms, then optionally late-bind mob tools via `MobToolsFactory`
9. Resolve hooks (override > filesystem layered config)
10. Build system prompt + `AgentBuilder` + wire memory / compactor / skill engine / ops-lifecycle / event-tap / checkpointer
11. Build agent, set `SessionMetadata` (persist override intent, not flattened booleans)

The factory validates `bindings.session_id == session.id()` for `SessionOwned` builds. Cross-wired bindings are rejected with `BuildAgentError::Config`.

**Precedence at every step:** `build_config override > factory field > config resolution > default`

**Dynamic tooling gotcha:** if a child dispatcher can change between turns (callback tools, agent mob tools), compose with `DynamicToolComposite` rather than a gateway that snapshots tool definitions once at construction.

## Multimodal Content

- `ContentBlock` (meerkat-core): `Text { text }` or `Image { media_type, data, source_path }`
- `ContentInput` (meerkat-core): `Text(String)` or `Blocks(Vec<ContentBlock>)`
- `ToolOutput` (meerkat-tools): `Json(Value)` or `Blocks(Vec<ContentBlock>)`

## Tool Scoping

`ToolScope` manages runtime tool visibility with staged-then-applied semantics:

- **External filters** — staged via `ToolScopeHandle`, applied atomically at `CallingLlm` boundary
- **Per-turn overlays** — `TurnToolOverlay` for mob flow step-scoped restrictions
- **Live MCP mutation** — `McpRouter` staging queue, applied at turn boundary
- **Composition rule** — most-restrictive wins
- **Dynamic child surfaces** — use `DynamicToolComposite` when child dispatchers can change between turns (callback tools, agent mob tools). Static tool-list caching is a regression risk.

Tool visibility state lives in MeerkatMachine DSL (`active_filter`, `staged_filter`, `active_visibility_revision`, `staged_visibility_revision`). The `ToolVisibilityOwner` trait exposes a read projection; `StagePersistentFilter`, `RequestDeferredTools`, `PublishCommittedVisibleSet` inputs drive state changes.

## Key files

- `meerkat/src/factory.rs` — `AgentFactory`, `DynAgent`, `AgentBuildConfig`
- `meerkat/src/service_factory.rs` — `FactoryAgentBuilder`, `FactoryAgent`, `build_ephemeral_service`
- `meerkat-core/src/agent.rs` — `Agent`, `AgentExecutionSnapshot` (reads turn state from DSL via `TurnStateHandle`)
- `meerkat-core/src/agent/runner.rs` — `run_inner`, run-loop reset
- `meerkat-core/src/agent/builder.rs` — `AgentBuilder`
- `meerkat-core/src/agent/state.rs` — `run_loop`, `WaitingForOps` dispatch
- `meerkat-core/src/tool_scope.rs` — runtime tool visibility
- `meerkat-core/src/content.rs` — `ContentBlock`, `ContentInput`
