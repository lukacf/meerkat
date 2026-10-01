# Owner composition for the first governed slice

Status: revision 7 implementation proposal, pending renewed review; not an accepted
or implemented runtime guarantee.
The [runtime tracer](runtime-tracer.md) establishes the existing owners and
missing associations. This document fixes the proposed handoffs for review
before changing those owners. It does not replace any ADR requirement.

## Admission and retained association

Canonical identity remains `meerkat_core::auth::PrincipalRef`. Qualification
distinguishes exact issuer/authority namespaces; it does not authenticate the
principal. An identity namespace is not an operation coordination domain.
The trusted composition declares the latter from canonical state ownership.

An ingress identity authority supplies an authenticated event and exact mapped
requester. The admission owner verifies that event, the grant lineage, the
message-resource classification and the intended context audience before any
protected input hydration, attachment custody transfer or transcript write.
Incoming wire claims alone cannot construct an admitted execution association.

The existing input admission transaction retains an immutable, non-secret
association separately from `persisted_input`, because the latter is retired
after consumption. Its identity covers the native `InputId`, exact input
digest, original requester, executing principal and binding, ingress event,
grant ancestors and original restrictions, profile and disclosure mode.
Immediate `InputOrigin` is retained independently. This record is admission
evidence, not a cached Allow or new work queue.

`MeerkatMachine` owns association selection for staged contributors. Every
contributor must be compatible before entering one model context. Core receives
an owner-issued association reference and its generated admission/entry handle
through the existing runtime binding bundle. It does not deserialize authority
from prompts, surface metadata, process-local tickets or opaque context strings.
Unknown legacy associations remain unavailable for governed execution.

Durable deduplication compares the authenticated association as well as the
native operation/payload identity. Reusing an idempotency key with another
requester, ceiling or audience is a conflict, with no old result disclosure.

## Final entry and participating authorities

The proposed initial local fence is an atomic transaction spanning the canonical
operation owner, its pre-entry receipt, and all same-domain authority state
needed for that attempt. Sharing a storage transaction does not transfer policy
or grant ownership: each participating owner defines its accepted generation
and the compare predicate applied in that transaction.

The first SQLite implementation will require participants that can join this
transaction. Existing custom policy providers need a declared adapter into
their canonical activation owner; taking a snapshot and copying its revision
into an authorization-owned table is forbidden. A participant with no atomic
join or proven reservation/handoff refuses governed bootstrap. This is an
explicit compatibility boundary, not permission to weaken the full ADR.

Pure slow preparation precedes this transaction. Protected preparation-side
reads, refreshes, mutations or disclosure require their own actual-owner entry
and custody before they occur, as specified below.
It compares the exact call/resource/binding digest, current grant ancestors,
policy/resource generations, owner incarnation and fencing generation. The
single entry commit consumes the exact attempt and commits its required
decision/intent evidence. Only generated owner feedback authorizes realization.
Cloning a preparation result or replaying an intent cannot enter another effect.

The composition fixes transaction/lock order and cancellation semantics. A
revocation committed before entry wins; a revocation after committed entry
cannot invent non-entry. The actual effect owner attempts cancellation where
supported and records its real outcome. The final tool gate follows consequence
preparation; a second late snapshot read does not implement this fence.

For an independently owned authority such as Elephant resource policy, a
separately authenticated protocol declares exact audience, request binding,
lease/freshness, revocation and replay limits. Elephant performs its own local
entry fence. A co-hosted Elephant remains independent for its own state.
A handle exposing Meerkat grant state remains a same-domain participant even
when transported elsewhere. No host- or adapter-chosen label bypasses the fence.

## Evidence and settlement

The receipt store is append-only evidence custody, never execution authority.
Stable append identity binds immutable content. Append outcomes distinguish
committed proof, definite non-commit and unresolved acknowledgment. A retry
with identical bytes reconciles that append; conflicting content refuses.

The existing operation owner retains the entered attempt and settlement duty.
For session work, generated per-attempt security obligations in
`MeerkatMachine` block dependent continuation and body release while required
proof is missing. They do not overwrite known effect truth. A known success
followed by a receipt failure remains success with pending evidence; recovery
retries evidence only. A crash that loses unpersisted outcome knowledge reports
Unknown and asks the actual destination/owner to reconcile.

Receipts may be physically group-committed while preserving every attempt and
obligation identity. Best-effort event projection, receipt export health and
console delivery cannot clear a mandatory obligation. Exact committed proof
must reach the generated owner transition.

Rollback resistance is not supplied by SQLite durability or a hash chain in the
same restored directory. Governed bootstrap needs a surviving current-authority
witness outside the restored application state, bound to the real deployment
and owner incarnation. An unavailable witness quarantines protected work. A
cloned fixture cannot become a second current owner by copying receipt files.

## Required model and sink witnesses before enabling the profile

The existing catalog/TLA+ pipeline must model revoke-before-entry,
entry-before-revoke, stale owner takeover, twenty independent attempts,
lost append acknowledgment, known effect with pending evidence, and restart
with unknown effect outcome. No independent security execution machine is
introduced. A new grant-validity owner may own issuance and revocation, but
cannot acquire session or effect lifecycle.

Production tests must observe the actual tool, provider, resource and delivery
sinks, including first bytes, helper output, history/replay and storage writes.
They must establish denied sink non-entry, durable cold restart, fresh context
reset and exact retained provenance. Pure contract tests are prerequisites only.

