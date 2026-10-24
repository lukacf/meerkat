Covers: controls on model calls in Meerkat 0.9: per-call output cap
(`max_tokens`), budgets and turn limits (`[limits]`, `budget_limits`,
`max_turns`, exhaustion), compaction (`[compaction]`, triggers, events),
structured output (`output_schema`, extraction, events), retries and
timeouts, automatic model fallback (`[model_fallback]`), model switching.
Hooks: references/hooks_models_hooks.md. Caching: references/providers_and_auth_models.md

# Model operations (Meerkat 0.9)

How to bound and shape model calls: output caps, budgets, compaction,
structured output, retries, timeouts, model fallback and model switching.

## Key facts

- `rkat run --max-tokens` caps the output of each model call; it is not a
  total token budget (that is `[limits] budget` or `budget_limits.max_tokens`).
- Token and tool-call budgets end a run successfully with
  `terminal_cause_kind: "budget_exhausted"`; a time budget is a hard failure.
- The legacy `[budget]` config table is recorded but not enforced; use
  `[limits]` or per-session `budget_limits`. Since 0.9.1 the `rkat mob deploy`
  budget flags and a mobpack's `[budget]` set the members' `[limits]` (with
  `--surface rpc` since 0.9.3).
- RPC `turn/start` refuses `max_tokens`, `output_schema` and
  `structured_output_retries` (-32602). Set them on `session/create`.
- No surface has a manual "compact now" command.
- Model fallback is off by default, switches only after retryable capacity or
  provider failures, and is sticky for the session.

## Output cap per model call (`max_tokens`)

- CLI `rkat run --max-tokens <N>` (default `[agent] max_tokens_per_turn`,
  else 16384); RPC `session/create.max_tokens`, REST `POST /sessions`, MCP
  `meerkat_run`, Python `max_tokens=`, TypeScript `maxTokens`.
- Default when none is given: explicit top-level `max_tokens` or
  `[agent] max_tokens_per_turn`, else the model's catalog `max_output_tokens`
  for reasoning models or 16384, bounded by the account's output limit.
- Each call uses the smaller of that cap and the active model's
  `max_output_tokens`; `provider_params.max_output_tokens` sets it per call.
- `turn/start` refuses `max_tokens`; REST `POST /sessions/{id}/messages`
  accepts it and rebuilds the agent.
