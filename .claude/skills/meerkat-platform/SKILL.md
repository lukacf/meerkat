---
name: meerkat-platform
description: "Comprehensive guide for building applications with the Meerkat agent platform. Covers all surfaces (CLI, REST, RPC, MCP, Python SDK, TypeScript SDK, Rust SDK), configuration, sessions, streaming, skills, hooks, memory, inter-agent communication, and mob orchestration (spawn, fork, helpers, flows). This skill should be used when users ask how to integrate with Meerkat, build agents, configure the runtime, use the SDK, set up multi-agent systems, or work with any Meerkat feature."
---

# Meerkat Platform Guide

Meerkat (`rkat`) is a library-first agent runtime exposed through multiple surfaces. The execution pipeline is shared, state is realm-scoped, and the current runtime-backed semantics are the default product path.

If the request is about upgrading older integrations or mental models, load:

- `references/migration_0_5.md`

Use that migration guide for:

- `host_mode` -> `keep_alive`
- `--host` -> `--keep-alive`
- runtime-backed ownership vs substrate
- request cancellation / commit semantics
- external-event behavior
- SDK and wire-shape changes

## Realm-first model

`realm_id` is the sharing key across all surfaces.

- Same `realm_id` => shared sessions/config/backend.
- Different `realm_id` => strict isolation.
- Backend (`sqlite` or `jsonl`) is pinned per realm in `realm_manifest.json`.

Default persistent backend:

- New persistent realms default to `sqlite` when sqlite support is compiled.
- `sqlite` is the normal same-realm multi-process backend.
- `jsonl` remains explicit for inspectable file-backed persistence.

Default realm behavior:

- CLI (`run/resume/sessions`): workspace-derived stable realm.
- RPC/REST/MCP/SDK: new opaque realm unless explicitly provided.

Use explicit realm to share:

```bash
rkat --realm team-alpha run "Plan release"
rkat-rpc --realm team-alpha
rkat-rest --realm team-alpha
rkat-mcp --realm team-alpha
```

## Runtime-backed vs standalone

Meerkat uses an explicit runtime-binding contract:

- runtime-backed surfaces should call `MeerkatMachine::prepare_bindings(session_id)`
- those bindings flow into `SessionBuildOptions.runtime_build_mode = RuntimeBuildMode::SessionOwned(bindings)`
- standalone/testing/embedded paths should opt into `RuntimeBuildMode::StandaloneEphemeral` explicitly

Use this mental model when helping users:

- **runtime-backed**: CLI, REST, RPC, MCP server, long-lived mob/member surfaces, keep-alive, comms drain, runtime wake/recovery
- **standalone/embedded**: direct in-memory substrate, WASM embedded sessions, examples/tests that intentionally do not want runtime-owned semantics

## Mob runtime binding (identity-first)

`RuntimeBinding` separates "what kind of backend" from "which specific runtime":

- `MobBackendKind` (definition/profile level): `Session` or `External` — declares what kind of backend a mob uses
- `RuntimeBinding` (spawn/provision level): `Session` or `External { peer_id, address }` — binds a specific member to a concrete runtime

External members must provide `RuntimeBinding::External` at spawn time with the real process comms identity. The provisioner uses that backend peer binding for the concrete `peer_id` and `address` instead of deriving them from the placeholder session.

The bridge session (placeholder) still exists for lifecycle transport (notifications, kickoff events) within the orchestrator process. `trusted_peer_spec` uses the bridge key for transport trust, not `BackendPeer.peer_id` (which is the real identity).

This is the first step toward identity-first mobs where `AgentIdentity` is the durable member identity and everything else (runtime incarnation, comms, bridge/session transport) is hidden binding detail.

## Surfaces

| Surface | Protocol | Use Case |
|---------|----------|----------|
| CLI | Shell commands | Developer workflows, scripting |
| REST | HTTP/JSON | Services and language-agnostic clients |
| RPC | JSON-RPC 2.0 (stdio) | IDE integration and SDK backend |
| MCP | Model Context Protocol | Expose Meerkat as tools |
| Python SDK | Async Python over RPC | Python applications |
| TypeScript SDK | TypeScript over RPC | Node.js applications |
| Web SDK | `@rkat/web` (WASM) | Browser applications |
| Rust SDK | Direct library API | Embedded Rust systems |

For full per-surface schemas and examples, load: `references/api_reference.md`.
For detailed mob behavior across all surfaces, load: `references/mobs.md`.

## Mob behavior (current contract)