The feature remains unadvertised until the complete declared operation catalog,
bootstrap refusals, owner compositions, evidence stores and sink cases pass.
Optional or initially unavailable behavior must fail at a production boundary
before private preparation; omission from a test is not profile enforcement.

## Review repairs and explicit implementation commitments

The first lead review rejected the proposal as incomplete (O1-O7). The following
decisions narrow the implementation mechanism, not the ADR requirements. They
remain subject to executable race tests and the other product reviews.

### O1: Storage topology and joins

The initial native local profile requires the existing SQLite
`HeadCanonicalV1` runtime/session topology. `SqliteRuntimeStore::new_head_canonical`
already co-locates these canonical rows and performs explicit activation; the
general `new` constructor preserves `WholeBlobV1` compatibility and is not
sufficient. A matching path string alone does not prove a joined transaction.
The governed factory must validate the durable profile and participant identity.

| Owner | Initial governed storage and join |
| --- | --- |
| Meerkat input, runtime entry and session transcript | Head-canonical runtime/session rows in one realm SQLite database; extend the existing sealed prepared-boundary transaction to include the association, entry and mandatory receipt rows. |
| New grant-validity owner | Canonical grant/lineage/revocation rows in the same realm database; generated grant mutations and operation entry compare the same rows in that transaction. No side revision registry. |
| Existing in-process tool-policy provider | Its actual canonical generation read lock is held from final validation until commit/promotion. Activation holds that same owner's write lock. Governed activation persists its own content identity/epoch in the realm store; this is an explicit provider contract migration, not a shadow common-evaluator map. |
| Schedule and occurrence owners | Their canonical tables must be in the same realm database and expose owner-prepared transaction joins. Merely using `SqliteScheduleStore` through a separate async worker does not qualify. Migrate composition explicitly before enabling governed occurrences. |
| Approval lifecycle | Add a realm SQLite implementation of the existing approval store/owner contract and atomic exact-event consumption with receipt evidence. The current JSON file approval store refuses governed use. Legacy approvals missing exact event/digest bindings do not become governed grants by copying their rows. |
| Blob and artifact custody | Bytes may use an external custody backend, but protected staging, immutable digest and access envelope require an owner-authorized crash-safe handoff before linking into admitted work. Uncoordinated pre-admission blob writes refuse. This handoff still needs its detailed implementation and tests. |
| MobKit mob membership, identity/session binding and continuity heads | Migrate these canonical binding/head rows into the realm transaction through their existing owner-prepared mutation. Until the migration and exact native handoff are proven, governed spawn, fork, helper creation and affected continuity hydration refuse with operation/member scope. A prior read of a separate continuity database is not a fence. |
| MobKit schedule host | Join the same canonical schedule/occurrence transaction described above, including its MobKit mandate and target binding. The current separate schedule database refuses governed occurrences until migrated or a proven owner handoff replaces this requirement. |
| MobKit agent memory | Protected retrieval, hydration and mutation need current source dependency/envelope custody and durable evidence under an owner-prepared join. The initial separate memory database refuses governed memory operations until that join is proven. Disabling memory must be explicit in the declared operation catalog, never a silent fallback. |
| MobKit detached jobs | The actual job owner retains the causing association, launch/settlement identity and protected output through a proven prepared handoff into canonical runtime custody. A standalone job row or completion event does not authorize continuation. Governed detached work refuses until this handoff is implemented. |
| MobKit console and event stores | Event projection stays advisory and cannot attest entry. Protected console materialization, history/replay and delivery need current audience and source dependencies plus release evidence from their actual owner. Existing ungoverned console rows cannot be served by assuming the event database is a security receipt store; affected projections/refetches refuse until their governed read/release contract is installed. |
| Elephant resources | Independent resource authority, its own durable entry and current policy, with authenticated cross-domain request/evidence and declared lease bounds. No relocation-based ownership change. |
| Custom runtime/session/approval stores | Must implement and pass the same declared join/handoff contract before becoming governed-capable. Existing trait implementation alone does not qualify. Initial memory/JSONL/file stores refuse this profile. |

Browser/wasm remains trusted-embedded initially. The native governed feature
must reject browser activation explicitly, while normal wasm builds stay green.
Source or wire compatibility does not advertise runtime capability.

### O2: Durability and WAL policy

Every governed writer uses the existing `meerkat-sqlite` PRIMARY profile
(`journal_mode=WAL`, `synchronous=FULL`) and verifies those effective settings.
Neither `NORMAL` nor an in-memory test double proves pre-entry durability.
An exact physical group commit may release its attempts only after the shared
commit/fsync barrier succeeds. Benchmark these settings, not a faster profile.

Governed connections disable automatic foreground checkpointing. SQLite WAL
page-count notifications schedule bounded PASSIVE checkpoint work at 1,000
pages or after a one-shot one-second deadline for pending work. There is no
periodic idle tick. Maintenance cannot turn a busy reader into an unbounded
commit stall. WAL bytes count against the storage quota; inability to keep up applies
typed backpressure before exhaustion. Restart/checkpoint failures never discard
active evidence. These are implementation targets, not current measured results.

### O3: Non-ingress derivation