- A response that hits the cap with only reasoning fails the run with
  `MaxTokensReached` ("turn exhausted max_tokens during thinking; raise
  max_tokens or set a thinking budget").

## Budgets and turn limits

| Limit | Config | Per session | Scope | When reached |
|---|---|---|---|---|
| total tokens | `[limits] budget` | `budget_limits.max_tokens` | agent lifetime | run completes, `budget_exhausted` |
| tool calls | `[limits] max_tool_calls` (0.9.1) | `budget_limits.max_tool_calls`, `--max-tool-calls` | agent lifetime | run completes, `budget_exhausted` |
| wall clock | `[limits] max_duration` | `budget_limits.max_duration`, `-d/--max-duration` | agent lifetime | run fails, class `budget` |
| per-run wall clock | `[limits] max_turn_duration` | `budget_limits.max_turn_duration` | each run | run fails, class `budget` |
| model calls per run | `[agent] max_turns` (default 100) | none | each run | run fails, class `max_turns` |

```toml
[limits]
budget = 2000000          # tokens: presented input (incl. cache) + output
max_duration = "2h"       # humantime, or an integer number of milliseconds
max_turn_duration = "30m" # unset by default; 0 is rejected

[agent]
max_turns = 50            # 0 stops a run before its first model request
```

Example JSON-RPC `session/create` params with limits and a schema:

```json
{"prompt": "Summarize the release notes", "max_tokens": 4096,
 "budget_limits": {"max_tokens": 200000, "max_tool_calls": 50,
                   "max_turn_duration": {"secs": 900, "nanos": 0}},
 "output_schema": {"type": "object",
                   "properties": {"summary": {"type": "string"}},
                   "required": ["summary"]},
 "structured_output_retries": 2}
```

- `budget_limits` is accepted on RPC `session/create`, REST `POST /sessions`,
  Python `create_session(budget_limits=...)` and TypeScript `budgetLimits`,
  with durations as `{"secs": 600, "nanos": 0}`. MCP
  `meerkat_run.budget_limits` uses `max_duration_secs` and
  `max_turn_duration_secs`. A request's `budget_limits` replaces `[limits]`
  entirely; there is none on `turn/start`.
- CLI: `-d/--max-duration` takes humantime (`5m`, `1h30m`) and
  `--max-tool-calls N`; both also apply with `--resume`. There is no CLI flag
  for total tokens, `max_turn_duration` or `max_turns`.
- Rust: `meerkat_core::AgentBuilder::max_turns(u32)`; the facade
  `meerkat::AgentBuilder` takes `[agent] max_turns` from the `Config` given to
  `with_config`.
- Lifetime counters accumulate across the turns of a long-lived session and
  restart when the agent is rebuilt. Every model call counts, including
  compaction summaries and structured-output extraction. Limits are checked
  between steps, so one call or one tool batch can overshoot.
- Token or tool-call exhaustion is a successful run:
  `terminal_cause_kind: "budget_exhausted"` on the result and on
  `run_completed`.
- `budget_warning {budget_type, used, limit, percent}` (`budget_type`
  `tokens`, `time` or `tool_calls`) fires once when usage reaches `[agent]
  budget_warning_threshold` of a limit (0.8 by default), with `percent` the
  fraction used; the per-turn time limit warns again each turn (0.9.1). A
  value takes effect when it is greater than 0 and at most 1 (`1.0` keeps only
  the warning at the limit); an out-of-range value loads with a warning and
  keeps the inherited threshold. In 0.9.0 it fired only when a limit was
  reached, with `percent` 1.0, and the threshold had no effect.
- The legacy `[budget]` table (`max_tokens`, `max_duration`,
  `max_tool_calls`) is recorded but not enforced, because enforcement reads
  `[limits]` and `budget_limits`. Since 0.9.1 `rkat mob deploy
  --max-total-tokens`, `--max-duration` and `--max-tool-calls` and a
  mobpack's `[budget]` write the enforced `[limits]` instead (`budget`,
  `max_duration` and the new `[limits] max_tool_calls`); with `--surface rpc`
  they apply since 0.9.3. In 0.9.0 they landed in `[budget]`.
- Usage fields: `input_tokens`, `output_tokens`, `cache_creation_tokens`,
  `cache_read_tokens`, `reasoning_tokens`. Results carry `usage`
  (session-cumulative: take the latest value, never sum it), `run_usage` (this
  run only) and `request_usage` (one row per provider request, including
  extraction calls and model-written compaction summaries, each with an
  `accounting` block). `output_tokens` includes reasoning (Gemini thought and
  xAI reasoning tokens are added in). Cumulative values keep
  `cache_read_tokens + cache_creation_tokens <= input_tokens` and
  `reasoning_tokens <= output_tokens`; a per-call Anthropic row counts only
  uncached input (use `accounting.presented_tokens`). `turn_completed.usage`
  is per agent-loop model call; when it is absent
  (`turn_usage_accounting_unmeasured`), skip it rather than count zero.
  `run_completed.usage` is session-cumulative.

## Compaction

```toml
[compaction]
# auto_compact_threshold = 150000  # omit for the model-aware default
# max_request_bytes = 9000000      # omit for the provider's approximate cap
recent_turn_budget = 4             # recent complete turns kept verbatim
max_summary_tokens = 4096          # max_tokens of the summary call
min_turns_between_compactions = 3  # model calls between token-triggered runs
history_retained_rewrites = 4      # older pre-compaction bodies are retired
```

- Included in the stock `rkat`, `rkat-rpc`, `rkat-rest`, `rkat-mcp` and web
  runtime builds; Rust embedders need the `session-compaction` feature. All
  values must be greater than 0.
- Checked before every model call except a session's first, so it can run
  between tool rounds. It triggers when the last reported input tokens, the
  estimated history or the forecast request reaches the threshold, or when
  request bytes reach 4/5 of min(`max_request_bytes`, provider cap). The input
  and forecast triggers wait `min_turns_between_compactions` model calls after
  the previous attempt; the byte trigger does not, and neither does the history
  trigger while the last rewrite got the history under the threshold.
