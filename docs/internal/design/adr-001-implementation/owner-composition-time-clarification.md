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

A fresh observation is a local monotonic clock read now, combined with the
most recent authenticated synchronization sample under the selected clock
profile. Its conservative uncertainty grows by the declared drift bound and
elapsed time since that sample. The producer refuses a sample older than its
validity or an uncertainty above 30 seconds. The profile must specify clock
suspension/discontinuity behavior and bind samples to the actual clock session;
serialized ticks or samples cannot silently establish freshness after reboot.
This is a measured sample with explicit validity, not a cached healthy flag.

Sample refresh is demand-driven when a check finds it stale, outside the
namespace and database fences. There is no background polling loop or clock
IPC round trip on every dispatch. After refresh, a new decision reacquires
its fences and reads the local clock again. The full path still must meet the
agreed dispatch budgets: tool dispatch p50 2 ms / p95 10 ms and model attempt
p50 5 ms / p95 25 ms. These are acceptance targets, not measured performance.

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

## Eligible time sources and client clocks

The selected profile may use a deployment-controlled authenticated
synchronization source at the witness, or a fresh authenticated witness-time
response at a client. The latter avoids requiring a second authenticated NTP
service on every client host. An authenticated response alone does not prove
correct UTC: the witness must itself produce a bounded trusted-time interval
under its declared and actually qualified clock-source profile.

The client binds a fresh challenge, exact witness identity and time epoch, and
brackets send/receive with its actual suspend-aware monotonic clock. For a
witness interval `[earliest, latest]` observed while handling that challenge,
a conservative receive interval is `[earliest, latest + measured_RTT]`, widened
for declared measurement/drift bounds. A midpoint representation may use half
RTT only when its center is shifted consistently. Raw witness time with only
half RTT uncertainty is not sufficient; no symmetric-network assumption is
made. Oversized RTT, unknown source bound, stale/replayed response, unexpected
epoch, clock discontinuity or arithmetic overflow refuses.

Subsequent local dispatch observations advance and widen that interval from
local monotonic reads under its finite validity. Refresh stays demand-driven
outside fences. Restart invalidates the sample and any serialized clock ticks.
The profile must explicitly handle suspend; a clock that ignores sleep cannot
silently preserve sample validity. Witness availability without bounded trusted
time is insufficient. These are eligible profile contracts awaiting real
producers and fixtures, not a claim that the current service supplies them.

All authority deadlines and schedule mandate windows use absolute Unix/UTC
instants on the declared time scale. TTLs and maximum realization age use
checked absolute deadline arithmetic or monotonic durations. Local wall time,
time zone labels and DST folds cannot duplicate an occurrence's authority or
extend its window. A schedule's exact canonical occurrence and UTC instant
select one window; an ambiguous local display string is not that identity.

## Expiry after input consumption

If authority expires after durable input consumption but before release, the
native turn/operation owner records a typed `ExpiredBeforeRelease` outcome and
zero sink entry for that attempted release. The authorized projection surfaces
the refusal, and H2 recovery-payload custody remains retained under its actual
retention obligations. The consumed work cannot disappear as a silent no-op.

The exact retained outcome may support owner-authorized re-admission under
fresh current authority, preserving the original InputId lineage and recovery
payload reference. This is a new explicitly authorized admission, not mutation
of the old immutable association or automatic replay of an effect. A new native
identity refers back to the original work. If any earlier attempt may have
realized or has an unknown outcome, its actual owner/destination must reconcile
before the re-admission protocol may authorize a duplicate effect. Fresh
credentials or a timer callback alone cannot confer re-admission authority.

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
selected bounded local observation work. It reads the valid synchronization
sample and local monotonic clock described above, with no blocking I/O while
the physical write fence is held. Its execution bound counts against lock-hold
time. If the producer cannot answer within that bound, it refuses through the
same explicit cancellation path; it never refreshes or retries inside the held
transaction. A timeout or later cancellation cannot prove a synchronous commit
failed to occur; uncertain commit still follows exact reconciliation.

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
7. With client OS NTP unavailable, a qualified witness-time profile may supply
   an interval within the declared bound. Test actual authenticated responses,
   asymmetric delay bounds, stale/replayed challenges, suspension, restart,
   absent witness trusted time and unavailable witness. Refusals are typed;
   neither reachability nor signature validity substitutes for a time bound.
8. Consume input, stall past authority expiry, then resume. Assert zero sink
   entry, visible authorized ExpiredBeforeRelease, retained H2 custody and
   explicit fresh re-admission with original lineage. An uncertain previous
   effect must reconcile and cannot auto-replay.
9. Exercise both sides of a local DST fold through the same canonical schedule
   owner. Each exact UTC occurrence has one distinct fixed authority window;
   rendering it twice or changing the display zone never mints or extends it.

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
