# Lane C Plan

## Owned Rows

- `DOGMA-12`
- `DOGMA-19`

## Owned Milestones

- `C1` = `DOGMA-12`, `DOGMA-19`

## Owned Files

- `meerkat-comms/src/io_task.rs`
- `meerkat-comms/src/inbox.rs`
- `meerkat-runtime/src/comms_drain.rs`
- adjacent commit-point helpers if needed:
  - `meerkat-runtime/src/meerkat_machine/comms_drain.rs`

## Shared Files Touched Under Round Map

- none outside the lane-owned trust seam

## Blocked By

- none

## Unblocks

- none hard-blocked, but reduces trust/regression risk for the round

## Public / Type / Schema Changes

- no public rename surface
- internal ingress/rollback semantics only
- no schema or SDK regen expected

## Defensive Scan Commitments

- add or update a scan banning:
  - trust publication before DSL bind/authorize commit
  - ack-before-final-admission behavior
  - dual ownership of ingress trust decisions
- fulfilled in lane-owned regression tests inside:
  - `meerkat-comms/src/io_task.rs`
  - `meerkat-comms/src/inbox.rs`
  - `meerkat-runtime/src/comms_drain.rs`

## Requested Orchestrator Probes

- `./scripts/repo-cargo check -p meerkat-comms`
- `./scripts/repo-cargo check -p meerkat-runtime`

## Requested Build Gate Commands

- Quick probe:
  - `./scripts/repo-cargo check -p meerkat-comms`
  - `./scripts/repo-cargo check -p meerkat-runtime`
- Full gate:
  - `./scripts/repo-cargo nextest run -p meerkat-comms --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-runtime --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`

## Risks

- row `12` and `19` are one semantic seam and must stay commit-first, trust-second
- handler rollback paths are easy to leave half-correct without explicit tests

## Open Decisions

- none

## Handoff Notes

- handoff commit: none
- changed files:
  - `meerkat-comms/src/io_task.rs`
  - `meerkat-comms/src/inbox.rs`
  - `meerkat-runtime/src/comms_drain.rs`
  - `.rct/round5/agents/C-checklist.md`
  - `.rct/round5/agents/C-plan.md`
- generated output request: none
- known risks:
  - cargo checks/tests not run in-lane per instruction; orchestrator should run the requested probes before merge
  - defensive scan is implemented as owned regression tests rather than a new top-level script to stay within lane-owned file scope
  - INT quick probe found and this lane fixed one compile issue: `SupervisorBindingStageError` import had been test-gated while rollback helpers use it in production code; re-probe requested
  - full gate found and this lane fixed one test compile issue: `meerkat-comms/src/io_task.rs` test module needed `InboxItem` imported back into scope; re-probe/full-gate rerun requested
  - full gate then found two stale `io_task` test expectations that still assumed untrusted ingress returned `Ok(())`; this lane updated them to assert `Err(IoTaskError::IngressDropped(DropReason::UntrustedSender))`
  - full gate then found one borrow/move issue in the new rollback coverage: `bind_member_rolls_back_binding_when_trust_publication_fails` now clones the sender peer id before moving `payload` into `BridgeCommand::BindMember`
