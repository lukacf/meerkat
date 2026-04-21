# Agent 02 Findings

## Scope reviewed

- Concepts docs audit for configuration/provider/realm/session/tool behavior.
- Validation against current implementation and adjacent reference docs.

## Files reviewed

- `docs/concepts/configuration.mdx`
- `docs/concepts/providers.mdx`
- `docs/concepts/realms.mdx`
- `docs/concepts/sessions.mdx`
- `docs/concepts/tools.mdx`
- `meerkat-core/src/config.rs`
- `meerkat-core/src/config_runtime.rs`
- `meerkat-core/src/config_store.rs`
- `meerkat-core/src/session_locator.rs`
- `meerkat-core/src/agent.rs`
- `meerkat-core/src/types.rs`
- `meerkat-core/src/model_profile/capabilities/anthropic.rs`
- `meerkat-core/src/model_profile/capabilities/openai.rs`
- `meerkat-core/src/model_profile/capabilities/gemini.rs`
- `meerkat-core/src/model_profile/schema_builder.rs`
- `meerkat/src/factory.rs`
- `meerkat-rpc/src/handlers/config.rs`
- `meerkat-rpc/src/handlers/mcp.rs`
- `meerkat-rpc/src/router.rs`
- `meerkat-rpc/src/session_runtime.rs`
- `meerkat-rest/src/lib.rs`
- `meerkat-mcp-server/src/lib.rs`
- `docs/reference/builtin-tools.mdx`
- `docs/reference/session-contracts.mdx`
- `docs/api/mcp.mdx`

## Findings

1. `docs/concepts/configuration.mdx:25-33` still describes environment variables as a normal config-precedence layer, but the current config loader no longer mutates `Config` from env at all. `Config::load()` still mentions `defaults -> file -> env`, yet `apply_env_overrides_from()` is now an intentional no-op with a comment explaining that provider credentials are resolved later through binding resolution rather than written into config state (`meerkat-core/src/config.rs:106-143`, `meerkat-core/src/config.rs:576-598`). The concept page should stop implying that env vars override realm `config.toml` fields and instead distinguish runtime credential lookup from persisted realm config.

2. `docs/concepts/providers.mdx:17-23` has a stale/misleading Anthropic model table. It lists `claude-opus-4-6` twice, including one row that reads like the top-tier/current recommendation, while the actual curated catalog marks `claude-opus-4-7` as the recommended Opus row and `claude-opus-4-6` as the supported fallback (`meerkat-core/src/model_profile/capabilities/anthropic.rs:86-114`, `meerkat-core/src/model_profile/capabilities/anthropic.rs:127-155`). As written, the table can make readers think 4.6 is both the highest-quality choice and a fallback at the same time.

3. `docs/concepts/providers.mdx:145-152` has stale OpenAI parameter guidance. It says `reasoning_effort` is for “o-series models” and only documents `low`, `medium`, and `high`, but the current GPT-5.4 path is what actually uses this field, and the catalog/client support `none`, `low`, `medium`, `high`, and `xhigh` (`meerkat-core/src/model_profile/capabilities/openai.rs:10-18`, `meerkat-core/src/model_profile/capabilities/openai.rs:31-70`, `meerkat-openai/src/client.rs:189-197`). This should be rewritten around GPT-5-era behavior, not older o-series wording.

4. `docs/concepts/realms.mdx:11-15` overstates realm ID freedom by calling realms “opaque strings.” Explicit realm IDs are validated across surfaces: they must be 1-64 chars, start with ASCII alphanumeric, use only ASCII alnum / `_` / `-`, cannot contain whitespace or `:`, and UUID-like values are rejected (`meerkat-contracts/src/session_locator.rs:42-63`). The docs should describe realm IDs as user-chosen stable identifiers with syntax constraints, not arbitrary opaque strings.

