# Meerkat Crate Map

## Dependency Order (bottom to top)

```
meerkat-sqlite            (shared SQLite mechanics: connection profiles, meerkat_schema migration
                           ledger, JsonColumnBytes codec, per-operation maintenance-fence guards,
                           error classification — rusqlite only, no meerkat deps; consumed by
                           store/runtime/tools/memory/workgraph/mob)
meerkat-models            (canonical provider model catalog/capabilities data; core stays provider-free)
meerkat-llm-core          (LLM client trait surface, streaming primitives shared by providers)
meerkat-auth-core         (token stores, OAuth helpers, MCP OAuth discovery/DCR/PKCE/refresh,
                           cloud authorizers — no meerkat-core deps)

meerkat-core              (pure types, traits, agent loop, session-store contract, model_profile vocabulary + ModelCatalog mechanics,
                           DSL handle traits, StorageLayout path authority + realm-id-first dual-root
                           resolution, DurabilityClass vocabulary, StorageMigrator diagnose seam)
  ├── meerkat-capabilities    (typed capability vocabulary, feature-owned declaration collection)
  ├── meerkat-contracts       (wire types, error codes, generated surface schema projections)
  ├── meerkat-store-conformance (published storage conformance harness: per-trait capability
                                profiles, capability-discovery, append-only media, legacy-data,
                                blob/artifact chapters — depends only on meerkat-core)
  ├── meerkat-anthropic       (Anthropic streaming client, implements AgentLlmClient through llm-core)
  ├── meerkat-openai          (OpenAI client, including realtime transport — implements AgentLlmClient)
  ├── meerkat-gemini          (Gemini client, including inline video — implements AgentLlmClient)
  ├── meerkat-providers       (compatibility shim: ProviderRuntimeRegistry surface + cloud authorizer wiring)
  ├── meerkat-client          (compatibility shim: re-exports provider crates — do NOT add new code here)
  ├── meerkat-store           (session persistence: SQLite, Jsonl, Memory; realm manifest v2 pinning,
                                disk doctor/migrate implementations — built on meerkat-sqlite)
  ├── meerkat-tools           (tool registry, builtins, shell, session-scoped task store)
  ├── meerkat-session         (session service: Ephemeral, Persistent)
  ├── meerkat-runtime         (runtime control plane, policy engine, completion-feed wake,
                                DSL handle impls)
  ├── meerkat-workgraph       (realm-scoped durable WorkGraph service, stores, tools, read surface)
  ├── meerkat-live            (LiveAdapterHost, live projection sink, WebSocket transport)
  ├── meerkat-comms           (inter-agent: inproc, TCP, UDS, Ed25519)
  ├── meerkat-hooks           (hook engine: in-process, command, HTTP)
  ├── meerkat-skills          (skill loading: filesystem, git, HTTP, embedded)
  ├── meerkat-memory          (semantic memory: HNSW, simple)
  └── meerkat-mcp             (MCP protocol client)

meerkat-machine-dsl-core      (DSL primitives: machine/composition modeling base)
meerkat-machine-derive        (proc macros for machine DSL)
meerkat-machine-dsl           (DSL frontend — used by catalog sources)
meerkat-machine-schema        (formal machine/composition catalog + seam/handoff protocol metadata)
meerkat-machine-kernels       (generated kernel interpreter — centralized, no owner-crate re-exports)
meerkat-machine-codegen       (TLA+ generation, TLC verification, drift detection)

meerkat (facade)              (AgentFactory, FactoryAgentBuilder, persistence helpers, re-exports,
                                SessionLlmReconfigureHost wiring, RealmStorageProvider seam +
                                DiskStorageProvider + fail-closed durability enforcement)
  ├── meerkat-mob              (multi-agent: MobBuilder, MobActor, FlowEngine, FlowFrameEngine,
                                  member provisioning, identity-first binding, supervisor bridge)
  ├── meerkat-mob-pack         (mobpack archive: signing, trust, validation)
  ├── meerkat-mob-mcp          (mob tools as MCP dispatcher + agent delegation surface, profile tools)
  ├── meerkat-schedule         (scheduler: cron/interval triggers, occurrence lifecycle, delivery, schedule tools)
  └── meerkat-web-runtime      (WASM embedded runtime — wasm_bindgen exports)

Surface binaries:
  ├── meerkat-cli           → rkat                       (CLI)
  ├── meerkat-rpc           → rkat-rpc                   (JSON-RPC; stdio default, --tcp opt-in)
  ├── meerkat-rest          → rkat-rest                   (REST + SSE)
  └── meerkat-mcp-server    → rkat-mcp                    (MCP server)
```

There are no separate public reduced-surface binaries. Reduced-surface distributions are source builds of the same surface crates with a narrower Cargo feature set.

## Storage Flow (0.8.4 unification arc)

