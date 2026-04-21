# Unified Documentation Remediation Checklist

This checklist consolidates the eight independent audit reports in this folder
into one working remediation plan.

Use this file as the primary cleanup tracker. The per-agent reports remain the
detailed evidence/source material.

## Source reports

- [README.md](/Users/luka/.codex/worktrees/fc6a/meerkat/audit/docs-review-2026-04-21/README.md)
- [agent-01-getting-started-and-nav.md](/Users/luka/.codex/worktrees/fc6a/meerkat/audit/docs-review-2026-04-21/agent-01-getting-started-and-nav.md)
- [agent-02-concepts-and-configuration.md](/Users/luka/.codex/worktrees/fc6a/meerkat/audit/docs-review-2026-04-21/agent-02-concepts-and-configuration.md)
- [agent-03-guides-auth-providers-models.md](/Users/luka/.codex/worktrees/fc6a/meerkat/audit/docs-review-2026-04-21/agent-03-guides-auth-providers-models.md)
- [agent-04-guides-tools-hooks-skills-memory.md](/Users/luka/.codex/worktrees/fc6a/meerkat/audit/docs-review-2026-04-21/agent-04-guides-tools-hooks-skills-memory.md)
- [agent-05-guides-mobs-comms-realtime-scheduling.md](/Users/luka/.codex/worktrees/fc6a/meerkat/audit/docs-review-2026-04-21/agent-05-guides-mobs-comms-realtime-scheduling.md)
- [agent-06-surfaces-cli-api-sdk-rust.md](/Users/luka/.codex/worktrees/fc6a/meerkat/audit/docs-review-2026-04-21/agent-06-surfaces-cli-api-sdk-rust.md)
- [agent-07-reference-architecture-and-contracts.md](/Users/luka/.codex/worktrees/fc6a/meerkat/audit/docs-review-2026-04-21/agent-07-reference-architecture-and-contracts.md)
- [agent-08-meerkat-skill-files.md](/Users/luka/.codex/worktrees/fc6a/meerkat/audit/docs-review-2026-04-21/agent-08-meerkat-skill-files.md)

## How To Use This Checklist

- Mark items done only after the docs change lands and the affected examples are re-checked against current code.
- Treat `P0` items as copy-paste-breakage or major contract drift.
- Treat `P1` items as important user-facing accuracy fixes.
- Treat `P2` items as discoverability, consistency, and maintenance hardening.

## P0: Broken Or Highly Misleading Examples And API Contracts

- [x] Fix the quickstart CLI install command so the main path uses the public install flow instead of repo-local `cargo install --path meerkat-cli`.
  Affected: `docs/quickstart.mdx`
  Source: agent 01

- [x] Update quickstart Python and TypeScript setup notes to say the SDKs auto-resolve/download `rkat-rpc` by default instead of requiring it on `PATH`.
  Affected: `docs/quickstart.mdx`
  Source: agent 01

- [x] Replace the stale custom dispatcher signature in the concepts/tools docs so it returns `ToolDispatchOutcome`, not `ToolResult`.
  Affected: `docs/concepts/tools.mdx`
  Source: agent 02

- [x] Rewrite the hook examples to match the current `HookEntryConfig` wire shape.
  Required fixes: use `foreground`/`background`, include `capability`, and nest runtime config under `runtime`.
  Affected: `docs/examples/hooks.mdx`
  Source: agent 04

- [x] Refresh the Rust custom-tools examples to match the current `AgentToolDispatcher` and `CreateSessionRequest` APIs.
  Affected: `docs/examples/tools.mdx`
  Source: agent 04

- [x] Refresh the structured-output guide and examples to use the current REST and Rust surfaces.
  Required fixes: `POST /sessions` instead of `/v1/run`, remove `StructuredOutputParams`, and update the Rust session creation snippets.
  Affected: `docs/guides/structured-output.mdx`, `docs/examples/structured-output.mdx`
  Source: agent 04

- [x] Fix memory examples to use the live budget field names and live `BudgetType` values.
  Required fixes: stop using `max_total_tokens`, `max_duration_ms`, and `budget_type = "max_tokens"` in places where the current schema expects `max_tokens`, `max_duration`, and `tokens`/`time`/`tool_calls`.
  Affected: `docs/examples/memory.mdx`
  Source: agent 04

