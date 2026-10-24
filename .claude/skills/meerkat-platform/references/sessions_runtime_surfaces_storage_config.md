Covers: Meerkat 0.9 state and configuration: realm ids and where state lives
(project-local and user-global roots, fallback realm), backends, JSONL index
degradation, multiprocess hosting claims and cross-process wakes (store
watch sweep, notification limits), SQLite WAL and error codes, forward-only
versions and upgrade notes, `rkat storage doctor|migrate|prune`; runtime
behaviour, health, shutdown and job delivery retries; WorkGraph limits, join policies and
store errors; schedules; `rkat config` get/set/patch, precedence and keys.

# Storage, Runtime, WorkGraph, Schedules and Config (Meerkat 0.9)

Names are exact and checked against the 0.9.0 source. Session operations:
`references/sessions_runtime_surfaces_sessions.md`. API changes, error codes, SDKs,
prebuilt binaries and Rust embedding:
`references/sessions_runtime_surfaces_api_changes.md`.

## Where state lives

Realm ids: 1 to 64 characters, first character ASCII letter or digit, then
letters, digits, `-` or `_`; no `:`, no whitespace, and UUID-shaped ids are
reserved for sessions. Generated ids are `realm-<uuid>` (`--isolated` and
servers without `--realm`); the CLI default is the workspace realm
`ws-<hash of the context root>`.

State root resolution is realm-id first (0.8.4):

1. An explicit `--state-root` wins with no probing.
2. Otherwise the realm is used where it already exists: under the project-local
   `<project-root>/.rkat/realms` (the nearest ancestor containing `.rkat`) or
   the user-global root (`<data dir>/meerkat/realms`: Linux
   `~/.local/share/meerkat/realms`, macOS
   `~/Library/Application Support/meerkat/realms`, Windows
   `%APPDATA%\meerkat\realms`).
3. A realm under both is a typed split-brain refusal; fix it with
   `rkat storage migrate --apply --adopt-root <PATH>`.
4. A new realm goes to the surface default: project-local for the CLI,
   user-global for `rkat-rpc`, `rkat-rest` and `rkat-mcp` (servers probe the
   project-local root only when started with `--context-root`).

If the workspace realm's storage cannot open, a fresh prompt-first `rkat run`
(or `rkat help`) uses a newly generated durable SQLite realm under the same
state root (0.8.15). Workspace config, auth and tool policy still apply, no
history from the failed realm is loaded, and stderr warnings name the
generated realm and the recovery command (`rkat --state-root <root> --realm
<realm> storage migrate --apply --bridge-pre-0-8-10`). Explicit `--realm`,
`--isolated`, resumes, session commands and governed realms never fall back.

Typical realm directory (`<state-root>/<realm>/`):

| Path | Holds |
|---|---|
| `realm_manifest.json` | Pinned backend and provider (manifest format 2) |
| `config.toml`, `config_state.json` | Raw realm config and its CAS generation |
| `sessions.sqlite3` | SQLite backend: sessions, schedules and the HeadCanonical runtime store |
| `sessions_jsonl/`, `runtime.sqlite3` | JSONL backend: session files and the WholeBlob runtime store |
| `jobs.sqlite3`, `workgraph.sqlite3`, `memory/memory.sqlite3` | Detached jobs, WorkGraph, semantic memory |
| `blobs/`, `artifacts/` | Blob bytes (generated images, spooled output) and artifacts |
| `trajectories/` | ATIF exports from the CLI |
| `hosting/`, `delivery/` | Multi-process hosting claims and delivery coordination |
| `mobs/` | Per-mob databases |

Every SQLite file has a `meerkat_schema` ledger table and a sibling
`<file>.mfence` fence-lock file; since 0.9.4 also a `<file>.mpresence`
presence-lock file, which maintenance creates next to every database it fences
and a kept connection holds shared while it is open. The `global` realm's
config is `~/.rkat/config.toml`.

Backends (pinned at first creation; `--realm-backend` is only a creation
hint):

| Backend | Use |
|---|---|
| `sqlite` | Default for new persistent realms; the normal same-realm multi-process backend |
| `jsonl` | Inspectable session files; schedules are disabled on this backend |
| `memory` | Explicitly ephemeral; nothing survives the process |

