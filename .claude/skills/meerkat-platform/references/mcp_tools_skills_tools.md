Covers: Agent tools in Meerkat 0.9. Built-in tool families and exact names
(shell, apply_patch, view_image, blob, task, web_search, WorkGraph rules), CLI
`--tools` presets (`none` is not a ban), `[shell]`/`[tools]` config, tool error codes,
filters and overlays (`--allow-tool`, `tool_filter`, `turn_tool_overlay`),
`tool_access_policy` (deny hides; provider-native tools off), mob profile `deny`,
streaming tool deadlines, and callback tools (RPC `tools/register`, `rkat-mcp` resume).

# Tools (0.9)

Names are exact: anything not listed does not exist. "(Rust host)" marks
Rust-embedder-only APIs. MCP servers and the deferred tool catalog are in
`mcp_tools_skills_mcp.md`; skills are in `mcp_tools_skills_skills.md`.

## Quick answers

- Give the CLI agent a shell: `rkat run --tools workspace "..."` (`--tools full`
  adds WorkGraph, memory and mob tools).
- Read or edit files: there are no `read_file`/`write_file` tools; the agent
  uses `shell` to read and `apply_patch` to edit.
- `access_denied` "Tool 'X' is not allowed by policy": a tool hidden by an
  allow/deny policy, a read-only policy, or a deferred tool that was not loaded
  with `tool_catalog_load` (MCP and callback tools; see the MCP file).
- Restrict tools for one CLI turn: `--allow-tool <TOOL>` / `--block-tool <TOOL>`.
- `--tools none` is not a tool ban: schedule tools, comms, image generation,
  MCP and provider-native web search can remain (see the presets below).

## Built-in tools

| Family | Tools | Enabled by |
|---|---|---|
| Builtins | `task_list`, `task_create`, `task_get`, `task_update`, `datetime`, `apply_patch`, `view_image`, `blob_save_file`, `blob_load_file`, `blob_inspect` (blob tools need a blob store) | CLI `--tools safe` and up; `enable_builtins`; `tools.builtins_enabled` |
| Shell | `shell`, `shell_jobs`, `shell_job_status`, `shell_job_cancel` (`monitor_start` only by explicit host policy) | CLI `--tools workspace` or `full`; `enable_shell`; `tools.shell_enabled` |
| Image generation | `generate_image` | Visible when the runtime wires it (default CLI, RPC, REST) |
| Web search | Provider-native search, or the `web_search` fallback | Native is on where the model supports it, unless `--no-web-search` or a restricting tool policy turns it off; fallback only with `enable_web_search: true` |
| Memory | `memory_search` | `--tools full`; `enable_memory` |
| Schedule | `meerkat_schedule_*` | `tools.schedule_enabled` (default true); `enable_schedule` |
| WorkGraph | `workgraph_*` | `--tools full`; `enable_workgraph`; `tools.workgraph_enabled` |
| Comms | `send_message`, `reply_to_peer`, `send_request`, `send_response`, `peers` | The comms runtime (see the comms guide) |
| Mob | `delegate`, `conclude_objective`, `mob_*` | `--tools full`; `enable_mob`; `tools.mob_enabled` |
| Skills | `browse_skills`, `load_skill`, `skill_list_resources`, `skill_read_resource`, `skill_invoke_function` | Off; only an explicit host tool policy enables them |
| Model switch | `brain_swap {target_model}` | Only when the host can switch and more than one model is reachable |

There are no `read_file`, `write_file`, `grep` or `glob` tools: read files
with `shell`, edit them with `apply_patch`.

CLI `--tools` (default `safe`; `--yolo` = `full`): `safe` = builtins;
`workspace` = builtins + shell; `full` = builtins + shell + WorkGraph, plus
memory and mob tools when compiled in; `none` = neither. For a new run the
preset decides builtins and shell whatever `tools.builtins_enabled` or
`tools.shell_enabled` say; WorkGraph and mob are also on when their config key
is. `rkat run --resume` keeps the stored choices unless `--tools`/`--yolo` is
given. `none` only turns those families off: schedule tools still follow
`tools.schedule_enabled` (default true), WorkGraph and mob tools their config
keys, comms its own options (for example `--comms-name`; `--no-comms` turns it
off), `generate_image` stays where wired, configured MCP servers still load,
and provider-native web search stays unless `--no-web-search`.

Shell:

- Arguments: `command`; `working_dir` (relative to the project root, must stay
  inside it); `timeout_secs` (at least 1); `background` (offered only where
  durable background jobs exist).
