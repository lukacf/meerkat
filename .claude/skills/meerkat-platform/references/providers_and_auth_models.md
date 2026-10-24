Covers: choosing a provider and model on every surface and the resolution
order; default models and the catalog (`rkat models`); capabilities, thinking
and effort; `provider_params` and prompt caching; realtime, voice and image
models; self-hosted servers; OpenAI Responses and vLLM stream behavior;
provider errors, refusals and content filters. Env keys, `rkat auth`, bindings,
backends, redirects, credential storage: references/providers_and_auth_credentials.md.

# Providers and Models (Meerkat 0.9)

## Quick start

The fastest path is an API key in the environment for the provider of the
model you run:

```bash
export RKAT_ANTHROPIC_API_KEY=sk-ant-...     # or ANTHROPIC_API_KEY
rkat run -m claude-opus-5-5 "hello"
rkat run -p anthropic "hello"                # provider default model
rkat --default-model claude-opus-5-5         # persist a default, then plain `rkat run`
```

- With no `--model`, `--provider`, `--auth-binding` or configured default, the
  model is `gpt-6-astra` (OpenAI). Meerkat never picks a provider from the
  keys that happen to be set: with only an Anthropic key, a plain
  `rkat run "hi"` fails for lack of OpenAI credentials.
- Env key names and precedence, subscriptions (`rkat auth login`) and
  realm bindings: references/providers_and_auth_credentials.md.
- Checks: `rkat doctor`, `rkat models`, `rkat auth bindings`,
  `rkat auth test <binding>`.

## Providers and selection

| Where | Provider values |
|---|---|
| CLI `-p, --provider` | `anthropic`, `openai`, `gemini`, `self-hosted` |
| Config, RPC `session/create` and `turn/start`, REST, Python/TS SDK `provider` | `anthropic`, `openai`, `gemini`, `self_hosted` |
| MCP tool `meerkat_run` `provider` | `anthropic`, `openai`, `gemini`, `other` |
| `provider_params.provider_tag.provider` | `anthropic`, `open_ai`, `gemini` (note `open_ai`) |

GitHub Copilot, Azure OpenAI, AWS Bedrock, GCP Vertex, Azure Foundry and
ChatGPT OAuth are backends of these providers, not providers (see Backends
in references/providers_and_auth_credentials.md).

Every session-creating surface (CLI, RPC, REST, MCP, SDKs) resolves the
model and provider the same way:

1. An explicit model wins. Its provider is the explicit provider, else the
   auth binding's provider, else the registry owner of the model id.
2. Else the named auth binding's `default_model`.
3. Else, when a provider is fixed (by `--provider` or by the binding):
   `[agent] model` if it belongs to that provider, else `[models] <provider>`,
   else the catalog default of that provider.
4. Else `[agent] model`.
5. Else the first non-empty `[models] anthropic`, `openai`, `gemini` (in that
   order).
6. Else the global default `gpt-6-astra`.

Rules:

- Provider inference is an exact lookup in the effective registry (built-in
  catalog, `[models.<id>]`, `[self_hosted.models.<alias>]`). Prefixes such as
  `claude-` or `gpt-` are never guessed. An unknown id fails with
  `cannot infer provider from model '<id>'; specify a provider or auth binding`.
- An explicit provider that contradicts the registry owner fails: `model
  '<id>' is registered for provider '<owner>', not provider '<given>'; explicit
  provider overrides must match catalog ownership`.
- An auth binding whose provider differs from the explicit provider fails
  before any session is created.
- A model id must be in the registry to run. A provider hint alone resolves
  the provider, but building the client then fails with `model profile
  '<provider>:<model>' is unavailable`. Declare new ids under `[models.<id>]`
  or `[self_hosted.models.<alias>]`.

## Default models

| Scope | 0.9.1 default |
|---|---|
| No hints at all (global default) | `gpt-6-astra` |
| Anthropic | `claude-opus-5-5` |
| OpenAI | `gpt-6-astra` |
| Gemini | `gemini-4-argon` (paid Gemini API tiers; on the free tier select `gemini-3.8-flash`, for example `[models] gemini = "gemini-3.8-flash"`) |
| Self-hosted | `[self_hosted] default_model`, or the only configured alias |
| Image generation (OpenAI / Gemini) | `gpt-image-2` / `gemini-nano-banana-2.1` |

