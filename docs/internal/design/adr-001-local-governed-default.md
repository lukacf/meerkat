# ADR-001 amendment: a local governed default

## Status and reason

Candidate r4, 2026-10-01. One irreducible-request disposition below
is explicitly pending Luka's clarification; no new scheduler path is selected.
Luka has approved replacing the previous first-profile requirements with a
simpler default. The detailed contract below
is undergoing adversarial review. It is a design decision, not an implementation
or performance claim.

This amendment supersedes the default/first-profile requirements in
[ADR-001](adr-001-runtime-security.md),
[governed profiles](governed-deployment-profiles.md) and the
[implementation acceptance plan](adr-001-implementation/acceptance-plan.md)
where they conflict. Earlier review evidence remains historical evidence for
its exact scope. It does not establish acceptance of this amendment.

The default must make fine-grained authorization routine across Meerkat,
MobKit and Elephant without introducing a second execution system. The previous
first profile made remote witness availability, authenticated time, special
transport ownership and uncertain-call recovery prerequisites for ordinary
work. Its complexity, latency and exclusions are unacceptable for the default.

## Decision and threat model

Introduce `local-governed-v1` as the first governed profile. The existing
`trusted-embedded` profile remains explicit and isolated. A governed operation
never silently falls back to trusted-embedded after refusal or configuration
failure. Host administration selects the profile; request bodies do not.

The host process, OS, storage, clock, configured authentication issuers, resource
adapters and provider/tool implementations are trusted. The profile protects
against unauthorized users, agents and service callers; confused-deputy use;
cross-principal batching; unauthorized tool actions; and disclosure to the wrong
processor or audience. Prompts, model output and caller-supplied identity or
resource attributes cannot grant authority. It is not a sandbox for malicious
native plugins or a defense against compromised host administrators, restored
host images, forged host time or malicious trusted adapters. It cannot make an
external processor forget information already disclosed.

All profiles and execution modes, including every provider, streaming, tools,
compaction, memory, comms and live/voice, are in the delivery scope. A slim profile
is only a smoke-test fixture, never the end state. Enforce at shared semantic
boundaries, with typed provider/source observations where needed. A particular unknown destination,
unsupported resource contract or unlabelled protected dependency refuses the
particular operation. Refusing entire existing execution modes is not the
implementation strategy. Built-in routes must satisfy this contract before
delivery acceptance. Extensions use an explicit conservative tool/server contract
or supply trusted metadata for finer resource and destination decisions.

There are no additional network round trips or fsyncs on admission, tool,
model or output hot paths. Existing authentication handshakes, requested tool
or source calls and runtime storage commits remain. Policy administration,
connection setup and asynchronous control-plane distribution are outside those
hot paths and have separate availability and startup measurements.

Deployment policy defines protected source classes and supplies explicit default
envelopes for ordinary messages, email, sensors and connector results. Absence of
a label is neither public access nor automatic rejection of every input. The
configured source authority assigns a versioned, ingress/source-scoped envelope
before use; callers cannot choose it. Defaults never erase known restrictions
or implicitly adopt legacy data. A bounded classifier may propose only outcomes expressly authorized
by its scoped mandate, with isolated per-item context. Broader outcomes are an
explicit trust/release choice, not proof of classification accuracy.

Existing histories and stores have an adoption path. An applicable resource
policy authority can adopt an immutable enumerated legacy bundle under a stated
processor, audience, retention and revocation contract that explicitly covers
unknown internal provenance. Known source restrictions still bind; adoption
invents neither historical requester identities nor permission to replay old
work. Host administration installs this policy but storage custody alone does
not confer release authority. Record the bundle/version and adoption authority;
new bytes need their own envelope. The non-witness classification and adoption
semantics in the profile companion remain applicable. Test adoption on real-sized
histories; an unadopted partition must not disable unrelated members or work.

## One association and one authority

The existing native `InputId` row owns an immutable association: qualified
requester, authenticated ingress actor, logical executor/target, original work
and authentication references, grant lineage, restriction ceiling, source
provenance and the host-selected contract. A fresh credential or restart does
not rewrite original provenance. Authentication is performed by shared ingress
adapters, using the existing identity contract; provider API credentials are a
separate authentication domain.