- A call without `timeout_secs` gets `[shell] timeout_secs` (default 30 s; 0
  keeps 30 s) since 0.9.1. In 0.9.0 `[shell] timeout_secs` was read but not
  applied, so every such call got 30 s.
- `[shell]`: `program` (default `nu`, falling back to `$SHELL`, `bash`, `zsh`,
  `sh`), `security_mode` (`unrestricted` default, `allow_list`, `deny_list`),
  `security_patterns` (globs matched against the whole command line;
  `allowlist = [...]` implies `allow_list`), `max_output_chars` (40000).
- Output is one text block: a status line (`exit code N (Xs)` or the
  timeout), stdout, then `[stderr]` when non-empty. Stdout over the cap keeps
  head and tail around a marker with a `sed -n 'A,Bp'` range; stderr gets half
  the cap. Commands read stdin from `/dev/null`. `shell_job_status` is also
  one compact text block (job state in the text). Since 0.8.43 neither has
  JSON fields such as `exit_code` or `stdout`.
- `background: true` submits a durable realm job and returns a receipt; the
  result arrives later as a notice. It needs a persistent realm (job and blob
  stores); elsewhere it fails: "detached shell execution requires a durable
  realm job/blob runtime; process-local background execution is not
  available". A shell `&` is not a background job.
- Session param `shell_env` (RPC, REST, SDK) sets env vars for shell commands;
  the model never sees them. `shell` is classed mutating, never read-only.
- Only `working_dir` is checked, which is not a sandbox. OS launch
  confinement is off by default: the stock binaries turn it on with the
  `[confinement]` config section or the `--confinement` flags, a Rust host
  with `AgentFactory::with_shell_confinement` (and
  `with_mcp_stdio_launch_profile` for stdio MCP servers). A refused confined
  launch is `confinement_refused`.

Others: `apply_patch {patch}` takes `*** Begin Patch` ... `*** End Patch` with
`*** Add File:`, `*** Update File:` (optionally `*** Move to:`) and `*** Delete
File:` sections, paths relative to the project root. `view_image {path}` reads
PNG, JPEG, GIF, WebP or SVG up to 5 MB inside the project root, and is hidden
when the model cannot take image tool results. `blob_save_file {blob_id, path,
overwrite?}`, `blob_load_file {path, media_type?}` and `blob_inspect {blob_id}`
move blobs to and from project files. `web_search {query, provider?,
context?}` is the fallback: offered only on an explicit enable when native
search is not active (the model lacks it, or provider-native tools are off).

Tool errors reach the model as `{"error": "<code>", "message": ...}` and the
run continues. Codes: `tool_not_found`, `tool_unavailable`,
`invalid_arguments`, `execution_failed`, `timeout`, `inactivity_timeout`,
`access_denied`, `outcome_uncertain`, `operation_refused`,
`confinement_refused`, `hook_denied`, `hook_launch_refused`,
`operation_observation_unavailable`, `operation_authorization_unavailable`,
`review_unsatisfied`, `review_unavailable`, `policy_denied`,
`policy_indeterminate`, `callback_pending`, `tool_error`. Since 0.9.4 an
`operation_refused` result also carries `data: {"kind": "reprepare_required"
| "denied" | "malformed_facts", "tool_invoked": false | "unknown"}`; neither
field permits a retry, and a new call is a new operation.

## Tool modes, filters and overlays

- Visibility changes are staged and applied at the next turn boundary
  (`tool_config_changed` event plus a notice to the model). Allow lists
  intersect, deny lists union, and deny wins.
- CLI `--allow-tool <TOOL>` and `--block-tool <TOOL>` (repeatable) restrict
  only the turn the command runs; they cannot add tools that were not composed.
- RPC `session/create` `tool_filter`: `"All"`, `{"Allow": ["a", "b"]}` or
  `{"Deny": ["c"]}` (TypeScript `toolFilter`): sticky session visibility.
- Per-turn `turn_tool_overlay {allowed_tools?, blocked_tools?,
  tool_choice_plan?}` on RPC `turn/start`, REST `POST /sessions/{id}/messages`
  and `mob/turn_start`; Python `turn_tool_overlay=` on `Session.turn` and
  `stream`; TypeScript `turnToolOverlay: {allowedTools, blockedTools,
  toolChoicePlan}`.
- `tool_choice_plan` entries: `{"mode": "auto"}`, `{"mode": "required"}`,
  `{"mode": "none"}` or `{"mode": "tool", "name": "<tool>"}`. Entry k applies to
  the run's k-th model call, later calls are `auto`. A choice the model cannot
  honor is an error (`ToolChoiceUnsupported`), never silently downgraded.
