---
name: meerkat-platform
description: "Build on the Meerkat agent platform. Covers every shipping surface (CLI, REST, JSON-RPC stdio/TCP, MCP, Python/TypeScript/Web/Rust SDKs), realm-scoped sessions, streaming, skills, hooks, memory, multimodal content, mob orchestration (spawn/fork/helpers/flows/profiles), durable scheduling, and provider auth via `auth_binding` + AuthMachine (env keys, OAuth, cloud IAM). Use when integrating with Meerkat, picking a surface, wiring auth, building agents, scheduling automated runs, deploying mobpacks, or asking how a feature exposes through a particular SDK."
---

# Meerkat Platform Guide

Meerkat (`rkat`) is a library-first agent runtime exposed through multiple surfaces. One execution pipeline, realm-scoped state, runtime-backed semantics by default. This guide is task-oriented; deeper schemas live under `references/`.

References:

- `references/api_reference.md` — per-surface methods, schemas, examples (CLI, REST, RPC, MCP, Python/TS/Rust SDKs).
- `references/mobs.md` — multi-agent orchestration in depth.
- `references/migration_0_5.md` — only if you're upgrading from 0.5 (`host_mode` → `keep_alive`, etc.).

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

- CLI (`run`, `run --resume`, `session`): workspace-derived stable realm.
- RPC/REST/MCP/SDK: new opaque realm unless explicitly provided.

Use explicit realm to share:

```bash
rkat --realm team-alpha run "Plan release"
rkat-rpc --realm team-alpha
rkat-rest --realm team-alpha
rkat-mcp --realm team-alpha
```

## Runtime-backed vs standalone

- **Runtime-backed** (CLI, REST, `rkat-rpc`, `rkat-mcp`): keep-alive sessions, durable comms drain, completion-feed wakeups, recovery on restart. This is the default product path for daemons and long-running agents.
- **Standalone / embedded** (Web SDK / WASM, in-process Rust SDK without a runtime, tests): in-memory substrate, no keep-alive, no cross-process recovery.

The Rust SDK lets you pick: `RuntimeBuildMode::SessionOwned(bindings)` (runtime-backed) or `RuntimeBuildMode::StandaloneEphemeral` (default). Surfaces other than the Rust SDK make this choice for you.

## External mob members (advanced)

When a mob member runs in a different process or host, declare it with `MobBackendKind::External` and pass `RuntimeBinding::External { peer_id, address }` at spawn time so the orchestrator can route comms to the real backend. Internal-process members use `Session` and need no extra wiring. Members are addressed everywhere by stable `AgentIdentity`; runtime/binding identity rotates underneath.

## Surfaces

| Surface | Protocol | Use case |
|---------|----------|----------|
| `rkat` CLI | Shell commands | Developer workflows, scripting |
| REST (`rkat-rest`) | HTTP/JSON + SSE | Services and language-agnostic clients |
| JSON-RPC (`rkat-rpc`) | JSON-RPC 2.0 over stdio (default) or TCP (`--tcp <addr>`); optional realtime websocket hosting | IDE integration, SDK backend, embeddable services |
| MCP (`rkat-mcp`) | Model Context Protocol | Expose Meerkat as tools to other agents |
| Python SDK (`meerkat`) | Async Python over RPC | Python applications |
| TypeScript SDK (`@rkat/sdk`) | TypeScript over RPC | Node.js applications |
| Web SDK (`@rkat/web`) | WASM in-browser | Browser applications, mobpack deployment target |
| Rust SDK (`meerkat`) | Direct library API | Embedded Rust systems |

For full per-surface schemas and examples, load: `references/api_reference.md`.
For detailed mob behavior across all surfaces, load: `references/mobs.md`.

## MCP server config (CLI)

Use `rkat mcp ...` to manage local MCP server configuration. This writes config; new `rkat run` and `rkat run --resume` sessions load configured MCP servers and expose their tools to the agent.

Config locations:

- Project scope (default): `.rkat/mcp.toml`
- User scope: `~/.rkat/mcp.toml`

Command forms:

```bash
rkat mcp add <NAME> [--transport stdio|http|sse] [--scope project|user|local] [-H KEY:VALUE...] [-e KEY=VALUE...] (--url <URL> | -- <CMD...>)
rkat mcp remove <NAME> [--scope project|user|local]
rkat mcp list [--scope project|user|local] [--json]
rkat mcp get <NAME> [--scope project|user|local] [--json]
```

Examples:

```bash
# stdio server; command and args go after --
rkat mcp add filesystem -- npx -y @modelcontextprotocol/server-filesystem .

# HTTP server
rkat mcp add linear --transport http --url https://mcp.example.com

# User-wide server with environment passed to the stdio process
rkat mcp add github --scope user -e GITHUB_TOKEN=ghp_... -- npx -y @modelcontextprotocol/server-github

rkat mcp list
rkat mcp get filesystem --scope project
```

Notes:

- Use `--transport http` for streamable HTTP and `--transport sse` for legacy SSE.
- `-H KEY:VALUE` is for HTTP/SSE headers; `-e KEY=VALUE` is for stdio process environment.
- `rkat mcp` is a config surface. Live mutation of a running session uses JSON-RPC `mcp/add|remove|reload`, REST `POST /sessions/{id}/mcp/*`, MCP-server tools, or SDK helpers.

## Mob behavior (current contract)

- CLI `run`/`run --resume` compose `mob_*` tools through `meerkat-mob-mcp` dispatcher integration when mob tools are enabled, for example with `--tools full` or config `tools.mob_enabled=true`.
- CLI `mob ...` is the explicit lifecycle surface for persisted mob registry operations.
- RPC/REST/MCP server/Python SDK/TypeScript SDK expose mob capability via the same dispatcher composition model (`SessionBuildOptions.external_tools`) in host integrations.
- Member runtime default is `autonomous_host` when `runtime_mode` is omitted; `turn_driven` is explicit opt-in for controlled dispatch paths.
- Spawned mob members use deferred initial turn semantics; mob actor lifecycle starts autonomous loops explicitly after spawn registration.
- Mob persistence is SQLite/WAL-backed (`MobStorage::persistent()` opens `SqliteMobStores`). In-memory storage is used for tests and WASM. The previous exclusive-handle mob store has been removed.
- Prefabs are gone. All mob creation uses `MobDefinition` only (CLI, REST, RPC, MCP, SDKs).
- Agent-facing delegation tools (`delegate`, `mob_create`, `mob_destroy`, `mob_spawn_member`, `mob_retire_member`, `mob_check_member`, `mob_list_members`, `mob_list`, `mob_wire`, `mob_unwire`) are provided by `AgentMobToolSurface` in `meerkat-mob-mcp`. These tools let agents spawn and manage mob members through implicit session-owned mobs, and create/remove peer-to-peer comms links between members.
- Portable mob artifacts are available through mobpack (`rkat mob pack/deploy/inspect/validate`) and browser deployment (`rkat mob web build`).
- Public realtime attachment capability is named `realtime`, not `voice`: surfaces describe `ModelCapabilities.realtime`, `session/realtime_attachment_status`, and `mob/member_status.realtime_attachment_status`. Realtime transport is capability-driven — there is no caller-initiated attach/detach RPC; set the session's model to a realtime-capable one (e.g. `gpt-realtime-1.5`) and the runtime manages attach/detach automatically.
### Realtime voice attachment

Realtime is a delivery mode of the session's LLM, not a separate subsystem. Enable it by setting the session's model to a realtime-capable one (today `gpt-realtime-1.5`; `gpt-realtime` is a compatibility alias). The runtime reads `ModelCapabilities.realtime` and attaches an OpenAI Realtime transport automatically. The session keeps a single canonical history; audio commits at turn boundaries.

