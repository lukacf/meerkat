Covers: Meerkat 0.9 API surface changes: JSON-RPC methods and REST routes
added or renamed since 0.8.40; the full `ErrorCode` table (-32001 to -32037)
with HTTP status, input readiness kinds (`original_admission_pending`), and
real `rkat` exit codes; Python, TypeScript and Web SDK changes (RunInput,
CONNECTION_CLOSED stderr tail, wasm stack, 0.9.4 `run_failed` content-filtered
reason and compaction event fields); prebuilt binaries, `cargo install` and
low-memory builds; Rust embedding (`crates/<name>` layout, path dependencies,
`[patch]`, 0.9.4 host API changes); removals.

# API Changes, Error Codes, SDKs and Binaries (Meerkat 0.9)

Names are exact and checked against the 0.9.0 source. Session operations:
`references/sessions_runtime_surfaces_sessions.md`. Storage, runtime, WorkGraph,
schedules and config: `references/sessions_runtime_surfaces_storage_config.md`.

## JSON-RPC and REST changes since 0.8.40

| Change | JSON-RPC | REST | Version |
|---|---|---|---|
| Run-fenced stop | `turn/stop_run`, `mob/stop_member_run` added | `POST /sessions/{id}/runs/{run_id}/stop`, `POST /mob/{id}/members/{agent_identity}/runs/{run_id}/stop` added | 0.8.49 |
| Cancel a pending OAuth attempt | `auth/login/cancel` added | `POST /auth/login/cancel` added | 0.8.51 |
| Live WebRTC media health | `live/media_health` added (`live-webrtc` builds) | none | 0.8.51 |
| Host job leases | `mobkit/jobs/*` renamed `meta/jobs/{heartbeat,progress,checkpoint,complete,fail,cancel_ack}`, no aliases | none | 0.9.0 |
| Human consent | `approval/consent_notice` notification; `approval/consent_decide` only on a stock governed stdio pipe | none | 0.9.0 |

No other method or route was added or removed between 0.8.40 and 0.9.0.
Earlier in the 0.8 line: `jobs/*` and `monitors/start` (0.8.8);
`session/update_system_prompt`, REST `POST /sessions/{id}/system_prompt` and
`session/export_atif` (0.8.22); the mob member declaration methods (0.8.26);
`session/activate_instruction`, `session/instruction_activations` and
`live/playback_complete` (0.8.30). A REST create whose first turn fails after
the session was durably created returns `SESSION_CREATED_WITH_TURN_FAILURE`
with the resumable session identity in `details`.

## Error codes

`meerkat_contracts::ErrorCode`, with JSON-RPC code and HTTP status:

| Code | RPC | HTTP | Meaning |
|---|---|---|---|
| `SESSION_NOT_FOUND` | -32001 | 404 | No such session in this realm |
| `SESSION_BUSY` | -32002 | 409 | Needs an idle session (fork, transcript edit), still materializing, served by another process, or tearing down |
| `SESSION_NOT_RUNNING` | -32003 | 409 | Not runnable; durable resume holds use this code |
| `DUPLICATE_INPUT` | -32004 | 409 | Same idempotency key already admitted |
| `REQUEST_CANCELLED` | -32005 | 499 | Cancelled before commit |
| `PROVIDER_ERROR` | -32010 | 502 | Upstream LLM failure |
| `BUDGET_EXHAUSTED` | -32011 | 429 | Token, time or tool-call budget hit |
| `HOOK_DENIED` | -32012 | 403 | A hook denied the operation |
| `AGENT_ERROR` | -32013 | 500 | Agent loop failure |
| `CAPABILITY_UNAVAILABLE` | -32020 | 501 | Feature not compiled, not composed, outside the runtime profile (browser), or persistence disabled |
| `SKILL_NOT_FOUND` | -32021 | 404 | Unknown skill |
| `SKILL_RESOLUTION_FAILED` | -32022 | 422 | Skill could not be resolved |
| `SCHEDULE_NOT_FOUND` | -32023 | 404 | Unknown schedule |
| `SUPERVISOR_ROTATION_INCOMPLETE` | -32024 | 409 | Mob supervisor rotation partially failed |
| `SCOPE_DENIED` | -32025 | 403 | Mob control scope missing |
| `HOST_UNAVAILABLE` | -32026 | 503 | Member host unreachable |
| `STALE_CURSOR` | -32027 | 410 | Event cursor fell out of a host's buffer |
| `STALE_FENCE` | -32028 | 409 | Superseded generation or write fence |
| `MEMBER_RELOAD_REQUIRED` | -32029 | 409 | Member needs an explicit registration reload (0.8.35) |
| `INPUT_REFUSED` | -32030 | 403 | Native owner declined the input (0.8.51) |
| `INPUT_NOT_READY` | -32031 | 503 | Input readiness unavailable (0.8.51) |
| `SESSION_RUNTIME_UNAVAILABLE` | -32032 | 503 | Runtime authority or hosting claim unavailable |
| `APPROVAL_STORE_UNAVAILABLE` | -32033 | 503 | Approval store committed nothing (0.9.0) |
| `APPROVAL_CONFLICT` | -32034 | 409 | Approval changed by another owner (0.9.0) |
| `APPROVAL_COMMIT_UNKNOWN` | -32035 | 500 | Approval commit unknown; reconcile (0.9.0) |
| `STALE_DELIVERY_SCOPE` | -32036 | 409 | Member session rotated; re-capture the delivery scope (0.9.0) |
| `FORK_INSTRUCTION_REFUSED` | -32037 | 422 | A fork refused before commit by the fork instruction policy; `data.fork_instruction_refusal` names the cause and keys; CLI exit 57 (0.9.4) |
| `INVALID_PARAMS` | -32602 | 400 | Invalid request |
| `INTERNAL_ERROR` | -32603 | 500 | Unexpected failure |

