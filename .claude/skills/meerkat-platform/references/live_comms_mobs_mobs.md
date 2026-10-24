Covers: Meerkat mob changes since 0.8.40 (through 0.9.4): stop, shutdown
(ShutdownInProgress), destroy, admission close reasons, cancelling member work,
member lifetime, reset and re-spawn, member binding history, helpers,
fork_off, council, profile keys, flows and runs (run-flow waits, --detach),
multi-host mobs, mobpacks and deploy budgets, error codes. Live:
live_comms_mobs_live.md. Comms: live_comms_mobs_comms.md.

# Mob Changes Since 0.8.40 (0.9)

What changed for mobs since 0.8.40, through 0.9.4. `references/mobs.md`
stays the baseline for mob definitions, flows and agent mob tools; where
they disagree, this file wins. Member live channels are in
live_comms_mobs_live.md and comms in live_comms_mobs_comms.md.

## Quick answers

- Every mob profile that spawns a member needs `tools.comms = true`.
- `rkat mob run-flow` waits for the run to finish before it prints the run
  id. `rkat mob run --detach` does not keep executing after the CLI exits.
- `rkat mob deploy` builds the mob in memory: it lives only as long as the
  command (`--surface cli`) or the JSON-RPC stream (`--surface rpc`). Its
  budget flags are enforced since 0.9.1, and with `--surface rpc` since
  0.9.3 (see Mobpacks).

## Stop, resume, shutdown, destroy

- `mob/lifecycle {mob_id, action}` takes `stop`, `resume`, `complete`,
  `reset` or `destroy` (also `mob/destroy`). There is no CLI lifecycle verb;
  use RPC, MCP `meerkat_mob_lifecycle` or SDK `Mob.lifecycle(action)`.
- Stop now pauses members (0.8.51). It holds run starts, so input admitted
  before Stop runs only after Resume. The result carries `stop_report` with
  each member's run outcome. A member reported not holdable (a remote host
  without `run_start_hold`) may still start a turn while the mob is stopped.
  A member reported `reload_required` needs a durability reload: it starts no
  turn until it is reloaded, and the reloaded member stays held until Resume.
  A spawn that finishes into a stopped mob leaves it stopped. Since 0.9.4 a
  local member whose registration needs a durability reload is reported with
  `run` and `starts` `reload_required` instead of failing the Stop; a remote
  one still fails it (reload it on its host, then retry). A Resume refuses
  with `MEMBER_RELOAD_REQUIRED` while a local member needs a durability
  reload; reload it (a reload while Stopped answers `discarded`), then Resume.
  Since 0.9.4 a member reloaded while the mob is stopped stays held until
  Resume, as does one spawned into a stopped mob or held again after a failed
  Resume; retire, Shutdown and destroy release the hold for the members they
  dispose of, so a later registration of the same session is not held, and a
  mob reopened in the same process holds its members only if it is restored
  stopped. A resume finishes a Stop an earlier process left half done before
  it returns the mob (waiting up to 5 s), so the mob reads Stopped. An attempt
  still running after 5 s (held on a member or a placed completion) goes on in
  the background; wait for `MobStopped`.
- Rust `MobHandle::stop()` returns a `MobStopReport`.
  `shutdown_with_report(ShutdownOptions)` cancels in-flight runs immediately
  and reports each member, except that since 0.9.5 its default
  (`QueuedInputsOnShutdown::RequeueDurable`) suspends the running turn of a
  member whose session commits every round for the next start (except a
  retiring member, a retiring or host-placed autonomous member, and a run
  without round commits; see `references/mobs.md`). Since 0.9.4 an admitted
  Shutdown closes admission first (reason `Shutdown`): while it runs,
  commands the close refuses fail
  with `AdmissionClosed` and every other request (stop, destroy, complete,
  retirement, reads, `status()`) gets the retryable
  `MobError::ShutdownInProgress` (wire `SESSION_BUSY`, data kind
  `mob_shutdown_in_progress`); 0.9.3 answered `ActorCommandChannelClosed`,
  which now means the actor has exited or is exiting. A member reported
  `UnregisterPending` carries a `MemberUnregisterSettlement` that keeps
  following its registration (`HandedOver` once a later owner took it), and a
  durability-degraded member is now unregistered. Details in
  `references/mobs.md`.
