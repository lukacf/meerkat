# RCT-Lite Trace: Catalog-Authoritative Machine Generation

**ADR**: [Catalog-Authoritative Machine Generation and Authority Ratchet](./catalog-authoritative-machine-generation.md)
**Method**: RCT-Lite
**Owner**: Codex Product Owner / Orchestrator
**Started**: 2026-04-27

This file is the persistent RCT-Lite trace for implementing the ADR. It carries
the requirement matrix, typed-but-unwired list, and phase evidence across the
migration.

## Requirements

| ID | Requirement | Source | Phase |
| --- | --- | --- | --- |
| INV-001 | Canonical machine bodies are catalog-owned; production output is generated or shared from the catalog body. | ADR Constraints | 1 |
| INV-002 | Production must not introduce DSL state, input, signal, effect, transition, helper, invariant, disposition, or handoff metadata absent from catalog. | ADR Constraints | 1 |
| INV-003 | Bridge code is representational glue only and cannot participate in transition decisions. | ADR Bridge Code Boundary | 1 |
| INV-007 | Production-only drift must be classified in the parity ledger before lift; semantic lifts require small-batch TLC evidence with state-count and elapsed-time deltas. | ADR Constraints | 1 |
| INV-004 | Runtime commands are classified by typed metadata, not by string matching or string whitelist exemptions. | ADR Constraints | 1, 2 |
| INV-005 | Recovery cannot seed canonical lifecycle/request/member fields from projection. | ADR Constraints | 3 |
| INV-006 | Projections cannot decide lifecycle phase, terminal class, barrier satisfaction, request completion, or recovery truth. | ADR Constraints | 4, 5 |
| CONTRACT-001 | A full schema equality gate detects non-input drift between catalog and production schemas. | ADR Gate Layering | 1 |
| CONTRACT-002 | Applied machine transition journals replay strictly against catalog and payload schema digests. | ADR Recovery Journal Decision | 3 |
| CONTRACT-003 | State/effect hashes use canonical schema-shaped JSON and SHA-256. | ADR Recovery Journal Decision | 3 |
| TYPE-001 | Phase 0 inventory lists every canonical production machine body and production consumer. | ADR Phase 0 | 0 |
| TYPE-002 | Phase 0 inventory identifies bridge-only code mixed into production DSL files. | ADR Phase 0 | 0 |
| TYPE-003 | Phase 0 records generated/shared ownership and bridge extraction ownership. | ADR Phase 0 | 0 |
| REQ-001 | Phase 0 has an ignored fixture proving the current input-only gate misses a non-input drift class. | ADR Phase 0 | 0 |
| CHOKE-001 | The Phase 0 fixture uses the real machine schema types and existing input alphabet comparison shape. | ADR Phase 0 | 0 |
| E2E-001 | Phase 0 evidence includes a real `cargo test` entrypoint for the ignored fixture. | RCT-Lite Reality Gate | 0 |
| REQ-101 | Phase 1 removes second handwritten production bodies for canonical runtime-executed machines. | ADR Phase 1 | 1 |
| REQ-102 | Phase 1 protects generated production output with rerun-and-diff and generated-header audits. | ADR Generated Output Enforcement | 1 |
| REQ-201 | Phase 2 emits and gates typed command classification metadata. | ADR Phase 2 | 2 |
| REQ-301 | Phase 3 rebuilds `MeerkatMachine` recovery from applied transition records, not projection seeds. | ADR Phase 3 | 3 |
| REQ-401 | Phase 4 moves mob member lifecycle barrier semantics into `MobMachine`. | ADR Phase 4 | 4 |
| REQ-501 | Phase 5 moves request lifecycle semantics into `MeerkatMachine` and leaves surfaces with transport mechanics. | ADR Phase 5 | 5 |
| REQ-601 | Phase 6 adds rejected-pattern and accepted-pattern fixtures for every audit. | ADR Phase 6 | 6 |

## Requirement Traceability Matrix