Change defaults:

```toml
[agent]
model = "claude-opus-5-5"        # the realm's default model

[models]                         # used when a provider is fixed but no model
anthropic = "claude-sonnet-5-5"
openai = "gpt-5.5"
gemini = "gemini-3.8-flash"
```

`rkat --default-model <MODEL>` validates the id against the effective
registry and patches only `agent.model` in the scope's config (unknown ids
are refused with the list of known models). A catalog entry is Meerkat's capability contract,
not account entitlement: if `gpt-6-astra` is not enabled for your account,
choose another model (for example `gpt-5.5`).

## Model catalog

`rkat models` takes no flags and prints pretty JSON: the effective registry
(built-in catalog plus `[models.<id>]` entries and self-hosted aliases). The
same data comes from RPC `models/catalog`, REST `GET /models/catalog`, MCP
tool `meerkat_models_catalog`, Python `get_models_catalog()` and TypeScript
`getModelsCatalog()`.

Each provider group has `provider`, `default_model_id` and `models`; each
model has `id`, `display_name`, `tier` (`recommended` or `supported`),
`release_stage` (`stable`, `experimental`, `operator_defined` for custom and
self-hosted entries), the token limits, `server_id` (self-hosted) and a
`profile` with the capability flags below plus `params_schema`.

Built-in models (context / max output tokens, 1.05M = 1,050,000; R =
recommended, S = supported):

| Anthropic | Tier | Context / output | Thinking | Effort |
|---|---|---|---|---|
| `claude-fable-5-1`, `claude-fable-5` | R | 1M / 128K | adaptive only | low, medium, high, xhigh, max |
| `claude-opus-5-5` (default) | R | 1M / 128K | adaptive only | low..max |
| `claude-opus-5` | R | 1M / 128K | adaptive or `disabled` (effort `high` or below) | low..max |
| `claude-opus-4-8` | R | 1M / 128K | adaptive or `disabled` | low..max |
| `claude-opus-4-7` | S | 1M / 128K | adaptive or `disabled` | low..max |
| `claude-sonnet-5-5` | R | 1M / 128K | adaptive or `between_tools` | low..max |
| `claude-sonnet-5` | S | 1M / 128K | adaptive or `disabled` | low..max |
| `claude-sonnet-4-6` | R | 1M / 64K | adaptive, enabled (budget) or `disabled` | low, medium, high, max |
| `claude-haiku-5-5` (0.9.1) | R | 1M / 128K | adaptive or `disabled` (effort `high` or below) | low..max |
| `claude-haiku-4-5`, `claude-haiku-4-5-20251001` | R | 200K / 64K | enabled (budget) or `disabled` | - |
| `claude-sonnet-4-5` | S | 200K / 64K | enabled (budget) or `disabled` | - |
| `claude-opus-4-5` | S | 200K / 64K | enabled (budget) or `disabled` | low, medium, high |

| OpenAI | Tier | Context / output | Reasoning effort |
|---|---|---|---|
| `gpt-6-astra` (default) | R | 1.05M (922K input) / 128K | low..max |
| `gpt-6-sol`, `gpt-6-luna` | S | 1.05M (922K input) / 128K | none..max |
| `gpt-6.1-sol` | S | 1.05M (922K input) / 128K | low..max |
| `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.6` (GPT-5.6 Sol) | S | 1.05M / 128K | none..max |
| `gpt-5.5`, `gpt-5.5-pro` | R | 1.05M / 128K | none..xhigh |
| `gpt-5.4` | S | 1.05M / 128K | none..xhigh |
| `gpt-5.4-mini` | S | 128K / 16,384 | none..xhigh |
| `gpt-5.3-codex` | S | 400K / 128K | low..xhigh |
| `gpt-realtime-2` | R | 128K / 32K | minimal..xhigh |
| `gpt-live-1` | S | not published | - |
| `gpt-live-1-codex` (experimental) | S | not published | - |