- CLI `run`/`resume` compose `mob_*` tools through `meerkat-mob-mcp` dispatcher integration.
- CLI `mob ...` is the explicit lifecycle surface for persisted mob registry operations.
- RPC/REST/MCP server/Python SDK/TypeScript SDK expose mob capability via the same dispatcher composition model (`SessionBuildOptions.external_tools`) in host integrations.
- Member runtime default is `autonomous_host` when `runtime_mode` is omitted; `turn_driven` is explicit opt-in for controlled dispatch paths.
- Spawned mob members use deferred initial turn semantics; mob actor lifecycle starts autonomous loops explicitly after spawn registration.
- Mob persistence is SQLite/WAL-backed (`MobStorage::persistent()` opens `SqliteMobStores`). In-memory storage is used for tests and WASM. The previous exclusive-handle mob store has been removed.
- Prefabs are gone. All mob creation uses `MobDefinition` only (CLI, REST, RPC, MCP, SDKs).
- Agent-facing delegation tools (`delegate`, `mob_create`, `mob_destroy`, `mob_spawn_member`, `mob_retire_member`, `mob_check_member`, `mob_list_members`, `mob_list`, `mob_wire`, `mob_unwire`) are provided by `AgentMobToolSurface` in `meerkat-mob-mcp`. These tools let agents spawn and manage mob members through implicit session-owned mobs, and create/remove peer-to-peer comms links between members.
- Portable mob artifacts are available through mobpack (`rkat mob pack/deploy/inspect/validate`) and browser deployment (`rkat mob web build`).
- Public realtime attachment capability is named `realtime`, not `voice`: surfaces should describe `ModelCapabilities.realtime`, `runtime/realtime_attachment_status`, `runtime/realtime_attachment_statuses`, and `mob/member_status.realtime_attachment_status`. Realtime transport is capability-driven — there is no caller-initiated attach/detach RPC; set the session's model to a realtime-capable one (e.g. `gpt-realtime-1.5`) and the runtime manages attach/detach automatically.
- OpenAI realtime integration is an internal runtime-backed companion, not a
  public protocol authority: host/facade composition wires it, and RPC/REST
  surfaces expose only the `runtime/realtime_attachment_status(es)` status
  projections plus the `realtime/*` bootstrap methods.

### Realtime voice attachment

Realtime is a delivery mode of the session's LLM, not a separate
subsystem. Enable it by pointing the session at a realtime-capable
model (currently `gpt-realtime-1.5`, with `gpt-realtime` as a compatibility alias); the runtime reads
`ModelCapabilities.realtime` and attaches an OpenAI Realtime transport
automatically. The session retains a single canonical history; audio
commits into it at turn boundaries.

User-facing entry point: `docs/guides/realtime.mdx`.
Internal architecture reference: `.claude/skills/meerkat-architecture/references/realtime-attachment.md`.

**Public API surface (all surfaces):**

| Surface | Enable realtime | Observe status | Open audio channel |
|---------|-----------------|----------------|---------------------|
| JSON-RPC | `session/create` (or profile default) with `model: "gpt-realtime-1.5"` | `runtime/realtime_attachment_status`, `runtime/realtime_attachment_statuses`, `mob/member_status.realtime_attachment_status` | `realtime/open_info` |
| REST | `POST /sessions` with `{"model":"gpt-realtime-1.5"}` | `GET /runtime/{id}/realtime_attachment_status` | `realtime/open_info` via RPC |
| MCP (public) | (set model via host composition) | `meerkat_realtime_status`, `meerkat_realtime_capabilities` | `meerkat_realtime_open_info` |
| Python SDK | `client.create_session(model="gpt-realtime-1.5", ...)` | `client.runtime_realtime_attachment_status(...)` | `client.realtime_open_info(...)` |
| TypeScript SDK | `client.createSession({ model: "gpt-realtime-1.5", ... })` | `client.runtimeRealtimeAttachmentStatus(...)` | `client.realtimeOpenInfo(...)` |
| Rust | `SessionBuildOptions { model: Some("gpt-realtime-1.5".into()), ... }` | `MeerkatMachine::realtime_attachment_status` | provider integration in `meerkat-client` |

There is **no** caller-initiated attach/detach RPC. Transport lifecycle
is a function of the session's resolved model capability.

**Capability source.** `ModelCapabilities.realtime: bool` is set per
model in `meerkat-models`. OpenAI capability derivation matches any
model name containing `realtime` (production: canonical `gpt-realtime-1.5`; `gpt-realtime` remains a compatibility alias).
Gemini derivation matches `*-live*` (no production entries today).
Anthropic and self-hosted default to `false`.

**Attachment states** (`RealtimeAttachmentStatus` enum):
`Unattached` (model is not realtime-capable), `IntentPresentUnbound`,
`BindingNotReady`, `BindingReady` (stable, audio flowing),
`ReplacementPending`, `ReattachRequired` (stable, requires a
reconfigure retry).

**Authority token.** `RealtimeAttachmentSignalAuthority { session_id,
authority_epoch }` is minted by the runtime (via internal
`BeginRealtimeBinding` / `ReplaceRealtimeBinding` DSL transitions) and
required on every provider callback. The DSL `PublishRealtimeSignal`
guard validates the epoch against the current binding — stale tokens
are rejected before any mutation.

