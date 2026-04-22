# Lane A Checklist

- [x] `A-01` `DOGMA-08` Move peer request/progress/terminal projection out of handwritten runtime ownership into a protocol or machine seam. Done when `meerkat-runtime/src/runtime_loop.rs` no longer owns the semantic projection logic directly.
- [x] `A-02` `DOGMA-08` Move peer terminal context-key ownership into the chosen projection seam. Done when runtime loop no longer assembles the canonical peer terminal key itself.
- [x] `A-03` `DOGMA-09` Move `wait_all` short-circuit and barrier semantics behind lifecycle authority. Done when shell code no longer decides “all terminal already” or barrier activation semantics as the semantic owner.
- [x] `A-04` `DOGMA-09` Move terminate-owner target scope behind lifecycle authority. Done when shell code no longer computes the authoritative non-terminal target set.
- [x] `A-05` `DOGMA-08` `DOGMA-09` Add or update ownership tests for peer projection and ops lifecycle. Done when the new test coverage is ready for orchestrator execution.
- [x] `A-06` `DOGMA-13` Unify runtime-backed turn-state authority. Done for current runtime-backed turn flows: pre-start `PrimitiveApplied` and runtime dispatch no longer rely on `LocalTurnExecutionState` for authority decisions.
- [x] `A-07` `DOGMA-13` Extend the turn-state snapshot/handle contract for runtime-backed paths. Done when runtime-backed snapshot assembly does not join handle state with a local shadow state machine.
- [x] `A-08` `DOGMA-13` Remove runtime-backed `WaitingForOps` shadow-state dependence. Done when `OpsBarrierSatisfied` and follow-up transitions use the unified authority seam only.
- [x] `A-09` `DOGMA-13` Add or extend a defensive scan banning runtime-backed authority on `LocalTurnExecutionState`. Done when the `A1b` runtime-backed call sites are covered by the lane scan.
- [x] `A-10` Self static review. Done: the `A1b` scan passes and the identified runtime-backed local-shadow authority sites were removed.
- [x] `A-11` Handoff note. Done when handoff commit, changed files, and risks are recorded in [A-plan.md](/Users/luka/.codex/worktrees/ec4d/meerkat/.rct/round5/agents/A-plan.md).
- [x] `A-12` Generated-output request. Done when any needed machine/schema/codegen follow-up is explicitly noted in the handoff, or marked `none`.