Gemini (all 1,048,576 / 65,536 tokens, thinking via `thinking_level`):
`gemini-4-argon` (default, R; paid Gemini API tiers only; levels `low`,
`medium`, `high`, default `medium`), `gemini-3.8-flash` (R; the recommended
Flash model, also on the free tier), then `gemini-3.7-flash`,
`gemini-3.6-flash`, `gemini-3.5-flash`, `gemini-3.5-flash-lite`,
`gemini-3.1-pro-preview`, `gemini-3.1-flash-lite` and
`gemini-3.1-flash-lite-preview` (S). `gemini-4-argon`, `gemini-3.8-flash`,
`gemini-3.7-flash` and `gemini-3.1-pro-preview` accept only `low`, `medium`
and `high` (Google refuses `minimal` for them); the other rows also accept
`minimal`, which is the default on `gemini-3.5-flash-lite` and the 3.1
Flash-Lite rows.

Custom hosted models (an API provider serves an id the catalog lacks):

```toml
[models.my-model]
provider = "openai"          # anthropic | openai | gemini
display_name = "My model"    # optional; defaults to the id
context_window = 400000
max_input_tokens = 350000    # optional separate input ceiling
max_output_tokens = 64000
vision = true                # image input and image tool results
web_search = false
call_timeout_secs = 600
```

Unknown keys are rejected. `provider = "self_hosted"` is refused (use
`[self_hosted.models]`). Ids must be unique across built-in, custom and
self-hosted entries, so a built-in id cannot be redefined. Undeclared
capabilities are absent; custom entries get no thinking or reasoning
controls.

## Capabilities and reasoning controls

Capability flags come from the catalog row, never from the model name:

- `vision` / `image_input` (images in user messages): every built-in row
  except `gpt-live-1` and `gpt-live-1-codex`.
- `image_tool_results`: images inside tool results. All built-in text rows;
  not the realtime rows. Self-hosted aliases only with `api_style =
  "responses"`.
- `inline_video`: Gemini rows only.
- `realtime`: `gpt-realtime-2`, `gpt-live-1`, `gpt-live-1-codex`.
- `image_generation` through the chat model: OpenAI GPT-5.x rows and
  `gpt-realtime-2`; not the GPT-6 rows.
- Provider-native web search: every text row except `gpt-5.3-codex`.
- Forced tool choice (`required` or a named tool) is refused locally on
  `claude-opus-5-5` and `claude-sonnet-5-5`
  (`ModelDoesNotSupportForcedToolChoice`).
- Sampling: the Claude 5 family (including `claude-haiku-5-5`), Claude Opus
  4.8/4.7, all GPT-5.x and GPT-6 rows, `gemini-4-argon`, `gemini-3.8-flash`,
  `gemini-3.7-flash`, `gemini-3.6-flash` and `gemini-3.5-flash-lite` take no
  `temperature`, `top_p` or `top_k` (`supports_temperature: false`). The Anthropic and
  OpenAI clients drop a generic `temperature` for them.
- Default per-call timeout (catalog `call_timeout_secs`): 300 s for Claude
  Opus and Fable rows, 120 s for Sonnet rows, `claude-haiku-5-5` and the
  Gemini Flash and Flash-Lite rows, 60 s for Haiku 4.5, 600 s for OpenAI rows
  and `gemini-3.1-pro-preview`, 900 s for `gemini-4-argon`, 7200 s for
  `gpt-5.5-pro`. Custom and self-hosted entries set their own.

Thinking and effort are typed per model (tables above); `between_tools`, and
`disabled` on `claude-opus-5` and `claude-haiku-5-5`, are accepted only at
effort `high` or below. An unsupported thinking type, budget or `top_k` is
refused when the agent is built and before each request.

## Provider parameters

`provider_params` is a typed object; unknown keys are rejected:

```json
{"temperature": 0.2, "top_p": 0.9, "max_output_tokens": 4096,
 "reasoning": "emit", "thinking_budget_tokens": 8000,
 "provider_tag": {"provider": "anthropic", "effort": "high"}}
```