**Live-topology reconfigure.** `MeerkatMachine::reconfigure_live_topology`
(invoked by host composition through `SessionLlmReconfigureHost`) swaps
provider/model on a session via a 6-phase DSL-guarded flow
(Idle → Reconfiguring → Detached → HostIdentityApplied →
HostVisibilityApplied → Idle). The final step branches on the target
model's `realtime` capability — realtime-capable → `attach_live`
(mints fresh authority); non-realtime → `ValidationFailed` (binding is
gone). Two typed failure modes:
`AbortLiveTopologyBeforeDetach` (pre-detach failure, binding preserved)
and `FailLiveTopologyAfterDetach` (post-detach failure, binding gone,
reattach required).

**Known limitations:**
- `gpt-realtime-1.5` is the canonical production realtime-capable model today (`gpt-realtime` remains a compatibility alias).
- Single realtime binding per session; per-member realtime in mobs is
  achieved by spawning members on realtime-capable profiles.
- Idle sessions cannot host a binding — the transport is brought up as
  part of the executor binding, so start a turn first or spawn via a mob.
- No remote-callable reconfigure RPC; `SessionLlmReconfigureHost` is the
  integration seam for host-driven model swaps.

### Mob lifecycle (standard/default usage)

Primary CLI usage is tool-driven through `run`/`resume`:

```bash
rkat run "create a mob with a lead and workers, then wire and report status"
rkat run --resume <session_id> "retire worker-2 and add worker-4"
```

Where needed, the current helper-oriented CLI mob surface is:

```bash
rkat mob spawn-helper <mob_id> <prompt> [--agent-identity <id>] [--profile <profile>]
rkat mob fork-helper <mob_id> <source_member> <prompt> [--agent-identity <id>] [--profile <profile>]
rkat mob member-status <mob_id> <agent_identity>
rkat mob respawn <mob_id> <agent_identity> [--message <msg>]
rkat mob run-flow <mob_id> --flow <flow_id> [--params <json>]
rkat mob flow-status <mob_id> <run_id>
rkat mob wait-kickoff <mob_id> [--member <agent_identity>...]
```

### Mobpack + web build quick paths

```bash
# Build portable artifact
rkat mob pack ./mobs/release-triage -o ./dist/release-triage.mobpack --sign ./keys/release.key
rkat mob inspect ./dist/release-triage.mobpack
rkat mob validate ./dist/release-triage.mobpack

# Deploy with trust policy
rkat mob deploy ./dist/release-triage.mobpack "triage latest regressions" --trust-policy strict

# Browser bundle
cargo install wasm-pack
rkat mob web build ./dist/release-triage.mobpack -o ./dist/release-triage-web
```

Web build env overrides:

- `RKAT_WASM_PACK_BIN`: explicit wasm-pack binary path
- `RKAT_WEB_RUNTIME_CRATE_DIR`: explicit web runtime crate directory for build

### WASM runtime + Web SDK (embedded deployment target)

The `meerkat-web-runtime` crate is an **embedded deployment target for mobpacks** — the JavaScript equivalent of the Rust SDK. It compiles the real meerkat agent stack to wasm32, routing through the same `AgentFactory::build_agent()` pipeline as all other surfaces.

**`@rkat/web` npm package** (`sdks/web/`): TypeScript wrapper with `MeerkatRuntime`, `Mob`, `Session`, `EventSubscription` classes. Ships with pre-built WASM binary and a Node.js provider proxy.

**Not a protocol server** — unlike RPC/REST, the WASM runtime is deployed INTO a host application (browser). A mobpack defines what it is. The host provides credentials and drives interaction.

**Provider proxy** (`sdks/web/proxy/`): Node.js auth-injecting reverse proxy so API keys stay server-side. The WASM runtime uses per-provider base URLs (`anthropicBaseUrl`, `openaiBaseUrl`, `geminiBaseUrl`) to point at the proxy natively — no fetch override needed.

```bash
ANTHROPIC_API_KEY=sk-... npx @rkat/web proxy --port 3100
```

```typescript
import { MeerkatRuntime } from '@rkat/web';
import * as wasm from '@rkat/web/wasm/meerkat_web_runtime.js';

const runtime = await MeerkatRuntime.init(wasm, {
  anthropicApiKey: 'proxy',
  anthropicBaseUrl: 'http://localhost:3100/anthropic',
});
const mob = await runtime.createMob(definition);
await mob.spawn([{ profile: 'worker', agent_identity: 'w1' }]);
```

**Architecture:**
- Routes through `EphemeralSessionService<FactoryAgentBuilder>` → `AgentFactory::build_agent()` with override-first resource injection
- Uses explicit standalone runtime mode (`RuntimeBuildMode::StandaloneEphemeral`) for embedded sessions rather than relying on runtime-backed bindings
- `MobMcpState` handles all mob lifecycle operations (same state manager as native MCP mob surface)
- `tokio_with_wasm` provides the async runtime (single-threaded, JS event loop backed)
- `reqwest` uses browser `fetch` on wasm32
- `InprocRegistry` provides peer discovery for comms (all sessions share one process-global registry)
- 10 dependency crates compile for wasm32 (core, client, store, tools, session, hooks, comms, mob, mob-mcp, facade)
- Gemini uses `x-goog-api-key` header (not query param) for auth