| ID | Phase | Implemented In | Runtime Caller | Test Name | Status |
| --- | --- | --- | --- | --- | --- |
| INV-001 | 1 | `xtask/src/machines.rs`; catalog DSL macro bodies | `machine-check-drift` / BuildBuddy machine-authority target | `phase1_production_body_audit_accepts_only_declared_carry_forward_bodies`; `phase1_production_machine_schemas_match_catalog_shape` | VALIDATED |
| INV-002 | 1 | `meerkat-machine-codegen/tests/runtime_schema_parity.rs` | `runtime_schema_parity` / BuildBuddy machine-authority target | `phase1_production_machine_schemas_match_catalog_shape`; `phase1_schema_parity_inventory_pins_remaining_drift_counts` | VALIDATED |
| INV-003 | 1 | catalog macro bindings plus generated protocol bridge modules | runtime adapters and protocol submit/extract helpers | `runtime_schema_parity`; targeted MCP/Mob/Schedule runtime tests | VALIDATED |
| INV-007 | 1 | `docs/architecture/catalog-production-schema-parity-ledger.md` | Maintainer/spec-compliance review | ledger guardrails and batch rule | TYPED |
| INV-004 | 1, 2 | `meerkat-machine-derive/src/lib.rs`; runtime command manifest consumers | runtime alphabet parity tests | `runtime_alphabet_parity` | VALIDATED for Phase 1 parity; Phase 2 may extend metadata |
| INV-005 | 3 | - | - | - | MISSING |
| INV-006 | 4, 5 | - | - | - | MISSING |
| CONTRACT-001 | 1 | `meerkat-machine-codegen/tests/runtime_schema_parity.rs` | BuildBuddy machine-authority target / local fallback | `phase1_production_machine_schemas_match_catalog_shape`; `schema_parity_gate_rejects_production_only_non_input_drift` | VALIDATED |
| CONTRACT-002 | 3 | - | - | - | MISSING |
| CONTRACT-003 | 3 | - | - | - | MISSING |
| TYPE-001 | 0 | docs/architecture/catalog-authoritative-machine-generation-rct.md | Maintainer review | - | TYPED |
| TYPE-002 | 0 | docs/architecture/catalog-authoritative-machine-generation-rct.md | Maintainer review | file-specific bridge inventory | TYPED |
| TYPE-003 | 0 | docs/architecture/catalog-authoritative-machine-generation-rct.md | Maintainer review | ownership ledger columns | TYPED |
| REQ-001 | 0 | `meerkat-machine-codegen/tests/runtime_schema_parity.rs` | cargo test | `input_only_parity_misses_production_only_effect_drift` | VALIDATED |
| CHOKE-001 | 0 | `meerkat-machine-codegen/tests/runtime_schema_parity.rs` | cargo test | `input_only_parity_misses_production_only_effect_drift` | VALIDATED |
| E2E-001 | 0 | `meerkat-machine-codegen/tests/runtime_schema_parity.rs` | cargo test | `input_only_parity_misses_production_only_effect_drift` | VALIDATED |
| REQ-101 | 1 | catalog DSL macro bodies; production consumers | `machine-check-drift` / schema parity tests | `phase1_production_body_audit_accepts_only_declared_carry_forward_bodies`; `phase1_production_machine_schemas_match_catalog_shape` | VALIDATED |
| REQ-102 | 1 | `xtask/src/machines.rs`; generated artifacts | `machine-check-drift`; generated-header audit remains existing `xtask audit-generated-headers` lane | `phase1_production_body_audit_rejects_new_canonical_body_outside_catalog`; `machine-check-drift --all` | VALIDATED |
| REQ-201 | 2 | - | cargo test | - | MISSING |
| REQ-301 | 3 | - | runtime recovery tests | - | MISSING |
| REQ-401 | 4 | - | mob runtime tests | - | MISSING |
| REQ-501 | 5 | - | RPC/MCP/REST adapter tests | - | MISSING |
| REQ-601 | 6 | - | audit fixtures | - | MISSING |

## Typed But Unwired

- No schema/body parity debt remains for Phase 1. The full schema parity gate is
  unignored, the carry-forward production body list is empty, and the drift-count
  ratchet expects zero rows.
- Mob input coverage is closed-world by classification: each non-surface input
  must either have a real-entrypoint probe or a typed runtime-internal reason.
  Typed runtime-internal records must exactly match
  `schema.runtime_internal_inputs`; they are metadata, not fake probe coverage.
  The Phase 1 parity gate remains full-schema equality plus typed command /
  `ShellMechanic(reason)` classification.
- `TYPE-001` / `TYPE-002` / `TYPE-003`: retained as historical Phase 0
  inventory, now consumed by the Phase 1 audit and parity gates.
- `INV-007`: the parity ledger remains the review surface for future reopenings.
  It is no longer a license to carry MeerkatMachine or MobMachine drift.
- `CONTRACT-002` / `CONTRACT-003`: journal and hash contracts are specified in
  the ADR, but no runtime journal consumer exists until Phase 3.

## Phase Tasks

### Phase 0: Boundary Validation and Representations

| Task | Owner | Requirement | Done When |
| --- | --- | --- | --- |
| P0-T1 | orchestrator | TYPE-001, TYPE-002, TYPE-003 | This trace file lists the canonical production bodies, consumers, and bridge extraction notes. |
| P0-T2 | impl | REQ-001, CHOKE-001 | A Rust test constructs catalog and synthetic production schemas with identical input alphabets but different non-input schema shape, proving old input parity passes and schema parity fails. |
| P0-T3 | orchestrator | E2E-001 | A real `cargo test` command runs the ignored Phase 0 fixture by exact test name and records output. |
| P0-T4 | orchestrator | REQ-001 | A negative control proves the fixture fails if the synthetic drift is removed. |