Branch on `error.data` kinds, not message text: `session_served_elsewhere`,
`session_hosting_unavailable`, `runtime_teardown_in_progress`,
`durable_resume_hold`, the JSON-RPC session runtime's -32032 kinds
`runtime_shutting_down` (its shutdown began) and `host_runtime_stopped` (the
session's host runtime stopped and its registration waits for its reload; send
the input again after the next start), and auth `reason` values. Ordinary REST
errors use `{"error", "code", "details"?}` with route-specific codes (`BAD_REQUEST`,
`NOT_FOUND`, `CONFLICT`, `DUPLICATE_REQUEST_ID`, `SERVICE_UNAVAILABLE`, ...);
REST routes that project the canonical code use the HTTP status above. A failed
SDK call raises (Python) or rejects with (TypeScript) a `MeerkatError` that has
`code`, `message` and `details`; `details` keeps the structured payload
(0.8.35).

`INPUT_REFUSED` and `INPUT_NOT_READY` carry the native owner's typed detail
as `error.data`: `{"type": "refused", "kind": "denied" | "malformed_facts"}`
or `{"type": "not_ready", "reason": {"kind": ...}}`. Readiness kinds:
`policy_unavailable`, `executor_unavailable`, `facts_unavailable`, `busy`,
`authority_unavailable`, `authority_changed`, `unsupported_scope`,
`credential_unusable` (with `disposition`), `credential_preparation_failed`
(with `cause`), `policy_changed`, `replacement_not_empty`, `unknown`, and
since 0.9.4 `original_admission_pending`: an original the input or operation
relies on is still its source owner's submission candidate, whose admission
the owner has not settled yet. 0.9.3 and earlier refused such work as
malformed or denied; it is now unavailable and nothing waits, so retry later
(a retained resume stays retryable until the owner settles the original; a
cancelled original is still refused). A Rust decoder of
`WireControllerReadinessFailure` built before 0.9.4 rejects the new kind; the
Python, TypeScript and Web SDKs keep error details as untyped data.

`rkat` exit codes (0.9.1). The first typed error in a failure's cause chain
decides, with its `ErrorCode::cli_exit_code()` number (the code JSON-RPC
reports); a failure with no typed owner exits 1.

- 0: success, including a run stopped at its token or tool-call budget
  (`terminal_cause_kind: "budget_exhausted"`) and a run left pending on a
  callback tool.
- 1: a turn hard failure (the `--max-duration` time budget, `max_turns`, a
  provider, hook, tool or structured-output failure), a check the CLI makes
  itself (arguments, config, storage, `--default-model`, scope, a `--resume`
  handle that matches nothing or is ambiguous, no subcommand), a governed
  realm's local refusal ("the governed realm refused this prompt"), and a
  typed error whose owner has no public code.