**Fully available on wasm32:**
- Agent loop (streaming, retries, error recovery, budget enforcement)
- All three LLM providers (Anthropic, OpenAI, Gemini) via browser fetch
- Session lifecycle (EphemeralSessionService)
- Mob orchestration (MobBuilder, MobActor, FlowEngine, FlowFrameEngine, in-memory storage)
- Comms (inproc — InprocRegistry, Ed25519 signing, peer discovery)
- Tool dispatch (task tools, `datetime`, comms tools, skill tools — no shell and no filesystem-mutating builtins)
- Skills (embedded + memory sources from mobpack)
- Hooks (in-process + HTTP — no command hooks)
- Config (in-memory, programmatic)
- Compaction (DefaultCompactor)

**Not available on wasm32 (inherent browser limitations):**
- Filesystem (config loading, AGENTS.md, skill files, session persistence)
- Shell tool, process spawning
- MCP protocol client (rmcp blocked by tokio/mio)
- Network comms (TCP/UDS sockets — inproc only)

**WASM API surface:** See the meerkat-wasm skill (`references/api_surface.md`) for the complete export table. The 0.5 notes in `references/migration_0_5.md` are legacy migration context, not the current release contract.

**Build:**
```bash
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' wasm-pack build meerkat-web-runtime --target web --out-dir <dir>
# Note: --out-dir is relative to crate root (meerkat-web-runtime/), not workspace root
```

### Mob flows (DAG runtime)

Flow commands are now part of the CLI mob lifecycle:

```bash
rkat mob flows <mob_id>
rkat mob run-flow <mob_id> --flow <flow_id> [--params <json-object>]
rkat mob flow-status <mob_id> <run_id>
```

Flow model highlights:

- declarative DAG step graph (`depends_on`),
- dependency mode (`all` or `any`),
- branching via `branch` + `condition`,
- dispatch mode (`one_to_one`, `fan_out`, `fan_in`),
- topology policy enforcement (`strict|permissive` + role rules including `"*"` wildcard),
- persisted run snapshots (`MobRun`) with `step_ledger` and `failure_ledger`,
- frame-based execution via `FlowSpec.root: FrameSpec` (v2 flows),
- `repeat_until` loop nodes with `until` condition, `max_iterations` guard, and nested `body: FrameSpec`,
- `FlowFrameEngine` drives frame execution; `FlowFrameMachine` owns frame-local state; `LoopIterationMachine` owns loop body/evaluate lifecycle.

Operational notes:

- `run-flow` waits until terminal and persists a terminal snapshot.
- `flow-status` checks live run state first and falls back to terminal snapshots.
- flow limits are defined in mob `limits` (`max_flow_duration_ms`, `max_step_retries`, `cancel_grace_timeout_ms`, `max_orphaned_turns`).

Terminology:

- **Mob runtime contract**: where `mob_*` tools and `rkat mob` lifecycle are exposed.
- **Backend selection**: realm-level storage backend (`sqlite`/`jsonl`) pinned in `realm_manifest.json`.

Do not conflate the two: mob tool availability is a surface behavior, backend is a realm storage choice.

## Scheduling

The `meerkat-schedule` crate provides durable, authority-backed scheduling for automated agent task execution.

### Schedule model

A `Schedule` defines when and how agent sessions are materialized:
- **Trigger**: `Cron(CalendarTriggerSpec)` or `Interval(IntervalTriggerSpec)`
- **Target**: `Session(SessionTargetBinding)` or `Mob(MobTargetBinding)`
- **Policies**: `MisfirePolicy` (Execute/Skip/SkipIfOlderThan), `OverlapPolicy` (Allow/Skip/Queue), `MissingTargetPolicy` (Skip/Fail/Create)
- **Phase**: `Active`, `Paused`, `Completed`, `Cancelled`

Each schedule produces `Occurrence` instances (individual fires) projected within a planning horizon.

### Schedule tools (agent-facing)

| Tool | Description |
|------|-------------|
| `meerkat_schedule_create` | Create a schedule with trigger + target |
| `meerkat_schedule_get` | Read one schedule |
| `meerkat_schedule_list` | List schedules |
| `meerkat_schedule_update` | Update trigger, target, or policies |
| `meerkat_schedule_pause` / `meerkat_schedule_resume` | Pause/resume a schedule |
| `meerkat_schedule_delete` | Delete a schedule |
| `meerkat_schedule_occurrences` | Read schedule occurrences |

Tools are available across all surfaces (CLI, REST, RPC, MCP) via `ScheduleToolDispatcher` (implements `AgentToolDispatcher`), wired as a first-class surface capability. WASM uses `DisabledScheduleStore`.

