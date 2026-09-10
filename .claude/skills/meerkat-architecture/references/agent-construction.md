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
11. Build system prompt + `AgentBuilder` + wire memory / compactor / skill engine / ops-lifecycle / event-tap / provisional persistence hook
12. Build agent, set `SessionMetadata` (persist override intent + `auth_binding`, not flattened booleans)

The factory validates `bindings.session_id == session.id()` for `SessionOwned` builds. Cross-wired bindings are rejected with `BuildAgentError::Config`.

**Precedence at every step:** `build_config override > factory field > registry/config resolution > default`. For credentials specifically, "registry resolution" means the realm/binding lookup against `ProviderRuntimeRegistry`, not raw config or env reads.

**Dynamic tooling gotcha:** if a child dispatcher can change between turns (callback tools, agent mob tools), compose with `DynamicToolComposite` rather than a gateway that snapshots tool definitions once at construction.

## Model Fallback Chain

The fallback chain is an agent-facing LLM client decorator assembled by
`AgentFactory`, but it is not an authority for recovery semantics. Keep these
ownership boundaries intact:

- `meerkat` facade resolves only explicit, enabled `[model_fallback]` targets
  against the effective `ModelRegistry`. Default is off, with no catalog walk;
  `meerkat-core` remains provider-data-free.
- `ProviderRuntimeRegistry` is still the only credential path for every
  fallback target. Provider boundary is checked before foreign auth lookup.
  Exact typed `auth_binding` and shared-account affinity are preserved, including
  same-model/provider credential failover. Generated AuthMachine credential-use
  admission, not JSON pressure or lease acquisition, owns current usability.
- The generated turn recovery authority classifies whether an LLM failure is
  recoverable. The fallback client only selects the next prebuilt candidate; it
  must not replace generated recoverability or make provider calls while
  preparing a switch. Policy trigger/threshold filtering follows that verdict.
- `Agent::apply_model_fallback_switch()` owns applying the switch: rotate auth
  lease binding, apply request policy, persist `SessionLlmIdentity`, recompute
  provider params, preserve admitted output reserve, filter tools by the target model's
  capability base filter, and append the hidden `model_fallback` system notice.
- The retry is allowed only when core decides it is pre-stream safe. Do not add
  hidden retry paths outside generated recovery or after user-visible
  text/reasoning stream output escaped. Transport and empty output are not
  default migration triggers.
- Extraction fallback must merge the target request policy with the current
  extraction overrides: reapply structured output for the new provider and keep
  provider-native web search disabled.
- Tool visibility projections must use the active model's
  `active_capability_base_filter()` on every call and visible-tool snapshot, so
  later sticky fallback turns and control-plane reads agree.
- Selection and precommit use `ModelFallbackRequest` over actual materialized
  messages/tools/params/output schema. Use canonical context forecast/pressure
  owners, never bytes-as-tokens or only the last ContextExceeded error.
- Typed skips survive all-candidate rejection. Staged is not committed.
  A target-bound provenance marker rides the existing sticky control delta and
  compensation transaction, never a separate identity store. New proven unsafe
  resumes can hold for reconfiguration; explicit identity overrides clear the
  marker. Old notice/brain-swap history is not fallback authority or migration.

## Multimodal Content

- `ContentBlock` (meerkat-core): `Text { text }`, `Image { media_type, ... }`,
  `Video { media_type, duration_ms, ... }`, `Structured { data }`, or
  `SkillContext { skill_key, text }`
- `ContentInput` (meerkat-core): `Text(String)` or `Blocks(Vec<ContentBlock>)`
- `ToolOutput` (meerkat-tools): `Json(Value)`,
  `JsonWithEffects { value, session_effects }`, or
  `Blocks(Vec<ContentBlock>)`
- `AssistantBlock::Image` (meerkat-core): canonical generated assistant image output; stores `image_id`, `blob_ref`, dimensions, revised prompt disposition, and provider metadata.

## Assistant Image Generation

`generate_image` is a privileged built-in dispatch path, not an ordinary external tool. `AgentFactory` wires it only when the build has an image-generation machine, planner, executor, and blob store. The machine owns lifecycle semantics; provider crates own image target profiles and provider-specific parameters; the tool layer only normalizes the model-facing request, calls the planner/executor, commits blobs, and appends assistant image blocks after tool results to preserve provider tool-call adjacency.

Dependency availability alone does not expose the tool: the session-owned
machine, planner, executor, and blob store must all be wired. In the ordinary
`CompositeDispatcher` path, `override_image_generation = Inherit` resolves
visible when that substrate is present; `Disable` hides it and `Enable` makes
the intent explicit. One narrow early-return path differs: when both builtins
and shell are disabled, the factory constructs a composite solely for image
generation only when the override is explicitly `Enable`. A `ToolScope` allow
list can narrow a visible catalog but cannot enable a policy-disabled category.

For an `Auto` image target, the planner first uses the build's configured
`image_generation_provider`; only when that is absent does it fall back to the
typed `SessionModelRoutingStatus.session_provider`. It never infers provider
identity from a model string. References to the current turn's generated
images travel as the typed `CurrentTurnImageRef` newtype
(`meerkat-core/src/agent.rs`), not raw indices.

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
- `meerkat-core/src/types.rs` - `ContentBlock`, `ContentInput`