- [x] Replace stale mob host-surface examples that still teach agent-tool payloads instead of the current `mob/*` host APIs and SDK methods.
  Affected: `docs/guides/mobs.mdx`, `docs/examples/mobs.mdx`
  Source: agent 05

- [x] Rebuild the scheduling guide against the current schedule schema and host APIs.
  Required fixes: stop using `schedule_create` via `tool/call`, old `cron` field objects, simplified interval triggers, and obsolete target/policy/lifecycle shapes.
  Affected: `docs/guides/scheduling.mdx`
  Source: agent 05

- [x] Fix the comms examples so typed comms and durable external events use the current distinct APIs.
  Required fixes: current `kind`-based `comms/send` shape, correct Python/TypeScript SDK signatures, and a clear split from `session/external_event`.
  Affected: `docs/examples/comms.mdx`
  Source: agent 05

- [x] Fix the WASM cross-mob example to use the actual `wire_cross_mob(mob_a, agent_a, mob_b, agent_b)` signature.
  Affected: `docs/examples/wasm.mdx`
  Source: agent 05

- [x] Expand the REST API docs from the live router instead of the current subset so they match the actual shipped surface.
  Required fixes: include the missing session helper, schedule, realtime, runtime, input, auth, realm, and extended mob endpoints.
  Affected: `docs/api/rest.mdx`
  Source: agent 06

- [x] Correct the Python SDK docs for runtime requirements and `send_external_event(...)`.
  Required fixes: document the `websockets` dependency and the required `event_type` argument.
  Affected: `docs/sdks/python/overview.mdx`, `docs/sdks/python/reference.mdx`
  Source: agent 06

- [x] Correct the TypeScript SDK docs for `sendExternalEvent(...)` and default realm behavior.
  Required fixes: include the `eventType` argument and stop claiming omitted realm options use a shared default realm when the current default is isolated mode.
  Affected: `docs/sdks/typescript/overview.mdx`, `docs/sdks/typescript/reference.mdx`
  Source: agent 06

- [x] Rework the Rust tools-and-stores page so all public API snippets compile against current code.
  Required fixes: `ToolRegistry` usage, `ToolOutput` import/source, `SessionStoreError`, persistent-store wiring, and MCP config constructors.
  Affected: `docs/rust/tools-and-stores.mdx`
  Source: agent 06

- [x] Replace stale scheduler tool names and override semantics in the built-in tools reference.
  Required fixes: current `meerkat_schedule_*` names, `occurrences`, scheduler category docs, and tri-state `ToolCategoryOverride`.
  Affected: `docs/reference/builtin-tools.mdx`
  Source: agents 02 and 07

## P1: User-Facing Accuracy And Behavioral Drift

- [x] Add the auth guide to docs navigation and link it directly from the introduction/quickstart flow.
  Affected: `docs/docs.json`, `docs/introduction.mdx`, `docs/quickstart.mdx`
  Source: agent 01

- [x] Rework the introduction/examples path so first-time users can actually discover the hello-world starters.
  Options: add a starter section to the gallery or change the CTA target.
  Affected: `docs/introduction.mdx`, `docs/examples/gallery.mdx`
  Source: agent 01

- [x] Clarify configuration docs so env vars are described as runtime credential lookup, not as a normal config-precedence layer that mutates `Config`.
  Affected: `docs/concepts/configuration.mdx`
  Source: agent 02

- [x] Fix the Anthropic provider table so it no longer presents `claude-opus-4-6` as both the top recommendation and the fallback.
  Affected: `docs/concepts/providers.mdx`
  Source: agent 02

- [x] Update OpenAI provider parameter guidance to current GPT-5-era `reasoning_effort` semantics and supported values.
  Affected: `docs/concepts/providers.mdx`
  Source: agent 02

- [x] Document realm ID syntax constraints instead of calling realm IDs opaque/free-form strings.
  Affected: `docs/concepts/realms.mdx`
  Source: agent 02

- [x] Clarify archive semantics in the sessions concept docs.
  Required fix: explain that archive removes the live/runtime handle and hides the session from normal listing, but committed history remains readable.
  Affected: `docs/concepts/sessions.mdx`
  Source: agent 02