```
RuntimeBootstrap (surface flags)
  → StorageLayout::resolve (meerkat-core: path authority; realm-id-first
     dual-root resolution — explicit --state-root wins, single existing
     candidate is used where it lies, both = typed RealmSplitBrain refusal,
     neither = surface default; candidate_roots feed first-start reservation)
  → RealmStorageProvider::open (meerkat facade seam; DiskStorageProvider is
     the built-in — realm manifest v2 pin: builtin backend or external
     provider pin, refused typed on mismatch/future format)
  → RealmStoreSet (one provider supplies session/runtime/schedule/workgraph/
     blob/artifact stores + one DurabilityDeclaration per slot;
     enforce_fail_closed_durability refuses undeclared non-persistent
     durable slots at startup)
  → PersistenceBundle (facade composition; store-only seam — mob storage
     stays mob-owned to avoid the meerkat → meerkat-mob → meerkat cycle)
```

Beneath the stores, `meerkat-sqlite` owns the shared mechanics every SQLite
store opens through: named connection profiles (DDL-free opens), the
`meerkat_schema(domain, version)` ledger (pinned concurrent-open protocol,
typed `SchemaFromTheFuture` refusal checked preflight before WAL), sibling
`<file>.mfence` per-operation fence guards (offline `rkat storage migrate`
takes the exclusive side), and `classify_sqlite_error`. The
`meerkat_core::StorageMigrator` diagnose seam is what `rkat storage doctor`
renders (disk implementation: `meerkat-store/src/doctor.rs`; the CLI storage
verbs dispatch before runtime-scope resolution). Backends prove the store
contracts against `meerkat-store-conformance` (per-trait capability
profiles; the in-repo stores run the same suite in
`meerkat-store/tests/conformance.rs`).

## Key Traits

### Core traits (defined in meerkat-core)

| Trait | Purpose | Implementors |
|-------|---------|-------------|
| `AgentLlmClient` | LLM provider abstraction | `LlmClientAdapter` (meerkat-llm-core; re-exported by the meerkat-client shim) |
| `AgentToolDispatcher` | Tool routing and dispatch | `CompositeDispatcher` (meerkat-tools), `DynamicToolComposite` / `ToolGateway` (meerkat-core), `AgentMobToolSurface` (meerkat-mob-mcp), `EmptyToolDispatcher` (meerkat-tools) |
| `AgentSessionStore` | Agent-loop-facing persistence adapter | `StoreAdapter<S>` (meerkat-store) |
| `SessionStore` | Canonical session persistence contract for backend authors | `MemoryStore`, `JsonlStore`, `SqliteSessionStore` (meerkat-store) |
| `SessionService` | Full substrate lifecycle; the runtime control plane (`MeerkatMachine`, meerkat-runtime) layers canonical runtime semantics on top | `EphemeralSessionService<B>` (meerkat-session), `PersistentSessionService<B>` (meerkat-session) |
| `CommsRuntime` | Inter-agent communication | `meerkat_comms::CommsRuntime` |
| `HookEngine` | Hook execution | `DefaultHookEngine` (meerkat-hooks) |
| `SkillEngine` | Skill resolution + rendering | `DefaultSkillEngine` (meerkat-skills) |
| `Compactor` | Context compaction | `DefaultCompactor` (meerkat-session) |
| `CompactionCurator` | Host-supplied compaction summary producer (substitutes the summary LLM call; failure is typed `CuratorFailed`, no LLM fallback) | host-provided via `AgentBuildConfig.compaction_curator_override` |
| `MemoryStore` | Semantic memory: index/search + lifecycle (`drop_scope`, paged `enumerate_scoped`; defaults are typed `Unsupported`) | `HnswMemoryStore` (lazy per-scope loading), `SimpleMemoryStore` (meerkat-memory) |
| `OpsLifecycleRegistry` | Async operation tracking (wait_all, collect_completed, bounded retention, timestamps, concurrency, detached wake) | `RuntimeOpsLifecycleRegistry` (meerkat-runtime) |
| `MobToolsFactory` | Late-binding session-scoped mob tool construction | `AgentMobToolSurfaceFactory` (meerkat-mob-mcp) |
| `WorkGraphStore` | Durable realm-scoped work item, edge, claim, event, and snapshot storage | `MemoryWorkGraphStore`, `SqliteWorkGraphStore` (meerkat-workgraph) |
| `StorageMigrator` | Shape-stable storage diagnose seam (`diagnose(&DiagnoseScope) → StorageDiagnosis`; mutation verbs arrive as defaulted methods) | `DiskStorageMigrator` (meerkat-store/src/doctor.rs) |

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

### Storage traits (defined in the meerkat facade)

| Trait | Purpose | Implementors |
|-------|---------|-------------|
| `RealmStorageProvider` | One provider supplies all durable stores for a realm (`open(RealmOpenContext) → RealmStoreSet` with per-slot `DurabilityDeclaration`s over the six required domains; optional `migrator()` hook) | `DiskStorageProvider` (meerkat/src/storage_provider.rs); downstream remote/mobkit providers |

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
| `MobMemberSnapshot` | Public: status, output_preview, error, tokens_used, is_final, current_session_id, peer_connectivity, kickoff, external_member, resolved_capabilities. Binding atoms (agent_identity, agent_runtime_id, fence_token, current_bridge_session_id) are `pub(crate)` + `#[serde(skip)]` — bridge-internal, never app-facing |
| `FrameSpec` / `FlowNodeSpec` / `RepeatUntilSpec` | Frame-based flow graphs and repeat-until loop nodes |
| `MobDefinition.owner_bridge_session_id` / `MobDefinition.is_implicit` | Session-scoped mob ownership, access control, implicit cleanup |