- Destroy deletes the mob's database files only after its actor ended and its
  observer drained. Until then it answers `mob_destroy_incomplete` with
  `destroy_report` and `retryable: true` (JSON-RPC and MCP: internal error
  data; REST session archive: HTTP 500). Drop every `MobHandle` or store in
  your process and retry as the same caller. The mob id cannot be re-created
  until the cleanup finishes. Since 0.9.4 an admitted destroy (its durable
  marker written) closes admission first (reason `Destroyed`); an incomplete
  destroy keeps it closed while the retry stays available, and a mob restored
  with an unfinished destroy starts with admission closed, so nothing is
  spawned into it.
- Since 0.9.4 a mob keeps its incarnation across resume (only destroy plus
  create starts a new one), and opening a mob store migrates it to schema 12,
  which 0.9.3 refuses: back up mob stores before upgrading. A store whose
  creation record disagrees with its log, or that holds a finished destroy,
  refuses resume (`IncarnationInconsistent`, `IncarnationDestroyed`), and a
  delivery scope from an earlier incarnation gets `IncarnationMismatch`. Such
  a store no longer stops a server's other mobs from restoring. After a
  restart, the session that created a mob with the agent mob tools manages
  it again (not a creating member reset to a later generation since; an
  operator grant restores manage).

## Admission close (Rust hosts)

Every exported `MobHandle` clone shares one owner token. When the last owner
drops and the mob is idle, admission closes for good: new work (spawn,
respawn, work, flow runs, wiring, resume, reset, peer messages, live opens,
host binds) fails with `MobError::AdmissionClosed`; stop, shutdown, destroy,
retirement, cancellation and reads still work. `read_handle()`, `events()` and
`observer()` are not owners; `acquire_owner()` takes a lease. Mobs managed by
`rkat-rpc`, `rkat-rest` or `rkat-mcp` stay owned by the server's registry.
Since 0.9.4 every actor exit closes admission first with a typed
`MobAdmissionCloseReason` (`Shutdown`, `Destroyed`, `DurabilityFailStop`,
`ActorLost` or `DispatchFailed`, besides `IdleClose`, `LastOwnerReleased` and
`StandDown`; an earlier close keeps its reason), so no `acquire_owner()`
succeeds after it. `MobError` is `#[non_exhaustive]`. Details in
`references/mobs.md`.

## Stopping and cancelling member work

| Need | Use |
|---|---|
| Interrupt a member's current turn, keep the member | `rkat mob force-cancel <MOB_ID> <AGENT_IDENTITY>`, RPC `mob/force_cancel`, REST `POST /mob/{id}/members/{agent_identity}/cancel` |
| Stop one exact run | RPC `mob/stop_member_run {mob_id, agent_identity, run_id, reason}`, REST `POST /mob/{id}/members/{agent_identity}/runs/{run_id}/stop`, Python `Mob.stop_member_run`, TypeScript `Mob.stopMemberRun` |
| Immediate operator interrupt | RPC `mob/hard_cancel_member {mob_id, agent_identity, reason}`, Python `Mob.hard_cancel`, TypeScript `Mob.hardCancel`; not on CLI, REST or MCP |

- `force_cancel` cancels the current run at its next boundary and drops
  input queued before the cancel. It is idempotent, and an idle member answers
  ok. A never-seen identity fails with `MemberNotFound`; since 0.9.4 a member
  whose runtime needs a durability reload fails with `MemberReloadRequired`
  (`-32029`) until it is reloaded.
- The run id comes from the `run_started` event. A stale id returns the
  receipt `not_current`, not an error; other outcomes are `stopped` and
  `not_stoppable`.

## Members and member lifetime

- `autonomous_host` (default) members run a host loop and start with an
  automatic kickoff turn after spawn; `turn_driven` members run only when
  dispatched. `rkat mob wait-kickoff <MOB_ID>` (default timeout 10 minutes),
  RPC `mob/wait_kickoff` and `mob/wait_ready` wait for kickoff.