Persist the association with its existing input/work owner and ordinary
transaction, and decisions with their native operation. Memory, schedules,
connectors and console history retain references/envelopes in their own existing
records; they need not all move into runtime SQLite. An in-memory work item's
association lives and dies with that item. Restore/reseed must preserve the
association or perform explicitly authorized fresh admission; a dashboard or
message replay cannot impersonate the lost original. Do not add a side ledger,
requester registry or independently mutable permission map. Exact qualified idempotency scope and exact association/content comparison
survive replay. Generated native batching preserves order and never mixes
qualified requesters.
The native owner compares the actual requester, realm, logical executor, target
and context generation, conjoins every contributor ceiling and retains each
complete original association. Different original event/authentication references
do not themselves forbid otherwise compatible work. Incompatible contributors
remain separate; no union of permissions or loss of provenance is allowed.
Delegation can only narrow the parent's effective
permissions; it cannot substitute the tool host's service account for the
requester. Existing canonical grant/policy owners remain authoritative. Scheduled
and connector work can use a host-issued scoped service mandate, retained by the
actual schedule/connector owner with its commissioning actor and bound to each
occurrence. Each occurrence checks current authority/deadlines; credentials and
timers confer no authority themselves. No human requester is invented or required.

Evaluate current policy and the canonical restriction algebra at admission and
turn/context preparation. Produce an immutable, private compiled decision for
that exact work and context. It holds indexed action/resource/processor/audience
bounds, the earliest validity deadline and a coherent current owner/version
snapshot. Correlated policy predicates remain intact: indexes are ceilings and
lookup aids, never a Cartesian product that invents new permitted combinations.
It is a disposable projection of canonical owners, not a permission authority
or a serializable bearer token. A new tool target, source, destination, provider
fallback or changed context must be evaluated and bound before use. An earlier
turn decision does not bless arbitrary future arguments.

Each enforcement point performs a constant-time final check of that exact
prepared decision, current local generation and deadline. Work that resolves
new attributes or builds a changed context is counted in the authorization
cost; moving it into preparation does not hide its overhead. Context dependency
aggregation uses the existing preparation pass. Partition context and compaction
before inference by compatible source/processor/reader restrictions: retain restricted segments
separately so permitted shared context remains usable. Within each derived item,
retain the complete union of contributing restrictions through summaries, tool
results and memory. A summary or model label never declassifies its sources. Splitting one mixed-context
response afterward does not establish independent provenance.

Build each model request as an authorized context projection. Omit disallowed
items and their complete transitive data/control-derived closure before
inference, then continue with eligible context. Retained artifacts keep their
envelopes; removing a citation does not sanitize a summary, argument or later
message. Unknown closure excludes the whole uncertain artifact. Use a fixed
typed withheld marker only under an explicit audience-safe contract, including
whether existence/count may be revealed. Reset provider-held context/cache that
contains excluded dependencies. Preserve the original input and association;
projection cannot silently drop or reinterpret the request to manufacture an
eligible call.

The checked local generation covers every depended-on local fact: policy, grant
and ancestor validity, identity relationships, resource classification and
route/audience binding. Their canonical owners publish changes and invalidate
decisions under one coherent local ordering contract. The derived invalidation
stamp owns no policy; stale decisions cannot be retagged as current. Restart
discards decisions under a fresh host incarnation. Other processes must share
that ordering or advertise a bounded distribution contract, not instantaneous
revocation. No subscriber or console cache decides local currentness. A final
authorization check, after relevant awaits, is the local linearization point: work authorized before a concurrent revoke may already be
in flight; later checks must see the new generation and recompute or refuse.
Do not hold a policy lock across network or tool execution. Streaming checks at
bounded publication units stop future delivery after local revocation is
observed; already delivered bytes cannot be recalled.

## Enforcement points and information flow

