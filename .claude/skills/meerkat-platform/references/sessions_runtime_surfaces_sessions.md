Covers: Meerkat 0.9 sessions on every surface (CLI, JSON-RPC, REST, Python,
TypeScript): create, continue, `rkat run --resume`, list, read, history and
summaries, archive; per-turn `turn/start` fields and refused build settings;
turn queueing and the 3-attempt retry cap; interrupt versus `turn/stop_run`;
forks, transcript edits and retention; system prompts; events (0.9.4
content-filtered `run_failed` reason, compaction summary source), cursor
replay, ATIF export; durable runs (0.9.5 per-round commits, same-run resume,
tool reruns and the dedupe key), durable resume holds, repair-wholeblob,
tool-process recovery.

# Sessions on Every Surface (Meerkat 0.9)

Names are exact and checked against the 0.9.0 source. Storage, runtime,
WorkGraph, schedules and config: `references/sessions_runtime_surfaces_storage_config.md`.
API changes, error codes, SDKs, prebuilt binaries and Rust embedding:
`references/sessions_runtime_surfaces_api_changes.md`.

## Session lifecycle by surface

| Operation | CLI | JSON-RPC | REST | Python | TypeScript |
|---|---|---|---|---|---|
| Create and run | `rkat run "<prompt>"` (or `rkat "<prompt>"`) | `session/create` | `POST /sessions` | `client.create_session(prompt, ...)` | `client.createSession(prompt, opts)` |
| Create without running | none | `session/create` with `"initial_turn": "deferred"` | none | `create_deferred_session(...)` | `createDeferredSession(...)` |
| Continue | `rkat run --resume <target> "<prompt>"` | `turn/start` | `POST /sessions/{id}/messages` | `Session.turn(...)` | `Session.turn(...)` |
| Interrupt current turn | `rkat session interrupt <id>` | `turn/interrupt` | `POST /sessions/{id}/interrupt` | `Session.interrupt()` | `Session.interrupt()` |
| Stop one exact run | none | `turn/stop_run` | `POST /sessions/{id}/runs/{run_id}/stop` | `Session.stop_run(run_id, reason=...)` | `Session.stopRun(runId, reason)` |
| Read state | `rkat session show <id>` | `session/read` | `GET /sessions/{id}`, `GET /sessions/{id}/status` | `read_session(id)` | `readSession(id)` |
| History | none | `session/history` | `GET /sessions/{id}/history?offset=&limit=` | `read_session_history(id, offset=, limit=)` | `readSessionHistory(id, {offset, limit})` |
| List | `rkat session list [--limit N] [--offset N] [--label K=V]` | `session/list` | `GET /sessions?limit=&offset=&label=k%3Dv` | `list_sessions(labels=, limit=, offset=)` | `listSessions({labels, limit, offset})` |
| Archive | `rkat session delete <id>` | `session/archive` | `DELETE /sessions/{id}` | `Session.archive()` | `Session.archive()` |
| Fork | none | `session/fork_at`, `session/fork_replace` | none | `Session.fork_at(...)`, `fork_replace(...)` | `Session.forkAt(...)`, `forkReplace(...)` |
| Export ATIF | `rkat session export-atif <id> [--output FILE]`, `rkat run --export-atif` | `session/export_atif` | none | `export_session_atif(...)` | `exportSessionAtif(...)` |

- `rkat session list` defaults to `--limit 20`; repeated `--label` filters
  must all match (the same AND rule applies to RPC `labels` and REST `label`).
- `rkat session delete` archives the session (it prints `Deleted session:` and
  the `Session Ref:`); archive is a visibility change, persistent history stays
  readable.
- `GET /sessions/{id}/status` returns `{"state": ...}` with one of
  `initializing`, `idle`, `attached`, `running`, `retired`, `stopped`,
  `destroyed`.
- One run executes at a time. A `turn/start` (or REST message) sent during a
  run is queued first in, first out and runs next; the call returns when that
  turn completes. `SESSION_BUSY` (-32002) means the session cannot take the
  request now: it is still being materialized, served by another process, or
  tearing down.
- A `turn/start` whose run's durable outcome is unknown (since 0.9.4 a
  boundary commit with no answer within the store's deadline, 120 seconds for
  SQLite) or whose boundary commit was rolled back fails with `-32603` and
  `data.reason` `completion_authority_unavailable`. The input stays durably
  admitted: read it with `session/input_status` after the session reloads
  instead of resubmitting it (see `references/sessions_runtime_surfaces_storage_config.md`).
