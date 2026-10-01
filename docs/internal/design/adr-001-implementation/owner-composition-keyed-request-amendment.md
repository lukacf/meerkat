# ADR-001 owner composition: keyed request amendment

Status: proposed implementation contract amendment, 2026-10-01. This revision
clarifies mutex scope, demand-driven recovery and phase binding after the lead,
Homecore, OB3 and Toolkit reviewed its predecessor. Review of these exact revised bytes remains pending.
No implementation or production-readiness acceptance follows from protocol
verdicts.

This amends [owner composition r7](owner-composition-proposal.md) and its
[operational addendum](owner-composition-operational-addendum.md). Their other
requirements remain in force. In particular, independent witness availability
remains mandatory for every governed model attempt. The original ADR and profile
documents retain their frozen design hashes.

## One atomic join and physical custody

The initial generated join profile is synchronous and uses one native database.
One poisoned custody mutex covers producer and target loading, exact base-state
validation, the physical transaction, installation of both accepted states, and
consistent reads. There is no await between durable commit acknowledgement and
the two live-state swaps. Both swaps are infallible `mem::swap` operations; old
states are retained until both complete. A panic between installations poisons
that composition's custody instance. Its future reads and operations refuse
until fresh custody reloads and verifies the actual durable owner rows.

This is a per-authority control-request composition, not a realm-wide registry
or lock. It covers Reserve, Abort, Finalize and their joined authority changes.
No remote call or wait for witness round trips occurs while holding this mutex.
Ordinary protected entry reads the active head inside its existing short realm
transaction and participating-authority fence; it does not acquire this control
join mutex. Input consumption and ingress dedup need their own reviewed hot-path
handoffs before governed turns are enabled. Underlying SQLite transactions still
have their declared physical writer-serialization and storage failure scope.

The witness namespace owner automatically retires the poisoned instance and
discards its live authorities. Recovery is demand-driven: the next control
request, a refused dependent protected entry, or an explicit
storage-owner-restored signal triggers one durable reload and re-verification
attempt under the existing 300 ms operation budget. Only
one recovery attempt may be in flight for that namespace. A failed or timed-out
attempt keeps that namespace quarantined until the next qualifying request or
restoration signal. There is no periodic retry loop or backoff timer, and no attempt clears
poison or trusts cached state. Successful verification installs fresh custody
without requiring operator approval or a process restart. If physical connection retirement or
storage integrity cannot be established, the declared storage scope remains
unavailable and the existing crash/recovery rules apply. Unrelated namespaces
must continue unless they depend on that same failed physical storage resource.
A quarantined namespace refuses dependent protected entry until verified
recovery. That refused entry triggers recovery without holding its entry
transaction or fence, then may retry entry only after recovery has completed.
Control requests need not arrive for recovery to progress. Concurrent triggers
join the existing single-flight attempt or return the typed unavailable result;
they do not start another attempt or spin.

Canonical DSL declarations mark joined inputs on both participating owners.
Ordinary public apply, batch, preparation and convenience paths cannot commit
those inputs. Generated provisional values are noncommittable outside the
composed API, bind the exact originating owner token and base state, and use a
sealed internal route. The canonical schema crate and its verified generator
remain trusted code; crate privacy is not a sandbox against that crate itself.

This mutex serializes every join and consistent read for the composition. It is
intended for low-rate authority mutations. A hot-path or multi-database profile
requires a new design and review.

## Exact row selection

A composition declares its keyed target family and checked route selector.
Producer effects determine the exact key and target input. An owner-provided
selector, missing selector, wrong key or inconsistent key type refuses before
persistence. The store loads only the selected request row; the head does not
carry a growing map of request history.

An existing-target route requires the exact existing row. `None` is not a fresh
request and cannot become an initial machine state. Recovery constructors and
row-key equality do not establish authenticated origin, currentness or complete
recovery. Those remain physical store and namespace custody obligations.

## Freshness belongs to the generated head

Request identity includes the authenticated namespace and a monotonic request
ordinal. The immutable body also binds the authenticated caller and exact
canonical mutation. Reserve, Abort and Finalize are separately authenticated
phase commands over that same immutable mutation binding; hashing their whole
phase envelopes as different mutation bodies would incorrectly reject legitimate
progression. Every authority-relevant mutation field remains bound. The request ordinal and the authority's semantic revision are
different facts. A first Abort consumes an ordinal without advancing the
authority's semantic revision.

ReadHead may advertise the next ordinal, but that observation grants no
permission. Before transmitting a control request, its existing operation owner
durably fixes the exact namespace, ordinal, caller and body binding. An uncertain
outcome never permits silently selecting another ordinal.

The canonical head transition applies these rules under the custody mutex:

- A positive ordinal at or below the allocated watermark selects an existing
  row. A missing row above the retained floor is an integrity fault. It never
  means that the request did not happen.
