# Agent 08 Findings

## Scope reviewed

- `.claude/skills/meerkat-platform/SKILL.md`
- `.claude/skills/meerkat-architecture/SKILL.md`
- `.claude/skills/meerkat-wasm/SKILL.md`
- Parity against current repo layout, selected runtime/tooling code paths, and current `docs/` pages that cover architecture, capability matrix, CLI, realtime, and WASM behavior

## Files reviewed

- `.claude/skills/meerkat-platform/SKILL.md`
- `.claude/skills/meerkat-platform/references/api_reference.md`
- `.claude/skills/meerkat-platform/references/migration_0_5.md`
- `.claude/skills/meerkat-architecture/SKILL.md`
- `.claude/skills/meerkat-architecture/references/machine-system.md`
- `.claude/skills/meerkat-wasm/SKILL.md`
- `.claude/skills/meerkat-wasm/references/api_surface.md`
- `docs/reference/architecture.mdx`
- `docs/reference/capability-matrix.mdx`
- `docs/guides/realtime.mdx`
- `docs/cli/commands.mdx`
- `docs/examples/mobs.mdx`
- `docs/examples/wasm.mdx`
- `Cargo.toml`
- `meerkat-machine-schema/src/catalog/dsl/mod.rs`
- `meerkat-models/src/lib.rs`
- `meerkat-tools/src/builtin/composite.rs`
- `meerkat-web-runtime/src/lib.rs`
- `meerkat-cli/src/main.rs`

## Findings

### 1. `meerkat-architecture/SKILL.md` still documents a four-machine world, but the current catalog and public architecture docs now expose five canonical machines

- The skill says "Exactly four canonical machines" and claims `meerkat-machine-schema/src/catalog/dsl/` contains exactly those four DSLs: `.claude/skills/meerkat-architecture/SKILL.md:64-77`.
- The same stale framing is repeated in `.claude/skills/meerkat-architecture/references/machine-system.md:5-18` and `:153-167`.
- Current repo state disagrees:
  - `meerkat-machine-schema/src/catalog/dsl/mod.rs:29-54` exports `auth_machine` alongside `meerkat_machine`, `mob_machine`, `schedule_lifecycle`, and `occurrence_lifecycle`.
  - `docs/reference/architecture.mdx:44-59` explicitly says the current catalog exposes **five** canonical machines, including `AuthMachine`.
- Impact: the architecture skill will steer readers toward an outdated machine model and can cause bad assumptions when touching auth or schema/catalog parity work.

### 2. `meerkat-architecture/SKILL.md` has stale crate ownership for `meerkat-models`

- The skill says `meerkat-models` owns "Model catalog, provider profiles, parameter schemas": `.claude/skills/meerkat-architecture/SKILL.md:95-118`, especially line 97.
- Current repo state shows `meerkat-models` is now a shim:
  - `meerkat-models/src/lib.rs:1-19` says all model catalog/profile data moved to `meerkat_core::model_profile` on 2026-04-18 and that the crate is retained as a thin re-export layer.
  - `docs/reference/architecture.mdx:75-89` matches that newer ownership model and describes `meerkat-models` as a compatibility shim.
- Impact: the skill points maintainers at the wrong ownership boundary for model-catalog work.

### 3. `meerkat-architecture/SKILL.md` is internally inconsistent on `voice intent` after the repo’s vocabulary convergence to `realtime`

- The skill correctly says public surfaces describe `realtime`, not `voice`: `.claude/skills/meerkat-architecture/SKILL.md:42-62`.
- It also says per-member voice intent was removed: `.claude/skills/meerkat-architecture/SKILL.md:69`.
- But later it still uses `voice intent` as the example of a fact that survives respawn and should be keyed by `AgentIdentity`: `.claude/skills/meerkat-architecture/SKILL.md:88-91`.
- Current guide text uses the converged `realtime` vocabulary instead: `docs/guides/realtime.mdx:12-18`.
- Impact: this is a confusing, self-contradictory instruction inside the same skill file.