- `INPUT_NOT_READY` (-32031) means the runtime did not admit the input now;
  `error.data.reason.kind` says why. Since 0.9.4 the kind
  `original_admission_pending` means the input names an original whose
  source owner has not settled its admission yet: retry later (0.9.3 and
  earlier refused it as malformed or denied).
- A turn that keeps failing is attempted back to back at most 3 times in
  total within one wake (about three times the model spend), then its input
  ends abandoned:
  `session/input_status` shows `terminal_outcome: "abandoned"` (Rust
  `InputAbandonReason::MaxAttemptsExhausted {attempts}`). At the cap the whole
  refused batch ends this way, so other queued inputs can show it too.
  Provider-level retries are separate (0.8.23).
- Summaries (`message_count`, `total_tokens`, `last_assistant_text` in
  `session/read` and `session/list`) are seeded when the session actor is
  created: a new session reports `message_count` of at least 1 (its system
  prompt), and a resumed or forked session reports its committed transcript
  during its first turn (0.8.45).
- Reading a session (for example `GET /sessions/{id}`) no longer waits for an
  in-flight turn to finish (0.8.51).
- `rkat run` and `rkat run --resume` wait for the session event log
  (`.rkat/sessions/<id>/events.jsonl`) to finish writing before exiting
  (0.8.43).

## Resuming from the CLI

There is no `rkat resume`, `rkat continue` or `rkat c` command; those words
fail to parse. Resume with `rkat run --resume`:

```bash
rkat run --resume last "keep going"
rkat run --resume ~2 "go back to that"    # third most recent session
rkat run --resume 019c8b99 "continue"     # UUID prefix or tail handle
rkat run --resume team-alpha:<uuid> "hi"  # realm-qualified; must match the active realm
rkat --resume "keep going"                # root shorthand, rewritten to `run --resume last`
```

A bare `--resume` resumes `last`. Since 0.9.1, when `--resume` is followed by
the run's only positional argument, that argument is the prompt and the
session is `last` (`rkat run --resume "keep going"`). A lone argument that
names a session (a UUID, `<realm>:<uuid>`, a hexadecimal handle of 8 to 36
characters, `last`, `~` or `~N`) is read as the session instead, and the
prompt is still required. Option values, such as `gpt-5.5` in
`-m gpt-5.5`, are not positional arguments.

Accepted targets: a full UUID, a UUID prefix or tail handle,
`<realm>:<uuid>`, `last`, `~`, `~0`, and `~N` (counted from the most
recently updated session).
A realm-qualified id for another realm fails with "Session belongs to realm
'X', but active realm is 'Y'. Use --realm X ...". Resume only finds sessions in
the realm the command resolves (see "Where state lives" in
`references/sessions_runtime_surfaces_storage_config.md`).

## Per-turn inputs

JSON-RPC `turn/start` (SDK `Session.turn`) accepts:

- `prompt`; `skill_refs` (the retired `skill_references` field returns a typed
  error); `additional_instructions`; `keep_alive`.
- `model`, `provider`, `self_hosted_server_id`, `provider_params` and
  `auth_binding`: per-turn LLM identity. For
  `provider_params` and `auth_binding` a plain value (or
  `{"action": "set", "value": ...}`) sets it, `{"action": "clear"}` removes
  it, and omitting it keeps the session value. Unknown fields are refused
  (`turn/start` uses `deny_unknown_fields`).
- `system_prompt` on a turn appends exactly one ordinary System message at
  that turn boundary (since 0.8.11). It does not replace earlier System
  messages. From the second turn on it sits mid-conversation, which only
  models with `supports_mid_conversation_system_messages` accept:
  claude-fable-5-1, claude-fable-5, claude-opus-5-5, claude-opus-5,
  claude-opus-4-8, claude-sonnet-5-5, claude-haiku-5-5 (0.9.1), and OpenAI
  API models except
  `gpt-live-1` and `gpt-live-1-codex`. Gemini, other Anthropic models and the
  ChatGPT-account backend refuse the request as a non-retryable invalid
  request (`InvalidInputShape`); history is not rewritten. On create, `system_prompt` is a policy: omit or `null` to inherit,
  a string to set, `{"action": "disable"}` to suppress every prompt source.
  The same per-turn append is
  `rkat run --resume <ID> --system <TEXT> <PROMPT>` (on a new run `--system`
  sets the create-time prompt), REST `POST /sessions/{id}/messages`
  `system_prompt`, MCP `meerkat_resume` `system_prompt` and RPC
  `mob/turn_start` `system_prompt`.
