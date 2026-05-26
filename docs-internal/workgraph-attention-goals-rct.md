# WorkGraph Attention Goals RCT Ledger

Source proposal: `docs/architecture/workgraph-attention-goals.md`

Status: Implementation in progress; core WorkGraph attention, host surfaces, explicit runtime continuation, tool scoping, and mob lowering are wired. Automatic idle polling and full e2e/CI remain pending.

## Requirements

| ID | Requirement | Evidence Needed | Status |
| --- | --- | --- | --- |
| TYPE-001 | WorkGraph exposes `WorkAttentionBindingId`, `WorkItemRef`, `WorkAttentionTarget`, `WorkAttentionMode`, `WorkAttentionStatus`, `AttentionDelegatedAuthority`, and `AttentionProjectionPolicy` as typed serializable contracts. | Round-trip tests and schema export include the new types. | VALIDATED |
| TYPE-002 | `WorkCompletionPolicy` is a WorkGraph item property, not an attention binding property. | Work item contract and lifecycle tests prove completion policy is stored on the item and survives store round trip. | VALIDATED |
| TYPE-003 | Timed pause is binding-local via `Paused { until }`; item `snoozed_until` is not used for attention pause. | Attention binding round-trip plus machine eligibility test proves paused bindings are ineligible without mutating the item. | VALIDATED |
| CONTRACT-001 | Host goal creation is transactional: create or reference a WorkGraph item and create the initial attention binding. | Real service test for atomic store-level goal creation plus RPC/REST catalog/handler surface. | PUBLIC-VALIDATED |
| CONTRACT-002 | Host goal status returns both referenced item status and attention status. | Real service test for `goal_status` plus RPC/REST catalog/handler surface. | PUBLIC-VALIDATED |
| CONTRACT-003 | Host confirmation evidence and close request are policy-gated. | Real service and REST entrypoint tests for confirm/request-close success and denial. | PUBLIC-VALIDATED |
| INV-001 | WorkGraph remains the only durable work truth owner; runtime does not store a parallel goal table. | Code search plus tests showing goal APIs use WorkGraph service/store. | SERVICE-VALIDATED |
| INV-002 | Runtime remains the only admission/wake authority; WorkGraph attention only produces eligible projections. | Runtime integration test proves continuation goes through runtime `Input::Continuation` admission. | RUNTIME-SEAM-VALIDATED |
| INV-003 | Core Meerkat does not name `AgentIdentity` or `MobId` for attention targets. | Type/API grep and mob lowering tests. | VALIDATED |
| REQ-001 | Active non-terminal session-bound attention can produce an eligible continuation projection. | Runtime/service test with real session target and non-terminal item. | RUNTIME-SEAM-VALIDATED |
| REQ-002 | Terminal item, paused/stopped/superseded binding, missing item, stale revision, or unresolved target fails closed. | Negative tests for each ineligibility reason. | PARTIAL; paused public continuation and service projection fail closed |
| REQ-003 | Projections are typed, bounded, revisioned, and frame parent objective text as data rather than instruction priority. | Projection builder tests inspect structured output. | VALIDATED |
| REQ-004 | Attention-aware WorkGraph tool filtering narrows permissions by mode and item scope after category/profile exposure. | Dispatcher/service authorization tests for pursue/review/falsify/judge/observe. | TOOL-SURFACE-PARTIAL |
| REQ-005 | Mob lowers `AgentIdentity` attention targets to WorkGraph owner keys and resolves current runtime/session binding without leaking mob types into core. | Mob integration tests across respawn/reassign. | LOWERING-PARTIAL |
| E2E-001 | Session-only `/goal` flow can create, continue, pause, resume, status, and complete through real public entrypoints. | CLI/RPC/REST real-entrypoint scenario. | PARTIAL; CLI host commands plus RPC/REST goal/attention/continue surfaces wired |
| E2E-002 | Mob planner/coder/reviewer flow preserves specialized attention modes and prevents reviewer parent closure. | Mob real-entrypoint or integration scenario. | MISSING |

## Phase Plan

### Phase 0: Representation And External Contracts