### 4. `meerkat-platform/SKILL.md` still frames current platform guidance around `0.5`, even though the workspace and docs are on `0.6.0`

- The skill intro says "0.5 semantics are runtime-backed by default": `.claude/skills/meerkat-platform/SKILL.md:8`.
- It immediately directs users to `references/migration_0_5.md`: `.claude/skills/meerkat-platform/SKILL.md:10-21`.
- It later describes WASM additions as "Key additions in 0.5": `.claude/skills/meerkat-platform/SKILL.md:276`.
- Its dependency examples still pin `meerkat = "0.5"` and `version = "0.5"`: `.claude/skills/meerkat-platform/SKILL.md:527-542`.
- Current repo/docs disagree:
  - `Cargo.toml:48` sets the workspace version to `0.6.0`.
  - `docs/reference/capability-matrix.mdx:57-72` uses `0.6.0` dependency examples.
  - `sdks/web/package.json:3` is also `0.6.0`.
- Impact: readers using the skill as current guidance are likely to copy stale dependency snippets and infer that 0.5 is still the current contract rather than legacy migration context.

### 5. Both the platform and WASM skills overstate which built-in tools are available on wasm32

- `meerkat-platform/SKILL.md` says wasm32 tool dispatch includes utility builtins like `wait`, `datetime`, and `apply_patch`: `.claude/skills/meerkat-platform/SKILL.md:258-266`, especially line 264.
- `meerkat-wasm/SKILL.md` says wasm32 includes non-shell utility/task/comms/skill surfaces, "including `view_image` for vision-capable models": `.claude/skills/meerkat-wasm/SKILL.md:124-128`.
- Current code disagrees:
  - `meerkat-tools/src/builtin/composite.rs:195-235` shows `CompositeDispatcher::new_wasm()` only registers task tools plus `DateTimeTool`.
  - There is no wasm registration of `apply_patch` or `view_image` in that constructor.
  - `meerkat-web-runtime/src/lib.rs:441-449` wires the wasm runtime through that dispatcher.
- Impact: both skills currently advertise wasm capabilities that the runtime does not actually ship.

### 6. No stale path/layout issues found in the three skill directories themselves

- All three scoped skill files exist.
- Their local `references/` files also exist:
  - `.claude/skills/meerkat-platform/references/*`
  - `.claude/skills/meerkat-architecture/references/*`
  - `.claude/skills/meerkat-wasm/references/*`
- The top-level repo paths they mention most often also exist (`sdks/web/`, `meerkat-web-runtime/`, `meerkat-machine-schema/`, `meerkat-runtime/`, `meerkat-mob-mcp/`, `tests/integration/`, `scripts/`).
- So the main problems are guidance drift and architectural drift, not broken relative paths.

## Suggested follow-ups

- Update `.claude/skills/meerkat-architecture/SKILL.md` and `.claude/skills/meerkat-architecture/references/machine-system.md` to reflect the current five-machine catalog and the current `AuthMachine` role.
- Update the crate ownership table in `.claude/skills/meerkat-architecture/SKILL.md` so `meerkat-models` is described as a compatibility shim, not the primary owner of catalog/profile truth.
- Remove or rename the lingering `voice intent` wording in `.claude/skills/meerkat-architecture/SKILL.md` so the file consistently uses the post-convergence `realtime` model.
- Reframe `.claude/skills/meerkat-platform/SKILL.md` so `0.5` appears only as legacy migration context, and refresh example dependency snippets to `0.6.0`.
- Narrow the wasm capability claims in `.claude/skills/meerkat-platform/SKILL.md` and `.claude/skills/meerkat-wasm/SKILL.md` to match `CompositeDispatcher::new_wasm()`.
- Outside the scoped skill files, reconcile related public docs that are already drifting in the same area:
  - `docs/cli/commands.mdx` still shows a different `spawn-helper` syntax than the current CLI implementation and `docs/examples/mobs.mdx`.
  - `docs/examples/wasm.mdx` still documents an older low-level API/capability picture that does not match the current `@rkat/web` wrapper focus or current runtime behavior.