- Session-create switches (RPC, REST, SDKs): `enable_builtins`,
  `enable_shell`, `enable_memory`, `enable_schedule`, `enable_workgraph`,
  `enable_mob`, `enable_web_search` (omit to use the host default).
- `[tools]` config: `builtins_enabled`, `shell_enabled`, `comms_enabled`,
  `mob_enabled`, `workgraph_enabled` (default false), `schedule_enabled`
  (default true), `default_timeout` ("600s"), `tool_timeouts.<tool>`,
  `max_concurrent` (10), `mcp_servers`. There is no `tools.allow` or
  `tools.deny` in `[tools]`: deny lists live in mob profiles and launch
  policies.
- (Rust host) `ToolScopeHandle` stages external filters on a live session.

## Tool access policy and profiles

Visibility decides what the model is offered; the execution gate decides what
may run. A refused call is an ordinary `access_denied` tool error.

- `tool_access_policy`: `{"type": "inherit"}`, `{"type": "allow_list",
  "value": [...]}`, `{"type": "deny_list", "value": [...]}` or `{"type":
  "read_only"}` (Rust also has `Constraints`). Accepted on RPC `mob/spawn`,
  `session/fork_at` and `session/fork_replace` (Python
  `tool_access_policy=`, TypeScript `toolAccessPolicy`), and on Rust spawn
  specs, `HelperOptions` and `AgentBuildConfig`. `inherit` takes the parent's
  policy; a child can only narrow it.
- Allow and deny lists hide the excluded tools from the model (since 0.8.52)
  and the gate refuses any call that still reaches one. `read_only` leaves the
  list unchanged and refuses, at call time, every tool whose owner does not
  declare it read-only: MCP tools, `shell` and undeclared host tools are
  refused; `tool_catalog_search` and `tool_catalog_load` are allowed.
- An application tool (consequence) policy and dispatch admission are separate
  gates: their tools also stay visible and are refused at call time
  (application policy refusals are `policy_denied` or `policy_indeterminate`).
- Provider-native tools (for example native web search) never pass the gate,
  so nothing extra is needed: the build turns every provider-native tool off
  under any `allow_list`, `deny_list` or `read_only` policy, a governing
  application tool policy, dispatch admission, or a `copilot` backend. With
  `enable_web_search: true` the model then gets the ordinary `web_search` tool,
  which the policy governs like any other.
- Mob profile `[profiles.<name>.tools]` keys: `builtins`, `shell`, `comms`
  (must be true), `memory`, `workgraph`, `mob`, `schedule`,
  `image_generation`, `read_only`, `deny`, `mcp`, `mcp_servers`,
  `rust_bundles`. There is no `allow` key. `deny` hides and refuses the named
  tools; each name must be a composed tool, a built-in family tool, a mob tool,
  an MCP `tool_names` entry or a bundle tool, or the build fails with
  `DeclaredToolUnknown` (unknown names are left to the call-time gate while the
  profile has an MCP server without `tool_names`). `deny` and `read_only` are
  re-applied on every build, resume included; a spawn cannot widen them.
- Profiles supplied by a model through agent mob tools cannot set
  `mcp_servers`, `rust_bundles` or host-path skills.

## WorkGraph tools

`workgraph_create`, `workgraph_get`, `workgraph_list`, `workgraph_ready`,
`workgraph_snapshot`, `workgraph_events`, `workgraph_claim`,
`workgraph_release`, `workgraph_update`, `workgraph_block`, `workgraph_close`,
`workgraph_link`, `workgraph_add_evidence`; `workgraph_policy_escalate` and
`workgraph_attention_reassign` only with a runtime-bound attention authority.

- `workgraph_create` has no parent field. Link a child with
  `workgraph_link {kind: "parent", from_id: <child>, to_id: <parent>}` right
  after creating it. Edge kinds: `blocks` (`from_id` blocker, `to_id`
  blocked), `parent`, `related`, `supersedes`, `derived_from`. Edges are
  permanent (no unlink).
- A `blocks` target is ready only once the blocker is `completed`; a `failed`
  or `cancelled` blocker keeps it blocked.
- A parent is not ready while any child is live. Its `failed_child_join_policy`
  and `cancelled_child_join_policy` (set on `workgraph_create`) decide what a
  failed or cancelled child does: `require_success` (default) keeps the parent
  waiting, `accept` lets it proceed without that child, `propagate` closes it
  with the child's status.
- Claim, update, release, block, close and add-evidence calls require the
  item's current `expected_revision`; each success advances it. On a stale
  revision, reload with `workgraph_get` and retry only if the work still makes
  sense.