Since 0.9.5 every tool round of a durable run commits twice (durable runs in
`references/sessions_runtime_surfaces_sessions.md`). On `sqlite` a commit
writes the round's rows; on `jsonl` (WholeBlob) it writes the whole session
document, so long, tool-heavy runs belong on `sqlite`.

Host stores (0.9.5): a Rust host can keep a realm's durable state in its own
database by implementing `meerkat_core::host_store::HostStore` (`acquire_fence`,
`renew_fence`, `fence_timing`, `load_head`, `list_heads`, `read_rows`,
`commit`). Core builds the realm's stores over it and stays its only writer.
Contract, obligations and conformance: `docs/reference/host-store-spi.mdx`.

- Fence: opening acquires it exclusively, and a live holder makes the open
  fail with `Held { holder, remaining }`. A takeover supersedes the holder,
  then waits out what remained of its lease before serving anything. Core
  renews every renew interval (default 10 s, at least 1 s) under a lease
  (default 30 s, from 3 intervals to 1 hour) that the store sizes with
  `fence_timing()`. It makes no model call, dispatches nothing and writes
  nothing unless its last successful acquire or renew started less than the
  lease minus the self-fence margin ago. The margin defaults to one interval;
  a store declares its own with `FenceTiming::try_with_margin`, at least
  `renew interval + 2 x visibility lag`, and the window must stay at least
  two intervals. A store also declares its longest commit
  (`try_with_max_commit_latency`, default 2 s, at most the lease): a fenced
  write that meets a commit in flight waits up to that long before it backs
  off. `FencedOut` stops every write until the realm is reopened;
  after any other failed renew, or a lease that ran out, the holder writes
  and dispatches nothing until it re-acquires the fence, which it does by
  itself before its next write (a later renew never restores it); if
  another process took the fence or committed meanwhile, the realm stops
  until it is reopened.
- Opening lists every head once. A session body loads at open only when
  recovery must act on it (unsettled inputs, an open run cursor, a current
  run), otherwise on first use, and stays loaded until the session is
  archived.
- Limits: 8 MiB of heads and rows per batch, 1 MiB per row. Core writes
  large values first as digest-keyed chunk scopes, then one commit batch that
  refers to them; a change that cannot fit fails whole with a typed size
  error, never truncated or partly committed. An unknown commit outcome is
  settled before the
  scope's next version. A refused batch, or one still unknown after the retry
  budget, poisons the scopes it touched until the realm is reopened.
- The runtime store is WholeBlob, with the cost above; the run's next round
  commit after a compaction re-bases the run rather than stopping its
  commits. An interactive OAuth login in
  progress is process-local: after a restart or a takeover, start it again;
  completed credentials are kept.
- Test a store with the host store chapter of `meerkat-store-conformance`.

On a JSONL realm, if the session index (`sessions_jsonl/session_index.sqlite3`)
fails to update after a committed save or delete, the write stays committed
but session listing refuses with `refusing session.list: projection
'jsonl-session-index' is known degraded relative to canonical authority
'jsonl-session-file' (...)` (`ProjectionReadRefused`) until the index checks
out against the `.jsonl` files again; `rkat storage doctor` checks it.

Each realm's store set declares durability for seven domains: `sessions`,
`runtime`, `schedule`, `workgraph`, `jobs`, `blobs`, `artifacts`. A durable
domain that resolves to non-persistent storage without the manifest declaring
it ephemeral is a startup error, never a silent in-memory fallback.

Multi-process hosting (0.8.52):

```toml
[storage]
hosting = "multiprocess"   # default "single_process"
```

