# Agent 08 Findings

## Scope reviewed

- `.claude/skills/meerkat-platform/SKILL.md`
- `.claude/skills/meerkat-architecture/SKILL.md`
- `.claude/skills/meerkat-wasm/SKILL.md`
- Parity checks against current repo layout, current CLI/runtime code, and current docs under `docs/`

## Files reviewed

- `.claude/skills/meerkat-platform/SKILL.md`
- `.claude/skills/meerkat-architecture/SKILL.md`
- `.claude/skills/meerkat-wasm/SKILL.md`
- `docs/cli/commands.mdx`
- `docs/guides/mobs.mdx`
- `docs/api/rest.mdx`
- `docs/reference/architecture.mdx`
- `meerkat-cli/src/main.rs`
- `meerkat-rest/src/lib.rs`
- `meerkat-machine-schema/src/catalog/mod.rs`
- `meerkat-client/src/lib.rs`
- `meerkat-providers/src/lib.rs`
- `meerkat-openai/src/lib.rs`
- `meerkat-mob/src/runtime/loop_iteration_authority.rs`
- `meerkat-mcp/src/external_tool_surface_authority.rs`
- `sdks/web/package.json`
- `meerkat-web-runtime/src/lib.rs`

## Findings

1. `.claude/skills/meerkat-platform/SKILL.md` still documents removed direct CLI mob lifecycle commands. The block at lines 185-199 advertises `rkat mob create`, `list`, `status`, `wire`, `unwire`, and `force-cancel` as the current operational CLI surface, but the actual `MobCommands` enum only exposes artifact commands plus helper/flow commands such as `run-flow`, `flow-status`, `spawn-helper`, `fork-helper`, `member-status`, `respawn`, `wait-kickoff`, and `web build` in `meerkat-cli/src/main.rs:1772-1904`. Current docs also describe the CLI as helper-oriented rather than lifecycle-oriented in `docs/guides/mobs.mdx:200-204`, while `docs/cli/commands.mdx:55` now summarizes `rkat mob ...` as an artifact surface. This is a real surface mismatch, not just wording drift.

2. `.claude/skills/meerkat-platform/SKILL.md` has stale realtime surface guidance for REST and Rust. The realtime table at lines 124-131 says REST opens the channel via RPC and says Rust uses provider integration in `meerkat-client`, but the current REST API exposes `POST /realtime/open_info` directly in both docs and code (`docs/api/rest.mdx:81-85`, `meerkat-rest/src/lib.rs:1123-1130`). On the Rust side, `meerkat-client` is now a compatibility shim, while the OpenAI realtime attachment implementation lives under `meerkat-openai` (`meerkat-client/src/lib.rs:1-12,20-57`, `meerkat-openai/src/lib.rs:1-28`). Agents following the skill literally will reach for the wrong crate and understate the REST surface.

3. `.claude/skills/meerkat-platform/SKILL.md` and `.claude/skills/meerkat-architecture/SKILL.md` both still teach pre-split crate ownership. Platform lines 519-522 still say provider-runtime, OAuth, auth-store, and authorizers live in `meerkat-providers` and are re-exported by `meerkat-client`, while the architecture table at lines 98-119 says `meerkat-client` owns the LLM providers/realtime transport. Current crate headers say the opposite: `meerkat-client` is a thin compatibility shim (`meerkat-client/src/lib.rs:1-12`), `meerkat-providers` is also a compatibility shim for generic provider-runtime/auth primitives and explicitly does not re-export per-provider verticals (`meerkat-providers/src/lib.rs:1-13`), and provider implementations now live in provider-specific crates such as `meerkat-openai` and `meerkat-anthropic` (`meerkat-openai/src/lib.rs:1-28`). `docs/reference/architecture.mdx:76-98` already reflects the new split, so the skill files are behind current docs and current code.

4. `.claude/skills/meerkat-architecture/SKILL.md` contains an internal contradiction about canonical machines. It correctly says “Exactly five canonical machines” at lines 64-72, but the quick index later says `meerkat-machine-schema/src/catalog/dsl/` is the truth for “all 4 machines” at line 142. The code confirms there are five canonical machine schemas in `meerkat-machine-schema/src/catalog/mod.rs:18-25`, and `docs/reference/architecture.mdx:45-60` also documents five. This is small, but it is exactly the kind of inconsistency that makes an architecture navigator less trustworthy.

5. `.claude/skills/meerkat-architecture/SKILL.md` overstates the “no handwritten authority” story. Lines 76-78 say no handwritten `*_authority.rs` state machines remain outside the catalog and imply remaining `_authority.rs` files are only projections/helpers, but the repo still contains handwritten authority-style mutators with `apply(...)` methods, including `LoopIterationAuthority` in `meerkat-mob/src/runtime/loop_iteration_authority.rs:1-80` and `ExternalToolSurfaceAuthority` in `meerkat-mcp/src/external_tool_surface_authority.rs:1-16,416-438,1800-1815`. Even if these are transitional or standalone-only paths, the current skill wording is too absolute.

6. No stale paths or obvious surface drift were found in `.claude/skills/meerkat-wasm/SKILL.md` during this pass. The referenced paths exist (`sdks/web/`, `sdks/web/proxy/index.mjs`, `meerkat-web-runtime/src/lib.rs`, `tests/integration/src/e2e_lanes.rs`), the build guidance matches `docs/examples/wasm.mdx:23-24` and `sdks/web/scripts/build-wasm.mjs`, and the package/runtime surface described there still matches the current `@rkat/web` package and wasm exports (`sdks/web/package.json`, `meerkat-web-runtime/src/lib.rs:931-936`).

## Suggested follow-ups

- Refresh `.claude/skills/meerkat-platform/SKILL.md` so its CLI mob section matches the current `MobCommands` surface, and update the realtime table to point REST users at `/realtime/open_info` and Rust users at the current provider-vertical crates instead of `meerkat-client`.
- Refresh `.claude/skills/meerkat-architecture/SKILL.md` against `docs/reference/architecture.mdx` and the 2026-04-18 crate split, especially the crate-ownership table and the “zero handwritten authority” claim.
- Decide which mob CLI doc is canonical, because `docs/guides/mobs.mdx` and `docs/cli/commands.mdx` currently frame the `rkat mob` surface differently; once that is settled, align the skills and the docs together.
- Consider adding a lightweight docs audit check that flags removed CLI subcommands and crate-role mismatches by comparing skill snippets against `meerkat-cli/src/main.rs` and `docs/reference/architecture.mdx`.
