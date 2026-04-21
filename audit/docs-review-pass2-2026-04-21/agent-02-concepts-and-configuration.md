# Agent 02 Findings

## Scope reviewed

- `docs/concepts/configuration.mdx`
- `docs/concepts/providers.mdx`
- `docs/concepts/realms.mdx`
- `docs/concepts/sessions.mdx`
- `docs/concepts/tools.mdx`

This pass compared the concept docs against the current runtime/config/session/tooling code and adjacent reference docs for config envelopes, model capability gating, provider inference, and archived-session behavior.

## Files reviewed

- Concept docs:
  - `docs/concepts/configuration.mdx`
  - `docs/concepts/providers.mdx`
  - `docs/concepts/realms.mdx`
  - `docs/concepts/sessions.mdx`
  - `docs/concepts/tools.mdx`
- Key implementation and reference checks:
  - `meerkat-core/src/config_runtime.rs`
  - `meerkat-rest/src/lib.rs`
  - `meerkat-rpc/src/handlers/config.rs`
  - `meerkat-mcp-server/src/lib.rs`
  - `meerkat-core/src/provider.rs`
  - `meerkat-core/src/model_profile/capabilities/openai.rs`
  - `docs/reference/capability-matrix.mdx`
  - `meerkat-rpc/src/router.rs`
  - `meerkat-rest/tests/live_rest_regression.rs`

## Findings

1. `docs/concepts/configuration.mdx:41-48` and `docs/concepts/realms.mdx:75-79` overstate `resolved_paths` as part of one universal config envelope across surfaces.

   The runtime now has explicit public vs diagnostic config-envelope policies. `ConfigEnvelope.resolved_paths` is optional, and `ConfigEnvelopePolicy::Public` strips it entirely (`meerkat-core/src/config_runtime.rs:31-68`). RPC currently returns the diagnostic shape (`meerkat-rpc/src/handlers/config.rs:38-42`), but REST and MCP only include `resolved_paths` when `expose_paths` is enabled; otherwise they return the public shape without it (`meerkat-rest/src/lib.rs:2589-2596`, `meerkat-rest/src/lib.rs:2617-2624`, `meerkat-rest/src/lib.rs:2651-2658`, `meerkat-mcp-server/src/lib.rs:1670-1679`). As written, both concept pages imply `resolved_paths` is always present everywhere, which no longer matches product behavior.

2. `docs/concepts/providers.mdx:7` is too strong when it says tools work identically across Anthropic, OpenAI, Gemini, and self-hosted models.

   Session/config surfaces are largely provider-agnostic, but tool visibility and result handling are capability-gated per model. The capability matrix explicitly calls out that `view_image` is hidden when either `vision` or `image_tool_results` is false (`docs/reference/capability-matrix.mdx:88-93`). The curated OpenAI row for `gpt-5.4` sets `image_tool_results: false` (`meerkat-core/src/model_profile/capabilities/openai.rs:39-66`), so OpenAI sessions do not actually get the same effective tool surface as Anthropic/Gemini for image-returning tools. The opening copy should be softened to reflect “shared session/config model with capability-gated differences,” otherwise it promises stronger cross-provider equivalence than the runtime provides.

3. `docs/concepts/providers.mdx:182-186` has stale provider auto-detection guidance.

   The page says OpenAI inference covers `gpt-*`, `o1-*`, `o3-*`, and `chatgpt-*`, but the current provider inference implementation only recognizes `gpt-*` and `chatgpt-*` for OpenAI (`meerkat-core/src/provider.rs:41-61`). That means the docs currently promise automatic provider inference for `o1-*` and `o3-*` names that the runtime will not infer.

4. `docs/concepts/sessions.mdx:19` and `docs/concepts/sessions.mdx:55` under-describe archived-session readability and make archive sound closer to “hide + history only” than the real lifecycle contract.

   The current RPC behavior explicitly keeps archived sessions readable via `session/read` while rejecting mutating operations afterward (`meerkat-rpc/src/router.rs:4297-4345`). REST also treats archived reads as potentially still available on persistent services, not as a hard-delete guarantee (`meerkat-rest/tests/live_rest_regression.rs:297-314`). The concept page does explain that history remains available, but it omits the more important mental model: archiving retires the live runtime handle and blocks further mutation, while read availability can still persist depending on the surface/service. That omission can mislead readers into treating `archive` like deletion.

5. No additional factual mismatches found in `docs/concepts/tools.mdx`.

   The dispatcher example, built-in category split, staged tool-scope behavior, live MCP mutation notes, multimodal `ToolResult` description, and mob-tool composition all matched the current code/reference docs closely enough for this pass.

6. No additional factual mismatches found in `docs/concepts/realms.mdx` beyond the shared `resolved_paths` wording called out in finding 1.

   Realm ID validation, workspace-vs-opaque defaulting, backend pinning, and SQLite-as-default-persistent-backend messaging all matched the current runtime bootstrap/store behavior.

## Suggested follow-ups

- Update `configuration.mdx` and `realms.mdx` to describe config envelopes as `config`, `generation`, `realm_id`, `instance_id`, and `backend` everywhere, with `resolved_paths` called out as diagnostic/opt-in rather than universal.
- Rephrase the `providers.mdx` intro so it distinguishes portable session/config concepts from provider-specific capability gating, and cross-link to the capability matrix or tools docs for examples like `view_image`.
- Fix the auto-detection list in `providers.mdx` to match `Provider::infer_from_model(...)`, or expand the implementation if those legacy prefixes are still meant to resolve.
- Expand `sessions.mdx` archive language so it says archived sessions keep committed state, reject new mutation/streaming work, and may remain readable depending on the surface/service implementation.
- Optional docs improvement: `providers.mdx` could use a short “modern parameter caveats” note for Anthropic/Gemini, since the current page only shows legacy `thinking_budget` examples and does not explain newer capability-specific knobs such as Anthropic `effort` or Gemini’s still-unwired `thinking_level`.