- Since 0.9.4, when the last rewrite left the history at or above the
  threshold (for example System rows larger than an explicit
  `auto_compact_threshold`), a history crossing waits for the cadence instead
  of paying for a summary at every model call, and a warning names the
  remedies: raise `auto_compact_threshold` above that floor, omit it so it
  derives from the model window, or shrink the System rows. Such a crossing
  skips the cadence again only when the request no longer fits the model
  window and the history added since that rewrite covers the overage (never
  for a model that declares no window). 0.9.3 and earlier compacted again at
  every model call.
- Default threshold: 4/5 of min(context window, input ceiling) of the model the
  session was built with (800000 for 1M-token Claude models, 737600 for
  `gpt-6-astra`), or 100000 when the model declares no window. Mob profiles
  can set `auto_compact_threshold`.
- The session's active model (after any model fallback) or the admitted
  controller writes the summary in one call with no tools and no retries.
  Since 0.9.4 its source is sized for that model: two bytes per token of its
  context window left after `max_summary_tokens` (capped by a declared input
  ceiling, less the prompt, at least 4 KiB); a model that declares no window
  gets a quarter of the threshold, the 0.9.3 rule for every model. A larger
  source is trimmed (tool results, then other context, then user requests,
  each cut marked); if that still does not fit, only the newest user requests
  that fit are sent, under a note that it carries the newest N of the M user
  requests. The discarded history becomes one `compaction_summary` user message starting "[Context compacted]"; system
  prompts and up to `recent_turn_budget` recent complete turns stay. With a
  memory store attached, discarded messages are indexed for `memory_search`
  (summaries, injected context, tool results, notices and empty or
  whitespace-only text, such as tool-call-only turns, are not). Forking or
  restoring a revision older than the last `history_retained_rewrites`
  rewrites fails with `TranscriptRevisionRetired`.
- Events: `compaction_started {input_tokens, estimated_history_tokens,
  message_count}`, `compaction_completed {summary_tokens, messages_before,
  messages_after, summary_source?, estimated_history_tokens_after?}`,
  `compaction_failed {reason}` (`reason.kind`: `llm_failed`,
  `empty_summary`, `curator_failed`, `estimation_failed`,
  `memory_indexing_failed`, `transcript_rewrite_failed`,
  `projection_handoff_refused`).
- `summary_source` (since 0.9.4) says who wrote the summary:
  `{"kind": "provider_call", "source_bound": B}`, `{"kind": "host_curator"}`
  or `{"kind": "mechanical_fallback"}`; only `provider_call` is a provider
  request. B is `{"kind": "whole"}`, `{"kind": "trimmed", "through":
  "tool_results" | "context" | "user_requests"}` (the last kind of text cut)
  or `{"kind": "requests_only", "kept": N, "of": M}`.
  `estimated_history_tokens_after` is the history estimate the rewrite left, in
  the unit of `estimated_history_tokens`. Both are absent on completions
  recorded before 0.9.4.
