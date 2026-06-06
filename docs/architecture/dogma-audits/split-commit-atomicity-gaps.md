---
title: "Dogma Audit — Split Commit / Atomicity Gaps"
description: "Workspace audit for multi-step semantic commits with crash/rejection/cancel windows (Rules 1/2/8)."
icon: "magnifying-glass"
---

# Dogma Audit: Split Commit / Atomicity Gaps (Rules 1 / 2 / 8)

**Audit target:** detached `origin/main` @ `a1ec8de86` (includes merged R4/R6/R7 fixes)
**Date:** 2026-06-06
**Dogma:** cross-cuts Rule 1 (Authority Is Singular), Rule 2 (no semantic effects outside the owning transition),
Rule 8 (cancellation is a transition; state-mutating async must be cancel-safe / rollback-guarded / wrapped in
explicit transition state). Reference-correct pattern: the supervisor bridge rotation that **fails closed on partial
remote failure** (retains pending rotation metadata, commits only after every remote confirms).
**Test:** authority / persistence / side effects / events / trust / token writes / public replies must not happen in
separate steps with a crash, rejection, cancellation, or concurrent-interleave window that leaves **durable
inconsistency** or a **false caller-belief**. Smells: "persist first, publish machine later", "send reply before
local authority accepts", "trust before authorization terminality".

## Method

Two agent workflows (same harness). (1) A 10-agent locus map inventoried **97 sites** (51 suspect; schedule and the
agent loop over-flagged at 11/12 and 10/11). (2) A 54-candidate verify pass: each candidate got an independent
classifier that **re-traced both the split steps AND the recovery path**; suspects faced a 3-lens adversarial panel
(atomic-or-protected / ordering-correct / recovery-closes-window). Crucially the synthesis **independently
re-verified the full pool**, and I then **hand-verified the load-bearing claims** (mob wire rollback, schedule CAS,
auth token ordering, hook spawn).

**Result:** 54 candidates → **0 confirmed, 0 contested, 54 legitimate at stage 1.** But the per-candidate classifiers
were **lenient** (they waved in-memory/bounded gaps through as "no durable inconsistency"); the synthesis re-verify +
hand-check surfaced **2–3 genuine but bounded Low residuals.** **SYSTEMIC: false.**

## Bottom line

- **Widespread?** **No — the cleanest of the seven aspects audited.** ~50 of 54 candidates are legitimate
  event-sourcing / atomic-transaction / in-memory-crash-symmetric / single-lock-CAS / fail-closed-rollback patterns.
  Several candidates were factually **stale**.
- **Systemic?** **No.** There is no recurring split-commit root shape and no cross-crate transaction redesign needed.
  The codebase's durable-commit discipline is strong (see below).
- **Durable gaps?** **None found.** No two-durable-writes-without-a-transaction, no token-write-split, no
  reply-before-accept with persisted consequence. Every genuine residual is **in-memory or best-effort-by-design**.
- **Complexity?** **Low.** 2–3 localized hardening edits; no durable schema work, no new authorities.

## What makes it clean (the dominant legitimate patterns — verified)

- **Event-sourcing.** AgentEvents are non-blocking observer notifications; canonical truth is the event-sourced
  projector/checkpointer (`.rkat/sessions/` are derived projections). A crash between an event/effect and the save
  loses no canonical state — the in-memory `Session` is a CoW projection rebuilt from the durable log; `JsonlStore`
  reconciles on `open_index`; `projector::resume()` is idempotent. This refuted the entire `event_before_state`
  cluster (core loop + store/projector + schedule receipts).
- **Atomic transactions.** The schedule store runs claim/transition inside one `begin_immediate_transaction` (SQLite)
  or one `state.write().await` (in-memory) — a true CAS on `attempt_count`+`claim_token` with no await between
  read-check-write; stale completions fail closed. Refuted the `read_modify_write_race` cluster.
- **Fail-closed compensation rollback.** Auth token refresh (`meerkat-auth-core/src/resolver.rs:843`) does AuthMachine
  accept → `store.save`, and on save failure **releases the lease, restores the token-lifecycle snapshot, restores
  the token store**, returning a typed Err. **Verified — the highest-stakes concern (token-write-split / credential
  bleed) is properly handled.**
- **In-memory authority rebuilt from the durable owner.** `StagedSessionRegistry` / `staged_capacity_admissions`
  (RwLock/Mutex `HashMap`, crash-symmetric, error path rolls back via abandon+cleanup); the comms trust store
  (projection of the generated trust authority); the `AuthMachine` lease (rebuilt from the durable token store each
  call); HNSW `next_id` (re-derived `SELECT MAX(point_id)` on every open). Refuted the bulk of
  `persist_before_authority` / `trust_before_terminality`.
- **Fail-closed two-phase.** The supervisor bridge rotation (pending metadata + per-remote confirm + retry) is the
  reference, and it holds.

## Genuine residuals (all Low, in-memory / best-effort)

### R1 — Multi-peer mob WIRE rollback is best-effort, weaker than the rotation reference
**`no_rollback_on_midfail` / `effect_before_commit` · Low · meerkat-mob · members 0,1**