### Phase 1: Catalog/Production Body Parity Ratchet

Phase 1 may close only for machines whose production-executed body is proven to
come from the catalog or to have catalog-equivalent schema shape. A partial
Phase 1 implementation must leave any unconverged machine explicitly carried
forward and must not mark `REQ-101` complete.

| Task | Owner | Requirement | Done When |
| --- | --- | --- | --- |
| P1-T1 | impl | CONTRACT-001, INV-002 | A schema parity gate compares catalog and production schema shape for every Phase 0 production body; the full-convergence assertion is unignored and the inventory test pins zero drift. |
| P1-T2 | impl | INV-001, REQ-101 | A production-owner schema relation audit proves every canonical generated kernel has typed production owner metadata; source-only `machine!` token matches do not certify ownership. |
| P1-T3 | impl | REQ-102 | Generated kernel output drift is checked through existing `machine-check-drift` / generated-header mechanisms, preferably via BuildBuddy-backed lanes where available. |
| P1-T4 | orchestrator | INV-001, INV-002 | A negative control proves the schema parity gate fails when a production-only state/effect/transition shape change is introduced in the test fixture. |
| P1-T5 | orchestrator | INV-001 | The trace names zero carry-forward handwritten production bodies; any future body drift must be rejected or newly classified before landing. |
| P1-T6 | orchestrator | INV-007 | A committed parity ledger classifies every production-only item before any lift; any item not yet itemized remains unclassified carry-forward debt. |

### Phase 1 Carry-Forward Dogma Debt

There is no Phase 1 carry-forward dogma debt. The old
`collect_phase1_production_body_mismatches` source-string scanner has been
removed from the authoritative drift gate; production ownership is now certified
by `canonical_machine_production_owner_relations` and generated-kernel drift.
Reopening any production-only body drift requires a parity-ledger row, typed
transition rule, invariant review, and TLC/runtime evidence before the schema
parity gate may be updated.

Closed Phase 1 batches:

| Machine | Production Body | Batch Evidence | Dogma Status |
| --- | --- | --- | --- |
| `AuthMachine` | `meerkat-runtime/src/auth_machine/dsl.rs` | Production now invokes `meerkat_machine_schema::auth_catalog_machine_dsl!`; schema parity inventory no longer reports Auth drift; TLC after convergence: Auth `1763 generated / 305 distinct / 00s`, `auth_lease_bundle` `2 generated / 1 distinct / 00s`. | Closed for Phase 1 body parity; future auth changes must start in catalog and pass TLC. |
| `ScheduleLifecycleMachine` delete batch | `meerkat-schedule/src/machines/schedule_lifecycle.rs` | Deleted four production-only tautological timestamp guards from `PauseActiveOrPaused`, `ResumeActiveOrPaused`, `DeleteActive`, and `DeletePaused`; TLC after deletion: ScheduleLifecycleMachine `11861 generated / 758 distinct / 00s`, `schedule_bundle` `2 generated / 1 distinct / 00s`. | Delete batch closed; full body parity is closed by the later Schedule body-parity batch. |
| `OccurrenceLifecycleMachine` delete batch | `meerkat-schedule/src/machines/occurrence_lifecycle.rs` | Deleted three production-only tautological timestamp guards from `LeaseExpiredFromClaimed`, `LeaseExpiredFromDispatching`, and `LeaseExpiredFromAwaitingCompletion`; TLC after deletion: OccurrenceLifecycleMachine `33646 generated / 8045 distinct / 00s`, `schedule_bundle` `2 generated / 1 distinct / 00s`. | Delete batch closed; full body parity is closed by the later Occurrence body-parity batch. |
| `ScheduleLifecycleMachine` body parity | `meerkat-schedule/src/machines/schedule_lifecycle.rs` | Production now re-exports `meerkat_machine_schema::catalog::dsl::schedule_lifecycle::*`; schema parity diagnostic reports no Schedule drift; runtime tests prove `DeleteDeleted` idempotence and `ConfirmOccurrencesSuperseded` ack recording; TLC after convergence: ScheduleLifecycleMachine `11861 generated / 758 distinct / 00s`, `schedule_bundle` `2 generated / 1 distinct / 00s`. | Closed for Phase 1 body parity; future schedule lifecycle changes must start in catalog and pass TLC. |
| `OccurrenceLifecycleMachine` body parity | `meerkat-schedule/src/machines/occurrence_lifecycle.rs` | Production now re-exports `meerkat_machine_schema::catalog::dsl::occurrence_lifecycle::*`; schema parity diagnostic reports no Occurrence drift; runtime tests prove `Supersede` emits reciprocal ack; TLC after convergence: OccurrenceLifecycleMachine `33646 generated / 8045 distinct / 00s`, `schedule_bundle` `2 generated / 1 distinct / 00s`. | Closed for Phase 1 body parity; future occurrence lifecycle changes must start in catalog and pass TLC. |