`reasoning` is `emit`, `silent` or `off`. Set `provider_params` with CLI
`--param KEY=VALUE` (repeatable; one top-level key each, not a path, so pass
`provider_tag` as one JSON value; values parse as JSON, else string) or
`--params-json '<object>'` (`--param` wins on conflicts); RPC
`session/create` and `turn/start` `provider_params`; REST `POST /sessions`
(not on `POST /sessions/{id}/messages`); MCP `meerkat_run`; mob profiles;
Rust `AgentBuildConfig.provider_params`. A realm-level
`[agent] provider_params` table is a configuration error.

`provider_tag` fields:

- Anthropic (`"provider": "anthropic"`): `thinking` (`{"type": "adaptive"}`,
  `{"type": "enabled", "budget_tokens": N}`, `{"type": "between_tools"}`,
  `{"type": "disabled"}`),
  `thinking_budget_tokens`, `effort` (`low`, `medium`, `high`, `xhigh`,
  `max`), `top_k`, `inference_geo` (`{"kind": "us"}`, `{"kind": "global"}`,
  `{"kind": "other", "region": "..."}`),
  `cache_control` (`disabled`,
  `automatic`, `system_prefix`, `system_and_conversation`), `cache_ttl`
  (`5m` default, `1h`).
- OpenAI (`"provider": "open_ai"`): `reasoning_effort` (`none`, `low`,
  `medium`, `high`, `xhigh`, `max`, checked against the model), and on GPT-5.6
  only `reasoning_mode` (`standard`, `pro`), `reasoning_context` (`auto`,
  `current_turn`, `all_turns`) and `text_verbosity` (`low`, `medium`,
  `high`); also `seed`, `frequency_penalty`, `presence_penalty`, `store`
  (default false), `prompt_cache_enabled`, `prompt_cache_key`,
  `prompt_cache_retention` (`in_memory`, `24h`), `prompt_cache_options`
  (`{"mode": "implicit" | "explicit", "ttl": "30m"}`). For OpenAI-compatible
  endpoints, `reasoning`, `chat_template_kwargs` and `thinking` bodies are
  forwarded verbatim.
- Gemini (`"provider": "gemini"`): `thinking` (`include_thoughts`,
  `thinking_level`, `thinking_budget`), `thinking_level` (`minimal`, `low`,
  `medium`, `high`; each model's `params_schema` lists the levels it accepts:
  `gemini-4-argon`, `gemini-3.8-flash`, `gemini-3.7-flash` and
  `gemini-3.1-pro-preview` refuse `minimal`), `thinking_budget`, `top_k`, `top_p`,
  `cached_content_name`.

```bash
rkat run -m gpt-5.6-terra --params-json '{"provider_tag":{"provider":"open_ai","reasoning_effort":"max","reasoning_mode":"pro"}}' "Prove it"
rkat run -m gemini-3.8-flash --params-json '{"provider_tag":{"provider":"gemini","thinking_level":"medium"}}' "Analyze"
rkat run -m claude-sonnet-4-6 --param temperature=0.2 "Draft it"
```

Prompt caching uses the `provider_tag` keys above:

- Anthropic: `cache_control` defaults to `automatic` on the `anthropic_api`,
  `vertex` and `foundry` backends and to `disabled` on `bedrock` and
  `copilot`; those two refuse `automatic` but accept `system_prefix` and
  `system_and_conversation`. `cache_ttl` (`5m` default, `1h`) is refused
  when the effective `cache_control` is `disabled` (so on `bedrock` and
  `copilot` set a manual mode first). Opt out with
  `cache_control = "disabled"`.
- OpenAI: only catalog rows with cache support get defaults. The GPT-5.6
  rows use `explicit` mode and the GPT-6 rows `implicit`, both with TTL
  `30m` and `prompt_cache_key = "meerkat:profile:openai:<model>"`, shared
  across sessions (a caller key wins). These rows refuse
  `prompt_cache_retention = "in_memory"` when the agent is built.
  `prompt_cache_enabled = false` opts out: the request then carries only
  `prompt_cache_options = {"mode": "explicit"}` with no breakpoints, so
  nothing is cached.