| Input/work cause | Association producer and rule | Receiving verification |
| --- | --- | --- |
| Human/API ingress | Declared ingress identity and message-resource authorities; root mandate and audience verified by admission. | Existing input admission owner, before protected preparation. |
| Peer request/comms | Sending comms owner hands off the causing work plus explicitly attenuated grant and exact destination. Peer key proves only the hop. | Receiving comms/admission owner verifies original association, grant lineage, target and current source/destination rights. |
| Mob spawn, fork and helper | Existing MobMachine/helper owner requests attenuation from grant authority and binds the child registration and context partition. | Child admission verifies native parent/child handoff and grant; no inheritance of unrelated executor privilege. |
| Detached completion and merge turn | Actual detached-effect owner retains the entered attempt association and produces its correlated settlement/handoff. | Runtime verifies native operation correlation and retained association; System origin does not mint a mandate. New model/use attempts reauthorize. |
| Schedule occurrence | Schedule owner derives from its explicitly issued, independently revocable mandate, preserving commissioning provenance, creator/target/scope and grant generation. | Occurrence plus target admission owners recheck that exact mandate and current policy. No grant inferred from schedule existence. |
| Compaction, retry and model continuation | Existing session owner preserves the contributing association and complete observed dependencies; each physical attempt is new. | Same session's current-use/entry owner; no origin-based service privilege or lost ancestor dependencies. |
| Service work without causal requester | Declared service authority issues an explicit independent mandate with bounded inputs, purpose, processors and release contract. | The actual operation owner validates this mandate; no default authority for background inputs. |

Legacy causal work with unavailable authority is refused explicitly. Newly
generated governed work must carry a verified owner derivation or fail before
effects; it cannot silently become a legacy/unavailable input and continue.

### O4: Rollback witness deployment

The supported implementation candidate is an authenticated independent witness
service with durable compare-and-swap state for deployment identity, current
owner incarnation, authority watermark and integrity anchor. Its reference
backend must have separate restore/backup custody and its own crash/replay
tests. Every governed owner uses its fencing proof; a copied local state
directory cannot register a concurrent owner with historical authority.

This task does not deploy a new service or select production credentials.
Homecore's owner identified an existing separate household host with separate
backup/restore custody as a viable deployment candidate. That is a topology
recommendation, not evidence that a witness is installed or accepted. Until a
supported authority is configured outside its restore/clone domain, Homecore
cannot activate governed mode. A same-host Elephant or separate local
directory is not sufficient merely because it has another process or path.
The implementation must include explicit refused bootstrap and outage tests.

### O5: Association custody and limits

The immutable admission association survives prompt-payload retirement. Its
minimum custody lasts until the latest of required receipt retention, scoped
idempotency conflict retention, and completion of all related execution/evidence
obligations. Session deletion alone cannot erase unresolved custody. Authorized
retention/deletion records its own evidence and may replace private historical
details with an integrity-bound protected archive reference under that contract.

The initial inline association limit is 16 KiB per admitted input. Larger
provenance uses immutable governed references whose complete custody bytes and
indexes count in the storage/memory budgets; references do not hide unbounded
storage. Exceeding supported bounds refuses admission. No credentials or
private input body belongs in this association.

### O6-O7: Lock order and generated handoff

The proposed final local commit order is: operation-owner locks in canonical
owner-key order; then participating canonical policy/resource generation locks
in canonical authority-key order; then the single SQLite write transaction.
No participant may acquire an earlier class while holding a later class. Slow
preparation and remote calls finish before acquisition. Failed comparison,
expiry, cancellation before entry or bounded database contention rolls back the
uncommitted draft and releases all locks in reverse order. Cancellation after
commit belongs to entered-effect settlement and does not undo entry.

The generated extension candidate is `EnterGovernedAttempt`, bound to the
existing native operation identity (tool call, model attempt or registered
`RegisterOp` operation as applicable), owner incarnation, physical-attempt
ordinal and exact preparation digest. It consumes that attempt only in the
owning MeerkatMachine state and emits typed entry feedback as part of the
durable prepared boundary. `RecordGovernedAttemptEvidence` clears only exact
per-attempt receipt obligations after verified commit. Existing effect
completion transitions retain execution truth. These are proposed new inputs,
not shipped names, and await the shared catalog ownership handoff and model
review. They must not create a second effect ticket/lifecycle.


### Product review: durable custody and recovery

The first Homecore and OB3 implementation reviews reject governed activation
on their current storage topologies. These are concrete compatibility limits
that bootstrap must detect before protected preparation. Existing trusted
embedded operation remains available under its explicit profile.

Durable operation state, immutable admission associations, grant authority,
entry records and pending evidence obligations must share a recoverable
canonical history. A durable transcript projection cannot substitute for
those rows. An in-memory mob owner plus ephemeral runtime database and a
separate durable analytics store does not meet this requirement. A custom
remote backend must prove the complete owner join and recovery contract; an
existing session/event-store implementation is insufficient.

Homecore currently rebuilds runtime state from a separate continuity store.
Governed rebuilding requires an owner-authorized migration that preserves the
exact admission associations, entry/settlement obligations, idempotency
conflicts and witness continuity. It must never silently reconstruct them
from conversation text. If those rows cannot be carried forward exactly,
restored protected work is quarantined. The reference recovery test starts
with three queued inputs and one entered attempt with unresolved settlement:
queued work re-admits by its original InputId and retained association; the
entered attempt remains Unknown with its receipt and is reconciled, not
replayed; a migration receipt continues the integrity chain.