| Shared boundary | Required decision and attribution |
| --- | --- |
| Ingress and native admission | Authenticate the real actor, establish requester/mandate, authorize target and operation, bind immutable association and qualified replay identity. Console filtering is only a projection. |
| Tool and source dispatch | Authorize the exact tool action, resolved resource attributes and destination. Bind requester/executor and source restrictions. Built-in, MCP and host tools share this check; visibility is not invocation permission. |
| Model context/request construction | Authorize the complete context dependency union for the actual provider/account/processor and request semantics, including hosted tools and caches. Cover main turns, direct compaction, curator, web search, image generation, fallback and live sessions. Existing adapters supply typed facts; no replacement HTTP transport. |
| Comms, delegation and schedules | Authorize send/spawn/execute and recipient/audience; carry the original association and narrowed mandate. A timer or agent address does not create authority. |
| Output, stream and live publication | Carry authenticated subscriber identity through every surface, including replay and WASM raw polling. Gate each recipient and bounded text/audio/event unit against the envelope and current generation/deadline, including audio dequeue/write. Internal broadcast taps are not public authorization. |
| Persistent memory, blobs, history, export and audit | Retain source envelopes with writes and authorize every reader/retrieval before hydration. Owner-scoped memory is not automatically readable by all requesters. IDs confer no access; diagnostics exclude raw protected fields by default. |

Ordinary MCP tools need no new per-resource protocol to function: policy may
explicitly authorize a named tool and its actual server/account/credential scope
as one conservative unit, with
result envelopes inherited from inputs and the configured source contract.
Resource-specific rules require trusted host mappings or tool descriptors; a
missing mapping refuses that call. Argument-selected processors/destinations
still require resolution unless expressly covered by that unit grant. A server
using its own broad credential is
inside that declared trust boundary, not evidence of per-resource enforcement.
Provider-hosted tools likewise require their actual action/destination contract;
protected retrieval must have its source envelope or trusted source enforcement
bound before provider consumption, not merely a label on the returned result.

Deduplicated blob bytes keep each reference's envelope; sharing content identity
never selects the least restrictive reference. An output envelope conservatively
inherits all contributing source restrictions.
Explicit declassification requires current feature-specific authority and a
trusted transformation contract. Tools or provider-native transformations may
introduce new dependencies; apply their envelopes before publishing resulting
content or including it in a later request. A live conversation retains these
same facts for its evolving context; context changes invalidate its compiled
decision before the next affected disclosure. The underlying live/turn owner
continues to own cancellation, completion and recovery. If a provider retains
context that is no longer eligible, use the ordinary fresh provider-context path
with permitted segments; do not terminate the native run/session or leave live
output permanently silent. A room/voice device uses an explicit physical-audience
policy, not an invented authenticated human. Dynamic channels declare retention
and future-reader access or an explicit broader release policy. Send-time
membership alone is insufficient; monitoring copies are separate disclosures.

Elephant remains the authority for its resources and ABAC attributes. Its normal
query/fetch response supplies resource identity, policy/envelope version and
restriction metadata under the existing authenticated connection. Meerkat
combines those source restrictions with work authority. Meerkat policy cannot
widen Elephant access, and an Elephant reader permission is not permission to
send the data to every model or recipient. Shared principal/resource contracts
and conformance vectors connect the systems; neither imports the other's
internal policy database.

Remote revocation is not magically instantaneous without communication. For
retained remote information, use an explicit source-issued policy lease with an
agreed, advertised staleness bound. Authenticated invalidation, when available,
can shorten it. Disconnect alone does not invalidate an unexpired lease. Expiry
refuses only dependent operations until normal source access or control-plane
refresh supplies current metadata. Derivation, caching and reconnect never
renew a lease; only its source authority can do that. No extra
synchronous validation round trip is inserted into every dispatch. Local
revocation takes effect at the next local check; remote staleness is a distinct,
documented guarantee, not a local generation counter pretending to be remote
truth.

Ordinary Slack, Gmail or device APIs need not implement Elephant's protocol.
Under an explicit source contract, a declared trusted adapter/local policy
owner can attest local classification, retained-copy use and freshness bounds.
It cannot invent a vendor-issued lease or claim stronger remote ACL currentness
than authenticated observations establish. If remote-current access is required,
that remote authority's accepted lease/observation is necessary. Remote-issued
restrictions keep their own issuer/expiry and cannot be renewed locally. A local
retained-copy contract must state its separate use, revocation and deletion
semantics; a TTL, cache hit or reconnect is not new permission.