- OpenAI backends: only the `openai_api` backend receives these fields
  (backend profiles report `prompt_cache_applicable`). Every other route
  drops them, whether they came from model defaults or an explicit tag:
  `chatgpt_backend`, `azure_openai`, self-hosted OpenAI-compatible servers
  (Chat Completions and Responses) and the Copilot routes; a disabled cache
  sends nothing there. 0.9.0 still sent them to self-hosted and Copilot
  routes; fixed in 0.9.1.
- Gemini: `cached_content_name` names an explicit context cache you created
  (`cachedContents/<id>`); the Copilot route refuses it. Meerkat sets no
  Gemini cache defaults.
- Usage reports `cache_read_tokens` and `cache_creation_tokens`. The
  `provider_cache_breakpoints_discarded` event records cache breakpoints
  dropped at a turn commit; the turn still completes with weaker caching.

```bash
rkat run -m claude-opus-5-5 --params-json '{"provider_tag":{"provider":"anthropic","cache_control":"system_and_conversation","cache_ttl":"1h"}}' "Summarize the repo"
```

Provider-native web search is on for models that support it. Turn it off per
run with `rkat run --no-web-search`, or in config with
`[provider_tools.anthropic] web_search = false`,
`[provider_tools.openai] web_search = false`,
`[provider_tools.gemini] google_search = false`.

## Realtime, voice and image models

- `gpt-realtime-2`: OpenAI Realtime voice model (text, audio and image input;
  provider-managed turns or explicit commit; input transcription by
  `gpt-4o-mini-transcribe`; no structured output). A session must use it (or
  another realtime row) before opening a live channel; the `openai-realtime`
  feature is in the default `rkat` and `rkat-rpc` builds.
- `gpt-live-1`: public OpenAI Live voice model, opened only through
  `live/open` with an execution identity over WebRTC and an OpenAI API-key
  binding. Requires the opt-in `openai-live` build feature, which default
  binaries do not enable.
- `gpt-live-1-codex`: experimental private path (`experimental-gpt-live`),
  deprecated since 0.8.38; use `gpt-live-1`.
- Image generation (`generate_image` builtin) models: OpenAI `gpt-image-2`
  (default), `gpt-image-1`, `dall-e-3`, `dall-e-2`; Gemini
  `gemini-nano-banana-2.1` (default since 0.9.1),
  `gemini-3.1-flash-image-preview` (the 0.9.0 default; select it by model to
  keep it), `gemini-3-pro-image-preview`, `gemini-2.5-flash-image`.

## Self-hosted and OpenAI-compatible endpoints

Ollama, LM Studio, vLLM and private gateways are servers plus model aliases,
authenticated by a realm binding:

```toml
[self_hosted]
default_model = "gemma-4-31b"     # required when more than one alias exists

[self_hosted.servers.ollama]
transport = "openai_compatible"   # the only transport
base_url = "http://127.0.0.1:11434"
api_style = "chat_completions"    # default; or "responses"

[self_hosted.models.gemma-4-31b]
server = "ollama"
remote_model = "gemma4:31b"       # the name the server expects
context_window = 256000
max_output_tokens = 8192
vision = true
supports_reasoning = true

[realm.global.backend.local]
provider = "self_hosted"
backend_kind = "self_hosted"      # or "openai_compatible"
server = "ollama"                 # the server this credential authenticates

[realm.global.auth.local_auth]
provider = "self_hosted"
auth_method = "none"              # or "api_key" / "static_bearer"
source = { kind = "platform_default" }

[realm.global.binding.local]
backend_profile = "local"
auth_profile = "local_auth"
```

```bash
rkat run -m gemma-4-31b "Say hello"   # provider inferred from the alias
rkat run -p self-hosted "Say hello"   # uses [self_hosted] default_model
```

- Meerkat appends `/v1` to a server `base_url` that does not end in `/v1`,
  then calls `<base>/chat/completions` (or the Responses API). A `base_url` on
  the realm backend profile replaces the server's and is used as written.