A payload digest cannot recover the input bytes. The governed custody owner
must retain recoverable protected payload bytes or a verified immutable
reference for queued, abandoned-but-unconsumed and unresolved work until
consumption or separately authorized deletion. Retirement of prompt bodies
cannot erase this recovery right. Tests recover an abandoned input after a
one-hour simulated delay, prove exact byte identity, and prove consumed work
cannot be replayed through recovery. Re-admission uses the original InputId,
original association and a distinct authenticated recovery operator. Dispatch
of new text is new work even when its bytes happen to match.

### Product review: owner takeover and quarantine scope

Overlapping processes or pods must use a shared canonical owner-incarnation
CAS and current fencing authority, or the deployment refuses the profile.
A process-local SQLite lock or historical witness token is insufficient when
two pods can both reach an effect sink. An explicit two-process test keeps the
old process alive after takeover and proves that it cannot enter a new effect.

A copied production configuration must not advance the production witness.
Boot proves deployment identity, owner incarnation and the retained state head
before any witness mutation. Mismatch enters quarantine with read-only witness
inspection. Creating a new deployment, promoting a clone, and authorized
restore/re-witness are distinct authenticated operations, separately scoped
from data access. Their evidence binds the old/new state digest and new owner
incarnation. Missing local state never means permission to bootstrap a fresh
authority under the same deployment identity. Re-witness cannot turn an unknown
entered effect into permission to repeat it.

Quarantine follows the smallest canonical ownership unit whose proof is
missing. A missing member-local participant blocks that member's protected
work; it does not mark all other members corrupt or trigger repair loops.
A shared mandatory receipt or witness authority outage blocks the affected
whole deployment. An eighteen-member test removes one member-local authority
and proves that seventeen healthy members continue independently. Reserved
alarm work still needs a current independent mandate and durable evidence;
this design does not promise its availability during a shared authority outage.

A local watermark plus asynchronous remote reconciliation alone is not an
accepted outage protocol. It cannot establish current same-domain revocation
or exclude a restored competing owner. Any later alternative must prove the
ADR's surviving-authority and entry-fence invariants before it can be offered
as a supported profile. The initial independent-witness composition refuses
protected entry when it cannot obtain the required current proof.

### Product review: bounded contention and remote durability

Final entry uses bounded database contention. It distinguishes definite
non-commit (safe to prepare/retry the same attempt), committed entry, and
unresolved acknowledgment. A thirty-second competing writer must yield a
bounded typed non-commit/backpressure result without poisoning member health
or sending the runtime into repair-block. No protected effect can enter while
its entry result is unresolved. WAL and checkpoint bytes count toward the same
custody quota, including periods with long-lived readers.

New optional backend traits default to an explicit unsupported-governed
capability while preserving trusted embedded behavior for existing custom
stores. This default cannot manufacture a successful admission receipt.
Remote backends may group exact independent records into one physical commit,
but each effect waits for its own definitive durable pre-entry evidence.
Seconds-scale remote storage needs its own measured product budget. A local
SQLite benchmark neither certifies a remote analytics backend nor permits
post-effect evidence to masquerade as pre-entry durability.

### Status after review

O1-O7 dispositions have directional lead acceptance. Homecore H1-H6 and OB3
M1-M2/S1/N1-N2 are incorporated here for renewed review. This document is still
an implementation proposal. None of these dispositions is runtime acceptance,
a deployment authorization, a passing race test or an advertised capability.


### Second lead review: commit ordering and contention

Generated semantic drafts are computed under the existing owner's exclusive
transition custody without applying them to live state. The store commits the
exact generated next state, attempt consumption and evidence atomically. Only
a verified committed result permits application/promotion of that draft and
effect realization. Definite non-commit discards the draft without changing
the live owner state. Unresolved acknowledgment uses a generated
EntryUnresolved observation that blocks dependent work until the exact append
and owner boundary are reconciled. It is neither non-entry nor entry authority.
A crash after physical commit but before in-memory apply restores the committed
owner rows; it never repeats the effect by interpreting the missing in-memory
acknowledgment as failure. The model includes both this window and commit
failure after draft preparation. Proposed transition names remain provisional.

Canonical policy activation uses a writer-preferring lock or equivalent
owner-generated activation fence: once activation is pending, new entry
readers cannot overtake it. The acceptance bound is 100 ms p95 to acquire
activation custody under sixteen sustained dispatchers, excluding independent
slow policy preparation and accounting separately for fsync failures. Failure
to meet that bound rejects the advertised deployment budget. Entries already
committed preserve their real execution truth; pending activation does not
retroactively declare them absent.

The initial shared realm database is one writer across all members. Benchmark
sixteen simultaneous attempts per member across at least four concurrently
active members, and report p95 entry cost against the same 10 ms tool-entry
budget. Exact group commit is allowed, provided every attempt waits for its
own durable pre-entry proof. A failure requires a topology redesign or an
explicitly revised deployment contract; per-member databases cannot be added
without their own proven join and witness composition. Add a two-hour soak
with a long-lived console-style reader, recording WAL growth, checkpoint
progress, quota backpressure and latency on the minimum advertised hardware.

### Second Homecore review: offline carry-forward

