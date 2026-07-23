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
- Different `realm_id` => strict STATE isolation (sessions, stores, event logs are realm-local).
- Backend (`sqlite` or `jsonl`) is pinned per realm in `realm_manifest.json`.

Config inheritance (state still never inherits):

- A realm config doc may declare a parent (`[realm.<id>] parent = "<other>"`). Resolution walks head → parent chain → the implicit `global` tail (the universal default head; its config is the single HOME-rooted `~/.rkat/config.toml`). No flat sibling scan, no literal `default` realm.
- CONFIG inherits down the chain: models, mcp servers, hooks, skills, limits, and auth bindings. Surfaces compose the effective config over the head realm on the agent-build path, so inheritance applies everywhere, not just to auth.
- Credential READS inherit (a child resolves a binding defined in a parent or `global`); a resolved binding's owning realm is where it is DEFINED. Credential/config WRITES are strict-owner — they land only in the realm that owns the binding, and `config get`/`set` use the raw head doc (read/write split), so an inherited entry is never flattened into a child.
- Children may add/override but cannot remove inherited mcp/hook entries (no tombstones).

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
- Without overrides, CLI uses `context_root = CWD` and `state_root = <context_root>/.rkat/realms`, creating it on first use.
- `--context-root` changes the derived `ws-...` realm id and, unless `--state-root` is supplied, the project-local realm state root.
- `--state-root` changes storage location only; it does not change the realm id.

Use explicit realm to share:

```bash
rkat --realm team-alpha run "Plan release"
rkat-rpc --realm team-alpha
rkat-rest --realm team-alpha
rkat-mcp --realm team-alpha
```

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
  and first materialization reserves the realm across all probed candidates so
  racing processes cannot mint twins.

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
resolved) over six required domains — sessions, runtime, schedule, workgraph,
blobs, artifacts. Completeness is enforced (exactly one declaration per
domain), and a durable slot resolving non-persistent without the manifest
declaring that domain ephemeral is a startup error — never a silent in-memory
fallback. Downstream backends validate against the published
`meerkat-store-conformance` harness (per-trait capability profiles; the same
suite CI runs against the in-repo stores).

Checkpoint roles (runtime-backed persistent sessions): the runtime store owns
the machine-committed checkpoint authority; the session store holds the
committed-boundary projection of the same stamped document. Resume verifies
both stamps and refuses conflicting authorities typed instead of guessing.
Sessions persisted before typed stamps existed read as `LegacyUnverified`;
`rkat storage doctor` reports the verified-vs-legacy census and
`rkat storage migrate --apply` adopts legacy checkpoints in bulk (sqlite
realms; jsonl realms heal lazily on the run path).

Operator verbs: `rkat storage doctor` (read-only, live-realm-safe, `--json`),
`rkat storage migrate` (dry-run by default, `--apply` for the fenced
migration), `rkat storage prune` (registered `*.pre-<version>-<timestamp>` /
`*.corrupt-<timestamp>` artifacts only; age from the name's embedded
timestamp). All three dispatch before runtime-scope resolution and reject
`--isolated`/`--default-model`. Exact flags: the meerkat-cli-reference skill.

## Runtime-backed vs standalone

- **Runtime-backed** (CLI, REST, `rkat-rpc`, `rkat-mcp`): keep-alive sessions, durable comms drain, completion-feed wakeups, recovery on restart. This is the default product path for daemons and long-running agents.
- **Standalone / embedded** (Web SDK / WASM, in-process Rust SDK without a runtime, tests): in-memory substrate, no keep-alive, no cross-process recovery.

The Rust SDK lets you pick: `RuntimeBuildMode::SessionOwned(bindings)` (runtime-backed) or `RuntimeBuildMode::StandaloneEphemeral` (default). Surfaces other than the Rust SDK make this choice for you.

## External mob members (advanced)

When a mob member runs in a different process or host, declare it with `MobBackendKind::External` and pass a `RuntimeBinding::External` at spawn time so the orchestrator can route comms to the real backend. The current external binding carries the advertised address, the peer's Ed25519 public identity, and a typed `bootstrap_token`; the mob derives the concrete peer id from the public key. Internal-process members use `Session` and need no extra wiring. Members are addressed everywhere by stable `AgentIdentity`; runtime/binding identity rotates underneath.