- [x] Update concepts/tools to include current schedule-tool availability and correct the description of live MCP mutation vs external filter control.
  Affected: `docs/concepts/tools.mdx`
  Source: agent 02

- [x] Update concepts/tools multimodal output guidance to the current content model.
  Required fixes: include `Video`, remove obsolete `source_path` expectations, and describe the current `ToolResult.content` model.
  Affected: `docs/concepts/tools.mdx`
  Source: agent 02

- [x] Correct the auth guide so it does not imply `rkat auth login` is currently realm-scoped.
  Required fix: distinguish `env_default`, current CLI `dev:*` behavior, and realm/profile-targeted REST/RPC flows.
  Affected: `docs/guides/auth.mdx`
  Source: agent 03

- [x] Fix stale backend/auth identifiers in the auth matrix.
  Required fixes: `bedrock_aws_sigv4`, `google_code_assist`, and related exact names from the provider matrix.
  Affected: `docs/guides/auth.mdx`
  Source: agent 03

- [x] Expand or narrow the credential-source section so it matches the current surface.
  Required fixes: account for `PlatformDefault` and `FileDescriptor`, and stop overstating `ManagedStore.profile` if runtime lookup is binding-driven.
  Affected: `docs/guides/auth.mdx`
  Source: agent 03

- [x] Add a clear caveat that self-hosted Gemma audio capability is not currently surfaced as a Meerkat self-hosted capability/realtime path.
  Affected: `docs/guides/self-hosted-gemma4.mdx`
  Source: agent 03

- [x] Add the current `HookToolResult.has_images` caveat and explain that tool-result hook patches preserve image blocks rather than replacing the entire multimodal result.
  Affected: `docs/guides/hooks.mdx`
  Source: agent 04

- [x] Rewrite skills docs around the current activation model.
  Required fixes: CLI `--skill` as preload-only, structured `SkillKey`/`skill_refs` on APIs, and legacy string refs clearly labeled as deprecated compatibility input.
  Affected: `docs/guides/skills.mdx`, `docs/examples/skills.mdx`
  Source: agent 04

- [x] Separate semantic memory enablement from compaction enablement in the memory guide.
  Affected: `docs/guides/memory.mdx`
  Source: agent 04

- [x] Fix stale memory CLI guidance.
  Required fixes: `cargo build -p rkat` or equivalent current guidance, and stop documenting a non-existent `rkat run --memory` flag.
  Affected: `docs/guides/memory.mdx`
  Source: agent 04

- [x] Rewrite mob docs so agent-side `mob_*` tools and host-side `mob/*` / `meerkat_mob_*` surfaces are clearly separated.
  Affected: `docs/guides/mobs.mdx`, `docs/examples/mobs.mdx`
  Source: agent 05

- [x] Add a realtime caveat that `binding_ready` does not by itself guarantee `realtime/open_info` works; the host must expose the realtime websocket service.
  Affected: `docs/guides/realtime.mdx`
  Source: agent 05

- [x] Refresh realtime examples to prefer the current canonical realtime model while documenting any legacy aliasing explicitly.
  Affected: `docs/guides/realtime.mdx`
  Source: agent 05

- [x] Expand the CLI command docs to include the currently shipped top-level commands.
  Required fixes: include `auth`, `blob`, `realtime`, and anything else missing from the current binary.
  Affected: `docs/cli/commands.mdx`
  Source: agent 06

- [x] Correct stale CLI exit-code documentation.
  Affected: `docs/cli/configuration.mdx`
  Source: agent 06

- [x] Fix the REST docs’ stated realm config path so it points to the shared realm root rather than a REST-specific directory.
  Affected: `docs/api/rest.mdx`
  Source: agent 06

- [x] Expand the TypeScript docs to cover the currently exported realtime and auth/realm surfaces.
  Affected: `docs/sdks/typescript/overview.mdx`, `docs/sdks/typescript/reference.mdx`
  Source: agent 06

- [x] Split session-layer transport error mapping from canonical wire-envelope error mapping in the reference docs.
  Affected: `docs/reference/capability-matrix.mdx`, `docs/reference/api-reference.mdx`
  Source: agent 07

