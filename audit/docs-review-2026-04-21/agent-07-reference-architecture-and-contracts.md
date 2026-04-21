# Agent 07 Findings

## Scope reviewed

- Reference docs for architecture, API/contracts, built-in tools, capability/error behavior, skills governance, and historical comms design context.
- Architecture/design docs under `docs/architecture/*` and `docs/design/mobpack.md` for drift against the current crate graph, runtime/machine seams, and CLI/runtime contracts.
- Current code for the machine catalog, session/runtime seams, tool registration, mobpack CLI support, skill identity governance, and transport error mapping.

## Files reviewed

Docs in scope:

- `docs/reference/api-reference.mdx`
- `docs/reference/architecture.mdx`
- `docs/reference/builtin-tools.mdx`
- `docs/reference/capability-matrix.mdx`
- `docs/reference/comms-redesign-v6-hard-cut.md`
- `docs/reference/design-philosophy.mdx`
- `docs/reference/session-contracts.mdx`
- `docs/reference/skills-governance-runbook.mdx`
- `docs/architecture/RMAT.md`
- `docs/architecture/deferred-tool-catalog-proposal.md`
- `docs/architecture/finite-ownership-ledger.md`
- `docs/architecture/formal-seam-closure.md`
- `docs/architecture/identity-first-live-voice-proposal.md`
- `docs/architecture/identity-generation-and-runtime-epoch.md`
- `docs/architecture/machine-simplification-proposal.md`
- `docs/architecture/meerkat-runtime-dogma.md`
- `docs/architecture/meerkat-runtime-schema-parity-ledger.md`
- `docs/architecture/mob-runtime-schema-parity-ledger.md`
- `docs/architecture/realtime-259-port-plan.md`
- `docs/architecture/realtime-transcript-fidelity.md`
- `docs/architecture/rebuild-trigger-audit.md`
- `docs/architecture/surface-composability-and-cli-layering-proposal.md`
- `docs/design/mobpack.md`

Primary code checked for verification:

- `Cargo.toml`
- `meerkat-machine-schema/src/catalog/mod.rs`
- `meerkat-cli/src/main.rs`
- `meerkat-core/src/service/mod.rs`
- `meerkat-core/src/service/transport.rs`
- `meerkat-core/src/session.rs`
- `meerkat-core/src/skills/mod.rs`
- `meerkat-core/src/agent/state.rs`
- `meerkat-contracts/src/error/mod.rs`
- `meerkat-contracts/src/version.rs`
- `meerkat-session/Cargo.toml`
- `meerkat/src/factory.rs`
- `meerkat-schedule/src/tools.rs`
- `meerkat-mob-pack/src/validate.rs`
- `meerkat-models/src/lib.rs`
- `meerkat-auth-core/src/lib.rs`
- `meerkat-client/src/lib.rs`
- `meerkat-providers/src/lib.rs`
- `meerkat-mcp-server/src/lib.rs`
- `meerkat-web-runtime/src/lib.rs`

## Findings

1. `docs/reference/architecture.mdx` currently conflicts with the settled runtime-backed construction path and understates the live crate ownership split.

- The architecture diagram routes `SURFACES -> SessionService -> MeerkatMachine -> AgentFactory` at `docs/reference/architecture.mdx:17-20`, but the canonical runtime-backed seam elsewhere in the reference set is `MeerkatMachine::prepare_bindings(...)` first, then `SessionBuildOptions.runtime_build_mode = SessionOwned(...)`, then `SessionService::create_session(...)` (`docs/reference/session-contracts.mdx:17-27`, `docs/reference/api-reference.mdx:58-63`, `docs/reference/design-philosophy.mdx:35-39`). The code follows the latter ordering; for example the CLI prepares bindings before building the create request in `meerkat-cli/src/main.rs:5277-5329`.
- The crate ownership table at `docs/reference/architecture.mdx:75-89` omits several now-important crates from the live workspace graph in `Cargo.toml:3-44`, including the April 18 split shims and new ownership boundaries: `meerkat-auth-core` (`meerkat-auth-core/src/lib.rs:1-12`), `meerkat-client` (`meerkat-client/src/lib.rs:1-12`), `meerkat-providers` (`meerkat-providers/src/lib.rs:1-12`), `meerkat-mob-pack` (`meerkat-mob-pack/src/lib.rs:1-7`), `meerkat-mcp-server` (`meerkat-mcp-server/src/lib.rs:1-6`), `meerkat-web-runtime` (`meerkat-web-runtime/src/lib.rs:1-20`), `meerkat-store` (`meerkat-store/src/lib.rs:1-19`), and `meerkat-memory` (`meerkat-memory/src/lib.rs:1-14`). That makes the page a partial map rather than the “current architecture” source of truth it claims to be.