- 2: usage errors (argument parsing, storage verbs given `--isolated` or
  `--default-model`, an invalid `RKAT_WORKER_STACK_BYTES`,
  `RKAT_WORKER_THREADS` or `TOKIO_WORKER_THREADS`), a typed config error, and
  a host whose main thread the OS refused at startup (a thread limit).
- 10 `SESSION_NOT_FOUND`: `run --resume <exact id>` or `session show <id>`
  for a session the realm does not hold.
- 11 `SESSION_BUSY`: a busy session, one served by another process, or a
  teardown in progress.
- 12 `SESSION_NOT_RUNNING`: a durable resume hold.
- 14 `REQUEST_CANCELLED`: a cancelled or interrupted turn.
- 20 `PROVIDER_ERROR`: a provider or build failure while the session is
  built.
- 22 `HOOK_DENIED` and 42 `SKILL_RESOLUTION_FAILED`.
- 50 `INPUT_REFUSED` and 51 `INPUT_NOT_READY`: the runtime refused the input
  or its readiness is unavailable.
- 52 `SESSION_RUNTIME_UNAVAILABLE`: hosting or the runtime is unavailable.
- Mob commands (including `rkat mob run <pack>`): 45 `SCOPE_DENIED`, 46
  `HOST_UNAVAILABLE`, 47 `STALE_CURSOR`, 48 `STALE_FENCE`, 56
  `STALE_DELIVERY_SCOPE`, 49 `MEMBER_RELOAD_REQUIRED`, 44 (supervisor
  protocol upgrade required), 11 (retirement, lifecycle or admission pending,
  reload refused or timed out, actor command timeout), 40 (capability or
  delivery scope unavailable) and 52 (held before activation, runtime
  unavailable). An error with a typed console detail also prints one
  `detail: <json>` line on stderr.
- 21 `BUDGET_EXHAUSTED`: a typed budget error, printed as "Budget exhausted:
  ..."; `rkat run` does not produce one, because a budget-stopped run
  succeeds.

`rkat` 0.9.0 used these numbers only for the mob codes 45-48 and 56 and
exited 1 for nearly every other failure. `rkat-rpc`, `rkat-rest` and
`rkat-mcp` exit 1 on a fatal error.

## SDK changes

Python (`meerkat-sdk`, import `meerkat`), TypeScript (`@rkat/sdk`) and
`@rkat/web` are released with the binaries; pin all three to the same
release as `rkat-rpc`. The handshake accepts a server with the same major and
minor contract version (any 0.9.x).

Changed in 0.9.4 (event wire and SDK types):

- `run_failed` with error class `content_filtered` (the provider refused or
  filtered the response) now carries `error_report.reason`
  `{"reason_type": "content_filtered", "stop": "refusal" | "content_filter",
  "detail": {...}}` instead of `null`; `detail` is the provider's own account,
  tagged by `provider` (`anthropic`, `open_ai` or `gemini`; fields per provider
  in `references/providers_and_auth_models.md`, "Refusals and content
  filters"), and is omitted when the provider sent none. Python reads it as `AgentErrorReason` with
  `reason_type == "content_filtered"`, `stop` and `detail`; TypeScript as
  `ContentFilteredAgentErrorReason` (`reasonType`, `stop`, `detail?`). SDKs
  before 0.9.4 read it as an unknown reason and keep it raw. A 0.9.3 binary
  cannot read a session event log that holds such a line, so a realm whose
  sessions recorded one cannot go back to 0.9.3.
- `compaction_completed` carries `summary_source`: `{"kind":
  "provider_call", "source_bound": {...}}` (bound `whole`, `trimmed` with
  `through` = `tool_results`, `context` or `user_requests`, or `requests_only`
  with `kept` and `of`), `{"kind": "host_curator"}` or
  `{"kind": "mechanical_fallback"}`. Python
  `CompactionCompleted.summary_source` (`CompactionSummarySource`), TypeScript
  `CompactionCompletedEvent.summarySource`; a kind the SDK does not know parses
  to `unknown` with the wire object (`raw_kind`/`raw`, `rawKind`/`raw`). It is
  absent on completions recorded before it was reported.
- `compaction_completed` carries optional `estimated_history_tokens_after`,
  the history the rewrite left in the unit of `compaction_started`'s
  `estimated_history_tokens` (Python
  `CompactionCompleted.estimated_history_tokens_after`, TypeScript
  `CompactionCompletedEvent.estimatedHistoryTokensAfter`); absent for older
  completions or when the history could not be measured.
- `tool_config_changed`: the payload's lifecycle bit is `settled` (a settled
  lifecycle fact, not pending or draining), not `persisted`; it never meant a
  configuration write (that is the `persisted` of `mcp/add`, `mcp/remove` and
  `mcp/reload`, unchanged). Python and TypeScript
  `ToolConfigChangedPayload.persisted` is now `settled`, derived from
  `status_info` as rkat-rpc derives it. Events and stored notices from 0.9.3
  and earlier keep `persisted` when read back, so raw readers accept either
  key. Tool-scope, deferred-catalog and failed-closed notices now report
  settled where they reported `persisted: false`. The `tool_config_changed`
  entry of `known_payloads` in `artifacts/schemas/events.json` is generated
  from the typed payload; the hand-written one required a `status` field the
  event never carried, so real events failed validation against it.
