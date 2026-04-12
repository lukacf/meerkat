# Mob-Meerkat Seam Shadow Checks

Status: active pre-cutover validation plan

This note turns `mob-meerkat-composition-refinement-delta.md` into concrete
bridge-level shadow checks.

The aim is to verify that the live implementation shadows the frozen seam
without exposing hidden Meerkat internals back to Mob semantics.

## Check families

### B1. Lifecycle bridge checks

Compare live bridge events against seam lifecycle states:

- provisioning
- ready
- retiring
- destroyed
- failed

Reject as seam leaks:

- direct dependence on hidden `SessionId` or epoch/cursor detail in Mob-facing
  semantics
- lifecycle decisions that require hidden Meerkat queue/barrier/tool state

### B2. Work bridge checks

Compare live bridge events against seam work states:

- pending decision
- accepted
- rejected
- completed
- failed
- canceled

Reject as seam leaks:

- direct dependence on raw internal turn helper results rather than seam work
  outcomes
- flow-step decisions that require hidden Meerkat turn internals

### B3. Supersession / fencing checks

Compare:

- runtime identity
- fence token
- stale lifecycle/work events

Reject as seam leaks:

- Mob acting on stale Meerkat work/lifecycle events as current truth
- multiple concurrent incarnations being treated as one unfenced member state

### B4. Destroy / retire bookkeeping checks

Compare:

- in-flight destroy
- retiring runtime with pending submit
- replacement provisioning while old runtime drains

Reject as seam leaks:

- duplicate destroy semantics
- hidden Meerkat terminal bookkeeping needed for Mob correctness

## Exit criteria

This plan is complete enough to start implementation once:

- live bridge observation points are identified for all four families
- mismatch output can point back to one of the four seam check families
- failures can be triaged with the same three-class scheme:
  - `implementation_detail`
  - `semantic_gap`
  - `dogma_violation`

## Read with

- `mob-meerkat-composition-refinement-delta.md`
- `mob-meerkat-composition-freeze.md`
- `mob-meerkat-composition-proof-handoff.md`
- `two-kernel-refinement-program.md`