Several processes may then serve one durable realm; each session is hosted by
exactly one of them through an OS-locked claim. Another process touching that
session gets `SESSION_BUSY` with `data.kind = "session_served_elsewhere"`; an
unavailable claim is `SESSION_RUNTIME_UNAVAILABLE` with
`data.kind = "session_hosting_unavailable"`. `multiprocess` refuses to start
on stores that cannot provide trusted cross-process hosting (memory or
external stores, network or userspace filesystems). The setting layers like
any realm option (a child's explicit value wins). Only the process hosting a
session applies that session's job deliveries. A claim is a file lock the
kernel releases when the hosting process exits; there is no lease, expiry or
heartbeat.

A process learns of rows another process committed from a store watch: a
bounded sweep (every 5 seconds by default) that file notifications only
shorten. Since 0.9.4:

- `multiprocess` no longer refuses to start when the runtime store's file
  notifications cannot be registered (at an inotify watch or instance limit,
  for example); the sweep is the cross-process wake. It still refuses when the
  sweep cannot start.
- A watch whose registration fails keeps its sweep (0.9.3 and earlier did not
  start the watch, so a mob event observer had no cross-process catch-up).
  It logs "sqlite change watch notifications are unavailable; the watch runs
  on its sweep alone" with the reason; wakes then take up to one sweep. To get
  notifications back, raise the limit (Linux
  `fs.inotify.max_user_watches`, `fs.inotify.max_user_instances`) and restart.
- Starting a watch no longer waits for the operating system to register
  notifications. On macOS, FSEvents registration held the first mob storage
  open for over 30 seconds and every other mob storage open waited behind it.
  Registration runs on a thread of its own; at most 64 run at once in a
  process, and a watch started while 64 are pending runs on its sweep alone
  and logs the same warning. Stopping a watch never waits for a registration
  still in progress: its thread stays with the process until the platform
  call returns, then discards the result. A retired runtime delivery owner
  whose watch left one logs (info) "runtime delivery owner retired; its store
  watch's file notification registration stays with the process until the
  platform call returns".
- A registration that cannot start notify's inotify event-loop thread (the
  process at its thread limit) no longer aborts the process; that watch runs
  on its sweep alone.

Blobs: fetch bytes with `rkat blob get <BLOB-ID> --output <FILE>` (or `--json`
for the payload), RPC `blob/get {blob_id}`, MCP `meerkat_blob_get`, SDK
`get_blob` / `getBlob`. Blob ids come from session history (for example
`data.blob_ref.blob_id` on generated images); the CLI does not list them.

## Forward-only state and storage maintenance

Upgrades write state that older binaries refuse; there is no downgrade path.
Upgrade every process that shares a realm together.

| Since | Change |
|---|---|
| 0.8.9 | Checkpoint stamp schema v3 for sessions that keep a transcript-history graph |
| 0.8.10 | Session-store ledger v2 and the transcript-history splice encoding; 0.8.9 refuses them |
| 0.8.11 | Compatibility floor is 0.8.10; older state needs an older binary or the bridge below |
| 0.8.21 | SQLite session stores move HeadCanonical authority from v1 to v2 (schema v4) |
| 0.8.51 | Session envelopes written as v4; SQLite session schema v5; memory store schema v3 |
| 0.8.52 | Runtime delivery schema v3 |
| 0.9.0 | Stored runtime input state version 6 (versions 3 to 5 are read and rewritten) |
| 0.9.4 | Stored runtime input state version 7 (versions 3 to 6 are read; every row written is version 7) |
| 0.9.5 | Stored runtime input state version 8 for a row that carries a stop mark or the `RequeueExpired` reason (a requeue Shutdown); every other row is still version 7. Machine lifecycle record version 6 for an unregister in progress with the requeue disposition; every other record is still version 5 |

Since 0.9.4 every input row a binary writes is version 7, including a row it
read at an older version. 0.9.3 and older refuse a version 7 row before
installing state (the stored bytes stay in place), so once a 0.9.4 binary has
written any input row there is no downgrade to 0.9.3. Upgrade every process
that shares a runtime store together. 0.9.4 refuses a version 8 row with a stop mark, or a version 6 lifecycle record, with its typed unsupported-version error, and fails to read a version 8 row that ended `RequeueExpired` (an abandon reason it does not know), so a store that a requeue Shutdown wrote does not open in 0.9.4. Likewise a HeadCanonical session that
records a failed realtime response writes realtime sidecar event schema 4,
which 0.9.3 and earlier refuse ("unsupported realtime event schema 4").
A session whose last turn 0.9.2 or 0.9.3 committed but never finalized (for
example a mob member stopped at a compaction) finalizes that turn at its first
0.9.4 start and accepts work again: the store compares a row an earlier
release wrote by its typed content rather than its bytes.

