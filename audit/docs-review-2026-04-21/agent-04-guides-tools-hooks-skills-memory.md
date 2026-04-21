# Agent 04 Findings

## Scope reviewed

- Guides:
  - `docs/guides/hooks.mdx`
  - `docs/guides/skills.mdx`
  - `docs/guides/memory.mdx`
  - `docs/guides/structured-output.mdx`
- Examples:
  - `docs/examples/hooks.mdx`
  - `docs/examples/skills.mdx`
  - `docs/examples/memory.mdx`
  - `docs/examples/tools.mdx`
  - `docs/examples/structured-output.mdx`
- Current implementation checked for parity in:
  - `meerkat-core`
  - `meerkat-hooks`
  - `meerkat-memory`
  - `meerkat-cli`
  - `meerkat-rest`
  - `meerkat-rpc`
  - `meerkat-mcp-server`
  - `sdks/python`
  - `sdks/typescript`

## Files reviewed

- `docs/guides/hooks.mdx`
- `docs/guides/skills.mdx`
- `docs/guides/memory.mdx`
- `docs/guides/structured-output.mdx`
- `docs/examples/hooks.mdx`
- `docs/examples/skills.mdx`
- `docs/examples/memory.mdx`
- `docs/examples/tools.mdx`
- `docs/examples/structured-output.mdx`
- `meerkat-core/src/hooks.rs`
- `meerkat-core/src/config.rs`
- `meerkat-core/src/agent.rs`
- `meerkat-core/src/agent/runner.rs`
- `meerkat-core/src/agent/state.rs`
- `meerkat-core/src/service/mod.rs`
- `meerkat-core/src/types.rs`
- `meerkat-core/src/budget.rs`
- `meerkat-core/src/event.rs`
- `meerkat-hooks/src/lib.rs`
- `meerkat-memory/src/tool.rs`
- `meerkat/src/factory.rs`
- `meerkat-cli/src/main.rs`
- `meerkat-rest/src/lib.rs`
- `meerkat-rpc/src/handlers/session.rs`
- `meerkat-mcp-server/src/lib.rs`
- `sdks/python/meerkat/client.py`
- `sdks/python/meerkat/session.py`
- `sdks/typescript/src/client.ts`

## Findings

1. `docs/examples/hooks.mdx` shows a hook-override wire shape that no longer deserializes.
   - The examples at `docs/examples/hooks.mdx:16-24`, `docs/examples/hooks.mdx:125-133`, and `docs/examples/hooks.mdx:261-268` put `command` or `url` directly on each entry and use `mode: "blocking"`.
   - The live wire type is `HookEntryConfig { id, enabled, point, mode, capability, priority, failure_policy, timeout_ms, runtime }`, where `mode` is `foreground` or `background`, and runtime config must be nested under `runtime`/`type` rather than flattened onto the entry itself. See `meerkat-core/src/config.rs:1525-1539`, `meerkat-core/src/hooks.rs:69-84`, and `meerkat-core/src/config.rs:1642-1679`.
   - As written, these examples omit required fields like `capability` and `runtime`, and use an invalid enum value for `mode`, so copy-pasting them into CLI/REST/RPC/MCP calls should fail before hook execution starts.

2. `docs/guides/hooks.mdx` is missing the current image-content caveat for tool-result hooks.
   - The guide’s supporting-type summary at `docs/guides/hooks.mdx:199-204` documents `HookToolResult` as `tool_use_id`, `name`, `content`, `is_error`, but the current type also includes `has_images`. See `meerkat-core/src/hooks.rs:222-234`.
   - This matters because hook authors only receive the text projection of tool output; non-text content is signaled separately via `has_images`, and `HookPatch::ToolResult` preserves image blocks instead of replacing the entire result. See `meerkat-core/src/hooks.rs:305-329`.
   - Without that caveat, users can incorrectly assume a rewrite hook has full fidelity over multimodal tool results.

3. The skills guide/examples still document legacy string refs and CLI behavior that do not match the current preferred API.
   - `docs/guides/skills.mdx:220-250` still promotes slash-prefixed user-message activation and frames legacy strings as routine compatibility input. In core, per-turn activation now comes from canonical `SkillKey` values staged by the surface; the runner explicitly notes that slash refs are no longer parsed directly in the core runtime. See `meerkat-core/src/agent/runner.rs:1011-1016`.
   - `docs/examples/skills.mdx:145-165`, `docs/examples/skills.mdx:188-200`, and `docs/examples/skills.mdx:307-367` use string arrays like `["/formatting/markdown"]` for `skill_refs`/`skill_references`, and the Python per-turn example at `docs/examples/skills.mdx:343-346` is also missing `await`.
   - The Python and TypeScript SDKs now normalize `skill_refs` to structured `{ source_uuid, skill_name }` payloads, and their legacy string fallback expects `"<source_uuid>/<skill_name>"`, not a bare skill ID. See `sdks/python/meerkat/session.py:37-60`, `sdks/python/meerkat/client.py:2485-2489`, `sdks/typescript/src/client.ts:185-205`, and `sdks/typescript/src/client.ts:2799-2802`.
   - The CLI `--skill` flag only resolves runtime-local skill sources and preloads skill IDs into `preload_skills`; it is not a per-turn `skill_refs` surface. See `meerkat-cli/src/main.rs:2471-2491` and `meerkat-cli/src/main.rs:2192-2255`.

