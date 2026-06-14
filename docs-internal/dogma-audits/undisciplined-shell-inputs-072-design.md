# 0.7.2 Design — Disciplined Shell Inputs

Companion to `undisciplined-shell-inputs-072.json` (38 confirmed sites, 5 teardown
seams). That file is the worklist; this file pins the architecture. Lane agents
implement against THIS document and the JSON entries for their lane.

## Problem

Background tasks (runtime loop, comms drain, peer callbacks, interaction streams,
OAuth pruning, schedule driver ticks, mob kickoff/spawn waiters) fire machine
inputs with no machine-minted proof the input is still legal. Teardown commits
between observation and fire. The guard rejects, the shell treats the rejection
as an error, and the consequences are real: completion waiters failed for
committed turns, drain slots diverging, peer interactions hung, ERROR logs for
legitimate interleavings.

"Not allowed" must never mean "runtime error for a legitimate arrival."

## Mechanisms (all existing, cited)

- **DSL transitions**: `meerkat-machine-schema/src/catalog/dsl/*.rs`. Syntax:
  `transition Name { on input X { .. } guard ".." { .. } update { .. } to Phase emit Effect { .. } }`.
  Self-loop with empty `update {}` is a legal no-op transition (precedent:
  `ObserveCredentialFreshnessValid`, auth_machine.rs:279).
- **Effects + seams**: every effect has a `disposition <Effect> => ... seam <class>`
  clause; seam-inventory (`xtask seam-inventory`, run by `make seam-inventory`
  and `rmat-audit`) audits them.
- **C-F3 teardown pairing**: typed declarations in
  `meerkat-machine-schema/src/catalog/compositions.rs` —
  `Route { teardown: Some(EffectTeardownClass::DestroyRequest { detach_obligation: protocol_id(..) }) }`
  (compositions.rs:534) paired with
  `EffectHandoffProtocol { teardown: Some(TeardownObligationClass::DetachBeforeDestroy) }`
  (compositions.rs:2110). End-to-end precedent: mob destroy →
  `RequestSessionIngressDetachForMobDestroy` effect → shell detaches →
  `SessionIngressDetachedForMobDestroy` feedback input (mob_machine.rs:8140-8268).
- **Codegen/verify**: `cargo xtask machine-codegen --all` regenerates;
  `cargo xtask machine-verify --all` runs drift + TLC; `make machine-check-drift`
  gates CI. Generated MeerkatMachine code is macro-expanded into
  `meerkat-runtime/src/meerkat_machine/dsl.rs` etc.
- **Dispatch**: shells stage inputs via `stage_session_dsl_input` /
  `commit_session_dsl_transition`; rejection classification lives in
  `meerkat-runtime/src/meerkat_machine/dispatch_session.rs:99-111`.

## Architecture decisions

### D1 — Layer 1: machine-owned drain obligations on teardown seams

Where the racing producers are **in-process tasks owned by the same runtime**,
teardown becomes two-phase, machine-owned:

1. A `Begin<Teardown>` (or equivalent) transition moves the machine into an
   explicit draining state (prefer an existing sub-state field, e.g.
   `registration_phase`, over a new lifecycle phase when one fits) and **emits
   drain-request effects** — one per producer class that must quiesce.
2. The shell discharges each effect: stop/abort the task, **await its
   JoinHandle**, resolve outstanding waiters with a typed terminal outcome
   (never a generic error for committed work).
3. The shell fires feedback inputs closing each obligation.
4. The final teardown transition is **guarded on all drain obligations closed**.
   Only after it commits may the shell remove map entries / drop state.

Effects routed across machines get the C-F3 `DestroyRequest`/`DetachBeforeDestroy`
typed pairing in `compositions.rs`. Same-machine obligations follow the
mob-destroy effect→feedback shape.

Invariant after D1: no in-process producer can fire at a torn-down machine,
because teardown completion is *defined* as "producers quiesced."

### D2 — Layer 2: total inputs for legitimately-post-teardown arrivals

Producers that **cannot be quiesced** (external peer messages, store-driven
pollers, timers owned elsewhere) may legitimately deliver observations after
teardown. Two sub-mechanisms — pick per site based on machine-instance lifetime:

- **D2a (DSL no-op transition)** — when the machine instance still exists in the
  post-teardown state (AuthMachine `Released`, occurrence terminal phases,
  SessionTurnAdmission `ShuttingDown`, MobMachine retired members): add an
  explicit self-loop transition accepting the input in that state with empty
  `update {}`. Optionally emit a classification effect (disposition
  `local seam NoOwnerRealization`) if observability is wanted. The shell sees
  Ok, not a guard rejection.
- **D2b (typed dispatch outcome)** — when the machine instance is *removed*
  (MeerkatMachine entry dropped from the sessions map post-unregister): the
  dispatch layer returns a typed benign outcome (e.g.
  `RuntimeDriverError::NotReady { state: Destroyed }` mapped at the call site to
  a first-class `PostTeardownArrival` disposition, logged at debug, never ERROR,
  never failing waiters). Only inputs **declared observation-shaped** get this
  treatment; command-shaped inputs (user-initiated) keep hard errors.

D2 is defense-in-depth on seams that also get D1: with producers quiesced, D2
covers only what genuinely arrives from outside.

### D3 — Layer 3: capability-token pilot (two seams)

Tokens are unforgeable values minted only at machine-transition commit points
and consumed **by value** at input-firing APIs. Rules:

- Newtype struct, **private constructor**, `#[must_use]`, no `Clone`/`Copy`.
- The ONLY mint site is the dispatch code that translates the committed
  transition's effect into the token (generated effect → token wrap in the
  owning crate, `pub(crate)` at most).
- The consuming API takes the token by value; the old token-less path is
  **deleted**, not deprecated.
- TLC verifies the minting transition; rustc verifies consumption linearity.

**Pilot A — completion-resolution token (MeerkatMachine).**
`commit_runtime_loop_run`'s terminal commit (driver.rs ~2703, effect
`RuntimeCompletionResultResolved` family) mints `RuntimeCompletionResolutionToken`
carrying the committed authority facts (session_id, agent_runtime_id,
fence_token, runtime_generation, runtime_epoch_id, result_class).
`resolve_runtime_completion_waiters` consumes it by value and resolves waiters
**from the token's facts** — it no longer re-locks current driver authority, so
the AuthorityUnavailable-for-committed-turn failure mode is structurally
impossible. Composes with D1: unregister's drain obligation guarantees the loop
(and its pending resolutions) finish before teardown commits.

**Pilot B — durable-bridge-snapshot token (mob member discard).**
The 0.6.x casualty: RetryExhausted → `discard_live_session` →
"missing bridge session snapshot" on revival. Fix: discarding a live
mob-member bridge session requires by-value proof of one of:
- `DurableSnapshotCommitted` token, minted where `commit_session_snapshot`
  durably commits (SessionDocument truth), or
- a typed `DiscardWithoutDurableSnapshot` decision token minted by the OWNING
  machine authority (explicit machine-recorded fact), so a later revival finds
  a typed "no snapshot, by recorded decision" instead of a mystery-missing
  snapshot error.
The bare `discard_live_session(id)` call shape is deleted on this path.

## Per-lane assignment

Site indices refer to the order in `undisciplined-shell-inputs-072.json#confirmed`.

| Lane | Sites | Crates / DSL files | Mechanisms |
|---|---|---|---|
| L1 runtime-unregister | 0-11, 31-33, 34, 37 | meerkat-runtime, meerkat-rpc (sites 34/37 call-sites only), dsl/meerkat_machine.rs | D1 (unregister two-phase drain), D2b for observation-shaped stragglers, shell rejection-handling cleanup |
| L2 session-archive | 15-20 | meerkat-session, dsl/session_turn_admission.rs (+ session_document if touched) | D1 (archive drains admission), D2a in ShuttingDown/archived states |
| L3 auth-release | 24-30 | meerkat-runtime (oauth_flow.rs, auth_lease.rs), dsl/auth_machine.rs | D1 (release cancels in-flight flows as obligations), D2a (Expire*/Confirm* no-ops in Released/terminal flow states) |
| L4 schedule | 21-23 | meerkat-schedule, dsl/schedule_lifecycle.rs + occurrence_lifecycle.rs | D1 (delete/pause drain or revoke claimed occurrences), D2a (occurrence completion inputs total in superseded/deleted context) |
| L5 mob-kickoff | 12-14 | meerkat-mob, dsl/mob_machine.rs | D1 (retire/destroy aborts+awaits kickoff/spawn waiter tasks as obligations), D2a for late waiter arrivals |
| L6 surfaces | 35, 36 | meerkat-rest | D2b: archive-cleanup vs accept_input free-fire → typed benign outcome; depends on L1 API |
| L7 token pilot | seam A + seam B | meerkat-runtime / meerkat-mob + meerkat-session | D3 (wave 2, after L1/L5 land) |

Shared files (`compositions.rs`, seam-inventory registries, xtask): lane agents
DO NOT edit these; they record required deltas in their lane notes and the lead
applies them centrally.

## TDD protocol (batched red/green)

Per memory discipline: write-only waves, one regen/build loop, never parallel
cargo.

1. **Stage A (all lanes, parallel, write-only)**: DSL changes + complete test
   files. Machine-level tests (input X in state Y → accepted no-op / rejected /
   obligation emitted) and shell-level interleaving tests that drive the racy
   order deterministically (call teardown between commit and delivery; assert
   waiters resolve with committed outcome, no ERROR-class result). No cargo.
2. **Lead**: apply central deltas, run `cargo xtask machine-codegen --all`,
   build, run tests once. Expectation: machine-shape tests green (DSL is the
   fix), shell-behavior tests RED (old shell sequencing). Red list recorded.
3. **Stage B (all lanes, parallel, write-only)**: shell wiring per D1/D2/D3.
4. **Lead**: codegen (if DSL touched again), build, full test loop to green;
   `cargo xtask machine-verify --all`; `make seam-inventory`; clippy.
5. Wave 2 = L6 + L7 with the same cycle.
6. Adversarial workflow review gate; fix findings; e2e lanes; CI; release 0.7.2.

## Non-negotiables

- No `.unwrap()`/`.expect()`/panic in library code.
- No backwards-compat shims: replaced call shapes are deleted.
- Guard-rejection ERROR logs for legitimate interleavings must be gone at the
  end — grep gate: the strings flagged in the JSON verdicts
  ("rejected by machine authority", "authority missing") only remain on
  genuinely-illegal paths.
- Every new effect gets a `seam` classification; cross-machine teardown effects
  get typed C-F3 pairing.
- New transitions ship with TLC passing (`machine-verify`), codegen drift clean.