### Architecture

```
ScheduleService         — CRUD + occurrence planning
ScheduleDriver          — tick loop, claims due occurrences, dispatches delivery
ScheduleLifecycleAuthority   — authority-backed schedule state transitions
OccurrenceLifecycleAuthority — authority-backed occurrence state transitions
ScheduleStore           — persistence (Memory, SQLite)
```

**Delivery**: The driver claims due occurrences, probes target availability, dispatches delivery (creates session + runs prompt), and monitors completion. The schedule host surface (`meerkat/src/surface/schedule_host.rs`) bridges delivery to `MeerkatMachine`.

**Planning**: On create/update/resume, `ScheduleService` projects occurrences within a horizon (30 days or 64 occurrences). Old occurrences are superseded atomically via `atomic_plan_mutation()`.

**Stores are per-realm, not per-instance** — multiple processes may share a schedule store. Use `atomic_plan_mutation()` for safe multi-step changes.

### Rust SDK usage

```rust
use meerkat_schedule::{ScheduleService, MemoryScheduleStore, CreateScheduleRequest};
use std::sync::Arc;

let store = Arc::new(MemoryScheduleStore::new());
let service = ScheduleService::new(store);

let schedule = service.create(CreateScheduleRequest {
    display_name: "hourly-report".into(),
    trigger: TriggerSpec::Interval(IntervalTriggerSpec {
        every: Duration::from_secs(3600),
    }),
    target: TargetBinding::Session(SessionTargetBinding {
        model: "claude-sonnet-4-6".into(),
        prompt: ContentInput::Text("Generate hourly report".into()),
        ..Default::default()
    }),
    ..Default::default()
}).await?;
```

## Quick start

### CLI

```bash
rkat "What is Rust?"                     # "run" is the default subcommand
rkat run "What is Rust?"                 # equivalent explicit form
rkat --realm team-alpha run "Create a todo app" --tools workspace --stream -v
# Global flags: --realm, --isolated, --instance, --realm-backend, --state-root, --context-root, --user-config-root
rkat run --resume "keep going"           # resume most recent session
rkat run --resume 019c8b99 "continue"    # resume by short prefix
rkat --realm team-alpha run --resume sid_abc123 "Now add error handling"
# Batch context: pipe finite content as context
cat document.txt | rkat run "Summarize this document"
git diff | rkat run "Review these changes" --tools safe
# Chained pipes: each rkat reads stdin, writes response to stdout
cat data.csv | rkat run "Extract entities" | rkat run "Write a story about them"
# Live streaming: --keep-alive --stdin reads stdin line-by-line as events
tail -f app.log | rkat run --keep-alive --stdin lines "Monitor and alert on anomalies"
rkat mob spawn-helper coding-swarm "Join as lead-1 and summarize the current plan." --profile lead --agent-identity lead-1
rkat mob run-flow coding-swarm --flow triage --params '{"severity":"high"}'
```

### Python SDK

```python
from meerkat import MeerkatClient

client = MeerkatClient()
await client.connect(realm_id="team-alpha")
result = await client.create_session("What is Rust?")
print(result.text)
await client.close()
```

### TypeScript SDK

```typescript
import { MeerkatClient } from "@rkat/sdk";

const client = new MeerkatClient();
await client.connect({ realmId: "team-alpha" });
const session = await client.createSession("What is Rust?");
await client.close();
```

### Rust SDK

```rust
use meerkat::{
    AgentFactory, Config, CreateSessionRequest, SessionService,
    build_persistent_service, open_realm_persistence_in,
};
use meerkat_core::service::InitialTurnPolicy;
use meerkat_store::RealmBackend;

let config = Config::load().await?;
let realms_root = std::env::current_dir()?.join(".rkat").join("realms");
let (_manifest, persistence) = open_realm_persistence_in(
    &realms_root,
    "team-alpha",
    Some(RealmBackend::Sqlite),
    None,
).await?;
let factory = AgentFactory::new(realms_root.clone())
    .runtime_root(realms_root)
    .builtins(true)
    .shell(true);
let service = build_persistent_service(factory, config, 64, persistence);
let result = service.create_session(CreateSessionRequest {
    model: "claude-sonnet-4-6".into(),
    prompt: "What is Rust?".into(),
    system_prompt: None,
    max_tokens: None,
    event_tx: None,
    skill_references: None,
    initial_turn: InitialTurnPolicy::RunImmediately,
    build: None,
    labels: None,
}).await?;
```

For detailed surface schemas and examples, also load:

- `references/api_reference.md`
- `references/mobs.md`
- `references/migration_0_5.md`

## Configuration

Config APIs return a realm-scoped envelope:

- `config`
- `generation`
- `realm_id`
- `instance_id`
- `backend`
- `resolved_paths`

`config/set` and `config/patch` support `expected_generation` for CAS.