- [x] Update the skills governance runbook for current source statuses and lineage events.
  Required fixes: `disabled` status and `rename_or_relocate`.
  Affected: `docs/reference/skills-governance-runbook.mdx`
  Source: agent 07

- [x] Update `meerkat-architecture` skill docs for the current five-machine catalog, current `meerkat-models` ownership, and the repo-wide `realtime` vocabulary.
  Affected: `.claude/skills/meerkat-architecture/SKILL.md`, `.claude/skills/meerkat-architecture/references/machine-system.md`
  Source: agent 08

- [x] Reframe `meerkat-platform` skill guidance so `0.5` is treated as legacy migration context, not as the current platform contract.
  Affected: `.claude/skills/meerkat-platform/SKILL.md`
  Source: agent 08

- [x] Narrow both platform/WASM skill files to the actual wasm32 built-in tool surface.
  Affected: `.claude/skills/meerkat-platform/SKILL.md`, `.claude/skills/meerkat-wasm/SKILL.md`
  Source: agent 08

## P2: Discoverability, Cleanup, And Maintenance Hardening

- [x] Align the quickstart Rust snippet with one maintained in-repo starter example or explicitly point readers to the maintained example as the source of truth.
  Affected: `docs/quickstart.mdx`, `meerkat/examples/simple.rs`
  Source: agent 01

- [x] Sweep adjacent docs for copied stale tool examples/status text after fixing the concepts/tools page.
  High-value follow-up targets: `docs/examples/tools.mdx`, `docs/reference/builtin-tools.mdx`
  Source: agent 02

- [x] Remove the duplicated “Spawn members” block in the mobs examples page.
  Affected: `docs/examples/mobs.mdx`
  Source: agent 05

- [x] Add or improve schedule-tool reference coverage so the docs do not mention the category only in passing.
  Affected: `docs/reference/builtin-tools.mdx` and any schedule docs that should link back to it
  Source: agents 02 and 07

- [x] Update the reference architecture page so its execution-path diagram matches the settled runtime-binding contract.
  Affected: `docs/reference/architecture.mdx`
  Source: agent 07

- [x] Expand or relabel the architecture crate ownership table so it reflects the current workspace split instead of reading like a complete map while omitting important crates.
  Affected: `docs/reference/architecture.mdx`
  Source: agent 07

- [x] Decide how to position historical/proposal docs so they are not mistaken for shipped product behavior.
  Priority targets: `docs/design/mobpack.md`, `docs/reference/comms-redesign-v6-hard-cut.md`
  Source: agent 07

- [x] Add stronger implementation-status notes to `docs/design/mobpack.md` wherever it documents commands or flags the CLI does not currently ship.
  Affected: `docs/design/mobpack.md`
  Source: agent 07

- [x] Reconcile public docs and skill docs anywhere the same architectural drift appears in both.
  Current overlap hot spots: machine count, `meerkat-models` ownership, realtime/voice vocabulary, wasm tool availability.
  Affected: `docs/reference/architecture.mdx`, `docs/guides/realtime.mdx`, `.claude/skills/meerkat-*`
  Source: agents 07 and 08

## Recommended Execution Order

- [x] Batch 1: Fix quickstart, auth discoverability, and obviously broken example snippets.
- [x] Batch 2: Fix current host/API surface docs for tools, schedules, mobs, comms, REST, Python, TypeScript, and Rust examples.
- [x] Batch 3: Fix concept/reference drift and error-code/ownership tables.
- [x] Batch 4: Fix `.claude/skills/meerkat-*` parity and historical/proposal doc positioning.
- [x] Batch 5: Do a final cross-doc consistency pass for vocabulary, tool names, and surface names.

## Completion Criteria

- [x] Every changed example has been compared against current code or a current test/example.
- [x] Quickstart paths work for CLI, Rust, Python, and TypeScript without hidden prerequisites.
- [x] Host-surface docs consistently distinguish CLI/REST/RPC/MCP/SDK calls from in-agent tool calls.
- [x] Architecture/reference pages no longer contradict current runtime/machine ownership.
- [x] `.claude/skills/meerkat-*` no longer teach stale platform assumptions.