## Time, failures, retries and audit

Use the host clock with configured bounded leeway for expiry, following the
same host-trusted model as bearer credentials. Compute the effective deadline
as the minimum of the individually permitted grant/source deadlines, including
only leeway each issuer allowed. A deployment cannot extend an Elephant lease
unilaterally. Leeway is explicit in policy, tests and audit and is applied once. Clock
rollback protection and authenticated time are outside this default profile.

A stale or expired compiled decision recomputes once from current local facts.
An actual denial, expired source lease or unavailable required metadata produces
`OperationRefused`, with a sanitized reason and the affected native operation
identity. This is a normal operation outcome, never `FatalFailure`, `RunErrored`,
turn cancellation, session hold, teardown or a poisoned owner. Invalidate only
the decision, release only its operation resources and continue other eligible
work in the current turn. Tool refusals return as typed ordinary tool results alongside executable
sibling results so the agent can adapt. Admission refuses only that submitted work; output
refusal affects only the recipient/publication unit. Memory read/write,
compaction and comms refusals use their operation-specific typed results. A
refused subscriber receives an audience-safe notice while others continue;
notices themselves have a fixed authorized disclosure contract.

If projection cannot produce an authorized model request, first schedule an
independently authorized context, route or other ready operation. Never erase
source restrictions, re-use a tainted summary or escalate permissions. Identical
denied bindings are not retried without a relevant change. Real process,
provider or storage failure retains its existing behavior; governance does not
widen an operation denial into one.

**One disposition is pending Luka's direct clarification:** no permitted
processor can process the original current request at all. The smaller option
returns a typed, audience-safe request-refusal outcome, releases that request's
run slot and lets queued work proceed, without claiming the task succeeded.
The strict non-completion option preserves the run/input/turn and payload, gives
waiters a visible nonterminal refusal, and still lets independent queued work
proceed. The current owner has one run slot per session; the latter would require
an explicit native scheduling extension. Simply adding `AwaitingWork`, keeping
that slot occupied or reusing `WaitingForOps` would violate the no-session-hold
requirement. Neither option is selected or implemented here. Do not silently
convert queued work to steer, merge incompatible requesters or introduce a
security latch while this decision is pending.

An uncertain disclosure-only model call is audited and may retry automatically
after fresh authorization under existing bounded retry/backoff policy. Repeated
disclosure of the same bytes to the same processor is acceptable while authorized.
Build the next attempt from the canonical current transcript and observed outcomes,
retain its predecessor/Unknown link, and never replay old tool effects or append
a restarted stream prefix as if it were new continuation output. Deduplicate or
explicitly present a replacement attempt through existing output semantics.

Classify the actual effect: a provider-hosted mutation inside a model request
obeys the same existing idempotency/reconciliation contract as a mutating tool.
No blind retry of payments, actuator commands or other uncertain mutations is
authorized. Any required reconciliation belongs to that one operation; it cannot
stop the turn or session. There is no parallel security retry machine or
human-only default recovery gate.

Record the requester/actor/executor, work and parent operation, policy version,
source/processor/audience bindings, decision/reason, start and observed outcome
through the native operation/event owner. Authoritative audit batches join the
next existing native commit boundary, with existing retention/access controls;
there is no hidden per-dispatch commit/fsync. An in-memory embedder names the
actual existing durable host commit it joins (such as ordinary per-turn session
persistence), or explicitly declares process-lifetime audit. Neither a native
in-memory commit nor an optional exporter manufactures durability. Accepted
pending records cannot
silently drop on backpressure or clean close. Cover denials, delegation, reads,
compaction, model/tool calls, comms and publication. Known authoritative staging
failure refuses the affected new protected operation; native transaction failure
keeps its ordinary scope. Never retroactively relabel a completed effect failed.