## Auth (realm/binding model)

Every session resolves credentials through `ProviderRuntimeRegistry`.
Two paths:

- **Env fallback**: set `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` /
  `GEMINI_API_KEY` (or `RKAT_*` variants) and Meerkat synthesizes a
  default realm + binding for you. No config changes needed.
- **Realm bindings**: declare `[realm.<id>.{backend,auth,binding}]` in
  config, run `rkat auth login <provider>`, then pass
  `--connection-ref <realm>:<binding>` on `rkat run` / session-create /
  `mob_spawn_member` to scope a specific session or mob member.

Key facts:

- The canonical owner of credential acquisition is
  `meerkat_providers::ProviderRuntimeRegistry`. No other code reads
  `std::env::var` for provider keys (dogma §5/§7/§14).
- `SessionLlmIdentity.connection_ref` is persisted; hot-swap
  (`apply_live_session_llm_identity`) re-enters `resolve` against
  the same binding — no cross-realm credential bleed.
- `AuthMachine` (per-binding) owns refresh semantics: Valid →
  Expiring → Refreshing → (Valid | ReauthRequired). Refresh dedup is
  DSL-owned (in-process) + filesystem-lockfile (cross-process, feature
  `refresh-file-lock`).
- OAuth methods: `claude_ai_oauth` (Anthropic), `managed_chatgpt_oauth`
  (OpenAI), `google_oauth` (Code Assist). All PKCE S256.
- Cloud IAM: Bedrock (SigV4), Vertex (GoogleAuth), Foundry (Azure AD).
- Every accepted transition emits a `target = "meerkat::auth::audit"`
  tracing event (deferral §5). REST/RPC login/logout emit matching
  events for user-initiated actions.

Provider-runtime/auth compatibility shims live in `meerkat-providers` and
`meerkat-client`, but the concrete provider implementations now live in the
provider-specific crates such as `meerkat-openai`, `meerkat-anthropic`, and
`meerkat-gemini`.

See `docs/guides/auth.mdx` for the full walkthrough and
`docs/architecture/meerkat-runtime-dogma.md` for the §1/§10/§12
rationale.

## Feature composition

The `meerkat` facade crate defaults to providers only (Anthropic, OpenAI, Gemini). Everything else is opt-in via Cargo features:

```toml
# Default: three providers, no storage/comms/tools
meerkat = "0.6.0"

# Single provider, minimal
meerkat = { version = "0.6.0", default-features = false, features = ["anthropic"] }

# Add persistence + memory + comms
meerkat = { version = "0.6.0", features = [
    "jsonl-store", "session-store", "session-compaction",
    "memory-store-session", "comms", "mcp", "skills"
] }
```

Available features: `anthropic`, `openai`, `gemini`, `all-providers`, `jsonl-store`, `memory-store`, `session-store`, `session-compaction`, `memory-store-session`, `comms`, `mcp`, `skills`, `schedule`.

Prebuilt binaries (`rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`) include everything. Custom binary builds:

```bash
cargo install rkat --no-default-features --features "anthropic,openai,session-store,mcp"
```

Disabled features return typed errors (e.g. `SessionError::PersistenceDisabled`) — no panics.

### Model catalog

The compatibility `meerkat-models` crate re-exports the curated model catalog queryable from all surfaces:

- CLI: `rkat models`
- RPC: `models/catalog`
- REST: `GET /models/catalog`
- MCP: `meerkat_models_catalog`

Returns `ModelsCatalogResponse` with providers, default models, and per-model profiles (capabilities, parameter schemas).

### Mid-session model hot-swap

Model and provider can be changed on a live session without rebuilding the agent:

- **RPC**: `turn/start` with `model`, `provider`, `provider_params` fields. Works on both pending (deferred) and materialized sessions.
- **REST**: `POST /sessions/{id}/messages` with `model`, `provider` fields.
- **MCP**: `meerkat_resume` with `model`, `provider` fields.
- **Rust SDK**: `Agent::replace_client()` for direct library usage.

On materialized sessions, the LLM client is hot-swapped for the remainder of the session. Ephemeral sessions return `SessionError::Unsupported`.

## Core features

### Multimodal content

Prompts and tool results support multimodal content (text, images, and video). The `ContentInput` type (`Text(String)` or `Blocks(Vec<ContentBlock>)`) is used for all prompt parameters across surfaces.

**Content block types:**
- `ContentBlock::Text { text }` — plain text
- `ContentBlock::Image { media_type, data }` — base64-encoded image with `ImageData::Inline` or `ImageData::Blob`
- `ContentBlock::Video { media_type, duration_ms, data }` — base64-encoded inline video with `VideoData::Inline`

