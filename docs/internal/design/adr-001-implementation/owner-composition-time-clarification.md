# Revision 7 clarification: time decisions and historical replies

Status: proposed implementation clarification, conditionally accepted by the
Meerkat/MobKit lead on 2026-10-01. The blocking acceptance conditions below
remain implementation and test obligations. This document does not close GL4,
establish a trusted clock, or enable governed execution.

This extends the frozen [owner composition proposal](owner-composition-proposal.md)
and [operational addendum](owner-composition-operational-addendum.md). It does
not change their rule that local committed entry precedes release or their
requirements for independent acknowledgment and honest uncertain outcomes.

## Decision point and durable acknowledgment

A witness Reserve or Finalize obtains a fresh, bounded time observation only
after the namespace and actual physical write fences. Its time predicate is
decided at that logical point, conditional on the exact canonical guards and
held-base CAS subsequently succeeding. The later durable reply describes the
recorded decision. It is historical evidence, not current execution permission.

A process pause after that observation can delay commit and acknowledgment.
There is no claim that an external clock read is atomic with physical fsync.
A post-commit expiry does not undo the committed fact, change its ordinal or
prove non-entry. Lost acknowledgment reconciles the exact existing request.

Every later deadline-guarded entry, grant promotion and physical release needs
its own authoritative fresh check. The earlier witness decision is an
admission filter; it cannot supply the later check. Known time uncertainty
and the declared maximum apply at every such boundary, not just at Reserve.
An unavailable, unknown or excessive bound refuses the new operation.

For each supported effect, the actual release owner must identify the final
check and its ordering relative to I/O. It must occur after all fallible
preparation, queueing and awaits that can stale an earlier observation. Any
further wait requires another check before release. No freely retained or
reconstructed permit may carry a historical time check to a later dispatch.
The physical transport contract must state its release linearization point;
a check made when merely creating an async future is insufficient evidence of
the ordering at its actual effect boundary. This requirement does not promise
network arrival or effect completion before a deadline.

## Required integration inventory

| Deadline-guarded class | Actual boundary to qualify and test |
| --- | --- |
| Tool dispatch | The concrete external, builtin, shell or MCP sink after the tool semaphore, preparation and other admission awaits. The generic policy wrapper's earlier result is insufficient. |
| Model attempt and compaction disclosure | The actual qualified adapter/transport release of exact prepared bytes and processor/account/route, separately for every retry, fallback and compaction request. The assistant message id is not the physical attempt identity. |
| Independent per-item anchoring | The actual authenticated witness request owner and the later client effect release. Batched replies identify every exact attempt; a successful old batch cannot authorize a new disclosure. |
| Grant promotion | The exact generated promotion join under the participating grant/control fences, followed by a fresh check at each later use. Historical finalized facts are not reinterpreted as a new issuance. |
| Protected audience release | The real first-byte delivery and subsequent protected replay/read, with current audience and complete source dependencies. An earlier turn admission does not authorize a new subscriber. |

These are required integration points, not claims that the current adapters
already implement them. A custom or unsupported adapter refuses the governed
profile until its actual release boundary passes qualification.

## Generated observation and cancellation

The existing generic atomic join accepts a fully constructed input before
opening physical storage. The proposed additive `apply_observing` entry point
opens and validates the real transaction first, then invokes an internal
input producer exactly once. It shares the canonical preparation, target
binding, commit classification and post-ack owner swaps with the existing
join. The existing `apply` error order remains unchanged.

The callback supplies data only. It confers no authentication, currentness,
clock-source trust or replay authority. It receives no mutable owner or raw
connection, cannot await or reenter custody, and must perform only the
selected bounded local observation work.

A rejected observation explicitly cancels the no-op transaction. Successful
rollback returns a typed observation refusal with unchanged owner state and
healthy custody. Failed cleanup retires the actual physical handle while
held and invalidates the join before releasing its namespace. It cannot
report a clean refusal while ignoring an unsuccessful rollback. Panics keep
the existing poison and retirement behavior.

A failed begin or CAS never queues an observed value for reuse. A new attempt
acquires new fences and obtains a fresh observation. No generic helper may
retry a captured input as though its time were still current.

## Blocking acceptance cases

1. Inject a pause between witness observation and commit, expire authority,
   then finish the historical commit. The actual tool/model/grant/release
   boundary must refuse the later effect with zero sink entry.
2. Fail begin and independently fail the held-base CAS. Retry through new
   custody with a new clock value and prove the earlier observation is never
   reused. An uncertain commit reconciles instead of being retried as absent.
3. Exercise observer refusal and every precommit cleanup branch against real
   SQLite. Successful cancellation releases locks and preserves exact rows;
   failed rollback retires the shared physical handle before any alias enters.
4. Refuse unknown uncertainty, a bound greater than 30 seconds, stale samples,
   clock discontinuity and unavailable selected source at the final dispatch
   boundary as well as Reserve/Finalize. Preserve the exact expiry boundary
   and conservative interval rules of revision 7.
5. Execute a fixture where the configured authenticated source is absent and
   time-bound authority refuses. The inspected GCP VM currently lacks the
   proposed managed authenticated profile; default unauthenticated metadata
   time does not satisfy this requirement.
6. Qualify every declared effect class at the actual sink. A generic callback
   test or a green witness backend cannot close missing adapter checks.

The lead also recommends a second observation after commit and before the
caller-facing reply. If already expired, return a typed expired-at-ack result
that refers to the committed historical outcome and grants no authority.
This is useful additional protection, not permission to omit the final
release check or to erase committed history. If this observation is unavailable,
the reply must remain explicit about historical outcome and lack of current
use authority.

## Evidence boundary

The lead's conditional design acceptance covers this logical distinction.
GL4 closes only when the required dispatch ordering, fresh retry observation,
uncertainty refusal, cleanup and written contract are all implemented and
independently tested. Mechanical generated observation work alone closes none
of the deployment time-source, enrollment/currentness or release-sink claims.