2. `docs/reference/builtin-tools.mdx` is stale for the scheduler tool surface and for per-build tool override semantics.

- The overview table documents scheduler tools as `schedule_create`, `schedule_list`, `schedule_read`, `schedule_update`, `schedule_pause`, `schedule_resume`, and `schedule_delete` at `docs/reference/builtin-tools.mdx:27-33`. The actual exported tool names are `meerkat_schedule_create`, `meerkat_schedule_get`, `meerkat_schedule_list`, `meerkat_schedule_update`, `meerkat_schedule_pause`, `meerkat_schedule_resume`, `meerkat_schedule_delete`, and `meerkat_schedule_occurrences` in `meerkat-schedule/src/tools.rs:60-181`. `schedule_read` is especially misleading because the real API split is `get` plus `occurrences`.
- The factory flag table omits the scheduler category entirely at `docs/reference/builtin-tools.mdx:40-46`, even though `AgentFactory` has `enable_schedule` and `.schedule(bool)` in `meerkat/src/factory.rs:797-803` and `meerkat/src/factory.rs:1045-1048`.
- The per-build override section says the override fields are `Option<bool>` and lists only builtins/shell/memory/mob at `docs/reference/builtin-tools.mdx:53-64`. The current build seam uses tri-state `ToolCategoryOverride` values (`inherit` / `enable` / `disable`) in `meerkat-core/src/session.rs:956-1025`, and the factory/build config now includes `override_schedule` and `schedule_tools` as well as the other categories in `meerkat-core/src/service/mod.rs:199-209` and `meerkat/src/factory.rs:223-242`.
- There is also an internal-reference gap: the page advertises schedule tools in the overview but never gives them a dedicated reference section, while it does so for task/shell/memory/utility/comms. That makes the only schedule details live in source rather than in the docs page that claims to be comprehensive.

3. The reference error-code tables in `docs/reference/capability-matrix.mdx` and `docs/reference/api-reference.mdx` no longer match the current `SessionError` transport projections.

- `docs/reference/capability-matrix.mdx:37-45` maps `PersistenceDisabled` and `CompactionDisabled` to `CAPABILITY_UNAVAILABLE`, JSON-RPC `-32020`, and CLI `exit 40`. The current session-layer codes are `SESSION_PERSISTENCE_DISABLED` and `SESSION_COMPACTION_DISABLED` in `meerkat-core/src/service/mod.rs:88-100`; their session transport projections are JSON-RPC `-32003` and `-32004`, HTTP `501`, and CLI exit code `0` in `meerkat-core/src/service/transport.rs:8-42`.
- `docs/reference/api-reference.mdx:281-296` similarly presents a single “Capability unavailable” row as the reference contract, but the session layer now distinguishes capability-disabled session operations from the higher-level wire `ErrorCode::CapabilityUnavailable`. The wire conversion still collapses some `SessionError`s to capability-unavailable at the contracts boundary in `meerkat-contracts/src/error/mod.rs:202-216`, so the docs need to clearly separate “session-layer transport mapping” from “canonical wire envelope mapping” instead of mixing them.
- The prose at `docs/reference/capability-matrix.mdx:49-53` is also stale for CLI behavior, because the actual session transport helper returns `0` for persistence/compaction-disabled cases, not a non-zero hard failure (`meerkat-core/src/service/transport.rs:34-42`).

4. `docs/design/mobpack.md` is materially ahead of the shipped CLI and mixes implemented behavior with commands that do not exist.