- TYPE-001: Add WorkGraph attention binding types and wire schema/serde tests.
- TYPE-002: Add item-level completion policy.
- TYPE-003: Add binding-local pause representation and eligibility helper tests.
- CONTRACT-001/002/003: Pin host wire request/response shapes as typed contracts even if runtime behavior is still missing.

Done when the new contracts round-trip, schema/export surfaces include them, attention pause is machine-owned, completion policy operands are item-owned, and core host goal targets cannot bypass mob identity lowering.

### Phase 1: WorkGraph Service Semantics

- CONTRACT-001/002/003: Implement transactional goal create/status/confirm/request-close in `meerkat-workgraph`.
- REQ-001/002/003: Implement attention eligibility and projection building.
- INV-001: Prove all durable goal state is in WorkGraph.

Done when service tests validate success and fail-closed behavior through `WorkGraphService`.

### Phase 2: Host Surfaces

- CONTRACT-001/002/003 and E2E-001: Expose the narrow host goal/attention APIs through real public entrypoints.

Done when a real REST/RPC/CLI entrypoint drives goal create/status/pause/resume/confirm/request-close.

### Phase 3: Runtime Handoff

- INV-002 and REQ-001/002/003: Runtime observes eligible WorkGraph attention and enqueues continuation through existing admission.

Done when a runtime-backed integration test proves eligible attention wakes through `Input::Continuation` and ineligible attention is silent.

### Phase 4: Tool Authority And Mob Lowering

- INV-003, REQ-004, REQ-005, E2E-002: Add attention-aware WorkGraph authorization and mob identity lowering.

Done when reviewer/falsifier modes cannot close parent work, and mob member attention survives binding rotation through `AgentIdentity` lowering.

### Final Phase: Full Gates And Release

- All required rows are `VALIDATED`.
- No typed-but-unwired items remain.
- `make check`, `make lint`, targeted tests, and relevant e2e lanes pass.
- PR is created and CI is green.

## Typed But Unwired

- Automatic idle polling is not wired yet. Runtime continuation can be triggered explicitly through `workgraph/attention/continue`, which still routes through runtime admission and hidden `Input::Continuation`.
- Stop/reassign/get attention controls are intentionally not public entrypoints yet; reassign remains a future/service-internal request shape.
- Mob `AgentIdentity` lowering is implemented as a feature-owned helper; current-session binding rotation remains a mob integration follow-up.

## Evidence Log