- `workgraph_close` without `status` records `completed`: pass `failed` or
  `cancelled` explicitly. `workgraph_list` and `workgraph_snapshot` hide
  terminal items unless `include_terminal: true`; a `labels` filter matches
  items carrying every listed label.

## Streaming tools (Rust host)

A host dispatcher can declare a tool streaming: a `ToolExecutionContract` with
`ToolExecutionMode::Streaming` and `StreamingToolExecutionPolicy::new(inactivity,
absolute)` (both above zero, inactivity at most absolute). RPC callback tools
have no streaming mode.

- While running, the tool reports liveness through its dispatch context's
  `streaming()` handle: `progress().try_report(frame)` with a
  `ToolProgressFrame` (a `message`, or `completed_units` with `total_units`
  above zero); `cancellation()` is its cancellation token.
- Each accepted frame restarts the inactivity timer; a malformed,
  backpressured or closed report does not. Frames are liveness only.
- No accepted frame within the inactivity timeout: the call fails
  `inactivity_timeout` ("Streaming tool '<name>' stalled after <N>ms of
  inactivity"). The absolute deadline (the smallest of the declared absolute
  timeout and the call's other deadlines, such as
  `tools.tool_timeouts.<tool>`) fails it `timeout` even while frames arrive.
  Either way the tool's cancellation token fires.

## Callback tools

RPC (the client answers inline):

- RPC `tools/register {tools: [{name, description, input_schema,
  execution?}]}` returns `{registered: N}`. During a turn the server sends
  `tool/execute {tool_use_id, name, arguments, dedupe_key?}`; reply
  `{content, is_error}` within 120 s or the call fails `timeout`.
  `dedupe_key {run_id, round, call_id, call_digest}` (0.9.5) rides every call
  of a run's tool round; a durable run continued after a restart sends the
  call again with the same key (`references/sessions_runtime_surfaces_sessions.md`,
  Durable runs).
- Registration is per connection; a session's callbacks go to the connection
  that created it and fail `tool_unavailable` once it is gone.
- `execution` is `{"mode": "fast"}` (default) or `{"mode": "detached", ...}`
  for durable job-backed callbacks (see the durable jobs guide).
- Python `@client.tool(name, description=..., input_schema=...)`; TypeScript
  `client.registerTool(name, description, inputSchema, handler)`. RPC
  `session/create` also takes `external_tools`. Since 0.9.5 a Python handler
  that declares a `dedupe_key` parameter gets the call's
  `ToolCallDedupeKey` (`None` without one); a TypeScript or Web handler gets
  it as an optional second argument.
- Mob members use the callbacks of the connection that holds the mob's
  callback route: its `mob/create` connection, or the one that claimed it
  with `mob/claim_callback_route` and the claim token `mob/create` returned
  to the creator. A restored mob has no route until a connection claims it;
  until then a member's callback fails `tool_unavailable`. A mob not made by
  `mob/create` has no claim token, so no connection claims its route unless
  its host mints one.
- Give input schemas an object root; a root that declares another `type` is
  refused with an error naming the tool. For Anthropic, a root `oneOf`,
  `anyOf` or `allOf` of plain object schemas is folded into one object.

`rkat-mcp` (the turn suspends and resumes):

- `meerkat_run` and `meerkat_resume` take `tools: [{name, description,
  input_schema, handler?}]`; `handler` defaults to `"callback"`.
- When the model calls one, the turn stops and the result is
  `{"status": "pending_tool_call", "session_id": ..., "resumable": true,
  "pending_tool_calls": [{"tool_use_id", "tool_name", "args"}], "content":
  [...]}` (text "Agent is waiting for tool results"). Parallel callback calls
  arrive together in one list, each keyed by its `tool_use_id`; since 0.9.5
  an entry also carries the call's `dedupe_key` when it has one.
- Resume with `meerkat_resume {session_id, prompt: "", tools: <same tools>,
  tool_results: [{tool_use_id, content, is_error?}]}`. The ids must be exactly
  the pending set: a mismatch, a duplicate or a conflicting redelivery is
  refused as invalid params (for example `callback result ids ... do not
  match pending ids ...`). Re-sending results that were already applied
  returns `{"status": "callback_results_already_accepted", "session_id": ...,
  "resumable": true}` and runs nothing.
- `rkat-mcp` sets no deadline on a suspended turn; the 120 s limit is the RPC
  `tool/execute` one.
- REST: a turn that stops on a callback tool fails with HTTP 500 and
  `code: "CALLBACK_PENDING"`; `details` carries `session_id`,
  `session_ref`, `session_created`, `resumable: true`, `tool_name`, `args` and
  `pending_tool_calls`.