## Phase 0 Inventory

| Canonical Machine | Catalog Body | Production Consumer / Body | Current Production Body Owner | Bridge Extraction Owner | Phase 1 Ownership Decision |
| --- | --- | --- | --- | --- | --- |
| `MeerkatMachine` | `meerkat-machine-schema/src/catalog/dsl/meerkat_machine.rs` via `dsl_meerkat_machine()` and `meerkat_catalog_machine_dsl!` | `meerkat-runtime/src/meerkat_machine/dsl.rs` invokes the catalog macro | catalog-owned body | `meerkat-runtime::meerkat_machine` sidecar modules for domain conversions and effect realization | closed by shared source macro; production cannot carry a second Meerkat `machine!` body |
| `MobMachine` | `meerkat-machine-schema/src/catalog/dsl/mob_machine.rs` via `dsl_mob_machine()` and `mob_catalog_machine_dsl!` | `meerkat-mob/src/machines/mob_machine.rs` invokes the catalog macro | catalog-owned body | `meerkat-mob::machines` / `meerkat-mob::runtime` sidecar modules for ids, mob DSL value types, and shell adapters | closed by shared source macro; production cannot carry a second Mob `machine!` body |
| `AuthMachine` | `meerkat-machine-schema/src/catalog/dsl/auth_machine.rs` via `dsl_auth_machine()` and `auth_catalog_machine_dsl!` | `meerkat-runtime/src/auth_machine/dsl.rs` invokes the catalog macro | catalog-owned body | `meerkat-runtime::auth_machine` remains bridge-only; no bridge type declarations are in the DSL body | closed by Auth batch 1; production cannot carry a second Auth `machine!` body |
| `ScheduleLifecycleMachine` | `meerkat-machine-schema/src/catalog/dsl/schedule_lifecycle.rs` via `dsl_schedule_lifecycle_machine()` | `meerkat-schedule/src/machines/schedule_lifecycle.rs` re-exports the catalog module | catalog-owned body | `meerkat-schedule::lifecycle` for domain id/policy conversions and schedule ack persistence | closed by Schedule body-parity batch; production cannot carry a second Schedule lifecycle `machine!` body |
| `OccurrenceLifecycleMachine` | `meerkat-machine-schema/src/catalog/dsl/occurrence_lifecycle.rs` via `dsl_occurrence_lifecycle_machine()` | `meerkat-schedule/src/machines/occurrence_lifecycle.rs` re-exports the catalog module | catalog-owned body | `meerkat-schedule::lifecycle` for occurrence id/receipt/failure conversions | closed by Occurrence body-parity batch; production cannot carry a second Occurrence lifecycle `machine!` body |

### Bridge Extraction Notes

| Production File | Current Mixed Bridge / Proxy Code | Bridge Owner | Must Not Stay In Bridge |
| --- | --- | --- | --- |
| `meerkat-runtime/src/meerkat_machine/dsl.rs` | `OptionValueExt` support trait (`lines 4-11`), bridge newtypes and conversion impls for `SessionId`, `AgentRuntimeId`, `FenceToken`, `Generation`, `RunId`, `InputId`, `WorkId`, `OperationId` (`lines 15-175`), typed mirrors and structural projections for provider/connection/session/tool/realtime/surface/op/peer value types (`lines 179-1947`) | `meerkat-runtime::meerkat_machine` bridge sidecars, split by identifier/value-type families if needed | lifecycle phase decisions, terminal classification, request publish/complete/cancel ordering, guards, updates, effect emission decisions |
| `meerkat-mob/src/machines/mob_machine.rs` | `OptionValueExt` support trait (`lines 11-22`), bridge newtypes for mob/member/runtime/work/flow/peer identities (`lines 26-307`, `735-793`), DSL mirror enums/records for realtime binding, task/run/frame/loop/node/member/wiring/kickoff/work-origin state (`lines 312-735`) | `meerkat-mob::machines` bridge sidecars plus runtime adapters in `meerkat-mob::runtime` | wait/collect barrier satisfaction, member terminal class, respawn/retire/reset/destroy transition decisions, kickoff readiness decisions |
| `meerkat-runtime/src/auth_machine/dsl.rs` | production invokes the catalog-owned Auth macro; no bridge-only declarations identified in the DSL body | `meerkat-runtime::auth_machine` sidecars only | auth lifecycle legality, expiry/refresh transition semantics, emitted lifecycle effects |
| `meerkat-schedule/src/machines/schedule_lifecycle.rs` | none; production re-exports the catalog-owned schedule lifecycle module | `meerkat-schedule::lifecycle` for domain conversions and ack persistence | schedule lifecycle transition selection, revision bumping, planning cursor invariants, supersede effect decisions |
| `meerkat-schedule/src/machines/occurrence_lifecycle.rs` | compatibility alias `FailureClass = OccurrenceFailureClass` only; production re-exports the catalog-owned occurrence lifecycle module | `meerkat-schedule::lifecycle` for occurrence id/receipt/failure conversions | occurrence lifecycle legality, lease expiry behavior, delivery terminal class, failure classification decisions |
| RPC/MCP/REST surface adapters | transport framing, request id serialization, task handles, cancellation closures, cleanup closures | surface crates plus shared request executor bridge code | request publication policy, cancellation outcome, completion outcome, method/tool semantic commit classification |

