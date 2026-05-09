# Meerkat Platform API Reference

For upgrade and terminology changes, also load:

- `references/migration_0_5.md`

## Realm scope (all surfaces)

Use explicit realm to control sharing/isolation.

- Same `realm_id` => shared sessions/config/backend.
- Different `realm_id` => isolated state.
- Backend is pinned per realm via `realm_manifest.json`.
- New persistent realms default to `sqlite` when sqlite support is compiled.

Surface defaults when no realm is provided:

- CLI `run`, `run --resume`, and `session`: workspace-derived `ws-...` realm.
- CLI `mob ...`: workspace-derived `ws-...` realm.
- RPC/REST/MCP/SDK: new opaque `realm-...` realm.

Mob contract notes:

- CLI `run`/`run --resume` include `mob_*` tools via `meerkat-mob-mcp` dispatcher composition when mob tools are enabled (`--tools full` or config `tools.mob_enabled=true`).
- CLI `mob ...` commands provide helper/artifact operational verbs. Mob lifecycle creation/wiring/member management is through agent `mob_*` tools or RPC `mob/*`.
- RPC/REST/MCP/SDK surfaces gain mob capability by composing `meerkat-mob-mcp`
  (`MobMcpState` + `MobMcpDispatcher`) into `SessionBuildOptions.external_tools`.

For full mob behavior details (runtime model, flows, and surface matrix), also load:
`references/mobs.md`

---

## CLI

Global flags (available on all commands):

```bash
--realm <id>              # explicit realm ID
--isolated                # start in isolated mode (new generated realm)
--instance <id>           # optional instance ID inside a realm
--realm-backend <sqlite|jsonl>
--state-root <path>       # override state root directory
--context-root <path>     # convention context root for skills/hooks/AGENTS/MCP config
--user-config-root <path> # optional user-global convention root
```

Core commands:

```bash
rkat init                               # initialize local project config from global template
rkat run <PROMPT> [OPTIONS]
rkat <PROMPT>                            # shorthand — "run" is implied
cat file.txt | rkat run "Analyze this"   # stdin piped as context
rkat run --resume[=<SESSION-ID>] <PROMPT> # full UUID, short prefix, last, ~N
rkat session list [--limit N] [--offset N] [--label KEY=VALUE]
rkat session show <ID>
rkat session delete <ID>
rkat session interrupt <ID>
rkat realtime open-info|status|capabilities|bridge session <SESSION-ID>
rkat realtime open-info|status|capabilities|bridge member <MOB-ID> <AGENT-IDENTITY>
rkat blob get <BLOB-ID> [--output <FILE>] [--json]
rkat realm current|list|show|create|delete|prune ...
rkat mcp add|remove|list|get ...
rkat skill add <PATH> [--name <NAME>]
rkat skill remove|get <NAME_OR_SOURCE_UUID_OR_PATH> [--json for get]
rkat skill list [--json]
rkat skill inspect <SKILL_NAME> --source-uuid <SOURCE_UUID> [--json]
rkat models
rkat help <QUESTION> [--prompt <PROMPT>] [--plan-execution] [--json] [--stream|--no-stream]
rkat auth realms|profiles|profile|profile-delete|bindings|test|status|login|logout|refresh ...
# (Scheduling has no top-level CLI subcommand. Schedules are managed
# through the agent tools `meerkat_schedule_*` from `rkat run`,
# the RPC `schedule/*` methods, REST schedule endpoints, or SDK helpers.)
rkat mob spawn-helper|fork-helper|member-status|force-cancel|respawn|wait-kickoff|run-flow|flow-status|pack|inspect|validate|deploy|web ...
rkat config get|set|patch ...
rkat capabilities
rkat doctor
rkat-rpc                                # JSON-RPC stdio
rkat-rpc --tcp 127.0.0.1:9000           # JSON-RPC over TCP (stdio is default)
```

CLI keep-alive terminology:

- use `--keep-alive`
- do not use `--host`

`rkat init` has no command-specific options. Shared realm/context flags are
parsed by Clap but do not redirect the project config path or global config
source; it writes `./.rkat/config.toml` from `~/.rkat/config.toml`.

Important `rkat run` options:

```bash
--resume[=<SESSION>]                  # UUID, short prefix, realm:<uuid>, last, ~, ~N
-t, --tools <safe|workspace|full|none> # default safe; --yolo aliases full
--allow-tool <TOOL>                    # repeatable first-turn allow overlay
--block-tool <TOOL>                    # repeatable first-turn block overlay
--wait-for-mcp
--auth-binding <REALM:BINDING[:PROFILE]>
--output <text|json> / --json
--stream / --no-stream
--stdin <auto|blob|lines|off>
--line-format <text|json>
```

Hard CLI negatives: no `rkat sessions`, no `rkat resume`, no `rkat rpc`, no
`rkat image`, no `--tools all`, no `rkat blob get -o`, no
`rkat session show --json`, no `rkat models --json`, no
`rkat capabilities --json`.

### MCP CLI config surface

`rkat mcp` edits project/user MCP server config; it is not the live mutation surface for an already-running session.

```bash
rkat mcp add <NAME> [--transport stdio|http|sse] [--scope project|user|local] [-H KEY:VALUE...] [-e KEY=VALUE...] (--url <URL> | -- <CMD...>)
rkat mcp remove <NAME> [--scope project|user|local]
rkat mcp list [--scope project|user|local] [--json]
rkat mcp get <NAME> [--scope project|user|local] [--json]
```

Examples:

```bash
rkat mcp add filesystem -- npx -y @modelcontextprotocol/server-filesystem .
rkat mcp add linear --transport http --url https://mcp.example.com
rkat mcp list
```

### Mob CLI surface

Primary CLI mob usage is tool-driven from `run`/`run --resume` prompts using `mob_*` tools with `--tools full` or config `tools.mob_enabled=true`. The explicit `rkat mob <subcommand>` surface is helper-oriented — a small set of operational verbs:

| Subcommand | Purpose |
|------------|---------|
| `spawn-helper <mob_id> <prompt> [--profile] [--agent-identity] [--json]` | Spawn a short-lived helper, wait for completion, print the result |
| `fork-helper <mob_id> <source_member> <prompt> [--profile] [--agent-identity] [--fork-context full-history\|last-messages] [--last-messages N] [--json]` | Fork from an existing member's context and run a helper |
| `member-status <mob_id> <agent_identity> [--json]` | Execution status snapshot for a mob member |
| `force-cancel <mob_id> <agent_identity>` | Force-cancel a member's in-flight turn |
| `respawn <mob_id> <agent_identity> [--initial-message]` | Retire and respawn a member with the same profile |
| `wait-kickoff <mob_id> [--member ...] [--timeout-ms] [--json]` | Wait for autonomous-host kickoff turns to complete |
| `run-flow <mob_id> --flow <flow_id> [--params <json>] [-s\|--stream] [--no-stream]` | Start a flow run, block until terminal, print run id |
| `flow-status <mob_id> <run_id>` | Print live or terminal `MobRun` JSON; status: `pending`/`running`/`completed`/`failed`/`canceled` |
| `pack <dir> -o <pack> [--sign <key> --signer-id <id>]` | Pack a mob directory into a `.mobpack` archive |
| `inspect <pack>` | Inspect a `.mobpack` archive |
| `validate <pack> [--trust-policy permissive\|strict]` | Validate a `.mobpack` archive |
| `deploy <pack> <prompt> [--model] [--max-total-tokens] [--max-duration] [--max-tool-calls] [--trust-policy] [--surface cli\|rpc]` | Deploy a `.mobpack` with overrides |
| `web build <pack> -o <dir> [--trust-policy]` | Build a browser-deployable WASM bundle |

Lifecycle verbs that used to live on the CLI (`create`, `spawn`, `retire`, `wire`, `unwire`, `turn`, `stop`, `resume`, `complete`, `events`, `destroy`) are reached through the agent tools (`mob_create`, `mob_spawn_member`, `mob_wire`, ...) or through the RPC `mob/*` methods listed below.

---

## REST API

Server boot:

```bash
rkat-rest --realm team-alpha --instance rest-1 --realm-backend sqlite
```

Core endpoints:

- `GET /sessions` — list sessions
- `POST /sessions` — create and run a new session
- `GET /sessions/{id}` — get session details
- `GET /sessions/{id}/history` — get committed transcript history
- `DELETE /sessions/{id}` — archive (remove) a session
- `POST /sessions/{id}/interrupt` — interrupt an in-flight turn
- `POST /sessions/{id}/messages` — continue an existing session
- `GET /sessions/{id}/events` — SSE stream for agent events
- `POST /sessions/{id}/event` — (legacy) push external event
- `POST /requests/{request_id}/cancel` — cancel uncommitted in-flight work when `X-Meerkat-Request-Id` is supplied
- `GET /skills` — list skills with provenance
- `GET /health`
- `GET /models/catalog` — curated model catalog with provider profiles
- `GET /capabilities`
- `GET|PUT|PATCH /config`
- `POST /sessions/{id}/mcp/add` — stage live MCP server add (feature-gated)
- `POST /sessions/{id}/mcp/remove` — stage live MCP server remove (feature-gated)
- `POST /sessions/{id}/mcp/reload` — stage live MCP server reload (feature-gated)
- `POST /comms/send` (feature-gated)
- `GET /comms/peers` (feature-gated)
- `GET /mob/tools` — list mob tools (feature-gated)
- `POST /mob/call` — invoke a mob tool (feature-gated)
- `GET /mob/{id}/events` — SSE stream for mob events (feature-gated)

Config envelope shape (`GET/PUT/PATCH /config`):

```json
{
  "config": {"agent": {"model": "claude-sonnet-4-6"}},
  "generation": 4,
  "realm_id": "team-alpha",
  "instance_id": "rest-1",
  "backend": "sqlite",
  "resolved_paths": {
    "root": "...",
    "manifest_path": "...",
    "config_path": "...",
    "sessions_sqlite_path": "...",
    "sessions_jsonl_dir": "..."
  }
}
```

CAS writes:

```json
{"config": {...}, "expected_generation": 4}
{"patch": {...}, "expected_generation": 4}
```

---

## JSON-RPC (`rkat-rpc`)

Start the server (stdio is default; `--tcp <addr>` exposes the same protocol over TCP; some realtime hosts attach an optional websocket alongside):

```bash
rkat-rpc --realm team-alpha
rkat-rpc --realm team-alpha --tcp 127.0.0.1:9000
```

### Sessions, turns, history

- `initialize`
- `session/create`, `session/list`, `session/read`, `session/history`, `session/archive`
- `session/external_event`, `session/peer_response_terminal`, `session/inject_context`
- `session/stream_open` / `session/stream_close` — event streaming
- `events/latest_cursor`, `events/list_since`, `events/snapshot` — cursor-based event reads
- `turn/start` — accepts `model`, `provider`, `provider_params`, `max_tokens` for mid-session hot-swap
- `turn/interrupt`
- `blob/get` — fetch generated image / artifact payload bytes by blob id
- `artifact/list`, `artifact/get`, `artifact/download`

Generated assistant images appear in `session/history` as assistant blocks with `block_type: "image"`; fetch bytes via `blob/get` using `data.blob_ref.blob_id`.

CLI image generation is assistant-mediated; there is no direct `rkat image` command and no `rkat rpc` subcommand. Use:

```bash
rkat run --allow-tool generate_image "Use generate_image to create a 1024x1024 PNG of a meerkat on a laptop. Return the blob id."
rkat blob get <BLOB-ID> --output meerkat.png
rkat run "Use generate_image to create a 1024x1024 PNG of a meerkat on a laptop. Save it to meerkat.png." --yolo -m gpt-5.5
```

### Realtime (capability-driven)

- `realtime/open_info` — open audio channel info
- `realtime/status` — public realtime channel status projection for a target
- `realtime/capabilities` — capability projection
- `session/realtime_attachment_status` — single-session status (no batch sibling)
- `mob/member_status` includes `realtime_attachment_status` per member

Set `model: "gpt-realtime-1.5"` on `session/create` to enable realtime; transport follows the resolved model's `ModelCapabilities.realtime`.

### Auth (realm/binding model)

- `auth/profile/list`, `auth/profile/get`, `auth/profile/create`, `auth/profile/delete`
- `auth/login/start`, `auth/login/complete`
- `auth/login/device_start`, `auth/login/device_complete`
- `auth/login/provision_api_key`
- `auth/status/get`, `auth/logout`
- `realm/list`, `realm/get`

CLI auth forms:

```bash
rkat auth realms
rkat auth profiles
rkat auth profile <PROFILE_ID>
rkat auth profile-delete <PROFILE_ID> [-y|--yes]
rkat auth bindings
rkat auth test <BINDING_ID>
rkat auth status <PROFILE_ID>
rkat auth login [PROVIDER] [--backend <BACKEND>] [--method <METHOD>] [--non-interactive --secret <SECRET>]
rkat auth logout <PROFILE_ID>
rkat auth refresh <PROFILE_ID>
```

CLI `login` bootstraps fixed `dev:*` bindings. Interactive OAuth writes
`dev:anthropic_oauth`, `dev:openai_oauth`, or `dev:google_oauth`;
non-interactive api-key login writes `dev:default_<provider>`.

### Scheduling

- `schedule/create`, `schedule/get`, `schedule/list`, `schedule/update`
- `schedule/pause`, `schedule/resume`, `schedule/delete`
- `schedule/occurrences` — occurrences within the planning horizon
- `schedule/tools`, `schedule/call` — agent-facing schedule tool surface

### Skills, models, capabilities, runtime, approvals

- `skills/list`, `skills/inspect`
- `models/catalog`
- `capabilities/get`
- `runtime/host_info`, `runtime/capabilities`, `runtime/health`
- `approval/request`, `approval/list`, `approval/get`, `approval/decide`

### Config

- `config/get`, `config/set`, `config/patch` (same envelope + CAS as REST)

### MCP live mutation

- `mcp/add`, `mcp/remove`, `mcp/reload`
- These are JSON-RPC live session operations. CLI `rkat mcp add/remove/list/get` is the config surface.

### Mob (feature-gated)

Lifecycle and identity:

- `mob/create`, `mob/list`, `mob/status`, `mob/lifecycle`, `mob/destroy`, `mob/snapshot`
- `mob/members`, `mob/list_members_matching`, `mob/member_status`
- `mob/spawn`, `mob/spawn_many`, `mob/spawn_helper`, `mob/fork_helper`
- `mob/ensure_member`, `mob/reconcile`
- `mob/retire`, `mob/respawn`, `mob/force_cancel`
- `mob/wire`, `mob/unwire`, `mob/rotate_supervisor`

Member interaction and context:

- `mob/turn_start`, `mob/member_send`, `mob/ingress_interaction`
- `mob/append_system_context`

Work lanes (autonomous member work tracking):

- `mob/submit_work`, `mob/cancel_work`, `mob/cancel_all_work`

Waits and observation:

- `mob/wait_kickoff`, `mob/wait_ready`
- `mob/events`, `mob/stream_open`, `mob/stream_close`

Flows:

- `mob/flows`, `mob/flow_run`, `mob/flow_status`, `mob/flow_cancel`

Profiles (when a profile store is present):

- `mob/profile/create`, `mob/profile/get`, `mob/profile/list`, `mob/profile/update`, `mob/profile/delete`

### Comms (feature-gated)

- `comms/send`, `comms/peers`

### CLI parity (config)

```bash
rkat config get [--format toml|json] [--with-generation]
rkat config set [FILE] [--json <JSON> | --toml <TOML>] [--expected-generation <N>]
rkat config patch [FILE | --json <JSON>] [--expected-generation <N>]
```

CLI `get` defaults to raw TOML. `get --format json` returns raw config JSON.
`get --format json --with-generation` returns an envelope without
`resolved_paths`; `set`/`patch` print `generation=N`.

---

## MCP server (`rkat-mcp`)

Start scoped server:

```bash
rkat-mcp --realm team-alpha --instance mcp-1 --realm-backend sqlite
```

Core tools:

- `meerkat_run` — create and run a new session
- `meerkat_resume` — continue an existing session
- `meerkat_read` — get session details
- `meerkat_history` — get committed transcript history
- `meerkat_sessions` — list sessions
- `meerkat_interrupt` — cancel in-flight turn
- `meerkat_archive` — archive (remove) a session
- `meerkat_config` — get/set/patch config
- `meerkat_capabilities` — list runtime capabilities
- `meerkat_models_catalog` — curated model catalog with provider profiles
- `meerkat_skills` — list skills (`action: "list"`) or inspect one skill (`action: "inspect"`, typed `skill_key`, optional `source` UUID)
- `meerkat_mcp_add` — stage live MCP server add
- `meerkat_mcp_remove` — stage live MCP server remove
- `meerkat_mcp_reload` — stage live MCP server reload
- `meerkat_event_stream_open` / `meerkat_event_stream_read` / `meerkat_event_stream_close` — session event streaming

Mob tools (feature-gated, public MCP names — distinct from the agent-internal `mob_*` tool surface):

- Lifecycle: `meerkat_mob_create`, `meerkat_mob_list`, `meerkat_mob_status`, `meerkat_mob_lifecycle`
- Membership: `meerkat_mob_spawn`, `meerkat_mob_spawn_many`, `meerkat_mob_retire`, `meerkat_mob_respawn`, `meerkat_mob_force_cancel`, `meerkat_mob_wait_kickoff`
- Wiring: `meerkat_mob_wire`, `meerkat_mob_unwire`
- Member interaction: `meerkat_mob_member_send`, `meerkat_mob_append_system_context`
- Flows: `meerkat_mob_events`, `meerkat_mob_flows`, `meerkat_mob_flow_run`, `meerkat_mob_flow_status`, `meerkat_mob_flow_cancel`
- Profiles: `meerkat_mob_profile_create`, `meerkat_mob_profile_get`, `meerkat_mob_profile_list`, `meerkat_mob_profile_update`, `meerkat_mob_profile_delete`
- Event streams: `meerkat_mob_event_stream_open` / `meerkat_mob_event_stream_read` / `meerkat_mob_event_stream_close`

Schedule tools:

- `meerkat_schedule_create`, `meerkat_schedule_get`, `meerkat_schedule_list`, `meerkat_schedule_update`
- `meerkat_schedule_pause`, `meerkat_schedule_resume`, `meerkat_schedule_delete`
- `meerkat_schedule_occurrences`

Realtime tools:

- `meerkat_realtime_status`

Comms tools (feature-gated):

- `meerkat_comms_send` / `meerkat_comms_peers`

`meerkat_config` input:

```json
{"action":"get"}
{"action":"set","config":{...},"expected_generation":4}
{"action":"patch","patch":{...},"expected_generation":4}
```

Response includes the same config envelope fields.

---

## Python SDK

Connect options:

```python
await client.connect(
  realm_id="team-alpha",      # optional
  instance_id="py-worker-1",  # optional
  realm_backend="sqlite",     # optional creation hint
  isolated=False,             # optional — new generated realm
  state_root="/path",         # optional
  context_root="/path",       # optional
  user_config_root="/path",   # optional
)
```

Client methods:

- `create_session(prompt, *, model, auth_binding=None, ...)` → `Session` — `auth_binding` scopes credentials to a realm/binding
- `create_session_streaming(prompt, ...)` → `EventStream`
- `list_sessions()` → `list[SessionInfo]`
- `read_session(session_id)` → dict
- `read_session_history(session_id, offset=0, limit=None)` → `SessionHistory`
- `get_blob(blob_id)` → `BlobPayload`
- `create_mob(definition, ..., auth_binding=None)` → `Mob`
- `list_mobs()` → `list[MobSummary]`
- `get_config()` / `set_config(...)` / `patch_config(...)`
- `mcp_add(params)` / `mcp_remove(params)` / `mcp_reload(params)`
- `list_skills()`
- `capabilities` (property, populated during `connect()`)
- `get_runtime_host_info()` / `get_runtime_host_capabilities()` / `get_runtime_host_health()`

Realtime helpers:

- `runtime_realtime_attachment_status(session_id)` → status projection
- `realtime_open_info(session_id)` → audio channel info
- `realtime_capabilities()` / `realtime_status(params)`

Auth helpers:

- `auth_login_start(...)` / `auth_login_complete(...)`
- `auth_login_device_start(...)` / `auth_login_device_complete(...)`
- `auth_provision_api_key(...)`
- `auth_status(...)` / `auth_logout(...)`
- `list_auth_profiles(realm_id)` / `get_auth_profile(...)` / `create_auth_profile(...)` / `delete_auth_profile(...)`

Schedule helpers:

- `create_schedule(request)` / `get_schedule(id)` / `list_schedules(...)` / `update_schedule(request)`
- `pause_schedule(id)` / `resume_schedule(id)` / `delete_schedule(id)`
- `list_schedule_occurrences(id)`

Session methods:

- `Session.turn(prompt, ...)` → `RunResult`
- `Session.stream(prompt, ...)` → `EventStream`
- `Session.history(offset=0, limit=None)` → `SessionHistory`
- `Session.invoke_skill(skill_ref, prompt)` → `RunResult`
- `Session.interrupt()`
- `Session.archive()`
- `Session.send(**command)` / `Session.peers()`
- `Session.subscribe_events()` → `EventSubscription`

Mob methods:

- `Mob.id` (property) / `Mob.status()` / `Mob.lifecycle(action)`
- `Mob.spawn(...)` / `Mob.retire(agent_identity)` / `Mob.respawn(agent_identity)`
- `Mob.wire(a, b)` / `Mob.unwire(a, b)`
- `Mob.members()` / `Mob.member(agent_identity).send(content, handling_mode=...)`
- `Mob.flows()` / `Mob.run_flow(flow_id, params)` / `Mob.flow_status(run_id)` / `Mob.cancel_flow(run_id)`
- `Mob.subscribe_member_events(agent_identity)` → `EventSubscription`
- `Mob.subscribe_events()` → `EventSubscription`

`get_config()` / `patch_config()` return the config envelope.

Type/parsing notes:

- capability status may arrive as externally-tagged enum maps (e.g. `{"DisabledByPolicy": {...}}`) and is normalized to the tag string.
- event parsing defaults missing fields to empty/zero values to keep partial stream payloads parseable.
- `RunResult.skill_diagnostics` is typed as `SkillRuntimeDiagnostics`.
- generated image blocks preserve `image_id`, `blob_id`, `media_type`, `width`, `height`, `revised_prompt`, and provider `meta`.
- image-generation wire contracts are exported as `WireGenerateImageRequest`, `WireGenerateImageExecutionPlan`, `WireImageGenerationToolResult`, `WireImageOperationPhase`, and `WireAssistantImageRef`.

---

## TypeScript SDK

Package: `@rkat/sdk` (not `@meerkat/sdk`).

Connect options:

```ts
await client.connect({
  realmId: "team-alpha",      // optional
  instanceId: "ts-worker-1",  // optional
  realmBackend: "sqlite",     // optional creation hint
  isolated: true,             // optional — new generated realm
  stateRoot: "/path",         // optional
  contextRoot: "/path",       // optional
  userConfigRoot: "/path",    // optional
});
```

Client methods:

- `createSession(prompt, options?)` → `Session` — `options.authBinding` scopes credentials to a realm/binding
- `createSessionStreaming(prompt, options?)` → `EventStream`
- `listSessions()` → `SessionInfo[]`
- `readSession(sessionId)` → object
- `readSessionHistory(sessionId, { offset, limit }?)` → `SessionHistory`
- `getBlob(blobId)` → `BlobPayload`
- `createMob(definition, options?)` → `Mob` — accepts `authBinding`
- `listMobs()` → `MobSummary[]`
- `getConfig()` / `setConfig(...)` / `patchConfig(...)`
- `mcpAdd(params)` / `mcpRemove(params)` / `mcpReload(params)`
- `listSkills()`
- `capabilities` (property, populated during `connect()`)
- `getRuntimeHostInfo()` / `getRuntimeHostCapabilities()` / `getRuntimeHostHealth()`

Realtime helpers:

- `runtimeRealtimeAttachmentStatus(params)`
- `realtimeOpenInfo(params)`
- `realtimeCapabilities(params)` / `realtimeStatus(params)`

Auth helpers:

- `authLoginStart(...)` / `authLoginComplete(...)`
- `authLoginDeviceStart(...)` / `authLoginDeviceComplete(...)`
- `authLoginProvisionApiKey(...)`
- `authStatusGet(...)` / `authLogout(...)`
- `authProfileList(realmId)` / `authProfileGet(...)` / `authProfileCreate(...)` / `authProfileDelete(...)`

Schedule helpers:

- `createSchedule(request)` / `getSchedule(id)` / `listSchedules(options?)` / `updateSchedule(request)`
- `pauseSchedule(id)` / `resumeSchedule(id)` / `deleteSchedule(id)`
- `listScheduleOccurrences(id, ...)`
- `listScheduleTools()` / `callScheduleTool(request)`

Session methods:

- `Session.turn(prompt, options?)` → `RunResult`
- `Session.stream(prompt, options?)` → `EventStream`
- `Session.history({ offset, limit }?)` → `SessionHistory`
- `Session.invokeSkill(skillRef, prompt)` → `RunResult`
- `Session.interrupt()`
- `Session.archive()`
- `Session.send(command)` / `Session.peers()`
- `Session.subscribeEvents()` → `EventSubscription<AgentEventEnvelope>`

Mob methods:

- `Mob.status()` / `Mob.lifecycle(action)`
- `Mob.spawn(spec)` / `Mob.retire(agentIdentity)` / `Mob.respawn(agentIdentity)`
- `Mob.wire(a, b)` / `Mob.unwire(a, b)` — identity-keyed
- `Mob.listMembers()` / `Mob.sendMessage(agentIdentity, message)`
- `Mob.listFlows()` / `Mob.runFlow(flowId, params)` / `Mob.flowStatus(runId)` / `Mob.cancelFlow(runId)`
- `Mob.subscribeMemberEvents(meerkatId)` → `EventSubscription<AgentEventEnvelope>`
- `Mob.subscribeEvents()` → `EventSubscription<AttributedMobEvent>`

`getConfig()` / `patchConfig()` return the config envelope.

Type/parsing notes:

- capability status may arrive as externally-tagged enum maps (e.g. `{ DisabledByPolicy: {...} }`) and is normalized to the tag string.
- event parsing defaults missing fields to empty/zero values to keep partial stream payloads parseable.
- `RunResult.skillDiagnostics` is typed as `SkillRuntimeDiagnostics`.
- generated image blocks preserve `imageId`, `blobId`, `mediaType`, `width`, `height`, `revisedPrompt`, and provider `meta`.
- image-generation wire contracts are exported as `WireGenerateImageRequest`, `WireGenerateImageExecutionPlan`, `WireImageGenerationToolResult`, `WireImageOperationPhase`, and `WireAssistantImageRef`.

---

## Rust SDK

**AgentFactory vs AgentBuilder**: `AgentFactory` (facade crate) is the opinionated composition layer that
wires all tool categories (builtins, shell, comms, memory, mob, skills) into the dispatcher.
`AgentBuilder` (meerkat-core) is lower-level — it takes pre-built components and has no tool opinions.
All surfaces go through `AgentFactory`; direct `AgentBuilder` usage means manual dispatcher composition.

Recommended realm-aware factory bootstrap:

```rust
use meerkat::{AgentFactory, AgentBuildConfig};
use meerkat_core::Config;
use meerkat_store;

let config = Config::load().await?;
let realm = meerkat_store::realm_paths("team-alpha");
let factory = AgentFactory::new(realm.root.clone())
    .runtime_root(realm.root)
    .builtins(true)
    .shell(true)
    .mob(true);  // opt-in mob orchestration tools

let build = AgentBuildConfig::new("claude-sonnet-4-6");
let mut agent = factory.build_agent(build, &config).await?;
```

`AgentBuildConfig`/session metadata carry `realm_id`, `instance_id`, `backend`, and `config_generation`.

`AgentBuildConfig` also carries:
- `silent_comms_intents: Vec<String>` — intents injected silently (no LLM turn)
- `preload_skills: Option<Vec<SkillKey>>` — skills to inject at session creation
- `runtime_build_mode: RuntimeBuildMode` — required, determines ops lifecycle ownership

### Runtime build mode

All runtime-backed surfaces (CLI, RPC, REST, MCP) must use `SessionOwned` bindings. Standalone/test/WASM surfaces use `StandaloneEphemeral`.

```rust
use meerkat::{RuntimeBuildMode, SessionRuntimeBindings};
use meerkat_runtime::MeerkatMachine;
use meerkat_core::service::{CreateSessionRequest, SessionBuildOptions};

// Runtime-backed surface: prepare bindings from the adapter
let adapter = MeerkatMachine::persistent(store, blob_store);
let bindings = adapter.prepare_bindings(session_id.clone()).await?;
let build = SessionBuildOptions {
    runtime_build_mode: RuntimeBuildMode::SessionOwned(bindings),
    ..Default::default()
};

// Standalone/test/WASM: explicit opt-in (also the Default)
let build = SessionBuildOptions {
    runtime_build_mode: RuntimeBuildMode::StandaloneEphemeral,
    ..Default::default()
};
```

`prepare_bindings()` is the single canonical helper: it registers the session, mints the epoch, and returns `SessionRuntimeBindings { session_id, epoch_id, ops_lifecycle, cursor_state }`. The factory validates `bindings.session_id == session.id()` on `SessionOwned` builds.

Skill introspection (standalone, no session required):

```rust
if let Some(runtime) = factory.build_skill_runtime(&config).await {
    let entries = runtime.list_all_with_provenance(&SkillFilter::default()).await?;
    let doc = runtime.load_from_source(&"task-workflow".into(), Some("company")).await?;
}
```

---

## Comms, hooks, skills, multi-agent

- Inproc comms is namespace-scoped; realm namespace isolates peer discovery/sends.
- **Peer lifecycle typing**: mob lifecycle routing is typed at ingress. `mob.peer_added`, `mob.peer_retired`, and `mob.peer_unwired` are silent lifecycle notices; `mob.kickoff_failed` and `mob.kickoff_cancelled` are visible lifecycle notices. Do not depend on mob defaults in `silent_comms_intents` for canonical behavior.
- **Comms choice**: agents use `send_message` for ordinary collaboration, `send_request` for structured ask/reply, and `send_response` for replies. Public peer reservation streams were removed.
- Hooks and skills resolve from runtime root. Workspace-default CLI realms preserve project ergonomics.
- **Skill introspection**: `SkillRuntime::list_all_with_provenance()` returns active + shadowed skills; `load_from_source()` bypasses first-wins.
- Multi-agent orchestration uses mobs exclusively. `MemberLaunchMode::Fork` provides history branching via `Session::fork()`. `spawn_helper()`/`fork_helper()` provide one-call convenience.

---

## Flow spec essentials (mob definition)

Flow declarations live under `[flows.<flow_id>]`.

### v1: flat step declarations

Step declarations live under `[flows.<flow_id>.steps.<step_id>]`.

Key step fields:

- `role`
- `message`
- `depends_on`
- `depends_on_mode = "all"|"any"`
- `dispatch_mode = "one_to_one"|"fan_out"|"fan_in"`
- `collection_policy = { type = "any"|"all"|"quorum", ... }`
- `branch` (optional)
- `condition` (optional)
- `timeout_ms`
- `expected_schema_ref` (optional)

### v2: frame-based execution root

When `[flows.<flow_id>].root` is present, the flow uses frame-based execution via `FlowFrameEngine`.

Frame root declaration: `[flows.<flow_id>.root]` with `nodes` map of `FlowNodeSpec` entries.

Node types:

- `type = "step"` — a single step within a frame
- `type = "repeat_until"` — a loop node with `loop_id`, `body` (nested `FrameSpec`), `until` (condition expression), `max_iterations`, and `depends_on`

v2 flows still support flat `steps` for backward compatibility; when `root` is present it takes precedence.

### Topology contract

- `[topology] mode = "strict"|"permissive"`
- `rules = [{ from_role = "...", to_role = "...", allowed = true|false }]`
- wildcard `"*"` role matching is supported.

### Agent-facing delegation tools

`AgentMobToolSurface` provides 8 agent-internal mob tools:

| Tool | Purpose |
|------|---------|
| `delegate` | Quick helper spawn (implicit mob, auto-wire) |
| `mob_create` | Create a mob from a definition |
| `mob_destroy` | Destroy a mob and archive all members |
| `mob_spawn_member` | Spawn a member into any mob |
| `mob_retire_member` | Archive a member and its session |
| `mob_check_member` | Check a member's execution status and output |
| `mob_list_members` | List members of a mob |
| `mob_list` | List all mobs |

These tools are composed via `MobToolsFactory` late-binding. Operator capabilities are runtime-injected.