- Retiring a member archives its session and also retires every member it
  spawned with `fork_off`, deepest first, on every surface. Retirement is a
  durable saga: once started, the member is `Retiring` and new work for it
  is refused at once. If teardown outlasts the caller's wait, `mob/retire`
  answers retryable `SESSION_BUSY` with data `{kind:
  "mob_retirement_in_progress", stage}` while the mob keeps driving it. A
  retirement that stops on an error stays `Retiring` and reports
  `mob_retirement_stuck` (`redrive: true`; since 0.9.5 its `cause_data` is
  the cause's own data, such as `mob_member_reload_repair_blocked`: redrive
  later); archiving the member's session
  (`session/archive`, REST `DELETE /sessions/{id}`, MCP `meerkat_archive`)
  re-drives it once per request.
- `rkat mob respawn <MOB_ID> <AGENT_IDENTITY> [--initial-message <MSG>]`
  (RPC `mob/respawn`) replaces the member under a new runtime binding. Since
  0.9.4 the successor keeps its predecessor's spawn-time tool restrictions
  (never widened) and, while the provider matches, its auth binding; a
  respawn whose restrictions cannot be established (a lost-session Broken
  member too) is refused before anything is retired, and the member is kept.
- Since 0.9.4 a mob that was reset and then re-spawned a member under an
  identity it had before the reset resumes after a restart (0.9.3 and earlier
  failed that resume with an internal error), and the re-spawned member always
  gets a generation above every earlier one, so it never reuses its pre-reset
  runtime id or member address. A store written before 0.9.4 whose last epoch
  already reused a generation still resumes; that identity's next spawn goes
  above the recovered high-water.
- Since 0.9.4 a Rust host can read which member incarnation held which
  session: `MobHandle::member_binding_history()` (also on `read_handle()`;
  needs `ControlScope::List`) answers per session across mob resets, and per
  identity (live or retired) since the latest reset, without the actor queue.
  `MobStorage::read_member_binding_history(path)` reads a stopped mob's store
  read-only. Details in `references/mobs.md`.
- Since 0.9.4 a host refusal of a restored member's registration (also one
  the boot recreates on a fresh session) removes that registration and leaves
  only that member Broken with
  `MemberRegistrationRefused` (member status `registration_refusal`) until
  repaired; retire, respawn and destroy still work. A destination the runtime
  did not install is `MemberDestinationNotInstalled` (failure class
  `runtime_rejected`, was `mob_rejected`). Rust
  `MobHandle::retire_at_scope(&scope)` retires exactly the incarnation a
  delivery scope names, never a successor.
- Since 0.9.4 a respawned member gets a fresh health record, idle close
  ignores retired members' work, a restarted placed member shows no
  predecessor kickoff, and destroy reloads a member whose boundary commit
  failed, then retries it once (`reloaded_members`).