Carry-forward is an offline operation that needs no successful runtime boot.
It first fences the old owner incarnation, opens the old store read-only under
the existing maintenance/exclusive-custody protocol, and extracts exact rows.
It verifies row integrity against the store's retained history and the
independent witness's committed state head before writing into a fresh store
under a separately authorized migration receipt. Data access permission alone
does not authorize that migration or witness update. The destination is not
published as current until the complete exact migration commits. A failed or
interrupted migration keeps the old and candidate states quarantined for
reconciliation; it cannot bootstrap an empty authority.

Existing WholeBlob installations first perform the supported topology migration
under trusted embedded operation, then prove head-canonical and complete
security custody before activating governed mode. An unreceipted reseed cannot
produce that activation proof. Already governed migrations preserve their
receipt/witness history and the original work associations. A fixture whose
runtime cannot boot must still carry three queued inputs and one entered,
unsettled attempt forward exactly. Invalid member-local rows quarantine that
member alone when shared integrity remains provable; a broken shared integrity
anchor affects every dependent owner.

The MobKit O1 rows are mandatory participants for their affected operations.
Fork/helper work, scheduled occurrences and memory-hydrated turns each must
enter under their proven joins or refuse before protected preparation with a
typed scoped result. Separate MobKit-side checks cannot be treated as having
participated in the final entry fence.


### Toolkit review: protected preparation and witness handoff

Preparation is not a privilege exemption. Pure argument construction, parsing
and hash computation can precede the final outer entry. A credential lookup,
credential refresh, private policy/attribute read, source hydration, remote
processor call or preparation-side write crosses its own protected boundary.
Its actual owner must first authorize that exact action under the causing
association, retain its required evidence and custody, and preserve resulting
dependencies. The outer tool's later decision neither grants that earlier use
nor retroactively audits it. Acceptance separately revokes before preparation
and between authorized preparation and outer entry; each relevant denied sink
must remain untouched. Credentials remain confined to their binding owner and
never enter the model or general receipt body.

### Revision 7: local entry, independent generation custody

Revision 4's per-entry remote reservation is withdrawn. It would serialize the
realm across two witness round trips and cannot satisfy the agreed concurrent
entry budget. This section replaces that protocol completely. The local entry
commit remains the single entry point specified in O1 and O6-O7 above.

The witness owns deployment registration, owner-generation custody, independent
integrity anchors and explicit authority-mutation reservation state. It does
not evaluate grants, copy policy revisions into an authorization registry, or
own effect execution. Its mutable state is a canonical generated owner with a
durable reference backend. Its authority-mutation head is distinct from each
append-only receipt journal anchor. The existing grant, policy, approval, runtime
and resource owners keep their own state and transitions.

#### Physical topology and stale-process exclusion

The initial native profile has one physically shared realm SQLite database on
one host. Every protected entry and every participating authority writer uses
the same canonical incarnation/fencing row inside that database's transaction.
A separate per-process database, per-pod disk, NFS-mounted SQLite file or a
remote analytics store does not qualify. Other transactional backends need a
separate proven composition and are initially refused. A remote witness CAS
cannot by itself fence a process still writing to another database copy.

The realm database and its stable maintenance-custody identity live outside
restorable application-generation directories. Deployment generation flips
carry the current canonical state forward; they do not select an older realm
file. Homecore's separate persistent shared directory is a possible location
for the complete realm database. Putting only an incarnation row in a second
SQLite database is insufficient: it would break the required atomic join.

The existing shared `OperationGuard` and exclusive maintenance helpers are
building blocks, not already a governed fence. Governed construction must
prove actual supported lock acquisition, stable physical database/lock
identity, and complete participating-writer inventory. A no-op guard on
lock-open/unsupported-lock failure is refused. Process-wide holder
self-admission must not admit unrelated concurrent runtime work under a
maintenance holder. Newly created databases cannot escape a fixed inventory.
An open handle to a removed/replaced database must remain fenced after the
maintenance lock is released; path equality or a copied database UUID is
insufficient. If the backend cannot establish these properties it refuses.

The supported initial restore procedure never replaces the active database
beneath a live runtime handle. It takes exclusive canonical maintenance custody,
quiesces operations, invalidates old runtime handles, proves their closure and
performs the authorized replacement/carry-forward. It then binds the new
physical store identity and incarnation before releasing custody. Failure to
prove handle invalidation/closure keeps the fence held or the deployment
quarantined. Tests must keep an old process and handle alive through takeover
and attempted restore; ordinary process-stop scripts do not prove this.

#### Independent OPEN state and bounded receipt anchoring

Before any local protected commit in a recovery unit, the witness durably marks
that exact deployment, physical-store binding, incarnation and journal OPEN
at its last authenticated anchor. OPEN survives a crash, restored local files
and loss of the response. Registration retries reconcile an immutable request
identity; absence of a response grants no authority. A copied fixture cannot
register a new production owner using a historical token.

Ordinary entries append locally with the existing owner transition and required
receipts under WAL/FULL. They compare current incarnation, active authority
head and absence of a pending control mutation in that same transaction. They
do not perform a remote reservation. Writer-preferring canonical activation
custody still prevents revocation from starving behind new entries. One
committed attempt produces one generated realization handle; evidence
reconciliation cannot produce a second handle.