### Multimodal types (defined in meerkat-core)

| Type | Purpose |
|------|---------|
| `ContentBlock` | Text, Image, or Video content unit |
| `ContentInput` | Prompt type for session requests |
| `ToolOutput` | Return type for `BuiltinTool::call()` (defined in meerkat-tools) |
| `GenerateImageRequest` / `ImageGenerationToolResult` | Universal model-facing image-generation request/result contract |
| `ImageGenerationProviderProfile` | Provider-owned image target profile; implemented by image-capable provider crates |
| `AssistantBlock::Image` | Canonical generated-image transcript block with durable `image_id` and `blob_ref` |

### Post-0.5.0 load-bearing types

| Type | Purpose |
|------|---------|
| `ToolCategoryOverride` | Tri-state tooling intent across save/resume (`Inherit` / `Enable` / `Disable`) |
| `PeerInput.handling_mode` | Typed per-input policy override for actionable peer traffic only |
| `RuntimeBuildMode` | Explicit runtime ownership mode for builds (`SessionOwned` / `StandaloneEphemeral`) |
| `SessionRuntimeBindings` | Runtime-backed session bindings carrying `session_id`, `epoch_id`, ops lifecycle, cursor state, tool visibility owner, and all session-owned DSL handles |
| `RuntimeEpochId` / `EpochCursorState` | Epoch-local runtime continuity identity and consumer cursor state |
| `RuntimeCompletionFeed` | Read handle to the completion feed (implements `CompletionFeed`) — meerkat-runtime |
| `PersistedOpsSnapshot` | Serializable snapshot for durable epoch recovery — meerkat-runtime |
| `RuntimeBindingsError` | Error type for `prepare_bindings()` — meerkat-runtime |
| `CompletionFeed` | Trait for monotonic completion event log — meerkat-core |
| `CompletionEntry` | Single completion event in the feed — meerkat-core |

### 0.7.12 additions (PR #821, MobKit upstream asks)

| Type | Purpose |
|------|---------|
| `TranscriptUserRole::InjectedContext` | Slot-derived typed role for host-attached ambient context (separate user-channel messages; excluded from memory indexing; save-guard remains CompactionSummary-only) |
| `MemoryIndexExclusion::{CompactionSummary, InjectedContext}` | Typed indexing exclusions consulted by `Message::indexable_content()` via `transcript_role` |
| `StartTurnRequest.injected_context` / `CreateSessionRequest.injected_context` / `WorkSpec.injected_context` | Typed delivery slots for injected context on the submit-work paths (service, RPC/REST params, mob work lane, `BridgeDeliveryPayload` for remote members) |
| `CompactionWindow` / `CuratedCompactionSummary` | Curator inputs (same as the LLM path) and validated non-empty summary newtype — meerkat-core/src/compact.rs |
| `MemoryOwner`-scoped `drop_scope` / `MemoryEnumerationRequest` / `MemoryEnumerationPage` / `MemoryRecord` | MemoryStore lifecycle + enumeration vocabulary (raw-row offset paging; `source_overlap` + `indexed_after` filters) |
| `SenderContentTaint` / `SendTaintOverride` | Core-owned comms content-taint vocabulary; envelope field is inside the signed `MessageKind` region; per-send override is tri-state (absent = inherit runtime declaration); `SystemNoticeBlock::Comms.sender_taint` is the transcript carrier |
| `ToolExecutionPolicy` / `ExecutionPolicyGatedDispatcher` | Sealed resolved form of `ops::ToolAccessPolicy` + list-preserving call-level execution gate (deny = ordinary `access_denied` tool error; wraps outermost in the factory; `Inherit` resolves to the parent's effective policy) |
| `TargetBinding::HostRunnable` / `ScheduleRunnableHost` / `HostRunnableRegistry` | Host-registered schedule runnables delivered through the normal occurrence lifecycle (meerkat-schedule/src/runnable.rs) |
| `SessionTranscriptRevisionListQuery` / `SessionServiceHistoryExt::list_transcript_revisions` | Revision-list read (RPC `session/transcript_revisions`); head reads remain `read_transcript_revision` + `RevisionSelector::Current` |
| `HookToolCall.provenance` / `HookLlmResponse.server_tool_content` | Synchronous dispatch-time projections of `ToolProvenance` / `ServerToolKind` for foreground hook classification |

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

`keep_alive` is runtime-owned session behavior: `MeerkatMachine`
(meerkat-runtime) owns keep-alive and drain semantics. Direct substrate usage
does not carry them; runtime-backed surfaces get them by lowering into the
runtime control plane, never by implementing them surface-side.

Detached background ops now wake idle keep_alive sessions through the
completion feed and runtime loop via `ContinuationInput`; surface-local waker
tasks are a smell.

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