- `mob/spawn` also takes `labels`, `additional_instructions` (sections
  appended to the member's system prompt after its skills) and `context`: an
  opaque JSON application context for host build callbacks (a custom
  `SessionAgentBuilder`); the standard build does not read it. Since 0.8.45 a
  member rebuilt without an explicit context (revival, explicit resume,
  restart restore) keeps the context its session last persisted.
- `mob/spawn_many` returns one row per spec, `{status: "spawned" | "failed",
  result}`. A failed `result` carries `cause`, `message`, an optional
  canonical `code` and optional `structured_data`.
- A spawn still preparing or provisioning when stop, complete, reset,
  destroy, shutdown or the member's retirement happens fails with
  `MobError::SpawnCanceled` ("spawn canceled for '<id>': <reason>"); nothing
  of it was seated.
- Helpers from `spawn-helper`, `fork-helper` and `delegate` run one task,
  return a bounded result and are retired, also when their caller goes away.
  A completed `fork_off` child stays until its forker retires it; a failed one,
  or one stopped by `max_run_secs`, is retired automatically.
- Member status (`rkat mob member-status`, RPC `mob/member_status`,
  `mob_check_member`) never waits for a running turn. `progress.run_state` is
  `idle`, `run_open` or `unknown`; `preview_unavailable` means the preview is
  missing, not empty.
- Persistent realms keep mob state (definition, roster, events, runs)
  durably, but members execute only inside a live host process: `rkat run
  --keep-alive`, `rkat-rpc`, `rkat-rest`, `rkat-mcp`, or `rkat mob deploy
  --surface rpc`. One-shot `rkat mob` commands load the mob, act and exit.
- Two member states need the Rust-only `reload_member_registration` repair;
  both answer `retryable: false` with `required_action:
  "reload_member_registration"`. A durability-degraded registration fails
  with `MEMBER_RELOAD_REQUIRED` (-32029, HTTP 409, data kind
  `mob_member_reload_required`). A member whose session has no committed
  runtime attachment (detached) fails as invalid params with data kind
  `mob_member_runtime_detached` (Rust `MobError::MemberRuntimeDetached`).
- In `rkat-rpc`, members reach the callback tools of the connection that
  holds the mob's callback route: the one whose `mob/create` created it, or
  the one that claimed it with `mob/claim_callback_route` and the mob's claim
  token (`mob/create` returns it once, to the creator). A mob restored after
  a restart, or whose connection closed, has no route until a connection
  claims it. A claim with another token (`claim_token_mismatch`), for a mob
  created without one (`not_claimable`), or while the holding connection is
  open (`held_by_open_connection`) is refused (`-32002`).

## Helpers, forks and councils

- `rkat mob spawn-helper` and `fork-helper` require `--agent-identity`,
  `--result-label` and `--max-text-bytes`; every surface requires
  `result_label` and `max_text_bytes`, and results carry `bounded_result
  {label, status, text}`.
- `fork_off` (agent tool) takes `member_id`, `task`, `expected_output`,
  `message_count`, `result_label` (default `fork_off_result`),
  `max_text_bytes` (default 16384) and `max_run_secs` (at least 1, no
  default). On long-lived hosts it returns `status: "running"` with a
  `job_id`, and the outcome arrives later as a "Background fork_off job ...
  finished" notice. One-shot hosts block and return `blocked_because`.
- `council` (agent tool): `topic` plus up to 8 `participants`, each
  `{mob_id, member_id, role}`. Bounds: `max_rounds` 1 to 16 (default 2),
  `max_exchanges` 1 to 64, `max_result_bytes` 256 to 65536 (default 8192),
  and `timeout_seconds` (default 300). `council_id` allows only ASCII
  letters, digits, `-` and `_`. Like `fork_off`, it usually returns `status:
  "running"` with a `job_id`, and the result arrives as a "Background council
  job ... finished" entry.
- The agent loop's default tool deadline does not cut `fork_off` or
  `council`; an explicit `tools.tool_timeouts.<tool>` still does.
- Since 0.9.4 forking a member with activated host instructions follows the
  host's fork instruction policy (`MobBuilder::with_fork_instruction_policy`).
  By default `fork_off`, the self-starting member forks and council
  participants are refused before commit (`fork_instruction_refused`, -32037;
  a council ends `participant_fork_instruction_refused`) instead of
  committing a child whose first turn failed; `reactivate_inherited`
  re-activates the source's keys in the child. Upgrade member hosts before
  their controller. Details in `references/mobs.md`.
- `delegate` and `mob_spawn_member` accept `tooling`: `{mode:
  "inherit_parent"}` (default, with optional `allow_overlay` and
  `deny_overlay`), `{mode: "minimal"}` (comms tools only) or `{mode:
  "profile", source: ...}`, capped by the caller's visible tools. They also
  accept an optional `placement`. Host-only fields such as `mcp_servers` and
  `rust_bundles` are refused, and managed hosts without a child tool policy
  refuse `mob_create` and `delegate` (`child_tool_policy_required`).
- Since 0.9.4 a child mob's inline profile (a mob a member creates with
  `mob_create`) can mount a tool bundle the host offers to child mobs by
  naming it in `tools.bundles`. A name the host does not offer is refused
  (`ToolBundleNotOffered`, which lists the offered names). The opt-in persists
  with the definition; a member whose host no longer offers the bundle is
  built without it (`MobHandle::withheld_tool_bundles` reports it).
- `tooling` (any mode) needs a source identity for every tool the child
  keeps. Tools from a named Rust tool bundle get one automatically (0.8.52+);
  external tools a host mounts outside a bundle need `ToolDef.provenance` or
  must be filtered out, otherwise the handoff is refused.
- Fork-derived members (`fork_off` children, `MobHandle::fork_member` forks
  and local council participants) are built from their source's build
  inputs: its application context, application labels and per-spawn tool
  overlay, so they get the same tools and can reuse the source's cached
  prompt prefix. Host build callbacks see `fork_source` on the build.