- A failed compaction keeps the history and the run continues, but a provider
  policy stop ends the run, and a request still too large afterwards fails the
  turn with a non-retryable `RequestTooLarge` ("compaction is irreducible at
  the configured retention ...").
- With a session blob store, compaction first moves inline images in rows not
  yet committed into blobs; if that fails, the attempt is skipped with
  `compaction_failed` (`kind: transcript_rewrite_failed`) and the turn
  continues.
- Rust hosts: `AgentBuildConfig.compaction_curator_override` (a
  `CompactionCurator`) writes the summary without a model call; its errors are
  `curator_failed`, with no fallback. A custom `Compactor` plugs in only through
  the core `meerkat_core::AgentBuilder::compactor`; its result must mark every
  earlier row as retained or discarded, discard at least one and not grow the
  transcript, or the attempt fails with `CompactionError::InvalidRebuild`. On
  the facade `meerkat::AgentBuilder`, `compactor` makes `try_build` return a
  configuration error.
  Since 0.9.4 a `Compactor` implements `prepare_for_summarization(messages,
  budget: Option<SummarizationSourceBudget>)` with no default and returns a
  `SummarizationSource` that states its bound; returning
  `SummarizationSource::whole(meerkat_core::materialize_latest_system_prompt_versions(messages))`
  keeps the 0.9.3 behaviour. `CompactionContext.last_rewrite_history_tokens`
  feeds `should_compact`; `None` keeps the 0.9.3 decisions.
- Anthropic server-side compaction is separate: `provider_params`
  `{"provider_tag": {"provider": "anthropic", "compaction": {"kind": "auto"}}}`.

## Structured output

Schema: a raw JSON Schema object, or a wrapper `{"schema": {...}, "name":
"...", "strict": false, "compat": "lossy" | "strict", "format": "meerkat_v1"}`.
`structured_output_retries` defaults to 2 (`[agent] structured_output_retries`
changes the default). Since 0.9.1 a realm's `[agent] extraction_prompt`
replaces the built-in extraction prompt for every agent built from that
config; `[agent] output_schema` applies to no session (the schema is set per
run) and loads with a warning (`ConfigWarning::UnappliedAgentOutputSchema`).

| Surface | How |
|---|---|
| CLI | `rkat run --schema <SCHEMA>`: inline JSON if it starts with `{`, else a file path; not with HTML output; use `--output json` to see `structured_output` |
| RPC | `session/create` `output_schema`, `structured_output_retries` (`turn/start` refuses both) |
| REST | `POST /sessions`, `POST /sessions/{id}/messages` (continue rebuilds the agent) |
| MCP | `meerkat_run`, `meerkat_resume` |
| Python / TypeScript | `output_schema` / `outputSchema` on create or deferred create (per-turn values are refused) |
| Mob profile | `output_schema` (raw schema, compiled with `strict: true`) |
| Rust | `AgentBuildConfig.output_schema`, `structured_output_retries` |

How a run proceeds:

1. Each model request carries the schema in a `<structured_output>` system
   section (request only, never stored).
2. When the loop finishes, the final reply is validated first; if it matches,
   no extra call is made (`extraction_succeeded.origin = "final_reply"`).
3. Otherwise an extraction request runs with no tools, temperature 0,
   provider-native web search removed and the provider's native schema slot
   where one exists; failures retry up to `structured_output_retries` times.
4. `RunResult.structured_output` holds the value, or `extraction_error
   {last_output, attempts, reason}` explains the failure. The run still
   completes and its text stays the main reply. `schema_warnings` lists
   provider warnings (Gemini only).

Events: `run_completed` carries `extraction_required: true`, then exactly one
`extraction_succeeded {session_id, structured_output, schema_warnings?,
request_usage, origin?}` or `extraction_failed {session_id, last_output,
attempts, reason, request_usage}` follows; keep the stream open for it and
read the value there or from the result, not from `run_completed`. Extraction
streams `text_delta` but no `turn_started`, `text_complete` or
`turn_completed`.

Providers: Anthropic `output_config.format` (objects closed with
`additionalProperties: false`), OpenAI `text.format` (wrapper `strict`), Chat
Completions and self-hosted `response_format`, Gemini `responseJsonSchema`
(warns about unsupported keywords; `compat: "strict"` makes them errors).
For Anthropic the slot gets a relaxed copy: keywords it rejects (number bounds,
`multipleOf`, `maxItems`, `minItems` above 1, `uniqueItems`, `contains`,
`minProperties`/`maxProperties`, `propertyNames`, dependency keywords,
`unevaluatedProperties`, `not`, `discriminator`) move into the description
and `oneOf` becomes `anyOf`; Meerkat still validates the reply against the
full schema. Unsupported string `format` values still fail with HTTP 400.
Extraction requests run the model-request hooks; `run_completed` hooks run
before extraction. Budget exhaustion during extraction gives
`extraction_failed`. Model fallback re-applies the schema for the new provider.

## Retries and timeouts

```toml
[retry]
max_retries = 3                       # retries after the first attempt
initial_delay = "500ms"
max_delay = "30s"
multiplier = 2.0
# call_timeout = "10m"                # omitted: model default; "disabled": none
# stream_inactivity_timeout = "120s"  # omitted: model window, else 300s; "disabled": off
```

- Retried: rate limits, network and call timeouts, stalled streams, overload
  and 5xx errors, connection resets, incomplete or empty responses, and
  unknown provider failures. Not retried: invalid requests, authentication
  failures, context or request too large, content filtered, quota exhausted,
  model not found, policy stops.
- Delay: `initial_delay * multiplier^(n-1)` with +/-10% jitter, capped at
  `max_delay`; a larger `Retry-After` wins; rate limits wait at least 30s.
- `retrying {retry: {failure: {provider, kind, message, ...}, plan: {attempt,
  max_retries, selected_delay_ms, budget_capped, ...}}, assistant_message_id?}`;
  `failure.kind` is `rate_limited`, `network_timeout`, `call_timeout` (stalls
  too) or `retryable_provider_error`. A retry reuses `assistant_message_id`:
  drop text deltas buffered for it. Exhaustion fails the run (class `llm`,
  cause `retry_exhausted`); retry waits never exceed the remaining time budget.
- Call timeout: a hard deadline on one model call, from `[retry]
  call_timeout`, else the catalog's `call_timeout_secs` (Claude Opus and Fable
  300s, Sonnet, `claude-haiku-5-5` and Gemini Flash 120s, Haiku 4.5 60s, most
  OpenAI models and `gemini-3.1-pro-preview` 600s, `gemini-4-argon` 900s;
  custom models: `[models.<id>] call_timeout_secs`). No per-request setting.