- Version skew: a 0.9.3 Python or TypeScript SDK requires `persisted` and fails
  on every `tool_config_changed` event from a 0.9.4 `rkat-rpc`. The binary an
  SDK downloads always matches it, so this happens only when it runs one you
  supply (`MEERKAT_BIN_PATH`, an explicit path, or `rkat-rpc` on `PATH`);
  upgrade the SDK with the binary. 0.9.3 also cannot read back a transcript
  that 0.9.4 wrote with a `tool_config` notice.
- TypeScript `MeerkatClient.close()` resolves as soon as the `rkat-rpc` child
  exits (it ends stdin, signals the child, waits up to 5 seconds, then sends
  SIGKILL); before 0.9.4 its timer kept Node alive for about 5 seconds, and a
  grandchild holding the pipes could make it wait the full 5 seconds or, after
  the SIGKILL, forever. Requests waiting on a child that exits by itself (a
  crash) fail with `PROCESS_EXITED` at most about a second after the exit, even
  while a grandchild holds the pipes; before, they could wait forever. After a
  `PROTOCOL_ERROR` (a reply line that is not JSON) the client stops the child
  the way `close()` does.
- Python: requests waiting on a child that exits by itself fail with
  `CONNECTION_CLOSED` (with the stderr tail) at most about a second after the
  exit, and `MeerkatClient.close()` returns once the child has exited, even
  while a grandchild holds the pipes (on Python 3.11 and 3.12, and 3.13 or 3.14
  before the fix for CPython issue 119710, `close()` used to wait 5 seconds).
- Live observations: `turn_failed` (a response the provider ended without
  completing) carries `failure`, typed as `WireLiveResponseFailure`: `status`
  `failed`, `incomplete`, `in_progress` or `{"unknown": {"debug": ...}}`, plus
  diagnostic `reason`, `code` and `message`. Python and TypeScript export
  `WireLiveResponseFailure` and `WireLiveResponseFailureStatus` from the package
  root; `parse_live_observation` / `parseLiveObservation` type it.

Added since 0.8.40:

- Python `Session.stop_run`, `Mob.stop_member_run`,
  `MeerkatClient.stop_mob_member_run`; TypeScript `Session.stopRun`,
  `Mob.stopMemberRun`, `MeerkatClient.stopMobMemberRun`; Web
  `Session.stopRun`, `Mob.stopMemberRun`.
- `live_media_health` / `liveMediaHealth`; `on_live_notification` /
  `onLiveNotification`; `on_consent_notice` / `onConsentNotice`.
- Auth: Python `auth_mcp_login_start|complete|cancel|cancel_by_attempt_ref`,
  `auth_mcp_status`, `auth_mcp_logout`, `auth_connector_login_start|complete|cancel`,
  `auth_connector_status`; TypeScript `authLoginCancel`, `authMcpStatus`,
  `authMcpLogout`, `authConnectorStatus`; Web `loginCancel`, `mcpStatus`,
  `mcpLogout`.
- Web `Session.interrupt()` and `Session.wirePeer(peer)`.

Breaking:

- 0.9.0: `mobkit_job_*` / `mobkitJob*` renamed `meta_job_*` /
  `metaJob*` (no aliases).