- The page still presents `rkat mob embed`, `rkat mob compile`, `rkat mob publish`, `rkat mob web serve`, and `rkat mob web publish` as available commands (`docs/design/mobpack.md:19-23`, `docs/design/mobpack.md:214-218`, `docs/design/mobpack.md:249-310`, `docs/design/mobpack.md:354-362`). The actual mob CLI only exposes `pack`, `inspect`, `validate`, `deploy`, and `web build` in `meerkat-cli/src/main.rs:1773-1918`.
- `docs/design/mobpack.md:244-246` documents `--budget-max-tokens`, but the current deploy command uses `--max-total-tokens`, `--max-duration`, and `--max-tool-calls` (`meerkat-cli/src/main.rs:1786-1805`).
- `docs/design/mobpack.md:354-355` documents `rkat mob validate ... --target web`, but the current validator only accepts a pack path and does not parse any target selector (`meerkat-cli/src/main.rs:1781-1784`, `meerkat-cli/src/main.rs:8387-8394`, `meerkat-mob-pack/src/validate.rs:1-37`).
- The page is clearly labeled a proposal at the top (`docs/design/mobpack.md:3`), so this is not a correctness failure in isolation; the problem is that it lives in a design/reference path without a stronger “not implemented” fence or a link back to the real CLI surface. Right now it is easy to read it as current product documentation.

5. `docs/reference/skills-governance-runbook.mdx` is behind the current skill identity model.

- The “Required Source Record Fields” section only allows `status` values `active` or `retired` at `docs/reference/skills-governance-runbook.mdx:17-23`, but the actual enum also includes `Disabled` in `meerkat-core/src/skills/mod.rs:196-215`.
- The migration procedures only cover `rotate`, `split`, and `merge` at `docs/reference/skills-governance-runbook.mdx:35-51`, but the live lineage enum also supports `RenameOrRelocate` in `meerkat-core/src/skills/mod.rs:217-249`, and the identity logic handles it explicitly in `meerkat-core/src/skills/identity.rs:158-241`.
- The runbook otherwise aligns with the canonical `SkillKey { source_uuid, skill_name }` model and with the existence of remap validation errors such as `MissingSkillRemaps` / `RemapWithoutLineage` / `RemapCycle` (`meerkat-core/src/skills/mod.rs:251-259`, `meerkat-core/src/skills/mod.rs:627-649`), so this looks like partial doc drift rather than a wholesale rewrite need.

6. No material architectural drift was found in the doctrine/ADR set under `docs/architecture/*`, but a couple of historical pages would benefit from clearer linkage to current references.

- The canonical machine/composition counts in `docs/reference/architecture.mdx:46-73` still match `meerkat-machine-schema/src/catalog/mod.rs:18-35`.
- The machine/spec directories referenced by the reference pages exist under `specs/machines/*` and `specs/compositions/*`.
- `docs/reference/comms-redesign-v6-hard-cut.md` already has a historical warning banner at `docs/reference/comms-redesign-v6-hard-cut.md:3-9`, which is good. I did not find a code-contract mismatch that requires changing that page for correctness, but it still reads like a reference page while describing removed APIs in the body; that is a documentation-positioning issue more than a factual bug.

## Suggested follow-ups

- Update `docs/reference/architecture.mdx` so the execution-path diagram matches the settled runtime-binding contract, and either expand the crate ownership table to the current workspace split or relabel it as a curated subset.
- Refresh `docs/reference/builtin-tools.mdx` to use the real scheduler tool names, add the missing scheduler category/override docs, and document the tri-state `ToolCategoryOverride` model instead of `Option<bool>`.
- Split “session-layer transport mapping” from “wire `ErrorCode` mapping” in `docs/reference/capability-matrix.mdx` and `docs/reference/api-reference.mdx`, so readers can see why `SessionError` codes and `WireError` codes differ.
- Either move `docs/design/mobpack.md` out of the “current docs” path, or add a strong implementation-status note near every non-shipped command family and link readers to the live CLI command docs.
- Extend `docs/reference/skills-governance-runbook.mdx` to cover `disabled` source records and `rename_or_relocate` lineage events.