4. `docs/guides/memory.mdx` conflates semantic memory enablement with compaction enablement.
   - The guide says at `docs/guides/memory.mdx:16-42` that both systems require “per-request enablement” and that `AgentFactory.memory(true)` enables “semantic memory + compaction”.
   - In the factory, `override_memory` only gates the memory store and `memory_search` tool. The compactor is wired independently whenever the binary is built with `session-compaction`, regardless of the memory toggle. See `meerkat/src/factory.rs:2726-2759` and `meerkat/src/factory.rs:2761-2768`.
   - This is an important library-user distinction: compaction can be on without semantic memory, and `enable_memory` is not the runtime switch for compaction.

5. The memory guide/examples contain stale CLI and budget parameter guidance.
   - `docs/guides/memory.mdx:29` says `cargo build -p meerkat-cli`, but the CLI package name is `rkat`. See `meerkat-cli/Cargo.toml:1-17`.
   - `docs/guides/memory.mdx:223-225` suggests `rkat run --memory`, but there is no `--memory` CLI flag. The CLI only toggles memory through tool presets, with `--tools full` setting the memory override. See `meerkat-cli/src/main.rs:1171-1183` and `meerkat-cli/src/main.rs:400-427`.
   - `docs/examples/memory.mdx:185-189` and `docs/examples/memory.mdx:228-232` use `max_total_tokens` and `max_duration_ms` for RPC/REST/Python budget payloads, and `docs/examples/memory.mdx:268-272` uses `"budget_type": "max_tokens"`. The live structs/events use `BudgetLimits { max_tokens, max_duration, max_tool_calls }` and `BudgetType::{tokens,time,tool_calls}`. See `meerkat-core/src/budget.rs:10-19`, `meerkat-rpc/src/handlers/session.rs:101-103`, `meerkat-rest/src/lib.rs:832-834`, `meerkat-mcp-server/src/lib.rs:981-999`, and `meerkat-core/src/event.rs:392-406` plus `meerkat-core/src/event.rs:540-548`.

6. `docs/examples/tools.mdx` still uses pre-refactor Rust SDK interfaces for custom tools and session creation.
   - The custom tool snippet at `docs/examples/tools.mdx:33-41` returns `Result<ToolResult, ToolError>`, but `AgentToolDispatcher::dispatch` now returns `Result<ToolDispatchOutcome, ToolError>`. See `meerkat-core/src/agent.rs:275-283`.
   - The Rust session-creation snippet at `docs/examples/tools.mdx:137-141` relies on `CreateSessionRequest { ..Default::default() }`, but `CreateSessionRequest` is not `Default` and now requires fields such as `model`, `prompt`, `render_metadata`, `initial_turn`, and `deferred_prompt_policy`. See `meerkat-core/src/service/mod.rs:148-171`.
   - That makes the example misleading for library users trying to build a minimal custom-tool integration from the docs.

7. The structured-output guide/examples still carry removed surface names and outdated Rust API shapes.
   - `docs/guides/structured-output.mdx:264-279` still documents a REST `POST /v1/run` endpoint, but the REST server exposes `POST /sessions`. See `meerkat-rest/src/lib.rs:1084-1090` and `docs/api/rest.mdx:55-60`.
   - `docs/guides/structured-output.mdx:285-294` introduces a `StructuredOutputParams` type that does not exist in the current codebase; current surfaces carry `output_schema` and `structured_output_retries` directly on session/turn request structs. See `meerkat-rpc/src/handlers/session.rs:107-115` and `meerkat-rest/src/lib.rs:838-846`.
   - The Rust snippets in `docs/examples/structured-output.mdx:127-140` and `docs/examples/structured-output.mdx:227-241` are also stale for the same `CreateSessionRequest` reason noted above (`meerkat-core/src/service/mod.rs:148-171`).

## Suggested follow-ups

- Rewrite `docs/examples/hooks.mdx` so every override entry uses the actual `HookEntryConfig` wire shape:
  - include `mode: "foreground" | "background"`
  - include `capability`
  - nest command/http config under `runtime`
- Update `docs/guides/hooks.mdx` to document `HookToolResult.has_images` and explicitly state that tool-result patches preserve image blocks.
- Refresh `docs/guides/skills.mdx` and `docs/examples/skills.mdx` around current activation paths:
  - CLI `--skill` = preload only
  - API/SDK `skill_refs` = structured `SkillKey`
  - legacy string refs require `<source_uuid>/<skill_name>` and should be labeled deprecated everywhere
- Split memory docs into two clearly separate stories:
  - compaction availability (`session-compaction`)
  - semantic memory availability (`memory-store-session` plus runtime memory enablement)
- Fix all budget examples to use the live field names for each surface, and align event examples with `BudgetType` serialization (`tokens`, `time`, `tool_calls`).
- Refresh Rust snippets in `docs/examples/tools.mdx` and `docs/examples/structured-output.mdx` against the current `CreateSessionRequest` and `AgentToolDispatcher` signatures before the next docs publish.