## Profile keys

| Key | Meaning |
|---|---|
| `tools.comms = true` | Required for every spawned member |
| `tools.read_only = true` | Only tools declared read-only may execute; tools stay listed, other calls are refused `access_denied` |
| `tools.deny = [...]` | Named tools are hidden from members (0.8.52+) and refused by name; an unknown name fails the build (`DeclaredToolUnknown`) |
| `tools.bundles = [...]` | 0.9.4+: host tool bundles a child mob's inline profile opts into; only bundles the host offers to child mobs |
| `model_fallback` / top-level `[runtime] model_fallback` | Fallback model; precedence is profile, then mob runtime, then host config |
| `resume_overrides = ["model", "provider", "provider_params"]` | Profile fields that win over a resumed member's durable session |
| `external_addressable = true` | Required for `mob/member_send` and external `mob/submit_work` |

`system_prompt`, `prompt` and `instructions` under `[profiles.<name>]` used
to be dropped silently, so the member ran on the default prompt; since
0.8.34 a TOML definition refuses them (`MobError::UnsupportedProfileKey`; a
JSON definition is not inspected). The member prompt comes from `skills` and
`[skills.<id>]` tables. For extra role text use `additional_instructions` on
`mob/spawn`, or `mob/append_system_context` at run time; identity-first hosts
(Meta) set `DurableAgentSpec.additional_instructions`. Other unknown keys
only log a warning (diagnostic `unknown_profile_key` at
`profiles.<profile>.<key>`).

A member's tool restriction joins its role's current profile with the
profile it was spawned on (deny lists joined; either `read_only` applies). A
deny added to a role reaches existing members at their next rebuild (restart
restore, explicit resume, revival), not live. A removed deny stays for
members whose spawn snapshot (`override_profile`) still carries it, until
they are respawned. Turning `read_only` off takes effect when members
resume, unless their spawn snapshot still sets it.
`tool_access_policy` allow and deny lists on spawn specs also hide the
tools they block (0.8.52+). Conditional gates keep their tools listed and
refuse at call time: read-only intent, application tool (consequence) policy
(refused as `policy_denied` or `policy_indeterminate`) and dispatch
admission.

## Flows and runs

- `rkat mob run-flow <MOB_ID> --flow <FLOW_ID> [--params '<json object>']
  [-s|--stream] [--no-stream]` starts the run, streams member output when
  stdout is a terminal, then waits with no timeout until the run is
  `completed`, `failed` or `canceled`. It prints only the run id, whatever the
  outcome; read the outcome with `rkat mob flow-status <MOB_ID> <RUN_ID>`
  (MobRun JSON) or `rkat mob status <MOB_ID> <RUN_ID> [--json]`.
- `rkat mob run <PACK_OR_MOB_ID> [--flow F] [--param k=v]... [--prompt TEXT]
  [--detach] [--json]` waits and prints the result envelope with token
  `accounting` and per-member `rkat session export-atif` pointers. `--prompt`
  binds `params.prompt`, and `--param` values parse as JSON when possible. An
  installed mob defaults to flow `main`.
- `--detach` prints the run id and a warning: the flow executes only while
  that CLI process lives, and the next mob load turns an unfinished run into
  `canceled`. `rkat mob attach <MOB_ID> <RUN_ID> [--json]` waits for a run,
  `rkat mob runs <MOB_ID> [--flow F] [--json]` lists runs, and `rkat mob logs
  <MOB_ID> [--after-cursor N] [--limit N] [--json]` reads events.
- RPC: `mob/flows`, `mob/flow_run` (or `mob/run`), `mob/flow_status`,
  `mob/run_result`, `mob/flow_cancel`; SDK `Mob.run_flow`, `run`,
  `flow_status`, `run_result`, `cancel_flow` (TypeScript camelCase). A run
  spawns a turn-driven `__flow_<role>_<flow>` member for any step role that
  has no runnable member.
- Step messages template `{{ params.<path> }}`, `{{ steps.<id>.output.<path> }}`
  and `{{ loops.<loop>.iterations.<n>.steps.<id>.<path> }}`. Nested output
  lookups need `output_format = "json"` on the producer step.
- `failure_policy = "continue"` on a step, frame node or `repeat_until` node
  makes it advisory: the run can still complete, but dependents of the failed
  node are skipped unless another dependency completes. The default is
  `escalate`.