For a remote `rkat` process, use `rkat run --comms-listen-tcp <host:port> --comms-advertise-tcp <host:port> --comms-binding-out <path> --keep-alive ...`. The generated binding JSON is the safest source for mob spawn/host tooling because hand-written address-only bindings and query-string-only bootstrap tokens are rejected.

`rkat-rpc --tcp` is not this channel. It exposes JSON-RPC to hosts and SDKs; signed Meerkat peer traffic uses the `rkat run --comms-*` comms listener.

For the intentional RPC/REST/MCP/CLI/SDK exposure boundaries of managed
multi-host mobs, use the authoritative
[multi-host surface matrix](references/mobs.md#multi-host-surface-matrix).

## Surfaces

| Surface | Protocol | Use case |
|---------|----------|----------|
| `rkat` CLI | Shell commands | Developer workflows, scripting |
| REST (`rkat-rest`) | HTTP/JSON + SSE | Services and language-agnostic clients |
| JSON-RPC (`rkat-rpc`) | JSON-RPC 2.0 over stdio (default) or TCP (`--tcp <addr>`); optional live-channel WebSocket (`--live-ws`, serves `/live/ws`) | IDE integration, SDK backend, embeddable services |
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
rkat mcp add <NAME> [--transport stdio|http|sse] [--scope project|user|local] [-H KEY:VALUE...] [-e KEY=VALUE...] [--url <URL> | <URL> | -- <CMD...>]
rkat mcp login <NAME> [--scope project|user|local]
rkat mcp remove <NAME> [--scope project|user|local]
rkat mcp list [--scope project|user|local] [--json]
rkat mcp get <NAME> [--scope project|user|local] [--json]
```

Examples:

```bash
# stdio server; command and args go after --
rkat mcp add filesystem -- npx -y @modelcontextprotocol/server-filesystem .

# HTTP server
rkat mcp add linear --url https://mcp.example.com
rkat mcp add --transport http glean https://king-be.glean.com/mcp/default
rkat mcp login glean

# User-wide server with environment passed to the stdio process
rkat mcp add github --scope user -e GITHUB_TOKEN=ghp_... -- npx -y @modelcontextprotocol/server-github

rkat mcp list
rkat mcp get filesystem --scope project
```

Notes:

- Use `--transport http` for streamable HTTP and `--transport sse` for legacy SSE.
- `-H KEY:VALUE` is for HTTP/SSE headers; `-e KEY=VALUE` is for stdio process environment.
- OAuth for HTTP MCP servers is discovered at connect/login time. Keep config to name/url/transport/headers; do not add an auth schema. `rkat mcp login <name>` runs browser OAuth explicitly. `rkat run` defaults to `--mcp-auth stored`; use `--mcp-auth interactive` for first-use browser auth in a TTY run.
- `rkat mcp` is a config surface. Live mutation of a running session uses JSON-RPC `mcp/add|remove|reload`, REST `POST /sessions/{id}/mcp/*`, MCP-server tools, or SDK helpers.

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

- Default hosted text selections are OpenAI `gpt-5.6-sol` (limited preview;
  requires access for the relevant API organization or Codex workspace),
  Anthropic `claude-opus-4-8`, and Gemini `gemini-3.5-flash`.
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
- Spawn/fork tool containment: `tool_access_policy` (AllowList/DenyList/
  Inherit) on mob spawn specs and `AgentBuildConfig` is enforced at dispatch
  time without changing the model-visible tool list; denied calls surface as
  ordinary `access_denied` tool errors. `Inherit` composes transitively from
  the parent member.
- Schedules can target host-registered runnables (`TargetBinding::HostRunnable`)
  in addition to sessions, identities, and mobs.
- Comms envelopes can carry a signed sender content-taint declaration
  (`content_taint`); hosts set a runtime-level outbound declaration or a
  per-send tri-state override, and read the received declaration from the
  typed transcript comms notice.
- Semantic memory has host lifecycle APIs: per-scope delete (`drop_scope`)
  and paged enumeration (`enumerate_scoped`); the Hnsw store loads scope
  indexes lazily instead of re-embedding every session at agent build.
- CLI runs default to project-local realms unless `--realm`, `--isolated`,
  `--context-root`, or `--state-root` says otherwise.
- Storage (0.8.4): `StorageLayout` path authority + realm-id-first dual-root
  resolution with typed split-brain refusals; the `meerkat-sqlite` mechanics
  crate (profiles, `meerkat_schema` ledger, `.mfence` fences); the
  `RealmStorageProvider` seam with manifest v2 and fail-closed durability
  declarations; `rkat storage doctor|migrate|prune` offline verbs; the
  published `meerkat-store-conformance` harness. See the Storage architecture
  section above.
- The stream-inactivity watchdog (`[retry] stream_inactivity_timeout`) is ON
  by default at 300s; `"disabled"` opts out. Stalls surface as the retryable
  `StreamStalled` failure.
- `rkat run --output html` / `--html` writes standalone HTML artifacts.
- Image generation uses `generate_image` with OpenAI `gpt-image-2` and Gemini
  `gemini-3.1-flash-image-preview` as provider defaults.
- Mob host surfaces include batch wiring, helper/fork/respawn/cancel/wait
  operations, profile CRUD when a profile store is present, and safer
  runtime-committed session checkpoint projections for SDK consumers.

## Mob behavior (current contract)

- CLI `run`/`run --resume` compose `mob_*` tools through `meerkat-mob-mcp` dispatcher integration when mob tools are enabled, for example with `--tools full` or config `tools.mob_enabled=true`.
- CLI `mob ...` is the explicit lifecycle surface for persisted mob registry operations.
- RPC/REST/MCP server/Python SDK/TypeScript SDK expose mob capability through typed `mob/*` / `meerkat_mob_*` host control planes. Agent-internal `mob_*` tools are late-bound through `SessionBuildOptions.mob_tools` (`MobToolsFactory`); `external_tools` remains for callback/MCP-backed tools.
- Member runtime default is `autonomous_host` when `runtime_mode` is omitted; `turn_driven` is explicit opt-in for controlled dispatch paths.
- Spawned mob members use deferred initial turn semantics; mob actor lifecycle starts autonomous loops explicitly after spawn registration.
- Mob persistence is SQLite/WAL-backed (`MobStorage::persistent()` opens `SqliteMobStores`). In-memory storage is used for tests and WASM. The previous exclusive-handle mob store has been removed.
- Prefabs are gone. All mob creation uses `MobDefinition` only (CLI, REST, RPC, MCP, SDKs).
- Agent-facing delegation tools (`delegate`, `mob_create`, `mob_destroy`, `mob_spawn_member`, `mob_retire_member`, `mob_check_member`, `mob_list_members`, `mob_list`, `mob_wire`, `mob_unwire`) are provided by `AgentMobToolSurface` in `meerkat-mob-mcp`. When a realm profile store is present, `mob_profile_create`, `mob_profile_get`, `mob_profile_list`, `mob_profile_update`, `mob_profile_delete`, and `mob_profile_list_sources` are also surfaced. These tools let agents spawn and manage mob members through implicit session-owned mobs, create/remove peer-to-peer comms links, and manage reusable realm profiles when authorized.
- Portable mob artifacts are available through mobpack (`rkat mob pack/inspect/validate/run/deploy`) and browser deployment (`rkat mob web build`).
- Live (audio/video) channels are exposed through the caller-initiated `live/*` surface, not the previous attachment-status family. Capability detection still uses `ModelCapabilities.realtime` to decide whether a model can back a live channel; channel lifecycle is caller-initiated through the `live/*` JSON-RPC methods (and SDK equivalents) below. The previous `session/realtime_attachment_status`, `mob/member_status.realtime_attachment_status`, `realtime/open_info`, and `RealtimeAttachmentStatus` enum have been removed.
### Live channels (audio/video)

Live is the single subsystem for audio and other realtime modalities. Pick a realtime-capable model (today the only catalog realtime row is `gpt-realtime-2`; older realtime catalog rows are retired) and open a channel explicitly. `live/open` returns a typed `LiveOpenResult` with the negotiated transport bootstrap (e.g. WebSocket URL for `rkat-rpc`'s `--live-ws` listener at `/live/ws`) and a `WireLiveChannelCapabilities` advert describing supported input/output modalities, continuity mode, and tool semantics. Its optional positive `seed_max_chars` parameter requests a core-owned whole-turn seed suffix; omission preserves the full canonical seed.

| Surface | Open channel | Observe / control |
|---------|--------------|-------------------|
| JSON-RPC | `live/open` (returns `LiveOpenResult` with transport bootstrap + capabilities) | `live/status`, `live/refresh`, `live/send_input`, `live/commit_input`, `live/interrupt`, `live/truncate`, `live/close` |
| Python SDK | `client.live_open(session_id, seed_max_chars=...)` | `client.live_status / live_refresh / live_send_input_text / live_send_input_audio / live_send_input_image / live_send_input_video_frame / live_commit_input / live_interrupt / live_truncate / live_close` |
| TypeScript SDK | `client.liveOpen({ session_id: sessionId, seed_max_chars: ... })` or `LiveChannel.session(..., { seedMaxChars: ... })` | `client.liveStatus / liveRefresh / liveSendInput / liveSendInputImage / liveSendInputVideoFrame / liveCommitInput / liveInterrupt / liveTruncate / liveClose` |
| Rust | `meerkat_live::LiveAdapterHost` + `meerkat_core::live_adapter` adapter trait, exercised through `SessionRuntime` | typed observations and capability reads through the live host |

Seed-window contract: `seed_max_chars` must be positive; zero is rejected by
server validation. Core counts serialized seed messages and selects complete
recent turns rather than slicing a turn. The resolved root prompt is never
dropped and must fit the window; an existing compaction-summary head may be
retained. Any truncation must return degraded continuity. Runtime system
context and full canonical image identity, tombstone, and accounting sidecars
remain outside the message window, so this is not a hard cap on the provider's
entire instruction payload.

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

The `--live-ws` listener (`rkat-rpc --live-ws <addr>`) must be enabled for clients that need WebSocket bootstrap. A WebRTC-only deployment can register the `live/*` JSON-RPC methods through its WebRTC transport configuration instead; the methods are absent only when no live transport is configured. Each session keeps a single canonical history; audio commits at turn boundaries via `live/commit_input` / `live/interrupt` / `live/truncate`.

Practical caveats:

- One live channel per session at a time; for per-member live channels in mobs, open the channel against a member that runs on a realtime-capable profile.
- Idle sessions can't host a channel — start a turn or spawn the member first.
- Provider-native web search and tool-calling capability is per-model; check `ModelProfile` flat fields like `supports_web_search` if a tool unexpectedly disappears under a realtime model.

Wire types: see `LiveOpenParams`, `LiveOpenResult`, `WireLiveChannelCapabilities`, `LiveStatusResult`, `LiveSendInputParams`, `LiveTruncateParams`, `WireLiveAdapterStatus`, `WireLiveAdapterErrorCode`, and `WireLiveAdapterObservation` in `meerkat-contracts/src/wire/live.rs`.

### Mob lifecycle (standard/default usage)

Primary CLI usage is tool-driven through `run`/`run --resume` with mob tools enabled:

```bash
rkat run --tools full "create a mob with a lead and workers, then wire and report status"
rkat run --tools full --resume <session_id> "retire worker-2 and add worker-4"
```

Where needed, the current helper-oriented CLI mob surface is:

```bash
rkat mob spawn-helper <mob_id> <prompt> --agent-identity <id> [--profile <profile>] [--json]
rkat mob fork-helper <mob_id> <source_member> <prompt> --agent-identity <id> [--profile <profile>] [--fork-context full-history|last-messages] [--last-messages N] [--json]
rkat mob member-status <mob_id> <agent_identity> [--json]
rkat mob force-cancel <mob_id> <agent_identity>
rkat mob respawn <mob_id> <agent_identity> [--initial-message <msg>]
rkat mob run <pack_or_mob_id> [--flow <flow_id>] [--param key=value ...] [--prompt <text>] [--detach] [--json]
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
rkat mob runs <mob_id> [--flow <flow_id>] [--json]
rkat mob status <mob_id> <run_id> [--json]
rkat mob attach <mob_id> <run_id> [--json]
```

Flow model: declarative DAG (`depends_on`, `depends_on_mode = all|any`), dispatch modes (`one_to_one`, `fan_out`, `fan_in`), optional `branch` + `condition`, topology rules (`strict|permissive`, `"*"` wildcard), persisted `MobRun` snapshots with `step_ledger`/`failure_ledger` and typed output envelopes. Frame-based v2 flows add nested `FlowSpec.root: FrameSpec` and `repeat_until` loop nodes (`until`, `max_iterations`, nested `body`). `run` invokes a mobpack or installed mob as a typed callable; `--prompt` binds `params.prompt`; `--detach` returns a run id; `runs`/`status`/`logs`/`attach` operate on the same run resources. Per-flow limits live under mob `limits` (`max_flow_duration_ms`, `max_step_retries`, `cancel_grace_timeout_ms`, `max_orphaned_turns`).

Don't conflate **mob tool availability** (surface behavior — `mob_*` tools and `rkat mob` lifecycle) with **realm backend** (`sqlite`/`jsonl` in `realm_manifest.json`).

## Scheduling

The `meerkat-schedule` crate provides durable, authority-backed scheduling for automated agent task execution.

### Schedule model

A `Schedule` defines when and how agent sessions are materialized:
- **Trigger**: `Cron(CalendarTriggerSpec)` or `Interval(IntervalTriggerSpec)`
- **Target**: `Session(SessionTargetBinding)`, `Identity(IdentityTargetBinding)` (durable mob-member identity), `Mob(MobTargetBinding)`, or `HostRunnable(HostRunnableTargetBinding)` (host-registered callback)
- **Policies**: `MisfirePolicy` (Execute/Skip/SkipIfOlderThan), `OverlapPolicy` (Allow/Skip/Queue), `MissingTargetPolicy` (Skip/Fail/Create)
- **Phase**: `Active`, `Paused`, `Completed`, `Cancelled`

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
    target: TargetBinding::session(SessionTargetBinding::materialize_on_demand(
        SessionMaterializationSpec {
            model: "claude-sonnet-4-6".into(),
            ..Default::default()
        },
        ScheduledSessionAction::Prompt {
            prompt: ContentInput::Text("Generate hourly report".into()),
            system_prompt: None,
            render_metadata: None,
            skill_refs: Vec::new(),
            additional_instructions: Vec::new(),
        },
    )),
    ..Default::default()
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

**Quick start — env keys**: Meerkat resolves API keys from provider-specific env vars. Anthropic: `RKAT_ANTHROPIC_API_KEY`, then `ANTHROPIC_API_KEY`. OpenAI: `RKAT_OPENAI_API_KEY`, then `OPENAI_API_KEY`. Gemini: `RKAT_GEMINI_API_KEY`, `GEMINI_API_KEY`, `RKAT_GOOGLE_API_KEY`, then `GOOGLE_API_KEY`. Meerkat synthesizes an ephemeral env-default binding (resolved via the global chain head). No config edits.

**Realm bindings — OAuth, cloud IAM, multi-tenant**: declare `[realm.<id>.{backend,auth,binding}]` in config and pass `--auth-binding <realm>:<binding>[:profile]` on `rkat run` / `session/create` / `mob_spawn_member` to scope a session or mob member to that binding. CLI `rkat auth login <provider>` provisions the reserved `global` realm in the HOME-rooted doc (`~/.rkat/config.toml`), so one sign-in is inherited by every workspace realm via the chain tail: interactive OAuth writes `global:anthropic_oauth`, `global:openai_oauth`, or `global:google_oauth`; non-interactive api-key login writes `global:default_<provider>`. Credential reads inherit down the chain; a binding's owning realm is where it is DEFINED. Legacy `dev`-realm logins migrate to `global` on the run path (token + `[realm.global]` section), idempotent and no-clobber, so an existing sign-in keeps working.

```bash
rkat auth login anthropic                                            # OAuth (PKCE S256) → global
rkat run "ship the release notes"                                   # inherits global:anthropic_oauth via the chain tail
rkat run --auth-binding global:anthropic_oauth "ship the release notes"  # or scope explicitly
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
meerkat = "0.8"  # track the latest 0.8.x release

# Single provider, minimal
meerkat = { version = "0.8", default-features = false, features = ["anthropic"] }

# Add persistence + memory + comms + live channels
meerkat = { version = "0.8", features = [
    "jsonl-store", "session-store", "session-compaction",
    "memory-store-session", "comms", "mcp", "skills",
    "openai-realtime", "live"
] }
```

Available facade features: `anthropic`, `openai`, `openai-realtime`, `gemini`, `all-providers`, `jsonl-store`, `memory-store`, `sqlite-store`, `session-store`, `session-compaction`, `memory-store-session`, `comms`, `mcp`, `skills`, `schedule`, `workgraph`, `live`.

Prebuilt binaries (`rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`) include the normal shipping surfaces. The default `rkat` feature set does not include `memory-store`; memory capabilities appear only in binaries built with the memory-store feature. Custom binary builds:

```bash
cargo install rkat --version "0.8" --no-default-features --features "anthropic,openai,session-store,mcp"
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

`rkat models` prints pretty JSON by default and has no `--json` flag. It returns the effective runtime model registry for the active realm: built-in catalog entries plus configured `self_hosted` aliases, providers, default models, and per-model profiles.

### Model fallback chain

Factory-built agents support runtime model fallback through `[model_fallback]`
in realm config. Defaults are enabled with an empty `chain`, which means
"build the catalog-owned default backup order" from provider defaults plus the
global catalog default. A non-empty `[[model_fallback.chain]]` is a user-owned
ordered policy:

```toml
[model_fallback]
enabled = true

[[model_fallback.chain]]
model = "claude-opus-4-8"
provider = "anthropic"

[[model_fallback.chain]]
model = "gpt-5.5"
provider = "openai"
auth_binding = { realm = "global", binding = "openai_oauth" }
```

Operational rules to remember:

- `enabled = false` disables failover; `use_catalog_default_chain = true`
  restores the built-in chain over an inherited disabled/custom policy.
- Fallback is only for machine-classified recoverable LLM failures and core
  pre-stream-safe retries. Network/call timeouts do not trigger model fallback,
  and any user-visible text/reasoning stream output suppresses cross-model
  fallback for that failed call.
- Fallback target identity includes `auth_binding`; same model/provider with a
  different configured binding is a valid credential-failover target.
- The switch updates session LLM identity, request policy, auth lease, provider
  params, max-output clamp, and capability-based tool filtering before retry.
- Structured-output extraction fallback must reapply the extraction schema for
  the new provider and keep provider-native web search disabled.
- The active agent receives a hidden `model_fallback` system notice explaining
  the source model, fallback model, reason, skipped targets, limits, and hidden
  tools.
- A smaller-context backup target is skipped for a context-overflow failure
  when its catalog context window cannot satisfy the requested size.
- Catalog-default candidates stay scoped to the selected non-env auth realm's
  parent chain when a chain member has a provider binding. The model-swap filter
  accepts inherited candidates (binding owned by an ancestor or `global`), so a
  hot swap does not drop inherited bindings; each keeps its owning-realm
  provenance. Explicit chains may set `auth_binding`; missing explicit targets
  fail configuration instead of being silently skipped.

### Stream-inactivity watchdog

A provider stream that produces no events inside the watchdog window is
aborted with the retryable `StreamStalled` failure: one stall retries, and
repeated stalls exhaust the retry budget and fail the turn typed instead of
wedging it forever. Config is the tri-state `[retry] stream_inactivity_timeout`:

```toml
[retry]
stream_inactivity_timeout = "120s"       # explicit window
# stream_inactivity_timeout = "disabled" # opt out
# omitted = the built-in default window (300s) — ON by default
```

Unlike `call_timeout` (which caps the whole call), the watchdog resets on
every stream event, so long streams that keep producing are unaffected. A call
that previously sat silent for >5 minutes and eventually completed is now
aborted and retried.

### Mid-session model hot-swap

Model and provider can be changed on a running session without rebuilding the agent:

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

**generate_image builtin tool:** Session-owned assistant image generation. The model calls one stable tool with universal fields (`prompt`, `provider`, `model`, `size`, `quality`, `format`, `count`, reference/source images) plus provider-owned `provider_params`. These are Meerkat tool fields, not raw provider request JSON; for OpenAI, `format` lowers to provider-side `output_format`. Provider crates own image model profiles, advanced provider parameters, and backend selection. For normal OpenAI generate/edit requests, prefer top-level `intent` and omit hosted-tool-only `provider_params.action`. Generated images are stored in the blob store; user-facing surfaces fetch blob payload/bytes by blob id via `rkat blob get <BLOB-ID>`, JSON-RPC `blob/get`, or SDK `get_blob` / `getBlob`.

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

Background shell jobs (shell tool with `background: true`), mob member terminals, and async external tool results all deliver back into the agent through a single completion stream. A shell `&` metacharacter only backgrounds work inside the foreground shell invocation; it does not create a typed Meerkat background job or return a job ID. Each typed completion appears as a `[SYSTEM NOTICE][BG_JOB]` (or equivalent) message at the next LLM turn boundary, so the agent sees and reasons over it. Idle keep-alive sessions wake automatically when a completion lands. Current shell background jobs are process-local and volatile: they do not survive a Meerkat process restart, even though already-persisted terminal completion notices can be recovered.

If the runtime is backed by persistent storage, completion state and cursors survive process restarts (bounded-loss; you may see one duplicate notice on the seam). Without persistence, conversation history resumes but pending background work doesn't.

For internal seams (`CompletionFeed`, `OpsLifecycleRegistry`, `EpochCursorState`, `RuntimeEpochId`, runtime-loop feed wake) load the architecture skill.

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

Hook payloads carry typed dispatch-time context for synchronous host classification: `HookToolCall` and `HookToolResult` include the tool's `provenance` (`ToolProvenance` — builtin/MCP-with-server-id/etc.), and `HookLlmResponse` lists provider-native `server_tool_content` kinds (e.g. web search) from the response. A foreground PreToolExecution/PostToolExecution/PostLlmResponse hook therefore sees tool-source facts before results commit — no observe-stream race.

### Injected context (host-attached ambient context)

Hosts that inject per-turn ambient context (e.g. memory recall blocks) attach it ALONGSIDE the user's message, not fused into it: the `injected_context` parameter (a list of content inputs) on RPC `turn/start` / `session/create`, REST continue/create, Python `injected_context=` / TypeScript `injectedContext:` session params, and mob work submission. Each entry becomes a separate typed transcript message ordered immediately before the user's message. Injected messages are excluded from semantic-memory indexing at compaction (no echo loop), never satisfy the compaction save-guard, and cannot be forged from message text — the role is derived from the typed delivery slot. Transcript reads expose the role; transcript rewrites may preserve `injected_context` but `compaction_summary` stays runtime-mintable only.

### Delegated work

Use mobs for all multi-agent orchestration. Mobs support `MemberLaunchMode::Fork` for seeding a member from a source member's conversation (the mob fork lane renders source history into the new member's initial prompt; CoW `Session::fork()`/`fork_at` is the library-level primitive), `spawn_helper()`/`fork_helper()` for one-call convenience, `force_cancel_member()` for cancelling in-flight turns, and `member_status()`/`wait_one()`/`wait_all()`/`collect_completed()` for monitoring.

Force-cancel semantics (0.8.4+): force-cancelling a running member is legal
from `Running` — it interrupts the in-flight work without retiring the member
(the member stays in the roster and can take new turns). It converges
idempotently: a repeat cancel while the member is already retiring, or a
cancel when the member's runtime is no longer live, is a no-op success, never
an `invalid state transition` error. Only an identity the roster has never
seen is refused, as `MemberNotFound`.

Spawned/forked members can be capability-contained without changing their model-visible tool list: pass `tool_access_policy` (`allow_list` / `deny_list` / `inherit`) on the spawn spec / `HelperOptions` / `AgentBuildConfig`. Enforcement is call-level — a denied call returns an ordinary `access_denied` tool error and the run continues. `inherit` (and omitting the policy) composes the PARENT member's effective policy, so a restricted member cannot shed its restrictions by spawning helpers. Provider-native server tools (e.g. native web search) bypass the dispatcher — disable that capability on gated builds if it matters.

### Inter-agent comms

Comms supports `inproc`, TCP, and UDS. Inproc registry is namespace-segmented; Meerkat uses realm namespace for isolation.

**Peer directory contract**: `comms/peers` and REST `GET /comms/peers` return
typed `PeerDirectoryEntry` objects: canonical routing `peer_id`, display-only
`name`, `address: { transport, endpoint }`, discovery `source`,
`sendable_kinds`, versioned `capabilities`, and supplementary `meta`. Send with
`peer_id`; names can collide and address strings are not routing identities.

**Peer handling_mode override**: `PeerInput` with `Message`, `Request`, or no convention supports an explicit `handling_mode` field (`Queue` or `Steer`) that overrides kind-based policy defaults. `ResponseProgress` and `ResponseTerminal` reject the field at runtime admission. Built-in comms bridges leave it `None` (kind-based policy). The override is available on RPC `comms/send`, REST, and MCP `meerkat_comms_send` surfaces.

**Peer lifecycle typing**: mob lifecycle notices are typed at peer ingress. `mob.peer_added`, `mob.peer_retired`, and `mob.peer_unwired` are silent lifecycle context; `mob.kickoff_failed` and `mob.kickoff_cancelled` are visible lifecycle notices. Do not rely on mob defaults in `silent_comms_intents` for canonical behavior.

**Comms choice**: use `send_message` for ordinary collaboration. Use `send_request` only for structured ask/reply semantics (`intent + params` plus later `send_response`). Peer-side reservation streams were removed.

**Content taint (persistent-prompt-injection defense)**: content-bearing peer envelopes (`Message`/`Request`/`Response`) can carry a signed `content_taint` declaration (`clean` | `tainted`; absent = no declaration — receivers must not treat absent as clean). Sender side: hosts set a runtime-level outbound declaration (`CommsRuntime::set_outbound_content_taint`) stamped into every outbound content envelope, or override per send with a tri-state `content_taint` field on `comms/send` params (`declare` clean/tainted, `undeclared`, or absent = inherit the runtime declaration). Receiver side: the declaration lands typed on the transcript comms notice (`sender_taint`), including for cross-process senders; the model projection marks tainted content only. Envelopes carrying a declaration fail signature verification on pre-0.7.12 receivers (loud, fail-closed under default peer auth).

Host-consumable surfaces: inbound peer content emits a typed `peer_content_ingested` agent event at delivery-commit time (queued and steered), carrying the canonical peer identity, comms kind, request id, and the sender's `sender_taint` — taint trackers consume this instead of parsing rendered projection text. Mob hosts declare a member's outbound taint by stable identity via `MobHandle::declare_member_outbound_taint(identity, taint)` (or `member(identity).declare_outbound_taint(taint)`); the declaration installs on the member's own comms runtime (external members receive it over the supervisor bridge) and resets on respawn/reset — re-declare when your tracker re-marks the fresh context.

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
- **Per-turn overlay** — `TurnToolOverlay` on `StartTurnRequest.turn_tool_overlay`. Ephemeral, used by mob flow steps and attention producers to restrict tools per turn.
- **Configured MCP servers** — CLI `rkat mcp add/remove/list/get` edits `.rkat/mcp.toml` or `~/.rkat/mcp.toml`. New `rkat run` and `rkat run --resume` sessions load that config.
- **MCP HTTP OAuth** — streamable HTTP servers can require OAuth without any auth schema in MCP config. Stored mode (`rkat run --mcp-auth stored`, the default) uses persisted tokens only and reports `rkat mcp login <server>` when auth is missing. Interactive mode (`--mcp-auth interactive`) may open a browser from a TTY, store tokens, reconnect, and continue the run.
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

Semantic memory (`memory_search`) and compaction integrate through the same session/runtime pipeline. Compaction indexes discarded transcript history into the session's memory scope before committing the rebuilt transcript; compaction summaries and host-injected context carry typed roles that exclude them from indexing (no echo loops).

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
- `references/migration_0_5.md` (only if migrating from 0.5)
