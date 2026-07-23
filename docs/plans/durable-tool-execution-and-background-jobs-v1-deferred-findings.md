# Durable Tool Execution v1 — Deferred Findings

This ledger records review findings that are adjacent to, but outside, the
active delivery slice. Entries must identify the owning future slice and must
not be absorbed into current work without re-triage.

## Open

None at the Phase 0–1 checkpoint.

## Resolved in scope

- Phase 0–1: repeated runtime wake injection while `AgentApplied` lagged could
  duplicate an already accepted one-shot completion continuation. The runtime
  now injects once; boundary-level delivery owns Busy/cursor retry.
- Phase 0–1: macOS could return `EPERM` when probing a process-group identity
  after an accepted drop-time SIGKILL fence, preventing unreturned-operation
  rollback forever. The stale identity is retired only when the earlier kill
  fence is proven; ordinary probe failures remain armed.
