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
--realm <id>
--instance <id>
--realm-backend <redb|jsonl>
```

Core commands:

```bash
rkat run <PROMPT> [OPTIONS]
rkat resume <SESSION-ID> <PROMPT>
rkat sessions list [--limit N]
rkat sessions show <ID>
rkat sessions delete <ID>
rkat mob prefabs|create|list|status|spawn|retire|wire|unwire|turn|stop|resume|complete|flows|run-flow|flow-status|destroy
rkat config get|set|patch ...
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
- `wire`
- `unwire`
- `turn`
- `stop`
- `resume`
- `complete`
- `destroy`

---

## REST API

Server boot:

```bash
rkat-rest --realm team-alpha --instance rest-1 --realm-backend redb
```

Core endpoints:

- `POST /sessions`
- `POST /sessions/{id}/messages`
- `GET /sessions/{id}`
- `POST /sessions/{id}/event`
- `GET /sessions/{id}/events`
- `GET /health`
- `GET /capabilities`
- `GET|PUT|PATCH /config`
- `POST /comms/send` (feature-gated)
- `GET /comms/peers` (feature-gated)

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
- `capabilities/get`
- `comms/send` (feature-gated)
- `comms/peers` (feature-gated)
- `comms/stream_open` / `comms/stream_close` (feature-gated)

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

Tools exposed:

- `meerkat_run`
- `meerkat_resume`
- `meerkat_config`
- `meerkat_capabilities`

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
)
```

Core methods:

- `create_session(...)`
- `start_turn(session_id, prompt, ...)`
- `interrupt(session_id)`
- `list_sessions()`
- `read_session(session_id)`
- `archive_session(session_id)`
- `get_config()` / `set_config(...)` / `patch_config(...)`
- `get_capabilities()`

`get_config()` / `patch_config()` return the config envelope.

---

## TypeScript SDK

Connect options:

```ts
await client.connect({
  realmId: "team-alpha",      // optional
  instanceId: "ts-worker-1",  // optional
  realmBackend: "redb",       // optional creation hint
});
```

Core methods:

- `createSession(...)`
- `startTurn(sessionId, prompt, ...)`
- `interrupt(sessionId)`
- `listSessions()`
- `readSession(sessionId)`
- `archiveSession(sessionId)`
- `getConfig()` / `setConfig(...)` / `patchConfig(...)`
- `getCapabilities()`

`getConfig()` / `patchConfig()` return the config envelope.

---

## Rust SDK

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
    .subagents(true);

let build = AgentBuildConfig::new("claude-sonnet-4-5");
let mut agent = factory.build_agent(build, &config).await?;
```

`AgentBuildConfig`/session metadata carry `realm_id`, `instance_id`, `backend`, and `config_generation`.

---

## Comms, hooks, skills, sub-agents

- Inproc comms is namespace-scoped; realm namespace isolates peer discovery/sends.
- Hooks and skills resolve from runtime root. Workspace-default CLI realms preserve project ergonomics.
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