**SDK prompt types:**
- Python: `prompt: str | list[dict]` — dicts with `{"type": "text", "text": "..."}`, `{"type": "image", "media_type": "...", "data": "<base64>"}`, or `{"type": "video", "media_type": "video/mp4", "duration_ms": 12000, "data": "<base64>"}`
- TypeScript: `prompt: string | ContentBlock[]` — `{type: "text", text: "..."}`, `{type: "image", mediaType: "...", data: "<base64>"}`, or `{type: "video", media_type: "video/mp4", duration_ms: 12000, data: "<base64>"}`
- Rust: `prompt: ContentInput` — `ContentInput::Text(s)` or `ContentInput::Blocks(vec![...])`; implements `From<&str>` and `From<String>`

**Video support:** Inline video is Gemini-only. Supported media types: `video/mp4`, `video/webm`, `video/quicktime`. Non-Gemini providers degrade replayed video to `[video: media_type]` text placeholders. Video in tool results is rejected at all providers. Ingress validation rejects video input for non-Gemini models at RPC, REST, and session boundaries.

**view_image builtin tool:** Reads images from disk (PNG/JPEG/GIF/WebP/SVG), returns base64 `ContentBlock::Image`. Path sandboxed to project root. 5 MB limit. Hidden on non-vision-capable models via `ToolScope` based on `ModelProfile.vision` and `ModelProfile.image_tool_results`.

**Provider capabilities:**
| Provider | `vision` | `image_tool_results` | `inline_video` |
|----------|----------|---------------------|----------------|
| Anthropic | Yes | Yes | No |
| OpenAI | Yes | No | No |
| Gemini | Yes | Yes | Yes |

### Sessions

Sessions are realm-scoped and surface-neutral. Visibility depends on `realm_id` matching.

### Streaming

Real-time events include `text_delta`, tool lifecycle events, hook events, and terminal run events.

### Background completion delivery (CompletionFeed)

Background shell job completions, mob member terminals, and async external tool results are delivered through the `CompletionFeed` seam. The feed is a monotonic append-only event log owned by the runtime epoch. Consumers (agent boundary, idle wake) read through cursor-based `list_since()`.

Key types: `CompletionFeed` trait (meerkat-core), `CompletionEntry`, `CompletionSeq`, `CompletionBatch`. Runtime implementation: `RuntimeCompletionFeed` in meerkat-runtime.

The agent boundary injects `[BG_JOB]` system notices at the `CallingLlm` boundary for each new terminal entry. The runtime loop's idle wake fires on `wait_for_advance()` to inject continuation turns for quiescent sessions.

Background shell jobs (started by the shell tool with `&` or via the `background` parameter) are tracked through the `OpsLifecycleRegistry` as detached operations. When a background job completes, the `CompletionFeed` delivers a `BackgroundJobCompleted` event. The agent receives a `[SYSTEM NOTICE][BG_JOB]` message containing the job name, ID, exit status, and output at the next `CallingLlm` boundary. Idle keep-alive sessions are woken via `DetachedWakeState` to process these completions.

### Durable runtime epoch recovery

Runtime epoch state (ops lifecycle, completion feed entries, consumer cursors) can be durably persisted via `PersistedOpsSnapshot`. On process restart with a persistent `RuntimeStore`, the epoch is recovered with the same `RuntimeEpochId`, preserving completion entries and cursor positions. Without a store, the epoch rotates (fresh state, conversation resumed only).

Recovery contract: bounded-loss, no invisible completions, possible duplicate notices. Terminal transitions are persisted via a bounded persistence channel; the loss window is the time between channel send and store commit.

Key types: `PersistedOpsSnapshot`, `EpochCursorState`, `EpochCursorSnapshot`. Recovery seam: `MeerkatMachine::recover_or_create_ops_state()`.

### Skills

Skill loading is runtime-root aware. Workspace realms use project `.rkat/skills`; non-workspace realms use realm runtime roots.

**Skill introspection** surfaces are available on all surfaces:

- CLI: `rkat skill list [--json]`, `rkat skill inspect <id> [--source <name>] [--json]`
- RPC: `skills/list`, `skills/inspect`
- REST: `GET /skills`, `GET /skills/{id}`
- MCP: `meerkat_skills` tool (`action: "list"` / `"inspect"`)
- Rust SDK: `SkillRuntime::list_all_with_provenance()`, `SkillRuntime::load_from_source()`

Introspection returns both active and shadowed skills with their source provenance, enabling debugging of skill resolution order.

### Hooks

Hook config is realm-aware, with compatibility layering from user/project hook files when available.

### Delegated work

Use mobs for all multi-agent orchestration. Mobs support `MemberLaunchMode::Fork` for forking a member's conversation history (via `Session::fork()` CoW), `spawn_helper()`/`fork_helper()` for one-call convenience, `force_cancel_member()` for cancelling in-flight turns, and `member_status()`/`wait_one()`/`wait_all()`/`collect_completed()` for monitoring.

### Inter-agent comms

Comms supports `inproc`, TCP, and UDS. Inproc registry is namespace-segmented; Meerkat uses realm namespace for isolation.

