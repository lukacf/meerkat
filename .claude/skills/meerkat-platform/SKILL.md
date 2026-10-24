---
name: meerkat-platform
description: "Build on the Meerkat agent platform (0.9 line). Covers every shipping surface (CLI, REST, JSON-RPC stdio/TCP, MCP, Python/TypeScript/Web/Rust SDKs), realm-scoped sessions, streaming, skills, hooks, memory, multimodal content, MCP servers, mob orchestration (spawn/fork/helpers/flows/profiles), durable scheduling, governed realms and approvals, and provider auth via `auth_binding` + AuthMachine (env keys, OAuth, cloud IAM). Use when integrating with Meerkat, picking a surface, wiring auth, building agents, scheduling automated runs, deploying mobpacks, upgrading to 0.9, or asking how a feature exposes through a particular SDK."
---

# Meerkat Platform Guide

Meerkat (`rkat`) is a library-first agent runtime exposed through multiple surfaces. One execution pipeline, realm-scoped state, runtime-backed semantics by default. This guide is task-oriented; deeper schemas live under `references/`.

## Where to find details

This guide is an overview. The reference documents below carry the detail; read the matching one before answering a detailed question. On a help surface (`rkat help`, JSON-RPC `help/ask`, REST `POST /help`, MCP `meerkat_help`), read a reference with the `skill_read_resource` tool, passing this skill's `source_uuid` and `skill_name` and the reference path as `path`; `skill_list_resources` lists them.

- `references/api_reference.md` - per-surface methods, schemas, examples (CLI, REST, RPC, MCP, Python/TS/Rust SDKs).
- `references/sessions_runtime_surfaces_sessions.md` - sessions on every surface: create, resume, list, read, history, archive, turn queueing and the attempt cap, interrupt versus `turn/stop_run`, forks, transcript edits, system prompts, events and replay, ATIF export, durable runs (0.9.5 per-round commits, same-run resume, tool reruns), durable resume holds and tool-process recovery.
- `references/sessions_runtime_surfaces_storage_config.md` - realm ids and where state lives, backends, host stores (0.9.5), multiprocess hosting, SQLite, `rkat storage`, runtime health and shutdown, WorkGraph limits, schedules, `rkat config` and config keys.
- `references/sessions_runtime_surfaces_api_changes.md` - JSON-RPC methods and REST routes added or renamed since 0.8.40, the `ErrorCode` table with HTTP statuses and `rkat` exit codes, SDK changes, binaries and installs, Rust embedding, removals.
- `references/providers_and_auth_models.md` - choosing a provider and model, default models and the catalog, capabilities, thinking and effort, `provider_params` and prompt caching, realtime, voice and image models, self-hosted servers, provider errors, refusals and content filters.
- `references/providers_and_auth_credentials.md` - env-var API keys, realm backends, auth profiles and bindings, base URLs and cloud routes, credential storage and keychain refusals, every `rkat auth` subcommand, `--auth-binding`, auth RPC/REST/SDK methods and errors, connector OAuth clients.
- `references/security.md` - OS sandboxing (process confinement) and what is not a sandbox, shell command policy, tool access policy and read-only, where credentials and secrets live, MCP security boundaries, audit, security errors and refusals.
- `references/security_governance.md` - governed realms (`rkat realm govern`, grants, `--as-agent`, governed `rkat-rpc`), review tiers R1/R2/R3, human consent and approvals, native authorization for Rust hosts, authorization changes while work runs.
- `references/mcp_tools_skills_mcp.md` - MCP servers: `mcp.toml` and realm config, every `rkat mcp` flag, which hosts start configured servers, MCP OAuth, access ceilings, live `mcp/*`, deferred tools, MCP Apps, MCP errors.
- `references/mcp_tools_skills_tools.md` - built-in tool families and exact names, `--tools` presets, `[shell]`/`[tools]` config, tool errors, filters and overlays, `tool_access_policy`, streaming tool deadlines, callback tools.
- `references/mcp_tools_skills_skills.md` - skills: SKILL.md format, identity and sources, `[skills]` config, `rkat skill`, `rkat run --skill`, `preload_skills` and `skill_refs`, mob skills, introspection, skill errors.
- `references/hooks_models_hooks.md` - hook points, `[[hooks.entries]]`, command/http runtimes, `hooks_override`, 0.9 mandatory model-request hooks, hook events.
- `references/hooks_models_model_operations.md` - `max_tokens`, budgets and turn limits, compaction, structured output, retries and timeouts, model fallback and model switching (prompt caching: `references/providers_and_auth_models.md`).
- `references/hooks_models_turn_control.md` - interrupt, `turn/stop_run`, request cancel, parallel tool calls, event types and delivery.
- `references/live_comms_mobs_live.md` - live (realtime voice) channels: `live/*` on `rkat-rpc`, realtime models, audio and size limits, SDK calls, the GPT Live preview, mob member live (`rkat mob live`).
- `references/live_comms_mobs_comms.md` - agent-to-agent comms: transports, `[comms]` config, `rkat run --comms-*` flags, comms tools, `comms/send` on every surface and its delivery outcomes, remote signed peers.
- `references/mobs.md` - multi-agent orchestration in depth.
- `references/live_comms_mobs_mobs.md` - mob changes since 0.8.40 (through 0.9.4): stop, shutdown, destroy and admission close, member lifetime, member binding history, helpers, `fork_off`, council, profile keys, flows and runs, multi-host mobs, mobpacks and deploy, error codes.
- `references/migration_0_9.md` - upgrading a 0.8 system to 0.9 (renames, config that no longer loads, behaviour changes), and 0.9.3 to 0.9.4.
- `references/migration_0_5.md` - only if you're upgrading from 0.5 (`host_mode` → `keep_alive`, etc.).

## Realm-first model

`realm_id` is the logical sharing key across all surfaces.

- Same `realm_id` plus the same physical storage provider/root => shared
  sessions and state.
- Different `realm_id` => strict STATE isolation (sessions, stores, event logs are realm-local).
- Backend (`sqlite`, `jsonl`, or feature-gated ephemeral `memory`) is pinned per
  realm in `realm_manifest.json`.

Config inheritance (state still never inherits):

- A realm config doc may declare a parent (`[realm.<id>] parent = "<other>"`). Resolution walks the requested head, its parent chain, and an optional configured `global` tail. The reserved `global` root uses the single HOME-rooted `~/.rkat/config.toml`. There is no flat sibling scan and no literal `default` realm.
- CONFIG inherits down the chain: models, mcp servers, hooks, skills, limits, and auth bindings. Surfaces compose the effective config over the head realm on the agent-build path, so inheritance applies everywhere, not just to auth.
- Credential READS inherit (a child resolves a binding defined in a parent or `global`); a resolved binding's owning realm is where it is DEFINED. Credential WRITES over RPC, REST and `HostAuthService` are strict-owner - callers must explicitly target the owner, and child-addressed inherited writes are rejected (reason `binding_inherited`). CLI `rkat auth logout <child>:<binding>` is the exception: it resolves the address through the loaded config and clears the owning realm's credential. Config `get`/`set`/`patch` instead read or mutate only the selected raw head doc; effective composition happens later on the agent-build path, so inherited entries are never flattened into a child automatically.
- Children may add/override but cannot remove inherited mcp entries (no tombstones). Hook entries can only be added: a child hook entry that reuses an inherited hook id is ignored, because the first occurrence (the most ancestral realm) wins. Disable an inherited hook for one session with `hooks_override.disable`.

Default persistent backend:

- New persistent realms default to `sqlite` when sqlite support is compiled.
- `sqlite` is the normal same-realm multi-process backend.
- `jsonl` remains explicit for inspectable file-backed persistence.

Default realm behavior:

- CLI (`run`, `run --resume`, `session`): workspace-derived stable realm from the context root, defaulting to the current directory.
- RPC/REST/MCP/SDK: new opaque realm unless explicitly provided.

CLI filesystem defaults:

- `context_root` controls workspace identity and project files (`.rkat/mcp.toml`, `.rkat/skills`, AGENTS/CLAUDE discovery, etc.).
- `state_root` is the parent directory containing realm directories.
- Without overrides, CLI uses `context_root = CWD` and walks upward to the nearest ancestor containing `.rkat`. A realm that already exists is used where it lies (`<project-root>/.rkat/realms` or the user-global data root; both is a split-brain refusal); a new realm goes to `<project-root>/.rkat/realms`, falling back to `<context_root>/.rkat/realms` outside a project.
- `--context-root` changes the derived `ws-...` realm id and the starting point for project-root discovery; contexts inside the same project can therefore share a state root unless `--state-root` is supplied.
- `--state-root` changes storage location only; it does not change the realm id.

Use explicit realm to share:

```bash
rkat --state-root /srv/meerkat/realms --realm team-alpha run "Plan release"
rkat-rpc --state-root /srv/meerkat/realms --realm team-alpha
rkat-rest --state-root /srv/meerkat/realms --realm team-alpha
rkat-mcp --state-root /srv/meerkat/realms --realm team-alpha
```

## OS sandboxing (execution confinement)

Realms isolate state, not processes. OS confinement is a separate, preview
boundary that the host installs. Stock `rkat`, `rkat-rpc`, `rkat-rest` and
`rkat-mcp` read the `[confinement]` config section (`all`, or `shell`,
`command_hooks`, `mcp_stdio`: `trusted_host`, `workspace` or `custom`) and the
`--confinement <trusted-host|workspace>` flags; a less trusted realm document
can add a requirement but never weaken one, and an ordinary config write
cannot change the section. A Rust host builds an `ExecutionConfinement`
requirement (file read and write grants, network, Unix sockets) and passes it
to `AgentFactory` with `with_shell_confinement` (shell tool commands,
background jobs and their monitors), `with_command_hook_confinement` (command
hooks) and `with_mcp_stdio_launch_profile` (stdio MCP servers), or installs a
resolved `HostConfinement` with `with_host_confinement`. Backends: macOS
Seatbelt (`MacOsSeatbeltV1`) and Linux namespaces plus seccomp
(`LinuxNamespaceSeccompV1`, x86_64 and aarch64); Windows has no backend. A
`Required` profile is enforced completely or the launch is refused before it
runs (a shell call gets `confinement_refused`); Meerkat never falls back to
running it unconfined. The default is `TrustedHost`, which is no OS sandbox.
`--tools none`, the shell's `restrict_to_project` check, realm isolation and
governed-realm grants are not OS sandboxes, and confinement grants no
permission of its own.

Details (requirement fields, what each backend refuses, environment rules,
refusal causes and events, what is not a sandbox): `references/security.md`.

## Storage architecture (0.8.4+)

One `StorageLayout` path authority is resolved at bootstrap and carried through
composition; no crate resolves ambient roots on its own (CI-gated). Realm state
roots resolve **realm-id-first** across two candidates — the project-local
`<project-root>/.rkat/realms` and the user-global data root:

- An explicit `--state-root` wins (no probing).
- A realm that already exists under exactly one candidate is used where it lies
  (resolution never creates an empty twin under the other root).
- A realm materialized under BOTH candidates is a typed split-brain refusal
  pointing at `rkat storage doctor`; `rkat storage migrate --apply
  --adopt-root <PATH>` reconciles it (the other copy is archived read-only,
  never merged).
- A realm that exists nowhere goes to the surface's documented default root,
  and first materialization reserves the realm across that invocation's probed
  candidate set so processes using the same set cannot mint twins. Different
  candidate sets do not coordinate; use a common explicit root or matching
  server context for cross-surface first starts.

Server surfaces (`rkat-rpc`/`rkat-rest`/`rkat-mcp`) keep their no-flags
behavior; the project-local candidate participates in their probing only when
they are started with an explicit context root.

SQLite mechanics are shared through the `meerkat-sqlite` crate:

- Named connection profiles (`Primary` / `ReadOnly` / `Maintenance`); opening a
  connection never runs schema DDL.
- Every database carries a `meerkat_schema(domain, version)` migration ledger;
  stores converge their domain at open under a pinned concurrent-open
  protocol, and a file whose ledger is ahead of the binary refuses typed
  (`SchemaFromTheFuture`) — checked preflight, before any mutating pragma
  (WAL) touches the file, so a rollback candidate fails cleanly instead of
  crash-looping.
- Each database has a sibling `<file>.mfence` fence-lock file: store
  operations hold shared per-operation guards; offline maintenance
  (`rkat storage migrate`) takes the exclusive side after in-flight operations
  drain.
- `WriteContact` types each profile's honest no-write guarantee (WAL reads may
  still need sidecar files).

Storage providers: one `RealmStorageProvider` (meerkat facade seam) supplies
all durable stores for a realm; the built-in `DiskStorageProvider` reproduces
sqlite/jsonl/memory realms. `realm_manifest.json` format 2 adds `provider` and
`ephemeral_domains`; readers refuse formats newer than they support, and
realms pinned to an external provider refuse built-in disk opens typed. Every
store slot carries a machine-readable `DurabilityDeclaration`
(`durable` / `rebuildable_cache` / `scratch`, plus how the slot actually
resolved) over seven required domains — sessions, runtime, schedule,
workgraph, jobs, blobs, artifacts. Completeness is enforced (exactly one declaration per
domain), and a durable slot resolving non-persistent without the manifest
declaring that domain ephemeral is a startup error — never a silent in-memory
fallback. Downstream backends validate against the published
`meerkat-store-conformance` harness (per-trait capability profiles; the same
suite CI runs against the in-repo stores).

Runtime-backed session authority belongs to the selected store representation,
not to the portable `Session` document. WholeBlob binds the exact serialized
row to a store revision and row SHA-256. HeadCanonical binds its store revision
to the exact canonical head token, row/rewrite prefixes, and component roots.
Final run promotion consumes the store-issued provisional receipt and updates
metadata only; it never asks a deserialized Session to certify itself.

The ordinary-open compatibility floor introduced in 0.8.11 is Meerkat 0.8.10. Backend activation owns the
one-time conversion in the same transaction that proves the exact released
schema, key, and source bytes, installs current store authority and catalog
state, and removes retired proof carriers. Ordinary reads and writes never
perform a lazy legacy conversion. For exact supported pre-floor SQLite schemas,
use the explicit offline recovery path
`rkat storage migrate --apply --bridge-pre-0-8-10`. It authenticates schemas
under the maintenance fence before normal migration; ordinary opens do not
invoke it. Unknown, ambiguous, malformed, future, or unsupported state is refused.
Inspect the per-domain report: legacy records may be refused and earlier
domains may have migrated before a later failure. Other old state still needs
a compatible older-binary migration before repinning. Per-mob databases under
`mobs/` are report-only in the offline sweep and converge through their owning
store on open; this is not arbitrary-vintage recovery.

