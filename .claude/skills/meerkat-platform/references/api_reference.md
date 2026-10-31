# Meerkat Platform API Reference

For upgrade and terminology changes, also load:

- `references/migration_0_9.md` (moving a 0.8 system to 0.9)
- `references/migration_0_5.md` (only when migrating from 0.4x/0.5)

## Realm scope (all surfaces)

Use explicit realm to control sharing/isolation.

- Same `realm_id` plus the same physical storage provider/root => shared
  sessions and state.
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
- RPC/REST/MCP/SDK host integrations use the typed `mob/*` or `meerkat_mob_*`
  control planes. Agent-facing `mob_*` tools are composed through
  `SessionBuildOptions.mob_tools` with `MobMcpState` +
  `AgentMobToolSurfaceFactory`; `external_tools` remains for callback and
  MCP-backed dispatchers.

For full mob behavior details (runtime model and flows), also load
`references/mobs.md`. For the intentional multi-host exposure differences in
particular, use its
[multi-host surface matrix](mobs.md#multi-host-surface-matrix).

---

## CLI

Global flags (available on all commands):

```bash
--realm <id>              # explicit realm ID
--isolated                # start in isolated mode (new generated realm)
--instance <id>           # optional instance ID inside a realm
--realm-backend <sqlite|jsonl|memory>
--state-root <path>       # explicit realm state root (no probing); otherwise the realm is used where it already exists (<project-root>/.rkat/realms or the user-global <data dir>/meerkat/realms), else <project-root>/.rkat/realms
--context-root <path>     # realm identity + project files root; default CWD
--user-config-root <path> # optional user-global convention root
--as-agent <AGENT>        # governed realm: act as this agent (default rkat-agent)
--default-model <MODEL>   # persist the default agent model into the scope-resolved config
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
rkat session export-atif <ID> [--output <FILE>]   # ATIF trajectory; default realm trajectory dir
rkat session repair-wholeblob <ID> [--apply [--accept-shorter]] [--runtime-store <FILE>] [--json]  # diagnose by default
rkat session resolve-held-tail <ID> [--apply] [--runtime-store <FILE>] [--json]  # report by default
rkat blob get <BLOB-ID> [--output <FILE>] [--json]
rkat realm current|list|show|create|delete|prune ...
rkat realm govern|grant|revoke-grant <REALM> ...  # governed realms (see below)
rkat mcp add|login|remove|list|get ...
rkat skill add <PATH> [--name <NAME>]
rkat skill remove|get <NAME_OR_SOURCE_UUID_OR_PATH> [--json for get]
rkat skill list [--json]
rkat skill inspect <SKILL_NAME> --source-uuid <SOURCE_UUID> [--json]
rkat workgraph list|show|ready|snapshot|events ...
rkat models
rkat help <QUESTION> [--prompt <PROMPT>] [--plan-execution] [--json] [--stream|--no-stream]
rkat auth realms|profiles|profile|profile-delete|bindings|test|status|login|logout|refresh|connector-client ...
# (Scheduling has no top-level CLI subcommand. Schedules are managed
# through the agent tools `meerkat_schedule_*` from `rkat run`,
# the RPC `schedule/*` methods, REST schedule endpoints, or SDK helpers.
# Not available on JSONL-backed realms or in the browser runtime.)
rkat mob spawn-helper|fork-helper|member-status|force-cancel|respawn|wait-kickoff|host|bind-host|revoke-host|hosts|grant|revoke-grant|grants|member-history|route-installs|live|run|runs|status|logs|attach|run-flow|flow-status|pack|inspect|validate|deploy|web ...
rkat config get|set|patch ...
rkat capabilities
rkat doctor
rkat storage doctor [--json] [--root PATH]...       # read-only storage diagnosis
rkat storage migrate [--apply] [--bridge-pre-0-8-10] [--adopt-root PATH]  # offline fenced migration (dry-run default)
rkat storage prune [--apply] [--older-than-days N]  # registered backup-artifact lifecycle
rkat run --comms-listen-tcp 0.0.0.0:4200 --comms-advertise-tcp 203.0.113.7:4200 --comms-binding-out target.binding.json --keep-alive "..."
rkat-rpc                                # JSON-RPC stdio
rkat-rpc --tcp 127.0.0.1:9000           # JSON-RPC over TCP (stdio is default)
rkat-rpc --tcp 127.0.0.1:9000 --live-ws 127.0.0.1:9001
```

CLI keep-alive terminology:

- use `--keep-alive`
- do not use `--host`

`rkat init` has no command-specific options. Shared realm/context flags are
parsed by Clap but do not redirect the project config path or global config
source; it writes `./.rkat/config.toml` from `~/.rkat/config.toml`.

Important `rkat run` options:

```bash
--resume[=<SESSION>]                  # UUID, prefix/tail handle, realm:<uuid>, last, ~, ~N
-t, --tools <safe|workspace|full|none> # default safe; --yolo aliases full
--allow-tool <TOOL>                    # repeatable first-turn allow overlay
--block-tool <TOOL>                    # repeatable first-turn block overlay
--wait-for-mcp
--mcp-auth <stored|interactive>
--auth-binding <REALM:BINDING[:PROFILE]>
--output <text|json|html> / --json / --html / --browser
--stream / --no-stream
--stdin <auto|blob|lines|off>
--line-format <text|json>
--export-atif                          # write an ATIF trajectory when the turn completes
```

Hard CLI negatives: no `rkat sessions`, no `rkat resume`, no `rkat rpc`, no
`rkat live`, no `rkat image`, no `--tools all`, no `rkat blob get -o`, no
`rkat session show --json`, no `rkat models --json`, no
`rkat capabilities --json`.

### Governed realms (CLI)

Default `rkat` builds include the `local-authorization` feature:

```bash
rkat realm govern <REALM>
rkat realm grant <REALM> [--agent <AGENT>] (--mcp <SERVER>=<OP>[,<OP>...] | --mcp <SERVER>=* | --catalog | --controller --route <ROUTE_KEY>...) [--expires-in-secs <N>]
rkat realm revoke-grant <REALM> [--agent <AGENT>] (--mcp <SERVER> | --catalog | --controller | --worker)
rkat --realm <REALM> run --as-agent <AGENT> "..."
```

`govern` durably selects governed posture for an existing realm and provisions
its grant owner with the local user as operator; every later `rkat` process
opens that realm governed (no flag selects it). `grant` issues one grant per
target (an installed MCP server and its raw operations, the tool catalog, or
the agent's controller routes) and supersedes only that target's previous
grant; a controller grant takes no expiry. `--agent` defaults to `rkat-agent`.
Review tiers above R1 come from the realm config's `[[governance.review.rules]]`;
ordinary config writes never change the `governance` section (RPC and MCP
refuse with invalid params and `data: {"type": "governance_administration_required"}`).

### WorkGraph host observability

WorkGraph is a realm-scoped durable commitment graph. Agents mutate it through
the `workgraph_*` tool family when `enable_workgraph` / `tools.workgraph_enabled`
is active. Host REST/RPC/SDK surfaces provide observability, while CLI and
trusted in-process hosts also expose narrow goal/attention controls:

```bash
rkat workgraph list [--namespace <NS>] [--all-namespaces] [--status <STATUS>] [--label <LABEL>] [--include-terminal] [--limit <N>] [--json]
rkat workgraph show <ID> [--namespace <NS>] [--json]
rkat workgraph ready [--namespace <NS>] [--label <LABEL>] [--limit <N>] [--json]
rkat workgraph snapshot [--namespace <NS>] [--all-namespaces] [--status <STATUS>] [--label <LABEL>] [--include-terminal] [--limit <N>] [--json]
rkat workgraph events [--namespace <NS>] [--all-namespaces] [--after-seq <N>] [--limit <N>] [--json]
rkat workgraph goal-create <SESSION_ID> <TITLE> [--namespace <NS>] [--description <TEXT>] [--mode pursue|coordinate|review|falsify|judge|observe] [--completion-policy self-attest|host-confirmed] [--json]
rkat workgraph goal-status <BINDING_ID> [--namespace <NS>] [--json]
rkat workgraph goal-confirm <BINDING_ID> --expected-revision <N> --kind <KIND> --id <ID> [--namespace <NS>] [--label <TEXT>] [--summary <TEXT>] [--json]
rkat workgraph goal-close <BINDING_ID> --expected-revision <N> [--namespace <NS>] [--status completed|cancelled|failed] [--json]
rkat workgraph attention-list [--namespace <NS>] [--status active|paused|stopped|superseded] [--json]
rkat workgraph attention-pause <BINDING_ID> --expected-revision <N> [--namespace <NS>] [--json]
rkat workgraph attention-resume <BINDING_ID> --expected-revision <N> [--namespace <NS>] [--json]
```

These namespace flags are syntactically valid, but the current CLI composes a
grant for the active realm's `default` namespace only. It rejects non-default
`--namespace` requests and `--all-namespaces` scans. Broader access needs
separate host capability composition; the flags cannot widen the grant.

RPC exposes `workgraph/get`, `workgraph/list`, `workgraph/ready`,
`workgraph/snapshot`, `workgraph/events`, `workgraph/goal/status`, and
`workgraph/attention/list`. REST exposes
`GET /workgraph/items`, `/workgraph/items/{id}`, `/workgraph/ready`,
`/workgraph/snapshot`, `/workgraph/events`, `POST /workgraph/goal/status`, and
`POST /workgraph/attention/list`. Python and TypeScript SDKs wrap the same
read-only observability surface.

### MCP CLI config surface

`rkat mcp` edits project, user, or realm MCP server config; it is not the live mutation surface for an already-running session.

```bash
rkat mcp add <NAME> [--transport stdio|http|sse] [--scope project|user|local|realm] [-H KEY:VALUE...] [-e KEY=VALUE...] [--oauth-account <ACCOUNT_ID> | --oauth-account-selection discover|unverified] [--allow-operation <OP>...] [--expected-generation <N>] [--url <URL> | <URL> | -- <CMD...>]
rkat mcp login <NAME> [--scope project|user|local|realm] [--allow-headless]
rkat mcp remove <NAME> [--scope project|user|local|realm] [--expected-generation <N>]
rkat mcp list [--scope project|user|local|realm] [--json]
rkat mcp get <NAME> [--scope project|user|local|realm] [--json]
```

`add` defaults to `--scope project`; `local` is an alias for `project`.
`--scope realm` writes the selected realm's own config (`[[tools.mcp_servers]]`,
inherited by child realms), and `--expected-generation` guards that write.
`--allow-operation` (repeatable) allows only the named raw MCP operations; every
other operation is refused and not offered.

Examples:

```bash
rkat mcp add filesystem -- npx -y @modelcontextprotocol/server-filesystem .
rkat mcp add linear --url https://mcp.example.com
rkat mcp add --transport http remote-tools https://mcp.example.com/api --oauth-account-selection discover
rkat mcp login remote-tools
rkat mcp list
```

HTTP OAuth endpoints are discovered at connect or login time; no auth schema is stored. An `mcp.toml` entry has `name`, then `command`/`args`/`env` (stdio) or `url`/`headers`/`transport` (HTTP), and optionally `connect_timeout_secs`, `tool_names`, `access`, `oauth_account` or `oauth_account_selection` ("discover" | "unverified"). `rkat mcp add` writes the account selection: `--oauth-account <ACCOUNT_ID>` (a provider subject ID, not an email), `--oauth-account-selection discover` (bind the account the provider verifies at the first login) or `--oauth-account-selection unverified` (resource-bound access with no account evidence); the flags conflict. `rkat mcp login <name>` performs explicit browser login and needs a streamable HTTP server without a static `Authorization` header and with an account selection; without one it refuses before any network request (`MCP server '<name>' has no OAuth account selection...`) and prints the `rkat mcp remove`/`rkat mcp add` commands for each choice. A `discover` or `oauth_account` server whose issuer publishes no OpenID Connect UserInfo is refused before client registration, nothing stored, naming `unverified` as the deliberate alternative; login never changes the selection. RPC and REST hosts see these refusals as the auth reasons `account_selection_required` and `verification_unavailable`. `rkat run` defaults to `--mcp-auth stored`; `--mcp-auth interactive` lets a TTY run authenticate and retry when the server has any account selection.

### Mob CLI surface

Primary CLI mob usage is tool-driven from `run`/`run --resume` prompts using `mob_*` tools with `--tools full` or config `tools.mob_enabled=true`. The explicit `rkat mob <subcommand>` surface is helper-oriented — a small set of operational verbs:

| Subcommand | Purpose |
|------------|---------|
| `spawn-helper <mob_id> <prompt> --agent-identity <id> --result-label <label> --max-text-bytes <n> [--profile <profile>] [--model <model>] [--auth-binding <REALM:BINDING[:PROFILE]>] [--json]` | Spawn a short-lived helper, wait for completion, print the result |
| `fork-helper <mob_id> <source_member> <prompt> --agent-identity <id> --result-label <label> --max-text-bytes <n> [--profile <profile>] [--model <model>] [--auth-binding <REALM:BINDING[:PROFILE]>] [--fork-context full-history\|last-messages] [--last-messages N] [--json]` | Fork from an existing member's context and run a helper |
| `member-status <mob_id> <agent_identity> [--json]` | Execution status snapshot for a mob member |
| `force-cancel <mob_id> <agent_identity>` | Force-cancel a member's in-flight turn |
| `respawn <mob_id> <agent_identity> [--initial-message]` | Retire and respawn a member with the same profile |
| `wait-kickoff <mob_id> [--member ...] [--timeout-ms] [--json]` | Wait for autonomous-host kickoff turns to complete |
| `host [--listen-tcp ...] [--advertise-tcp tcp://HOST:PORT] [--descriptor-out ...]` | Run the member-host daemon and write its single-use binding descriptor (a non-loopback bind needs `--allow-remote` and `--advertise-tcp`) |
| `bind-host <mob_id> --descriptor <path>` / `revoke-host <mob_id> <host_id>` / `hosts <mob_id>` | Bind, revoke, and observe managed member hosts |
| `grant <mob_id> <principal> --scope ...` / `revoke-grant ...` / `grants <mob_id>` | Manage remote-console control scopes |
| `member-history <mob_id> <agent_identity>` / `route-installs <mob_id>` | Read placed-member history and route-install obligations |
| `live open\|close\|status\|control ...` | Operate a member live channel; there is intentionally no `live send` verb |
| `run <pack_or_mob_id> [--flow] [--param] [--prompt] [--detach] [--json] [--trust-policy]` | Invoke a mobpack or installed mob as a typed callable run; `--prompt` binds `params.prompt` |
| `runs <mob_id> [--flow] [--json]` | List persisted run resources for a mob |
| `status <mob_id> <run_id> [--json]` | Read one run resource status |
| `logs <mob_id> [--after-cursor] [--limit] [--json]` | Read mob event history for run diagnostics |
| `attach <mob_id> <run_id> [--json]` | Wait for a detached run and print its typed output envelope |
| `run-flow <mob_id> --flow <flow_id> [--params <json>] [-s\|--stream] [--no-stream]` | Start a flow, stream member output (default on a TTY), wait with no timeout until the run is terminal, then print only the run id; read the outcome with `flow-status` or `status`. `--params` must be a JSON object |
| `flow-status <mob_id> <run_id>` | Print live or terminal `MobRun` JSON; status: `pending`/`running`/`completed`/`failed`/`canceled` |
| `pack <dir> -o <pack> [--sign <key> --signer-id <id>]` | Pack a mob directory into a `.mobpack` archive |
| `inspect <pack>` | Inspect a `.mobpack` archive |
| `validate <pack> [--trust-policy permissive\|strict]` | Validate a `.mobpack` archive |
| `deploy <pack> <prompt> [--model] [--max-total-tokens] [--max-duration] [--max-tool-calls] [--trust-policy] [--surface cli\|rpc]` | Validate a `.mobpack`, build its mob in memory and queue the prompt to the orchestrator. `--surface cli` (default) returns at once and the mob ends with the command; `--surface rpc` serves JSON-RPC on stdin/stdout until EOF. Orchestrator-less packs warn that prompts are not delivered and flows are not started. Since 0.9.1 the budget flags and a pack's `[budget]` set the members' `[limits]` (a flag wins over the realm's `[limits]`, which wins over the pack default). Since 0.9.3 the `--surface rpc` host builds every member and session from the same deploy config, so deploy flags and pack overlays apply there too, as defaults that an explicit `session/create` or mob spawn parameter replaces |
| `web build <pack> -o <dir> --wasm <PKG_DIR\|name_bg.wasm> [--trust-policy]` | Build a browser-deployable WASM bundle from required prebuilt wasm-pack output |

Lifecycle verbs that used to live on the CLI (`create`, `spawn`, `retire`, `wire`, `unwire`, `turn`, `stop`, `resume`, `complete`, `events`, `destroy`) are reached through the agent tools (`mob_create`, `mob_spawn_member`, `mob_wire`, ...) or through the RPC `mob/*` methods listed below.

Signing a pack does not install its signer in a trust store. The executable
local path for an uninstalled signer is explicit permissive mode:

```bash
rkat mob validate ./dist/release-triage.mobpack --trust-policy permissive
rkat mob run ./dist/release-triage.mobpack --flow main --trust-policy permissive
rkat mob web build ./dist/release-triage.mobpack -o ./dist/release-triage-web --wasm <PKG_DIR|name_bg.wasm> --trust-policy permissive
```

---

## REST API

Server boot:

```bash
rkat-rest --realm team-alpha --instance rest-1 --realm-backend sqlite
```

Core endpoints:

- `POST /help` — ask Meerkat usage help with the embedded platform skill
- `GET /sessions` — list sessions
- `POST /sessions` — create and run a new session
- `GET /sessions/{id}` — get session details
- `GET /sessions/{id}/status` — get a session's current runtime state
- `GET /sessions/{id}/history` — get committed transcript history
- `DELETE /sessions/{id}` — archive (remove) a session
- `POST /sessions/{id}/interrupt` — interrupt an in-flight turn
- `POST /sessions/{id}/runs/{run_id}/stop` - stop one exact run and terminalize every contributor bound to it
- `POST /sessions/{id}/system_context` - append one ordinary durable ordered System message (`content`, optional `source` and `idempotency_key`; RPC twin `session/inject_context`)
- `POST /sessions/{id}/system_prompt` - replace one durable versioned system-prompt key
- `POST /sessions/{id}/messages` — continue an existing session
- `POST /sessions/{id}/external-events` — queue a runtime-backed external event
- `POST /sessions/{id}/peer-response-terminal` — admit a correlated terminal peer response
- `GET /sessions/{id}/events` — SSE stream for agent events
- `POST /requests/{request_id}/cancel` — cancel uncommitted in-flight work when `X-Meerkat-Request-Id` is supplied
- `GET /schedule/tools`, `POST /schedule/call`
- `GET|POST /schedules`
- `GET|PATCH|DELETE /schedules/{id}`
- `POST /schedules/{id}/pause`, `POST /schedules/{id}/resume`
- `GET /schedules/{id}/occurrences`
- `GET /skills` — list skills with provenance
- `GET /health`
- `GET /models/catalog` — effective model catalog: built-in models plus `[models.<id>]` and self-hosted aliases, with provider defaults and per-model profiles
- `GET /capabilities`
- `GET /runtime/host_info`, `GET /runtime/capabilities`, `GET /runtime/health`
- `GET|PUT|PATCH /config`
- `POST /sessions/{id}/mcp/add` — stage live MCP server add (feature-gated)
- `POST /sessions/{id}/mcp/remove` — stage live MCP server remove (feature-gated)
- `POST /sessions/{id}/mcp/reload` — stage live MCP server reload (feature-gated)
- `POST /comms/send` (feature-gated)
- `GET /comms/peers` (feature-gated)
- `GET /mob/{id}/events` — SSE stream for mob events (feature-gated)
- `POST /mob/{id}/spawn-helper`, `POST /mob/{id}/fork-helper`
- `POST /mob/{id}/wait-kickoff`
- `POST /mob/{id}/wire-members-batch`
- `GET /mob/{id}/members/{agent_identity}/status`
- `POST /mob/{id}/members/{agent_identity}/cancel`
- `POST /mob/{id}/members/{agent_identity}/runs/{run_id}/stop`
- `GET|PUT /mob/{id}/members/{agent_identity}/tool-declaration`, `POST /mob/{id}/members/{agent_identity}/identity-adoption`, `POST /mob/{id}/members/{agent_identity}/tool-convergence/resolve` - durable member tool/identity declarations
- `POST /mob/{id}/members/{agent_identity}/respawn`
- `GET /mob/{id}/members/{agent_identity}/history`
- `GET /mob/{id}/hosts`
- `GET /mob/{id}/route-installs`
- `GET|POST /auth/profiles`
- `GET|DELETE /auth/bindings/{binding_id}`
- `POST /auth/bindings/{binding_id}/test`
- `POST /auth/login/start`, `POST /auth/login/complete`, `POST /auth/login/cancel`
- `POST /auth/login/device/start`, `POST /auth/login/device/complete`
- `GET /auth/bindings/{binding_id}/status`
- `POST /auth/bindings/{binding_id}/logout`
- `GET /realms`, `GET /realms/{id}`

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

`resolved_paths` is omitted unless `rkat-rest` (or `rkat-mcp`) runs with
`--expose-paths`; JSON-RPC `config/*` includes it when available.

CAS writes:

```json
{"config": {...}, "expected_generation": 4}
{"patch": {...}, "expected_generation": 4}
```

---

## JSON-RPC (`rkat-rpc`)

Start the server (stdio is default; `--tcp <addr>` exposes the same protocol over TCP; `--live-ws <addr>` enables the live-channel WebSocket listener):

```bash
rkat-rpc --realm team-alpha
rkat-rpc --realm team-alpha --tcp 127.0.0.1:9000
rkat-rpc --realm team-alpha --tcp 127.0.0.1:9000 --live-ws 127.0.0.1:9001
```

`--tcp` and `--live-ws` accept loopback binds by default; a non-loopback bind
requires `--allow-remote`, which only opts in to network exposure (it adds no
authentication or encryption).

`rkat-rpc --tcp` is only the JSON-RPC host transport. It is not the signed
Meerkat peer/comms channel used by remote agents or external mob members. For a
remote peer, start `rkat run` with `--comms-listen-tcp` and usually
`--comms-advertise-tcp` plus `--comms-binding-out`.

### Sessions, turns, history

- `initialize`
- `tools/register` - register client callback tools (`tools: [{name, description, input_schema, execution?}]`, result `{registered: N}`); registration is per connection and a session's callbacks go to the connection that created it. During a turn the server sends `tool/execute` requests (`tool_use_id`, `name`, `arguments`) that the client answers with `{content, is_error}` within 120 s (else the call fails `timeout`; a closed connection fails it `tool_unavailable`). `execution` is `{"mode": "fast"}` (default) or `{"mode": "detached", ...}`. Two or more registered callback tools are served through the deferred tool catalog.
- `session/create` (`initial_turn: "deferred"` returns a pending `session_id` without running the first turn), `session/list`, `session/read`, `session/history`, `session/archive`
- `session/fork_at`, `session/fork_replace`
- `session/rewrite_transcript`, `session/transcript_revision` (single read; `revision: "current"` reads the head), `session/transcript_revisions` (revision list), `session/restore_transcript_revision`
- `session/update_system_prompt` (replace one durable versioned system-prompt key), `session/activate_instruction` (append one immutable instruction revision at an idle boundary), `session/instruction_activations`
- `session/export_atif` - export the projected event log as an ATIF v1.7 trajectory (requires event replay)
- `session/external_event`, `session/peer_response_terminal`, `session/inject_context`
- `session/input_status` - read an input's stored runtime state by input id or idempotency key
- `session/stream_open` / `session/stream_close` — event streaming
- `events/latest_cursor`, `events/list_since`, `events/snapshot` — cursor-based event reads
- `turn/start` - accepts `model`, `provider`, `self_hosted_server_id`, `provider_params` and `auth_binding` for mid-session hot-swap (`provider_params`/`auth_binding`: a value or `{"action":"set","value":...}` sets, `{"action":"clear"}` removes); `max_tokens`, `output_schema` and `structured_output_retries` are parsed but refused with -32602 ("Cannot override ... on a runtime-routed turn") on every session, so set them on `session/create` (including a deferred create); the SDK `turn` arguments of those names are refused the same way; `system_prompt` appends one System message before the turn (it does not replace earlier ones; from the second turn on it needs a model that accepts mid-conversation System messages, so Gemini and older Anthropic models refuse it); optional `injected_context` (list of content inputs) delivers host-attached ambient context as separate typed messages before the user message (`session/create` accepts the same field); `transient_turn_context` is request-only, must be non-empty and is never written to the transcript; unknown fields are refused
- `turn/interrupt`
- `turn/stop_run` - stop one exact run (`session_id`, `run_id` from the `run_started` event, `reason`) and terminalize every input bound to it; receipt `outcome` is `stopped`, `not_current`, or `not_stoppable`
- `blob/get` — fetch generated image / artifact payload bytes by blob id
- `artifact/list`, `artifact/get`, `artifact/download`

Generated assistant images appear in `session/history` as assistant blocks with `block_type: "image"`; fetch bytes via `blob/get` using `data.blob_ref.blob_id`.

There is no direct `rkat image` command and no `rkat rpc` subcommand. Normal
runtime composites treat image-generation `Inherit` as visible when the image
machine, executor, planner, and blob store are wired. `--allow-tool
generate_image` can narrow that catalog but cannot create missing dependencies.
The minimal no-builtins/no-shell path requires explicit `Enable`, available
through a Mob profile or an in-process runtime-backed build. After a generated
result returns a blob id, fetch it with `rkat blob get <BLOB-ID>`.

### Live channels (caller-initiated)

- `live/open` — open a live audio/text channel with model-gated image input; optional positive `seed_max_chars` bounds serialized whole-turn seed messages
- `live/status` — get the status of a live channel
- `live/close` — close a live channel
- `live/send_input` — send an audio, text, or model-supported image chunk to a live channel
- `live/commit_input` — commit pending input on a live channel (turn boundary)
- `live/interrupt` — interrupt the assistant turn on a live channel (barge-in)
- `live/truncate` — truncate assistant output at a client-tracked playback cursor
- `live/playback_complete` - report completed client playback for one generated assistant output
- `live/refresh` — apply mutable session config (instructions/tools/audio) to an open live channel
- `live/webrtc/answer` - exchange a browser offer for a server SDP answer using the single-use token returned by a WebRTC `live/open`
- `live/media_health` - report decoded-audio counters for the output a `live/media_health_requested` notification named

`live/webrtc/answer` and `live/media_health` are served only by an
`rkat-rpc` built with `live-webrtc` and started with `--live-webrtc`, which
also sends the `live/assistant_output_available`,
`live/media_health_requested`, and `live/assistant_playback_hint`
notifications.

Set `model: "gpt-realtime-2"` on `session/create` and call `live/open` to open a live channel; capability is gated on `ModelCapabilities.realtime`. Check the returned `capabilities.image_in` boolean before calling `live/send_input` with `{ "kind": "image", "idempotency_key": "turn-42-diagram", "mime": "image/png", "data": "<base64>" }`. Image input is staged context for the next text, audio, or explicitly committed response. `status: "sent"` is queue acceptance only; wait for `user_content_committed` with the matching key for durable success. An exact same-key replay returns the prior receipt without resending provider input; different MIME/bytes reject with `image_input_idempotency_conflict`. Commit staged text/audio before submitting an image (`image_input_requires_commit`). Canonical live image history is capped at 40 MiB decoded in aggregate; new input that would cross it rejects before provider send as `image_input_history_budget_exceeded`. Legacy/out-of-band overflow, missing blobs, and content-address mismatches fail `live/open`; reconnect never trims accepted image context.

`live/open.seed_max_chars` is optional. Omission preserves the full canonical
seed. A positive value bounds serialized seed messages and requests a recent
whole-turn suffix; zero is rejected. The resolved root prompt is never dropped
and must fit, an existing compaction-summary head may be retained, and any
truncation reports degraded continuity. Runtime system context and full
canonical image identity, tombstone, and accounting sidecars stay outside the
message window.

The `live/*` methods require at least one configured live transport: use
`rkat-rpc --live-ws <addr>` for WebSocket, or build with the `live-webrtc`
feature and pass `rkat-rpc --live-webrtc` for WebRTC signaling. A WebRTC
client requests `transport: "webrtc"`, creates a local offer, calls the
returned `answer_method` (currently `live/webrtc/answer`) with the channel id,
single-use token, and `offer_sdp`, then applies the returned `answer_sdp`.

### Auth (realm/binding model)

- `auth/profile/list`, `auth/profile/get`, `auth/profile/create`, `auth/profile/delete`
- `auth/login/start`, `auth/login/complete`, `auth/login/cancel`
- `auth/login/device_start`, `auth/login/device_complete`
- `auth/login/provision_api_key`
- `auth/status/get`, `auth/logout`
- `realm/list`, `realm/get`

`auth/login/start` and `auth/login/complete` also accept an MCP server target
(`{"mcp": {...}}`) or a connector slot (`{"connector": {...}}`).
`auth/login/cancel` retires a pending MCP or connector attempt by its `state`,
or an MCP attempt by the `attempt_ref` that `auth/status/get` reports.
`auth/status/get` also reports MCP and connector targets, and `auth/logout`
also clears an MCP target's stored credential. Auth errors carry a typed
`error.data.reason` (`WireAuthErrorReason`, for example `slot_occupied`,
`attempt_missing`, `reauth_required`; 0.9.0 adds `credential_access_denied`,
`credential_interaction_required`, `credential_access_pending`, and
`credential_store_unavailable`). Branch on the reason, not the message.

CLI auth forms:

```bash
rkat auth realms
rkat auth profiles
rkat auth profile <PROFILE_ID>
rkat auth profile-delete <PROFILE_ID> [-y|--yes]
rkat auth bindings
rkat auth test <BINDING_ID>
rkat auth status <PROFILE_ID>
rkat auth login [PROVIDER] [--backend <BACKEND>] [--method <METHOD>]
rkat auth login <PROVIDER> --non-interactive [--secret <SECRET>] [--backend <BACKEND>] [--method <METHOD>]
rkat auth logout [<REALM>:]<BINDING_ID>
rkat auth refresh <PROFILE_ID>
rkat auth connector-client set <ID> --issuer <ISSUER> --client <CLIENT_ID> [--client-secret [--secret-stdin]] [--authorize-param NAME=VALUE...] [--no-resource-indicator] [--protocol <ID>] [--redirect-uri <URI>...]
rkat auth connector-client rotate-secret|remove <ID>
rkat auth connector-client list
```

`connector-client` manages the connector OAuth clients of `--realm` (default
`global`): writes go to that realm's own config, and a client secret is read
from stdin and kept in the credential store, never in config.

CLI `login` provisions the reserved `global` realm in the HOME-rooted doc
(`~/.rkat/config.toml`), inherited by every workspace realm via the chain tail.
Direct-provider OAuth writes `global:anthropic_oauth`, `global:openai_oauth`, or
`global:google_oauth`; non-interactive api-key login stores only a token
under `global:default_<provider>` (no backend/auth/binding config; add a
matching `managed_store` binding and select it with `--auth-binding`). With `copilot` compiled (default),
`rkat auth login copilot` runs GitHub device OAuth (`github_copilot_oauth`) and
provisions `global:copilot_openai`, `global:copilot_anthropic`, and
`global:copilot_gemini`, all sharing the `github_copilot` credential account.
These are provider-specific Copilot backends, not a fourth LLM provider family.
Legacy `dev`-realm logins migrate to `global` on the
run path (idempotent, no-clobber). Credential reads inherit down the chain; RPC, REST and `HostAuthService`
writes are strict-owner and must explicitly address the realm that defines the
binding (a child-addressed inherited write is rejected with
`binding_inherited`, not forwarded). CLI logout resolves the positional
`[<realm>:]<binding>` through the loaded config: a bare binding means
`global`, `--realm` does not retarget it, and logging out `<child>:<binding>`
for an inherited binding clears the owning realm's stored credential (possibly
an account shared by other routes), so address the true owner.

### Scheduling

- `schedule/create`, `schedule/get`, `schedule/list`, `schedule/update`
- `schedule/pause`, `schedule/resume`, `schedule/delete`
- `schedule/occurrences` — occurrences within the planning horizon
- `schedule/tools`, `schedule/call` — agent-facing schedule tool surface

### Durable jobs and monitors

Realm-scoped detached jobs (`shell(background: true)` runs as one; requires a
durable realm):

- `jobs/get`, `jobs/list`, `jobs/progress`, `jobs/result`, `jobs/artifacts`, `jobs/health` — read projections
- `jobs/cancel`, `jobs/retry` — machine-authorized mutations
- `jobs/subscribe`, `jobs/unsubscribe` — durable delivery subscriptions
- `monitors/start` — high-trust durable script monitor with explicit restart/output contracts
- `meta/jobs/{heartbeat,progress,checkpoint,complete,fail,cancel_ack}` — host-worker lease surface (exact attempt/fence authority)

Wire types live in `meerkat_contracts::wire::jobs`; the generated method
catalog in `docs/api/rpc.mdx` is authoritative.

### Skills, models, capabilities, runtime, approvals

- `help/ask`
- `skills/list` (no advertised `skills/inspect`; that method returns method-not-found)
- `models/catalog`
- `capabilities/get`
- `runtime/host_info`, `runtime/capabilities`, `runtime/health`
- `approval/request`, `approval/list`, `approval/get`, `approval/decide`
- `approval/consent_decide` (`approval_id` and `decision` only; the connection names the approver) - listed by `initialize` only on a stock governed stdio pipe that serves R3 human consent; that pipe also sends `approval/consent_notice` notifications (`kind`: `requested`, `approved`, `denied`, `used`, `withdrawn`; a `requested` notice carries `presentation`)

Approval records are audit data: creating or deciding one gates or executes
nothing by itself. REST, MCP, and CLI expose no approval methods. Store
failures return typed codes: `APPROVAL_STORE_UNAVAILABLE` (nothing committed),
`APPROVAL_CONFLICT` (another owner changed it first), and
`APPROVAL_COMMIT_UNKNOWN` (reconcile from the store before reuse).

### Config

- `config/get`, `config/set`, `config/patch` (same envelope + CAS as REST)

### MCP live mutation

- `mcp/add {session_id, server_config, persisted?}`, `mcp/remove {session_id, server_name, persisted?}`, `mcp/reload {session_id, server_name?}`; response `{session_id, operation, server_name, status: "staged", persisted}`. Changes apply at the session's next turn boundary; `persisted: true` also writes the host's project `mcp.toml`.
- These are JSON-RPC live session operations. CLI `rkat mcp add/remove/list/get` is the config surface.

### Mob (feature-gated)

Lifecycle and identity:

- `mob/create`, `mob/list`, `mob/status`, `mob/lifecycle`, `mob/destroy`, `mob/snapshot`
- `mob/members`, `mob/list_members_matching`, `mob/member_status`
- `mob/spawn`, `mob/spawn_many`, `mob/spawn_helper`, `mob/fork_helper`
- `mob/ensure_member`, `mob/reconcile`
- `mob/retire`, `mob/respawn`, `mob/force_cancel`, `mob/stop_member_run` (stop one exact member run)
- `mob/wire`, `mob/wire_members_batch`, `mob/unwire`, `mob/rotate_supervisor`
- `mob/claim_callback_route` (bind the mob's member callback route to this connection with the mob's claim token from `mob/create`)
- `mob/member_tool_declaration`, `mob/apply_member_tool_declaration` (revision-fenced), `mob/adopt_member_identity_declaration`, `mob/resolve_identity_convergence_block` - durable member tool/identity declarations

Member interaction and context:

- `mob/turn_start`, `mob/member_send`, `mob/ingress_interaction`
- `mob/append_system_context`

Work lanes (autonomous member work tracking):

- `mob/submit_work`, `mob/cancel_work`, `mob/cancel_all_work`
- `mob/conclude_objective` - explicitly conclude a durable kickoff objective

Waits and observation:

- `mob/wait_kickoff`, `mob/wait_ready`
- `mob/events`, `mob/stream_open`, `mob/stream_close`
- `mob/member_history`, `mob/hosts`, `mob/route_installs`

Multi-host operator control:

- `mob/grant_scopes`, `mob/revoke_scopes`, `mob/grants`
- `mob/bind_host`, `mob/revoke_host`, `mob/hard_cancel_member`
- `mob/member_live_open`, `mob/member_live_close`, `mob/member_live_status`, `mob/member_live_control`

The mob member-live family leaves the transport to the member's own live host,
for local and placed members alike: `transport: "webrtc"` returns a WebRTC
bootstrap answered with `live/webrtc/answer` on the same server, and a host
that serves no WebRTC refuses it (`LiveTransportUnsupported`; a member host
started by `rkat mob host` serves WebSocket only). `execution_identity`
(gpt-live-1) is served for a member local to the server. Session-scoped
`live/open` refuses a mob member's session, and the controller's generic
session surface cannot proxy a placed member's remote session.

Flows:

- `mob/flows`, `mob/run`, `mob/flow_run`, `mob/flow_status`, `mob/run_result`, `mob/flow_cancel`

Profiles (when a profile store is present):

- `mob/profile/create`, `mob/profile/get`, `mob/profile/list`, `mob/profile/update`, `mob/profile/delete`

### Comms (feature-gated)

- `comms/send`, `comms/peers`

`comms/peers` returns `PeerDirectoryEntry` objects with canonical `peer_id`,
display-only `name`, typed `address: { transport, endpoint }`, discovery
`source`, `sendable_kinds`, versioned `capabilities`, and supplementary
`meta`. Use `peer_id` as `comms/send.to`; never route by name or by rebuilding
an address string.

`comms/send` accepts an optional tri-state `content_taint` override (`{"declare": "clean"|"tainted"}` or `"undeclared"`; absent = inherit the runtime-level outbound declaration). The declaration rides inside the signed envelope; receivers read it from the typed transcript comms notice (`sender_taint`).
In 0.9.0, an ambiguous peer delivery (the envelope may
have been delivered) is `send_ambiguous` on REST (`retry_safe: false`,
`required_action: "reconcile"`) but plain `send_failed` with "peer delivery
outcome is ambiguous" in the message on JSON-RPC `comms/send`. Reconcile
before resending.

### Notifications

Server to client: `session/event` (`AgentEvent` payload during turns),
`session/stream_event` / `session/stream_end`, `mob/stream_event` /
`mob/stream_end`, plus the conditional `approval/consent_notice` and WebRTC
`live/*` notifications above. Client to server: `initialized` (acknowledged
silently) and `cancel` (`{"request_id": <id>}`), which ends a still-uncommitted
request with `REQUEST_CANCELLED` (-32005); committed success is never rewritten.

### Error codes

`meerkat_contracts::ErrorCode` JSON-RPC codes (besides the standard
-32700/-32600/-32601). Each code also has a fixed HTTP status; see the error
table in `references/sessions_runtime_surfaces_api_changes.md`, which also lists
the exit codes `rkat` returns (since 0.9.1 the first typed error decides, with its own number).

- Session and requests: `SESSION_NOT_FOUND` -32001, `SESSION_BUSY` -32002 (also a session served by another process, `data.kind = "session_served_elsewhere"`, and a teardown still finishing, `data.kind = "runtime_teardown_in_progress"`), `SESSION_NOT_RUNNING` -32003 (also every durable resume hold, `data.durable_resume_hold`), `DUPLICATE_INPUT` -32004, `REQUEST_CANCELLED` -32005, `SESSION_RUNTIME_UNAVAILABLE` -32032 (also `data.kind = "session_hosting_unavailable"`; on the JSON-RPC session runtime also `data.kind = "runtime_shutting_down"` for work that reaches admission after its shutdown began, and `data.kind = "host_runtime_stopped"` for an input to a session whose host runtime stopped and whose registration waits for its reload: nothing is kept, send it again after the next start)
- Agent: `PROVIDER_ERROR` -32010, `BUDGET_EXHAUSTED` -32011, `HOOK_DENIED` -32012, `AGENT_ERROR` -32013
- Capabilities, skills, schedules: `CAPABILITY_UNAVAILABLE` -32020, `SKILL_NOT_FOUND` -32021, `SKILL_RESOLUTION_FAILED` -32022, `SCHEDULE_NOT_FOUND` -32023
- Sessions: `FORK_INSTRUCTION_REFUSED` -32037 (new in 0.9.4: a `session/fork_at` or `session/fork_replace` refused before commit by the fork instruction policy; see `inherited_instructions`)
- Mob: `SUPERVISOR_ROTATION_INCOMPLETE` -32024, `SCOPE_DENIED` -32025, `HOST_UNAVAILABLE` -32026, `STALE_CURSOR` -32027, `STALE_FENCE` -32028 (also session write-fence conflicts), `MEMBER_RELOAD_REQUIRED` -32029, `STALE_DELIVERY_SCOPE` -32036 (new in 0.9.0: the member session moved under a current fence; re-capture the delivery scope)
- Native input: `INPUT_REFUSED` -32030, `INPUT_NOT_READY` -32031 (`data` is `{"type": "not_ready", "reason": {"kind": ...}}`; since 0.9.4 the kind `original_admission_pending` means an original the input names is still waiting on its source owner: retry later)
- Approvals (new in 0.9.0): `APPROVAL_STORE_UNAVAILABLE` -32033, `APPROVAL_CONFLICT` -32034, `APPROVAL_COMMIT_UNKNOWN` -32035
- Generic: `INVALID_PARAMS` -32602, `INTERNAL_ERROR` -32603

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
- `meerkat_help` — ask Meerkat usage help with the embedded platform skill
- `meerkat_read` — get session details
- `meerkat_history` — get committed transcript history
- `meerkat_sessions` — list sessions
- `meerkat_blob_get` — fetch raw blob bytes and metadata by blob id
- `meerkat_interrupt` — cancel in-flight turn
- `meerkat_archive` — archive (remove) a session
- `meerkat_config` — get/set/patch config
- `meerkat_capabilities` — list runtime capabilities
- `meerkat_models_catalog` — effective model catalog (built-in plus config-defined models), same data as `rkat models`
- `meerkat_skills` — list skills (`action: "list"`) or inspect one skill (`action: "inspect"`, typed `skill_key`, optional `source` UUID)
- `meerkat_mcp_add` - stage live MCP server add (refuses `oauth_account`, `oauth_account_selection`, and `account`, which are host configuration; refused on a governed runtime)
- `meerkat_mcp_remove` — stage live MCP server remove
- `meerkat_mcp_reload` — stage live MCP server reload
- `meerkat_event_stream_open` / `meerkat_event_stream_read` / `meerkat_event_stream_close` — session event streaming

Mob tools (feature-gated, public MCP names — distinct from the agent-internal `mob_*` tool surface):

- Lifecycle: `meerkat_mob_create`, `meerkat_mob_list`, `meerkat_mob_status`, `meerkat_mob_lifecycle`
- Membership: `meerkat_mob_spawn`, `meerkat_mob_spawn_many`, `meerkat_mob_retire`, `meerkat_mob_respawn`, `meerkat_mob_force_cancel`, `meerkat_mob_wait_kickoff`, `meerkat_mob_wait_ready`
- Wiring: `meerkat_mob_wire`, `meerkat_mob_wire_members_batch`, `meerkat_mob_unwire`
- Member interaction: `meerkat_mob_member_send`, `meerkat_mob_append_system_context`
- Runs and flows: `meerkat_mob_events`, `meerkat_mob_flows`, `meerkat_mob_run`, `meerkat_mob_flow_run`, `meerkat_mob_flow_status`, `meerkat_mob_run_result`, `meerkat_mob_flow_cancel`
- WorkGraph Flow bridge: `meerkat_workgraph_flow_launch`, `meerkat_workgraph_flow_reconcile`, `meerkat_workgraph_flow_binding_get`, `meerkat_workgraph_flow_binding_list`, `meerkat_workgraph_flow_abandon_uncertain`
- Multi-host observation only: `meerkat_mob_member_history`, `meerkat_mob_hosts`, `meerkat_mob_route_installs` (host/grant admin, hard cancel, and member live channels are deliberately not MCP tools)
- Profiles: `meerkat_mob_profile_create`, `meerkat_mob_profile_get`, `meerkat_mob_profile_list`, `meerkat_mob_profile_update`, `meerkat_mob_profile_delete`
- Event streams: `meerkat_mob_event_stream_open` / `meerkat_mob_event_stream_read` / `meerkat_mob_event_stream_close`

Schedule tools:

- `meerkat_schedule_create`, `meerkat_schedule_get`, `meerkat_schedule_list`, `meerkat_schedule_update`
- `meerkat_schedule_pause`, `meerkat_schedule_resume`, `meerkat_schedule_delete`
- `meerkat_schedule_occurrences`

WorkGraph tools (no `meerkat_` prefix): `workgraph_create`, `workgraph_get`,
`workgraph_list`, `workgraph_ready`, `workgraph_snapshot`, `workgraph_events`,
`workgraph_claim`, `workgraph_release`, `workgraph_update`, `workgraph_block`,
`workgraph_close`, `workgraph_link`, `workgraph_add_evidence`. The
attention-scoped `workgraph_attention_reassign` and `workgraph_policy_escalate`
are not on this unscoped surface.

Comms tools (feature-gated):

- `meerkat_comms_send` / `meerkat_comms_peers`

`meerkat_config` input:

```json
{"action":"get"}
{"action":"set","config":{...},"expected_generation":4}
{"action":"patch","patch":{...},"expected_generation":4}
```

Response includes the same config envelope fields (`resolved_paths` only with
`rkat-mcp --expose-paths`).

---

## Python SDK

Package: `meerkat-sdk` on PyPI (import `meerkat`); the client drives an
`rkat-rpc` subprocess.

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
  live_ws=False,              # optional: spawn with --live-ws 127.0.0.1:0
  live_webrtc=False,          # optional: spawn with --live-webrtc (live-webrtc builds)
)
```

Client methods:

- `create_session(prompt, *, model, auth_binding=None, ...)` → `Session` — `auth_binding` scopes credentials to a realm/binding
- `create_session_streaming(prompt, ...)` → `EventStream`
- `create_deferred_session(prompt, ...)` → `DeferredSession` (creates without running; `start_turn(...)` runs the first turn)
- `ask_help(question, *, prompt=None, ...)` → `RunResult` (`help/ask`)
- `@client.tool(name, description=..., input_schema=...)` decorator registers a callback tool (`tools/register`)
- `list_sessions(*, labels=None, limit=None, offset=None)` → `list[SessionSummary]`
- `read_session(session_id)` → `SessionDetails`
- `read_session_history(session_id, *, offset=0, limit=None)` → `SessionHistory`
- `export_session_atif(...)` / `input_state(...)` / `subscribe_session_events(session_id)`
- `get_blob(blob_id)` → `BlobPayload`
- `create_mob(*, definition)` → `Mob` — call `await client.create_mob(definition=definition)`
- `list_mobs()` → `list[dict[str, Any]]`
- `get_config()` / `set_config(...)` / `patch_config(...)`
- `mcp_add(session_id, server_config, *, persisted=False)`
- `mcp_remove(session_id, server_name, *, persisted=False)`
- `mcp_reload(session_id, *, server_name=None, persisted=False)`
- `list_skills()`
- `capabilities` (property, populated during `connect()`) / `has_capability(id)` / `require_capability(id)`
- `get_runtime_host_info()` / `get_runtime_host_capabilities()` / `get_runtime_host_health()`

Durable jobs, approvals, artifacts, event replay, WorkGraph:

- `jobs_get`, `jobs_list`, `jobs_cancel`, `jobs_progress`, `jobs_result`, `jobs_artifacts`, `jobs_retry`, `jobs_health()`, `jobs_subscribe`, `jobs_unsubscribe`, `monitors_start`
- `meta_job_heartbeat`, `meta_job_progress`, `meta_job_checkpoint`, `meta_job_complete`, `meta_job_fail`, `meta_job_cancel_ack` (host-worker `meta/jobs/*`; renamed from `mobkit_job_*` in 0.9.0)
- `request_approval`, `list_approvals`, `get_approval`, `decide_approval`; `on_consent_notice(callback)` delivers typed `approval/consent_notice` and returns an unsubscribe function
- `list_artifacts`, `get_artifact`, `download_artifact`; `latest_event_cursor`, `list_events_since`, `event_snapshot`
- `get_workgraph_item`, `list_workgraph_items`, `list_ready_workgraph_items`, `get_workgraph_snapshot`, `list_workgraph_events`, `get_workgraph_goal_status`, `list_workgraph_attention`

Live channel helpers:

- `live_open(session_id, turning_mode=None, transport=None, seed_max_chars=None)` → `dict[str, Any]` carrying the `LiveOpenResult` wire shape
- `live_status(channel_id)` / `live_close(channel_id)`
- `live_send_input_text(channel_id, text)`
- `live_send_input_audio(channel_id, data_base64, sample_rate_hz, channels)`
- `live_send_input_image(channel_id, idempotency_key, mime, data_base64)`
- `live_send_input_video_frame(channel_id, codec, data_base64, timestamp_ms)`
- `live_webrtc_answer(channel_id, token, offer_sdp)`
- `live_commit_input(channel_id, response_modality=None)` / `live_interrupt(channel_id)`
- `live_truncate(channel_id, item_id, content_index, audio_played_ms)`
- `live_refresh(channel_id)`
- `live_playback_complete(channel_id, output_id)`
- `live_media_health(channel_id, output_id, decoded_frames, audible_frames, max_rms)` (live-webrtc builds)
- `on_live_notification(callback)` → unsubscribe function (`live/*` server notifications)
- `LiveChannel.session(client, session_id, *, turning_mode=None, transport=None, seed_max_chars=None)` - session-bound wrapper (`open()`, `send_input_*`, `commit_input`, `playback_complete`, `close()`)

Call `opened = await client.live_open(session_id, ...)`, then keep
`channel_id = opened["channel_id"]`. All subsequent helpers above address that
channel, not the session ID. Both audio format arguments are required.

Auth helpers:

- `auth_login_start(...)` / `auth_login_complete(...)`
- `auth_login_device_start(...)` / `auth_login_device_complete(...)`
- `auth_provision_api_key(...)`
- `auth_status(...)` / `auth_logout(...)`
- `list_auth_profiles(realm_id)` / `get_auth_profile(...)` / `create_auth_profile(...)` / `delete_auth_profile(...)`
- `auth_mcp_login_start(...)` / `auth_mcp_login_complete(...)` / `auth_mcp_login_cancel(...)` / `auth_mcp_login_cancel_by_attempt_ref(...)` / `auth_mcp_status(...)` / `auth_mcp_logout(...)`
- `auth_connector_login_start(...)` / `auth_connector_login_complete(...)` / `auth_connector_login_cancel(...)` / `auth_connector_status(realm_id, slot_id)`
- `list_realms()` / `get_realm(realm_id)`

Schedule helpers:

- `create_schedule(request)` / `get_schedule(id)` / `list_schedules(...)` / `update_schedule(request)`
- `pause_schedule(id)` / `resume_schedule(id)` / `delete_schedule(id)`
- `list_schedule_occurrences(id, *, include_terminal=None)`
- `list_schedule_tools()` / `call_schedule_tool(request)`

Session methods:

- `Session.turn(prompt, ...)` → `RunResult`
- `Session.stream(prompt, ...)` → `EventStream`
- `Session.history(*, offset=0, limit=None)` → `SessionHistory`
- `Session.invoke_skill(skill_ref, prompt)` → `RunResult`
- `Session.interrupt()`
- `Session.stop_run(run_id, *, reason)` → `StopRunResult` (`turn/stop_run`; `run_id` from the `run_started` event)
- `Session.archive()`
- `Session.inject_context(text, *, source=None, idempotency_key=None)` / `send_external_event(...)` / `send_peer_response_terminal(...)`
- `Session.transcript_revision(...)` / `transcript_revisions(...)` / `rewrite_transcript(...)` / `restore_transcript_revision(...)` / `fork_at(...)` / `fork_replace(...)`
- `Session.update_system_prompt(key, content, ...)` / `activate_instruction(activation)` / `instruction_activations(...)`
- `Session.send(**command)` / `Session.peers()`. Since 0.9.1 the result validator accepts the `delivery` outcome on every peer send kind (`acked`, `volatile_handed_off`, `queued`, or `{"durably_resolved": {"outcome": "accepted" | "deduplicated" | "rejected"}}`). A 0.9.0 SDK raised `INVALID_RESPONSE` for `peer_request`, `peer_response` and `peer_lifecycle` sends, and for a `peer_message` whose `delivery` was `volatile_handed_off` or `durably_resolved`, after the send had already happened: do not retry such a send.
- `Session.subscribe_events()` → `EventSubscription`
- `DeferredSession.start_turn(prompt, ...)` → `RunResult` (first turn of `create_deferred_session`)

Mob methods:

- `Mob.id` (attribute) / `Mob.status()` / `Mob.lifecycle(action)`
- `Mob.spawn(...)` / `Mob.spawn_many(specs)` / `Mob.retire(agent_identity)` / `Mob.respawn(agent_identity)` / `Mob.force_cancel(agent_identity)` / `Mob.stop_member_run(agent_identity, run_id, *, reason)`
- `Mob.wire(a, b)` / `Mob.wire_members_batch(edges)` / `Mob.unwire(a, b)`
- `Mob.members()` / `Mob.member_status(agent_identity)` / `Mob.member(agent_identity).send(content, handling_mode=...)` / `Mob.append_system_context(agent_identity, text, ...)`
- `Mob.spawn_helper(prompt, *, result_label, max_text_bytes, ...)` / `Mob.fork_helper(source_member_id, prompt, *, result_label, max_text_bytes, ...)` → `MobHelperResult`
- `Mob.wait_for_kickoff_complete(*, member_ids=None, timeout_ms=None)` / `Mob.wait_for_ready(...)` / `Mob.read_events(*, after_cursor=0, limit=100)`
- `Mob.flows()` / `Mob.run(params=None, *, prompt=None, flow_id=None)` / `Mob.run_flow(flow_id, params)` / `Mob.flow_status(run_id)` / `Mob.run_result(run_id)` / `Mob.cancel_flow(run_id)`
- Multi-host: `Mob.grant_scopes(...)` / `revoke_scopes(...)` / `grants()` / `bind_host(...)` / `revoke_host(host_id)` / `hosts()` / `route_installs()` / `member_history(...)` / `hard_cancel(agent_identity, reason)` / `member_live_open|close|status|control(...)`
- Declarations: `Mob.member_tool_declaration(...)` / `apply_member_tool_declaration(...)` / `adopt_member_identity_declaration(...)` / `resolve_identity_convergence_block(...)`
- `Mob.subscribe_member_events(agent_identity)` → `EventSubscription`
- `Mob.subscribe_events()` → `EventSubscription`

`get_config()` / `patch_config()` return the config envelope.

Type/parsing notes:

- `SessionSummary` and `SessionDetails` inherit `SessionInfo` and its shared
  fields; they are dataclasses, not dictionaries. Mob listing currently
  returns dictionaries.
- Mob creation accepts only the definition, not `auth_binding`. A mob role
  `Profile` does not declare an initial auth binding either. Configure
  host/realm credentials, or select a preconfigured realm binding through
  supported per-member spawn/helper auth options.
- capability status may arrive as externally-tagged enum maps (e.g. `{"DisabledByPolicy": {...}}`) and is normalized to the tag string.
- event-envelope parsing requires canonical `event_id`, typed `source`, `seq`, `timestamp_ms`, and object `payload` facts; missing or malformed facts fail with `INVALID_RESPONSE` rather than being defaulted.
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
  stateRoot: "/path",         // optional
  contextRoot: "/path",       // optional
  userConfigRoot: "/path",    // optional
  liveWs: false,              // optional: spawn with --live-ws 127.0.0.1:0
  liveWebrtc: false,          // optional: spawn with --live-webrtc (live-webrtc builds)
  liveToolTimeoutMs: 60000,   // optional: --live-tool-timeout-ms
});
```

For a fresh isolated realm, use `await client.connect({ isolated: true })`
instead. `realmId` and `isolated: true` are mutually exclusive.

Client methods:

- `createSession(prompt, options?)` → `Session` — `options.authBinding` scopes credentials to a realm/binding
- `createSessionStreaming(prompt, options?)` → `EventStream`
- `createDeferredSession(prompt, options?)` → `DeferredSession` (`startTurn(...)` or `stream(...)` runs the first turn)
- `askHelp(question, options?)` → `RunResult` (`help/ask`)
- `registerTool(name, description, inputSchema, handler)` registers a callback tool (`tools/register`)
- `listSessions(options?)` → `SessionInfo[]` (`labels`, `limit`, `offset`)
- `readSession(sessionId)` → `SessionInfo`
- `readSessionHistory(sessionId, { offset, limit }?)` → `SessionHistory`
- `exportSessionAtif(sessionId, options?)` / `inputState(sessionId, { inputId } | { idempotencyKey })` / `subscribeSessionEvents(sessionId)`
- `getBlob(blobId)` → `BlobPayload`
- `createMob({ definition })` → `Mob` — call `await client.createMob({ definition })`; neither mob creation nor a mob role `Profile` declares an initial auth binding. Configure host/realm credentials, or select a preconfigured realm binding through supported per-member spawn/helper auth options.
- `listMobs()` → `MobSummary[]`
- `getConfig()` / `setConfig(...)` / `patchConfig(...)`
- `mcpAdd(params)` / `mcpRemove(params)` / `mcpReload(params)`
- `listSkills()`
- `capabilities` (property, populated during `connect()`) / `hasCapability(id)` / `requireCapability(id)`
- `getRuntimeHostInfo()` / `getRuntimeHostCapabilities()` / `getRuntimeHostHealth()`

Durable jobs, approvals, artifacts, event replay, WorkGraph:

- `jobsGet`, `jobsList`, `jobsCancel`, `jobsProgress`, `jobsResult`, `jobsArtifacts`, `jobsRetry`, `jobsHealth()`, `jobsSubscribe`, `jobsUnsubscribe`, `monitorsStart`
- `metaJobHeartbeat`, `metaJobProgress`, `metaJobCheckpoint`, `metaJobComplete`, `metaJobFail`, `metaJobCancelAck` (host-worker `meta/jobs/*`; renamed from `mobkitJob*` in 0.9.0)
- `requestApproval`, `listApprovals`, `getApproval`, `decideApproval`; `onConsentNotice(listener)` delivers typed `approval/consent_notice` (`ConsentNotice`) and returns an unsubscribe function
- `listArtifacts`, `getArtifact`, `downloadArtifact`; `latestEventCursor`, `listEventsSince`, `eventSnapshot`
- `getWorkGraphItem`, `listWorkGraphItems`, `listReadyWorkGraphItems`, `getWorkGraphSnapshot`, `listWorkGraphEvents`, `getWorkGraphGoalStatus`, `listWorkGraphAttention`

Live channel helpers:

- `liveOpen({ session_id, turning_mode?, transport?, seed_max_chars? })` / `liveStatus(params)` / `liveClose(params)`
- `LiveChannel.session(client, sessionId, { seedMaxChars? })` — session-bound convenience wrapper
- `liveSendInput(params)` — raw params-object API
- `liveSendInputImage(channelId, idempotencyKey, mime, dataBase64)`
- `liveSendInputVideoFrame(channelId, codec, dataBase64, timestampMs)`
- `liveCommitInput(params)` / `liveInterrupt(params)` / `liveTruncate(params)`
- `liveRefresh(params)`
- `livePlaybackComplete(params)`
- `liveWebrtcAnswer(params)` / `liveMediaHealth(params)` (live-webrtc builds)
- `onLiveNotification(listener)` → unsubscribe function (`live/*` server notifications)

Auth helpers:

- `authLoginStart(...)` / `authLoginComplete(...)` / `authLoginCancel(...)`
- `authLoginDeviceStart(...)` / `authLoginDeviceComplete(...)`
- `authLoginProvisionApiKey(...)`
- `authStatusGet(...)` / `authLogout(...)`
- `authProfileList(realmId)` / `authProfileGet(...)` / `authProfileCreate(...)` / `authProfileDelete(...)`
- `authMcpStatus(...)` / `authMcpLogout(...)` / `authConnectorStatus(...)`
- `realmList()` / `realmGet(realmId)`

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
- `Session.stopRun(runId, reason)` → `StopRunResult` (`turn/stop_run`)
- `Session.archive()`
- `Session.injectContext(...)` / `sendExternalEvent(...)` / `sendPeerResponseTerminal(...)`
- `Session.transcriptRevision(...)` / `transcriptRevisions(...)` / `rewriteTranscript(...)` / `restoreTranscriptRevision(...)` / `forkAt(...)` / `forkReplace(...)`
- `Session.updateSystemPrompt(...)` / `activateInstruction(...)` / `instructionActivations(...)`
- `Session.send(command)` / `Session.peers()`. Since 0.9.1 the result validator accepts the `delivery` outcome on every peer send kind (`acked`, `volatile_handed_off`, `queued`, or `{"durably_resolved": {"outcome": "accepted" | "deduplicated" | "rejected"}}`). A 0.9.0 SDK raised `INVALID_RESPONSE` for `peer_request`, `peer_response` and `peer_lifecycle` sends, and for a `peer_message` whose `delivery` was `volatile_handed_off` or `durably_resolved`, after the send had already happened: do not retry such a send.
- `Session.subscribeEvents()` → `EventSubscription<AgentEventEnvelope>`
- `DeferredSession.startTurn(prompt, options?)` / `DeferredSession.stream(prompt, options?)` (first turn of `createDeferredSession`)

Mob methods:

- `Mob.status()` / `Mob.lifecycle(action)`
- `Mob.spawn(spec)` / `Mob.spawnMany(specs)` / `Mob.retire(agentIdentity)` / `Mob.respawn(agentIdentity)` / `Mob.forceCancel(agentIdentity)` / `Mob.stopMemberRun(agentIdentity, runId, reason)`
- `Mob.wire(a, b)` / `Mob.unwire(a, b)` — identity-keyed
- `Mob.listMembers()` / `Mob.member(agentIdentity)` → `Member`
- `Member.send(content, options?)` — call `mob.member(agentIdentity).send(content, options?)`
- `Mob.listFlows()` / `Mob.run(params, options)` / `Mob.runFlow(flowId, params)` / `Mob.flowStatus(runId)` / `Mob.runResult(runId)` / `Mob.cancelFlow(runId)`
- `Mob.subscribeMemberEvents(agentIdentity)` → `EventSubscription<AgentEventEnvelope>`
- `Mob.subscribeEvents()` → `EventSubscription<AttributedMobEvent>`
- `Mob.wireMembersBatch(edges)` / `Mob.memberStatus(agentIdentity)` / `Mob.appendSystemContext(...)` / `Mob.readEvents(options?)`
- `Mob.spawnHelper(prompt, { resultLabel, maxTextBytes, ... })` / `Mob.forkHelper(sourceMemberId, prompt, { resultLabel, maxTextBytes, ... })` → `MobHelperResult`
- `Mob.waitForKickoffComplete(options?)` / `Mob.waitForReady(options?)`
- Multi-host: `Mob.grantScopes(...)` / `revokeScopes(...)` / `grants()` / `bindHost(...)` / `revokeHost(hostId)` / `hosts()` / `routeInstalls()` / `memberHistory(...)` / `hardCancel(agentIdentity, reason)` / `memberLiveOpen|Close|Status|Control(...)`
- Declarations (client-level): `mobMemberToolDeclaration(...)` / `applyMobMemberToolDeclaration(...)` / `adoptMobMemberIdentityDeclaration(...)` / `resolveMobIdentityConvergenceBlock(...)`

`getConfig()` / `patchConfig()` return the config envelope.

Type/parsing notes:

- capability status may arrive as externally-tagged enum maps (e.g. `{ DisabledByPolicy: {...} }`) and is normalized to the tag string.
- event-envelope parsing requires canonical `event_id`, typed `source`, `seq`, `timestamp_ms`, and object `payload` facts; missing or malformed facts fail with `INVALID_RESPONSE` rather than being defaulted.
- `RunResult.skillDiagnostics` is typed as `SkillRuntimeDiagnostics`.
- generated image blocks preserve `imageId`, `blobId`, `mediaType`, `width`, `height`, `revisedPrompt`, and provider `meta`.
- image-generation wire contracts are exported as `WireGenerateImageRequest`, `WireGenerateImageExecutionPlan`, `WireImageGenerationToolResult`, `WireImageOperationPhase`, and `WireAssistantImageRef`.

---

## Rust SDK

**AgentFactory vs AgentBuilder**: use `AgentFactory::build_agent()` for supported
construction, including standalone Rust embedding. Customize
`AgentBuildConfig`'s client, dispatcher, store, hook, and skill overrides instead
of bypassing the factory. `AgentBuilder` remains a public low-level
configuration type, but production finalization crosses the private
facade-authorized bridge; its standalone finalizers are core-test-only, not a
downstream construction escape hatch.

Named-realm configuration with standalone factory construction:

```rust
use std::sync::Arc;
use meerkat::{AgentFactory, AgentBuildConfig};
use meerkat_core::{Config, EffectiveConfigReader, connection::RealmId};
use meerkat_store::{FilesystemRealmConfigSource, realm_paths_in};

let realms_root = std::env::current_dir()?.join(".rkat").join("realms");
let realm_id = RealmId::parse("team-alpha")?;
let realm = realm_paths_in(&realms_root, realm_id.as_str());
let global_doc = Config::global_config_path().ok_or("home config path unavailable")?;
let reader = EffectiveConfigReader::new(Arc::new(FilesystemRealmConfigSource::new(
    realms_root,
    global_doc,
    meerkat_models::canonical(),
)));
let mut config = reader.effective_config(&realm_id).await?;
config.validate(meerkat_models::canonical())?;
let factory = AgentFactory::new(realm.root.clone())
    .runtime_root(realm.root)
    .builtins(true)
    .shell(true);

let mut build = AgentBuildConfig::new("claude-sonnet-4-6");
build.realm_id = Some(realm_id);
let mut agent = factory.build_agent(build, &config).await?;
```

This composes a config snapshot for the named realm, but deliberately retains
the default `StandaloneEphemeral` build mode; paths alone do not make it a
runtime-backed service. The main skill's persistent-service quickstart uses
the same explicit root for config and storage. A manually supplied raw
`Config` is also valid for embedding, but `Config::load()` and opening named
persistence do not themselves compose that realm's parent chain.

Mob tools need both a `MobToolsFactory` (for example
`AgentMobToolSurfaceFactory`) and explicit per-build
`ToolCategoryOverride::Enable`, resolved through generated create-only
operator authority. The factory's `.mob(true)` ambient default supplies
neither prerequisite by itself and grants no scope over existing mobs.
See the complete `with_mob_tools` handoff in `mobs.md`.

`AgentBuildConfig`/session metadata carry `realm_id`, `instance_id`, `backend`, and `config_generation`.

`AgentBuildConfig` also carries:
- `silent_comms_intents: Vec<String>` — intents injected silently (no LLM turn)
- `preload_skills: Option<Vec<SkillKey>>` — skills to inject at session creation
- `runtime_build_mode: RuntimeBuildMode` — required, determines ops lifecycle ownership

### Runtime build mode

All runtime-backed surfaces (CLI, RPC, REST, MCP, and Web/WASM) use
`SessionOwned` bindings. Explicit standalone Rust embeddings and tests may use
`StandaloneEphemeral`. Browser direct sessions and mob members share the
cached `MeerkatMachine` adapter with in-memory storage; persistence is a profile
capability, not a choice of session authority.

```rust
use meerkat::{RuntimeBuildMode, SessionRuntimeBindings};
use meerkat_runtime::MeerkatMachine;
use meerkat_core::service::{CreateSessionRequest, SessionBuildOptions};

// Runtime-backed surface: prepare bindings from the adapter
let adapter = MeerkatMachine::persistent(store, blob_store)?;
let bindings = adapter.prepare_bindings(session_id.clone()).await?;
let build = SessionBuildOptions {
    runtime_build_mode: RuntimeBuildMode::SessionOwned(bindings),
    ..Default::default()
};

// Standalone Rust embedding or test: explicit choice (also the Default)
let build = SessionBuildOptions {
    runtime_build_mode: RuntimeBuildMode::StandaloneEphemeral,
    ..Default::default()
};
```

`prepare_bindings()` returns the canonical epoch-owned `SessionRuntimeBindings`;
the factory validates `bindings.session_id == session.id()` on `SessionOwned`
builds. For browser direct creation, use the shared
`meerkat::surface::materialize_ephemeral_runtime_session` path. Its exact
prepared-materialization transaction supplies the bindings, attaches the actor,
and retains cancellation cleanup authority until creation completes. Do not
replace that transaction with surface-local registration or cleanup.

Skill introspection (requires facade feature `skills`; standalone, no session required):

This example assumes the stock native factory's embedded sources, including
the `task-workflow` skill. With custom sources or identity configuration, use
a canonical key that the configured runtime can actually load.

```rust
use meerkat_core::skills::{SkillFilter, SkillKey, SkillName, SourceUuid};

if let Some(runtime) = factory.build_skill_runtime(&config).await? {
    let entries = runtime.list_all_with_provenance(&SkillFilter::default()).await?;
    let key = SkillKey::new(
        SourceUuid::builtin(),
        SkillName::parse("task-workflow")?,
    );
    let doc = runtime.load_from_source(&key, None).await?;
}
```

`None` loads the resolved canonical key. In the factory's composite source,
an explicit `Some(...)` selector is a registered source UUID as text, not a
human-readable repository name. That attached source must be able to load
the full canonical key; selecting an unrelated source does not rewrite it.

---

## Comms, hooks, skills, multi-agent

- Inproc comms is namespace-scoped; realm namespace isolates peer discovery/sends.
- **Peer lifecycle typing**: mob lifecycle routing is typed at ingress. `mob.peer_added`, `mob.peer_retired`, and `mob.peer_unwired` are silent lifecycle notices; the six `mob.kickoff_*` kinds (pending, starting, started, callback_pending, failed, cancelled) are visible lifecycle notices that open no inbound request and need no reply. Do not depend on mob defaults in `silent_comms_intents` for canonical behavior.
- **Comms choice**: agents use `send_message` for ordinary collaboration, `send_request` for structured ask/reply, and `send_response` for replies; `reply_to_peer` answers the peer whose message was delivered in the turn without restating addressing (it fails with `no_reply_capability` otherwise). Public peer reservation streams were removed.
- Hook entries come from the effective realm config, then the project and user `.rkat/config.toml` files (the first entry per id wins). Skills resolve from the embedded built-ins, the realm's `[[skills.repositories]]` and `<context-root>/.rkat/skills`; the runtime root only holds the Git skill cache (and is the base for relative filesystem repository paths when there is no context root). Workspace-default CLI realms preserve project ergonomics.
- **Skill introspection**: `SkillRuntime::list_all_with_provenance()` returns active + shadowed skills. `load_from_source(key, None)` loads the canonical key; the factory composite accepts registered source UUID text for explicit source selection.
- Multi-agent orchestration uses mobs exclusively. `MemberLaunchMode::Fork`
  (including prompt-context `fork_helper`) creates a fresh member/session
  seeded with rendered source-history context, not an O(1) copy-on-write
  transcript clone. Low-level `Session::fork()` / `fork_at()` are separate
  structural primitives (every fork starts with zero usage); the agent tool
  `fork_off` persists a durable child from an exact committed prefix, resumes
  it, and runs its task detached; the forker owns the child and its own
  forks. `spawn_helper()` /
  `fork_helper()` require `result_label` and `max_text_bytes` and return a
  `BoundedHelperRunOutcome` carrying the certified bounded result. A helper is
  retired after its turn, and also when its caller stops waiting first (a
  cancelled turn or an abandoned RPC/REST request).

---

## Flow spec essentials (mob definition)

Flow declarations live under `[flows.<flow_id>]`.

### Flat step authoring

Step declarations live under `[flows.<flow_id>.steps.<step_id>]`. Flat
authoring normalizes once at decode/construction to the canonical root frame;
all execution uses `FlowFrameEngine`.

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
- `output_format = "json"|"text"` (optional; omitted resolves to `json` when `expected_schema_ref` is set, else `text`)
- `failure_policy = "escalate"|"continue"` (default `escalate`; `continue` records the failure without deciding the frame's outcome; frame nodes accept it too)
- `allowed_tools` / `blocked_tools` (optional per-step tool overlay)

### Explicit frame root

An explicit `[flows.<flow_id>].root` takes precedence over the root that would
otherwise be generated from flat step dependencies. It does not select a
different execution engine. Named `steps` still own the role/message bodies.

Frame root declaration: `[flows.<flow_id>.root]` with `nodes` map of `FlowNodeSpec` entries.

Node types:

- `kind = "step"` — references a named `step_id`; supply `depends_on` and
  `depends_on_mode = "all"|"any"` (optional `branch`).
- `kind = "repeat_until"` — supply `loop_id`, `depends_on`, `depends_on_mode`,
  `body` (nested `FrameSpec`), `max_iterations`, and a typed `until`, e.g.
  `{ op = "eq", path = "steps.run_tests.all_pass", value = true }`.

Minimal complete flow using explicit frame authoring:

```toml
[flows.review.steps.check]
role = "reviewer"
message = "Review the change."

[flows.review.root.nodes.check]
kind = "step"
step_id = "check"
depends_on = []
depends_on_mode = "all"
```

For the loop example, define `run_tests` with `output_format = "json"` and
request `{"all_pass": boolean}` output; see the complete
[release-flow example](mobs.md#v2-flows-frame-based-execution-with-loops).

### Topology contract

- `[topology] mode = "advisory"|"strict"` — write the mode explicitly when
  supplying a topology table; the Rust enum's `Advisory` default does not make
  that required serialized field optional.
- `rules = [{ from_role = "...", to_role = "...", allowed = true|false }]`
- wildcard `"*"` role matching is supported.

### Agent-facing delegation tools

With generated authority, `AgentMobToolSurface` exposes thirteen base
agent-internal tool definitions; visibility does not bypass per-call scope or
objective/durable-fork prerequisites:

| Tool | Purpose |
|------|---------|
| `delegate` | Quick helper spawn (implicit mob, auto-wire) |
| `conclude_objective` | Supply only `outcome` for this turn's pre-addressed kickoff objective |
| `mob_create` | Create a mob from a definition |
| `mob_destroy` | Destroy a mob and archive all members |
| `mob_spawn_member` | Spawn a member into an authorized mob |
| `fork_off` | Take `member_id` + `task`, fork an exact committed transcript prefix through the durable resume path into a child the caller owns, and run the task; returns `status: "running"` + `job_id`; the outcome is recorded once in the forker's transcript as a durable `BackgroundJob` system notice (`persisted: true`) that joins a running forker's turn at its next model call or wakes an idle forker (blocks on one-shot hosts such as `rkat run` without `--keep-alive`); optional `expected_output` is guidance, not a schema; optional `max_run_secs` is an opt-in autokill; unknown arguments are rejected |
| `council` | Take a `topic` and existing `{mob_id, member_id, role}` participants; fork them into a bounded temporary discussion mob and clean up; the sealed outcome is recorded once in the convener's transcript as a durable `BackgroundJob` system notice (blocks on one-shot hosts) |
| `mob_retire_member` | Archive a member and its session; manage scope, or the caller owns the member (retiring a fork child also retires its own forks) |
| `mob_check_member` | Check a member's execution status and output; manage scope, or the caller owns the member |
| `mob_list_members` | List members of a mob; without manage scope, only the caller's own descendants |
| `mob_list` | List all mobs |
| `mob_wire` | Wire a local mob member to a local or typed external peer |
| `mob_unwire` | Remove a mob wiring relationship |

These tools are composed via `MobToolsFactory` late-binding. Operator
capabilities are runtime-injected. A profile store adds `mob_profile_create`,
`mob_profile_get`, `mob_profile_list`, `mob_profile_update`, and
`mob_profile_delete`; `mob_profile_list_sources` additionally requires a parent
tool snapshot provider. See `mobs.md` for bounds, cleanup, and authority details.