- Stream watchdog: a stream silent for the window is aborted as a retryable
  stall. The window is `[retry] stream_inactivity_timeout`, else the model's
  own (`gemini-4-argon` 900s, `gpt-5.5-pro` 7200s; self-hosted models:
  `[self_hosted.models.<id>] stream_inactivity_timeout_secs`, at least 60),
  else 300s. It is resolved for each call, so a model switch or fallback uses
  the new model's window. An explicit window must be above zero (`"0s"` is
  refused; `"disabled"` turns the watchdog off), and one under 60s can abort
  and retry a stream whose answer already arrived. The compaction summary
  call has none of these guards.
- Tools: `[tools] default_timeout = "10m"`; per tool `[tools.tool_timeouts]
  shell = { secs = 120, nanos = 0 }` (not humantime). A timed-out tool returns
    a `timeout` error result and the run goes on. Since 0.9.1 a call that hits
  its per-call or inactivity timeout first emits `tool_execution_timed_out`
  (with the bound in `timeout_ms`), then its `tool_execution_completed` and
  `tool_result_received`, and ATIF exports record it as one observation marked
  `timed_out`; 0.9.0 never emitted the event. The shell tool's `timeout_secs`
  comes from the model
  (minimum 1); without it `[shell] timeout_secs` applies (default 30, and 0
  keeps 30) since 0.9.1, where 0.9.0 always used 30.

## Model fallback and model switching

Automatic fallback (`[model_fallback]`) is off by default. Set it in realm
config, a mob definition's `[runtime.model_fallback]` or a mob profile's
`model_fallback` (profile beats mob runtime beats realm config; a present
table replaces the inherited one whole). There is no RPC, REST, MCP or CLI
per-session parameter; Rust hosts can set `AgentBuildConfig.model_fallback`.

```toml
[model_fallback]
enabled = true                     # requires a non-empty chain

[model_fallback.policy]
cross_provider = true              # default false: other-provider targets skipped
min_context_headroom = 0.10        # target window must keep 10% free
require_tool_parity = true         # skip targets that lose provider web search
trigger_after_attempts = 3         # failed attempts before a switch
triggers = ["provider_unavailable", "capacity"]   # also "transport", "empty_output"

[[model_fallback.chain]]
model = "claude-opus-5-5"
provider = "anthropic"             # optional; default from the catalog

[[model_fallback.chain]]
model = "gpt-5.5"
auth_binding = { realm = "global", binding = "openai_oauth" }
```

- Validation: "model_fallback.enabled = true requires a nonempty explicit
  chain", empty `model`, `min_context_headroom` outside [0, 1) and
  `trigger_after_attempts = 0` are errors; unknown keys are rejected. A
  pre-0.8.37 `enabled = true` table with no chain, or the removed
  `use_catalog_default_chain` key, loads as no policy with a config warning
  (rkat prints it on stderr).
- A switch happens only on a failure the retry policy is about to retry, once
  the failed attempt number reaches `trigger_after_attempts` (keep it at or
  below `[retry] max_retries`; with the defaults the fourth attempt uses the
  fallback), and only for listed trigger classes: `capacity` (rate limits,
  overload), `provider_unavailable` (provider 5xx), `transport` (connection
  resets), `empty_output` (incomplete responses). Timeouts and stream stalls
  never switch, and neither does a failure after text or reasoning streamed.