- `injected_context`: a list of content items stored as typed
  injected-context user rows immediately before the turn's user message;
  excluded from semantic-memory indexing.
- `transient_turn_context`: request-only text. It is kept with the pending
  input for crash retry and shown to the provider for that logical turn, but is
  never written to the transcript, compaction or extraction. It must be
  non-empty ("transient turn context must not be empty") and its whitespace is
  kept exactly. A deferred create does not accept it; attach it to the
  eventual turn.
- `turn_tool_overlay`: a per-turn tool overlay.
- Refused on every `turn/start`: `max_tokens`, `output_schema` and
  `structured_output_retries` are build settings. Sending any of them fails
  with -32602 ("Cannot override <fields> on a runtime-routed turn; set these
  at session/create time or use a deferred session"). Set them on
  `session/create` (a deferred create works too). The Python and TypeScript
  `turn` arguments with these names (`max_tokens`, `maxTokens`, ...) are
  refused the same way.

REST `POST /sessions/{id}/messages` differs: its body `session_id` must equal
the path id, `auth_binding` is a plain `{realm, binding, profile}` reference,
and it has no `provider_params` or `self_hosted_server_id`. A body that sets
`model`, `provider`, `auth_binding`, `max_tokens`, `output_schema`,
`structured_output_retries`, `hooks_override`, `enable_web_search`,
`comms_name` or `peer_meta` makes REST rebuild the session with those
settings before the turn, so REST can change build settings that RPC refuses.

## Interrupt versus stop_run

- `turn/interrupt` cancels the run in flight. Its result is
  `{"session_id", "interrupted": true|false, "result": "interrupted" |
  "staged_noop"}`. An idle session also answers `interrupted` (a successful
  no-op), so `interrupted: true` does not prove a run was cancelled.
  `staged_noop` (`interrupted: false`) is only for a deferred session whose
  first turn has not started. A session still being materialized answers
  `SESSION_BUSY`, an unknown or destroyed one `SESSION_NOT_FOUND`, and a
  runtime in a state that cannot be interrupted answers invalid request
  ("Session is not interruptible while runtime is <state>"). A cancelled
  run ends with a `run_failed` event of error class `cancelled` (0.8.49).
- `turn/stop_run` stops one exact run and terminalizes every input bound to
  it (added in 0.8.49). Take the `run_id` from the `run_started` event
  (`identity.run_id`) or from an input's `last_run_id`. Params:
  `{"session_id", "run_id", "reason"}` (unknown fields refused). REST:
  `POST /sessions/{id}/runs/{run_id}/stop` with body `{"reason": "..."}`.
- The receipt is `{"session_id", "receipt": {"outcome": ...}}` where `outcome`
  is `stopped` (with `run_id` and `contributors[{input_id, completion,
  terminal}]`), `not_current` (stale run id, with `current_run_id`; nothing is
  touched) or `not_stoppable` (a runtime stop or teardown owns the run; carries
  `state`).
- A malformed run id is invalid params on RPC and `400` on REST. REST maps a
  missing runtime to `404`; a not-ready runtime, stale authority or teardown in
  progress to `409`.
- Mob members have the same contract: `mob/stop_member_run`,
  `POST /mob/{id}/members/{agent_identity}/runs/{run_id}/stop`,
  `Mob.stop_member_run` / `Mob.stopMemberRun`.

## Forks and transcript edits

- `session/fork_at {session_id, message_index}` and
  `session/fork_replace {session_id, message_index, replacement}` create a new
  session from an idle source. `running_behavior` only accepts `reject` (the
  default): a busy source gets `SESSION_BUSY`. A fork point that would split
  a tool call from its result is refused (0.8.22). The fork result carries
  `cache_inheritance`, and forks start with zero usage (0.8.43). An optional
  `tool_access_policy` constrains the fork's calls.
- Fork children run turns since 0.9.4: every durable fork (`fork_at`,
  `fork_replace`, and mob member, turn-boundary and planned-identity forks)
  commits a child build state derived from the source's. The child keeps the
  source's model, provider, output schema, hook overrides, budget limits,
  callback tool definitions (answer them for the child's session id), comms
  settings, application context, additional instructions and shell and tool
  call timeouts; it does not get the source's `shell_env`, MCP attachments
  (attach servers to the child), mob operator authority, comms name, peer
  metadata or `keep_alive`. A source with session metadata but no build state
  is refused before anything is committed: `INVALID_PARAMS` with data
  `{"kind": "fork_source_build_state_missing", "source_session_id": ...}`. A child
  forked by 0.9.3 or earlier that never ran a turn still fails `turn/start`
  with `INTERNAL_ERROR` "turn abandoned: persisted session <id> is missing
  session build state": fork its source again.
- Fork instruction policy (0.9.4): `session/fork_at` and `session/fork_replace`
  take `inherited_instructions` (`require_explicit_activation`, the default,
  `reactivate_inherited` or `refuse`) for the instruction activations the
  child copies with the prefix (`session/activate_instruction`). An RPC fork
  child is not materialized, so nobody can activate it before its first turn:
  under the default and under `refuse`, a prefix that carries activation rows
  is refused before anything is committed with `FORK_INSTRUCTION_REFUSED`
  (-32037, HTTP 422), `data.kind` `fork_instruction_refused` and
  `data.fork_instruction_refusal` (`cause`: `policy_refuses`,
  `not_effective_in_source`, `lineage_body_budget_exceeded`, `activation` or
  `member_host_unsupported`; `keys`). `reactivate_inherited` appends the
  child's own activation of every key that was effective in the source; the
  result's `reactivated_instruction_count` counts those rows, which
  `message_count` includes. A prefix without activation rows forks the same
  way under every value. In 0.9.3 such a fork succeeded and the child's turns
  then failed. SDKs: Python `fork_session_at(..., inherited_instructions=...)`
  and `SessionForkResult.reactivated_instruction_count`; TypeScript
  `forkSessionAt(id, index, { inheritedInstructions })` and
  `reactivatedInstructionCount` (the same on the replace calls).
- Same-session edits keep the session id and are append-audited:
  `session/rewrite_transcript`, `session/transcript_revisions` (list with the
  current head), `session/transcript_revision` (read one; `"revision":
  "current"` reads the head), `session/restore_transcript_revision`.
- Rewrites may keep the `injected_context` user role, but a rewrite can never
  mint a `compaction_summary` row, and new system-prompt versions are minted
  only by `session/update_system_prompt`. Rewritten rows lose any
  `assistant_message_id` (0.8.45).
- These transcript-edit methods are JSON-RPC and SDK only. REST deliberately
  has no `rewrite-transcript` / `transcript-revisions` routes.
- Retention (0.8.51): after each compaction only the most recent
  `[compaction] history_retained_rewrites` rewrites (default `4`) keep their
  message bodies; older ones keep their commits and digests but lose their
  bodies, so the session document stays bounded. Fork, rewind, restore or
  revision reads older than the window fail with `TranscriptRevisionRetired`
  ("transcript revision X was retired with N older rewrite(s); the oldest
  retained revision is Y"). Raise the setting to keep more history.

## System prompts, instructions and injected system context

- `session/update_system_prompt` (REST `POST /sessions/{id}/system_prompt`)
  replaces one durable, versioned system-prompt key (0.8.22). Boot, resume,
  rewrite and compaction cannot mint prompt versions.
- `session/activate_instruction` activates one immutable instruction revision
  at a safe boundary; `session/instruction_activations` reads the records
  (0.8.30). Once durable instruction activations are effective, system-prompt
  replacement for that target is refused.
- `session/inject_context {session_id, content, source?, idempotency_key?}`
  (REST `POST /sessions/{id}/system_context`) appends one ordinary durable
  System message at the admitted boundary; plain text content is accepted.
- `session/input_status {session_id, selector: {"by": "input_id" |
  "idempotency_key", "value": "..."}}` returns the stored runtime state of one
  input (`state` is absent when nothing matches).

## Events, streaming and replay

- During RPC turns the server sends `session/event` notifications. For a
  standing stream call `session/stream_open {session_id}` (returns
  `stream_id`), receive `session/stream_event` / `session/stream_end`, and
  close with `session/stream_close {stream_id}`. REST streams the same events
  as SSE on `GET /sessions/{id}/events`. SDKs: `subscribe_session_events` /
  `subscribeSessionEvents`, `Session.subscribe_events()` /
  `subscribeEvents()`.
- Cursor replay: `events/latest_cursor {scope}`,
  `events/list_since {scope, cursor?, limit?}` and `events/snapshot {scope}`
  with `scope = {"type": "session", "session_id": "..."}` and
  `cursor = {"scope", "sequence"}` (sequence `0` is the empty-source cursor).
  Cursor errors are `scope_mismatch`, `ahead_of_latest` and
  `sequence_overflow`. SDKs: `latest_event_cursor`, `list_events_since`,
  `event_snapshot` (TypeScript `latestEventCursor`, `listEventsSince`,
  `eventSnapshot`).
- A slow live subscriber receives a typed `StreamTruncated` marker instead of
  silent loss; refresh from history. Event sequence numbers continue across
  actor incarnations of the same session (0.8.49).
- `run_started`, `run_completed` and `run_failed` carry an `identity` with the
  run id (0.8.44). Since 0.9.5 `run_resumed {run_id, round, resume_point}`
  marks a run that continued after a restart (see the durable runs section).
  `turn_completed` is published for every committed provider
  call, including tool-loop calls (0.8.43). Assistant events and history rows
  carry an optional `assistant_message_id`; history rows from live speech also
  carry `realtime_origin` (0.8.45). Runtime-appended `system_notice` rows
  carry `runtime_origin {session_id, run_id, input_id, append_ordinal}` in
  history, revision exports and live boundary events (0.8.44); rewrites strip
  it and content digests exclude it.
- Since 0.9.4 a `run_failed` event of error class `content_filtered` (the
  provider refused or filtered the response) carries `error_report.reason`
  `{"reason_type": "content_filtered", "stop": "refusal" | "content_filter",
  "detail": {...}}` (`detail` only when the provider gave its own account)
  instead of `null`. `compaction_completed` carries `summary_source` (who
  wrote the summary: `provider_call` with how much history it saw,
  `host_curator` or `mechanical_fallback`) and
  `estimated_history_tokens_after` (compare with `compaction_started`'s
  `estimated_history_tokens`). Shapes and SDK names:
  `references/sessions_runtime_surfaces_api_changes.md`. A 0.9.3 binary
  cannot read a session event log that holds a content-filtered `run_failed`
  line.
- ATIF export: `rkat session export-atif <id>` writes
  `<state-root>/<realm>/trajectories/<session_id>.json` unless `--output` is
  given; `rkat run --export-atif` (also with `--resume`) writes one to the
  same place after the turn (a failed export only prints a warning). Each
  provider request is its own ATIF step with metrics (tool-call turns, the
  answer, structured extraction; 0.8.43). RPC
  `session/export_atif {session_id, agent_name?, agent_version?, model_name?}`
  returns the ATIF v1.7 document; more than 500,000 events is
  `INVALID_PARAMS`, and a host without event replay refuses with "event
  replay is not enabled for this runtime host". Rust embedders need the
  facade's `atif` feature (off by default) for `meerkat::atif`.

## Durable runs: per-round commits and resume (0.9.5)

Contract and limits: `docs/guides/durable-runs.mdx`.

- Every session with a checkpointer (persistent `sqlite`, `jsonl` and
  host-store realms) commits twice per tool round, round 0 included; there is
  no switch, and ephemeral sessions never do. The first commit, after the
  model response and before any tool runs, stages the assistant message in
  the session's run cursor, not the transcript. The second, after the tools,
  writes the message and its results to the transcript together and moves the
  cursor to the next model call, so a committed transcript never ends in a
  tool call without its result. The final round (no tool calls) ends with the
  normal end-of-run save, which clears the cursor. Structured-output
  extraction takes no round commits. A failed round commit ends the run; a
  failed first commit dispatches nothing. Exception: a round whose model
  response contains an image takes only the second commit (the cursor cannot
  hold images); a crash during its tools re-asks the model for that round.
- Cost: `sqlite` (HeadCanonical) writes the round's rows; `jsonl` and
  host-store realms (WholeBlob) write the whole session document at each
  commit, so the bytes grow with the square of the tool rounds. A compaction
  keeps the commits going: on WholeBlob the run's next round commit re-bases
  it (one store call commits the document, compaction included, as the
  committed base and restarts the run's tail on it; a crash before that
  commit continues from the previous one and compacts again).
- Resume: when the session is opened again (any surface, or a mob rebuilding
  its `durable = true` workers) its open run continues under the same run id;
  no new input is needed. There is no boot sweep: a session nobody opens
  keeps its open run until its next open. A crash before the run's first
  commit runs its input again as a new run. A run that fails and whose inputs
  are requeued for another attempt discards its round commits and starts over,
  as before 0.9.5; a WholeBlob run that has re-based instead consumes its
  inputs on failure, since they are already committed.
- Event `run_resumed {run_id, round, resume_point}`: `tools_pending` (the
  saved response stands in for the model call and its calls are dispatched
  again under the same call ids, with no new assistant, tool-call request or
  turn-completed events for it) or `awaiting_model` (the next model call;
  asynchronous work the run was waiting for ended with the process, and a
  system notice tells the model those results will not arrive).
- Carried over: turn and tool-call counts, the run's usage and its elapsed
  time (the budget continues; downtime is not charged). Lost: the round in
  progress; its model call is billed but its usage is not recorded.
- A resumed run waits for its tool surface first (each MCP server up to its
  connect timeout); a tool still missing answers with an error result.
- Inputs: while a run waits to resume, no other run starts, but new inputs
  still queue and run after it. Bound inputs are the start input plus every
  steer or peer input applied at a committed round; an input applied only in
  the lost round is redelivered. A stop before the resume abandons them as
  cancelled and fails the run, which is closed as interrupted with its
  committed rounds kept; retire, destroy, reset and unregister abandon
  them; a graceful unregister that keeps queued work leaves the run to resume
  at the next start, unless a bound input is older than the stop's max age
  by then.
- Not resumed, closed as interrupted with content kept: a bound input already
  settled, a retired runtime, or a run with no bound inputs (direct and
  machine-service turns). A run settled without resuming (a stop first, a
  refused resume, a requeue stop past its max age) is closed the same way,
  never discarded: committed rounds stay, a round's staged response is
  written with an `outcome_uncertain` error result per tool call (the calls
  may have run), the interrupted notice is appended, and its inputs never
  run again. Held (`SESSION_DURABLE_TAIL_HELD_FOR_RECOVERY`),
  never committed: a cursor that disagrees with the transcript, and a tail
  left behind with requeued inputs.
- Reruns are at least once and pass authorization, approval review and
  pre- and post-tool hooks again. Rust tools read
  `ToolDispatchContext::tool_call_dedupe_key(call) -> Option<ToolCallDedupeKey>`
  (`run_id`, `round`, `call_id`, `call_digest` = lowercase hex SHA-256 of the
  tool name and canonical arguments; `ToolCallDedupeKey::new` derives it) and
  deduplicate on all four fields. MCP tools and host callback tools are
  dispatched again too, and a callback request is sent again. Core delivers
  the key to every call of a run's tool round, per-round commits or not: MCP
  `tools/call` requests carry it in `_meta` under
  `io.meerkat/tool_call_dedupe_key` (`{run_id, round, call_id,
  call_digest}`; host metadata using that name is refused), and RPC
  `tool/execute`, `pending_tool_calls` entries and Web JS tool callbacks (2nd
  argument) carry an optional `dedupe_key` with the same fields. Calls
  outside a run's tool round and detached callback jobs carry none.
- Not rerun, answered with an `outcome_uncertain` tool error instead: a call
  whose process custody shows the process may have run, a Core comms tool
  that sends to another agent (every comms tool but `peers`), and a call that
  would run detached under a runner that declares itself non-resumable.
- Graceful stop for a deploy: an unregister with
  `UnregisterQueuedInputs::Requeue { max_age }` (or a mob Shutdown with
  `QueuedInputsOnShutdown::Requeue(max_age)`) keeps the run of a session
  with per-round commits in flight like a crash: the turn stops at once
  (work since the last round commit is redone), its inputs stay bound with
  waiters answered `requeued_across_stop`, and the run continues at the next
  open, before the kept queued prompts; if a bound input is older than
  `max_age` (from admission; unregister: no default, 1 ms to 24 h; mob:
  15 min default) at that start, the inputs end `requeue_expired` instead
  and the run is closed as interrupted, its committed rounds kept. No
  Requeue, an explicit stop/cancel, retire, destroy, reset, a requeue stop
  that meets a steer being written into the turn, or a session without
  per-round commits still ends the run. Requeue (15 min) is the default of
  every graceful shutdown that does not choose, for sessions with per-round
  commits: `rkat-rpc` (input end, SIGTERM, interrupt; its client sees a
  shutdown error, not `requeued_across_stop`), REST (SIGTERM, interrupt,
  before its HTTP drain: a run in flight at the signal is suspended and a
  request waiting on it is answered `409` `requeued_across_stop`; a run that
  a request starts after the signal is drained, not suspended) and a mob
  Shutdown without `with_queued_inputs` (its default
  `QueuedInputsOnShutdown::RequeueDurable`, per member); an explicit `Stop`
  keeps 0.9.4 behaviour, and a host's own unregister without a disposition
  stops (`UnregisterQueuedInputs::default()` is `Stop`).
- Durable workers and the mob resume gate: `references/mobs.md`. Realms in a
  host's own database: `references/sessions_runtime_surfaces_storage_config.md`.

## Durable recovery holds and repair

A persistent session can be intact but not runnable. JSON-RPC answers
`SESSION_NOT_RUNNING` (-32003) with data
`{"durable_resume_hold": <hold>, "session_error_code": <code>, "session_id",
"content_retained": true}`. Treat these as "session preserved, not runnable
yet", not as a crash.

| `session_error_code` | `durable_resume_hold` | Meaning and action |
|---|---|---|
| `SESSION_DURABLE_TAIL_HELD_FOR_RECOVERY` | `tail_held_for_recovery` | A verified tail lies past the last committed boundary. The next start or attach of the session (a turn, resume or external event on a cold session) recovers it; plain reads (`session/read`, history, status) never do. A tail ending in a tool call without its result stays held, because the call may already have run (0.8.9). Since 0.9.5 an operator resolves a tail held as ambiguous, or an interrupted tail held beside inputs another run left staged or queued, with `rkat session resolve-held-tail <id> [--apply] [--runtime-store <FILE>] [--json]` (dry run by default; sqlite and jsonl realms, or a runtime file by `--runtime-store` with a full session UUID; refused while the persisted runtime state is in use or has a current run; stop every host first; exit 0 for nothing held, not held, a dry run and a repair): apply commits it as interrupted, keeps every row, answers each call without a result with an `outcome_uncertain` tool error, ends the inputs bound to the run (beside a run cursor, those it binds), requeues another run's stranded inputs and appends the interrupted notice (for an ambiguous tail it names the command). Library hosts: `meerkat_runtime::recovery::resolve_held_durable_tail(store, session_id, apply)` |
| `SESSION_DURABLE_TAIL_RECOVERY_REFUSED` | `recovery_refused` | Another live runtime or contradicting receipts blocked recovery; retry after it quiesces |
| `SESSION_DURABLE_EVIDENCE_QUARANTINED` | `evidence_quarantined` | Evidence is forked or unverifiable; investigate before repair |
| `SESSION_WHOLEBLOB_AUDITED_ENDPOINT_DIVERGENCE` | `audited_endpoint_divergence` | A WholeBlob document fails its audited-endpoint guard; repair it with the CLI below |

```bash
rkat session repair-wholeblob <session-id> --json          # diagnose only (default)
rkat session repair-wholeblob <session-id> --apply         # re-anchor, keeps every message
rkat session repair-wholeblob <uuid> --apply --accept-shorter   # only if the live transcript is shorter
rkat session repair-wholeblob <uuid> --runtime-store /path/runtime.sqlite --json
rkat session resolve-held-tail <session-id>                  # held tail: dry run (default)
rkat session resolve-held-tail <session-id> --apply          # commit it as interrupted, rows kept
```

WholeBlob runtime databases are the `runtime.sqlite3` of JSONL-backed realms
(or a host's own runtime file named with `--runtime-store`, which then needs a
full session UUID). SQLite-backed realms keep HeadCanonical sessions in
`sessions.sqlite3`.

If the host dies while a tool process runs (a shell call, background shell
job, monitor or command hook), process custody (Linux and macOS) stops that
earlier incarnation's processes before the agent is rebuilt. The run's inputs are
settled as interrupted and not replayed, and the transcript gets a
`tool_process_recovery` notice with a `tool_process_interrupted` block (the
tool's effects may be partial or complete). Since 0.9.5 a run that can resume
from a committed round (above) resumes instead: the call whose process may
have run gets an `outcome_uncertain` result rather than running again, and the
notice is still added. Custody knows whether the tool
process started: a spawn gate holds the command until the host has durably
recorded it, and if the host dies first nothing runs. When custody cannot be
proven,
building the agent fails closed with `ProcessCustodyError`, whose message
names the operator action (0.8.49).