- Other alias keys: `display_name`, `family`, `tier`, `max_input_tokens`,
  `image_tool_results` (effective only with `api_style = "responses"`),
  `inline_video`, `supports_temperature` (default true), `supports_thinking`,
  `call_timeout_secs`, `stream_inactivity_timeout_secs` (this model's
  stream-inactivity window in seconds, at least 60, for a long prefill for
  example; omitted: the `[retry]` window), `supports_web_search` (keep false:
  there is no self-hosted native search).
- Self-hosted credentials come only from realm bindings (realm model in
  references/providers_and_auth_credentials.md); there is no env-var
  fast path. Use `auth_method = "static_bearer"` with
  `source = { kind = "env", env = "LOCAL_LLM_TOKEN" }` for a token. The legacy
  server keys `bearer_token` and `bearer_token_env` are rejected.
- The binding is chosen by the model's server: the nearest binding that
  names that server wins (child realm first; within a realm
  `default_binding`, then `provider_default`, then id order). A binding names
  a server through its backend's `server = "<id>"` or, without one, through
  a `default_model` that is an alias on that server; both count the same. A
  backend that declares one server while its `default_model` sits on another
  never matches. With no naming binding, the only binding that names no
  server is used; otherwise selection fails closed and lists every binding
  considered. An explicit `--auth-binding` is accepted unless its backend
  declares a different server.
- Native clients follow no HTTP redirects; configure the final URL (a `3xx`
  fails the call; see references/providers_and_auth_credentials.md).
- Check with `rkat models`, `rkat doctor` (probes `<base>/models` with the
  resolved credential) and `rkat auth test --realm global local`.

## OpenAI Responses and compatible streams

- Function tools go to the Responses API with `"strict": false`, so optional
  parameters stay optional (the API could otherwise make them required);
  Meerkat's tool dispatcher validates the arguments.
- Requests are stateless (`store` defaults to false) and every turn replays
  the transcript. Assistant messages keep their Responses item `id` and
  `phase` (`commentary` or `final_answer`), and reasoning items keep their
  `encrypted_content` (requested with `include:
  ["reasoning.encrypted_content"]` when reasoning is on). These replay in
  order with the tool calls and outputs and survive session save and
  resume. A provider switch or Chat Completions mode never receives this
  OpenAI-private state.
- With `store = true` in an `open_ai` tag, the client continues from
  `previous_response_id` and falls back to a full replay when the server
  rejects that id.
- Hosted web search on the public OpenAI API: a request with the hosted
  `web_search` tool also asks for `web_search_call.action.sources`, and
  later steps replay the search call with those sources and the answer with
  its `url_citation` annotations (only citations whose range fits the
  replayed text, and only for an answer of one `output_text` part; its
  citations block records `text_parts`). The pages' text never comes back;
  for that, use `store = true` or the Meerkat `web_search` fallback tool.
  The ChatGPT backend, Azure OpenAI and Copilot wires replay the search call
  without sources and the answer with `annotations: []`.
- Unknown output item types or execution semantics (async, namespaced or
  programmatic function calls) fail the turn instead of being dropped.
- OpenAI-compatible Chat Completions streams (vLLM and similar): after a
  `finish_reason`, the client waits at most 30 s for the usage trailer or
  `[DONE]`. It is one budget from the finish; SSE keepalives do not extend
  it, and expiry ends the stream normally without invented usage. An error
  envelope in the stream (`{"object": "error", ...}` or
  `{"error": {"message": ...}}`) fails the turn with
  `provider error event: <message>` unless the answer had already finished.

## Refusals and content filters

Since 0.9.4 a refusal or a provider content filter ends the response with a
typed stop, never as answer text. Nothing of the turn is committed and none
of its tool calls run. `run_failed` carries `error_report.class =
"content_filtered"` and `reason = {"reason_type": "content_filtered", "stop":
"refusal" | "content_filter", "detail": {...}}` (0.9.3 and earlier sent no
reason). `detail` is the provider's own account, tagged by `provider`, and is
omitted when the provider sent none:

| `provider` | `detail` fields |
|---|---|
| `anthropic` | `category` (policy category, for example `cyber`), `explanation` (display text, not stable) |
| `open_ai` | `reason` (`refusal`, or the raw filter reason), `refusal` (the refusal text) |
| `gemini` | `stage` (`prompt` or `candidate`), `reason` (`blockReason` or `finishReason`), `blocked_categories` |