Operator verbs: `rkat storage doctor` (read-only, live-realm-safe, `--json`),
`rkat storage migrate` (dry-run by default, `--apply` for the fenced
migration), `rkat storage prune` (registered `*.pre-<version>-<timestamp>` /
`*.corrupt-<timestamp>` artifacts only; age from the name's embedded
timestamp). All three dispatch before runtime-scope resolution and reject
`--isolated`/`--default-model`. Exact flags: the meerkat-cli-reference skill.

## Runtime-backed vs standalone

- **Runtime-backed** (CLI, REST, `rkat-rpc`, `rkat-mcp`, Web SDK / WASM): `MeerkatMachine` owns input admission, keep-alive, comms drain, cancellation, and terminal classification. Persistent profiles also support recovery on restart.
- **Browser profile**: direct sessions and mob members share the canonical runtime with in-memory storage and execution limited to the page lifetime. `BrowserRuntimeProfile` owns typed exclusions and their clearing actions. Direct `keepAlive: true` sessions with a `commsName` accept runtime-admitted peer work.
- **Standalone Rust embeddings and tests**: explicit in-memory execution without runtime-owned keep-alive or cross-process recovery.

The Rust SDK lets you pick `RuntimeBuildMode::SessionOwned(bindings)` or
`RuntimeBuildMode::StandaloneEphemeral` (the build-options default). Web/WASM
always uses `SessionOwned`; the browser profile does not select a second
session substrate.

## Managed remote and external mob members (advanced)

A managed remote member remains a Meerkat-owned, session-backed agent. Bind a
member host, grant the required scope, and set `SpawnMemberSpec.placement` to
that host. The member-host protocol materializes the session in the remote
host's realm. Placement is separate from runtime binding: do not mark a managed
placed member `External` and do not attach an external binding.

`MobBackendKind::External` and `RuntimeBinding::External` instead bind one
already-running, unmanaged peer process. The current external binding carries
the advertised address, the peer's Ed25519 public identity, and a typed
`bootstrap_token`; the mob derives the concrete peer id from the public key.
Local session-backed members use `RuntimeBinding::Session` and need no extra
wiring. Members are addressed everywhere by stable `AgentIdentity`;
runtime/binding identity rotates underneath.

For a remote `rkat` process, use `rkat run --comms-listen-tcp <ip:port> --comms-advertise-tcp <host:port> --comms-binding-out <path> --keep-alive ...` (since 0.9.1 `--comms-advertise-tcp` takes `HOST:PORT` or `tcp://HOST:PORT`, where HOST is a DNS name, an IPv4 address or a bracketed IPv6 address, resolved when a peer connects; the flag wins over a realm `[comms] advertise_address`, which wins over the address derived from the listener, and a missing flag no longer erases the configured value; 0.9.0 accepted only IP literals and ignored the realm setting). The generated binding JSON is the safest source for mob spawn/host tooling because hand-written address-only bindings and query-string-only bootstrap tokens are rejected.

`rkat-rpc --tcp` is not this channel. It exposes JSON-RPC to hosts and SDKs; signed Meerkat peer traffic uses the `rkat run --comms-*` comms listener.

For the intentional RPC/REST/MCP/CLI/SDK exposure boundaries of managed
multi-host mobs, use the authoritative
[multi-host surface matrix](references/mobs.md#multi-host-surface-matrix).

## Surfaces

| Surface | Protocol | Use case |
|---------|----------|----------|
| `rkat` CLI | Shell commands | Developer workflows, scripting |
| REST (`rkat-rest`) | HTTP/JSON + SSE | Services and language-agnostic clients |
| JSON-RPC (`rkat-rpc`) | JSON-RPC 2.0 over stdio (default) or TCP (`--tcp <addr>`; non-loopback binds need `--allow-remote`, which adds no auth); optional live-channel WebSocket (`--live-ws`, serves `/live/ws`) | IDE integration, SDK backend, embeddable services |
| MCP (`rkat-mcp`) | Model Context Protocol | Expose Meerkat as tools to other agents |
| Python SDK (PyPI `meerkat-sdk`, import `meerkat`) | Async Python over RPC | Python applications |
| TypeScript SDK (`@rkat/sdk`) | TypeScript over RPC | Node.js applications |
| Web SDK (`@rkat/web`) | WASM in-browser | Browser applications, mobpack deployment target |
| Rust SDK (`meerkat`) | Direct library API | Embedded Rust systems |

For full per-surface schemas and examples, load: `references/api_reference.md`.
For detailed mob behavior across all surfaces, load: `references/mobs.md`.

## MCP server config (CLI)

Use `rkat mcp ...` to manage MCP server configuration. This writes config; new `rkat run` and `rkat run --resume` sessions load configured MCP servers (realm, project and user) and expose their tools to the agent. A governed `rkat-rpc` carries the realm's configured servers into each session, gated by the agent's grants, and `rkat-mcp` sessions a schedule creates start the realm's own servers. Ordinary `rkat-rpc`, `rkat-rest` and `rkat-mcp` sessions start no configured servers (attach them with live `mcp/add`).

Config locations:

- Project scope (default): `.rkat/mcp.toml`
- User scope: `~/.rkat/mcp.toml`
- Realm scope: `[[tools.mcp_servers]]` in the selected realm's own config, inherited by its child realms; `--expected-generation <N>` makes the write conditional

A realm server and an `mcp.toml` server with the same name must be defined identically, or sessions fail with a conflict naming both. Realm servers are literal: `${VAR}` expands only in `mcp.toml`, and realm writes refuse it.

Command forms:

```bash
rkat mcp add <NAME> [--transport stdio|http|sse] [--scope project|user|local|realm] [--expected-generation <N>] [-H KEY:VALUE...] [-e KEY=VALUE...] [--oauth-account <ACCOUNT_ID> | --oauth-account-selection discover|unverified] [--allow-operation <OP>...] [--url <URL> | <URL> | -- <CMD...>]
rkat mcp login <NAME> [--scope project|user|local|realm] [--allow-headless]
rkat mcp remove <NAME> [--scope project|user|local|realm] [--expected-generation <N>]
rkat mcp list [--scope project|user|local|realm] [--json]
rkat mcp get <NAME> [--scope project|user|local|realm] [--json]
```

Examples:

```bash
# stdio server; command and args go after --
rkat mcp add filesystem -- npx -y @modelcontextprotocol/server-filesystem .

# HTTP server
rkat mcp add linear --url https://mcp.example.com
rkat mcp add --transport http remote-tools https://mcp.example.com/api --oauth-account-selection discover
rkat mcp login remote-tools

# User-wide server with environment passed to the stdio process
rkat mcp add github --scope user -e GITHUB_TOKEN=ghp_... -- npx -y @modelcontextprotocol/server-github

rkat mcp list
rkat mcp get filesystem --scope project
```

Notes:

- Use `--transport http` for streamable HTTP and `--transport sse` for legacy SSE.
- `-H KEY:VALUE` is for HTTP/SSE headers; `-e KEY=VALUE` is for stdio process environment.
- OAuth for HTTP MCP servers is discovered at connect/login time; do not add an auth schema. Interactive login needs an account selection on the server entry, written by `rkat mcp add` (HTTP only; the flags conflict): `--oauth-account <ACCOUNT_ID>` (`oauth_account`, a provider subject ID, not an email), `--oauth-account-selection discover` (bind the account the provider verifies at the first login) or `--oauth-account-selection unverified` (resource-bound access with no account evidence). `rkat mcp login <name>` runs the login explicitly (`--allow-headless` without a terminal); it never sets a selection, and without one it refuses before any network request with the `rkat mcp remove`/`rkat mcp add` commands for each choice. A `discover` (or `--oauth-account`) server whose issuer publishes no OpenID Connect UserInfo is refused before client registration, nothing stored, naming `unverified` as the deliberate alternative. `rkat run` defaults to `--mcp-auth stored`; use `--mcp-auth interactive` for first-use browser auth in a TTY run (any account selection). MCP OAuth credentials are per realm (0.9): each realm logs in for itself.
- `--allow-operation <OP>` (repeatable) writes an access ceiling, `access = { allowed_operations = [...] }`: every other raw MCP operation is neither published nor callable. Sessions started after the write enforce it; a live session keeps its definition until that server reloads. A ceiling narrows access only; it does not make the allowed operations read-only.
- A stdio server can name secret environment variables in `secret_env` (names only). Only servers defined in a realm's own config document have custody: that realm's credential store holds the values (set by a Rust host with `meerkat::set_mcp_service_secret`), they are read at launch for that exact definition, never from the host environment, and an `mcp.toml` or live-added server that declares `secret_env` is refused.
- `rkat mcp` is a config surface. Live mutation of a running session uses JSON-RPC `mcp/add|remove|reload`, REST `POST /sessions/{id}/mcp/*`, MCP-server tools, or SDK helpers. After an RPC or REST `auth/login/complete` for an MCP target, live sessions awaiting that exact server (name, URL and account) re-add it at their next turn boundary; an RPC `auth/logout` releases it from the sessions connected with it the same way. No host `mcp/reload` is needed.

## WorkGraph

WorkGraph is the realm-scoped durable commitment graph for shared agent work.
It owns work item lifecycle, dependency readiness, claims/leases, evidence
references, and event snapshots. It is optional and enabled through
`enable_workgraph` / `tools.workgraph_enabled`.

- Agents mutate WorkGraph items through `workgraph_*` tools.
- CLI/REST/RPC/SDK expose WorkGraph observability. CLI and trusted in-process
  hosts additionally expose narrow goal/attention controls.
- CLI: `rkat workgraph list|show|ready|snapshot|events|goal-create|goal-status|goal-confirm|goal-close|attention-list|attention-pause|attention-resume`.
- RPC: `workgraph/get`, `workgraph/list`, `workgraph/ready`,
  `workgraph/snapshot`, `workgraph/events`, `workgraph/goal/status`,
  `workgraph/attention/list`.
- REST: `GET /workgraph/items`, `/workgraph/items/{id}`,
  `/workgraph/ready`, `/workgraph/snapshot`, `/workgraph/events`,
  `POST /workgraph/goal/status`, `POST /workgraph/attention/list`.
- Python/TypeScript SDKs expose typed read-only wrappers and session options
  `enable_workgraph` / `enableWorkGraph`.

Use WorkGraph for durable pending work, dependencies, claims, and evidence. Use
Schedule for time and recurrence; use memory for retrieved knowledge; use comms
for live messages.

## Current Branch Surface Snapshot

For platform integration answers in this checkout, assume the current branch
surface, not the old live-adapter/docs-refresh snapshot:

- **0.9.4.** Breaking changes and a checklist are in the last section of
  `references/migration_0_9.md`. In short:
  - No downgrade to 0.9.3 after 0.9.4 writes a runtime store (stored input
    state version 7); upgrade every process that shares one together.
  - SKILL.md frontmatter is parsed by serde-saphyr and refuses duplicated
    keys, a null `description` or a null value under `metadata`, custom tags
    and `...` lines. Unquoted extension values resolve as YAML (`0755` reads
    as `755.0`), so quote values that must stay text.
  - An admitted mob Shutdown closes admission at once (`AdmissionClosed`,
    reason `shutdown`); requests during its drain get the retryable
    `MobError::ShutdownInProgress`; destroy, durability fail-stop, actor loss
    and dispatch failure also end owner leases. `MobError` is non-exhaustive.
  - Provider refusals and content filters fail the run as `content_filtered`
    with a typed stop and detail (`error_report.reason` is an object); refusal
    text is never committed as an answer.
  - Compaction sizes the summary source for the summarizing model and reports
    it (`summary_source`, `estimated_history_tokens_after`); custom
    `Compactor`s implement the new `prepare_for_summarization`.
  - `Agent::replace_client` and `SessionService::set_session_client` refuse a
    client for another provider or model
    (`AgentError::ClientModelChangeRequiresHotSwap`); change the model of a
    running session with `hot_swap_llm_identity` /
    `hot_swap_session_llm_identity`.
  - Governed realms: `bind_prompt` binds only operator prompts; host-reported
    outcomes use `bind_host_outcome`; an environment model key's route now
    reaches the controller grant, which admits it only when a `--route` key
    names it (0.9.3 refused it as an unusable controller credential).
  - The tool configuration change bit is `settled`, not `persisted` (rows
    stored before 0.9.4 keep `persisted`); a 0.9.3 SDK against a 0.9.4
    `rkat-rpc` fails on `tool_config_changed`, so upgrade SDKs together.
  - Realtime: a response the provider ends without completing it arrives as
    the non-terminal `turn_failed` live observation (route on
    `failure.status`); nothing of it is committed and it is not retried.
  - Forks of a source with activated host instructions that start the
    child's first turn themselves (`fork_off`, council participants, RPC
    `session/fork_at` / `session/fork_replace`) are refused with
    `FORK_INSTRUCTION_REFUSED` unless the host sets, or the RPC call passes
    as `inherited_instructions`, the `reactivate_inherited` policy; upgrade
    member hosts before their controller.
  - In-turn steers are written into the running turn's transcript (or one
    follow-up turn), and a 0.9.3 peer that checks signatures drops a 0.9.4
    `reply_to_peer` reply, so upgrade comms peers together.
  - Mobs keep their incarnation across resume (operator grants survive it);
    mob stores migrate to schema 12, which 0.9.3 refuses, so back them up;
    the operator grant functions take `MobHandle::live_incarnation()`.
  - Release binaries build SQLite without its process-wide locks; a
    `cargo install rkat` or embedding build sets `LIBSQLITE3_FLAGS` itself
    (see the deploying guide, "SQLite Lock Profile"); `rkat storage doctor`
    warns `sqlite-lock-profile`.