Independent anchors are batched by canonical recovery unit. The reference
bound K is 64 unanchored receipt slots, enforced inside the local owner/append
transaction before consumption; concurrent groups cannot overshoot it. Entry
reserves capacity for its declared mandatory pre-entry and eventual settlement
evidence before release. Reserved unsettled slots count against K. Additional
protected sub-effects need their own capacity and entry. A receipt never gets
discarded or relabeled optional to keep the bound. At most 256 pending requests
await entry/evidence capacity; that queue does not confer entry and cannot
increase K. Known outcomes and receipt-only retry duties retain durable custody
in the existing operation owner during backpressure, with dependent release
blocked. Their reserved slots remain available for required late settlement. Anchors
are triggered at the 64-receipt boundary, by an authority mutation, by graceful
CLOSE, and when the actual owner's append/entry queue transitions to drained.
The drain event comes from the native owner state, not an independently polled
counter or console projection. There is no anchor timer or idle health ping.
These bounds remain measured targets, not current throughput claims.

Anchor requests bind the exact previous and proposed sequence/digest and
incarnation. Duplicate identical requests reconcile; divergent or regressed
heads refuse. A transport error or unresolved acknowledgment blocks new entry
for dependent units until the exact anchor reconciles successfully; it does
not permanently poison member health. An outage that has not yet been observed
cannot create more than 64 unanchored receipts. OPEN custody already covers
that potential tail. No age of a cached response grants permission, and no
periodic health lease participates in ordinary entry.

Witness transport retries reconcile the same immutable request, with a bounded
300 ms preparation/reconciliation budget and no retry of effects. Exhaustion
returns a typed retryable witness-unavailable result. A later authenticated
successful reconciliation clears that transport condition automatically. The
local entry/refusal budget remains in force: a blocked entry may return promptly
while owner-managed reconciliation continues. Reference product acceptance
includes 1% packet loss without user-visible refusals under the declared load,
a 10-second outage with bounded typed refusal once the anchor dependency is
reached, and recovery without an operator step. These availability goals may
fail measurement and must not be advertised until proven.

The end-to-end budgets remain 2 ms p50/10 ms p95 added tool-entry cost and
5 ms p50/25 ms p95 added model-entry cost at 1,000 dependencies, including
local fencing, evidence and contention. Independent witness cost is measured
in anchor throughput and backpressure, OPEN/CLOSE, and synchronous authority
mutation latency. Reports must include workloads crossing the 64-receipt bound,
authority/approval mutations, packet loss and idle-to-active calls. A warm
local-only microbenchmark cannot certify the full deployment.

A graceful shutdown first fences new entries and retires or joins every actual
append producer, including already-entered effect settlements and evidence-only
retry tasks. It resolves or records every pending append, anchors the exact final
head and obtains durable CLOSED-at-N. Merely draining the current queue is not
producer retirement. If unsettled producers cannot retire, leave OPEN. Any later
append requires a newly established OPEN before its commit.
CLOSED permits verification of that exact state, not replay of any entered
attempt. Loss of the close acknowledgment is reconciled by request identity.
No timeout alone proves that closure happened.

No portable software-only record can prove whole-host byte continuity after an
unclean shutdown: a snapshot may restore the database and every local custody
record together. The reference profile does not claim such proof. A surviving
separate custodian can be an additional deployment capability, but it cannot
be assumed and does not relax the common recovery rules.

On OPEN-without-CLOSE, let N be the last independently anchored receipt sequence
and H the largest locally verified sequence. Recovery records the witnessed
anchor N, the observed local head H, the enforced bound K=64, and a possible
unknown tail through N+K under that exact generation. H=N still carries this
uncertainty. Present records in (N,H] are retained as locally observed after
the gap; re-anchoring them now attests present bytes, not historical completeness.
Do not label them previously witnessed. Slots reserved for unknown settlements
are represented explicitly; the bound applies to actual enforced capacity,
not an optimistic batching target. H<N, a divergent anchored prefix or a
finalized control head with missing content blocks its dependent authority.

Before reopening work, recover the witness's exact finalized authority-mutation
head and the actual canonical grant, policy, approval and consumption contents.
A digest mismatch is detectable but cannot reconstruct missing content. The
reference durable handoff therefore needs independent custody of the exact
protected recovery content or an immutable retained source that can supply it.
If it is unavailable, that authority and its dependent operations remain
quarantined. A new InputId is not independent of a revoked grant.

The existing operation owner records the gap and classifies each surviving
attempt by its actual evidence. Missing records never establish non-entry.
Any recovered work whose non-entry would rely on absence in or after the possible
tail stays Unknown unless the destination can reconcile, or accepts the exact
original attempt identity under a proven idempotency contract. Recheck the
original endpoint, account, key, retention interval and authenticated association.
An expired idempotency key or an empty lookup is not proof of non-entry.
A tool's reversible/internal consequence label alone permits no replay. Current
authority is still required for any new entry or permitted recovery action.

Fresh work may proceed only with positively established current authority and
independent canonical dependencies. An unresolved effect fences new work whose
resource/consequence dependencies overlap it under the actual feature owner's
contract, even with a new InputId. Hydration cannot infer freshness from missing
history. If the lost scope cannot be reconstructed, quarantine the containing
owner unit; do not guess that a new request is unrelated. One member's proven
local uncertainty need not block seventeen independently proven members.

Input consumption at turn/context start is an absence-sensitive synchronous
handoff. It binds every consumed original InputId, association and recovery
payload custody before any model/tool work can proceed. Late steering or
co-contributor consumption must join the same rule. The witness preserves its
exact finalized consumption evidence and current native head. A queued input
may resume automatically only after authoritative reconciliation establishes
that its exact consumption did not finalize; absence in restored local rows is
insufficient. Recovery never reissues a previously consumed input as new work.
The one in-flight input can remain Unknown while proven queued inputs resume.