- 0.8.46 (`@rkat/web`): `MeerkatRuntime.createSession`,
  `MeerkatRuntime.destroy`, `Session.getState`, `Session.destroy` and the
  `Session.sessionId` getter return promises; `Session.isDestroyed` and
  `TurnResult.response` were removed.
- 0.8.44: Python `RunStarted` exposes `session_id` and a typed `input`;
  TypeScript `RunStartedEvent.prompt` was replaced by `input`. `input` is a
  `RunInput`: `kind` is `content` (with `content`) or `pending_tool_results`
  (a continuation with no caller prompt). An event in the old prompt-only
  shape is delivered as `malformed_event`, not as a typed event.

`@rkat/web` 0.8.30 to 0.8.44 linked a 1 MiB wasm stack and failed the first
turn with `RuntimeError: memory access out of bounds`; 0.8.45 and later link
8 MiB, so upgrade. Wasm you build yourself for `rkat mob web build --wasm`
needs the same stack (`-C link-arg=-zstack-size=8388608`, as
`sdks/web/scripts/build-wasm.mjs` sets); the CLI does not compile wasm.

Connect options: Python `connect(realm_id=, instance_id=, realm_backend=,
isolated=, state_root=, context_root=, user_config_root=, live_ws=,
live_webrtc=)` (`realm_id` with `isolated` raises `INVALID_ARGS`); TypeScript
`connect({realmId, instanceId, realmBackend, isolated, stateRoot, contextRoot,
userConfigRoot, liveWs, liveWebrtc, liveToolTimeoutMs})`. Both SDKs find
`rkat-rpc` through `MEERKAT_BIN_PATH`, then `PATH`, then download the
matching release binary.

When `rkat-rpc` exits, the Python client fails pending and later calls with
`CONNECTION_CLOSED` ("rkat-rpc process closed; stderr tail: ..."), keeping
the last 16 KiB of the child's stderr in the message and in
`details["stderr_tail"]`, so a startup refusal reaches the caller.
`MeerkatClient.close()` is idempotent and does not raise for a child that
already exited (0.8.34).

## Prebuilt binaries and building from source

Every release publishes `rkat`, `rkat-rpc`, `rkat-rest` and `rkat-mcp` as
`<binary>-<version>-<target>.tar.gz` (`.zip` on Windows), with
`checksums.sha256` and `index.json`, for `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-apple-darwin`
and `x86_64-pc-windows-msvc`. Each archive carries a GitHub build provenance
attestation (`gh attestation verify <archive> --repo lukacf/meerkat`). The
Homebrew formula (`brew install lukacf/meerkat/rkat`, macOS and Linux)
installs all four, and the SDKs download `rkat-rpc` from the same assets.

- Linux binaries run on glibc 2.31 or newer (Debian 11, Ubuntu 20.04 and
  later) and need no OpenSSL: the release fails if a binary references a newer
  glibc symbol or links libssl/libcrypto. No musl (Alpine) build is published.
  macOS binaries are not notarized; Windows is x86_64 only.
- Every binary includes the Anthropic, OpenAI (with realtime models), Gemini
  and Copilot providers, the sqlite, jsonl and memory session stores, memory
  search and skills.
  Package defaults on top (`cargo install` builds exactly these):
  `rkat`: `openai-realtime`, `comms`, `mcp`, `schedule`, `workgraph`, `mob`,
  `rpc-surface`, `local-authorization`; `rkat-rpc` (crate `meerkat-rpc`):
  `comms`, `mcp`, `mob`, `schedule`, `workgraph`, `openai-realtime`,
  `local-authorization`; `rkat-rest` (crate `meerkat-rest`): `comms`, `mcp`,
  `mob`, `schedule`, `workgraph`; `rkat-mcp` (crate `meerkat-mcp-server`):
  `comms`, `mob`, `schedule`, `workgraph`.
- No package enables these by default, so do not rely on them in a prebuilt
  binary; build from source with the feature: `live-webrtc` (WebRTC live
  transport; `rkat-rpc --live-webrtc` exists only in such builds, which need
  `cmake`), `openai-live`, `experimental-gpt-live`, and the cloud IAM auth
  methods listed in `references/providers_and_auth_credentials.md`.

```bash
cargo install rkat --locked              # Rust 1.94 or newer
cargo install meerkat-rpc --locked       # installs rkat-rpc; likewise meerkat-rest, meerkat-mcp-server
CARGO_BUILD_JOBS=2 CARGO_PROFILE_RELEASE_OPT_LEVEL=1 cargo install rkat --locked \
  --config 'profile.release.package.meerkat-machine-schema.opt-level=0'   # small hosts
```

