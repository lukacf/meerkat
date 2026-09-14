# live_source_reservation

_Generated from the Rust composition catalog. Do not edit by hand._

## Machines
- `transcript`: `LiveTranscriptMachine` @ actor `transcript_authority`
- `request`: `LiveRequestMachine` @ actor `request_authority`

## Routes
- `selected_range_freezes_source`: `transcript`.`RangeSelected` -> `request`.`Reserve` [Immediate]
- `ingress_close_fences_request_sources`: `transcript`.`IngressClosed` -> `request`.`CloseIngress` [Immediate]

## Target Selectors
- `(none)`

## Driver
- `(none)`

## Transaction Plans
- `freeze_source_and_selected_frontier` via `reserve_selected_source` / `RuntimeLiveLedgerOps::commit_live_ledger` — Exact source lookup precedes capture; the composite actor/Live witness and complete selected-range bytes bind one source row and both generated successors. Refusal spends no request, input or run; unavailable storage capacity publishes neither frontier nor key.
- `fence_observations_and_requests_before_archive` via `fence_for_archive` / `RuntimeLiveLedgerOps::commit_live_ledger` — The archive owner fences observation and source admission together before ordinary retirement; source cancellation preserves callback and unknown-effect obligations.

## Scheduler Rules
- `(none)`

## Structural Requirements
- `selected_frontier_and_source_publish_together` — The captured selected range freezes exactly one source through the joint store CAS; neither a failed request transition nor a failed commit spends a frontier.

## Behavioral Invariants
- `(none)`

## Coverage
### Code Anchors
- `live_source_joint_store_commit` (route `selected_range_freezes_source`): `meerkat-runtime/src/live_ledger/authority/source_reservation.rs` — Native selected-range producer binds exact composite context and both generated candidates to one source/head CAS; public provider installation and formal qualification remain separate.

### Scenarios
- `source-selected-range-transaction` — Native source tests cover empty and paged exact content, delayed admission, source-first replay and actual SQLite reopen; this is not public provider qualification.
