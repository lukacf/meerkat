# Lane D1 Plan

## Owned Rows

- `DOGMA-15`
- `DOGMA-17`

## Owned Milestones

- `D1-1` = `DOGMA-15`, `DOGMA-17`, plus the `ValidatedBinding.connection_ref` contract

## Owned Files

- `meerkat-core/src/connection.rs`
- `meerkat-core/src/auth/token_store.rs`
- `meerkat-runtime/src/handles/auth_lease.rs`
- `meerkat-llm-core/src/provider_runtime/binding.rs`
- `meerkat-llm-core/src/provider_runtime/registry.rs`
- `meerkat-rpc/src/handlers/auth.rs`
- `meerkat-rest/src/auth_endpoints.rs`
- `meerkat-web-runtime/src/external_auth.rs`
- auth-binding plumbing in `meerkat/src/factory.rs`

## Shared Files Touched Under Round Map

- `meerkat/src/factory.rs` as first owner
- provider runtimes only for identity contract if strictly required before `D2`

## Blocked By

- none

## Unblocks

- `D2-2`

## Public / Type / Schema Changes

- canonical auth identity becomes binding-scoped `ConnectionRef`
- `ValidatedBinding` gains `connection_ref: ConnectionRef`
- REST/RPC auth inputs normalize to binding identity
- WASM external-auth callback key uses `<realm_id>:<binding_id>`
- no SDK regen expected directly, but schema/freshness may be affected indirectly by REST/RPC auth payloads

## Defensive Scan Commitments

- add or update a scan banning `profile_id` as runtime/persistence auth identity in resolution and storage paths

## Requested Orchestrator Probes

- `./scripts/repo-cargo check -p meerkat-core`
- `./scripts/repo-cargo check -p meerkat-llm-core`
- `./scripts/repo-cargo check -p meerkat-auth-core`
- `./scripts/repo-cargo check -p meerkat-web-runtime`
- `./scripts/repo-cargo check -p meerkat-rpc`
- `./scripts/repo-cargo check -p meerkat-rest`

## Requested Build Gate Commands

- Quick probe:
  - `./scripts/repo-cargo check -p meerkat-core`
  - `./scripts/repo-cargo check -p meerkat-llm-core`
  - `./scripts/repo-cargo check -p meerkat-auth-core`
  - `./scripts/repo-cargo check -p meerkat-web-runtime`
  - `./scripts/repo-cargo check -p meerkat-rpc`
  - `./scripts/repo-cargo check -p meerkat-rest`
- Full gate:
  - `./scripts/repo-cargo nextest run -p meerkat-core --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-llm-core --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-auth-core --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-web-runtime --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rpc --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rest --test rest_auth_endpoints --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`

## Risks

- `D1` is the auth identity checkpoint for the round
- if `ValidatedBinding.connection_ref` is incomplete, `D2` will build on the wrong contract
- no shim or dual-key fallback is acceptable in handoff commits

## Open Decisions

- none

## Handoff Notes

- handoff commit: none (worker stopped at handoff-ready uncommitted state)
- changed files:
  - `meerkat-llm-core/src/provider_runtime/binding.rs`
  - `meerkat-llm-core/src/provider_runtime/runtime.rs`
  - `meerkat-llm-core/src/provider_runtime/registry.rs`
  - `meerkat-openai/src/runtime/mod.rs`
  - `meerkat-anthropic/src/runtime/mod.rs`
  - `meerkat-gemini/src/runtime/mod.rs`
  - `meerkat-rest/src/auth_endpoints.rs`
  - `meerkat-rpc/src/handlers/auth.rs`
  - `meerkat-web-runtime/src/external_auth.rs`
  - `scripts/d1_auth_identity_scan.sh`
  - `.rct/round5/agents/D1-checklist.md`
  - `.rct/round5/agents/D1-plan.md`
- generated output request: none
- known risks:
  - no `cargo`/`nextest` commands were run locally per lane instruction, so compile/test confirmation is still required from the orchestrator probes below
  - INT quick probe found and this lane fixed one borrow-checker issue in `resolve_binding_identity` (`realm.realm_id` moved while `binding` borrowed); D1 is ready for re-probe but still depends on orchestrator confirmation
  - REST/RPC auth payloads now surface canonical `connection_ref`/`binding_id`; any SDK/catalog/schema follow-up belongs to the public-surface lane if they choose to formalize those payloads
  - the worktree contains unrelated in-flight edits from other lanes; only the D1-owned/auth-identity files above were changed for this handoff
- D2 checkpoint contract:
  - treat `ValidatedBinding.connection_ref` as the only canonical auth identity in provider/runtime policy work
  - persisted token lookup and external-auth callback identity are both binding-scoped `<realm_id>:<binding_id>` now; do not reconstruct identity from `auth_profile.id` or `backend_profile.id`
  - profile ids remain informational metadata only in REST/RPC responses; do not reintroduce them as storage keys or policy lookup keys
