# Dual-authority seam assessment - mobkit lane

**In response to** the downstream app's `dual-authority-seam-inventory.md` (their PR
#154), at the owner's request. The meerkat lead has answered separately; this
is the mobkit chair. Written the same week we lived incidents 1.2, 1.3, 1.4,
1.5, 1.6 and 1.11, so the classifications below are from the trench, not the
whiteboard.

**Headline position:** the inventory's tally is correct and its minimum
invariant should be adopted verbatim by all three parties:

> No layer keeps a second copy of another layer's durable state, and no layer
> makes a semantic ruling about a store it does not own.

Both of this week's defects violated exactly that sentence, and both fixes
consisted of restoring it (the wrapper fold stopped ruling on transcript
authoring; the mint stopped seeding from a lagging projection). I also concur
with the meerkat lead's projection rule: one-way, typed, observable,
rebuildable, non-authoritative. MobKit's job below is to say, per row, which
of its holdings are *host state* (legitimate, distinct scope) and which are
*shadows* (to be demoted or deleted), and what that costs.

---

## Row-by-row (the mobkit column)

### 1. Session transcript
- **Authority:** meerkat. Not contested, and not actually dual today: the
  mobkit continuity store is a meerkat `SessionStore` *implementation*
  (the adapter), not a second truth.
- **The real shadows:** (a) the WholeBlob snapshot lane - known to lag
  (inventory 1.6), already demoted from every read path this week
  (head-canonical-first reads; the cold mint seeds from `session_store.load`
  and hydrates the rewrite graph as of 0.8.13). (b) `runtime.sqlite` - a
  projection by design; the reset-reseed lane exists *because* it is
  disposable.
- **Ruling:** keep both demotions and make them enforceable: the
  representation+count load traces (shipped 0.8.13) plus a `doctor` check
  that flags any read served from the blob lane while a head row exists.
  Classification: **mechanical, mostly already paid for.**
- **Blast radius of finishing it:** legacy blob-canonical sessions still
  need the fallback until the head-canonical migration deadlock (1.11) has a
  supported crossing. That crossing is the one structural item in this row
  and it is meerkat-store-format work; mobkit's part is refusing to invent
  side doors around it.

### 2. Schedules - the row I most want consolidated
- **Authority:** conceded to meerkat per their assessment, **on one
  condition that is the actual lesson of Bug C: no store may accept a write
  that no driver drains.** Authority without the drainer is how 30
  occurrences rotted in a store nobody read.
- **Consolidation shape:** mobkit's firing driver becomes a client of the
  single meerkat store (or meerkat hosts the driver outright); either way
  `schedule.sqlite` stops being a second truth. Until that lands, the
  fail-closed stopgap is cheap and mobkit-side: **refuse schedule writes
  when no firing host is bound to the target store** - turning the silent
  Bug C class into a typed refusal at write time. I will ship the stopgap
  in 0.8.15 regardless of the consolidation schedule.
- **Classification:** stopgap mechanical; consolidation structural
  (store migration + driver rehoming).

### 3. Mob orchestration / runtime projections
- **Authority:** meerkat-mob for mob durable lifecycle (per their
  assessment). MobKit's identity-first layer holds *host desired state*
  (roster intent, identity continuity, leases) - legitimate, distinct
  scope - plus *projections* of runtime lifecycle (`runtime_state` rows,
  bindings) that Bug K proved can become load-bearing lies.
- **Ruling:** adopt the owner's durable-kernel doctrine mechanically here:
  every mobkit projection row must be rebuildable from meerkat authority at
  boot, and boot reconciliation (not operator surgery) is the repair path.
  The #56/#61 walks were steps; the remaining gap is an explicit
  "reconcile projections from authority" pass replacing per-incident fixes.
- **Classification:** structural, staged; aligns with meerkat's post-0.8.18
  direction. This row and row 2 are my answer to question 5: **the two I
  would least like the operator deployment to discover at scale** - both fail silently and both
  scale with fleet size.

### 4. Instruction/prompt semantics
- **Authority:** meerkat, full stop. The wrapper fold was mobkit making a
  transcript-authoring ruling it did not own; 0.8.14 removes it on resumes.
