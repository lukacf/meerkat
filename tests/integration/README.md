# Integration and Turbo S scenarios

## Turbo S oracle rule (review checklist)

Turbo S is the end-to-end run of the live stack against the real provider. A
scenario's verdict must mean something, so every check in it is one of two
kinds:

- **A contract assertion that fails the scenario.** It names a product
  contract: a typed event (`session.delegation.created`, a settlement or
  close signal), a canonical transcript row, a machine state, or a file the
  executor was asked to write. It must hold for every valid model behaviour.
  Where gpt-live-1 is inherently variable, make the scenario deterministic
  (instructions, fixture cut points, typed anchors) or assert only the
  invariant every valid behaviour satisfies.
- **A measurement with no verdict** (`record_metric`). Model wording and
  wall-clock latency are journaled and printed for diagnosis, never judged.

Not allowed:

- "Tolerant", soft-fail, advisory or record-only checks: anything evaluated
  that cannot fail the run. `scripts/turbo-s-oracle-gate`
  (`make turbo-s-oracle-gate`, run in CI) rejects the known shapes: that
  vocabulary, a `passed: bool` parameter or any bool parameter on a
  `record_*`/`note_*`/`check_*`/`observe_*` function, and any `let mut
  *_failures` list besides `deterministic_failures`. The gate is lexical: it
  stops the old shapes from returning, not a determined workaround (a
  computed predicate that is only printed, `let _ = condition`, a macro).
  Review is the backstop: every new check either pushes onto
  `deterministic_failures` (which the scenario turns into its error) or is a
  plain `record_metric` with no pass/fail field.
- Wall-clock margins as pass criteria, and waits that pace on timers. Wait
  on typed events; a hang guard is a failure bound, not a pass condition.
- Retries. Each scenario runs once (`--flaky_test_attempts=1`). A failure is
  classified as a product bug, oracle or fixture brittleness, or a
  provider-degraded void (the scenario's typed `GPT_LIVE_VERDICT`), never
  absorbed.

Pull requests that touch the live stack run the GPT Live scenarios before
merge (`.github/workflows/live-gate.yml`).