Externally retried ingress also needs synchronously preserved admission/dedup
custody before acknowledgment or protected processing. This includes external
event IDs, dashboard idempotency keys and flow run IDs. A new internal InputId
cannot turn a redelivered old event into fresh work. Without that surviving
custody, the affected dedup namespace is Unknown until reconciliation; current
wall-clock arrival time cannot prove first delivery. A native admission owner
owns this mapping and conflict comparison, not the witness or an audit exporter.

#### Effect release that needs an independent anchor

Irreversible external effects, external sends and disclosures require the exact
pre-entry receipt and consumed attempt identity to be independently anchored
before physical release. This includes actual model-processor disclosure,
protected delivery and other operations whose irreversible consequences cannot
be recovered solely from an incomplete local tail. The effect/resource owner
declares the complete behavior; a tool's coarse consequence label cannot hide
its private reads, disclosures or sub-effects. Optional stronger anchoring is
allowed for other operations, but does not create a new permission source.

The ordinary local commit remains the entry linearization point. The native
owner retains an entered-but-not-yet-released evidence obligation until the
exact independent anchor is acknowledged. Revocation after entry preserves
that ordering; declared expiry/maximum-realization-age rules may still cancel
physical release. Lost anchor acknowledgment blocks release pending exact
reconciliation. A crash in this interval leaves the native attempt entered and
possibly unresolved; an anchor receipt never reconstructs a new realization
handle or commands a retry. Actual destination idempotency/reconciliation remains
necessary to recover an uncertain external outcome, even with a surviving entry.

The independent anchor must retain enough exact protected entry/association
custody to reconcile the original native work and dependency scope after a local
restore. A bare hash with no retained content is insufficient. The witness is
custody for the native owner's immutable handoff, not a second effect executor.
For destinations without usable idempotency or reconciliation, an uncertain
outcome remains Unknown and is surfaced through an authorized projection. It
is never auto-replayed. Human intervention cannot infer a missing effect absent.

Synchronous input consumption, admission dedup, approvals and protected external
release carry real witness RTT/fsync cost. The earlier local tool-entry target
is not a claim that these composed paths cost 10 ms. Keep the original total
added-runtime target, report the added turn-start and release latency separately,
and measure the share of real turn operations in the synchronous class. A
profile that misses the full deployment budget is not accepted by hiding that
cost outside a timer. Explicitly changing a product budget requires a renewed
review before advertising support.

Recovery units follow existing canonical owner/journal boundaries. A missing
member-local journal can quarantine that member when shared authority integrity
is proven. Missing shared control or realm-incarnation custody quarantines all
owners that depend on it. One realm-wide journal cannot claim member-local
selectivity without additional independent evidence. Journal grouping does not
create a second lifecycle or split one physical authority transaction.

#### Synchronous authority mutations and approval consumption

Grant issuance/revocation, policy activation, approval consumption, input
consumption, protected ingress replay/dedup custody, issuer-trust changes, takeover
and restore promotion cannot acknowledge success based only
on a tail awaiting its first independent anchor. Their actual canonical owner
uses the following prepared handoff. The witness records exact mutation
identity/digest and authority head; it does not become their policy evaluator.

Pending-control exclusion is scoped by the mutating canonical authority's
exact dependency predicate. A grant mutation blocks entries using that grant
or its descendants; policy activation blocks entries depending on that policy
authority; approval consumption blocks only its exact event/attempt. Unrelated
member work continues. Incarnation/takeover and realm-wide issuer-trust changes
are shared exclusions. An authority that cannot express a complete safe scope
must declare a wider scope before activation and include its cost in acceptance;
it cannot guess a smaller set from current subscribers. Tests consume member
A's approval while member B continues entering, and revoke an intermediate
parent while every affected descendant entry is excluded.

| Step | Canonical owners and realm transaction | Witness | Entry status |
| --- | --- | --- | --- |
| Exclude | Persist a generated pending-control boundary under the actual owner's writer-preferring custody. It prevents affected entries and competing mutations; no live policy/grant change is applied. Release local locks before network I/O. | Unchanged. | Previously entered effects retain their real truth; affected new entries block. |
| Reserve | Bind exact prior/next canonical head, mutation, incarnation and receipt bytes. | Authenticated CAS creates one exact bounded reservation. | No mutation success and no affected new entry. |
| Commit draft | Reacquire ordered canonical custody, validate every exact input, deadline and current head, and commit the generated unpromoted draft plus receipt under WAL/FULL. Pending exclusion stays in force. | Reservation remains pending. | No promotion based on a draft or lost acknowledgment. |
| Finalize | Establish local durability, then submit the exact immutable mutation with no local database or generation lock held. | CAS finalizes only while reservation/incarnation/deadline are current; it retains exact finalization and new head. | Mutation is ordered, but entries remain excluded until exact local promotion. |
| Promote | Reconcile finalized proof, apply/promote the exact generated canonical draft atomically, record the new active head and clear pending exclusion. | Identical retry only reconciles the same mutation. | The mutation may acknowledge success; later entries compare this actual active state. |
| Abort/reconcile | Query the exact reservation when replies are lost. Discard an unpromoted draft only on authenticated terminal-abort evidence. | Abort-if-still-reserved and finalize race by CAS; one wins. Expiry may terminally abort a still-reserved candidate. Finalized candidates cannot abort. | Neither timeout nor missing local rows establishes non-finalization. |