- Chain targets after the current model are tried in order. Each skipped
  target emits `model_fallback_skipped` with a reason: `provider_boundary`,
  `auth_unavailable`, `context_fit`, `context_unknown`, `output_budget`,
  `modality_parity`, `tool_parity` (output schema cannot compile, or web
  search lost), `tool_choice_unsupported`, `request_unsupported`,
  `admission_unavailable`.
- A switch emits `model_fallback_staged` and `model_fallback_committed`, adds
  a `model_fallback` system notice (models, reason, limits, hidden tools) to
  the transcript, re-applies the output schema for the new provider, and runs
  the retry on the new model. `model_fallback_target_failed` reports a later
  failure of that target.
- The switch is sticky: later turns stay on the fallback model; nothing
  switches back automatically. On resume the fallback target is re-checked;
  if it no longer fits or its credential is gone the run fails with
  `ModelFallbackResumeHeld` ("fallback-origin session requires explicit model
  reconfiguration"). Choosing a model explicitly clears that state.
- A target's identity includes `auth_binding`, so the same model with another
  binding is a valid credential failover target.

Switching the model yourself:

| Surface | How |
|---|---|
| RPC | `turn/start` with `model`, `provider`, `self_hosted_server_id`, `provider_params`, `auth_binding`: hot-swaps the live session (deferred or materialized) without a rebuild and persists it for later turns |
| Mob member | `mob/turn_start` with `mob_id`, `agent_identity` and the same fields; it runs the member session's `turn/start`, so the same rules apply |
| REST | `POST /sessions/{id}/messages` with `model`, `provider`, `auth_binding` (rebuilds the agent; no `provider_params`) |
| MCP | `meerkat_resume` with `model`, `provider`, `provider_params` (rebuilds the agent) |
| CLI | `rkat run --resume <ID> -m <MODEL> [-p <PROVIDER>] [--params-json <JSON>]` |
| SDKs | Python `Session.turn(..., model=, provider=, provider_params=)`; TypeScript `turn(prompt, {model, provider, providerParams})` |
| Agent | builtin `brain_swap` tool; `target_model` is an enum of the reachable models. The switch is permanent: the current run stays on its model, and if the run finishes cleanly the request is saved with the session and applied before the next input, also after a restart. Offered only when more than one model is reachable |
| Rust | `Agent::hot_swap_llm_identity`; `SessionService::hot_swap_session_llm_identity` (the trait default returns `SessionError::Unsupported`). `Agent::replace_client` and `SessionService::set_session_client` keep the model: a client for another provider or model is refused with `AgentError::ClientModelChangeRequiresHotSwap` |

Rules for `turn/start`: `model` alone takes the provider that owns it in the
catalog (an uncatalogued model keeps the current provider); `provider`
without `model` is refused ("provider override requires model on an existing
session"), as is a provider that does not serve the model;
`self_hosted_server_id` needs provider `self_hosted` and must match the
configured server; `provider_params` and `auth_binding` take a value,
`{"action": "set", "value": ...}` or `{"action": "clear"}`; a provider change
without `auth_binding` drops the old binding. These errors and the refused
`max_tokens`, `output_schema` and `structured_output_retries` return -32602.

Experimental catalog rows (`release_stage: "experimental"`, today
`gpt-live-1-codex`) are listed by `rkat models` but refused as a session
model: "experimental model ... requires an admitted live-channel execution
identity".

A `system_prompt` after the first turn, or `session/inject_context`, adds a
System message mid-conversation. The catalog flag
`supports_mid_conversation_system_messages` (`rkat models`) shows which
models accept one: the OpenAI GPT text models and, among Anthropic models,
only claude-fable-5-1, claude-fable-5, claude-opus-5-5, claude-opus-5,
claude-opus-4-8, claude-sonnet-5-5 and (0.9.1) claude-haiku-5-5. On Gemini and
other Anthropic models
(and on the ChatGPT backend, which takes one leading System message) the
request fails with a non-retryable `invalid_request` (`InvalidInputShape`);
history is not rewritten. `injected_context` is a user message and is not
affected.

## Prompt caching

Prompt caching knobs, per-provider defaults, the backends that receive them and
all `provider_params` keys: references/providers_and_auth_models.md (Provider
parameters).
Anthropic bills cache writes at a premium (1.25x input for the default
five-minute cache; hits about 0.1x), reported as `cache_creation_tokens` and
`cache_read_tokens`.
