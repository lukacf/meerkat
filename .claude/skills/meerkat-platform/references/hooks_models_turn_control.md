Covers: turn control and agent-loop behaviour in Meerkat 0.9: interrupt
versus run-fenced `turn/stop_run` versus request cancel on every surface,
what a cancelled run leaves behind, shutdown, parallel tool calls, the
tool-call limit, tool choice, and the full list of streaming event types with
how RPC, REST, MCP, the SDKs and the CLI deliver them.
Hooks: references/hooks_models_hooks.md; model calls: references/hooks_models_model_operations.md

# Turn control, agent loop and events (Meerkat 0.9)

How to stop work, what a stopped run leaves behind, how the agent loop runs
tool calls, and which events a client receives.

## Key facts

- `turn/interrupt` cancels whatever run is current; `turn/stop_run` stops only
  the named run (a stale `run_id` gets `not_current` and touches nothing).
- A cancelled run ends with `run_failed` (`error_report.class = "cancelled"`);
  there is no `run_cancelled` event.
- `rkat session interrupt` cannot stop a turn that another process is running.

## Turn control

| Goal | RPC | REST | MCP | SDK |
|---|---|---|---|---|
| cancel the current run | `turn/interrupt {session_id}` | `POST /sessions/{id}/interrupt` | `meerkat_interrupt` | Python `Session.interrupt()`, TS `session.interrupt()` |
| stop one exact run | `turn/stop_run {session_id, run_id, reason}` | `POST /sessions/{id}/runs/{run_id}/stop` with `{"reason": "..."}` | none | Python `Session.stop_run(run_id, reason=...)`, TS `session.stopRun(runId, reason)` |
| cancel one request | notification `cancel {"request_id": <id>}` | `POST /requests/{request_id}/cancel` | `notifications/cancelled` | none |

- Interrupt returns `{session_id, interrupted, result}` with `result`
  `interrupted` or `staged_noop` (deferred session). An idle session also
  reports `interrupted: true`. It is not fenced: it cancels whatever run is
  current when it arrives. Errors: `SESSION_NOT_FOUND` (-32001),
  `SESSION_BUSY` (-32002, still materializing), `INVALID_REQUEST` (-32600, not
  interruptible).
- `rkat session interrupt <ID>` acts through a runtime it builds in its own
  process, so it cannot stop a turn another process is running; use that
  host's interrupt. Ctrl-C during a one-shot `rkat run` ends the process
  without a graceful cancel; a keep-alive run shuts down cleanly on Ctrl-C or
  SIGTERM once its first turn has finished.
- Stop run: take `run_id` from `run_started.identity.run_id` (or an input's
  `last_run_id` from `session/input_status`); it must be a UUID. The receipt
  `outcome` is `stopped {run_id, contributors: [{input_id, completion,
  terminal?}]}`, `not_current {run_id, current_run_id?}` (stale id; nothing is
  touched, and a repeat stop also gets this) or `not_stoppable {run_id,
  state}`. The call returns once every contributing input is terminal. After
  a stop, steers that joined the run are cancelled with it instead of being
  requeued; queued inputs that never joined run afterwards.
- Request cancel ends a still-uncommitted request with `REQUEST_CANCELLED`
  (-32005, HTTP 499); a committed result is never rewritten. For `turn/start`
  it cancels exactly that input (queued: abandoned; running: its run is
  interrupted). RPC can cancel `session/create` (unless deferred),
  `turn/start`, `help/ask`, `mob/turn_start`, `mob/bind_host` and
  `mob/revoke_host`. REST needs `X-Meerkat-Request-Id` on `POST /sessions`,
  `POST /sessions/{id}/messages` or `POST /help`, and answers
  `{"request_id", "cancelled": true}`, or `cancelled: false` with
  `reason: "already_terminal"`. Cancelling MCP `meerkat_run` /
  `meerkat_resume` only drops the response; the turn keeps running. The
  Python and TypeScript SDKs do not send cancel notifications.
- Mob members: `mob/stop_member_run` (run-fenced), `mob/hard_cancel_member`,
  `mob/force_cancel` (next boundary); see the mobs reference.

A cancelled run emits `run_failed` with `error_report.class = "cancelled"`
(there is no `run_cancelled` event). The waiting caller gets -32005 or 499,
and the run's inputs end abandoned as cancelled, not retried. A persistent
session drops the run's uncommitted work; the next turn continues from the
last committed transcript. Interrupt and stop do not target background shell
jobs (use `jobs/cancel` or the `shell_job_cancel` tool). `rkat-rpc` on
SIGTERM, Ctrl-C or stdin EOF cancels the tracked in-flight requests of
sessions without per-round commits after a short drain; the run in flight of
a session with per-round commits is suspended and continues at the session's
next open (see the durable runs guide). Then it exits.

## Agent loop behaviour

- A run calls the model, runs any tool calls in the response, and calls the
  model again until a response has no tool calls or a limit is reached.
- Parallel tool calls: calls in one response are admitted in order; refused
  calls (`hook_denied`, `confinement_refused`, `access_denied`, `not_found`)
  get error results without blocking the others. Admitted calls run
  concurrently, at most `[tools] max_concurrent` (default 10) at once; waiting
  for a slot counts toward a call's timeout. Results commit in call order after
  the whole batch. No setting makes calls sequential.
- Tool-call limit: `--max-tool-calls`, `budget_limits.max_tool_calls` or
  (since 0.9.1) `[limits] max_tool_calls`,
  unlimited by default; refused and failed calls count.
