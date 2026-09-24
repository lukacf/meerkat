# Agent 04 Findings

## Scope reviewed

- Guides and examples for hooks, skills, memory/compaction, structured output, and tools/MCP.
- Current runtime/code behavior for hook registration, skill parsing/resolution, memory wiring, and structured-output validation.

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
- `meerkat-cli/src/main.rs`
- `meerkat-core/src/config.rs`
- `meerkat-core/src/hooks.rs`
- `meerkat-core/src/schema.rs`
- `meerkat-core/src/types.rs`
- `meerkat-core/src/agent/extraction.rs`
- `meerkat-core/src/agent/state.rs`
- `meerkat-core/src/agent/skills.rs`
- `meerkat-core/src/skills/mod.rs`
- `meerkat-core/src/skills_config.rs`
- `meerkat-hooks/src/lib.rs`
- `meerkat-skills/src/parser.rs`
- `meerkat-skills/src/renderer.rs`
- `meerkat-skills/src/resolver.rs`
- `meerkat-skills/src/resolve.rs`
- `meerkat-memory/src/lib.rs`
- `meerkat-memory/src/tool.rs`
- `meerkat-memory/src/hnsw.rs`
- `meerkat-memory/src/simple.rs`
- `meerkat-tools/src/builtin/skills/mod.rs`
- `meerkat-tools/src/lib.rs`
- `meerkat-openai/src/client.rs`
- `meerkat-anthropic/src/client.rs`
- `meerkat-gemini/src/client.rs`
- `meerkat/src/factory.rs`
- `meerkat/src/sdk.rs`
- `meerkat/Cargo.toml`
- `meerkat-cli/Cargo.toml`

## Findings

- Hooks: the CLI examples are currently documenting flags that do not exist on `rkat run`. `docs/guides/hooks.mdx:289-311` and `docs/examples/hooks.mdx:14-30`, `docs/examples/hooks.mdx:153-169`, `docs/examples/hooks.mdx:320-336`, `docs/examples/hooks.mdx:476-482` all use `--hooks-override-json` / `--hooks-override-file`, but the `Run` clap args do not define those options in `meerkat-cli/src/main.rs:1013-1226`. The only parser for those flags is test-only (`#[cfg(test)]`) at `meerkat-cli/src/main.rs:2509-2537`. Library-user impact: the documented CLI path for trying hooks in isolation is not available in normal builds.

- Skills: the documented `SKILL.md` examples use invalid frontmatter for the current parser. `docs/guides/skills.mdx:54-74` and `docs/guides/skills.mdx:184-195` show human-readable `name` values like `Shell Patterns` / `Deployment Guide`, but `meerkat-skills/src/parser.rs:83-137` validates `name` via `SkillName::parse`, which only accepts lowercase slug names (`[a-z0-9-]`) and, when loaded from a directory, requires the slug to match the directory name (`meerkat-core/src/skills/mod.rs:108-145`). As written, those examples would be quarantined as parse errors instead of loading successfully.

- Skills: the repository configuration examples are stale relative to the current config schema. `docs/guides/skills.mdx:23-48` uses `transport = "git"` / `transport = "http"` and omits `source_uuid`, but the actual schema is `type = "git" | "http" | "filesystem" | "stdio"` plus a required `source_uuid` on every repository entry (`meerkat-core/src/skills_config.rs:75-133`). The HTTP example also documents `auth_bearer` and `cache_ttl_seconds`, while the runtime expects `auth_token`, optional `auth_header`, `refresh_seconds`, and `timeout_seconds` (`meerkat-skills/src/resolve.rs:92-173`). This is likely to produce copy-paste config failures for library users.

- Skills: the discovery/resolution narrative no longer matches the current implementation. `docs/guides/skills.mdx:83-97` claims exact case-insensitive name matching, but the dedicated resolver is slash-prefix only and explicitly says “No name matching” (`meerkat-skills/src/resolver.rs:1-18`); slash activation in the agent path is also start-of-message `/skill-id` parsing only (`meerkat-core/src/agent/skills.rs:1-31`). Separately, the source-precedence tables in `docs/guides/skills.mdx:9-18` and `docs/examples/skills.mdx:453-463` omit configured filesystem and stdio sources even though both are supported (`meerkat-skills/src/resolve.rs:69-173`), and the built-in skill table in `docs/guides/skills.mdx:148-160` omits `mob-communication`, which is registered in `meerkat/src/lib.rs:253-264`.

- Memory: the SDK override example is using an outdated type. `docs/guides/memory.mdx:34-42` sets `build_config.override_memory = Some(true)`, but `override_memory` is now a `ToolCategoryOverride`, not `Option<bool>` (`meerkat/src/factory.rs:223-231`, `meerkat/src/factory.rs:429-433`). This is a concrete compile-break for Rust users following the guide.

- Memory: the examples understate the build-feature caveat for semantic memory. `docs/examples/memory.mdx:9-83` and the “enable built-in tools” section in `docs/examples/tools.mdx:46-152` present `--tools full` / `enable_memory: true` as sufficient, but memory wiring is still feature-gated at build time (`meerkat/src/factory.rs:2726-2759`) and the default CLI feature set does not include `memory-store` (`meerkat-cli/Cargo.toml:82-99`). The guide already mentions this prerequisite in `docs/guides/memory.mdx:14-33`; the examples should echo or link that caveat so users do not assume the toggle works in a default source build.

- Structured output: the guide currently describes validation as unconditional and `strict` as a generic validation toggle, but the implementation is narrower. `docs/guides/structured-output.mdx:20-28` says the response is validated against the schema using `jsonschema::Validator`, yet validation is feature-gated and silently falls back to “accept parsed JSON without schema validation” when `jsonschema` is disabled (`meerkat-core/src/agent/state.rs:1761-1788`). Also, `docs/guides/structured-output.mdx:47-53` describes `strict` as “Whether to enforce strict schema validation,” but the runtime treats it as provider-lowering/constrained-decoding behavior instead (`meerkat-core/src/types.rs:576-584`, `meerkat-openai/src/client.rs:864-876`). This is an important capability caveat for library users embedding Meerkat with custom feature sets.

- Tools/MCP: no standalone request-shape mismatches found in `docs/examples/tools.mdx` beyond the memory-feature caveat above. The MCP add/remove/reload examples align with the current surfaced parameters.

## Suggested follow-ups

- Update the hooks docs to remove the nonexistent CLI override flags or add a note that hook overrides are currently available only via RPC/REST/MCP/SDK surfaces. If CLI support is intended, it should be wired into `Run` args before the docs keep advertising it.

- Rewrite the skills frontmatter examples so `name` is a lowercase slug matching the directory name, and refresh the repo config snippets to use the current `type = ...` + `source_uuid = ...` format, including stdio/filesystem transport coverage.

- Refresh the skills guide/examples to document the actual current resolution model: slash-ID activation, canonical `SkillKey`/`skill_refs`, supported source transports, and the full built-in skill inventory including `mob-communication`.

- Fix the Rust memory override example to use `ToolCategoryOverride`, and add a short “requires a memory-enabled build” note anywhere examples suggest `enable_memory` or `--tools full` is enough by itself.

- Add a structured-output caveat that schema validation depends on the `jsonschema` feature, and clarify that `strict` is provider-lowering behavior rather than a universal extra validation pass.