- **0.9.0 (2026-10-13).** Breaking changes and an upgrade checklist are in
  `references/migration_0_9.md`. In short:
  - MobKit is now Meta in Core's public surface, with no aliases: JSON-RPC
    `meta/jobs/{heartbeat,progress,checkpoint,complete,fail,cancel_ack}`
    (host jobs: an attempt's lease, progress and terminality under its fence),
    Python `meta_job_*`, TypeScript `metaJob*`, Rust `MetaJob*Params`.
  - Governed realms (preview; `local-authorization`, in default `rkat` and
    `rkat-rpc` builds): `rkat realm govern`, per-target grants with
    `rkat realm grant`, `--as-agent`, review above R1 from
    `[[governance.review.rules]]`, and R3 human consent on a stock governed
    `rkat-rpc` stdio pipe. See "Governed realms and review" below and
    `references/security_governance.md`.
  - MCP: per-server access ceilings, per-realm MCP OAuth credentials, realm
    service secrets for stdio servers (`secret_env`), automatic re-add and
    release after MCP login and logout, bounded graceful stdio close, a
    host-owned HTTP egress profile, and MCP Apps for UI hosts
    (`AgentFactory::mcp_apps(true)`, off by default).
  - A denying mandatory model-request hook withholds only that operation and
    the run continues; Guardrail configs that this cannot serve now fail to
    load (see Hooks).
  - Mobs: admission closes once the last exported `MobHandle` clone drops and
    the mob is idle; destroy can answer the retryable `mob_destroy_incomplete`;
    host-persisted deliveries are scope-bound (`STALE_DELIVERY_SCOPE`).
  - OS process confinement (preview): `AgentFactory`
    `with_shell_confinement`, `with_command_hook_confinement` and
    `with_mcp_stdio_launch_profile` enforce an `ExecutionConfinement` through
    macOS Seatbelt or, usable from 0.9.0, a Linux namespace and seccomp
    backend, or refuse the launch (`confinement_refused`); Windows has no
    backend. In 0.9.0 only a Rust host could turn it on; from 0.9.1 the stock
    binaries also read the `[confinement]` config section and the
    `--confinement` flags. See `references/security.md`.
  - Credentials stay in the file store `<config dir>/meerkat/credentials`;
    keychain access is typed (`credential_access_*` reasons) and never read
    as absence. See `references/security.md`.
  - `rkat-rpc` shuts down gracefully on SIGTERM or an interrupt, as on stdin
    EOF (with `--tcp`, connections drain first).
- Default hosted text selections are OpenAI `gpt-6-astra` (backend availability
  remains independent; Azure, Copilot, and custom support are not implied),
  Anthropic `claude-opus-5-5`, and Gemini `gemini-4-argon` (default; paid
  Gemini API tiers only; `gemini-3.8-flash` is the recommended Flash model,
  the free-tier choice, and the `rkat auth login google` / Copilot Gemini route
  default).
- WorkGraph is available through agent `workgraph_*` tools plus host
  observability (`workgraph/list`, `ready`, `snapshot`, `events`,
  `goal/status`, `attention/list`, REST and SDK equivalents). CLI and trusted
  in-process hosts can create, confirm, close, pause, and resume narrow
  session-bound goal attention bindings; public REST/RPC do not expose those
  mutators.
- Session transcript rewrite is same-session, append-audited state:
  `session/rewrite_transcript`, `session/transcript_revision`,
  `session/transcript_revisions` (revision list), and
  `session/restore_transcript_revision` update/read the transcript head
  without changing session identity. Head reads use
  `revision: "current"` (typed `RevisionSelector`); rewrites may preserve
  the `injected_context` role but can never mint `compaction_summary`
  (rejected fail-closed).
- Hosts can attach ambient context alongside (not inside) the user's message
  on every submit-work path via `injected_context` (RPC `turn/start` /
  `session/create`, REST continue/create, SDK `injected_context` /
  `injectedContext` params, mob work submission). Entries land as separate
  typed transcript messages excluded from semantic-memory indexing.
- Spawn/fork tool containment: `tool_access_policy` (`inherit`,
  `allow_list`, `deny_list`, `read_only`) on mob spawn specs, `HelperOptions`,
  `AgentBuildConfig` and RPC `session/fork_at|fork_replace`. Since 0.8.52 the
  tools an allow or deny list (or a mob profile `tools.deny`) rules out are
  removed from the model-visible tool list, and any call that still reaches
  one is refused as an ordinary `access_denied` tool error. `read_only` keeps
  the list unchanged and refuses at call time, as do the separate application
  tool policy and dispatch admission gates. `Inherit` composes transitively
  from the parent member.
- Schedules can target host-registered runnables (`TargetBinding::HostRunnable`)
  in addition to sessions, identities, and mobs.
- Comms envelopes can carry a signed sender content-taint declaration
  (`content_taint`); hosts set a runtime-level outbound declaration or a
  per-send tri-state override, and read the received declaration from the
  typed transcript comms notice.
- Semantic memory has host lifecycle APIs: per-scope delete (`drop_scope`)
  and paged enumeration (`enumerate_scoped`); the Hnsw store loads scope
  indexes lazily instead of re-embedding every session at agent build.
- CLI runs store realm state project-locally by default. `--realm` and
  `--isolated` change the realm id, not the location; `--context-root` moves
  the project-root search; only `--state-root` selects another root. A realm
  that already exists under the user-global root is used there.
- Storage (0.8.4): `StorageLayout` path authority + realm-id-first dual-root
  resolution with typed split-brain refusals; the `meerkat-sqlite` mechanics
  crate (profiles, `meerkat_schema` ledger, `.mfence` fences); the
  `RealmStorageProvider` seam with manifest v2 and fail-closed durability
  declarations; `rkat storage doctor|migrate|prune` offline verbs; the
  published `meerkat-store-conformance` harness. See the Storage architecture
  section above.
- The stream-inactivity watchdog (`[retry] stream_inactivity_timeout`) covers
  clients reporting a monotonic raw-event count through
  `stream_activity_count()`; `"disabled"` opts out. An explicit window applies
  to every model; omitted, each call uses the current model's own window
  (`gemini-4-argon` 900s, `gpt-5.5-pro` 7200s, a self-hosted model's
  `stream_inactivity_timeout_secs`), else 300s. Stalls surface as retryable
  `StreamStalled`. A custom client returning the default `None` has no
  inactivity watchdog, only any configured hard call/turn deadlines.
- `ToolAccessPolicy::ReadOnly` is a call-level, name-independent policy: only
  tools whose owning dispatcher positively declares `ToolMutationClass::ReadOnly`
  may execute. Unknown, MCP, shell, and undeclared host tools fail closed.
  Provider-native tools bypass the dispatcher, so the factory disables them
  itself (`ProviderNativeToolPolicy::DisableAll`) whenever the tool access
  policy restricts, an application tool policy governs or a dispatch
  admission is set; an explicit web-search enable then gets the ordinary
  `web_search` tool, which the gate checks. The model-visible tool list is
  unchanged and denied calls return ordinary `access_denied` tool results.
  Enable it with `read_only = true` in a mob profile's `tools` table or the
  wire value `{"type":"read_only"}` (`mob/spawn`, `session/fork_at`,
  `session/fork_replace`).
- Trusted Rust runtime hosts can use `meerkat_runtime::submit_bounded` for a
  bounded admission observation with a mandatory idempotency key and typed
  admitted, collapsed, refused, or timed-out outcomes. The bound never cancels
  the work. This is a Rust host seam, not a REST, RPC, or SDK method.
- A completed model turn can now have no provider accounting. In that case
  `turn_completed.usage` is absent and consumers must skip the row, not treat
  it as zero. `turn_usage_accounting_unmeasured` explains absence;
  `turn_usage_accounting_identity_disputed` reports counters whose provider or
  model attribution differs without rewriting them or failing the answer.
- `limits.max_turn_duration` is an optional positive per-turn aggregate
  wall-clock ceiling, re-armed at each run entry. It is unset by default and
  complements the agent-lifetime `max_duration` plus per-segment timeouts.
- Runtime health now measures `session_liveness` and `session_run_start` in
  addition to the earlier dimensions. Job health separates
  `pending_outbox_jobs` from `runtime_inbox_backlog`, carries complete versus
  truncated census coverage, and treats `unreadable` as a third state rather
  than healthy or degraded.
- Durable resume holds: a persistent session whose last turn lost its
  runtime boundary commit is recovered the next time the session is started
  or attached (a turn, resume or external event on a cold session); plain
  reads (`session/read`, history, status) never recover it. The durable
  content is never discarded. While a session is held, resume fails with
  JSON-RPC -32003 (`SESSION_NOT_RUNNING`, HTTP 409) and data
  `{durable_resume_hold, session_error_code, session_id, content_retained: true}`:
  `SESSION_DURABLE_TAIL_HELD_FOR_RECOVERY` (`tail_held_for_recovery`; an
  operator resolves an ambiguous held tail, or an interrupted one held
  beside another run's stranded inputs, with
  `rkat session resolve-held-tail <session-id>`, then `--apply`),
  `SESSION_DURABLE_TAIL_RECOVERY_REFUSED` (`recovery_refused`, 0.8.11: retry
  after the conflicting runtime quiesces),
  `SESSION_DURABLE_EVIDENCE_QUARANTINED` (`evidence_quarantined`), and
  `SESSION_WHOLEBLOB_AUDITED_ENDPOINT_DIVERGENCE`
  (`audited_endpoint_divergence`, 0.8.41: repair with
  `rkat session repair-wholeblob <session-id> --json`, then `--apply`).
  Treat these as "session intact, not runnable yet", not as service faults.
- `rkat run --output html` / `--html` writes standalone HTML artifacts.
- Image generation uses `generate_image` with OpenAI `gpt-image-2` and Gemini
  `gemini-nano-banana-2.1` as provider defaults.
- Mob host surfaces include batch wiring, helper/fork/respawn/cancel/wait
  operations, profile CRUD when a profile store is present, store-issued
  resume verdicts, and certified bounded helper results for SDK consumers.
- Durable member resume is same-role by default. A trusted host can declare
  `resume_from_role` on one exact cold Resume request to migrate the role while
  preserving mob id, member identity, session, and transcript. A missing or
  wrong predecessor role, or an already-live session, fails closed. This field
  is available on host `SpawnMemberSpec` and remote materialization, not
  agent-callable spawn commands or standing profiles.
- When installed, persistent event projection uses a dedicated unbounded queue,
  so bounded UI-ring lag does not drop projector input. EventStore append is
  still asynchronous best-effort derived state, not session commit authority;
  lagging live subscribers receive typed `StreamTruncated` events whose reason
  is `StreamLagged { dropped }`.
- Approvals are an RPC-host surface: `approval/request|list|get|decide`, with
  matching Python and TypeScript client wrappers. A host's one approval issuer
  keeps durable records in a `FileApprovalStore` under a persistent host's
  store path (a persistent host without one is an error, and the store needs a
  Unix directory durability barrier, so elsewhere a persisted host's approval
  service is unavailable) or process-local records for an explicitly
  ephemeral host. Store failures are typed: `APPROVAL_STORE_UNAVAILABLE`
  (-32033), `APPROVAL_CONFLICT` (-32034), `APPROVAL_COMMIT_UNKNOWN` (-32035).
  REST, MCP, and CLI do not expose the family, remote member-host forwarding
  is not part of multi-host v1, and a host-created record does not authorize
  an effect or gate tool execution; governed-realm review issues and spends
  its own approvals.
- `turn/stop_run` (SDK `stop_run` / `stopRun`) stops one exact run, named by
  the `run_id` of its `run_started` event; the receipt `outcome` is
  `stopped`, `not_current` (a stale run id), or `not_stoppable`.
- Provider cache breakpoints that no longer bind to the committed transcript
  emit `provider_cache_breakpoints_discarded`; the turn continues with reduced
  cache reuse. Genuinely unknown provider failures now enter bounded retry,
  while explicit terminal classes remain non-retryable.

## Mob behavior (current contract)

- CLI `run`/`run --resume` compose `mob_*` tools through `meerkat-mob-mcp` dispatcher integration when mob tools are enabled, for example with `--tools full` or config `tools.mob_enabled=true`.
- CLI `mob ...` is the explicit lifecycle surface for persisted mob registry operations.
- RPC/REST/MCP server/Python SDK/TypeScript SDK expose mob capability through typed `mob/*` / `meerkat_mob_*` host control planes. Agent-internal `mob_*` tools are late-bound through `SessionBuildOptions.mob_tools` (`MobToolsFactory`); `external_tools` remains for callback/MCP-backed tools.
- Member runtime default is `autonomous_host` when `runtime_mode` is omitted; `turn_driven` is explicit opt-in for controlled dispatch paths.
- Spawned mob members use deferred creation; after activation, fresh
  runtime-backed autonomous members automatically admit a kickoff using the
  initial message or fallback prompt. This is real runtime dispatch and can
  invoke the provider without another user call. Turn-driven members and
  recovery-suppressed kickoff paths are distinct; see the mobs reference.
- Mob persistence is SQLite/WAL-backed (`MobStorage::persistent()` opens `SqliteMobStores`). In-memory storage is used for tests, WASM and `rkat mob deploy`. The previous exclusive-handle mob store has been removed.
- Rust hosts: a mob closes admission once the last exported `MobHandle` clone drops and it is idle (`MobError::AdmissionClosed` for later spawns, work, flows, wiring and resume; stop, destroy, retirement and reads stay available), so keep a handle for as long as the mob must admit work. Since 0.9.4 an admitted Shutdown, an admitted destroy, a durability fail-stop, a lost actor and a dispatch failure close admission too (reasons `shutdown`, `destroyed`, `durability fail-stop`, `actor lost`, `dispatch failure`), and `acquire_owner` is refused after any of them; while a Shutdown drains, other requests get the retryable `MobError::ShutdownInProgress`. A destroy removes the mob's database files only after its actor ended and its observer drained; while a handle or store in the process still holds it, every surface answers the retryable `mob_destroy_incomplete` and keeps the files: drop the handles and retry.
- Prefabs are gone. All mob creation uses `MobDefinition` only (CLI, REST, RPC, MCP, SDKs).
- `AgentMobToolSurface` in `meerkat-mob-mcp` has thirteen base definitions:
  `delegate`, `conclude_objective`, `mob_create`, `mob_destroy`,
  `mob_spawn_member`, `fork_off`, `council`, `mob_retire_member`,
  `mob_check_member`, `mob_list_members`, `mob_list`, `mob_wire`, `mob_unwire`.
  `conclude_objective` accepts the final outcome for this turn's pre-addressed
  kickoff objective. `fork_off` takes a task and child identity, forks a durable
  committed transcript prefix through the resume path, and retains the child.
  `council` forks existing specialist members into a bounded temporary
  discussion mob and cleans it up. These are not `MemberLaunchMode::Fork`'s
  rendered-history prompt seeding. Generated authority is required for
  visibility; per-call scope, objective, and durable-fork prerequisites still
  apply. A realm profile store adds `mob_profile_create`, `mob_profile_get`,
  `mob_profile_list`, `mob_profile_update`, and `mob_profile_delete`;
  `mob_profile_list_sources` additionally requires a parent tool snapshot
  provider. See the mobs reference for bounds and arguments.
- Portable mob artifacts are available through mobpack (`rkat mob pack/inspect/validate/run/deploy`) and browser deployment (`rkat mob web build`).
- Live (audio/video) channels are exposed through the caller-initiated `live/*` surface, not the previous attachment-status family. Capability detection still uses `ModelCapabilities.realtime` to decide whether a model can back a live channel; channel lifecycle is caller-initiated through the `live/*` JSON-RPC methods (and SDK equivalents) below. The previous `session/realtime_attachment_status`, `mob/member_status.realtime_attachment_status`, `realtime/open_info`, and `RealtimeAttachmentStatus` enum have been removed.
### Live channels (audio/video)

Live is the single subsystem for audio and other realtime modalities. The
ordinary OpenAI Realtime path uses `gpt-realtime-2`; open its channel explicitly.
`live/open` returns a typed `LiveOpenResult` with the negotiated transport
bootstrap (e.g. WebSocket URL for `rkat-rpc`'s `--live-ws` listener at `/live/ws`)
and a `WireLiveChannelCapabilities` advert describing modalities, continuity,
and tool semantics. Optional positive `seed_max_chars` requests a core-owned
whole-turn seed suffix; omission preserves the full canonical seed.

The catalog also has public `gpt-live-1` and experimental `gpt-live-1-codex`.
Public GPT Live requires opt-in `openai-live` (including WebRTC and session-store
dependencies), a supported OpenAI API backend with API-key material, WebRTC
transport, and provider-managed turns (not explicit commit). The host must
install execution-profile authority and advertise `live.execution_identity.v1`;
callers select a registered profile with
`execution_identity: { version: "v1", profile_id: "<host-registered-profile>" }`.
That profile owns the channel's model/auth/execution identity; the selector
does not rewrite the durable session identity. The stock host does not supply
this authority merely because the feature is compiled. The Codex row uses the
separate deprecated private `experimental-gpt-live` path and its admission
requirements. Neither GPT Live row is a drop-in model-name replacement for
the ordinary Realtime factory.

| Surface | Open channel | Observe / control |
|---------|--------------|-------------------|
| JSON-RPC | `live/open` (returns `LiveOpenResult` with transport bootstrap + capabilities) | `live/status`, `live/refresh`, `live/send_input`, `live/commit_input`, `live/interrupt`, `live/truncate`, `live/playback_complete`, `live/close`; `live-webrtc` builds started with `--live-webrtc` also serve `live/webrtc/answer` (answer an offer) and `live/media_health` (reply to a `live/media_health_requested` notification) |
| Python SDK | `client.live_open(session_id, seed_max_chars=..., transport=...)` | `client.live_status / live_refresh / live_send_input_text / live_send_input_audio / live_send_input_image / live_send_input_video_frame / live_webrtc_answer / live_commit_input / live_interrupt / live_truncate / live_playback_complete / live_media_health / live_close`; `client.on_live_notification(callback)` for `live/*` notifications |
| TypeScript SDK | `client.liveOpen({ session_id: sessionId, seed_max_chars: ..., transport: ... })` or `LiveChannel.session(..., { seedMaxChars: ..., transport: ... })` | `client.liveStatus / liveRefresh / liveSendInput / liveSendInputImage / liveSendInputVideoFrame / liveWebrtcAnswer / liveCommitInput / liveInterrupt / liveTruncate / livePlaybackComplete / liveMediaHealth / liveClose`; `client.onLiveNotification(listener)`; for WebRTC, wait for ICE gathering and send `peer.localDescription.sdp` through `client.liveWebrtcAnswer(...)` |
| Rust | `meerkat_live::LiveAdapterHost` + `meerkat_core::live_adapter` adapter trait, exercised through `SessionRuntime` | typed observations and capability reads through the live host |

Seed-window contract: `seed_max_chars` must be positive; zero is rejected by
server validation. Core counts serialized seed messages and selects complete
recent turns rather than slicing a turn. The resolved root prompt is never
dropped and must fit the window; an existing compaction-summary head may be
retained. Any truncation must return degraded continuity. Runtime system
context and full canonical image identity, tombstone, and accounting sidecars
remain outside the message window, so this is not a hard cap on the provider's
entire instruction payload.

The TypeScript 0.8.24 `answerLiveWebrtcOffer(...)` convenience helper sends
the original offer before ICE gathering completes. Do not use it for the
canonical browser exchange. Keep the peer connection in the browser, wait for
ICE completion, and call `client.liveWebrtcAnswer(...)` with the completed SDP.

Live image input requires a caller-stable, session-scoped `idempotency_key` on
both `LiveInputChunkWire::Image` and `RealtimeImageChunk`. The convenience
signatures are
`live_send_input_image(channel_id, idempotency_key, mime, data_base64)` in
Python and `liveSendInputImage(channelId, idempotencyKey, mime, dataBase64)` in
TypeScript. A successful call is queue acceptance only. Route later scoped
failures from `command_rejected`; treat only the redacted
`user_content_committed` observation with the matching key as durable success.
An exact same-key replay returns the committed identity without another
provider send, while changed MIME/bytes reject as
`image_input_idempotency_conflict`. Commit staged text/audio before an image or
expect `image_input_requires_commit`. Reconnect image hydration is bounded to
40 MiB decoded across all user-image occurrences. New input that would cross
that canonical ceiling rejects before provider send as
`image_input_history_budget_exceeded`, so accepted live input cannot make a
valid session unreopenable. Legacy/out-of-band aggregate overflow, a missing
blob, or a content-address mismatch still fails `live/open`; replay never trims
or substitutes accepted image context.

Malformed base64 rejects as `image_input_invalid_base64`. `live/refresh` never
replays history: model/provider swaps and canonical transcript or user-content
registry rewrites require close + reopen, with rewrites reported as
`refresh_transcript_rewrite_requires_reopen`.

The `--live-ws` listener (`rkat-rpc --live-ws <addr>`) enables WebSocket
bootstrap. WebRTC is non-default: build `rkat-rpc` with the `live-webrtc`
feature and pass `--live-webrtc`. The client requests `transport: "webrtc"`,
creates a local SDP offer, calls the returned `answer_method` with the channel
id, single-use token, and `offer_sdp`, then applies `answer_sdp`. Live methods
are absent only when no live transport is configured. Each session keeps a
single canonical history. Ordinary explicit-commit Realtime channels use
`live/commit_input` / `live/interrupt` / `live/truncate` at turn boundaries;
GPT Live follows its advertised provider-managed turn policy instead.

Practical caveats:

- One live channel per session at a time; for per-member live channels in mobs, open the channel against a member that runs on a realtime-capable profile.
- `live/open` can recover an eligible existing idle session or promote a
  deferred session without a prior model turn. Provider, transport, and
  channel-ownership admission still apply. A deferred session with
  `injected_context` must first be promoted through `turn/start`; live open
  refuses rather than dropping that context.
- Provider-native web search and tool-calling capability is per-model; check `ModelProfile` flat fields like `supports_web_search` if a tool unexpectedly disappears under a realtime model.

Wire types: see `LiveOpenParams`, `LiveOpenResult`, `WireLiveChannelCapabilities`, `LiveStatusResult`, `LiveSendInputParams`, `LiveTruncateParams`, `WireLiveAdapterStatus`, `WireLiveAdapterErrorCode`, and `WireLiveAdapterObservation` in `crates/meerkat-contracts/src/wire/live.rs`.

### Mob lifecycle (standard/default usage)

Primary CLI usage is tool-driven through `run`/`run --resume` with mob tools enabled:

```bash
rkat run --tools full "create a mob with a lead and workers, then wire and report status"
rkat run --tools full --resume <session_id> "retire worker-2 and add worker-4"
```

Where needed, the direct CLI mob surface also covers helper execution,
run resources, multi-host placement and grants, member history, and member
live control (local or placed, WebSocket only):

```bash
rkat mob spawn-helper <mob_id> <prompt> --agent-identity <id> --result-label <label> --max-text-bytes <n> [--profile <profile>] [--model <model>] [--auth-binding <REALM:BINDING[:PROFILE]>] [--json]
rkat mob fork-helper <mob_id> <source_member> <prompt> --agent-identity <id> --result-label <label> --max-text-bytes <n> [--profile <profile>] [--model <model>] [--auth-binding <REALM:BINDING[:PROFILE]>] [--fork-context full-history|last-messages] [--last-messages N] [--json]
rkat mob member-status <mob_id> <agent_identity> [--json]
rkat mob member-history <mob_id> <agent_identity> [--from-index N] [--limit N]
rkat mob force-cancel <mob_id> <agent_identity>
rkat mob respawn <mob_id> <agent_identity> [--initial-message <msg>]
rkat mob grant <mob_id> <principal> --scope <scope>...
rkat mob revoke-grant <mob_id> <principal> [--scope <scope>...]
rkat mob grants <mob_id> [--json]
rkat mob host [--listen-tcp <ip:port>] [--advertise-tcp tcp://<host>:<port>] [--descriptor-out <path>]
rkat mob bind-host <mob_id> --descriptor <path>
rkat mob revoke-host <mob_id> <host_id>
rkat mob hosts <mob_id> [--json]
rkat mob route-installs <mob_id>
rkat mob live open|status|control|close ...
rkat mob run <pack_or_mob_id> [--flow <flow_id>] [--param key=value ...] [--prompt <text>] [--detach] [--json] [--trust-policy permissive|strict]
rkat mob runs <mob_id> [--flow <flow_id>] [--json]
rkat mob status <mob_id> <run_id> [--json]
rkat mob logs <mob_id> [--after-cursor N] [--limit N] [--json]
rkat mob attach <mob_id> <run_id> [--json]
rkat mob run-flow <mob_id> --flow <flow_id> [--params <json>] [-s|--stream] [--no-stream]
rkat mob flow-status <mob_id> <run_id>
rkat mob wait-kickoff <mob_id> [--member <agent_identity>...] [--timeout-ms N] [--json]
```

### Mobpack + web build quick paths

A mobpack source directory must contain these two files:

```toml
# manifest.toml
[mobpack]
name = "review-team"
version = "1.0.0"
```

```json
{
  "id": "review-team",
  "profiles": {
    "lead": {
      "model": "claude-sonnet-4-6",
      "tools": { "builtins": true, "comms": true }
    },
    "reviewer": {
      "model": "claude-sonnet-4-6",
      "tools": { "builtins": true, "comms": true }
    }
  },
  "wiring": {
    "auto_wire_orchestrator": false,
    "role_wiring": [{ "a": "lead", "b": "reviewer" }]
  },
  "flows": {
    "main": {
      "steps": {
        "review": { "role": "lead", "message": "Review the change" }
      }
    }
  }
}
```

`manifest.toml` is TOML and requires the `[mobpack]` table with non-empty
`name` and `version`. `definition.json` is JSON and requires a string `id`;
`profiles`, `wiring`, and `flows` may be omitted. Portable mobpacks may use only
inline profiles. A profile that will spawn a member must set `tools.comms` to
`true`, because member construction rejects `comms=false`. When present,
`wiring` is an object, never an array or boolean: `auto_wire_orchestrator` is a
boolean and `role_wiring` is an array of
`{ "a": "<profile>", "b": "<profile>" }` edges. Flow `steps` are objects keyed
by step id; each flat step requires `role` and `message`. A flow dispatches to
an existing runnable member of that role; declaring a profile does not spawn
one at create or pack time. `rkat mob run` and `MobHandle::run_flow`
automatically provision one turn-driven target for each missing flat-step role
before dispatch. Pre-spawn through a host or SDK only when you need to choose
the member identity or supply non-default spawn options.

```bash
# Build portable artifact
rkat mob pack ./mobs/release-triage -o ./dist/release-triage.mobpack \
  --sign ./keys/release.key --signer-id team@example.com   # --sign requires --signer-id
rkat mob inspect ./dist/release-triage.mobpack
rkat mob validate ./dist/release-triage.mobpack --trust-policy permissive
rkat mob run ./dist/release-triage.mobpack --flow main --trust-policy permissive

# Browser bundle from prebuilt wasm-pack output
rkat mob web build ./dist/release-triage.mobpack -o ./dist/release-triage-web \
  --wasm <PKG_DIR|name_bg.wasm> --trust-policy permissive
```

Signing records the signer ID and public key in the artifact; it does not add
that signer to the user or project trusted-signers store. The explicit
`permissive` policy above is the local-development posture: the signature is
still verified and the unknown signer produces a warning. Use `strict` only
after installing the signer in a trusted-signers store.

`mob web build` requires `--wasm` pointing to a wasm-pack `--target web` output
directory or its `*_bg.wasm` file with the sibling JavaScript glue. The CLI
does not compile wasm32 itself.

### WASM runtime + Web SDK (browser embedded)

`@rkat/web` exposes the canonical runtime inside the browser: agent execution,
provider fetch transport, direct and mob sessions, in-process comms, and
embedded skills. The host page supplies configuration and drives interaction;
`BrowserRuntimeProfile` owns capability exclusions, including configured hooks.

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

For a **custom/preconfigured host**, an external resolver can supply host-owned
credentials. The selected realm must already define a compatible backend,
auth profile, and binding whose credential source is `external_resolver` with
handle `wasm_host`. Registration installs only the callback, not those
definitions, and does not replace an inline-key source. The stock public
`MeerkatRuntime.init` bootstrap above synthesizes API-key bindings only and
offers no arbitrary realm-config injection option; use its server-side proxy
path rather than treating registration as a stock-browser OAuth recipe.

With `runtime` and `authBinding` supplied by that preconfigured host:

```typescript
import { registerExternalAuthResolver, withAuthBinding } from '@rkat/web';
registerExternalAuthResolver(wasm, async (authBinding) => {
  const token = await myHostFetchToken(authBinding);
  return { kind: 'inline_secret', secret: token, metadata: {} };
});
// withAuthBinding takes (authBinding, config) and returns a config with `authBinding` set.
const session = await runtime.createSession(withAuthBinding(authBinding, { model: 'claude-sonnet-4-6' }));
```

Include `expires_at` and any required account/provider metadata in the lease
when the selected binding needs them. `authBinding` selects an existing realm
and binding on `runtime.createSession({...})`, `mob.spawnHelper(...)`, and
`mob.forkHelper(...)`; plain `mob.spawn([...])` specs currently have no auth
override and use the host/runtime credential resolution. A mob role `Profile`
does not declare an initial `auth_binding`; select a preconfigured binding
through the session or helper APIs above.
Per-session `apiKey` fields were removed; stock init uses
`anthropicApiKey`/`openaiApiKey`/`geminiApiKey`, while an external resolver
requires the host provisioning described above.

Browser scope is defined by `BrowserRuntimeProfile::require`, not a surface
allowlist. Requests for excluded capabilities return `CAPABILITY_UNAVAILABLE`
with typed capability and clearing-action data. Storage remains in memory and
execution ends with the page/runtime. For WASM internals, exact exclusions,
build commands, and the export table, see the meerkat-wasm skill.

### Mob flows (DAG runtime)

```bash
rkat mob run-flow <mob_id> --flow <flow_id> [--params '{"k":"v"}']
rkat mob flow-status <mob_id> <run_id>
rkat mob runs <mob_id> [--flow <flow_id>] [--json]
rkat mob status <mob_id> <run_id> [--json]
rkat mob attach <mob_id> <run_id> [--json]
```

Flow model: declarative DAG (`depends_on`, `depends_on_mode = all|any`), dispatch modes (`one_to_one`, `fan_out`, `fan_in`), optional `branch` + `condition`, topology rules (`advisory|strict`, `"*"` wildcard), persisted `MobRun` snapshots with `step_ledger`/`failure_ledger` and typed output envelopes. Both flat step authoring and explicit `root` authoring normalize to one frame execution engine. Explicit roots support nested `FrameSpec` and `kind = "repeat_until"` nodes (`until`, `max_iterations`, nested `body`). `run` invokes a mobpack or installed mob as a typed callable; `--prompt` binds `params.prompt`; `--detach` returns a run id, but the flow still executes only inside that CLI process, and the next mob load turns an unfinished run into `canceled`; `runs`/`status`/`logs`/`attach` operate on the same run resources. Per-flow limits live under mob `limits` (`max_flow_duration_ms`, `max_step_retries`, `cancel_grace_timeout_ms`, `max_orphaned_turns`).

Don't conflate **mob tool availability** (surface behavior - `mob_*` tools and `rkat mob` lifecycle) with **realm backend** (`sqlite`, `jsonl`, or feature-gated ephemeral `memory` in `realm_manifest.json`).

## Scheduling

The `meerkat-schedule` crate provides durable, authority-backed scheduling for automated agent task execution.

### Schedule model

A `Schedule` defines when and how agent work is delivered:
- **Trigger**: `Once { due_at_utc }`, `Interval(IntervalTriggerSpec)`, or
  `Calendar(CalendarTriggerSpec)` in a named timezone
- **Target**: `Session(SessionTargetBinding)`, `Identity(IdentityTargetBinding)` (durable mob-member identity), `Mob(MobTargetBinding)`, or `HostRunnable(HostRunnableTargetBinding)` (host-registered callback)
- **Policies**: `MisfirePolicy` (`Skip` or `CatchUpWithin`),
  `OverlapPolicy` (`AllowConcurrent` or `SkipIfRunning`), and
  `MissingTargetPolicy` (`Skip` or `MarkMisfired`)
- **Phase**: `Active`, `Paused`, or `Deleted`

**Host runnables**: in-process hosts register named runnables at startup
(`HostRunnableRegistry` in `meerkat-schedule`, implements
`ScheduleRunnableHost`) and wire the registry via
`SharedScheduleTargetAdapter::with_runnable_host`. Occurrences targeting a
`HostRunnable` flow through the normal occurrence lifecycle (listing,
history, receipts, failure surfaces unchanged); an unregistered runnable
probes as Missing (then `MissingTargetPolicy` applies) and a callback error
maps to the RuntimeRejected failure reason. Use this for host-internal
periodic work that must not be a mob member. Runtime-backed hosts pass the
registry to `spawn_runtime_backed_schedule_host` / `_with_mobs` via the
optional `runnable_host` parameter.

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

CLI, REST, RPC, and MCP can compose these tools through
`ScheduleToolDispatcher` when runtime/tool policy enables scheduling and a
suitable schedule host is present. Always-linked schedule substrate does not
itself advertise tools. Schedules are unavailable on JSONL-backed realms
(the jsonl backend declares the schedule store ephemeral and disabled) and in
the browser profile, which has no scheduler service.

### Architecture

```
ScheduleService         — CRUD + occurrence planning
ScheduleDriver          — tick loop, claims due occurrences, dispatches delivery
ScheduleLifecycleMachine   - schedule state-transition authority
OccurrenceLifecycleMachine - occurrence state-transition authority
ScheduleStore           — persistence (Memory, SQLite)
```

**Delivery**: The driver claims due occurrences, probes target availability, dispatches delivery (creates session + runs prompt), and monitors completion. The schedule host surface (`crates/meerkat/src/surface/schedule_host.rs`) bridges delivery to `MeerkatMachine`.

**Planning**: On create/update/resume, `ScheduleService` projects occurrences within a horizon (default 30 days or 64 occurrences). The store commits a schedule mutation together with the supersession of its old pending occurrences in one transaction (`ScheduleStore::commit_schedule_mutation` / `commit_schedule_refill`, returning a `ScheduleMutationCommit`).

**Stores are per-realm, not per-instance** - multiple processes may share a schedule store; occurrence claims and transitions are lease- and revision-checked (`claim_due_occurrences`, `transition_occurrence_if_current`).

### Rust SDK usage

First create/persist the target session in the runtime host's realm. In this
example, `session_result` is the `RunResult` returned by that session creation;
a fresh random ID is not a resumable target. This snippet stores and plans a
schedule only: delivery additionally requires a running `ScheduleDriver` and
a host wired to that same session service. Use
`MaterializeOnDemandSession` with a `SessionMaterializationSpec` instead if the
schedule should create its session on first delivery.

```rust
use chrono::Utc;
use meerkat_core::ContentInput;
use meerkat_schedule::{
    CreateScheduleRequest, IntervalTriggerSpec, MemoryScheduleStore,
    MisfirePolicy, MissingTargetPolicy, OverlapPolicy, ScheduleService,
    ScheduledSessionAction, SessionTargetBinding, TargetBinding, TriggerSpec,
};
use std::{collections::BTreeMap, sync::Arc};

let store = Arc::new(MemoryScheduleStore::new());
let service = ScheduleService::new(store);

let schedule = service.create(CreateScheduleRequest {
    name: Some("hourly-report".into()),
    description: None,
    trigger: TriggerSpec::Interval(IntervalTriggerSpec {
        start_at_utc: Utc::now(),
        every_seconds: 3600,
        end_at_utc: None,
    }),
    target: TargetBinding::session(SessionTargetBinding::ResumableSession {
        session_id: session_result.session_id.clone(),
        action: ScheduledSessionAction::Prompt {
            prompt: ContentInput::from("Generate the hourly report"),
            system_prompt: None,
            render_metadata: None,
            skill_refs: Vec::new(),
            additional_instructions: Vec::new(),
        },
    }),
    misfire_policy: MisfirePolicy::Skip,
    overlap_policy: OverlapPolicy::SkipIfRunning,
    missing_target_policy: MissingTargetPolicy::MarkMisfired,
    labels: BTreeMap::new(),
    planning_horizon_days: None,
    planning_horizon_occurrences: None,
}).await?;
```

For host-internal periodic work, register a runnable and target it instead:

```rust
use meerkat_schedule::{HostRunnableName, HostRunnableRegistry, HostRunnableTargetBinding, TargetBinding};

let mut registry = HostRunnableRegistry::new();
let name = HostRunnableName::parse("memory-dream")?;
registry.register(name.clone(), my_runnable)?;     // Arc<dyn HostRunnable>; duplicate names are typed errors
let registry = Arc::new(registry);
// wire into the schedule host: SharedScheduleTargetAdapter::with_runnable_host(registry)
// then target it: TargetBinding::host_runnable(HostRunnableTargetBinding { runnable: name, params: None })
```

## Quick start

### CLI

```bash
rkat "What is Rust?"                     # "run" is the default subcommand
rkat run "What is Rust?"                 # equivalent explicit form
rkat --realm team-alpha run "Create a todo app" --tools workspace --stream -v
# Global flags: --realm, --isolated, --instance, --realm-backend, --state-root, --context-root, --user-config-root
rkat help "How do I add an MCP server?"
rkat run --resume last "keep going"      # resume most recent session
rkat run --resume 019c8b99 "continue"    # resume by short prefix
rkat --realm team-alpha run --resume 019c8b99 "Now add error handling"
# Batch context: pipe finite content as context
cat document.txt | rkat run "Summarize this document"
git diff | rkat run "Review these changes" --tools safe
# Chained pipes: each rkat reads stdin, writes response to stdout
cat data.csv | rkat run "Extract entities" | rkat run "Write a story about them"
# Live streaming: --keep-alive --stdin reads stdin line-by-line as events
tail -f app.log | rkat run --keep-alive --stdin lines "Monitor and alert on anomalies"
rkat mob spawn-helper coding-swarm "Join as lead-1 and summarize the current plan." --profile lead --agent-identity lead-1 --result-label plan-summary --max-text-bytes 4096
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

Enable the facade's `session-store` feature for this persistent-service
example, and exact-pin its `meerkat-core`, `meerkat-store`, and `meerkat-models`
dependencies to the same release. Compose the selected realm's config chain
explicitly; opening named persistence alone does not load its configuration.

```rust
use std::sync::Arc;
use meerkat::{
    AgentFactory, Config, CreateSessionRequest, SessionService,
    build_persistent_service, open_realm_persistence_in,
};
use meerkat_core::{
    EffectiveConfigReader, config::SystemPromptOverride, connection::RealmId,
    service::{DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions},
};
use meerkat_store::{FilesystemRealmConfigSource, RealmBackend, realm_paths_in};

let realms_root = std::env::current_dir()?.join(".rkat").join("realms");
let realm_id = RealmId::parse("team-alpha")?;
let realm = realm_paths_in(&realms_root, realm_id.as_str());
let global_doc = Config::global_config_path().ok_or("home config path unavailable")?;
let reader = EffectiveConfigReader::new(Arc::new(FilesystemRealmConfigSource::new(
    realms_root.clone(),
    global_doc,
    meerkat_models::canonical(),
)));
let mut config = reader.effective_config(&realm_id).await?;
config.validate(meerkat_models::canonical())?;
let (_manifest, persistence) = open_realm_persistence_in(
    &realms_root,
    realm_id.as_str(),
    Some(RealmBackend::Sqlite),
    None,
).await?;
let factory = AgentFactory::new(realm.root.clone())
    .runtime_root(realm.root)
    .builtins(true)
    .shell(true);
let service = build_persistent_service(factory, config, 64, persistence)?;
let result = service.create_session(CreateSessionRequest {
    model: "claude-sonnet-4-6".into(),
    prompt: "What is Rust?".into(),
    injected_context: Vec::new(),
    system_prompt: SystemPromptOverride::Inherit,
    max_tokens: None,
    event_tx: None,
    initial_turn: InitialTurnPolicy::RunImmediately,
    deferred_prompt_policy: DeferredPromptPolicy::Discard,
    build: Some(SessionBuildOptions {
        realm_id: Some(realm_id),
        ..Default::default()
    }),
    labels: None,
}).await?;
```

Initial-turn skill references/render metadata, when needed, belong in
`SessionBuildOptions.initial_turn_metadata`, not on `CreateSessionRequest`.

For detailed surface schemas and examples, also load:

- `references/api_reference.md`
- `references/mobs.md`
- `references/migration_0_9.md` (when moving a 0.8 system to 0.9, or 0.9.3 to 0.9.4)
- Sessions and storage: `references/sessions_runtime_surfaces_sessions.md`, `references/sessions_runtime_surfaces_storage_config.md`, `references/sessions_runtime_surfaces_api_changes.md`
- Providers and auth: `references/providers_and_auth_models.md`, `references/providers_and_auth_credentials.md`
- Security: `references/security.md`, `references/security_governance.md`
- MCP, tools and skills: `references/mcp_tools_skills_mcp.md`, `references/mcp_tools_skills_tools.md`, `references/mcp_tools_skills_skills.md`
- Hooks and model calls: `references/hooks_models_hooks.md`, `references/hooks_models_model_operations.md`, `references/hooks_models_turn_control.md`
- Live, comms and mobs: `references/live_comms_mobs_live.md`, `references/live_comms_mobs_comms.md`, `references/live_comms_mobs_mobs.md`

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

**Quick start - env keys**: Meerkat resolves API keys from provider-specific env vars. Anthropic: `RKAT_ANTHROPIC_API_KEY`, then `ANTHROPIC_API_KEY`. OpenAI: `RKAT_OPENAI_API_KEY`, then `OPENAI_API_KEY`. Gemini: `RKAT_GEMINI_API_KEY`, `GEMINI_API_KEY`, `RKAT_GOOGLE_API_KEY`, then `GOOGLE_API_KEY`. Azure OpenAI: `RKAT_AZURE_OPENAI_API_KEY` + `RKAT_AZURE_OPENAI_ENDPOINT`, then `AZURE_OPENAI_API_KEY` + `AZURE_OPENAI_ENDPOINT` (used only when no OpenAI key is set or an `RKAT_AZURE_OPENAI_API_KEY`/`RKAT_AZURE_OPENAI_ENDPOINT` is set). Meerkat synthesizes an ephemeral env-default binding outside the persisted realm chain; it is tried last, after configured bindings for the same provider. Keys do not choose the provider: with no model/provider/binding hint the model is OpenAI `gpt-6-astra`, so pass `-m`/`-p` or set `rkat --default-model` to run on another provider's key.

**Realm bindings — OAuth, cloud IAM, multi-tenant**: declare `[realm.<id>.{backend,auth,binding}]` in config and select a binding with CLI `--auth-binding <realm>:<binding>[:profile]` or the surface's typed `auth_binding` field. CLI `rkat auth login <provider>` provisions the reserved `global` realm in the HOME-rooted doc (`~/.rkat/config.toml`), inherited through the chain tail. Direct-provider OAuth writes `global:anthropic_oauth`, `global:openai_oauth`, or `global:google_oauth`; non-interactive api-key login stores only a token under `global:default_<provider>` (no backend, auth or binding config is written; add a binding whose auth profile has `source = { kind = "managed_store" }` and select it with `--auth-binding global:default_<provider>`). With `copilot` compiled (default), `rkat auth login copilot` uses GitHub device OAuth and provisions `global:copilot_openai`, `global:copilot_anthropic`, and `global:copilot_gemini`. All three share the `github_copilot` credential account and route through their provider-specific backends; Copilot is not a fourth LLM provider family. Credential reads inherit down the chain; a binding's owning realm is where it is DEFINED. Legacy `dev`-realm logins migrate to `global` on the run path (token + `[realm.global]` section), idempotent and no-clobber, so an existing sign-in keeps working.

```bash
rkat auth login anthropic                                     # OAuth (PKCE S256) -> global
rkat run --auth-binding global:anthropic_oauth "ship the release notes"   # uses the binding's default_model
rkat run -m claude-opus-5-5 "ship the release notes"          # an Anthropic model also resolves global:anthropic_oauth via the chain tail
# A plain `rkat run` uses the global default model gpt-6-astra (OpenAI), not this login.
```

Supported auth methods:

- API keys (env or per-binding)
- OAuth: `claude_ai_oauth` (Anthropic), `managed_chatgpt_oauth` (OpenAI), `google_oauth` (Code Assist), `github_copilot_oauth` (GitHub device flow; requires `copilot`)
- Cloud IAM: AWS Bedrock (`bedrock_aws_sigv4`), GCP Vertex (`vertex_google_auth`), Azure Foundry (`foundry_azure_ad`), Gemini ADC (`adc`, `compute_adc`). These compile only with the `meerkat-anthropic` features `bedrock`/`vertex`/`foundry` and `meerkat-gemini/adc`, which the prebuilt `rkat`, `rkat-rpc`, `rkat-rest` and `rkat-mcp` do not enable; key and bearer methods (`bedrock_bearer`, `foundry_api_key`, `api_key_express`) need no extra feature.

Tokens refresh automatically per credential identity (the binding, or a shared `credential_account` such as `github_copilot`), starting 60 seconds before expiry. `auth_binding` is persisted on the session. Switching to another model of the same provider keeps that binding; switching provider without an explicit `auth_binding` drops it, and the new provider's credential resolves through the session's realm chain (no cross-realm bleed). RPC `turn/start` can set (`{"action": "set", "value": ...}`) or clear (`{"action": "clear"}`) the binding per turn; REST `POST /sessions/{id}/messages` accepts `auth_binding` and rebuilds the agent; the SDK turn methods take none.

For stock Web SDK deployment, keep real keys in the server-side proxy.
`authBinding` plus `registerExternalAuthResolver` requires a custom/preconfigured
host with a matching external-resolver binding; see the Web SDK section above.

Surfaces: `auth/profile/{create,get,list,delete}`, `auth/login/{start,complete,cancel,device_start,device_complete,provision_api_key}`, `auth/status/get`, `auth/logout` over RPC (plus `realm/list`, `realm/get`; there is no `auth/profile/test` RPC). REST: `GET|POST /auth/profiles`, `GET|DELETE /auth/bindings/{binding_id}`, `POST /auth/bindings/{binding_id}/test`, `POST /auth/login/{start,complete,cancel}`, `POST /auth/login/device/{start,complete}`, `GET /auth/bindings/{binding_id}/status`, `POST /auth/bindings/{binding_id}/logout`, `GET /realms`, `GET /realms/{id}` (the profile list and the binding get, delete, status and logout routes take `?realm_id=`; the test route takes `realm_id` in its JSON body); REST has no `provision_api_key` route. SDK wrappers mirror the RPC methods. RPC login, cancel, status and logout also take an MCP server target (`{"mcp": {...}}`), and login and status a connector slot (`{"connector": {...}}`); REST `POST /auth/login/start|complete|cancel` take both targets too, but REST has no MCP or connector status and no MCP logout (use RPC or the SDKs: Python `auth_mcp_status` / `auth_mcp_logout` / `auth_connector_status`, TypeScript `authMcpStatus` / `authMcpLogout` / `authConnectorStatus`). Branch on the typed `error.data.reason` (`WireAuthErrorReason`; 0.9 adds `credential_access_denied`, `credential_interaction_required`, `credential_access_pending`, `credential_store_unavailable`), not the message. Connector OAuth clients are realm config managed by `rkat auth connector-client set|rotate-secret|list|remove` (secrets go to the credential store, never config). Full walkthrough: `docs/guides/auth.mdx`.

## Governed realms and review (preview)

A governed realm runs agent work under grants issued by its local operator.
It is opt-in per realm and needs the `local-authorization` feature (default in
`rkat` and `rkat-rpc`; the facade feature is off by default). Governing is
permanent (no command ungoverns a realm), works only on Unix (the operator is
the local OS user), needs an existing non-memory realm, and every other opener
(`rkat-rest`, `rkat-mcp`, ordinary Rust openers) refuses a governed realm. Governed
work can use the controller, the tool catalog, ceilinged and granted MCP
servers and host-registered tool bundle operations; builtin tools such as the
shell are refused (`operation_authorization_unavailable`). Full guide:
`references/security_governance.md`; integration contract:
`docs/guides/authorization-integration.mdx`.

```bash
rkat realm create team-alpha                                     # govern needs an existing, non-memory realm
rkat -r team-alpha mcp add github --scope realm --allow-operation search_issues --allow-operation get_issue -- /usr/local/bin/github-mcp
rkat realm govern team-alpha                                     # durable and permanent; Unix only
rkat realm grant team-alpha --catalog
rkat realm grant team-alpha --mcp github=search_issues,get_issue  # or github=*
rkat realm grant team-alpha --controller --route model-route/v1/0000000000000000000000000000000000000000000000000000000000000000
rkat --realm team-alpha run "hello"                              # refused; the error prints the grant command with the real route key
rkat realm grant team-alpha --controller --route model-route/v1/<sha256 hex>
rkat --realm team-alpha run --as-agent rkat-agent "triage today's issues"
rkat realm revoke-grant team-alpha --mcp github
```

- Each grant covers one target (an installed MCP server and its raw
  operations, the session tool catalog, or the agent's controller routes) and
  supersedes only that target's previous grant. A prompt needs a controller
  grant: without any, it is refused with no hint; under a controller grant for
  another route, the refusal names the exact `rkat realm grant ... --controller
  --route <key>` to run, so grant a placeholder key
  (`model-route/v1/` plus 64 zeros) first to learn the real one. Controller
  grants have no expiry. `--as-agent` works before or after the command name
  (0.9.0 accepted it only after).
- Review above R1 comes from the realm config's `[[governance.review.rules]]`
  (an exact MCP server, raw operation or tool bundle operation with tier `r2`
  or `r3`; `r3` also states `consent_assurance`, `device_trusted` or
  `person_authenticated`) and `[governance.review.reviewer]` (the explicit
  R2 reviewer model; there is no default). Without rules every stock operation
  stays R1.

```toml
[[governance.review.rules]]
target = { kind = "mcp_operation", server = "mail", operation = "send" }
tier = "r3"
consent_assurance = "device_trusted"     # the stdio pipe's operator; "person_authenticated" refuses it

[[governance.review.rules]]
target = { kind = "mcp_server", server = "calendar" }   # or kind = "tool_bundle_operation" with bundle + operation
tier = "r2"

[governance.review.reviewer]
identity = { model = "claude-sonnet-5-5", provider = "anthropic" }
max_output_tokens = 1024
```

- Ordinary config writes never change the `governance` section: RPC and MCP
  `config/set|patch` answer invalid params with
  `data: {"type": "governance_administration_required"}`; the realm's trusted
  host edits it in the document itself.
- R3 human consent on a stock governed `rkat-rpc` stdio pipe: the operator
  decides with `approval/consent_decide` (`approval_id` and `decision` only)
  and receives `approval/consent_notice` notifications (`requested`,
  `approved`, `denied`, `used`, `withdrawn`); SDKs deliver them through
  `on_consent_notice` / `onConsentNotice`. A decision records and notifies
  only; the retained call runs on a fresh explicit attempt.

## Feature composition

The `meerkat` facade crate defaults to the Anthropic, OpenAI, and Gemini
provider features plus the `copilot` backend/auth integration for those same
three provider identities. Realm persistence, compaction, comms, MCP, skills, semantic
memory, ATIF export, and live orchestration are compile-time options. Schedule,
WorkGraph, durable jobs, store/runtime/tools, and session substrates are always
linked; hosts still choose whether to compose and expose their services and
tools.

```toml
# Default: three providers + Copilot backend; no persistent realm/comms/MCP/skills/memory/live activation
meerkat = "=0.9.5"

# Single provider, minimal
meerkat = { version = "=0.9.5", default-features = false, features = ["anthropic"] }

# Add persistence + memory + comms + live channels
meerkat = { version = "=0.9.5", features = [
    "jsonl-store", "session-store", "session-compaction",
    "memory-store-session", "comms", "mcp", "skills",
    "openai-realtime", "live", "live-webrtc"
] }
```

Shipping facade features: `anthropic`, `openai`, `openai-realtime`, `openai-live`,
`gemini`, `copilot`, `all-providers`, `native-keyring`, `jsonl-store`,
`memory-store`, `sqlite-store`, `session-store`, `session-compaction`,
`memory-store-session`, `local-authorization`, `atif`, `comms`, `mcp`, `skills`,
`schedule`, `workgraph`, `live`, and `live-webrtc`. `local-authorization` is
the opt-in governed-realm composition (see "Governed realms and review").
`all-providers` includes
`copilot`; disable defaults for a genuinely single-provider build. `openai-live` is opt-in public GPT Live
and pulls `openai`, `live-webrtc`, `session-store`, and the provider's `live`
feature; its host/auth/execution-profile prerequisites are described above.
Keep deprecated private `experimental-gpt-live` and non-shipping
`experimental-gpt-live-gate0-harness` separate from that public feature.
`workgraph` is an empty compatibility alias. `schedule` only gates facade-local
predicate-schedule helper re-exports; it does not link the already-unconditional
schedule service.

Meerkat's pre-1.0 patch train permits declared public API breaks, so embedders
must exact-pin the crate family and bump deliberately. Prebuilt binaries
(`rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`) include their normal shipping
surfaces. The default `rkat` feature set includes `memory-store` and
`memory-store-session`; a custom reduced binary may omit them. Custom binary
builds:

```bash
cargo install rkat --version "=0.9.5" --no-default-features --features "anthropic,openai,session-store,mcp"
```

Disabled features return typed errors (e.g. `SessionError::PersistenceDisabled`) — no panics.

### CLI utility commands

- `rkat capabilities` prints pretty JSON `CapabilitiesResponse`; there is no `--json` or `--format` flag.
- `rkat doctor` has no command-specific flags. Output is tab-delimited `ok|warn <area> <message>`. Config/MCP/self-hosted hard issues exit 1; missing provider env vars and missing `wasm-pack` are warnings.

### Model catalog

The model catalog (canonical data: `meerkat-models`; `meerkat_core::model_profile` owns the vocabulary types and `ModelCatalog` mechanics) is queryable from all surfaces:

- CLI: `rkat models`
- RPC: `models/catalog`
- REST: `GET /models/catalog`
- MCP: `meerkat_models_catalog`

`rkat models` prints pretty JSON by default and has no `--json` flag. It returns the effective runtime model registry for the active realm: built-in catalog entries plus configured `[models.<id>]` custom models and `self_hosted` aliases, grouped by provider with each provider's `default_model_id` and per-model profiles.

### Model fallback chain

See `references/hooks_models_model_operations.md` (Model fallback and model switching).

Factory-built agents support runtime model fallback through `[model_fallback]`
in realm config. Defaults are OFF. `enabled = true` requires a non-empty
`[[model_fallback.chain]]`; no catalog chain is synthesized:

```toml
[model_fallback]
enabled = true

[model_fallback.policy]
cross_provider = true

[[model_fallback.chain]]
model = "claude-opus-5"
provider = "anthropic"

[[model_fallback.chain]]
model = "gpt-5.5"
provider = "openai"
auth_binding = { realm = "global", binding = "openai_oauth" }
```

Operational rules to remember:

- Explicit `enabled = false` wins over an inherited table. Writes reject
  unknown fallback keys (including `use_catalog_default_chain` and
  scope/expiry/revert) and newly introduced `enabled = true` with an empty
  chain. Persisted pre-0.8.37 files with either legacy shape still load: the
  legacy setting is ignored (the table counts as absent, so it inherits the
  parent's explicit policy, off if none), with a typed `ConfigWarning`; rkat
  prints it on stderr.
- Policy defaults: cross_provider=false, min_context_headroom=0.10,
  require_tool_parity=true, trigger_after_attempts=3,
  triggers=["capacity","provider_unavailable"].
- Fallback runs only on a failure the retry policy is about to retry, once the
  failed attempt number reaches `trigger_after_attempts` (keep it at or below
  `[retry] max_retries`), and only for trigger classes listed in
  `policy.triggers`: `capacity` (rate limits, overload) and
  `provider_unavailable` (provider 5xx) by default; `transport` (connection
  resets) and `empty_output` (incomplete responses) are opt-in. Call timeouts,
  network timeouts and stream stalls never switch models, and any streamed
  text or reasoning suppresses fallback for that failed call.
- Fallback target identity includes `auth_binding`; same model/provider with a
  different configured binding is a valid credential-failover target.
- The switch updates session LLM identity, request policy, auth lease, provider
  params and capability-based visibility in the existing canonical sticky
  transaction. Admission refuses lost output reserve or required capabilities.
- Structured-output extraction fallback must reapply the extraction schema for
  the new provider and keep provider-native web search disabled.
- The transcript receives a `model_fallback` system notice, which the model
  sees, naming the source model, fallback model, reason, skipped targets, the
  new model's limits and any tools hidden for it.
- Every candidate is checked against actual hydrated/compacted messages, tools,
  output reserve and target pressure, including on capacity failures. Forecast
  can veto optional migration, never ordinary operator-selected dispatch.
- Provider-boundary and unavailable-auth candidates produce typed
  `model_fallback_skipped` events; staging/commit have separate typed events.
  Credential-store/invariant faults are not silently treated as missing keys.
- Mob profile fallback > mob runtime fallback > host fallback. Optional typed
  carriers preserve explicit false through portable materialization/revival.
- New committed fallback provenance is exact-target-bound history in the same
  canonical transaction. Unsafe newly proven fallback-origin resumes hold for
  explicit reconfiguration; explicit identity masks obsolete the marker.
  Pre-0.8.37 notice text/brain-swap history is not authority. Historical unsafe
  sticky routes require operator migration; overlays/expiry/reversion are deferred.

### Stream-inactivity watchdog

See `references/hooks_models_model_operations.md`.

A liveness-reporting provider stream that produces no events inside the watchdog window is
aborted with the retryable `StreamStalled` failure: one stall retries, and
repeated stalls exhaust the retry budget and fail the turn typed instead of
wedging it forever. Config is the tri-state `[retry] stream_inactivity_timeout`:

```toml
[retry]
stream_inactivity_timeout = "120s"       # explicit window, every model
# stream_inactivity_timeout = "disabled" # opt out
# omitted = the model's own window, else 300s; ON by default
```

The window is resolved for each model call, so a model switch or fallback
uses the new model's window. The catalog sets one where a provider documents
long silent responses (`gemini-4-argon` 900s, `gpt-5.5-pro` 7200s); a
self-hosted model can set `stream_inactivity_timeout_secs` in
`[self_hosted.models.<id>]` (at least 60, for a long prefill, for example).
Custom `[models.<id>]` entries have no window of their own.

The client must implement `AgentLlmClient::stream_activity_count()` with a
monotonic count advanced for every raw provider event, including non-visible
events. Unlike `call_timeout` (which caps the whole call), the watchdog resets
on that activity, so long streams that keep producing are unaffected. A silent
liveness-reporting call is aborted and retried by default once its window
passes (five minutes for most models). A custom client returning the default
`None` has no inactivity watchdog; only any configured hard call/turn
deadlines remain.

### Mid-session model hot-swap

See `references/hooks_models_model_operations.md` (Model fallback and model switching).

A session's model can change between turns; the new identity is used for later turns.

- **RPC**: `turn/start` with `model`, `provider`, `self_hosted_server_id`, `provider_params` and `auth_binding` hot-swaps the live session without rebuilding the agent, on pending (deferred) and materialized sessions. `provider` needs `model`; a provider change without `auth_binding` drops the old binding; `max_tokens`, `output_schema` and `structured_output_retries` are refused (-32602).
- **REST**: `POST /sessions/{id}/messages` with `model`, `provider` or `auth_binding` rebuilds the session's agent (no `provider_params` on continue).
- **MCP**: `meerkat_resume` with `model`, `provider` or `provider_params` rebuilds the session's agent.
- **CLI**: `rkat run --resume <ID> -m <MODEL>` (optionally `-p`, `--params-json`).
- **Agent tool**: `brain_swap` stages a switch that applies before the next input, only if the run completes.
- **Rust SDK**: `Agent::hot_swap_llm_identity()` for direct library usage; `SessionService::hot_swap_session_llm_identity` is implemented by the ephemeral and persistent services (only the trait default returns `SessionError::Unsupported`). `Agent::replace_client()` and `SessionService::set_session_client` only swap in another client for the same model; a client for another provider or model is refused with `AgentError::ClientModelChangeRequiresHotSwap`.

## Core features

### Multimodal content

Prompts and tool results support multimodal content (text, images, and video). The `ContentInput` type (`Text(String)` or `Blocks(Vec<ContentBlock>)`) is used for all prompt parameters across surfaces.

**Content block types:**
- `ContentBlock::Text { text }` — plain text
- `ContentBlock::Image { media_type, data }` — base64-encoded image with `ImageData::Inline` or `ImageData::Blob`
- `ContentBlock::Video { media_type, duration_ms, data }` — base64-encoded inline video with `VideoData::Inline`

**SDK prompt types:**
- Python: `prompt: str | list[dict]` — dicts with `{"type": "text", "text": "..."}`, `{"type": "image", "media_type": "...", "data": "<base64>"}`, or `{"type": "video", "media_type": "video/mp4", "duration_ms": 12000, "data": "<base64>"}`
- TypeScript: `prompt: string | ContentBlock[]` — `{type: "text", text: "..."}`, `{type: "image", media_type: "...", data: "<base64>"}`, or `{type: "video", media_type: "video/mp4", duration_ms: 12000, data: "<base64>"}`
- Rust: `prompt: ContentInput` — `ContentInput::Text(s)` or `ContentInput::Blocks(vec![...])`; implements `From<&str>` and `From<String>`

**Video support:** Inline video is Gemini-only. Supported media types: `video/mp4`, `video/webm`, `video/quicktime`. Non-Gemini providers degrade replayed video to `[video: media_type]` text placeholders. Video in tool results is rejected at all providers. Ingress validation rejects video input for non-Gemini models at RPC, REST, and session boundaries.

**view_image builtin tool:** Reads images from disk (PNG/JPEG/GIF/WebP/SVG), returns base64 `ContentBlock::Image`. Paths are limited to the project root (a path check, not OS confinement). 5 MB limit. Hidden when the model's profile cannot take image tool results (`ModelProfile.image_tool_results`); re-evaluated when the model changes.

**generate_image builtin tool:** Session-owned assistant image generation. The model calls one stable tool with universal fields (`prompt`, `provider`, `model`, `size`, `quality`, `format`, `count`, reference/source images) plus provider-owned `provider_params`. These are Meerkat tool fields, not raw provider request JSON; for OpenAI, `format` lowers to provider-side `output_format`. Provider crates own image model profiles, advanced provider parameters, and backend selection. For normal OpenAI generate/edit requests, prefer top-level `intent` and omit hosted-tool-only `provider_params.action`. Generated images are stored in the blob store; user-facing surfaces fetch blob payload/bytes by blob id via `rkat blob get <BLOB-ID>`, JSON-RPC `blob/get`, or SDK `get_blob` / `getBlob`.

**Image generation enablement:** There is no direct image-generation CLI
command and no `rkat rpc` subcommand. In the normal composite dispatcher,
`Inherit` is visible when the session image machine, executor, planner, and blob
store are wired; default CLI and RPC runtime compositions use that path.
`--allow-tool generate_image` can narrow the visible catalog but cannot create
missing dependencies. The minimal no-builtins/no-shell path requires explicit
`Enable`, available through a Mob profile's `tools.image_generation = true` or
an in-process runtime-backed `SessionBuildOptions` override. Fetch the returned
blob with `rkat blob get <BLOB-ID>`.

**Provider capabilities:**
| Provider | `vision` | `image_tool_results` | `inline_video` |
|----------|----------|---------------------|----------------|
| Anthropic | Yes | Yes | No |
| OpenAI | Yes (model-dependent) | Model/backend-dependent: capable Responses routes support images; Chat Completions and unsupported models collapse them to text | No |
| Gemini | Yes | Yes | Yes |

### Sessions

Sessions are realm-scoped and surface-neutral. Visibility depends on `realm_id`
matching. For the lifecycle on every surface (create, resume with
`rkat run --resume`, interrupt versus `turn/stop_run`, forks, transcript edits,
archive, history, event replay, ATIF export, recovery holds and
`rkat session repair-wholeblob`), load `references/sessions_runtime_surfaces_sessions.md`.
Since 0.9.5 a run in a durable session commits every tool round and, after a
crash, continues under the same run id from its last committed round; tool
calls whose results were not committed run again at least once, keyed by a
dedupe key (same file, durable runs).
Storage locations, backends and config are in
`references/sessions_runtime_surfaces_storage_config.md`.

### Streaming

Real-time events include `text_delta`, tool lifecycle events, hook events, and terminal run events.

Event types and delivery: `references/hooks_models_turn_control.md`.

### Background work and recovery

Background shell jobs (shell tool with `background: true`), mob member terminals, and async external tool results all deliver back into the agent through a single completion stream. A shell `&` metacharacter only backgrounds work inside the foreground shell invocation; it does not create a typed Meerkat background job or return a job ID. Each typed completion appears as a typed `background_job` system notice at the next LLM turn boundary, so the agent sees and reasons over it. Idle keep-alive sessions wake automatically when a completion lands.

`shell(background: true)` runs as a realm-scoped durable detached job (the `meerkat-jobs` crate; durable (sqlite and jsonl) realms carry a per-realm `jobs.sqlite3`): progress and the terminal deliver durably back into the origin session, output is blob-spooled, and restart recovery reconciles still-running jobs instead of forgetting them. Host tooling supervises jobs over the `jobs/*` JSON-RPC family (`jobs/get`, `jobs/list`, `jobs/cancel`, `jobs/progress`, `jobs/result`, ...) and `monitors/start`. Detached execution REQUIRES a durable realm — a persistent job store, a persistent blob store, and a realm delivery projector. Where those are absent (memory-backend realms, ephemeral services, WASM, most examples) the `background` argument is not offered to the model, and a call that passes it fails closed ("detached shell execution requires a durable realm job/blob runtime; process-local background execution is not available"); there is no volatile in-process fallback.

If the runtime is backed by persistent storage, completion state and cursors survive process restarts (bounded-loss; you may see one duplicate notice on the seam). Without persistence, conversation history resumes but pending background work doesn't.

For internal seams (`CompletionFeed`, `OpsLifecycleRegistry`, `EpochCursorState`, `RuntimeEpochId`, runtime-loop feed wake) load the architecture skill.

### Skills

Skill sources, in order: embedded built-ins, `[[skills.repositories]]` from the composed realm config (`filesystem`, `git`, `http`, `stdio`), and `<context-root>/.rkat/skills` when the host has a context root. There is no user-wide skills directory. The realm runtime root only holds the Git cache and is the base for relative filesystem paths when there is no context root. A repository that fails to resolve degrades to the embedded skills.

Embedded built-ins have source UUID `00000000-0000-4b11-8111-000000000001` and
project `.rkat/skills` has `00000000-0000-4b11-8111-000000000002`, so
`rkat skill inspect task-workflow --source-uuid 00000000-0000-4b11-8111-000000000001`
works. `[skills]` keys: `enabled`, `max_injection_bytes` (default 32768),
`inventory_threshold` (default 12) and `repositories`.

Canonical skill identity is `SkillKey { source_uuid, skill_name }`. `preload_skills` carries plain `SkillKey` objects; API `skill_refs` carries tagged `SkillRef` objects (`kind: "structured"`, `source_uuid`, `skill_name`). Do not describe slash-delimited skill IDs as a public wire format.

**Skill introspection** surfaces:

- CLI: `rkat skill list [--json]`, `rkat skill inspect <skill-name> --source-uuid <uuid> [--json]`
- CLI config: `rkat skill add <PATH> [--name <NAME>]`, `rkat skill remove <NAME_OR_SOURCE_UUID_OR_PATH>`, `rkat skill get <NAME_OR_SOURCE_UUID_OR_PATH> [--json]`
- RPC: `skills/list`
- REST: `GET /skills`
- MCP: `meerkat_skills` tool (`action: "list"` or `"inspect"` with typed `skill_key` and optional `source` UUID)
- Rust SDK: `SkillRuntime::list_all_with_provenance()`, `SkillRuntime::load_from_source()`

Introspection returns typed keys plus source provenance. Shadowing is by full `SkillKey`, not by `skill_name` alone. Agent-facing skill tools (`browse_skills`, `load_skill`, resource tools, function invocation) also use `source_uuid` + `skill_name`.

`rkat run --skill <slug>` currently materializes only a bare embedded-builtin
key. It does not preload a filesystem path or a source registered by the
`rkat skill add` command; use typed RPC/SDK preload with the source UUID for
those skills.
The five agent-facing discovery/invocation tools are also default-disabled in
ordinary factory-built CLI, REST, and JSON-RPC sessions unless a custom host
explicitly enables them in `BuiltinToolConfig`.

### Hooks

See `references/hooks_models_hooks.md`.

Hook config is realm-aware, with compatibility layering from user/project hook files when available.

Mandatory model-request hooks are operation-local (0.9). A foreground
`guardrail` at `pre_llm_request` or `post_llm_response` that denies (or whose
launch confinement refuses) withholds only that model operation: the agent sends
a bounded `mandatory_hook_prerequisite_failed` notice to the run's retained
controller and the run continues. `post_llm_response` runs after transport, so
streamed text and provider-native tools that already ran are not undone. Each
such entry must state `model_operations = "ordinary_only"` (the default `all`
would also govern the controller-feedback request), and the work must carry a
retained authorized controller (work under a host work authorization with a
controller, as in governed realms); without one, run admission
rejects the run with `AgentError::HookConfigInvalid` before anything is
published. Config validation also rejects a model-request Guardrail left at
`all`, any Guardrail at `run_started` or `turn_boundary`, and
`model_operations` on a point without a model operation, so a 0.8 config with
such an entry fails to load after the upgrade. Guardrails at other points
(for example `pre_tool_execution`) are unchanged, and timeouts, handler errors
and invalid configuration still fail closed.

```toml
[[hooks.entries]]
id = "prompt-policy"
point = "pre_llm_request"
mode = "foreground"
capability = "guardrail"
model_operations = "ordinary_only"

[hooks.entries.runtime]
type = "command"
command = "python3"
args = ["hooks/prompt_policy.py"]
```

Hook payloads carry typed dispatch-time context for synchronous host classification: `HookToolCall` and `HookToolResult` include the tool's `provenance` (`ToolProvenance` — builtin/MCP-with-server-id/etc.), and `HookLlmResponse` lists provider-native `server_tool_content` kinds (e.g. web search) from the response. A foreground PreToolExecution/PostToolExecution/PostLlmResponse hook therefore sees tool-source facts before results commit — no observe-stream race.

### Injected context (host-attached ambient context)

Hosts that inject per-turn ambient context (e.g. memory recall blocks) attach it ALONGSIDE the user's message, not fused into it: the `injected_context` parameter (a list of content inputs) on RPC `turn/start` / `session/create`, REST continue/create, Python `injected_context=` / TypeScript `injectedContext:` session params, and mob work submission. Each entry becomes a separate typed transcript message ordered immediately before the user's message. Injected messages are excluded from semantic-memory indexing at compaction (no echo loop), never satisfy the compaction save-guard, and cannot be forged from message text — the role is derived from the typed delivery slot. Transcript reads expose the role; transcript rewrites may preserve `injected_context` but `compaction_summary` stays runtime-mintable only.

### Delegated work

Use mobs for all multi-agent orchestration. Mobs support `MemberLaunchMode::Fork` for seeding a member from a source member's conversation (the mob fork lane renders source history into the new member's initial prompt; CoW `Session::fork()`/`fork_at` is the library-level primitive), `spawn_helper()`/`fork_helper()` for one-call convenience (both require `result_label` and `max_text_bytes` and return a `BoundedHelperRunOutcome` with the certified bounded result), `force_cancel_member()` for cancelling in-flight turns, and `member_status()`/`wait_one()`/`wait_all()`/`collect_completed()` for monitoring.

Force-cancel semantics (0.8.51+): `force_cancel_member` (and `cancel_all_work`)
cancels the member's current run, exactly that run, at its next boundary, and
abandons input the member had admitted but not started; the member is not
retired (it stays in the roster and can take new turns). It converges
idempotently: with nothing left to cancel (an idle member, a run that already
ended, a member already retiring or no longer live) it is a no-op success,
never an `invalid state transition` error, and no cancel is left behind for a
later run. An identity the roster has never seen is refused, as
`MemberNotFound`. Since 0.9.4 a member whose runtime needs a durability reload
is refused, as `MemberReloadRequired`, until its registration is reloaded. For
an immediate, run-fenced stop use `mob/stop_member_run` (or
`mob/hard_cancel_member` for host-level immediate cancel).

Spawned/forked members can be capability-contained: pass `tool_access_policy` (`allow_list` / `deny_list` / `read_only` / `inherit`) on the spawn spec / `HelperOptions` / `AgentBuildConfig`. Since 0.8.52 (#1807) tools an allow or deny list (or a mob profile `tools.deny`) makes unreachable by name are hidden from the model's tool list as well as refused at call time; a denied call returns an ordinary `access_denied` tool error and the run continues. Conditional policy stays visible and is refused at call time, by three separate gates: `read_only` intent (decided per call on each tool's declared mutation class; `access_denied`), the application tool (consequence) policy (`policy_denied` or `policy_indeterminate`), and dispatch admission. `inherit` (and omitting the policy) composes the PARENT member's effective policy, so a restricted member cannot shed its restrictions by spawning helpers. Provider-native tools need no separate step: under any restricting `tool_access_policy`, a governing application tool policy or dispatch admission, the build turns off every provider-native tool (native web search included); an explicit web-search enable then gets the ordinary `web_search` tool.

### Inter-agent comms

Comms supports `inproc`, TCP, and UDS. Inproc registry is namespace-segmented; Meerkat uses realm namespace for isolation.

**Peer directory contract**: `comms/peers` and REST `GET /comms/peers` return
typed `PeerDirectoryEntry` objects: canonical routing `peer_id`, display-only
`name`, `address: { transport, endpoint }`, discovery `source`,
`sendable_kinds`, versioned `capabilities`, and supplementary `meta`. Send with
`peer_id`; names can collide and address strings are not routing identities.

**Peer handling_mode override**: lower-level `PeerInput` with `Message`,
`Request`, or no convention may omit the override for kind-based defaults.
Agent-facing `send_message` and `send_request` instead require and forward
`handling_mode: "queue"|"steer"`. `ResponseProgress` forbids the override;
`ResponseTerminal` supports a typed requester-reaction override or its
runtime-owned default. All paths still enforce runtime authority and response
contracts. Host overrides are available through RPC `comms/send`, REST, and
MCP `meerkat_comms_send`.

**Peer lifecycle typing**: mob lifecycle notices are typed at peer ingress. `mob.peer_added`, `mob.peer_retired`, and `mob.peer_unwired` are silent lifecycle context; the six `mob.kickoff_*` kinds (pending, starting, started, callback_pending, failed, cancelled) are visible lifecycle notices that open no inbound request and need no reply. Do not rely on mob defaults in `silent_comms_intents` for canonical behavior.

**Comms choice**: use `send_message` for ordinary collaboration, and `reply_to_peer` to answer the peer message that triggered the current turn (no `peer_id` needed). `send_request` accepts only the typed intents `checksum_token` and `supervisor.bridge` (answered with `send_response`); it is not a general ask-and-wait call, and replies always arrive as later turns. Peer-side reservation streams were removed.

**Content taint (persistent-prompt-injection defense)**: content-bearing peer envelopes (`Message`/`Request`/`Response`) can carry a signed `content_taint` declaration (`clean` | `tainted`; absent = no declaration — receivers must not treat absent as clean). Sender side: hosts set a runtime-level outbound declaration (`CommsRuntime::set_outbound_content_taint`) stamped into every outbound content envelope, or override per send with a tri-state `content_taint` field on `comms/send` params (`declare` clean/tainted, `undeclared`, or absent = inherit the runtime declaration). Receiver side: the declaration lands typed on the transcript comms notice (`sender_taint`), including for cross-process senders; the model projection marks tainted content only. Envelopes carrying a declaration fail signature verification on pre-0.7.12 receivers (loud, fail-closed under default peer auth).

Host-consumable surfaces: inbound peer content emits a typed `peer_content_ingested` agent event at delivery-commit time (queued and steered), carrying the canonical peer identity, comms kind, request id, and the sender's `sender_taint` — taint trackers consume this instead of parsing rendered projection text. Mob hosts declare a member's outbound taint by stable identity via `MobHandle::declare_member_outbound_taint(identity, taint)` (or `member(identity).declare_outbound_taint(taint)`); the declaration installs on the member's own comms runtime (external members receive it over the supervisor bridge) and resets on respawn/reset — re-declare when your tracker re-marks the fresh context.

#### Structured output with comms (autonomous agents)

See `references/hooks_models_model_operations.md`.

In `autonomous_host` mode, agents run a continuous loop: wake on inbox → process (LLM calls + tool calls including `send_message`/`send_request`/`send_response`) → produce final text output → sleep. Key architectural points:

- **`output_schema` asks for structured output next to the primary text**, not JSON-shaped primary assistant text or tool-call arguments. Every model request carries the schema in a `<structured_output>` system section. When the main loop finishes, the final reply is validated first; if it matches, no extra call is made (`extraction_succeeded.origin = "final_reply"`). Otherwise an extraction request runs with no tools, temperature 0, provider-native web search removed and the provider's native schema slot, retried up to `structured_output_retries` times (default 2).
- **Comms `send_message` tool body is required free text** (`String`), independent of typed request/response payloads. `output_schema` does not constrain that body.
- **Structured output is produced after each keep-alive processing cycle.** Each time the runtime comms drain consumes inbox work, the agent processes it, sends replies, and then produces a structured JSON summary of what it did, validated from the final reply or obtained with an extraction request. The schema comes from `output_schema` on the profile.
- **Use case**: Set `output_schema` on autonomous agent profiles to get structured turn summaries (e.g. `{headline: string, details: string}`) while letting agents communicate freely via `send_message`. The summaries power compact UI displays; the raw comms messages are available for detailed views.
- **Results and events**: Read `RunResult.structured_output` and handle
  `extraction_error` (native TypeScript: `structuredOutput` / `extractionError`).
  `RunCompleted.result` is primary text. When `extraction_required` is true,
  wait for `ExtractionSucceeded` / `ExtractionFailed` (wire
  `extraction_succeeded` / `extraction_failed`); read the success event's
  `structured_output`, not JSON parsed from `result`. Do not terminate the
  stream at main-run completion while extraction is pending.

### Tool scoping

Tool visibility can change during a session without restarting the agent. All changes are staged then atomically applied at the turn boundary.

A mob profile's `[profiles.<name>.tools]` has `deny` (hides and refuses those
names) and `read_only`, but no `allow`; the `[tools]` config section has no
`tools.allow` or `tools.deny`.

- **External filters** — stage sticky allow/deny visibility through the
  supported `ToolScopeHandle` API for turn-boundary application. Generated
  machine authority owns persistence as the typed
  `session_tool_visibility_state_v1` projection. `tool_scope_external_filter`
  is a legacy compatibility input, not a canonical field for hosts to edit;
  its fallback restore is restricted to non-runtime-owner builds.
- **Per-turn overlay** - `turn_tool_overlay {allowed_tools?, blocked_tools?, tool_choice_plan?}` on RPC `turn/start`, REST `POST /sessions/{id}/messages`, `mob/turn_start`, Python `turn_tool_overlay=` and TypeScript `turnToolOverlay` (Rust: `TurnToolOverlay` on `StartTurnRequest.runtime.turn_tool_overlay`, also used by mob flow steps). Ephemeral, not sticky session visibility. `tool_choice_plan` entries (`{"mode": "auto"|"required"|"none"}` or `{"mode": "tool", "name": ...}`) apply to the run's k-th model call.
- **Configured MCP servers** - CLI `rkat mcp add/remove/list/get` edits `.rkat/mcp.toml`, `~/.rkat/mcp.toml`, or (`--scope realm`) the realm's own `[[tools.mcp_servers]]`. New `rkat run` and `rkat run --resume` sessions load that config.
- **MCP HTTP OAuth** - streamable HTTP servers can require OAuth without any auth schema in MCP config; interactive login needs the server's account selection (`oauth_account` or `oauth_account_selection`), and credentials are per realm. Stored mode (`rkat run --mcp-auth stored`, the default) uses persisted tokens only and reports `rkat mcp login <server>` when auth is missing. Interactive mode (`--mcp-auth interactive`) may open a browser from a TTY, store tokens, reconnect, and continue the run, for any server with an account selection (`rkat mcp add --oauth-account` or `--oauth-account-selection discover|unverified`).
- **MCP access ceilings** - `access = { allowed_operations = [...] }` on a server entry (`rkat mcp add --allow-operation`) unpublishes every other raw operation; a call that still reaches one is refused as an `access_denied` tool result.
- **Live MCP mutation** — JSON-RPC `mcp/add`, `mcp/remove`, `mcp/reload`, REST `POST /sessions/{id}/mcp/*`, MCP-server tools, and SDK helpers stage server changes on the `McpRouter`. Applied at next turn boundary. Removals drain in-flight calls before finalizing.
- **Async MCP loading** - At startup, MCP servers connect in parallel in the background. The agent loop polls `poll_external_updates()` at each `CallingLlm` boundary, and servers become available as each completes its handshake. While any is connecting the model sees a pending notice (`Servers connecting: <names>. Tools will appear when ready.`).
- **Deferred tool catalog** - MCP tools and registered callback tools are deferred-eligible. While a server is connecting, or when there are 2 or more eligible tools (or their names, descriptions and schemas reach 160 characters), the session sends `tool_catalog_search` and `tool_catalog_load` instead of those schemas; the model loads tools by name and they become callable at the next boundary. A deferred tool called before loading fails `access_denied` ("Tool '<name>' is not allowed by policy").
  - Per-server timeout: `connect_timeout_secs` in `.rkat/mcp.toml` (default: 10s)
  - CLI: `--wait-for-mcp` blocks before the first turn; the CLI also waits automatically when a server may need OAuth (streamable HTTP without a static `Authorization` header). The wait is the largest `connect_timeout_secs` plus 5 s (plus 300 s with `--mcp-auth interactive`); servers still pending then abort the run (`MCP servers still pending after ...`).
  - SDK: `McpRouterAdapter::wait_until_ready(timeout)` provides the same blocking behavior
  - `AgentBuildConfig.wait_for_mcp: bool` field for programmatic surface control
- **Composition** — most-restrictive wins (allow-lists intersect, deny-lists union, deny beats allow).
- **Agent awareness** - a `tool_config_changed` event is emitted and a typed system notice ("Tool configuration changed: ...") is added to the conversation on any change. Notices are typed transcript rows; the literal `[SYSTEM NOTICE]` text prefix no longer exists.

Surface availability:

| Surface | MCP config / live mutation | Tool filter | Status |
|---------|----------------------------|-------------|--------|
| CLI | `rkat mcp add/remove/list/get` edits project/user/realm config | - | Config surface |
| JSON-RPC | `mcp/add`, `mcp/remove`, `mcp/reload` live session mutation | `session/create` `tool_filter`; per-turn `turn_tool_overlay` | Fully wired |
| REST | `POST /sessions/{id}/mcp/add|remove|reload` live session mutation | per-turn `turn_tool_overlay` on `POST /sessions/{id}/messages` | Fully wired |
| MCP server | `meerkat_mcp_add`, `meerkat_mcp_remove`, `meerkat_mcp_reload` live session mutation | — | Fully wired |

### Memory

Semantic memory (`memory_search`) and compaction integrate through the same session/runtime pipeline. Discarded transcript history becomes searchable only together with the rebuilt transcript: a durable store (the HNSW store) stages it invisibly and publishes it after the runtime commits the rewrite, and a process-local store publishes it all-or-none in the same step as the rewrite. With a store that does not support this (the `MemoryStore` default, or a durable store without a runtime), compaction fails with `memory_indexing_failed` and the original transcript is kept. Compaction summaries and host-injected context carry typed roles that exclude them from indexing (no echo loops).

Host lifecycle APIs on the `MemoryStore` trait (Rust hosts embedding the store):

- `drop_scope(&MemoryOwner)` — delete a session scope's entries (reclaim rows when a host rotates/retires session ids).
- `enumerate_scoped(scope, request)` — paged, deterministic read-back of a scope's records (`limit`/`offset` over raw rows, optional `source_overlap` message-range filter and `indexed_after` cutoff) — similarity search is no longer the only read surface.

`HnswMemoryStore` loads per-scope indexes lazily: opening the store at agent build no longer scans and re-embeds every session in the realm, so orphaned scopes cost disk only until dropped.

### Compaction curator (hosts)

Rust hosts can supply a `CompactionCurator` via `AgentBuildConfig.compaction_curator_override`: when present it produces the compaction summary content instead of the summary LLM call (zero-LLM-cost compaction for hosts that maintain incremental session notes). Curator failure fails the compaction attempt with a typed `curator_failed` reason — the session is left untouched; there is no silent fallback to the LLM path.

## Repo test lanes

For repo development, prefer the Make lanes over ad-hoc commands: `make build`, `make check`, `make lint`, `make test`, `make test-unit`, `make test-int`, `make e2e-fast`, `make e2e-system`, `make e2e-live`, `make e2e-smoke`. Cargo is default; `MEERKAT_BUILDBUDDY=1` routes the same lanes through Bazel/RBE when available. Per-package checks: `./scripts/repo-cargo {unit,int,e2e-fast,e2e-system,e2e-live,e2e-smoke}`. End-to-end lane ownership lives in `tests/integration/src/e2e_lanes.rs`.

## Reference

For complete method signatures and multi-surface examples, load:

- `references/api_reference.md`
- `references/mobs.md`
- `references/migration_0_9.md` (when moving a 0.8 system to 0.9, or 0.9.3 to 0.9.4)
- `references/migration_0_5.md` (only if migrating from 0.5)
- Sessions and storage: `references/sessions_runtime_surfaces_sessions.md`, `references/sessions_runtime_surfaces_storage_config.md`, `references/sessions_runtime_surfaces_api_changes.md`
- Providers and auth: `references/providers_and_auth_models.md`, `references/providers_and_auth_credentials.md`
- Security: `references/security.md`, `references/security_governance.md`
- MCP, tools and skills: `references/mcp_tools_skills_mcp.md`, `references/mcp_tools_skills_tools.md`, `references/mcp_tools_skills_skills.md`
- Hooks and model calls: `references/hooks_models_hooks.md`, `references/hooks_models_model_operations.md`, `references/hooks_models_turn_control.md`
- Live, comms and mobs: `references/live_comms_mobs_live.md`, `references/live_comms_mobs_comms.md`, `references/live_comms_mobs_mobs.md`