A database whose ledger is newer than the binary is refused as
`SchemaFromTheFuture` before any write. SQLite files must be regular files with
a single hard link (0.8.51). A read-write open converts a rollback-journal
file to WAL or fails with "could not establish journal_mode=WAL on `<path>`:
the conversion stayed lock-contended for N ms" (`WalConversionContended`,
0.8.23); the file is left as found, so retry once the other connection lets
go. Runtime-store failures read "SQLite operation failed: <message>;
primary_code=..; extended_code=..; path=..; operation=..; caller=.."
(`SqliteOperationFailed`, 0.8.36; 5 is `SQLITE_BUSY`); the codes do not make
retrying a write whose outcome is unknown safe. Since 0.9.4 a store no longer
refuses to open with "schema domain ... has no ledger row but already owns
objects" when another process creates the same schema domain at that moment
(most often two processes making their first durable delivery write, the
0.9.3 known issue "First concurrent durable delivery"): the open-time check
reads the schema ledger and the catalog in one snapshot.

```bash
rkat storage doctor [--json] [--root PATH]...      # read-only, safe on live realms; exit 1 on errors
rkat storage migrate [--json] [--root PATH]...     # dry run
rkat storage migrate --apply [--fence-wait-secs 10] [--adopt-root PATH]
rkat storage migrate --apply --bridge-pre-0-8-10   # exact pre-0.8.10 SQLite realms only
rkat storage prune [--apply] [--older-than-days 30] [--json] [--root PATH]...
```

- Storage verbs run before realm resolution, take no leases, and reject
  `--isolated` and `--default-model`. Scope them with `--realm`, the global
  `--state-root` or repeatable `--root` (explicit roots replace the default
  sweep of `<context-root>/.rkat/realms` plus the user-global root).
- `--bridge-pre-0-8-10` needs `--apply` and a `sqlite` realm, attempts every
  domain, commits those that authenticate and reports each refusal; read the
  per-domain report rather than the exit code alone.
- `storage doctor` also reports read-only census findings such as
  `transcript-history-oversized`, `strand-duplication-reclaimable`,
  `frozen-blob-archive-reclaimable` and `storage-census-unmeasured` (0.8.10).
  Since 0.9.4 it also reports the SQLite lock profile of its own binary (the
  `sqlite_lock_profile` object of `--json` and a text line, with the warning
  finding `sqlite-lock-profile` when a process-wide lock is on), and lists
  `*.mpresence` files with the `*.mfence` files as `maintenance-fence-lock`
  inventory findings.
- Since 0.9.4 `storage migrate --apply` also waits, within its fence wait, for
  kept connections to close. The only kept connection is a runtime delivery
  owner's reader of a shared store's delivery generation, which closes at its
  next read or after `KEPT_DELIVERY_READER_IDLE` (1 second) idle, well inside
  the default 10-second `--fence-wait-secs`; a fence wait of a second or less
  can meet an idle reader and refuse. Maintenance from a binary older than 0.9.4
  takes only the fence lock and does not wait for kept connections: run
  maintenance with the same release as the hosts. Windows keeps a fresh,
  verified open per store tick instead of a kept reader.
- `storage prune` deletes only registered `*.pre-<version>-<timestamp>` and
  `*.corrupt-<timestamp>` artifacts; deleting realms is `rkat realm delete` /
  `rkat realm prune`.
- `rkat session migrate` existed in 0.8.1 to 0.8.10 and was removed in
  0.8.11; use `rkat storage migrate`.
- Upgrading from 0.8.9 or older (0.8.10 notes): an input in flight at
  shutdown can be delivered once more after the upgrade, as after a restart,
  and the one-time conversion to head-canonical storage re-materializes every
  retained transcript revision, which can take minutes for a session with a
  large retained history ("head-canonical profile activation is required for
  runtime ..." until it completes). Prune such sessions before converting.
- Since 0.9.4 the SQLite session store lays out a whole-blob session whose
  transcript graph retired its oldest occurrences (any session compacted more
  often than `[compaction] history_retained_rewrites`, default 4), so the
  one-time migration of such a blob row no longer refuses typed; it writes the
  retired base and rewrite rows for the retained generations only. The
  in-memory store still refuses such a session.

## Runtime behaviour you can observe

- Busy and teardown: a runtime teardown that outlives the caller's bounded
  wait answers the retryable busy class with `data.kind =
  "runtime_teardown_in_progress"` (`retryable: true`); retrying joins the same
  teardown.
- `runtime/health` (REST `GET /runtime/health`) reports `status` `ok`,
  `degraded` or `unhealthy` plus per-dimension `checks` for `jobs`,
  `session_liveness`, `session_durability`, `session_runtime_loop` and
  `session_run_start`. `unmeasured:<dim>` means no probe exists (not counted in
  `status`); `unreadable:<dim>` means a probe failed (counts as `degraded`).
- `jobs/health` returns `status` `ok`, `degraded` or `unreadable`, counts such
  as `pending_outbox_jobs` and `runtime_inbox_backlog`, and `coverage`
  (`complete` or `truncated {scanned, limit}`).
- Detached work (`shell` with `background: true`, monitors) needs a durable
  realm; RPC callback tools registered with `tools/register` have a 120 second
  deadline.
- Job results and subscription events reach their origin session on every
  surface (CLI, REST, MCP server and RPC), and an `Event` job delivery wakes
  an idle session instead of waiting for an unrelated turn (0.8.52).
- Job delivery retries (since 0.9.4): a delivery whose projection into the
  runtime inbox failed is retried on the delivery owner's next wake of any
  kind; while it keeps failing, each further retry waits for twice as many
  wakes (up to 64). 0.9.3 and earlier retried it only on a job commit or a
  delivery generation change, so it could stay pending until the process
  restarted. A continuation the runtime is not ready to admit stays pending
  with its typed cause and is retried; the warning "durable job delivery left
  rows pending until the next delivery wake" names it (for example
  `... is blocked (NotReady(OriginalAdmissionPending)): ...`, an original
  whose source owner has not settled its admission yet). Nothing is stored for
  the cause; each retry reads it again.
