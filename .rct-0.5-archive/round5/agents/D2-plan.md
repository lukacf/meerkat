# Lane D2 Plan

## Owned Rows

- `DOGMA-16`
- `DOGMA-24`
- `DOGMA-25`

## Owned Milestones

- `D2-1` = `DOGMA-25`
- `D2-2` = `DOGMA-16`, `DOGMA-24`

## Owned Files

- `meerkat-contracts/src/capability/registry.rs`
- `meerkat-skills/src/engine.rs`
- `meerkat-auth-core/src/resolver.rs`
- provider runtimes:
  - `meerkat-openai/src/runtime/mod.rs`
  - `meerkat-anthropic/src/runtime/mod.rs`
  - `meerkat-gemini/src/runtime/mod.rs`
- capability-aware factory ranges in `meerkat/src/factory.rs`

## Shared Files Touched Under Round Map

- `meerkat/src/factory.rs` as second owner, after `D1`
- provider runtime `mod.rs` files as second owner, after `D1`

## Blocked By

- `D2-2` blocked by `D1-1`

## Unblocks

- none hard-blocked, but closes auth/runtime policy correctness

## Public / Type / Schema Changes

- no public rename surface
- effective capability truth becomes the only skill filtering seed
- external-authorizer flows materialize a real `DynamicLease`
- policy fields alter resolved runtime behavior

## Defensive Scan Commitments

- none mandatory beyond tests for `DOGMA-25`, but lane must preserve `D1`’s identity scan cleanliness

## Requested Orchestrator Probes

- `./scripts/repo-cargo check -p meerkat`
- `./scripts/repo-cargo check -p meerkat-skills`
- `./scripts/repo-cargo check -p meerkat-contracts`
- `./scripts/repo-cargo check -p meerkat-openai`
- `./scripts/repo-cargo check -p meerkat-anthropic`
- `./scripts/repo-cargo check -p meerkat-gemini`

## Requested Build Gate Commands

- Quick probe `D2-1`:
  - `./scripts/repo-cargo check -p meerkat`
  - `./scripts/repo-cargo check -p meerkat-skills`
  - `./scripts/repo-cargo check -p meerkat-contracts`
- Quick probe `D2-2`:
  - `./scripts/repo-cargo check -p meerkat-openai`
  - `./scripts/repo-cargo check -p meerkat-anthropic`
  - `./scripts/repo-cargo check -p meerkat-gemini`
  - `./scripts/repo-cargo check -p meerkat`
  - `./scripts/repo-cargo check -p meerkat-skills`
- Full gate `D2-1`:
  - `./scripts/repo-cargo nextest run -p meerkat-skills --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- Full gate `D2-2`:
  - `./scripts/repo-cargo nextest run -p meerkat-openai --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-anthropic --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-gemini --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-skills --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`

## Risks

- provider runtime files are shared with `D1` and must rebase cleanly
- `DOGMA-25` may appear smaller than it is because skill filtering and effective capability computation are split today
- placeholder empty leases are not acceptable end states

## Open Decisions

- none

## Handoff Notes

- handoff commit: none
- changed files:
  - `meerkat-contracts/src/lib.rs`
  - `meerkat-contracts/src/capability/registry.rs`
  - `meerkat-contracts/src/capability/mod.rs`
  - `meerkat-auth-core/src/resolver.rs`
  - `meerkat-openai/src/runtime/mod.rs`
  - `meerkat-anthropic/src/runtime/mod.rs`
  - `meerkat-gemini/src/runtime/mod.rs`
  - `meerkat/src/surface.rs`
  - `meerkat/src/factory.rs`
  - `.rct/round5/agents/D2-checklist.md`
  - `.rct/round5/agents/D2-plan.md`
- generated output request: none
- known risks:
  - `D2-1` is handoff-ready without cargo verification; requested orchestrator probes remain the source of truth for compile/test confirmation.
  - INT quick probe found one crate-root export miss for `resolve_capabilities`; fixed by re-exporting it from `meerkat-contracts/src/lib.rs`. `D2-1` is ready for re-probe after that repair.
  - `D2-2` is implementation-complete in owned files but still unverified locally because this lane must not run cargo; requested orchestrator probes are required before claiming build/test approval.
  - External resolvers that return `ResolvedAuthEnvelope::DynamicAuthorizer` still fail explicitly with `HostOwnedUnavailable`; owned runtimes now consume typed external auth material, but only inline-secret and static-header envelopes can be materialized across this boundary today.

## Milestone Split

- `D2-1` / `DOGMA-25`: complete and handoff-ready.
  - Added a shared config-aware capability resolution path in `meerkat-contracts`.
  - Switched `meerkat/src/surface.rs` to the shared resolver so surface reporting and runtime filtering use the same capability truth.
  - Switched `meerkat/src/factory.rs` skill-engine construction to use effective capabilities after config resolution plus factory/build overrides for builtins, shell, and memory.
- `D2-2` / `DOGMA-16`, `DOGMA-24`: implementation complete and handoff-ready for probe.
  - Implemented against the approved D1 `ValidatedBinding.connection_ref` contract.
  - Shared resolver now merges `metadata_defaults`, enforces required account/workspace policy, and materializes typed external-auth envelopes into real leases.
  - Owned provider runtimes now use `connection_ref` directly for persisted-token lookup, gate OAuth refresh/login by binding constraints, and replace placeholder authorizer-backed end states with real leases or explicit typed errors.
  - Ready for orchestrator re-probe; not locally cargo-verified in this lane.