## Multi-host mobs

```bash
# On the member host (own durable realm; --isolated is rejected):
rkat --realm worker-west mob host --listen-tcp 0.0.0.0:4300 \
  --advertise-tcp tcp://worker-west.example.com:4300 --allow-remote \
  --descriptor-out ./worker-west.host.json
# On the controller:
rkat --realm team mob bind-host <MOB_ID> --descriptor ./worker-west.host.json
rkat --realm team mob hosts <MOB_ID> --json
```

- `rkat mob host` defaults: `--listen-tcp 127.0.0.1:0`, descriptor
  `./host-binding.json` (mode 0600, single-use bootstrap token), identity dir
  `<state-root>/host-identity`. A non-loopback bind needs `--allow-remote`
  and `--advertise-tcp tcp://HOST:PORT`. Pairing (`--pairing-password-env` or
  `--pairing-password-file`) is accepted only on a loopback listener.
- Config `[mob_host]` mirrors the flags (`listen_tcp`, `advertise_tcp`,
  `live_ws`, `live_ws_advertise`, `identity_dir`, `descriptor_out`) plus
  bounds such as `max_connections`. A controller with both local and placed
  members also sets `listen_tcp` and `advertise_tcp` so hosts can reach it.
- Spawn a placed member with `placement: "<HOST_ID>"` on `mob/spawn`,
  `mob/spawn_many`, `delegate` or `mob_spawn_member`. Placement needs a
  portable spec: Rust tool bundles (`tools.bundles` opt-ins too), WorkGraph
  tools, shell or MCP secrets in env or headers are refused.
- Also: `rkat mob revoke-host`, `member-history` and `route-installs`
  (pending external trust installs show as `outstanding_external`).
- Since 0.9.4 a recorded placed lifecycle intent (such as a Reset) is not
  retried while the mob's admission is closed (0.9.3 and earlier retried it
  every 2 s with a warning); it stays recorded and the next start drives it.
- Grants for delegated principals: `rkat mob grant <MOB_ID> <PRINCIPAL>
  --scope <SCOPE>... [--expires-at-ms N | --expires-in 30m]`, `revoke-grant`
  (no `--scope` revokes all) and `grants`. The scopes are `list`,
  `read_history`, `subscribe_events`, `send_command`, `cancel`, `retire`,
  `wire_topology`, `live`, `admin_host` and `admin_grants`.
- JSON-RPC and the Python and TypeScript SDKs carry the full host, grant,
  hard-cancel and member-live family; REST and public MCP only read hosts,
  route installs and member history. See the multi-host surface matrix in
  `references/mobs.md`. Web/WASM is single-host.

## Mobpacks

```bash
rkat mob pack ./mobs/triage -o ./dist/triage.mobpack [--sign KEY --signer-id ID]
rkat mob inspect ./dist/triage.mobpack
rkat mob validate ./dist/triage.mobpack [--trust-policy permissive|strict]
rkat mob run ./dist/triage.mobpack --prompt "triage" --trust-policy permissive
rkat mob deploy ./dist/triage.mobpack "triage latest incidents" [-m MODEL] [--max-total-tokens N] [-d 15m] [--max-tool-calls N] [--surface cli|rpc]
rkat mob web build ./dist/triage.mobpack -o ./dist/web --wasm <PKG_DIR|name_bg.wasm>
```

- Trust policy: `--trust-policy`, then `RKAT_TRUST_POLICY`, then realm config
  `[trust] policy`, then `strict`. Strict rejects unsigned packs and unknown
  signers; permissive accepts them with warnings but still rejects bad
  signatures. Signing does not install the signer.
- A pack directory needs `manifest.toml` (`[mobpack] name`, `version`) and
  `definition.json`; optional `skills/`, `hooks/`, `mcp/`,
  `config/defaults.toml`, `adaptive/` and `schemas/`. Manifest `surfaces` take
  `cli`, `rpc`, `rest`, `mcp` and `web`.
- `config/defaults.toml` may set only `max_tokens`, `[models]`, `[budget]`
  and `[compaction]` knobs, as defaults under operator config.
