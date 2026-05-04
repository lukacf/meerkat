# Agent Construction, Content, and Tool Scoping

Load this reference when working on `AgentFactory::build_agent()`, agent builder wiring, multimodal content, or runtime tool visibility.

## AgentFactory::build_agent()

The single entry point for ALL surfaces. Key steps:

1. Validate `keep_alive`
2. Resolve provider (infer from model or explicit)
3. Resolve credentials through `ProviderRuntimeRegistry` against the build's realm + binding (or the persisted `SessionLlmIdentity.auth_binding` on hot-swap). The registry is the **only** legitimate path to provider credentials — no `std::env::var` reads outside it. The published `AuthMachine` lease is what the LLM client consumes; refresh is owned by `AuthMachine` per binding (Valid → Expiring → Refreshing → Valid|ReauthRequired).
4. Create LLM client (`build_config` override > registry-resolved binding lease > deny). There is no env-fallback at this layer — env keys are absorbed by the registry as a synthetic default binding upstream.
5. Create LLM adapter (with event channel and event tap)
6. Resolve `max_tokens`
7. Build skill engine, tool dispatcher, and any session-scoped task store
8. Create session store (contract lives in `meerkat-core`; impls live in `meerkat-store`)
9. Compose tools with comms, then optionally late-bind mob tools via `MobToolsFactory`
10. Resolve hooks (override > filesystem layered config)
11. Build system prompt + `AgentBuilder` + wire memory / compactor / skill engine / ops-lifecycle / event-tap / checkpointer
12. Build agent, set `SessionMetadata` (persist override intent + `auth_binding`, not flattened booleans)

The factory validates `bindings.session_id == session.id()` for `SessionOwned` builds. Cross-wired bindings are rejected with `BuildAgentError::Config`.

**Precedence at every step:** `build_config override > factory field > registry/config resolution > default`. For credentials specifically, "registry resolution" means the realm/binding lookup against `ProviderRuntimeRegistry`, not raw config or env reads.

**Dynamic tooling gotcha:** if a child dispatcher can change between turns (callback tools, agent mob tools), compose with `DynamicToolComposite` rather than a gateway that snapshots tool definitions once at construction.

## Multimodal Content

- `ContentBlock` (meerkat-core): `Text { text }`, `Image { media_type, ... }`, or `Video { media_type, duration_ms, ... }`
- `ContentInput` (meerkat-core): `Text(String)` or `Blocks(Vec<ContentBlock>)`
- `ToolOutput` (meerkat-tools): `Json(Value)` or `Blocks(Vec<ContentBlock>)`
- `AssistantBlock::Image` (meerkat-core): canonical generated assistant image output; stores `image_id`, `blob_ref`, dimensions, revised prompt disposition, and provider metadata.

## Assistant Image Generation

`generate_image` is a privileged built-in dispatch path, not an ordinary external tool. `AgentFactory` wires it only when the build has an image-generation machine, planner, executor, and blob store. The machine owns lifecycle semantics; provider crates own image target profiles and provider-specific parameters; the tool layer only normalizes the model-facing request, calls the planner/executor, commits blobs, and appends assistant image blocks after tool results to preserve provider tool-call adjacency.

Generated images must be surfaced through transcript history plus blob retrieval. SDKs should parse `AssistantBlock::Image` into typed image fields and fetch bytes through `blob/get`; do not inline generated image bytes into history.

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