Each text field is capped at 4 KiB; evidence of an unexpected shape reads as
absent and never fails the stream. A 0.9.3 binary cannot read a session event
log that holds such a `run_failed` line.

- OpenAI: a Responses `refusal` output part (streamed as
  `response.refusal.delta`/`.done`, or in the terminal response) and a Chat
  Completions `delta.refusal` end the response as a refusal; the refusal
  wins over text and tool calls in the same response. A Responses
  `incomplete_details.reason` or a Chat `finish_reason` of `content_filter`
  ends it as a content filter. Self-hosted OpenAI-compatible servers are read
  the same way.
- Anthropic: a `refusal` stop carries its `stop_details`, read from the
  `message_delta` (inside or beside its delta) or from `message_stop`; the
  `message_delta` value wins. A stream cut after the stop still ends as a
  refusal.
- Gemini: a prompt blocked before any candidate (`promptFeedback.blockReason`,
  any reason including `OTHER`) fails after one call and is not retried: the
  same prompt is blocked again. A candidate `finishReason` of `SAFETY`,
  `RECITATION`, `BLOCKLIST`, `PROHIBITED_CONTENT`, `SPII`, `IMAGE_SAFETY`,
  `IMAGE_PROHIBITED_CONTENT`, `IMAGE_RECITATION` or `LANGUAGE` is a content
  filter; `OTHER`, `MALFORMED_FUNCTION_CALL` or an unknown reason is retried
  as an incomplete response.
- The `web_search` fallback tool reads its native search call's stop: a
  refusal, content filter, filled context window or cancellation returns
  `status: "failed"` with an `error` naming the stop and no answer, evidence
  or native events. A call cut at `max_tokens` or paused (`pause_turn`) keeps
  its status and answer, and its `error` says the answer may be incomplete.

## Errors and troubleshooting

A provider failure that ends a run emits `run_failed`; its
`error_report.reason` has `reason_type = "llm_provider_error"`, `provider_error_kind` (for example
`quota_exhausted`, `policy_stop`, `request_too_large`, `server_error`),
`provider_error_retryability` (`retryable` or `non_retryable`) and
`provider_error` (the provider's code and message). SDK event types carry the
same fields. A refusal or content filter has its own reason (see Refusals and
content filters).

| Symptom | Cause and fix |
|---|---|
| Plain `rkat run` asks for OpenAI credentials, or ignores a login | The global default is `gpt-6-astra`. Pass `-m`, `-p` or `--auth-binding global:<binding>`, or set `rkat --default-model`. |
| `cannot infer provider from model '<id>'` | Not in the registry. Fix the id (`rkat models`) or add `[models.<id>]` / `[self_hosted.models.<alias>]`. |
| `model profile '<p>:<m>' is unavailable` | A provider hint with an unregistered model; declare it under `[models.<id>]`. |
| `provider_error_kind` `quota_exhausted` or `policy_stop` | Non-retryable: the turn fails on the first answer. Quota: HTTP 402, a 429 naming exhausted quota or a spend cap (`insufficient_quota`, an Anthropic usage limit), or Gemini `429 RESOURCE_EXHAUSTED` whose `QuotaFailure` names a daily (`...PerDay...`) window or a zero quota; per-minute Gemini limits stay retryable rate limits. Top up or wait for the reset. Policy: the provider stopped the conversation for review (`misalignment_policy_violation`); it never triggers retry or model fallback. |
| `[agent] provider_params is not applied to any session` | Move the table to the session request, mob profile or `AgentBuildConfig`, under `provider_tag`. |

## Not available or superseded

- There is no `rkat models catalog` and no `--json` on `rkat models`.
- Realm `[agent] provider_params`: refused since 0.8.34; set
  `provider_params` per session, mob profile or `AgentBuildConfig`.
- `gpt-live-1-codex` / `experimental-gpt-live`: deprecated since 0.8.38; use
  `gpt-live-1` with `openai-live`.