## Evidence

### Phase 0

**Commit SHA at evidence time**: `542bc0ff322e69b331b20271dc0b21c11eee2970`
**Timestamp**: `2026-04-27T07:13:05Z`

Commands and outcomes:

| Gate | Command | Outcome |
| --- | --- | --- |
| Inventory source scan | `rg -n "machine!\\s*\\{" meerkat-runtime/src meerkat-mob/src meerkat-schedule/src meerkat-machine-schema/src/catalog/dsl` | Found catalog bodies and five production `machine!` bodies listed in the inventory. |
| Bridge/proxy source scan | `rg -n "DSL proxy|Bridging|OptionValueExt|struct SessionId|enum MisfirePolicy|struct OccurrenceId|struct AgentIdentity" meerkat-runtime/src/meerkat_machine/dsl.rs meerkat-mob/src/machines/mob_machine.rs meerkat-runtime/src/auth_machine/dsl.rs meerkat-schedule/src/machines/schedule_lifecycle.rs meerkat-schedule/src/machines/occurrence_lifecycle.rs` | Found the bridge/proxy declarations now listed in the file-specific bridge inventory. |
| Formatting | `cargo fmt -p meerkat-machine-codegen -- --check` | Passed. |
| Fast-suite behavior | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_schema_parity` | Passed; fixture is ignored by default: `0 passed; 0 failed; 1 ignored`. |
| Reality gate | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_schema_parity -- --ignored input_only_parity_misses_production_only_effect_drift` | Passed; explicit fixture run: `1 passed; 0 failed`. |
| Existing parity regression | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_alphabet_parity` | Passed; `4 passed; 0 failed`. |
| Stub scan | `rg -n "todo!\\(|unimplemented!\\(|panic!\\(\\\"not implemented\\\"|TODO|FIXME" meerkat-machine-codegen/tests/runtime_schema_parity.rs` | Passed; no matches. |
| Public API / dead function scan | `rg -n "pub fn|pub struct|pub enum" meerkat-machine-codegen/tests/runtime_schema_parity.rs` | Passed; no public API added. |

Spec-compliance remediation:

- Initial spec-compliance review blocked because TYPE-002 and TYPE-003 had only
  generic bridge notes.
- Added file-specific bridge/proxy rows for each production DSL file and
  ownership columns for production body ownership, bridge extraction ownership,
  and Phase 1 ownership decisions.
- Re-ran the source scan and test gates above after the remediation.

Reviewer gates:

| Gate | Verdict | Notes |
| --- | --- | --- |
| Spec compliance | APPROVE | Initial BLOCK on TYPE-002/TYPE-003; approved after file-specific bridge inventory and ownership columns were added. |
| Integration correctness | APPROVE | Independently ran default fixture test, ignored fixture test, and existing alphabet parity test. |
| Code quality and tests | APPROVE | Independently ran default fixture test, ignored fixture test, and existing alphabet parity test. |

Negative control:

- Temporarily removed the synthetic `ProductionOnlyEffectDrift` effect variant
  from `runtime_schema_parity.rs`.
- Re-ran
  `cargo test -p meerkat-machine-codegen --test runtime_schema_parity -- --ignored input_only_parity_misses_production_only_effect_drift`.
- Expected failure occurred at `runtime_schema_parity.rs:26` with message:
  `full schema parity must detect non-input drift`.
- Restored the synthetic drift and re-ran the fixture successfully.

### Phase 1

**Commit SHA at evidence time**: `b9538aff4`
**Timestamp**: `2026-04-27T09:46:00Z`

Commands and outcomes:

| Gate | Command | Outcome |
| --- | --- | --- |
| BuildBuddy generation | `make buildbuddy-generate` | Passed after fixing the generator so non-test libraries omit local dev-deps while test and binary targets retain dev-deps needed by `#[cfg(test)]` code. The command no longer changes `meerkat-cli/BUILD.bazel`, preserving main's intentional `meerkat-llm-core` dependency. |
| BuildBuddy setup | `make buildbuddy-doctor` | Passed after rebase onto `origin/main`: `34 pass, 1 warn, 0 fail`; warning is expected local worktree changes. |
| BuildBuddy machine-authority lane | `scripts/buildbuddy-dev machine-authority` | Passed after taking explicit module-lock ownership and committing only intentional Cargo file hash refreshes. Invocation `5b63b7ad-8f3e-4536-8c6f-6489fa1670ca`: `Executed 3 out of 6 tests: 6 tests pass`. RBE does not currently have `tlc` on `PATH`, so the remote shell target used the hermetic `machine-check-drift --all` fallback; local TLC evidence is recorded separately below. |
| BuildBuddy wrong-lane controls | `env BUILDBUDDY_BAZEL_COMMAND=fast-test ./scripts/buildbuddy-bazel-poc //xtask:machines_contracts_test --jobs=64` and earlier `scripts/buildbuddy-dev test //xtask:machines_contracts_test` | Rejected as non-evidence because the target is excluded by lane filters: `No test targets were found`. |
| Supplemental production body audit | `./scripts/repo-cargo test -p xtask --features machine-authority --test machines_contracts phase1_production_body_audit -- --nocapture` | Passed after integration-review macro-bypass fix; `3 passed`. The fixtures reject bare, spaced, and qualified `machine!` forms. |
| Supplemental Cargo schema parity target | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_schema_parity -- --nocapture` | Passed after the final dogmatic pass; latest full run passed with `18 passed; 2 ignored` and no schema drift rows. The non-input drift negative fixture calls the same schema comparator as the catalog/production parity cases. |
| Supplemental codegen package tests | `./scripts/repo-cargo test -p meerkat-machine-codegen` | Passed earlier after the Schedule and Occurrence delete batches and Bazel generation changes. The final Phase 1 evidence uses the focused gates below: runtime alphabet parity `7 passed`, render contracts `14 passed`, and runtime schema parity `18 passed; 2 ignored`. |
| Post-spike back-out schema parity target | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_schema_parity` | Passed after removing the mechanical lift spike; the later convergence work leaves full schema parity unignored and green. |
| Post-spike back-out production body audit | `./scripts/repo-cargo test -p xtask --features machine-authority --test machines_contracts phase1_production_body_audit -- --nocapture` | Passed after removing the mechanical lift spike; `3 passed`. The audit accepts only declared carry-forward production bodies and rejects new bare/spaced/qualified canonical bodies. |
| Auth batch classification | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_schema_parity -- --ignored print_phase1_schema_drift_items --nocapture` | Passed; diagnostic no longer reports any `AuthMachine::...` drift after production was changed to invoke the catalog-owned Auth DSL body. Remaining drift is Meerkat, Mob, Schedule, and Occurrence only. |
| Auth batch TLC baseline | `cd specs/machines/auth && tlc -config ci.cfg model.tla`; `cd specs/compositions/auth_lease_bundle && tlc -config ci.cfg model.tla` | Passed before convergence edit against current catalog specs. Auth machine: `1763 states generated, 305 distinct states found, 00s`. `auth_lease_bundle`: `2 states generated, 1 distinct states found, 00s`. |
| Auth batch codegen | `./scripts/repo-cargo xtask machine-codegen --machine auth --composition auth_lease_bundle` | Regenerated the Auth kernel and Auth lease bundle specs after production switched to the catalog macro. |
| Auth batch TLC after convergence | `./scripts/repo-cargo xtask machine-verify --machine auth --composition auth_lease_bundle --profile ci` | Passed after the rebase onto `origin/main`. Auth machine: `1763 states generated, 305 distinct states found, 00s`. `auth_lease_bundle`: `2 states generated, 1 distinct states found, 00s`. |
| Supplemental drift entrypoint | `./scripts/repo-cargo xtask machine-check-drift --all` | Passed; `machine authority artifacts are up to date`. This includes the Phase 1 production-body carry-forward audit alongside the pre-existing generated artifact drift checks, but it is not the primary BuildBuddy closure evidence. |
| Supplemental full-convergence negative proof | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_schema_parity schema_parity_gate_rejects_production_only_non_input_drift -- --nocapture` | Passed; the comparator rejects a synthetic production-only effect while the old input alphabet comparison would pass. |
| Schedule delete-batch classification | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_schema_parity -- --ignored print_phase1_schema_drift_items --nocapture` | Passed. Diagnostic no longer reports `ScheduleLifecycleMachine::transition.changed.PauseActiveOrPaused`, `ResumeActiveOrPaused`, `DeleteActive`, or `DeletePaused`; remaining Schedule drift is `superseded_ack_ids`, `ConfirmOccurrencesSuperseded*`, and `DeleteDeleted`. |
| Schedule delete-batch TLC baseline | `cd specs/machines/schedule_lifecycle && tlc -config ci.cfg model.tla` | Passed before the production delete edit against current catalog specs. ScheduleLifecycleMachine: `11861 states generated, 758 distinct states found, 00s`. |
| Schedule delete-batch codegen | `./scripts/repo-cargo xtask machine-codegen --machine schedule_lifecycle --composition schedule_bundle` | Regenerated the ScheduleLifecycleMachine kernel and schedule bundle specs after deleting the production-only tautological guards. |
| Schedule delete-batch TLC after deletion | `./scripts/repo-cargo xtask machine-verify --machine schedule_lifecycle --composition schedule_bundle --profile ci` | Passed after deletion and regeneration. ScheduleLifecycleMachine: `11861 states generated, 758 distinct states found, 00s`. `schedule_bundle`: `2 states generated, 1 distinct states found, 00s`. |
| Occurrence delete-batch classification | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_schema_parity -- --ignored print_phase1_schema_drift_items --nocapture \| rg '^OccurrenceLifecycleMachine::\|^ScheduleLifecycleMachine::'` | Passed. Diagnostic no longer reports `OccurrenceLifecycleMachine::transition.changed.LeaseExpiredFromClaimed`, `LeaseExpiredFromDispatching`, or `LeaseExpiredFromAwaitingCompletion`; remaining Occurrence drift is failure-class proxy naming plus the supersede ack route group. |
| Occurrence delete-batch TLC baseline | `cd specs/machines/occurrence_lifecycle && tlc -config ci.cfg model.tla` | Passed before the production delete edit against current catalog specs. OccurrenceLifecycleMachine: `33646 states generated, 8045 distinct states found, 00s`. |
| Occurrence delete-batch codegen | `./scripts/repo-cargo xtask machine-codegen --machine occurrence_lifecycle --composition schedule_bundle` | Regenerated the OccurrenceLifecycleMachine kernel and schedule bundle specs after deleting the production-only tautological guards. |
| Occurrence delete-batch TLC after deletion | `./scripts/repo-cargo xtask machine-verify --machine occurrence_lifecycle --composition schedule_bundle --profile ci` | Passed after deletion and regeneration. OccurrenceLifecycleMachine: `33646 states generated, 8045 distinct states found, 00s`. `schedule_bundle`: `2 states generated, 1 distinct states found, 00s`. |
| Schedule/Occurrence body parity runtime proof | `./scripts/repo-cargo test -p meerkat-schedule --lib lifecycle::tests -- --nocapture` | Passed after production re-exported the catalog bodies; `5 passed`. Runtime tests prove deleted-schedule delete idempotence, schedule supersede ack recording, and occurrence supersede ack effect emission. |
| Schedule/Occurrence schema parity proof | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_schema_parity`; `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_schema_parity -- --ignored print_phase1_schema_drift_items --nocapture \| rg '^ScheduleLifecycleMachine::\|^OccurrenceLifecycleMachine::'` | Default parity test passed with current drift reduced to Meerkat/Mob only. The filtered diagnostic produced no Schedule/Occurrence rows after body convergence. |
| Remaining Meerkat/Mob drift count ratchet | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_schema_parity` | Passed with `phase1_schema_parity_inventory_pins_remaining_drift_counts`, which now pins an empty drift map. Any future drift requires a ledger-classified batch before the test may change. |
| Schedule body parity TLC after convergence | `./scripts/repo-cargo xtask machine-verify --machine schedule_lifecycle --composition schedule_bundle --profile ci` | Passed after production re-exported the catalog body. ScheduleLifecycleMachine: `11861 states generated, 758 distinct states found, 00s`. `schedule_bundle`: `2 states generated, 1 distinct states found, 00s`. |
| Occurrence body parity TLC after convergence | `./scripts/repo-cargo xtask machine-verify --machine occurrence_lifecycle --composition schedule_bundle --profile ci` | Passed after production re-exported the catalog body and `OccurrenceFailureClass` binding was registered. OccurrenceLifecycleMachine: `33646 states generated, 8045 distinct states found, 00s`. `schedule_bundle`: `2 states generated, 1 distinct states found, 00s`. |
| Formatting | `make fmt`; `git diff --check` | Passed after the final dogmatic pass and rebase onto `origin/main`. |
| BuildBuddy setup recheck | `make buildbuddy-generate`; `make buildbuddy-doctor` | Generation passed. Doctor passed: `34 pass, 1 warn, 0 fail`; warning is expected local worktree changes. |
| Supplemental final drift entrypoint | `./scripts/repo-cargo xtask machine-check-drift --all` | Passed after all current Phase 1 changes; `machine authority artifacts are up to date`. |
| Typed command classification gate | `./scripts/repo-cargo test -p meerkat-machine-codegen --test runtime_alphabet_parity -- --nocapture` | Passed with typed `CatalogInput` / `ShellMechanic(reason)` records and an identity check proving typed catalog inputs cannot remap to other schema strings; `7 passed`. This closes the #38 string-whitelist/subtraction gap for Phase 1. |
| Generated-kernel contract gate | `./scripts/repo-cargo test -p meerkat-machine-codegen --test render_contracts -- --nocapture` | Passed after the contract was corrected to reject legacy generated `transition` / `transition_signal` stubs; `14 passed`. |
| Xtask authority ratchet gate | `./scripts/repo-cargo test -p xtask --features machine-authority --test machines_contracts -- --nocapture` | Passed with rejected-pattern and accepted-pattern fixtures for production bodies, generated-kernel boundaries, and direct flow reducer/projection writes; `25 passed`. |
| Mob projection owner checks | `./scripts/repo-cargo check -p meerkat-mob`; `./scripts/repo-cargo test -p meerkat-mob --lib run::tests -- --nocapture`; `./scripts/repo-cargo test -p xtask --features machine-authority --test machines_contracts -- --nocapture` | Passed after flow projections stopped calling `flow_run::transition`, `flow_frame::transition`, and `loop_iteration::transition`. Core seed/terminal/loop projection paths now project from accepted `MobMachine` state; active reducer commands without machine-owned typed payload transitions remain fail-closed. `meerkat-mob` check passed, `run::tests` `19 passed`, and ratchets `25 passed`. |
| Mob TLC status | `./scripts/repo-cargo xtask machine-verify --machine mob --profile ci`; temporary non-empty CI-domain spike | The fast generated CI profile remains useful as a drift smoke test but uses empty identifier domains for tractability. A non-empty generated CI-domain spike was stopped without green evidence after several minutes. Do not count Mob CI TLC as semantic closure for the migrated reducer projections; use the owner checks and ratchets above until a dedicated bounded Mob witness profile lands. |
| All-machine local TLC attempt | `./scripts/repo-cargo xtask machine-verify --all --profile ci` | Stopped without evidence after `MeerkatMachine` made no progress for several minutes. Do not count this as a green gate; use per-machine TLC rows plus the BuildBuddy machine-authority lane above. |
| MCP late-bind surface owner proof | `RUST_LANE_ID=mcp_late_bind ./scripts/repo-cargo build -p mcp-test-server`; `RUST_LANE_ID=mcp_late_bind ./scripts/repo-cargo test -p meerkat-mcp adapter_bind_external_surface_handle_replays_pending_router_state -- --nocapture` | Passed without skipping after building the test server; pending completion after late bind lands on the bound surface owner. |