- `rkat mob run <pack>` validates `--param` input against
  `schemas/main-input.json` when present, creates the mob in the realm's
  durable store, and picks `--flow`, else `main`, else the pack's callable,
  else its only flow. `--detach` needs a callable flow.
- `rkat mob deploy` keeps the mob in memory. `--surface cli` (default) queues
  the prompt to the orchestrator and returns, and the mob ends with the
  command; `--surface rpc` serves JSON-RPC on stdin/stdout until the stream
  closes and does not start the realm's MCP servers (it warns), so members
  get no realm-configured MCP tools. Without an orchestrator, deploy warns
  that nothing was started.
- Since 0.9.1 `rkat mob deploy
  --max-total-tokens/--max-duration/--max-tool-calls` and a pack's `[budget]`
  set each member's `[limits]` (`budget`, `max_duration`, `max_tool_calls`)
  on the default `--surface cli` and for `rkat mob run`; a flag wins over the
  realm's `[limits]`, which wins over the pack default. 0.9.0 recorded them
  but did not enforce them.
- Since 0.9.3 the `rkat mob deploy --surface rpc` host builds every member
  and session from the same deploy config, so deploy flags and pack overlays
  (budget, models, `max_tokens`, compaction, `--model`) apply there too, as
  defaults: an explicit `session/create` or mob spawn parameter replaces them.
- `rkat mob web build` needs prebuilt wasm-pack `--target web` output via
  `--wasm` and writes `index.html`, `meerkat-bootstrap.js` (`bootMobpack`),
  `mobpack.bin`, `manifest.web.toml` and the wasm files. Booting initializes
  the runtime with the pack but creates no mob.

## Error codes

| Code | JSON-RPC | HTTP | Meaning |
|---|---|---|---|
| `SESSION_BUSY` | -32002 | 409 | Retry later (also a full 256-entry member lane, a retirement or teardown in progress, a Shutdown draining: `mob_shutdown_in_progress`) |
| `CAPABILITY_UNAVAILABLE` | -32020 | 501 | Feature not available here |
| `SUPERVISOR_ROTATION_INCOMPLETE` | -32024 | 409 | Finish supervisor rotation |
| `SCOPE_DENIED` | -32025 | 403 | Principal lacks a control scope |
| `HOST_UNAVAILABLE` | -32026 | 503 | Member host unreachable |
| `STALE_CURSOR` | -32027 | 410 | Re-read events from a fresh cursor |
| `STALE_FENCE` | -32028 | 409 | Re-resolve the member |
| `MEMBER_RELOAD_REQUIRED` | -32029 | 409 | Registration durability-degraded; needs `reload_member_registration` (Rust); since 0.9.4 also from `mob/force_cancel` and `cancel_all_work` |
| `SESSION_RUNTIME_UNAVAILABLE` | -32032 | 503 | Runtime not available (for example a held child mob) |
| `STALE_DELIVERY_SCOPE` | -32036 | 409 | Member session moved; re-capture the delivery scope |

`rkat mob` commands exit 45 (`SCOPE_DENIED`), 46 (`HOST_UNAVAILABLE`), 47
(`STALE_CURSOR`), 48 (`STALE_FENCE`) or 56 (`STALE_DELIVERY_SCOPE`) and print
one `detail: {...}` JSON line on stderr. Since 0.9.1 the other typed mob
errors exit with their codes too: 49 (`MEMBER_RELOAD_REQUIRED`), 44
(supervisor protocol upgrade required), 11 (retirement, lifecycle or admission
pending, reload refused or timed out, actor command timeout), 40 (capability
or delivery scope unavailable) and 52 (held before activation, runtime
unavailable); `rkat mob run <pack>` follows the same codes. Other failures
exit 1 (in 0.9.0, everything outside 45-48 and 56 exited 1).

## Removed or renamed

- `mobkit/jobs/*`, `mobkit_job_*` and `mobkitJob*` were renamed in 0.9.0 to
  `meta/jobs/*`, `meta_job_*` and `metaJob*`, with no aliases.
- Prefab mob templates were removed in 0.5.1; use `MobDefinition`. There is
  no generic `mob/tools` or `mob/call` method; use the typed `mob/*` methods.
- There are no `rkat mob create`, `spawn`, `wire`, `turn`, `stop` or
  `destroy` verbs; use agent `mob_*` tools or RPC `mob/*`.