- A default `cargo install rkat` compiles about 380 crates at Cargo's release
  defaults; no workspace profile reaches `cargo install`, so you set the
  profile (`.cargo/config.toml`, `CARGO_PROFILE_RELEASE_*` or `--config`).
  The project's measured cold build peaked near 25 GiB with 18 parallel jobs,
  and its largest single compiler process (`meerkat-mob`) near 11 GiB; only
  `--config 'profile.release.package.meerkat-mob.opt-level=0'` lowers that
  one (to about 6 GiB). Under 8 GiB, prefer a prebuilt binary.
- Builds need a C compiler and libc headers (bundled SQLite), no OpenSSL.
  Since 0.9.4 set `LIBSQLITE3_FLAGS="-DSQLITE_DEFAULT_MEMSTATUS=0
  -USQLITE_ENABLE_MEMORY_MANAGEMENT -DSQLITE_DISABLE_PAGECACHE_OVERFLOW_STATS"`
  for a build outside the repository (`cargo install`, an embedding
  application): it turns off SQLite's three process-wide locks, which the
  repository's own builds and the release binaries already do; otherwise
  `rkat storage doctor` warns `sqlite-lock-profile`.
- Reduced builds are source-only. The repository keeps `rkat
  --no-default-features --features session-store`, `session-store,mcp` and
  `session-store,comms,mcp,workgraph` compiling; leaving out the mob graph
  means omitting `mob`, `rpc-surface` and `openai-live`.