- Tool choice: `turn_tool_overlay.tool_choice_plan` (RPC `turn/start` and
  `mob/turn_start`, REST continue, Python and TypeScript SDKs; Rust
  `ToolChoice`) is a list of `{"mode": "auto" | "required" | "none"}` or
  `{"mode": "tool", "name": "..."}`; entry k applies to the run's k-th model
  request and later requests use auto. It maps to Anthropic and OpenAI
  `tool_choice` and Gemini `functionCallingConfig`. A choice the model cannot
  honour is refused before the call (non-retryable, class
  `tool_choice_unsupported`), for example `required` or a named tool on
  claude-opus-5-5 and claude-sonnet-5-5, or with explicit Anthropic thinking.
  The CLI and the MCP server tools have no tool choice option.
- The system prompt carries no tool list (since 0.8.43): tool definitions
  reach the model only through the request's tool array. Short guidance
  remains only for composed families (comms, the deferred tool catalog, and
  skill discovery when both `browse_skills` and `load_skill` are present).
- A response with no text, images or tool calls is retried unless the run
  already produced output.

## Streaming events

Every event is wrapped in an envelope `{event_id, source, seq, mob_id?,
timestamp_ms, payload}`; `payload.type` is the event type.

- Run: `run_started` (`identity.run_id`), `run_completed` (`result`, `usage`,
  `extraction_required`, `terminal_cause_kind?`), `run_failed`
  (`error_report {class, reason, message}`), `extraction_succeeded`,
  `extraction_failed`; since 0.9.5 `run_resumed` (`run_id`, `round`,
  `resume_point` `tools_pending` or `awaiting_model`) when a run continues
  after a restart (durable runs in
  `references/sessions_runtime_surfaces_sessions.md`).
- Model output: `turn_started`, `text_delta`, `text_complete`,
  `reasoning_delta`, `reasoning_complete`, `server_tool_content`,
  `assistant_image_appended`, `turn_completed` (`stop_reason`, `usage?`; once
  per model call).
- Tools: `tool_call_requested`, `tool_execution_started`,
  `tool_execution_completed`, `tool_result_received`,
    `tool_execution_timed_out` (since 0.9.1; 0.9.0 never emitted it),
  `tool_config_changed`. Since 0.9.5 `tool_execution_completed` carries
  `is_error` and `duration_ms` but no result body: `tool_result_received`
  with the same `id` is the only event with `content`.
- Hooks: `hook_started`, `hook_completed`, `hook_failed`, `hook_denied`,
  `hook_launch_refused`.
- Model operations: `compaction_started`, `compaction_completed`,
  `compaction_failed`, `budget_warning`, `retrying`,
  `model_fallback_skipped`, `model_fallback_staged`,
  `model_fallback_committed`, `model_fallback_target_failed`,
  `request_reasoning_lowered`, `provider_cache_breakpoints_discarded`,
  `turn_usage_accounting_unmeasured`,
  `turn_usage_accounting_identity_disputed`, `operation_observation_failed`,
  `tool_results_bounded_for_request` (a request carried the run's own tool
  results shortened for it only: `run_id`, `results` with `tool_use_id`,
  `tool_name`, `kept_bytes`, `total_bytes`, and `axis` `request_bytes` or
  `context_window`; the transcript keeps them whole).
- Reasoning: `run_started.request_reasoning` appears only when a host set a
  request-local reasoning preference (in 0.9.0 only GPT Live delegated member
  turns; no public turn input accepts it; `minimal` is refused). It holds the
  batch disposition (`{"disposition": "apply", "level": ...}`,
  `superseded_by_explicit`, `conflicting` or `mixed_with_unpreferred`);
  `request_reasoning_lowered {requested, baseline, outcome?}` reports what
  each provider attempt sent.
- Other: `skills_resolved`, `skill_resolution_failed`,
  `peer_content_ingested`, `boundary_append_applied`,
  `boundary_appends_discarded`, `background_job_completed`,
  `interaction_complete`, `interaction_callback_pending`,
  `interaction_failed`, `stream_truncated`, `live_channel_closed`,
  `transcript_rewrite_committed`, `transcript_rewrite_audit_receipt_committed`.

Delivery:

- RPC: `session/event {session_id, event}` notifications for runs the server
  executes (best effort: dropped without a marker if the client falls far
  behind); or `session/stream_open {session_id}`, then `session/stream_event
  {stream_id, sequence, session_id, event}` until `session/stream_end`, closed
  with `session/stream_close`. Replay (when `runtime/capabilities` reports
  `event_replay`): `events/latest_cursor`, `events/list_since` (limit default
  100, max 1000), `events/snapshot`.
- REST: `GET /sessions/{id}/events` (SSE): `session_loaded`, one SSE event per
  agent event (name = type, data = envelope), then `done`; a lagging client
  silently misses events.
- MCP: `meerkat_event_stream_open|read|close`; `meerkat_run` /
  `meerkat_resume` with `stream: true` send `notifications/message`.
- Python: `create_session_streaming(...)` and `Session.stream(...)` return an
  `EventStream` (async iterator of typed events, `.result` once consumed,
  `collect()`, `collect_text()`); `Session.subscribe_events()` yields
  envelopes. TypeScript: `createSessionStreaming`, `session.stream`,
  `session.subscribeEvents()`. Types without a typed class arrive as
  `UnknownEvent`.
- Live subscribers that fall behind get `stream_truncated` (`stream_lagged`).
- CLI: `rkat run --stream` prints text on stdout and tool, retry, compaction,
  budget and hook lines on stderr; `--output json` prints the final result. There is no JSON-lines event output; `--line-format` only sets how
  `--stdin lines` input is parsed.