**Peer handling_mode override**: `PeerInput` with `Message`, `Request`, or no convention supports an explicit `handling_mode` field (`Queue` or `Steer`) that overrides kind-based policy defaults. `ResponseProgress` and `ResponseTerminal` reject the field at runtime admission. Built-in comms bridges leave it `None` (kind-based policy). The override is available on RPC `comms/send`, REST, and MCP `meerkat_comms_send` surfaces.

**Peer lifecycle typing**: mob lifecycle notices are typed at peer ingress. `mob.peer_added`, `mob.peer_retired`, and `mob.peer_unwired` are silent lifecycle context; `mob.kickoff_failed` and `mob.kickoff_cancelled` are visible lifecycle notices. Do not rely on mob defaults in `silent_comms_intents` for canonical behavior.

**Comms choice**: use `send_message` for ordinary collaboration. Use `send_request` only for structured ask/reply semantics (`intent + params` plus later `send_response`). Peer-side reservation streams were removed.

#### Structured output with comms (autonomous agents)

In `autonomous_host` mode, agents run a continuous loop: wake on inbox → process (LLM calls + tool calls including `send_message`/`send_request`/`send_response`) → produce final text output → sleep. Key architectural points:

- **`output_schema` constrains the agent's final text output**, not tool call arguments. It triggers an extraction turn after the agentic loop completes, calling the LLM with no tools and API-enforced structured output.
- **Comms `send_message` tool body is free-text** (`Option<String>`). There is no schema enforcement on comms message content — agents communicate naturally.
- **The extraction turn fires after each keep-alive processing cycle.** Each time the runtime comms drain consumes inbox work, the agent processes it, sends replies, and then produces a structured JSON summary of what it did. This summary is API-enforced via `output_schema` on the profile.
- **Use case**: Set `output_schema` on autonomous agent profiles to get structured turn summaries (e.g. `{headline: string, details: string}`) while letting agents communicate freely via `send_message`. The summaries power compact UI displays; the raw comms messages are available for detailed views.
- **Event stream**: The structured output appears in `RunCompleted` events as a JSON string in the `result` field. Parse it to extract the schema-validated fields.

### Tool scoping

Tool visibility can change during a session without restarting the agent. All changes are staged then atomically applied at the turn boundary.

- **External filters** — allow-list or deny-list staged via `ToolScopeHandle`, applied at `CallingLlm` boundary. Persisted in session metadata (`tool_scope_external_filter`).
- **Per-turn overlay** — `TurnToolOverlay` on `StartTurnRequest.flow_tool_overlay`. Ephemeral, used by mob flow steps to restrict tools per step.
- **Live MCP mutation** — `mcp/add`, `mcp/remove`, `mcp/reload` stage server changes on the `McpRouter`. Applied at next turn boundary. Removals drain in-flight calls before finalizing.
- **Async MCP loading** — At startup, MCP servers connect in parallel in the background. The agent loop polls `poll_external_updates()` at each `CallingLlm` boundary. Tools appear as each server completes its handshake. A `[MCP_PENDING]` system notice is injected while servers are still connecting.
  - Per-server timeout: `connect_timeout_secs` in `.rkat/mcp.toml` (default: 10s)
  - CLI: `--wait-for-mcp` flag blocks before the first turn until all servers finish connecting
  - SDK: `McpRouterAdapter::wait_until_ready(timeout)` provides the same blocking behavior
  - `AgentBuildConfig.wait_for_mcp: bool` field for programmatic surface control
- **Composition** — most-restrictive wins (allow-lists intersect, deny-lists union, deny beats allow).
- **Agent awareness** — `ToolConfigChanged` event emitted + `[SYSTEM NOTICE]` injected into conversation on any change.

Surface availability:

| Surface | Live MCP | Tool filter | Status |
|---------|----------|-------------|--------|
| JSON-RPC | `mcp/add`, `mcp/remove`, `mcp/reload` | Via session runtime | Fully wired |
| REST | `POST /sessions/{id}/mcp/*` | — | Fully wired |
| CLI | `rkat mcp reload --session --live-server-url` | — | Host-managed live reload flow |

### Memory

Semantic memory (`memory_search`) and compaction integrate through the same session/runtime pipeline.

## Repo test lanes

For repository development and regression triage, use the standardized Cargo
lanes rather than ad hoc per-surface commands:

- `./scripts/repo-cargo unit`
- `./scripts/repo-cargo int`
- `./scripts/repo-cargo e2e-fast`
- `./scripts/repo-cargo e2e-system`
- `./scripts/repo-cargo e2e-live`
- `./scripts/repo-cargo e2e-smoke`

Authoritative end-to-end lane ownership lives in
`tests/integration/src/e2e_lanes.rs`. Python, TypeScript, and browser live
scenarios may still shell out internally, but the supported top-level runner is
the Rust-owned lane harness.

## Reference

For complete method signatures and multi-surface examples, load:
`references/api_reference.md`