Exclude can commit before a Reserve packet reaches the witness. Recovery must
not leave that local pending boundary wedged or treat an absent lookup as proof
that a delayed packet will never arrive. The witness supports an authenticated
terminal abort-or-fence for the exact immutable mutation identity and incarnation:
if absent, it durably records terminal refusal; if reserved, it wins or loses
against Finalize by CAS; if finalized, it returns that exact finalization.
Every later Reserve/draft/finalize for the terminally refused identity is denied.
The canonical owner clears/replaces exclusion only after that terminal result
or after completing the exact finalized mutation. Pending identity replacement
never authorizes a stale packet. Tombstone retention or an equivalent retired
incarnation/sequence floor must cover the entire possible replay lifetime.

The reservation has a declared monotonic duration at the witness and an
absolute authority deadline that can only shorten it. The candidate expires
rather than extending a grant or approval. Reference tests use a five-second
reservation limit and freeze the requester for 30-60 minutes: expired work
cannot finalize and the realm is not permanently wedged. Witness unavailability
still blocks reconciliation; expiry is not an unauthenticated local shortcut.
An authenticated abort can safely discard an unpromoted local draft even when
its local commit acknowledgment was lost, because finalized promotion is then
impossible. A lost finalize acknowledgment must query the terminal CAS result.

For approval-backed entry, the approval remains consumed only by its native
exact event/attempt binding. Witness finalization retains irreversible
consumption evidence; a crash before local entry promotion burns or reconciles
that approval, never makes it reusable. Promotion rechecks the entry's current
grant/resource/expiry predicates. If these no longer permit entry, preserve
approval consumption, record definite non-entry and require a newly authorized
attempt. A historical consumption receipt is not a permit to execute late.
The approval owner, not a generic receipt replay worker, performs this recovery.
It reports a required new approval through the authenticated channel used for
the original prompt; an audit projection cannot solicit or accept that approval.

Ordinary physical-attempt replay prevention uses the local canonical consumed
attempt row plus OPEN-generation gap quarantine. Any operation requiring
cross-generation single-use/replay protection beyond that conservative
quarantine must participate synchronously like approval consumption. It may
not advertise stronger replay guarantees from an asynchronously anchored tail.

#### Takeover, deployment rollback and time

A successor first gains the same physical canonical store's takeover/maintenance
custody and durably blocks old-incarnation entry. It resolves outstanding
control reservations, including Exclude-before-Reserve, by authenticated terminal
result, obtains surviving exact authority state and receipts, records/reconciles
OPEN history without inventing continuity, and binds a new incarnation through
the synchronous witness handoff. Only then does it reopen entries. Every old
process's subsequent entry transaction observes the newer fence and refuses.
A successor without shared or proven replicated predecessor custody cannot
promote. Remote takeover with isolated per-pod disk is initially unsupported.

Code rollback carries the current canonical security state forward to compatible
code. Reverting data to a historical generation is a separate authorized recovery
operation with gap/replay disposition and new incarnation evidence. Neither
reseed nor a successful old binary boot creates fresh authority. Clones use an
explicitly created test deployment and their own witness; production promotion
is separately authenticated and never inferred from copied configuration.

Absolute grant and token deadlines use a declared trusted-time source and
maximum skew. The reference contract allows at most 30 seconds of verified
skew and subtracts that bound when testing absolute expiration. Greater or
unknown skew refuses; tests exercise both +30 and -30 seconds and the exact
boundary. This conservative 30-second window makes an authority with 30 seconds
or less of remaining validity unusable; approval products must choose a TTL
that leaves their intended interaction window beyond that bound. Reservation
duration uses the witness monotonic clock, which cannot be resumed from a
serialized process clock value after reboot.
A wall-clock correction never extends an existing monotonic reservation.

Already entered effects retain honest settlement even if takeover or time
passes before physical realization. A feature owner may declare a stricter
maximum realization age, for example for a stale notification. It can cancel
that entered attempt before the actual sink, recording cancellation and its
receipt; it cannot relabel it as never entered or silently replay it later.

#### Required model and physical witnesses for revision 7

The canonical generated composition must cover: OPEN before any protected
commit; lost open/anchor/close replies; an unanchored consumed attempt followed
by restoration to exactly the last anchor; clean close and cold restart;
same-store SIGKILL and an indistinguishable in-window restore with the same
explicit gap handling; snapshot exactly at N and at N+10 after N+40; consumed
input versus proven queued input; externally redelivered event IDs; expired or
wrong-account destination idempotency; overlapping-resource fresh requests;
pre-send Exclude crash, delayed Reserve and terminal abort-fence; late settlement
producer during CLOSE; the K+1 capacity request with an unavailable witness;
control abort versus finalize in both orders; expired reservation after a
starved requester; revoke before/after local entry; draft commit before
promotion; two live processes and takeover; and invalidation of old handles
during restore. No fixture may assume away a missing tail or unavailable owner.

The implementation must demonstrate concurrent local entries while an unrelated
journal anchor is in flight, without a global remote single-flight gate.
Synchronous authority mutation exclusion and anchor backpressure/refusal cost
remain in the real latency measurements. The earlier multi-member, WAL-soak, outage,
minimum-hardware and evidence-size budgets still apply without relaxation.