`apply_wire_members_idempotent` / `wire_members_peer_only` (`meerkat-mob/src/runtime/actor.rs:9748-9830`, batch
`:10272-10385`) mutate the in-memory `dsl_authority` and in-memory comms trust per peer. On a later peer's
trust/wire failure, the rollback unwires the DSL edge and prior trust — but **the rollback's own error is discarded**
(`let _ = self.apply_trusted_peer_remove(...)`, ~`:9830`, **verified**), and the DSL unwire can itself be rejected
with no recovery, leaving an orphaned in-memory edge+trust. In-memory only (a live-actor restart rebuilds from the
durable authority), so no durable inconsistency — but it's weaker than `handle_rotate_supervisor`'s pending-metadata
fail-closed pattern. **Repair:** fold per-edge wire+trust into the existing `WireMembersWithTrust` single transition
(no inter-peer window), or surface the rollback errors and record pending-unwire metadata for the next reconcile.
*(The stage-1 classifier missed this as "in-memory, legit"; the synthesis + hand-check caught it.)*

### R2 — Background hook patch publish dropped on cancellation (Rule 8 leaked task)
**`effect_before_commit` / `cancel_window` · Low · meerkat-hooks · member 50**

`meerkat-hooks/src/lib.rs:814-854` fires background hooks via detached `tokio::spawn`; on session
cancellation/shutdown the task can be killed after `execute_one()` but before `publish_patches()`, silently dropping
patches (`tracing::warn` only). Best-effort by design and the caller never relies on it — but it's an unmodeled
leaked-task fault. **Repair:** track in-flight background-hook tasks against the session lifecycle (a `JoinSet` /
tracked registry) so cancellation drains pending `publish_patches` or records an explicit "patches dropped" signal.

### R3 — Schedule materialization-before-advance idempotency contract (recovery-protected)
**`effect_before_commit` · Low · meerkat-schedule · members 20-30**

Dispatch materializes a session (effect) before the occurrence advances, but the cluster is overwhelmingly
legitimate: `ClassifyDue` emits `LeaseExpired` to reclaim crashed/stuck occurrences with a new
`claim_token`+incremented `attempt_count` (covered by `driver_reclaims_expired_*` tests), so recovery re-drives. The
only residual is making the delivery adapter **contractually idempotent** on an already-bound
`materialized_session_id` (reuse, never re-create) — defensive, recovery already mostly present.

## Systemic read & clean path

There is **no systemic split-commit shape.** The durable-commit disciplines (event-sourcing replay, SQLite IMMEDIATE
transactions, single-lock CAS, fail-closed compensation rollback, the two-phase rotation reference) are applied
consistently across the subsystems that hold durable or cross-host state. What remains are a handful of **in-memory,
live-actor, best-effort** edges that don't rise to durable inconsistency.

**Ordered plan (residuals only):**
1. **R1 (mob wire, Low→med-ish):** fold per-edge wire+trust into `WireMembersWithTrust`, or surface rollback errors +
   pending-unwire metadata (mirror `handle_rotate_supervisor`).
2. **R2 (hooks, Low):** session-scoped tracked task set for background hooks tied to the cancellation token.
3. **R3 (schedule, Low):** delivery-adapter idempotency contract on bound `materialized_session_id`.

**Overall: Low.** Everything else is verified-correct and needs no change.

## Methodology note

This audit's 0-confirmed stage-1 result was **slightly optimistic** — the per-candidate classifiers, applying the
(correct) "in-memory rebuilt-from-authority = legitimate" carve-out, waved through R1 and R2, which are genuine (if
bounded) gaps. The synthesis's independent re-verification of the full pool, plus hand-verification of the
load-bearing claims (mob `let _ = apply_trusted_peer_remove`, schedule `begin_immediate_transaction`, auth
fail-closed rollback, hook detached spawn), corrected this. The lesson for this dogma specifically: "in-memory" is
not automatically "safe" — a cancellation/rejection window in a live-actor multi-step mutation can still orphan
in-memory authority, even without durable inconsistency.

## The seven audits, summarized

| Aspect | Rule | Verdict | High |
|---|---|---|---|
| Projection Promotion | R3 | not systemic — 3 live Medium | 0 |
| String/JSON/Option Folklore | R4 | widespread + systemic; **FIXED** | 2 |
| Surface-Owned Truth | R6 | narrow, SDK last-mile; **FIXED** | 0 |
| Generated-Artifact Theater | R9 | systemic in un-gated catalogs | 2 (live) |
| Provider/Auth Policy Escapes | R7 | cleanest surfaces-only; **FIXED** | 0 |
| Terminal Truth Laundering | R8 | not widespread after filter; systemic shape | 2 |
| **Split Commit / Atomicity Gaps** | **R1/R2/R8** | **cleanest — no durable gaps, 2-3 Low in-memory residuals** | **0** |

The through-line across all seven: the **durable/typed/authoritative core is well-governed**; residual violations are
leaves that drop a fault, re-derive a fact, or skip a window the surrounding system already models — and the strongest
durable invariant of all (atomicity) is the one the codebase protects most consistently.