| Surface | Enable | Observe status | Open audio channel |
|---------|--------|----------------|--------------------|
| CLI | Start or resume a session/member on a realtime-capable model | `rkat realtime status session <SESSION-ID>` / `rkat realtime status member <MOB-ID> <AGENT-IDENTITY>` | `rkat realtime open-info session <SESSION-ID>` / `... member <MOB-ID> <AGENT-IDENTITY>` |
| JSON-RPC | `session/create` with `model: "gpt-realtime-1.5"` | `session/realtime_attachment_status`, `mob/member_status.realtime_attachment_status` | `realtime/open_info` |
| REST | `POST /sessions` `{ "model": "gpt-realtime-1.5" }` | `GET /sessions/{id}/realtime-attachment-status` | `realtime/open_info` via RPC |
| MCP (public) | host sets model via composition | `meerkat_realtime_status`, `meerkat_realtime_capabilities` | `meerkat_realtime_open_info` |
| Python SDK | `client.create_session(model="gpt-realtime-1.5", ...)` | `client.runtime_realtime_attachment_status(...)` | `client.realtime_open_info(...)` |
| TypeScript SDK | `client.createSession({ model: "gpt-realtime-1.5", ... })` | `client.runtimeRealtimeAttachmentStatus(...)` | `client.realtimeOpenInfo(...)` |
| Rust | `SessionBuildOptions { model: Some("gpt-realtime-1.5".into()), .. }` | `MeerkatMachine::realtime_attachment_status` | provider integration in `meerkat-openai` |

`RealtimeAttachmentStatus` reports `Unattached` / `IntentPresentUnbound` / `BindingNotReady` / `BindingReady` / `ReplacementPending` / `ReattachRequired`. There is no caller-initiated attach/detach RPC — transport follows the resolved model capability.

Practical caveats:

- Single realtime binding per session. For per-member realtime in mobs, spawn members on realtime-capable profiles.
- Idle sessions can't host a binding — start a turn or spawn via a mob.
- Provider-native web search and tool-calling capability is per-model; check `ModelProfile` flat fields like `supports_web_search` if a tool unexpectedly disappears under a realtime model.

User-facing guide: `docs/guides/realtime.mdx`. Internal authority/reconfigure flow: load the architecture skill's `references/realtime-attachment.md`.

### Mob lifecycle (standard/default usage)

Primary CLI usage is tool-driven through `run`/`run --resume` with mob tools enabled:

```bash
rkat run --tools full "create a mob with a lead and workers, then wire and report status"
rkat run --tools full --resume <session_id> "retire worker-2 and add worker-4"
```

Where needed, the current helper-oriented CLI mob surface is:

```bash
rkat mob spawn-helper <mob_id> <prompt> [--agent-identity <id>] [--profile <profile>] [--json]
rkat mob fork-helper <mob_id> <source_member> <prompt> [--agent-identity <id>] [--profile <profile>] [--fork-context full-history|last-messages] [--last-messages N] [--json]
rkat mob member-status <mob_id> <agent_identity> [--json]
rkat mob force-cancel <mob_id> <agent_identity>
rkat mob respawn <mob_id> <agent_identity> [--initial-message <msg>]
rkat mob run-flow <mob_id> --flow <flow_id> [--params <json>] [-s|--stream] [--no-stream]
rkat mob flow-status <mob_id> <run_id>
rkat mob wait-kickoff <mob_id> [--member <agent_identity>...] [--timeout-ms N] [--json]
```

### Mobpack + web build quick paths

```bash
# Build portable artifact
rkat mob pack ./mobs/release-triage -o ./dist/release-triage.mobpack \
  --sign ./keys/release.key --signer-id team@example.com   # --sign requires --signer-id
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

### WASM runtime + Web SDK (browser embedded)

`@rkat/web` runs the full meerkat agent stack — agent loop, all three providers via browser `fetch`, sessions, mob orchestration, inproc comms, embedded skills/hooks — inside the browser. It's a deployment *target* for mobpacks, not a protocol server: the host page provides config and drives interaction.

```typescript
import { MeerkatRuntime } from '@rkat/web';
import * as wasm from '@rkat/web/wasm/meerkat_web_runtime.js';

