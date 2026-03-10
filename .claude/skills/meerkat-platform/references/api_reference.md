# Meerkat Platform API Reference

## Realm scope (all surfaces)

Use explicit realm to control sharing/isolation.

- Same `realm_id` => shared sessions/config/backend.
- Different `realm_id` => isolated state.
- Backend is pinned per realm via `realm_manifest.json`.

Surface defaults when no realm is provided:

- CLI `run/resume/sessions`: workspace-derived `ws-...` realm.
- CLI `mob ...`: workspace-derived `ws-...` realm.
- RPC/REST/MCP/SDK: new opaque `realm-...` realm.

Mob contract notes:

- CLI `run`/`resume` include `mob_*` tools via `meerkat-mob-mcp` dispatcher composition.
- CLI `mob ...` commands provide explicit mob lifecycle operations.
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
--realm-backend <redb|jsonl>
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
rkat resume <SESSION-ID> <PROMPT>         # full UUID, short prefix, last, ~N
rkat continue <PROMPT>                   # shortcut for resume last (alias: c)
rkat sessions list [--limit N] [--offset N] [--label KEY=VALUE]
rkat sessions show <ID>
rkat sessions delete <ID>
rkat sessions interrupt <ID>
rkat sessions locate <LOCATOR> [--extra-state-root <PATH>]
rkat comms send <SESSION-ID> --json <JSON>        # (comms feature)
rkat comms peers <SESSION-ID>                     # (comms feature)
rkat realms current|list|show
rkat skills list [--json]
rkat skills inspect <ID> [--source <SOURCE>] [--json]
rkat mob prefabs|create|list|status|spawn|retire|respawn|wire|unwire|turn|stop|resume|complete|flows|run-flow|flow-status|events|destroy|pack|inspect|validate|deploy|web
rkat config get|set|patch ...
rkat capabilities
rkat-rpc
```

Flow command details:

```bash
rkat mob flows <MOB_ID>
rkat mob run-flow <MOB_ID> --flow <FLOW_ID> [--params '{"k":"v"}']
rkat mob flow-status <MOB_ID> <RUN_ID>
```

- `run-flow` returns `RUN_ID` and waits for terminal run state.
- `flow-status` returns serialized `MobRun` JSON or `null`.
- status values: `pending`, `running`, `completed`, `failed`, `canceled`.
- run records include `step_ledger` + `failure_ledger`.

Primary CLI mob usage is tool-driven from `run`/`resume` prompts using `mob_*` tools.
Mob lifecycle (non-flow) commands remain available as explicit operational/compatibility controls:

- `prefabs`
- `create`
- `list`
- `status`
- `spawn`
- `retire`
- `respawn` — retire + re-spawn same profile
- `wire`
- `unwire`
- `turn`
- `stop`
- `resume`
- `complete`
- `events` — stream mob events to stdout as JSON lines (optionally `--member <id>`)
- `destroy`

Artifact/deployment commands:

- `pack` — pack mob directory into `.mobpack` archive
- `inspect` — inspect a `.mobpack` archive
- `validate` — validate a `.mobpack` archive
- `deploy` — deploy a `.mobpack` with a prompt (`--trust-policy`, `--surface`)
- `web build` — build browser-deployable WASM bundle from a `.mobpack`

---

## REST API

Server boot:

```bash
rkat-rest --realm team-alpha --instance rest-1 --realm-backend redb
```

Core endpoints:

- `GET /sessions` — list sessions
- `POST /sessions` — create and run a new session
- `GET /sessions/{id}` — get session details
- `DELETE /sessions/{id}` — archive (remove) a session
- `POST /sessions/{id}/interrupt` — interrupt an in-flight turn
- `POST /sessions/{id}/messages` — continue an existing session
- `GET /sessions/{id}/events` — SSE stream for agent events
- `POST /sessions/{id}/event` — (legacy) push external event
- `GET /skills` — list skills with provenance
- `GET /skills/{id}` — inspect a skill's full body
- `GET /health`
- `GET /capabilities`
- `GET|PUT|PATCH /config`
- `POST /sessions/{id}/mcp/add` — stage live MCP server add (feature-gated)
- `POST /sessions/{id}/mcp/remove` — stage live MCP server remove (feature-gated)
- `POST /sessions/{id}/mcp/reload` — stage live MCP server reload (feature-gated)
- `POST /comms/send` (feature-gated)
- `GET /comms/peers` (feature-gated)
- `GET /mob/prefabs` — list mob prefab templates (feature-gated)
- `GET /mob/tools` — list mob tools (feature-gated)
- `POST /mob/call` — invoke a mob tool (feature-gated)
- `GET /mob/{id}/events` — SSE stream for mob events (feature-gated)

Config envelope shape (`GET/PUT/PATCH /config`):

```json
{
  "config": {"agent": {"model": "claude-sonnet-4-5"}},
  "generation": 4,
  "realm_id": "team-alpha",
  "instance_id": "rest-1",
  "backend": "redb",
  "resolved_paths": {
    "root": "...",
    "manifest_path": "...",
    "config_path": "...",
    "sessions_redb_path": "...",
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

Start scoped server:

```bash
rkat-rpc --realm team-alpha
```

Core methods:

- `initialize`
- `session/create`
- `session/list`
- `session/read`
- `session/archive`
- `turn/start`
- `turn/interrupt`
- `config/get`
- `config/set`
- `config/patch`
- `skills/list` — list skills with provenance (active + shadowed)
- `skills/inspect` — inspect a skill's full body by ID
- `capabilities/get`
- `mcp/add` — stage live MCP server add for a session
- `mcp/remove` — stage live MCP server remove
- `mcp/reload` — stage live MCP server reload
- `mob/prefabs` — list built-in mob prefab templates (feature-gated)
- `session/stream_open` / `session/stream_close` — standalone session event streaming
- `mob/create`, `mob/list`, `mob/status`, `mob/members` — explicit mob lifecycle/state methods (feature-gated)
- `mob/spawn`, `mob/retire`, `mob/respawn`, `mob/wire`, `mob/unwire`, `mob/lifecycle`, `mob/send` — explicit mob control methods (feature-gated)
- `mob/events`, `mob/stream_open` / `mob/stream_close` — mob/member observation (feature-gated)
- `mob/append_system_context`, `mob/flows`, `mob/flow_run`, `mob/flow_status`, `mob/flow_cancel` — advanced mob runtime methods (feature-gated)
- `mob/tools` / `mob/call` — compatibility and escape-hatch mob tool access (feature-gated)
- `comms/send` (feature-gated)
- `comms/peers` (feature-gated)

`config/*` uses the same envelope + CAS semantics as REST.

CLI parity:

```bash
rkat config get --format json --with-generation
rkat config set --file config.toml --expected-generation 4
rkat config patch --json '{"agent":{"model":"gpt-5.2"}}' --expected-generation 4
```

---

## MCP server (`rkat-mcp`)

Start scoped server:

```bash
rkat-mcp --realm team-alpha --instance mcp-1 --realm-backend redb
```

Core tools:

- `meerkat_run` — create and run a new session
- `meerkat_resume` — continue an existing session
- `meerkat_read` — get session details
- `meerkat_sessions` — list sessions
- `meerkat_interrupt` — cancel in-flight turn
- `meerkat_archive` — archive (remove) a session
- `meerkat_config` — get/set/patch config
- `meerkat_capabilities` — list runtime capabilities
- `meerkat_skills` — list (`action: "list"`) or inspect (`action: "inspect"`, `skill_id: "..."`) skills
- `meerkat_mcp_add` — stage live MCP server add
- `meerkat_mcp_remove` — stage live MCP server remove
- `meerkat_mcp_reload` — stage live MCP server reload
- `meerkat_event_stream_open` / `meerkat_event_stream_read` / `meerkat_event_stream_close` — session event streaming

Mob tools (feature-gated):

- `meerkat_mob_prefabs` — list prefab templates
- `mob_create`, `mob_list`, `mob_lifecycle`, etc. — mob lifecycle tools (via `meerkat-mob-mcp`)
- `meerkat_mob_event_stream_open` / `meerkat_mob_event_stream_read` / `meerkat_mob_event_stream_close`

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
  realm_backend="redb",       # optional creation hint
  isolated=False,             # optional — new generated realm
  state_root="/path",         # optional
  context_root="/path",       # optional
  user_config_root="/path",   # optional
)
```

Client methods:

- `create_session(prompt, ...)` → `Session`
- `create_session_streaming(prompt, ...)` → `EventStream`
- `list_sessions()` → `list[SessionInfo]`
- `read_session(session_id)` → dict
- `create_mob(definition, ...)` → `Mob`
- `list_mobs()` → `list[MobSummary]`
- `get_config()` / `set_config(...)` / `patch_config(...)`
- `mcp_add(params)` / `mcp_remove(params)` / `mcp_reload(params)`
- `list_skills()` / `inspect_skill(id, source?)`
- `capabilities` (property, populated during `connect()`)

Session methods:

- `Session.turn(prompt, ...)` → `RunResult`
- `Session.stream(prompt, ...)` → `EventStream`
- `Session.invoke_skill(skill_ref, prompt)` → `RunResult`
- `Session.interrupt()`
- `Session.archive()`
- `Session.send(**command)` / `Session.peers()`
- `Session.subscribe_events()` → `EventSubscription`

Mob methods:

- `Mob.status()` / `Mob.lifecycle(action)`
- `Mob.spawn(spec)` / `Mob.retire(meerkat_id)` / `Mob.respawn(meerkat_id)`
- `Mob.wire(a, b)` / `Mob.unwire(a, b)`
- `Mob.list_members()` / `Mob.send_message(meerkat_id, message)`
- `Mob.list_flows()` / `Mob.run_flow(flow_id, params)` / `Mob.flow_status(run_id)` / `Mob.cancel_flow(run_id)`
- `Mob.subscribe_member_events(meerkat_id)` → `EventSubscription`
- `Mob.subscribe_events()` → `EventSubscription`

`get_config()` / `patch_config()` return the config envelope.

Type/parsing notes:

- capability status may arrive as externally-tagged enum maps (e.g. `{"DisabledByPolicy": {...}}`) and is normalized to the tag string.
- event parsing defaults missing fields to empty/zero values to keep partial stream payloads parseable.
- `RunResult.skill_diagnostics` is typed as `SkillRuntimeDiagnostics`.

---

## TypeScript SDK

Package: `@rkat/sdk` (not `@meerkat/sdk`).

Connect options:

```ts
await client.connect({
  realmId: "team-alpha",      // optional
  instanceId: "ts-worker-1",  // optional
  realmBackend: "redb",       // optional creation hint
  isolated: true,             // optional — new generated realm
  stateRoot: "/path",         // optional
  contextRoot: "/path",       // optional
  userConfigRoot: "/path",    // optional
});
```

Client methods:

- `createSession(prompt, options?)` → `Session`
- `createSessionStreaming(prompt, options?)` → `EventStream`
- `listSessions()` → `SessionInfo[]`
- `readSession(sessionId)` → object
- `createMob(definition, options?)` → `Mob`
- `listMobs()` → `MobSummary[]`
- `getConfig()` / `setConfig(...)` / `patchConfig(...)`
- `mcpAdd(params)` / `mcpRemove(params)` / `mcpReload(params)`
- `listSkills()` / `inspectSkill(id, options?)`
- `capabilities` (property, populated during `connect()`)

Session methods:

- `Session.turn(prompt, options?)` → `RunResult`
- `Session.stream(prompt, options?)` → `EventStream`
- `Session.invokeSkill(skillRef, prompt)` → `RunResult`
- `Session.interrupt()`
- `Session.archive()`
- `Session.send(command)` / `Session.peers()`
- `Session.subscribeEvents()` → `EventSubscription<AgentEventEnvelope>`

Mob methods:

- `Mob.status()` / `Mob.lifecycle(action)`
- `Mob.spawn(spec)` / `Mob.retire(meerkatId)` / `Mob.respawn(meerkatId)`
- `Mob.wire(a, b)` / `Mob.unwire(a, b)`
- `Mob.listMembers()` / `Mob.sendMessage(meerkatId, message)`
- `Mob.listFlows()` / `Mob.runFlow(flowId, params)` / `Mob.flowStatus(runId)` / `Mob.cancelFlow(runId)`
- `Mob.subscribeMemberEvents(meerkatId)` → `EventSubscription<AgentEventEnvelope>`
- `Mob.subscribeEvents()` → `EventSubscription<AttributedMobEvent>`

`getConfig()` / `patchConfig()` return the config envelope.

Type/parsing notes:

- capability status may arrive as externally-tagged enum maps (e.g. `{ DisabledByPolicy: {...} }`) and is normalized to the tag string.
- event parsing defaults missing fields to empty/zero values to keep partial stream payloads parseable.
- `RunResult.skillDiagnostics` is typed as `SkillRuntimeDiagnostics`.

---

## Rust SDK

**AgentFactory vs AgentBuilder**: `AgentFactory` (facade crate) is the opinionated composition layer that
wires all tool categories (builtins, shell, sub-agents, comms, memory, mob, skills) into the dispatcher.
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
    .subagents(true)
    .mob(true);  // opt-in mob orchestration tools

let build = AgentBuildConfig::new("claude-sonnet-4-5");
let mut agent = factory.build_agent(build, &config).await?;
```

`AgentBuildConfig`/session metadata carry `realm_id`, `instance_id`, `backend`, and `config_generation`.

`AgentBuildConfig` also carries:
- `silent_comms_intents: Vec<String>` — intents injected silently (no LLM turn)
- `preload_skills: Option<Vec<SkillId>>` — skills to inject at session creation

Skill introspection (standalone, no session required):

```rust
if let Some(runtime) = factory.build_skill_runtime(&config).await {
    let entries = runtime.list_all_with_provenance(&SkillFilter::default()).await?;
    let doc = runtime.load_from_source(&"task-workflow".into(), Some("company")).await?;
}
```

---

## Comms, hooks, skills, sub-agents

- Inproc comms is namespace-scoped; realm namespace isolates peer discovery/sends.
- **Silent comms intents**: `AgentBuildConfig.silent_comms_intents` suppresses LLM turns for informational intents. Mob meerkats default to `["mob.peer_added", "mob.peer_retired"]`.
- Hooks and skills resolve from runtime root. Workspace-default CLI realms preserve project ergonomics.
- **Skill introspection**: `SkillRuntime::list_all_with_provenance()` returns active + shadowed skills; `load_from_source()` bypasses first-wins.
- Sub-agents with comms enabled inherit parent realm namespace for inproc communication.

---

## Flow spec essentials (mob definition)

Flow declarations live under `[flows.<flow_id>]`.
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

Topology contract:

- `[topology] mode = "strict"|"permissive"`
- `rules = [{ from_role = "...", to_role = "...", allowed = true|false }]`
- wildcard `"*"` role matching is supported.