- `rkat-rpc` shuts down gracefully on SIGTERM or Ctrl-C as on stdin EOF: with
  `--tcp` it stops accepting, drains connections, then stops the runtime
  (0.9.0). Shutdown also closes MCP login admission and retires pending MCP
  login attempts within one deadline; an attempt it cannot retire stays
  recorded and is reported ("MCP login attempts were not all retired at
  shutdown: [...]", `McpAttemptSweepIncomplete`).
- `rkat-rpc`, `rkat-rest` and `rkat-mcp` refuse to start when the head,
  parent or global realm config fails to load (0.8.34); they no longer fall
  back to defaults. A fatal startup error prints as a Display chain on stderr
  (`rkat-rpc: <error>` then `rkat-rpc: caused by: <cause>` lines) and exits
  with status 1.
- Turn and agent bounds: `[limits] max_turn_duration` (unset by default) caps
  one turn's total wall clock and fails the turn when exceeded; `max_duration`
  is the agent-lifetime ceiling; `agent.max_turns` (default 100) bounds
  model-loop iterations per run, and `0` stops before the first request.
- `RKAT_WORKER_STACK_BYTES` overrides the 8 MiB host worker stack (minimum
  64 KiB) for stack-overflow diagnosis only.
- `RKAT_WORKER_THREADS` sets the host's Tokio worker count (default: one per
  available CPU; `TOKIO_WORKER_THREADS` is read when it is unset). Set it
  below a container's pids limit, which counts threads.
- Commits that never answer (since 0.9.4): a persistent session's boundary
  commit or run terminalization that reports no durable outcome within its
  store's declared deadline (120 seconds for the SQLite store, two 60-second
  busy windows) marks the session reload-required instead of holding its
  mutation gate for good (a stalled disk or hung network filesystem used to
  wedge every later wait, a mob destroy and force cancel included). The commit
  keeps running and keeps the gate until the store answers. The run's
  completion waiters fail (`DurableOutcomeUnknown`; a rejected boundary commit
  gives `BoundaryRolledBack`), but the input stays durably admitted: read it
  after the reload instead of resubmitting it. The public error class is
  still authority unavailable and only the message names the case, so
  resubmit only with an idempotency key. Until the store answers, a reload
  and every live-work wait for the gate (admission, interrupts, stops,
  recover and destroy, comms aborts, unregisters) refuse with the typed,
  retryable `RecoveryRepairBlocked`, and a read of an input's terminal
  completion refuses with the retryable `InputTerminalNotYetReadable`;
  afterwards the reload replaces the registration from durable truth. A mob
  member's tracked turn stays pending, and a scheduled delivery waits for the
  input's own terminal outcome (see Schedules below).
- Admissions that never answer (since 0.9.4): an input admission whose store
  write reports nothing within the same deadline refuses with the retryable
  `RecoveryRepairBlocked`, naming the input, and marks the session
  reload-required (`runtime_admission_deadline`). The write keeps the gate
  until the store answers, so the input may still be admitted: resubmit it
  with the same idempotency key, or read it after the reload. Sessions
  without durable state and stores that declare no deadline (the in-memory
  store) admit as before. Writes that no deadline covers still hold the gate
  until the store answers: registration, unregister, control writes,
  recovery, the runtime loop's other writes (a run's staging, for example)
  and the live-boundary work after an admission.