| Timestamp | Command | Evidence |
| --- | --- | --- |
| 2026-05-26T10:09:42Z | `jq empty docs/docs.json` | Proposal navigation JSON was valid before implementation start. |
| 2026-05-26T10:21:20Z | `./scripts/repo-cargo test -p meerkat-workgraph --test attention_contracts` | 3 tests passed: attention binding round-trip, item completion-policy memory round-trip, goal create wire shape. |
| 2026-05-26T10:21:20Z | `./scripts/repo-cargo test -p meerkat-workgraph` | 31 tests passed including unit tests, attention contracts, and doc tests. |
| 2026-05-26T10:21:20Z | `./scripts/repo-cargo test -p meerkat-workgraph --features schema` | 31 tests passed with schema feature enabled. |
| 2026-05-26T10:21:20Z | `./scripts/repo-cargo test -p meerkat-contracts --features schema` | 243 lib tests and contract test binaries passed; schema registry compiles with new WorkGraph contracts. |
| 2026-05-26T10:21:20Z | `./scripts/repo-cargo test -p meerkat-machine-schema --test schema_contracts` | 52 schema contract tests passed. |
| 2026-05-26T10:21:20Z | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_schema_parity` | 17 passed, 2 ignored; no runtime schema parity drift reported. |
| 2026-05-26T10:32:01Z | `./scripts/repo-cargo fmt --check` | Formatting check passed after applying rustfmt. |
| 2026-05-26T10:32:01Z | `./scripts/repo-cargo check -p meerkat-rest -p meerkat-rpc -p meerkat-workgraph -p meerkat-contracts` | Surface/package check passed after adding the WorkGraph item field. |
| 2026-05-26T10:32:01Z | `make machine-codegen` | Regenerated machine/composition artifacts after WorkGraph DSL change. |
| 2026-05-26T10:32:01Z | `make machine-check-drift` | Machine authority artifacts reported up to date. |
| 2026-05-26T10:32:01Z | `make machine-verify` | TLC verification passed for 6 machines and 5 compositions; owner tests passed. |
| 2026-05-26T10:35:31Z | `./scripts/repo-cargo test -p meerkat-workgraph --test attention_contracts` | 4 tests passed after adding narrow goal/status/confirm/request-close and attention control contract round-trips. |
| 2026-05-26T10:35:31Z | `./scripts/repo-cargo test -p meerkat-workgraph --features schema` | 32 tests passed with schema feature enabled after adding additional host/control contract types. |
| 2026-05-26T10:35:31Z | `./scripts/repo-cargo test -p meerkat-contracts --features schema emit::tests::emitted_auth_rpc_contract_names_resolve_to_exported_schemas` | Exported schema registry resolves with the new WorkGraph contract names present. |
| 2026-05-26T10:58:00Z | Phase 0 gate review | Independent reviewers failed the first Phase 0 attempt: attention lifecycle was DTO-only, completion policy lost operands, host targets could bypass mob lowering, and schema evidence was too broad. |
| 2026-05-26T12:50:00Z | `./scripts/repo-cargo test -p meerkat-workgraph --test attention_contracts` | 5 tests passed after adding `WorkAttentionLifecycleMachine`, binding-local pause eligibility, operand-carrying completion policy, and narrow `GoalAttentionTarget`. |
| 2026-05-26T12:53:00Z | `./scripts/repo-cargo test -p meerkat-contracts --features schema emitted_workgraph_goal_attention_contracts_are_exported` | Targeted schema export test passed for WorkGraph goal/attention contracts. |
| 2026-05-26T12:53:00Z | `./scripts/repo-cargo run -p meerkat-contracts --features schema --bin emit-schemas` then `python3 tools/sdk-codegen/generate.py` | Schema artifacts and SDK generated types were regenerated. |
| 2026-05-26T12:54:00Z | `make verify-schema-freshness` | Reported `rest-openapi.json` and `wire-types.json` stale relative to `HEAD`, which is expected before committing the regenerated artifacts. |
| 2026-05-26T12:54:00Z | `make machine-check-drift` | Machine authority artifacts reported up to date after regenerating WorkGraph lifecycle artifacts. |
| 2026-05-26T12:55:00Z | `make machine-verify` | First rerun failed because completion policy operand coherence was invariant-only; invalid model input could violate `non_reviewer_quorum_policy_has_no_threshold`. |
| 2026-05-26T12:57:00Z | `make machine-verify` | Passed after moving completion policy operand coherence into WorkGraph lifecycle transition guards. |
| 2026-05-26T12:57:00Z | `./scripts/repo-cargo test -p meerkat-workgraph` | 28 unit tests, 5 attention contract tests, and doc tests passed. |
| 2026-05-26T12:57:00Z | `./scripts/repo-cargo test -p meerkat-machine-schema --test schema_contracts` | 52 schema contract tests passed. |
| 2026-05-26T12:57:00Z | `./scripts/repo-cargo check -p meerkat-rest -p meerkat-rpc -p meerkat-workgraph -p meerkat-contracts` | Package check passed across REST/RPC/WorkGraph/contracts. |
| 2026-05-26T12:57:00Z | `./scripts/repo-cargo fmt --check` | Formatting check passed. |
| 2026-05-26T13:05:00Z | `./scripts/repo-cargo test -p meerkat-workgraph` | 28 unit tests and 7 attention contract/service tests passed after adding store-level atomic goal creation, persisted attention bindings, service goal status, list, pause, resume, and disabled-store fail-closed coverage. |
| 2026-05-26T13:06:00Z | `./scripts/repo-cargo test -p meerkat-contracts --features schema emitted_workgraph_goal_attention_contracts_are_exported` | Targeted schema export test still passes after adding attention event kinds and regenerating schema artifacts/SDK types. |
| 2026-05-26T13:12:00Z | `./scripts/repo-cargo test -p meerkat-contracts --features schema workgraph_rpc` | 2 tests passed for narrow WorkGraph goal/attention RPC catalog exposure and schema resolution. |
| 2026-05-26T13:12:00Z | `./scripts/repo-cargo check -p meerkat-rpc -p meerkat-contracts` | RPC handlers for `workgraph/goal/create`, `workgraph/goal/status`, `workgraph/attention/list`, `workgraph/attention/pause`, and `workgraph/attention/resume` compile. |
| 2026-05-26T11:21:30Z | `./scripts/repo-cargo test -p meerkat-workgraph --test attention_contracts` | 9 tests passed after adding policy-gated `goal_confirm` and `goal_request_close` service behavior. |
| 2026-05-26T11:21:30Z | `./scripts/repo-cargo test -p meerkat-rest test_workgraph_rest_routes_expose_observability_and_narrow_goal_control` | REST route test passed; close is rejected before host confirmation and accepted after confirmation evidence. |
| 2026-05-26T11:21:30Z | `./scripts/repo-cargo test -p meerkat-contracts --features schema workgraph_rpc` | 2 tests passed after advertising `workgraph/goal/confirm` and `workgraph/goal/request_close`. |
| 2026-05-26T11:21:30Z | `./scripts/repo-cargo check -p meerkat-rpc -p meerkat-rest -p meerkat-workgraph -p meerkat-contracts` | Package check passed after service, RPC, REST, and contract updates. |
| 2026-05-26T11:21:30Z | `./scripts/repo-cargo run -p meerkat-contracts --features schema --bin emit-schemas && python3 tools/sdk-codegen/generate.py` | Schema artifacts and SDK generated types were regenerated after new goal confirmation/close surfaces. |
| 2026-05-26T11:34:16Z | `./scripts/repo-cargo test -p meerkat-workgraph --test attention_contracts` | 10 tests passed after adding eligible, bounded, revisioned attention projections and paused fail-closed projection behavior. |
| 2026-05-26T11:34:16Z | `./scripts/repo-cargo test -p meerkat-runtime continuation_projection_can_carry_runtime_context_append` | Runtime continuation input can carry a context append that becomes pending system context at the turn boundary. |
| 2026-05-26T11:34:16Z | `./scripts/repo-cargo test -p meerkat-workgraph attention_scoped_surface_hides_parent_close_for_falsifier` | Attention-scoped WorkGraph tool surface hides parent close for falsifier mode and rejects wrong-item evidence mutation. |
| 2026-05-26T11:34:16Z | `./scripts/repo-cargo test -p meerkat-mob agent_identity_lowers_to_workgraph_owner_without_mob_id` | Mob lowers `AgentIdentity` to `WorkOwnerKey::Agent` / `WorkAttentionTarget::LoweredOwner` without WorkGraph naming `MobId`. |
| 2026-05-26T11:34:16Z | `rg -n "AgentIdentity\|MobId" meerkat-workgraph || true` | No core WorkGraph references to mob identity types. |
| 2026-05-26T11:34:16Z | `./scripts/repo-cargo check -p meerkat-rpc -p meerkat-rest -p meerkat-runtime -p meerkat-workgraph -p meerkat-contracts -p meerkat-mob` | Package check passed across WorkGraph, runtime, mob, RPC, REST, and contracts after projection/tool/mob lowering changes. |
| 2026-05-26T11:34:16Z | `./scripts/repo-cargo fmt --check` | Formatting check passed after rustfmt. |
| 2026-05-26T11:44:55Z | `./scripts/repo-cargo check -p rkat -p meerkat-rpc -p meerkat-rest -p meerkat-contracts -p meerkat-workgraph` | Package check passed after adding CLI goal/attention commands and explicit `workgraph/attention/continue` RPC/REST runtime handoff. |
| 2026-05-26T11:44:55Z | `./scripts/repo-cargo test -p meerkat-workgraph --test attention_contracts` | 10 attention contract/service tests passed. |
| 2026-05-26T11:44:55Z | `./scripts/repo-cargo test -p meerkat-contracts --features schema workgraph_rpc_surface_exposes_observability_and_narrow_goal_control` | RPC catalog test passed including `workgraph/attention/continue`. |
| 2026-05-26T11:44:55Z | `./scripts/repo-cargo test -p meerkat-rest test_workgraph_rest_routes_expose_observability_and_narrow_goal_control` | REST route test passed; paused attention fails closed through the public continuation endpoint. |
| 2026-05-26T11:44:55Z | `./scripts/repo-cargo run -p meerkat-contracts --features schema --bin emit-schemas && python3 tools/sdk-codegen/generate.py` | Schema artifacts and SDK generated types were regenerated after the explicit attention continuation surface. |
| 2026-05-26T11:44:55Z | `./scripts/repo-cargo test -p meerkat-contracts --features schema emitted_workgraph_goal_attention_contracts_are_exported` | Targeted schema export test passed after regeneration. |
| 2026-05-26T11:44:55Z | `make verify-schema-freshness` | Failed because the freshness script compares generated artifacts against `HEAD`; expected while schema artifacts are modified but not committed. |
| 2026-05-26T11:44:55Z | `./scripts/repo-cargo fmt --check` | Formatting check passed. |
| 2026-05-26T11:48:32Z | `./scripts/repo-cargo test -p meerkat-rpc workgraph_attention_continue_queues_runtime_continuation` | Runtime-backed RPC test passed: a real session-bound WorkGraph goal enqueues through runtime continuation admission. |
| 2026-05-26T12:42:17Z | `./scripts/repo-cargo test -p meerkat-mob test_retire_fanout_notifies_150_peers_with_bounded_parallelism` | Focused mob fanout test passed after replacing load-sensitive wall-clock assertion with observed peer-lifecycle concurrency bounds. |
| 2026-05-26T12:42:17Z | `./scripts/repo-cargo test -p meerkat-runtime --test driver_ephemeral` | 38 driver admission tests passed after aligning stale driver-owned post-admission expectations with machine-owned signals. |
| 2026-05-26T12:42:17Z | `./scripts/repo-cargo test -p meerkat-runtime --test regression_comms_runtime` | 23 comms runtime regression tests passed; explicit queue while running stays queued and requests an idle wake. |
| 2026-05-26T12:42:17Z | `./scripts/repo-cargo test -p xtask rpc_catalog_router_docs_and_sdk_wrappers_are_aligned` | RPC router, contract catalog, docs, and SDK method strings are aligned for the new WorkGraph goal/attention methods. |
| 2026-05-26T12:42:17Z | `make test` | 7311 tests passed, 70 skipped. |
| 2026-05-26T12:42:17Z | `make check` | Workspace check passed. |
| 2026-05-26T12:42:17Z | `make lint` | Clippy lint lane passed. |
| 2026-05-26T12:42:17Z | `make machine-check-drift` | Machine authority artifacts are up to date. |
| 2026-05-26T12:42:17Z | `make machine-verify` | TLC verification passed for 6 machines and 5 compositions; owner tests passed. |
| 2026-05-26T12:42:17Z | `make verify-schema-freshness` | Expected pre-commit failure: regenerated schema artifacts differ from `HEAD` and must be committed before the freshness check can compare cleanly. |
| 2026-05-26T13:13:21Z | `make test` | Rebased on `origin/main`/0.6.23; 7317 tests passed, 70 skipped. |
| 2026-05-26T13:13:21Z | `make verify-schema-freshness` | Rebased schema artifacts are fresh against the committed branch tip. |
| 2026-05-26T13:13:21Z | `make check` | Rebased workspace check passed. |
| 2026-05-26T13:13:21Z | `make lint` | Rebased clippy lint lane passed. |
| 2026-05-26T13:13:21Z | `make machine-check-drift` | Rebased machine authority artifacts are up to date. |
| 2026-05-26T13:13:21Z | `make machine-verify` | Rebased TLC verification passed for 6 machines and 5 compositions; owner tests passed. |