Phase 1 closure status:

- `P1-T1` is closed locally: the full schema parity gate is unignored and
  passes for MeerkatMachine, MobMachine, AuthMachine,
  ScheduleLifecycleMachine, and OccurrenceLifecycleMachine. The inventory
  ratchet now expects zero carry-forward rows.
- `P1-T2` is wired: `machine-check-drift` now rejects new handwritten
  canonical production bodies in any `meerkat` / `meerkat-*` crate `src`
  outside the catalog unless they are generated kernel output checked by the
  existing generated drift comparator or are in the explicit Phase 1
  carry-forward inventory. Integration-review remediation added coverage for
  bare, spaced, and qualified macro paths.
- `P1-T3` is wired through the existing `machine-check-drift` drift system; the
  existing generated-header audit remains the `xtask audit-generated-headers`
  lane rather than a second drift mechanism.
- `P1-T5` is updated above. `REQ-101` no longer has schema-parity drift from
  MeerkatMachine or MobMachine. BuildBuddy machine-authority is green with an
  intentional `MODULE.bazel.lock` hash-only refresh for the Cargo dependency
  change; no local absolute-path churn is committed. The RBE executor lacks
  `tlc`, so the remote shell target falls back to hermetic drift verification;
  local TLC evidence remains the source of truth for TLC pass/fail.
- `INV-004` / #38 is closed for Phase 1: runtime command manifests are derived
  from typed classification records, and every non-schema command carries a
  typed `ShellMechanic(reason)` classification that the parity gate checks.
  Mob runtime-internal inputs are classified by exact typed manifest equality;
  command manifests and runtime-internal manifests are not counted as real
  entrypoint probe evidence.
- The final dogmatic review found that Mob flow reducers were still semantic
  mini-machines behind coarse authorization. Phase 1 now fail-closes
  authorization-only reducer-visible mutations and admits only machine-owned
  typed transitions for the migrated slice (`DispatchStep`, `CancelStep`, frame
  seal, loop body terminal status, and loop until feedback). Remaining active
  reducer semantics must be migrated in later phases before they can execute.