- **Remaining debt:** on *mint* builds the wrapper still translates
  `additional_instructions` into `SystemPromptOverride::Set` because the RPC
  session lane lacks a native standing-instructions carrier. The complete
  fix is to carry them as meerkat's own `additional_instructions` (the mob
  spawn lane already does exactly this via
  `spawn_spec.with_additional_instructions`). If `CreateSessionRequest`
  cannot carry them today, that is a one-field meerkat ask; mobkit deletes
  the translation the day it exists. **Mechanical.**

### 5. Transcript repair
- **Authority:** meerkat's typed rewrite door - and the 0.8.14 maintenance
  binary is a *client* of that door, not a second implementation: it
  composes `commit_transcript_rewrite` through the adapter, and the door's
  guards hold on every path. What is host-owned by design (ratified by all
  three parties this week) is the *selection policy*. The 0.8.15 live verb
  will thread mobkit → `session/rewrite_transcript` with a proper member
  quiesce, not reimplement rewrite semantics.
- **Classification:** settled this week; the quiesce design is the
  remaining structural piece and it is deliberately not being rushed.

### 6. Agent memory
- **Authority proposal:** meerkat's semantic memory is the engine; mobkit's
  `agent_memory` RPCs + recorder are the single *host surface*; nothing
  else writes. The downstream app's shadow tool exists because mobkit's in-turn
  recall RPC deadlocks - that deadlock is a mobkit defect and the honest
  first move: fix it, then the shadow (and its direct sqlite reads) can be
  deleted by its owner. Bug D (host builder clobbering the recorder) argues
  for the recorder being non-clobberable rather than convention-protected.
- **Classification:** structural-lite; needs its own pass with the downstream app as
  the consumer. I will take the deadlock as a named 0.8.15 item.

### 7. Compaction / retention
- **Authority:** meerkat. MobKit's revision retention is storage policy
  over its own store - a distinct concern, not a copy - and the KB cap
  living in the downstream app is evidence for meerkat's already-queued
  context-budget/limit-degradation work, not for a mobkit holding.
- **Classification:** no mobkit change beyond observability hooks.

### 8. Live channels
- **Authority:** meerkat for adapter capability; the 0.8.12 per-open
  provider selection already made the binding explicit at `live_open`.
  Residual risk is regression, not architecture. **Closed, guard with
  tests.**

---

## Answers to the five questions, compressed

1. **Ownership:** meerkat owns rows 1, 2, 4, 5, 7, 8; mob durable lifecycle
   in row 3. MobKit legitimately holds: host desired state (roster/identity
   continuity/leases), selection policy for operator repair, the host memory
   surface, and projections - which must obey the projection rule.
2. **Blast radius:** the two store consolidations (schedules; blob-lane
   retirement behind the 1.11 crossing) are the only ones needing
   migrations. Everything else is field-threading or deletion of a
   translation.
3. **Sequencing:** mechanical first, exactly as this week showed
   (1.3 and 1.4 were both closed within a day of being named). Order:
   instruction carrier (4), schedule write-refusal stopgap (2), memory
   deadlock (6), then the structural pair (2's consolidation, 3's
   reconciliation pass) with meerkat.
4. **Where the seam is load-bearing:** runtime.sqlite's disposability IS the
   recovery model - consolidating it away would remove the reset-reseed
   lane that saved the fleet twice this week. Keep it, but as a projection
   under the rule, never as truth.
5. **Operator deployment ranking:** rows 2 and 3, for silence-at-scale. Row 6 third - the
   shadow-tool pattern will be copied by every host that hits the deadlock.

## Commitments (mobkit 0.8.15 queue unless noted)

- [0.8.14, shipped] resume signal + fold removal on resumes (#62); repair
  binary as a door-client (#63).
- Schedule write-refusal without a bound firing host (fail-closed Bug C).
- `additional_instructions` carried natively on the RPC session lane
  (meerkat one-field ask if needed); delete the mint-side translation.
- In-turn memory recall deadlock fix; recorder made non-clobberable.
- Doctor check: blob-lane read while a head row exists = flagged.
- Live rewrite verb through meerkat's door with a designed quiesce.