Optional projections/exporters fail observably without blocking execution. A
silently dropping queue cannot be the sole audit authority: in particular,
MobKit's current `EventLogHandle::ingest` ignores `try_send` failure
([issue 510](https://github.com/lukacf/meerkat-mobkit/issues/510)). Stall/overflow
that path in acceptance tests and prove native audit survives and export loss is
observable; export completeness is distinct from native audit.

This is host-trusted semantic audit, not a crash-complete external proof. With
no additional durable pre-effect barrier, a crash can lose the most recent
uncommitted attempt details. Recovery marks the unfinished native work as an
uncertain audit tail; it cannot assert the exact set of external effects or
claim that missing records prove non-execution. Operators who require externally
witnessed, rollback-resistant or complete pre-effect evidence need a separately selected future high-assurance profile.

## Acceptance and implementation order

The added authorization cost must be below **1 ms p99 per tool dispatch and
per model call**, and at most **10 percent per representative turn**. Include
policy matching, metadata resolution, context aggregation, generation misses,
recording and allocation attributable to authorization. Count any extra model
calls/tokens from partitioned compaction against the turn budget; they are not
free preparation work. Report warm hits, policy invalidation/recompute, cold setup and source refresh separately, with
unfavorable cases visible. Zero added hot-path RTT and fsync counts are explicit
assertions, not assumptions from latency alone.

Use matched trusted-embedded/governed runs with identical deterministic tools
and provider behavior, logical payloads, concurrency and existing store settings.
Report authorization-induced changes in call count rather than normalizing them
away. Measure at 1 and 16 concurrent operations, 1/100/1,000/10,000 dependencies and sustained
streaming/audio publication. Record at least 10,000 dispatch samples per
steady-state cell, p50/p95/p99, CPU, allocations, native write-lock hold time and
storage/network calls. Reserve an actual quiet host window; compile everything beforehand. Test active memory
pressure separately. Larger-context cost must scale with context changes, not
repeat a whole dependency walk on every tool dispatch or stream chunk.

1. Integrate native association, existing-transaction persistence and homogeneous
   batching with one real local policy owner and the shared enforcement seams.
   Demonstrate an admitted input through actual model/tool calls and authorized
   output, denial before sink entry, restart and revocation during a stream.
   Inject every refusal class while another tool, model or publication operation
   is ready: prove no run-terminal event, cancellation or session hold occurs.
   Revoke a grant ancestor without a policy edit, race compile/invalidation, and
   test excluded cross-product tuples through cache reuse, batching and fallback.
   Prove each refusal route (tool, model, recipient, memory/compaction/comms)
   leaves no unanswered waiter or session blocked by the refused operation. The
   irreducible-request case follows the explicit owner decision still pending above.
2. Extend that same path across every built-in provider, streaming/live,
   compaction, memory, comms/delegation and Elephant. Shared evaluation stays
   portable, including wasm32, without pulling native witness/clock dependencies
   into embeddings. Acceptance requires full coverage; the slim smoke fixture
   is not a narrower product profile. Include adopted 10,000-message history,
   newly enveloped connector input, service schedules, partitioned compaction,
   persistent-memory reader isolation, provider-route changes, MCP defaults,
   live context reset, audio revocation and independently gated replay subscribers.
3. Run focused races/fault cases and the measured budgets, then obtain independent
   implementation acceptance and green PR CI. Review deltas from accepted
   checkpoints; rerun broader gates only when affected by a concrete change.

| Disposition | Existing work |
| --- | --- |
| Retain and integrate | Qualified principals; immutable native association and exact replay binding; canonical restriction algebra; narrowed delegation; source provenance; homogeneous batching; operation-local refusals; portable governed feature isolation; ordinary transaction and generated-owner preparation where actually needed. |
| Park, preserve evidence | External witness service, commissioning/rollback detection, NTS/chronyd producer, external attempt anchors, witness-backed recovery, sealed witness storage and their physical control protocol. They are not prerequisites or transitive runtime dependencies of local-governed-v1. |
| Remove from default requirements | Human-only Unknown continuation, mandatory buffered output, a special owner-built HTTP transport, per-attempt extra durable barriers and blanket refusal of providers/live/compaction. Existing safe provider/tool contracts remain. |

The original ADR, frozen reviews and code branches remain available. This
amendment changes the required default deliberately; it does not relabel the
unfinished high-assurance implementation as complete or publish it as supported.