5. `docs/concepts/sessions.mdx:11-20` and `docs/concepts/sessions.mdx:43-54` miss an important archived-session behavior: archive removes the live/runtime handle and hides the session from normal listing, but committed history remains readable after archive on the primary runtime-backed path. That behavior is explicitly covered in tests (`meerkat-rpc/src/router.rs:5646-5670`) and aligns with the reference contract (`docs/reference/session-contracts.mdx:113-126`). The concepts page currently makes archive sound like full removal only, which is too lossy for users reasoning about audit/history retention.

6. `docs/concepts/tools.mdx:11-43` shows an outdated custom-dispatcher API. The example still returns `Result<ToolResult, ToolError>`, but `AgentToolDispatcher::dispatch()` now returns `Result<ToolDispatchOutcome, ToolError>` and synchronous results are wrapped into that outcome (`meerkat-core/src/agent.rs:275-283`; see current example style in `examples/006-custom-tools-rs/main.rs:81-140`). Readers copying the concept example will implement the wrong signature.

7. `docs/concepts/tools.mdx:63-70` has an incomplete built-in category list. Current agent factories also expose schedule tools behind `enable_schedule`, but the page only lists builtins, shell, memory, comms, and mob (`meerkat/src/factory.rs:797-803`, `meerkat/src/factory.rs:1045-1048`). Schedule tooling is live enough that the repo has RPC handlers and tool-name coverage for it (`meerkat-rpc/src/handlers/schedule.rs:423-427`). The concepts page should either include schedule explicitly or explain why it is intentionally omitted.

8. `docs/concepts/tools.mdx:77-83` conflates live MCP mutation with external tool filtering. The page says callers can stage allow/deny filters via session APIs such as `mcp/add` and `mcp/remove`, but those handlers only stage MCP server lifecycle changes (`meerkat-rpc/src/handlers/mcp.rs:13-169`, `meerkat-rest/src/lib.rs:4304-4376`). The actual allow/deny filter mechanism is `ToolScopeHandle::stage_external_filter(...)` in-process (`meerkat-core/src/tool_scope.rs:942-947`), and I did not find a parallel public RPC/REST control plane for it in the current repo. This is conceptually misleading because it suggests a public feature surface that does not exist.

9. `docs/concepts/tools.mdx:107-112` says REST live MCP support is only “Route stubs registered,” but the REST server wires real handlers for `POST /sessions/{id}/mcp/add|remove|reload` and those handlers stage the requested mutations (`meerkat-rest/src/lib.rs:1203-1207`, `meerkat-rest/src/lib.rs:4304-4376`). This should be updated from placeholder language to active support.

10. `docs/concepts/tools.mdx:115-129` contains stale multimodal result details. The page says `ContentBlock` has only text and image variants and documents an image `source_path` field, but the current core type includes `Text`, `Image`, and `Video`, and `ContentBlock::Image` itself no longer carries `source_path` on the wire shape (`meerkat-core/src/types.rs:70-95`). The general custom-tool return path is also centered on `ToolResult.content` / `ToolResult::with_blocks(...)` rather than only the built-in-tool `ToolOutput` abstraction (`meerkat-core/src/types.rs:1121-1164`). The section should be updated so readers do not internalize an obsolete content model.

## Suggested follow-ups

- Update `docs/concepts/configuration.mdx` to separate “persisted realm config precedence” from “runtime credential resolution,” and remove the claim that env vars currently override config fields.
- Refresh `docs/concepts/providers.mdx` from the curated capability rows instead of hand-maintained summaries, especially for Anthropic recommendations and OpenAI reasoning-effort values.
- Tighten `docs/concepts/realms.mdx` and `docs/concepts/sessions.mdx` around operational contracts: realm ID syntax, archive semantics, and what remains readable after archive.
- Rework `docs/concepts/tools.mdx` against current source-of-truth docs and types. The highest-priority fixes are the stale dispatcher signature, missing schedule tools, incorrect external-filter description, incorrect REST live-MCP status, and obsolete multimodal block shape.
- After the concepts pages are fixed, sweep adjacent docs that likely copied the same outdated tool examples/status text, especially `docs/examples/tools.mdx` and `docs/reference/builtin-tools.mdx`.