- SQLite lock profile (0.9.4): the bundled SQLite is built without its three
  process-wide locks (allocator statistics, one page cache shared by every
  connection, page-cache overflow statistics), which made concurrent sessions
  in one host queue on each other. Release binaries carry the setting. A
  build outside the repository (`cargo install rkat`, an application that
  embeds Meerkat) sets `LIBSQLITE3_FLAGS="-DSQLITE_DEFAULT_MEMSTATUS=0
  -USQLITE_ENABLE_MEMORY_MANAGEMENT -DSQLITE_DISABLE_PAGECACHE_OVERFLOW_STATS"`
  (`meerkat_sqlite::REQUIRED_LIBSQLITE3_FLAGS`) itself. Without it every host
  binary still turns allocator statistics off as the first statement of
  `main`, but the shared page cache and overflow statistics stay on; the first
  store connection then logs a warning naming the variable, and
  `rkat storage doctor` warns `sqlite-lock-profile`. Nothing Meerkat calls
  reads what the locks provided (SQLite memory statistics, heap limits,
  `sqlite3_release_memory`).
- Since 0.9.4 a runtime delivery owner on a store other processes share reads
  the delivery generation through one kept connection (about 19 microseconds
  per read instead of about 500 for a verified open, measured on a shared
  192-core host; store ticks come up to one per 10 ms). It still takes the
  shared fence guard for each read and opens again, verified, when the file,
  schema or delivery ledger row changed.
- Since 0.9.4 creating a persistent runtime over one store no longer waits for
  another store's OAuth flow rehydration: each store has its own slot in the
  process-wide provider-auth cache, and runtimes over one store still share
  one authority, rehydrated once.

## WorkGraph

WorkGraph is the realm-scoped ledger of shared work items, dependencies,
claims and evidence. Agents change it with `workgraph_*` tools when
`tools.workgraph_enabled = true` (default `false`) or a request sets
`enable_workgraph`. Hosts observe it:

```bash
rkat workgraph list [--status open|in-progress|blocked|completed|cancelled|failed]... [--label L]... [--include-terminal] [--limit N] [--json]
rkat workgraph show <ID> [--json]
rkat workgraph ready | snapshot | events [--after-seq N] ...
rkat workgraph goal-create <SESSION_ID> <TITLE> [--mode pursue|coordinate|review|falsify|judge|observe] [--completion-policy self-attest|host-confirmed]
rkat workgraph goal-status|goal-confirm|goal-close <BINDING_ID> ...
rkat workgraph attention-list|attention-pause|attention-resume ...
```