- Exactly watermark plus one may create a row, and only for a first Reserve or
  first Abort. All namespace, caller, body, current-head and domain guards must
  pass. The accepted head transition emits the exact creation key/input and
  advances the watermark. The selected row transition and head changes commit
  in the same transaction.
- Physical absence is an INSERT precondition after generated freshness exists,
  not a source of freshness. An already occupied purportedly fresh slot is an
  integrity conflict. Failed guards, insert or compare-and-swap promote neither
  owner.
- First Abort creates an exact Aborted tombstone. A delayed Reserve at that
  ordinal sees the tombstone and refuses. Finalize never creates a row.
- Exact retries return the existing bound result. Another caller or body at the
  same ordinal receives an authenticated identity conflict. Concurrent clients
  racing the advertised ordinal have one winner. A loser may select a new
  ordinal only after definitive conflict or terminal resolution, never timeout.
- Zero, skipped future ordinals and exhausted counters refuse. No watermark
  reset or ordinal reuse is allowed within a namespace. Retirement closes that
  namespace permanently. Recommissioning requires a distinct authenticated
  namespace and the existing independent activation/custody proof.

The watermark, retirement state and retained floor require the same currentness
and antirollback custody as the witness head. A well-formed restored snapshot
does not satisfy that requirement. Unavailable currentness means unavailable
creation, not permission to initialize rows again.

## Quota, restrictive controls and retention

Ordinary requests cannot consume capacity reserved for restrictive controls.
Reserve eligibility comes from authenticated evidence of the exact canonical
restrictive mutation, such as a generated grant revocation. A caller-supplied
action name, priority, Boolean or quota class is insufficient. Crossing the
configured high-water mark emits a typed operator-visible alarm.

The reserved capacity is finite. If it too is exhausted, the authority must
quarantine or stop new protected work and expose the alarm. This is not a
perpetual-revocation or notification-delivery guarantee. The existing bounded
settlement reservations and recovery-payload custody requirements still apply;
ordinary admission cannot spend capacity already owed to entered work.

Production use requires implemented and reviewed compaction or archival with a
retained rejection floor, not indefinite growth until quota exhaustion. The
floor and archive integrity commit with their canonical owner state. A covered
ordinal can never initialize or replay. The floor proves that rejection only;
it does not prove that every pruned request was Aborted, Finalized or successful.
Unavailable historical content returns a typed pruned/unavailable result. It
must not clear an unknown outcome or manufacture NoEffect.

Pending or unresolved requests and content required for current recovery remain
pinned. Compaction cannot delete them to make progress. If pinning prevents safe
reclamation, the quota and refusal contract still applies. Restore must preserve
watermark, floor, quota accounting, namespace retirement and required recovery
content together. Reanchoring cannot prove completeness of a lost past tail.

## Required evidence

The generic generated model uses two or three bounded keys, quantifies target
invariants over keys, applies a route at its exact selected key, and leaves all
other rows unchanged. Model samples do not limit runtime keys. Out-of-domain
scripted keys refuse; no default-row fallback is allowed. Expected target states
name their exact row key. Single-row success and TLC exit zero without reached
transitions do not prove isolation.

Both generated-model and real-store evidence must cover:

1. First Abort A, delayed Reserve A, and later Reserve B, preserving A's tombstone.
2. Pending A with Abort B, and restart/retire of A without changing B's result.
3. Missing allocated row, old-watermark restore, wrong loaded key and skipped
   future ordinal, with no target initialization or partial head update.
4. Committed first Reserve with lost acknowledgement, identical retry, conflicting
   body retry and two contenders in both orders. Successful Reserve-to-Finalize
   and Reserve-to-Abort traces retain the same mutation body while authenticating
   each phase separately. No retry allocates another ID.
5. Target guard refusal, transaction rollback, cancellation at every await and
   panic between swaps. No readable mixed producer/target state remains.
   Demonstrate poison, a request-triggered failed recovery, no further attempts
   without requests or restoration signals, then verified recovery on the next
   request. Repeat with only ordinary dependent entries arriving: their refusal
   triggers recovery and they can enter after verified recovery without an
   intervening control request. An unrelated member continues protected work while another control
   composition is poisoned or waiting for an injected long witness round trip.
   No realm-wide semantic lock is added.
6. Ordinary quota exhaustion followed by successful canonical revocation and
   typed refusal of another ordinary request; then reserved-capacity exhaustion
   with quarantine and alarm.
7. Compaction of eligible terminal records, rejection below the retained floor,
   honest unavailability of pruned outcomes, pinning of unresolved/recovery
   content, and restore of the complete coupled state.

These are acceptance obligations. The current generic join fixture, pure
contracts, retained SQLite mechanics and protocol reviews do not close them.
