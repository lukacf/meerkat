# MeerkatMachine Refinement Delta

Status: active pre-cutover refinement note

This note records the remaining gap between:

- the frozen exact-current baseline in `meerkat-machine-exact-current-freeze.md`
- the frozen target machine in `meerkat-machine-freeze.md`

It is not a claim that the target machine is unstable. It is the explicit list
of what still needs implementation-side lowering or cleanup before a write-side
cutover would be honest.

## Why this note exists

The branch is now at a good place:

- the exact-current `MeerkatMachine` baseline is frozen
- the target `MeerkatMachine` is frozen
- the target machine has an executable TLA scaffold
- the rebased exact-current tools seam is aligned again

The next question is no longer “what is the machine?” It is “what exactly still
separates the current implementation from the frozen target?”

## Delta classes

This delta classifies each gap as one of:

- `accepted_target_cleanup`
- `required_implementation_work`
- `target_promotion_not_yet_lowered`
- `resolved_on_branch`

## Major deltas

### 1. `CancelAfterBoundary` is now a live top-level lowering

Classification:

- `resolved_on_branch`

Current state:

- the runtime authority surface now lowers `CancelAfterBoundary` through:
  - `MeerkatMachine::cancel_after_boundary(...)`
  - `SessionService::cancel_after_boundary(...)`
  - the live boundary-cancel flag observed by the agent runner at safe phases

Target state:

- `CancelAfterBoundary` is part of the top-level `MeerkatMachine` alphabet

Cutover consequence:

- this item is no longer a write-side blocker; remaining cancel work is about
  runtime semantics and coverage rather than missing top-level lowering

### 2. Detached wake still has an exact-current compatibility fallback

Classification:

- `accepted_target_cleanup`

Current state:

- exact current freezes both:
  - feed-backed detached wake as canonical live behavior
  - legacy latch behavior as compatibility fallback

Target state:

- detached wake is single-pathed through machine state and machine effects
- legacy latch semantics are not part of the target machine

Cutover consequence:

- the fallback can be removed as cutover cleanup instead of being modeled as
  lasting semantic truth

### 3. Tools are now split in exact current, but target publication is still stronger

Classification:

- `required_implementation_work`

Current state:

- the rebased exact-current branch now honestly freezes the tools region as:
  - `tool_visibility`
  - `tool_surface`
- durable visibility ownership, exact catalog seams, and session-task mutation
  seams now exist in the live branch
- committed visibility publication now has one explicit helper seam:
  `Agent::publish_committed_visible_set()`

Target state:

- provider schema generation and dispatch gating consume one committed combined
  visible set
- publication is transactional and revisioned
- durable intent and surface mutation are one machine-owned story

Cutover consequence:

- this is now an implementation/lowering problem, not a missing target
  definition

### 4. Completion waiters move from supporting carrier to explicit machine region

Classification:

- `target_promotion_not_yet_lowered`

Current state:

- exact-current ownership still treats completion waiters as a supporting
  carrier refined from input-owned queued work

Target state:

- completion waiters are explicit machine state with their own target region

Cutover consequence:

- recovery/replay/publication lowerings must stop relying on support-carrier
  folklore and instead use explicit machine state

### 5. Peer replay remains narrower in exact current than in target

Classification:

- `required_implementation_work`

Current state:

- exact-current peer truth is canonical at the queue/authority seam
- the current branch does not durably replay raw peer queue contents as a
  second long-lived ledger

Target state:

- peer replay lineage is explicit machine state
- replay safety is stated from machine-owned admitted/terminalized lineage

Cutover consequence:

- implementation replay/reconstruction work must be tightened to match the
  frozen target peer story

### 6. Drain semantics are exact-current narrower than the full target

Classification:

- `required_implementation_work`

Current state:

- exact-current drain freeze follows the live adapter-owned seam and current
  public lowerings

Target state:

- drain / keep-alive is owned as whole-machine lifecycle state
- non-disabled drain state is tied to live binding and published mode

Cutover consequence:

- write-side cutover needs one canonical drain authority path rather than a
  mix of helper and adapter conventions

### 7. Recovery is still helper-rich even where target ownership is frozen

Classification:

- `required_implementation_work`

Current state:

- exact-current recovery behavior is frozen and tested
- the implementation still reaches parts of that behavior through helper-rich
  session/runtime/service seams

Target state:

- recovery reconstructs canonical machine truth region by region

Cutover consequence:

- recovery lowering must be centralized enough to treat the machine as the
  owner, not the documentation around the helpers

## What does not need reopening

This note does **not** reopen:

- the frozen exact-current baseline
- the frozen target `MeerkatMachine`
- the Meerkat TLA safety baseline

The point is to make cutover work explicit, not to make the target fuzzy again.

## Immediate use

Use this note to drive:

1. Meerkat cutover lowering inventory
2. shadow-validation mismatch classification
3. write-side cutover sequencing

This note should shrink over time as the implementation is brought into
alignment with the frozen target machine.