- Host binaries run their workers on an 8 MiB stack; `RKAT_WORKER_STACK_BYTES`
  overrides it for diagnosis only. They start one worker per available CPU;
  `RKAT_WORKER_THREADS` (then Tokio's `TOKIO_WORKER_THREADS`) sets the count,
  which a container whose pids limit is below its CPU count needs.

## Embedding the Rust crates

- Since 0.8.42 every workspace crate lives under `crates/<name>`, and test
  crates and fixtures under `tests/`. Directory names equal package names
  except `crates/meerkat-cli`, which publishes as `rkat`. `xtask`,
  `meerkat-machine-codegen` and `meerkat-web-runtime` are not published.
- Pin every `meerkat*` crate to the same exact version as your binaries and
  SDKs (`meerkat = "=0.9.0"`); the family is released and tested as one set.
- To build against a checkout, point path dependencies or `[patch.crates-io]`
  entries at `<checkout>/crates/<crate>`, one for every `meerkat*` crate you
  name, all from the same checkout. Inside the checkout the crates reach each
  other by path, so an unpatched sibling you also name would come from
  crates.io as a second, incompatible copy.

```toml
[dependencies]
meerkat = "=0.9.0"
meerkat-core = "=0.9.0"

[patch.crates-io]
meerkat = { path = "../meerkat/crates/meerkat" }
meerkat-core = { path = "../meerkat/crates/meerkat-core" }
```

- A `[patch]` entry is used only when the checkout's version satisfies your
  requirement: with `=0.9.0` pins the checkout must be at 0.9.0, otherwise
  Cargo reports the patch as unused and keeps the crates.io copy.
- Your own root `Cargo.toml` owns `[profile.*]`. Meerkat's workspace profile
  (only an `opt-level = 1` pin for `meerkat-machine-schema`) applies to builds
  run inside its checkout, never to `cargo install` or to your binary.

Rust host API changes in 0.9.4 (hosts that embed the crates):

- Source owners: `AuthenticatedOriginalSource::retained_original` is replaced
  by `retained_custody`, returning `RetainedCustody` (`Retained`,
  `AdmissionPending` or `NotRetained`; non-exhaustive). Answer
  `AdmissionPending` only for a submission candidate you will promote or
  withdraw yourself within your own bound, withdraw any candidate you cannot
  settle (it answers `NotRetained` from then on), and answer `NotRetained`
  where you returned `None`. Work naming a pending original reports
  `ControllerReadinessFailure::OriginalAdmissionPending` (new).
- `meerkat::ContinuationAdmitError` gains `NotReady(ControllerReadinessFailure)`
  and is `#[non_exhaustive]` (add a wildcard arm); such a continuation stays
  pending and is retried. `BlockedDeliveryReason` gains the same `NotReady`
  variant, so its variants no longer have defined discriminants; the job drain
  reports it from the new `JobOutboxProjectionError::NotReady`.
- `MeerkatMachine::durable_admitted_original` (and
  `SessionServiceRuntimeExt::durable_admitted_original`, default `Unavailable`)
  reads a keyed governed original's historical admission from the session's
  durable runtime store after the session retired, with no live state:
  `DurableAdmittedOriginal` (input id, retained association, durable phase,
  terminal outcome, last run, attempt count) or `DurableAdmittedOriginalRefusal`
  (`Conflict`, `NotAdmitted`, `UnknownRuntime`, `Unavailable`).
- `meerkat_sqlite::preflight_domain_version` runs the schema eligibility check
  and returns the ledger version it verified (`None` for a fresh domain).
- Fork instruction policy: `SessionError::ForkInstructionRefused` (wire
  `FORK_INSTRUCTION_REFUSED`; exhaustive matches handle it).
  `SessionForkAtRequest`, `SessionForkReplaceRequest` and
  `DurableSessionForkTarget` gain `inherited_instructions` (`None` is the
  default policy) and `SessionForkResult` gains `reactivated_instruction_count`
  (`prefix_message_count()` excludes those rows); struct literals set them.
  New: `ForkInstructionPolicy` (`RequireExplicitActivation` default,
  `ReactivateInherited`, `Refuse`) with `effective`, `ActivationWindow`,
  `ForkInstructionRefusal` and `ForkInstructionRefusalCause`,
  `Session::apply_fork_instruction_policy`, `Session::fork_reactivation_suffix_len`.
  A turn refused because a fork child never activated its inherited keys
  fails with `AgentError::InstructionActivation` (was `ConfigError(String)`;
  same error class and wire code).
- `CompletionWaitError` is `#[non_exhaustive]` (add a wildcard arm) and gains
  `BoundaryRolledBack { run_id, detail }` and `DurableOutcomeUnknown { run_id }`
  (predicates `is_boundary_rolled_back`, `is_durable_outcome_unknown`; both map
  to the `AuthorityUnavailable` wait-failure observation).
  `RuntimeStore::commit_outcome_deadline`, declared through
  `RuntimeSessionAuthorityOps::commit_outcome_deadline` (decorators forward
  it; a composed store adds the busy timeout of each lock-acquiring
  transaction of its own; default `DEFAULT_COMMIT_OUTCOME_DEADLINE` = two
  `COMMIT_BUSY_WINDOW`s of 60 s; `InMemoryRuntimeStore` declares none).
- `RuntimeDriverError::InputTerminalNotYetReadable { input_id, reason }`
  (retryable; the enum is `#[non_exhaustive]`):
  `SessionServiceRuntimeExt::input_terminal_completion` on a session degraded
  behind a commit past its deadline refuses with it instead of waiting for
  the held driver; read again after the reload.
- `ScheduleTargetDelivery::deliver_occurrence`: the completion of an exact
  replay (a deduplicated redelivery) now decides the occurrence's terminal,
  so it must resolve from the original input's own terminal outcome; the
  driver never records a terminal at admission.
- `MeerkatMachine::bind_scoped_context` refuses a destination whose
  `context_generation` does not advance with
  `RuntimeDriverError::ScopedContextGenerationNotAdvanced { bound, requested }`
  (was `Internal`). `PreparedSessionMaterialization::inserted_registration()`
  says whether the materialization's own prepare inserted the registration.
- `TurnStateHandle` gains provided methods `turn_phase`,
  `apply_turn_input_observing_phase` and `scalars` (`TurnStateScalars`);
  existing implementors keep working.
- `MeerkatMachine::cancel_input_if_present` and
  `cancel_tracked_input_for_member_incarnation` wait for an in-between input's
  next committed lifecycle step instead of re-reading it every 25 ms; the
  5-second bound is only a hang guard.
- `RuntimeDeliveryOwnerRetirement::join` returns `RuntimeDeliveryOwnerRetired`
  (`#[must_use]`) instead of `()`: `notification_registration_retained` names
  the directory (`meerkat_runtime::RetainedNotificationRegistration`) whose
  store watch registration the process still holds (macOS FSEvents can hold it
  for tens of seconds). Match `Ok(_)` or read the report.
  `DeliveryStoreWatch::stop` returns the same registration;
  `SqliteChangeWatch::pending_registration` and
  `meerkat_sqlite::watch::retained_registrations` describe them, and a mob
  observer drain counts them (`retained_registrations`).
- `SqliteRuntimeStore::convert_standalone_whole_blob_in_place` converts a
  standalone WholeBlob runtime database (a file a host composed with
  `SqliteRuntimeStore::new`, such as a 0.9.3 Meta gateway's `runtime.sqlite`)
  in place to the HeadCanonical profile, seeding each session from its
  digest-verified body; open `SqliteSessionStore` on the file first, write no
  session row before it returns, and never use it on a realm database. Report:
  `StandaloneWholeBlobConversionReport`.
- SQLite lock profile: `meerkat_sqlite::lock_profile` with
  `REQUIRED_LIBSQLITE3_FLAGS`, `SqliteLockProfile::compiled()` / `effective()`,
  `check_lock_profile()` (`SqliteLockProfileMismatch` names the setting) and
  `unsafe fn disable_memory_statistics_at_process_start()` (call it as the
  first statement of `main`; `MemoryStatisticsOutcome` `Applied`, `AlreadyOff`
  or `TooLate`, read back with `memory_statistics_outcome()`).
  `StorageDiagnosis::sqlite_lock_profile` (`SqliteLockProfileReport`,
  `SqliteMemoryStatisticsSwitch` in `meerkat-core`) is filled by the disk
  diagnosis only.
- Presence locks: `meerkat_sqlite::KeptConnectionPresence` (`<file>.mpresence`,
  `presence_lock_path`), held shared by a kept connection for its whole life.
  `ExclusiveFence` now also holds the presence lock: `try_acquire` takes both
  or neither, and `acquire` waits for the presence lock within the same
  deadline.
- `RuntimeStore::delivery_generation_reader` (defaulted, `None` reads through
  `load_delivery_generation`) returns an optional `DeliveryGenerationReader`
  for a delivery owner's store ticks; `RuntimeDeliveryInbox::delivery_generation_reader`
  and `KEPT_DELIVERY_READER_IDLE` (1 s). A store decorator forwards the method
  to the store it wraps or keeps the default.
- `meerkat_core::strand_layout_for_retained_history` lays out the retained part
  of a transcript graph, including one that retired its oldest occurrences,
  and returns `RetainedStrandLayout`; `strand_layout_for_history` still refuses
  a re-anchored graph.
- Forks: `SessionBuildState::fork_child_projection` is the build state a
  durable fork commits for its child; `SessionError::fork_source_build_state_missing`
  (`FORK_SOURCE_BUILD_STATE_MISSING_KIND`, `is_fork_source_build_state_missing`)
  is the typed refusal of a source with metadata but no build state.
- Runtime registrations: `RuntimeSessionRegistrationWitness::has_ended` (no
  runtime holds the exact registration any more);
  `MeerkatMachine::session_registration_use` returns an opaque
  `RuntimeSessionRegistrationUse`, freshly stamped whenever a registration is
  handed to a new user in place (a reusing `register_session`, an executor
  attachment, an actor materialization), so compare uses for equality only;
  `observe_unregister_session_registration_if_use_unchanged` starts or joins
  the exact unregister only while the use is unchanged
  (`RuntimeSessionUnregisterUseAdmission`: `Admitted`, `Changed`);
  `reload_required_registration_settlement_if_current`
  (`ReloadRequiredRegistrationUseSettlement`: `NotCurrent`, `NotDegraded`,
  `Discarded { successor, successor_use }`, `Changed`) discards a
  reload-required registration and names its successor;
  `stage_registration_run_start_hold_only` stages a run-start hold for the
  session's next registration, and `unstage_registration_run_start_hold`
  removes a staged reason (a runtime unregister keeps staged reasons, so a
  reload applies them).

## Removed or renamed

- `rkat session migrate`: removed in 0.8.11; use `rkat storage migrate`.
- `mobkit/jobs/*` and `mobkit_job_*` / `mobkitJob*`: renamed to `meta/jobs/*`,
  `meta_job_*`, `metaJob*` in 0.9.0 (no aliases).
- `[storage] directory`: deprecated since 0.8.4 and ignored; use
  `--state-root`.