- The browser (`@rkat/web`) profile has no WorkGraph or scheduler service.
- In 0.9.0 the CLI is bound to the active realm's `default` namespace. A
  non-default `--namespace` is refused ("WorkGraph namespace grant authorizes
  realm ... namespace ..."), and `--all-namespaces` is always refused
  ("all_namespaces requires a separate host capability; a namespace grant
  authorizes exactly one immutable namespace").
- Collection reads default to 100 items; `--limit` above 1000 is refused
  ("limit N exceeds the WorkGraph maximum of 1000"); attention lists are capped
  at 1000 rows.
- Mob members resolve attention in the mob's realm (`mob.<mob_id>`); an
  attention binding that names a mob member is refused in any other realm
  (0.8.41).
- `workgraph_create` takes `failed_child_join_policy` and
  `cancelled_child_join_policy`: `require_success` (default), `propagate` or
  `accept`, deciding how a parent treats failed or cancelled children linked
  by `parent` edges (0.8.22). Readiness facts (`item_ready`, `lease_expired`,
  `namespace_terminal`) are committed by the mutation that first observes
  them and appear in that event's `facts` list (`workgraph/events`,
  `rkat workgraph events`); WorkGraph runs no timer of its own (schedules own
  time).
- The SQLite WorkGraph store is never created, initialized or migrated
  implicitly; a missing or unusable store answers `BackingStoreUnavailable`
  ("work graph backing store 'sqlite' is unavailable: database could not be
  opened; inspect host diagnostics", or "...: storage is under offline
  maintenance"; the host path goes to logs only). Never treat it as an absent
  item or as leave to repeat a mutation (0.8.50).
- RPC: `workgraph/get`, `workgraph/list`, `workgraph/ready`,
  `workgraph/snapshot`, `workgraph/events`, `workgraph/goal/status`,
  `workgraph/attention/list`. REST: `GET /workgraph/items`,
  `GET /workgraph/items/{id}`, `GET /workgraph/ready`,
  `GET /workgraph/snapshot`, `GET /workgraph/events`,
  `POST /workgraph/goal/status`, `POST /workgraph/attention/list`. Goal and
  attention mutations exist only on the CLI and in trusted Rust hosts.

## Schedules

There is no `rkat schedule` command. Manage schedules with RPC `schedule/*`,
REST `/schedules`, SDK helpers, or the agent tools `meerkat_schedule_*`
(`tools.schedule_enabled` defaults to `true`). Schedules are unavailable on
JSONL-backed realms.

```json
{"jsonrpc": "2.0", "id": 1, "method": "schedule/create", "params": {
  "name": "hourly-health-check",
  "trigger": {"type": "interval", "start_at_utc": "2026-11-01T09:00:00Z", "every_seconds": 3600},
  "target": {"target_kind": "session", "type": "materialize_on_demand_session",
             "create": {"model": "claude-opus-5-5"},
             "action": {"type": "prompt", "prompt": "Check system health."}},
  "misfire_policy": {"type": "skip"},
  "overlap_policy": "skip_if_running",
  "missing_target_policy": "mark_misfired"}}
```

- Triggers (`type`): `once {due_at_utc}`, `interval {start_at_utc,
  every_seconds, end_at_utc?}`, `calendar {timezone, minute, hour,
  day_of_month, month, day_of_week, year?}` where each field is
  `{"kind": "any"}` or `{"kind": "values", "values": [...]}`.
- Targets (`target_kind`): `session` (`exact_session`, `resumable_session`,
  `materialize_on_demand_session`), `identity`, `mob`, `host_runnable`
  (host-registered callbacks). Session actions: `prompt {prompt,
  system_prompt?, skill_refs, additional_instructions}` or
  `event {event_type, payload}`.
- Only `trigger` and `target` are required. Defaults: misfire `skip` (30
  second grace; or `catch_up_within {window_seconds}`), overlap
  `skip_if_running` (or `allow_concurrent`), missing target `mark_misfired`
  (or `skip`), planning horizon 30 days or 64 occurrences
  (`planning_horizon_days`, `planning_horizon_occurrences`). The driver claims up to 32 due occurrences per tick under a
  60 second renewable lease, and only the holder of the store-issued executor
  lease fires.
- Occurrence phases: `pending`, `claimed`, `dispatching`,
  `awaiting_completion`, `completed`, `skipped`, `misfired`, `superseded`,
  `delivery_failed`. RPC: `schedule/create|get|list|update|pause|resume|delete`,
  `schedule/occurrences`, `schedule/tools`, `schedule/call`; REST:
  `GET|POST /schedules`, `GET|PATCH|DELETE /schedules/{id}`,
  `POST /schedules/{id}/pause|resume`, `GET /schedules/{id}/occurrences`;
  unknown ids return `SCHEDULE_NOT_FOUND` (-32023, HTTP 404).
- Delivery terminal (since 0.9.4): an occurrence ends only from its delivered
  input's own terminal outcome. A reclaim whose redelivery the target
  deduplicated stays `dispatching` and waits for the original input (its
  receipt still records the admission as deduplicated). While the input is
  live the occurrence waits and then ends with its outcome; a failed input
  ends it `delivery_failed` with the input's failure class, and so does a mob
  target's redelivery that finds its delivery already failed. While the
  outcome cannot be read yet (no finalized receipt, an unreadable store, or a
  session degraded behind a stalled commit), the occurrence keeps its lease
  and re-reads it for up to 5 lease periods (5 minutes at the default 60
  second lease), then ends `delivery_failed` with
  `runtime_completion_authority_unavailable`. An input that ended without a
  public completion or has left the runtime ends it the same way and is never
  admitted a second time. Nothing reclaims a waiting occurrence, whatever its
  misfire policy.
- A runtime-backed delivery whose completion wait fails with
  `BoundaryRolledBack` or `DurableOutcomeUnknown` (a commit with no durable
  answer, see above) re-reads the input's terminal outcome under the same
  bound: the input is durably admitted and runs again after its session
  reloads, or its stalled commit lands. The occurrence then ends `completed`
  or `delivery_failed` with the input's class; at the bound it ends
  `delivery_failed` with `runtime_completion_authority_unavailable`, and the
  detail names the bound and the last cause. The wait never reloads the
  session: the reload comes from the session's own traffic (its next delivery
  or input, or a mob's `reload_member_registration`). Any other wait failure
  still ends the occurrence at once, and so does a dispatch built with
  `AcceptedScheduledInput::with_runtime_handle`, which names no input to read.
- Known issue in 0.9.0: in a governed realm, sessions a schedule materializes
  are not bound to an agent, so their configured MCP servers are unavailable.

## Configuration

Precedence for an agent build: built-in defaults, then the `global` realm
(`~/.rkat/config.toml`) and any `parent` realms folded root-first, then the
head realm's `config.toml`, then per-request parameters (model, system
prompt, tool toggles). Environment variables are not a general config layer:
they supply provider credentials (`RKAT_*` and provider-native keys) at
resolve time. Config inherits down the chain; state never does.

```toml
[realm.team]
parent = "global"     # inherit from another realm's config
```

`rkat config` edits the raw head document of the selected realm:

```bash
rkat config get [--format toml|json] [--with-generation]
rkat config set <FILE> | --json '<JSON>' | --toml '<TOML>' [--expected-generation N]
rkat config patch <FILE> | --json '<JSON>' [--expected-generation N]
```

- `get` defaults to TOML and prints every default filled in;
  `--with-generation` adds a `# generation = N` header (TOML) or an envelope
  `{config, generation, realm_id, instance_id, backend}` (JSON). `set` and
  `patch` print `generation=N`.
- `patch` is RFC 7396 merge-patch applied to the persisted document (0.8.52):
  unnamed keys keep their values, comments and order; `null` removes a key so
  it inherits again. Prefer it for edits.
- `set` is a full replace. Keys you omit are written as defaults and become
  the realm's own overrides, which can re-enable capabilities a parent
  disabled (`tools.schedule_enabled`, provider web search, Gemini search,
  `skills.enabled`). `set` refuses a wrapped `{config, generation}` envelope
  and refuses FILE together with `--json`/`--toml`.
- Typed load warnings print on stderr once per file without changing the exit
  status (0.8.43).
- CAS: a stale `expected_generation` fails with "Generation conflict: expected
  N, current M" (RPC `INVALID_PARAMS`, REST `400`). RPC `config/get|set|patch`,
  REST `GET|PUT|PATCH /config` and MCP `meerkat_config` share the envelope;
  `resolved_paths` is included on RPC and on REST/MCP only with
  `--expose-paths`.
- Writes are refused when they add or change a realm MCP server containing a
  `${...}` environment reference, or when they would change the `governance`
  section. A realm document that sets `[agent] provider_params` fails to load
  (use per-profile or per-request `provider_params`).

Keys in this area (realm `config.toml`):

```toml
[storage]
hosting = "single_process"      # or "multiprocess"

[agent]
max_turns = 100                 # explicit values survive inheritance; 0 = stop before the first model call

[limits]
budget = 200000                 # token budget, agent lifetime
max_duration = "4h"             # agent-lifetime wall clock
max_turn_duration = "30m"       # unset by default; aggregate ceiling for one turn
max_sessions = 100000           # active-session admission capacity

[compaction]
history_retained_rewrites = 4   # transcript rewrites that keep their bodies

[tools]
schedule_enabled = true         # the only tool toggle that defaults to true
workgraph_enabled = false       # builtins_enabled, shell_enabled, comms_enabled, mob_enabled also default false

[rest]
host = "127.0.0.1"
port = 8080
```

`rkat --default-model <MODEL>` patches `agent.model` in the scope-resolved
config. Server binaries take scope flags (`--realm`, `--isolated`,
`--instance`, `--realm-backend`, `--state-root`, `--context-root`,
`--user-config-root`); `rkat-rest` and `rkat-mcp` add `--expose-paths`;
`rkat-rpc` adds `--tcp`, `--allow-remote`, `--live-ws`,
`--live-ws-scheme ws|wss`, `--live-tool-timeout-ms` and, in `live-webrtc`
builds, `--live-webrtc`.