const runtime = await MeerkatRuntime.init(wasm, {
  anthropicApiKey: 'proxy',                                   // dummy; proxy injects real key
  anthropicBaseUrl: 'http://localhost:3100/anthropic',
});
const mob = await runtime.createMob(definition);
await mob.spawn([{ profile: 'worker', agent_identity: 'w1' }]);
```

Run the auth-injecting proxy beside any Node host so API keys stay server-side:

```bash
ANTHROPIC_API_KEY=sk-... npx @rkat/web proxy --port 3100
```

For OAuth, cloud IAM, or any "auth handled by the host page" flow, register an external resolver instead of shipping bare API keys to the browser:

```typescript
import { registerExternalAuthResolver, withAuthBinding } from '@rkat/web';
registerExternalAuthResolver(wasm, async (authBinding) => {
  const token = await myHostFetchToken(authBinding);
  return { kind: 'bearer_token', token };
});
// withAuthBinding takes (authBinding, config) and returns a config with `authBinding` set.
const session = runtime.createSession(withAuthBinding(authBinding, { model: 'claude-sonnet-4-6' }));
```

`authBinding` is the structural way to scope a session/mob member to a specific realm + binding — set it on `runtime.createSession({...})`, `mob.spawn(...)`, etc. Per-session `apiKey` fields were removed; use `anthropicApiKey`/`openaiApiKey`/`geminiApiKey` at runtime init or rely on the resolver.

Browser scope: filesystem, shell, MCP client (rmcp), and network comms (TCP/UDS) are excluded by browser limitations. Everything else is intentionally wasm32-equivalent. For wasm internals, build commands, and full export table, see the meerkat-wasm skill.

### Mob flows (DAG runtime)

```bash
rkat mob run-flow <mob_id> --flow <flow_id> [--params '{"k":"v"}']
rkat mob flow-status <mob_id> <run_id>
```

Flow model: declarative DAG (`depends_on`, `depends_on_mode = all|any`), dispatch modes (`one_to_one`, `fan_out`, `fan_in`), optional `branch` + `condition`, topology rules (`strict|permissive`, `"*"` wildcard), persisted `MobRun` snapshots with `step_ledger`/`failure_ledger`. Frame-based v2 flows add nested `FlowSpec.root: FrameSpec` and `repeat_until` loop nodes (`until`, `max_iterations`, nested `body`). `run-flow` blocks until terminal and persists the terminal snapshot; `flow-status` checks live state then falls back to the snapshot. Per-flow limits live under mob `limits` (`max_flow_duration_ms`, `max_step_retries`, `cancel_grace_timeout_ms`, `max_orphaned_turns`).

Don't conflate **mob tool availability** (surface behavior — `mob_*` tools and `rkat mob` lifecycle) with **realm backend** (`sqlite`/`jsonl` in `realm_manifest.json`).

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
rkat help "How do I add an MCP server?"
rkat run --resume "keep going"           # resume most recent session
rkat run --resume 019c8b99 "continue"    # resume by short prefix
rkat --realm team-alpha run --resume 019c8b99 "Now add error handling"
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

Programmatic config APIs return a realm-scoped envelope:

- `config`
- `generation`
- `realm_id`
- `instance_id`
- `backend`
- `resolved_paths`

CLI config behavior differs: `rkat config get` defaults to raw TOML,
`rkat config get --format json` prints raw config JSON, and
`rkat config get --format json --with-generation` prints an envelope without
`resolved_paths`. `rkat config set` and `rkat config patch` print
`generation=N` after writes. CLI `set <FILE>` imports a file into the active
realm config; raw merge patch uses `rkat config patch --json '{...}'`.

`config/set` and `config/patch` support `expected_generation` for CAS.

## Auth

Every session resolves credentials through realm-scoped bindings. Two onramps:

**Quick start — env keys**: Meerkat resolves API keys from provider-specific env vars. Anthropic: `RKAT_ANTHROPIC_API_KEY`, then `ANTHROPIC_API_KEY`. OpenAI: `RKAT_OPENAI_API_KEY`, then `OPENAI_API_KEY`. Gemini: `RKAT_GEMINI_API_KEY`, `GEMINI_API_KEY`, `RKAT_GOOGLE_API_KEY`, then `GOOGLE_API_KEY`. Meerkat synthesizes a default realm + binding. No config edits.

**Realm bindings — OAuth, cloud IAM, multi-tenant**: declare `[realm.<id>.{backend,auth,binding}]` in config and pass `--auth-binding <realm>:<binding>[:profile]` on `rkat run` / `session/create` / `mob_spawn_member` to scope a session or mob member to that binding. CLI `rkat auth login <provider>` is a convenience bootstrap for fixed `dev:*` bindings: interactive OAuth writes `dev:anthropic_oauth`, `dev:openai_oauth`, or `dev:google_oauth`; non-interactive api-key login writes `dev:default_<provider>`.

```bash
rkat auth login anthropic                                            # OAuth (PKCE S256)
rkat run --auth-binding dev:anthropic_oauth "ship the release notes"
```

Supported auth methods:

- API keys (env or per-binding)
- OAuth: `claude_ai_oauth` (Anthropic), `managed_chatgpt_oauth` (OpenAI), `google_oauth` (Code Assist)
- Cloud IAM: AWS Bedrock (SigV4), GCP Vertex (GoogleAuth), Azure Foundry (Azure AD)

Tokens refresh automatically per binding. `auth_binding` is persisted on the session — hot-swapping the model re-resolves through the same binding (no cross-realm bleed).

In the Web SDK, ship a `authBinding` plus `registerExternalAuthResolver` instead of API keys; see the Web SDK section above.

Surfaces: `auth/profile/{create,get,list,delete}`, `auth/login/{start,complete,device_start,device_complete,provision_api_key}`, `auth/status/get`, `auth/logout` over RPC; equivalent over REST and SDKs. Full walkthrough: `docs/guides/auth.mdx`.

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

Prebuilt binaries (`rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`) include the normal shipping surfaces. The default `rkat` feature set does not include `memory-store`; memory capabilities appear only in binaries built with the memory-store feature. Custom binary builds:

```bash
cargo install rkat --no-default-features --features "anthropic,openai,session-store,mcp"
```

Disabled features return typed errors (e.g. `SessionError::PersistenceDisabled`) — no panics.

### CLI utility commands

- `rkat capabilities` prints pretty JSON `CapabilitiesResponse`; there is no `--json` or `--format` flag.
- `rkat doctor` has no command-specific flags. Output is tab-delimited `ok|warn <area> <message>`. Config/MCP/self-hosted hard issues exit 1; missing provider env vars and missing `wasm-pack` are warnings.

### Model catalog

The model catalog (canonical: `meerkat_core::model_profile`; `meerkat-models` is now a thin compatibility shim) is queryable from all surfaces:

- CLI: `rkat models`
- RPC: `models/catalog`
- REST: `GET /models/catalog`
- MCP: `meerkat_models_catalog`

`rkat models` prints pretty JSON by default and has no `--json` flag. It returns the effective runtime model registry for the active realm: built-in catalog entries plus configured `self_hosted` aliases, providers, default models, and per-model profiles.

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

**generate_image builtin tool:** Session-owned assistant image generation. The model calls one stable tool with universal fields (`prompt`, `provider`, `model`, `size`, `quality`, `format`, `count`, reference/source images) plus provider-owned `provider_params`. Provider crates own image model profiles, supported parameters, and backend selection. Generated images are stored in the blob store; user-facing surfaces fetch blob payload/bytes by blob id via `rkat blob get <BLOB-ID>`, JSON-RPC `blob/get`, or SDK `get_blob` / `getBlob`.

**Image generation via CLI:** There is no direct image-generation CLI command and no `rkat rpc` subcommand. Ask the assistant through a session, allow the image tool, and fetch the resulting blob:

```bash
rkat run --allow-tool generate_image "Use generate_image to create a square PNG of a cozy tabby cat by a sunlit window. Return the blob id."
rkat blob get <BLOB-ID> --output cat.png
```

Use `rkat blob get <BLOB-ID> --json` to inspect the blob payload. If the answer does not include a blob id, resume the session and ask for the blob id.

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

### Background work and recovery

Background shell jobs (shell tool with `&` or `background: true`), mob member terminals, and async external tool results all deliver back into the agent through a single completion stream. Each completion appears as a `[SYSTEM NOTICE][BG_JOB]` (or equivalent) message at the next LLM turn boundary, so the agent sees and reasons over it. Idle keep-alive sessions wake automatically when a completion lands.

If the runtime is backed by persistent storage, completion state and cursors survive process restarts (bounded-loss; you may see one duplicate notice on the seam). Without persistence, conversation history resumes but pending background work doesn't.

For internal seams (`CompletionFeed`, `OpsLifecycleRegistry`, `DetachedWakeState`, `RuntimeEpochId`) load the architecture skill.

### Skills

Skill loading is runtime-root aware. Workspace realms can discover project `.rkat/skills`; non-workspace realms use realm runtime roots and configured repositories.

Canonical skill identity is `SkillKey { source_uuid, skill_name }`. `preload_skills` carries plain `SkillKey` objects; API `skill_refs` carries tagged `SkillRef` objects (`kind: "structured"`, `source_uuid`, `skill_name`). Do not describe slash-delimited skill IDs as a public wire format.

**Skill introspection** surfaces:

- CLI: `rkat skill list [--json]`, `rkat skill inspect <skill-name> --source-uuid <uuid> [--json]`
- CLI config: `rkat skill add <PATH> [--name <NAME>]`, `rkat skill remove <NAME_OR_SOURCE_UUID_OR_PATH>`, `rkat skill get <NAME_OR_SOURCE_UUID_OR_PATH> [--json]`
- RPC: `skills/list`
- REST: `GET /skills`
- MCP: `meerkat_skills` tool (`action: "list"` or `"inspect"` with typed `skill_key` and optional `source` UUID)
- Rust SDK: `SkillRuntime::list_all_with_provenance()`, `SkillRuntime::load_from_source()`

Introspection returns typed keys plus source provenance. Shadowing is by full `SkillKey`, not by `skill_name` alone. Agent-facing skill tools (`browse_skills`, `load_skill`, resource tools, function invocation) also use `source_uuid` + `skill_name`.

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
- **Configured MCP servers** — CLI `rkat mcp add/remove/list/get` edits `.rkat/mcp.toml` or `~/.rkat/mcp.toml`. New `rkat run` and `rkat run --resume` sessions load that config.
- **Live MCP mutation** — JSON-RPC `mcp/add`, `mcp/remove`, `mcp/reload`, REST `POST /sessions/{id}/mcp/*`, MCP-server tools, and SDK helpers stage server changes on the `McpRouter`. Applied at next turn boundary. Removals drain in-flight calls before finalizing.
- **Async MCP loading** — At startup, MCP servers connect in parallel in the background. The agent loop polls `poll_external_updates()` at each `CallingLlm` boundary. Tools appear as each server completes its handshake. A `[MCP_PENDING]` system notice is injected while servers are still connecting.
  - Per-server timeout: `connect_timeout_secs` in `.rkat/mcp.toml` (default: 10s)
  - CLI: `--wait-for-mcp` flag blocks before the first turn until all servers finish connecting
  - SDK: `McpRouterAdapter::wait_until_ready(timeout)` provides the same blocking behavior
  - `AgentBuildConfig.wait_for_mcp: bool` field for programmatic surface control
- **Composition** — most-restrictive wins (allow-lists intersect, deny-lists union, deny beats allow).
- **Agent awareness** — `ToolConfigChanged` event emitted + `[SYSTEM NOTICE]` injected into conversation on any change.

Surface availability:

| Surface | MCP config / live mutation | Tool filter | Status |
|---------|----------------------------|-------------|--------|
| CLI | `rkat mcp add/remove/list/get` edits project/user config | — | Config surface |
| JSON-RPC | `mcp/add`, `mcp/remove`, `mcp/reload` live session mutation | Via session runtime | Fully wired |
| REST | `POST /sessions/{id}/mcp/add|remove|reload` live session mutation | — | Fully wired |
| MCP server | `meerkat_mcp_add`, `meerkat_mcp_remove`, `meerkat_mcp_reload` live session mutation | — | Fully wired |

### Memory

Semantic memory (`memory_search`) and compaction integrate through the same session/runtime pipeline.

## Repo test lanes

For repo development, prefer the Make lanes over ad-hoc commands: `make build`, `make check`, `make lint`, `make test`, `make test-unit`, `make test-int`, `make e2e-fast`, `make e2e-system`, `make e2e-live`, `make e2e-smoke`. Cargo is default; `MEERKAT_BUILDBUDDY=1` routes the same lanes through Bazel/RBE when available. Per-package checks: `./scripts/repo-cargo {unit,int,e2e-fast,e2e-system,e2e-live,e2e-smoke}`. End-to-end lane ownership lives in `tests/integration/src/e2e_lanes.rs`.

## Reference

For complete method signatures and multi-surface examples, load:

- `references/api_reference.md`
- `references/mobs.md`
- `references/migration_0_5.md` (only if migrating from 0.5)
