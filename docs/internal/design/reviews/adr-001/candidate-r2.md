# ADR-001: Shared runtime authorization and governed information flow

## Status

Proposed, 2026-09-30. Decision owner: Luka. This record proposes architecture,
not shipped behavior, an accepted API, or a completed security guarantee.
It is outside the 0.8.50 release scope. Implementation requires the evidence
gates below; this document does not authorize deployment or migration.

This proposal resolves the ABAC placement question in
[caller-context.md](caller-context.md), originating in
[PR #1333](https://github.com/lukacf/meerkat/pull/1333). If accepted, this ADR
owns the security architecture; that document remains the propagation and
first-profile investigation companion. Until acceptance, its status is unchanged.
Normative words in this document describe the proposed governed profile.

The [adversarial review record](adr-001-runtime-security-review.md) records
review rounds, findings, dispositions, and remaining limits.

## Context

Meerkat, MobKit, and Elephant have valuable but disconnected controls.
MobKit authenticates console callers and applies agent-oriented ABAC at HTTP
and SSE boundaries. Meerkat has typed principals, credential-binding witnesses,
tool execution ceilings, argument-aware consequence policies, signed comms,
and schedule ceilings. Elephant authorizes classified records and preserves
some security provenance in derived knowledge. None alone establishes the
authorization of a complete human-to-agent-to-knowledge-to-effect chain.

The requirement is to authenticate actors, restrict tools and information use,
attenuate delegated work, enforce current permissions, and reconstruct every
protected decision and effect. A caller field or larger console action map
cannot satisfy it. Reading, processing, forwarding, publishing, declassifying,
and administering information are different decisions.

The following source observations anchor the proposal. They are static findings
at the pinned baselines, not exploit demonstrations or claims about every host:

| Current fact | Implication |
| --- | --- |
| MobKit's [RPC classifier][mk-rpc] returns no requirement for unmapped methods; some reads filter results separately. | Every governed operation needs an exhaustive feature-owned declaration, including explicit public operations. |
| MobKit's optional [operator resolver][mk-operator] selects the last console speaker for an agent. | Work must carry exact requester authority; an agent-keyed interaction cache cannot decide whose memory it may use. |
| [AccessView][mk-view] captures policy and groups; [agent SSE][mk-sse] admits once. | Policy change and stream disclosure need an explicit freshness contract. |
| [Blob download][mk-blob] checks console authentication and blob ID possession. | Resource authorization must cover information independently of console agent visibility or content hashes. |
| Meerkat's [tool gate][mk-tool] is outermost, and [provider-native tools][mk-native] are disabled under restricted compositions. | Extend the existing gate; do not build an independent competing ABAC wrapper. |
| [InputHeader][mk-input] and [ToolDispatchContext][mk-context] lack general authenticated execution authority. | Add the missing carrier through existing admission and execution owners. |
| [Tool-consequence observation][mk-observer] represents denied and indeterminate outcomes, with optional observation. | Existing telemetry cannot witness every allowed effect. |
| Elephant's [policy principal][el-policy] has clearances and scopes but no actor; [MCP context][el-context] defaults record attribution to `mcp_tool`. | Preserve authenticated identity through resource decisions and mutations. |

Baselines: Meerkat `54d14e91bd426fcaaafc156227b0b5ab0e23d9a6`,
MobKit `e2795b5dbbabe224e2e912256d7b99e480b5c9c9`, Elephant
`1aefaad2de68535ca5b588f58e8ed1ae1bf5b56a`.

## Decision

Create a first-class, feature-owned authorization capability in Meerkat.
Share its dependency-light contracts and policy evaluation/composition machinery
with Elephant. MobKit becomes a policy administration and observation client.
Application rules remain application-owned. Resource and execution owners remain
authoritative for their own facts and effects.

The common capability owns decision semantics, constraint composition, grant
contracts, and security receipt contracts. It does not absorb session, mob,
comms, schedule, knowledge, or credential lifecycle into a universal machine.
Core holds only foundational vocabulary and required extension seams. Exact
crate and Rust type names follow the ownership tracer; concepts below are not
an instruction to create parallel versions of existing principal/grant types.

### 1. Ownership and composition

| Semantic fact | Canonical owner | Forbidden substitute |
| --- | --- | --- |
| Authenticated identity and assurance | Trusted ingress identity authority and its declared issuer mapping | Email alone, prompt text, self-declared metadata, a peer key without a principal binding |
| Grant validity, delegation bounds and revocation generation | Grant authority, with generated lifecycle authority for state transitions | Cached Allow, inherited host privilege, arbitrary policy-expression subset inference |
| Active policy content, revision, activation epoch and rollback | One declared policy-domain authority; preserve existing policy-provider ownership unless explicitly migrated | Application authoring state, reload order, a second accepted-revision map in the common evaluator |
| Resource identity, classification, version and security attributes | Owning feature or external data authority | Console roster projection, unverified tool arguments, derived display labels |
| Effective decision for an exact operation | Shared authorization composition contract, instantiated by the host | Surface-specific rule ordering, independent permissive wrappers |
| Operation entry, effects and settlement | Existing operation owner and its generated machine/composition/handoff | Policy evaluator executing work or audit projection deciding completion |
| Provider credential lifecycle | Existing binding-scoped AuthMachine and token-store contract | Requester identity inferred from provider credentials |
| Security evidence durability and append identity | Declared security receipt store contract | Best-effort UI event stream or mutable change journal |
| Policy administration UI and audit queries | MobKit projections over the owners above | A second policy store or runtime authority in the console |

One owner applies per fact. Pure attribute evaluation does not require a new
state machine. Changes to grant validity, admission, batch compatibility,
operation entry and lifecycle must lower through the owning generated authority
or explicit typed composition/handoff. Stores persist that authority; they do
not decide it.

Policy authoring and activation are separate. Each domain's policy authority
validates content identity, durably activates a revision under a monotonic epoch,
and restores that active epoch on recovery. Reusing a revision with different
content is invalid. Rollback is a newly authorized activation under a new epoch,
not restoration of stale authority. The existing tool-policy provider remains
the owner of accepted revisions until an explicit ownership migration replaces
it. The common evaluator consumes active-policy evidence without maintaining a
second mutable accepted-version map.

Authorization is a conjunction of the applicable constraints: original mandate,
delegation restrictions, executor ceiling, current host policy, exact resource
policy, destination policy, and mandatory obligations. All required decisions
must allow. Deny, indeterminate, missing authority, unsupported enforcement, or
unfulfilled obligations cannot become Allow. Within each policy domain, rule
combination is explicit and conformance-tested; no adapter may silently change
deny precedence or treat an unknown attribute as unrestricted.

Each feature owns a versioned action/resource/attribute declaration and lowers
its operations into it. Generated catalogs aggregate these declarations and
reject undeclared governed entry points. Public metadata operations are declared
public explicitly. Tool/schema discovery, list/search, events and administrative
operations are included. UI affordances remain non-authoritative projections.

### 2. Trust boundary and operating profiles

The governed profile treats users, model output, retrieved content, tool
arguments, remote peers and transport fields as untrusted unless their exact
claims are authenticated by an authorized source. Trusted host composition,
approved policy/identity services, operation owners, and declared resource
authorities form the trusted computing base. Resource providers may make claims
only within their registered domains; authenticating a provider does not let it
mint unrelated user identity or lower another source's restrictions.

In-process Rust code with unrestricted store/network access is inside that
boundary. The profile does not sandbox a malicious embedder or prove freedom
from timing, traffic-analysis, hardware or covert channels. Tool code, hooks and
plugins that need adversarial containment require process isolation, restricted
filesystem/network access and credential confinement. A shell tool with ambient
network credentials cannot be advertised as confined by a tool-name ACL.

Trusted-embedded and governed are explicit composition profiles. Durable
security evidence is mandatory in governed; there is no implicit non-audited
governed variant. Profile identity is persisted with admitted work and negotiated
on handoff. Governed bootstrap refuses missing required components and
unsupported feature paths.
No missing controller, legacy field, failed policy call or restart may downgrade
the profile. The trusted-embedded profile remains available for trusted hosts,
but cannot advertise governed guarantees. A governed request cannot cross into
an adapter that does not negotiate the required capability. Profile changes are
authorized administration and do not reinterpret existing work or erase its
restrictions. Mechanical cancellation and cleanup remain available without
granting new private reads, model calls or publication.

### 3. Identity, execution authority and delegation

Ingress verifies issuer, audience, lifetime and the appropriate authentication
proof, then resolves a canonical trust-domain-qualified principal. Email and
display names are attributes. Workload/peer authentication and human
authentication are distinct; cross-host forwarding requires both a trusted
transport and validation of the forwarded authority.

The durable execution association requires the original requester, executing
principal and concrete binding where applicable, authenticated ingress, exact
native work/contributor identities, grant lineage, original constraint ceiling,
and disclosure mode. Disclosure is either explicit recipients/destinations or
an explicit no-external-release mode. Actor and requester are never substituted
for one another. Background service work has a separately authorized service
mandate. Labeling a continuation System does not create one.

Persist non-secret provenance and ceilings atomically with existing input/work
admission. Bearer credentials and a perpetual Allow bit do not belong in durable
inputs. Resume and retries retain the original association and reauthorize new
uses. Idempotency is scoped to authenticated authority and exact operation
identity; a conflicting requester, ceiling or audience is a typed conflict,
not a duplicate entitled to the original result.

Delegation retains root provenance and adds only explicit restrictions over
actions, resource domains, processors, recipients, lifetime and delegation
depth. The supported restriction algebra is monotonic: child constraints are
conjoined with parent constraints, never replaced by a child policy. No general
implication test over arbitrary ABAC expressions is assumed. An unsatisfiable
effective permission set denies. Individual empty attribute sets retain their
domain meaning; an empty allowlist never becomes unrestricted. Absence, unknown,
unrestricted and an empty set have distinct types.
The grant authority checks both permission to delegate and the requested bound.
Using a more privileged agent never transfers that agent's unrelated authority
to the request. The model can request a grant but cannot mint one.

Every derived grant remains dependent on the validity of all required ancestors.
Root or intermediate revocation invalidates descendant use within the declared
entry/freshness contract even when the leaf token has not expired. A service
mandate that intentionally survives a human grant must be independently issued
by its own authority; it is not a detached descendant with forgotten ancestors.

Recurring work stores an independently revocable mandate bound to its creator,
target, scope and grant generation. Each occurrence rechecks that mandate and
current policy; a schedule's existence or retained tool ceiling is insufficient.
Human approval is an attributable, expiring grant for exact operation parameters,
not a reusable bypass. Policy administration, identity administration,
credential administration and data access are separate actions. Any emergency
access is an explicit bounded, auditable grant subject to mandatory constraints.

A privileged service handoff is a separate typed operation: the requester is
authorized to commission a specific service, while an independently issued
service mandate authorizes that service's bounded internal processing. The
commissioning operation's ceiling and service mandate must both allow the
handoff; the requester need not have direct read rights to every internal input.
Record the causal requester and service actor without substituting either's
identity. The handoff specifies service identity, purpose, resources, processors,
outputs, recipients, lifetime and revocation dependencies. It cannot expose
private intermediate data, service credentials or reusable internal permits to
the requester. Publication is separately authorized. This is an authority-owned
commissioning contract, not a generic agent privilege-elevation flag.

### 4. Protected operations and effect entry

An authorization request identifies the subject/actor chain, typed action,
canonical resource and version, resolved tool/provider binding, arguments or
protected canonical digest, destination, purpose, current grant/policy evidence,
and work contributors. Feature owners resolve these facts. Arbitrary tool JSON
must not force a generic evaluator to guess paths, recipients or resource IDs.
Security attributes carry authorized source, domain, version and freshness
evidence. Mutable labels under a caller's control are not clearance claims.

The decision returns denial, indeterminate/unavailable, or an operation-bound
permit with policy/resource versions, constraints, expiry and obligations.
A permit is authority only under the declared trusted-host or authenticated
remote protocol; a serializable Rust struct is not a cryptographic proof.
Changing arguments, target binding, destination, contributing data or policy
identity requires a new decision. Required obligations must be realized by a
capable owner; an unsupported obligation refuses the operation.

The operation owner performs fenced entry through a named participating-authority
composition. All slow preparation and policy evaluation precede the final fence.
Each grant, policy, resource and binding authority whose update must precede entry
must validate and hold its generation through the single durable entry commit,
using a reservation/fence or equivalent atomic compare protocol. The composition
defines participants, the entry linearization point, cancellation/expiry, crash
release, lock order and when revocation is acknowledged as effective. Holding only
the operation owner's lock or performing one last version read is insufficient.
Authorities unable to participate require an explicit bounded lease profile,
even within one process. Unsupported coordination refuses the claimed profile.
The exact resource binding remains covered through realization; changing it
requires a new decision. No unrelated await may separate validation from an
unfenced dispatch. An entered attempt has its own identity;
cloning a permit does not authorize another effect. Existing effect ownership,
idempotency and cancellation-resistant settlement remain intact. A generic
policy callback never acquires arbitrary executor authority.

Local operation-entry ordering defines whether revocation wins the race.
Before entry, revoked authority refuses; after entry, cancellation is attempted
where supported and the actual outcome is recorded. New attempts, retries,
fallbacks and disclosures reauthorize. Long-running tools must declare safe
checkpoints or an indivisible entered effect with its limits.

Across hosts there is no atomic global-currentness claim. A remote adapter must
declare its grant authority, audience binding, freshness/lease bound, revocation
check and replay protection. The destination enforces its own entry fence.
Unknown freshness or an unavailable mandatory authority refuses. Policy and
attribute caches are valid only under explicit version/freshness contracts;
they cannot silently extend grants. No policy/store lock is held across an
unbounded model or network wait.

### 5. Information use and disclosure

Resources include history, transcript revisions, injected context, memories,
documents, evidence, source media, search results, artifacts, blobs, credentials,
comms payloads and audit records. Features declare distinct read, use, disclose,
mutate, relabel/declassify, delegate and administer actions as applicable.
Permission to call a tool does not authorize every resource it can address.

The owning data service authorizes private body reads before hydration into
agent/tool/model context. Query execution inside a trusted search service can
use its separate service mandate, but caller-visible records, counts, scores,
citations, errors and pagination must not disclose denied records. Discovery
and retrieval require explicit projections appropriate to the caller.

Governed data carries source identity/revision, classification and handling
requirements, relevant subject entities and completeness, permitted processing
and disclosure constraints, and a derivation/decision reference. These facts
are provided by resource authority, not invented by the LLM. Retrieved content
retains its untrusted instruction status independently of confidentiality.
High clearance is not evidence that content is a legitimate user instruction.
The recipient-visible payload and metadata are separate from the protected
control envelope used by the enforcement TCB. Dependency IDs, people, labels,
grant references and refusal reasons can themselves be sensitive. They may be
opaque authority-resolved handles and must not automatically enter prompts,
citations, tool-visible JSON, logs or ordinary audit queries. Each public metadata
projection needs its own disclosure authorization. A safe refusal need not reveal
the hidden resource or policy that caused it.

Derived data preserves the conjunction of contributing restrictions and the
union of relevant source dependencies. Unknown contributors remain unknown and
cannot be silently discarded to obtain permission. Summarization, compaction,
embeddings, memory harvesting, extraction and caching do not declassify data.
Relabeling/declassification requires a separate authorized transformation,
defined release criteria, provenance and audit. Ordinary write permission is
insufficient. The initial profile conservatively attaches the complete observed
context dependency domain to model/curator-produced answers, summaries, tool
arguments and artifacts, including control-influencing input. Model citations
or relevance claims cannot remove dependencies.

Content revisions and live security state are separate identities. Before each
use or disclosure, every transitive dependency must satisfy its authority's
current access/classification/tombstone epoch or an explicit bounded lease that
covers that state. Retaining an immutable revision, old receipt or cache entry
does not retain permission. Missing dependencies or unavailable mandatory
authorities refuse use. The source authority defines whether deletion revokes
retained-copy use; absent an explicit surviving archival/release grant, deletion
does revoke it. Historical policy evidence remains audit evidence only.

Before each model attempt, authorize selected history/injected data and its
disclosure to the actual provider route, including credential binding and
processing destination. The context inventory includes provider-held caches,
persistent model sessions and referenced remote context, even when their bodies
are absent from the local request. Unknown remote dependencies refuse use.
Fallback and retry are new attempts. Native tools,
hooks, traces, exporters and custom sinks must join the same contract or be
unavailable in the governed composition. Preserve the existing fail-closed
provider-native disable behavior until an adapter proves coverage.

Before output release, authorize the actual recipient against all contributing
information. This covers tool arguments/results, comms, transcript reads,
replay, attachments and streaming. Execution can complete while disclosure is
denied; record the execution truth without releasing the withheld body or
inviting duplicate execution. Revocation cannot recall already disclosed bytes.

The first profile authorizes a complete selected context domain and buffers
output. Mixed-confidentiality reuse, dynamic audiences, multi-principal batching
and partial streaming remain unsupported until their profiles prove equivalent
per-use guarantees. Future message-level authorized projections must preserve
source dependencies; simply deleting a forbidden message after it influenced a
summary is insufficient. Exact-compatible batching must be a guard of the
existing queue/dequeue owner, with all contributors preserved. No first-writer
identity, audience union, or conflict-to-missing fallback is permitted.

### 6. Elephant federation

Elephant remains the resource authority for its spaces, records, classifications,
subject entities, evidence and derived knowledge. Meerkat owns runtime execution,
context use and disclosure. Both applicable decisions must allow; neither may
override the other. A shared evaluator does not copy Elephant ACLs into a
Meerkat cache or make Meerkat an issuer for arbitrary Elephant clearances.

Preserve Elephant's record checks and domain predicates when sharing policy
machinery. Its [subject-aware projections][el-subjects], security-separated
knowledge and [revocable wiki authority][el-wiki] are assets. Existing scope
semantics, including special service grants, require differential conformance
tests before migration. Sharing code must not broaden their meaning.

Elephant's literal `manage:wiki` grant is a privileged-service commissioning
contract from section 3: an unrestricted-subject manager can authorize bounded
wiki processing across security envelopes without gaining direct read access
to those inputs. A wildcard scope does not substitute for that literal grant.
The service's live space-bound grant fences future work; protected outputs and
intermediate context remain separately authorized. Changing space, processor,
operation or output destination cannot reuse the commissioning authority.

The integration protocol carries issuer-qualified actor/requester identity,
authenticated delegation, exact space, actions/resource bounds, audience,
purpose, expiry and revocation evidence. Elephant validates the issuer's right
to attest each claim and maps it to local policy. A general static service token
must not substitute for the caller's authority. Credential minting/exchange is a
declared service with constrained delegation; prompts never receive secrets.

Elephant returns only authorized data, with the governed result envelope from
section 5 and a correlated local decision receipt. Meerkat validates that the
resource authority and response bind to the exact request before consuming it.
Knowledge subjects describe entities a record is about; they are a different
type from the authenticated principal. Labels and provenance survive transport,
helper delegation, storage, model use and final publication.

Migration must first preserve empty subject restrictions as restrictive:
[current conversion][el-empty] collapses an empty allowlist into unrestricted.
Preserve domain semantics: `None` is unrestricted subject scope; `Some(empty)`
admits no named subjects, but an explicit `handling:subjectless_ok` waiver on a
record, together with caller clearance for it, can permit empty or incomplete
subjects under Elephant's existing predicate. Named subjects must still be in
the allowlist. This permission-bearing waiver is distinct from a restrictive
handling caveat and cannot be unioned from one contributor onto another. Derived
waiver validity requires every contributing source to authorize the exception.
It must also introduce separate relabel/declassification authorization:
[entity update][el-relabel] currently checks ordinary read/write on old and new
records while allowing replacement security fields. These independent fixes
do not require completion of the shared runtime feature.

### 7. Durable security evidence

A security receipt records stable decision/operation/attempt and causation IDs;
root request and contributors; authenticated actor/requester and delegation;
action; resource and policy versions; destination; restrictions and obligations;
decision and bounded reason; and the operation owner's entry/settlement outcome.
Authentication, grant/attribute/policy changes, approvals and declassification
also emit correlated records. Protect payloads and existence-sensitive reasons;
exclude credentials and unnecessary private plaintext. Exact historical policy
and attribute evidence must be retained or referenced under an explicit retention
contract so a digest does not masquerade as an explanation.

Security receipts are append-only under a declared integrity-verifiable store
contract. The governed profile requires a durable authorization and
effect-intent record before effect release, coupled to existing owner admission
or an explicit crash-safe handoff. Failed persistence refuses new protected
effects. There is no second work queue, lifecycle reducer or recovery journal:
operation owners retain execution authority and link their durable evidence.

The entry commit couples consumed attempt identity, operation-owner state and
pre-effect evidence in one transaction or declared crash-safe handoff. An intent
row alone never authorizes execution on recovery. Only the existing operation
owner can authorize realization or a retry. Dynamic tools, detached operations
and deferred session effects must declare their actual effect owners: successful
tool dispatch can mean launch acceptance, not terminal completion. Record launch
and each subsequent effect under owner-generated correlations; terminal receipts
come from the actual owner after settlement, including failed deferred commits.

Settlement appends the real outcome. A crash or ambiguous remote timeout may
leave Unknown; recovery cannot invent success, cancellation or safe retry.
External exactly-once effects require destination idempotency/reconciliation
support and cannot be obtained from a local receipt alone. Security evidence
must distinguish decision, entry and outcome, including incomplete audit chains.

Execution, evidence durability and disclosure are separate typed axes:

| Situation | Execution truth | Evidence/custody and allowed action |
| --- | --- | --- |
| Entry has not committed | Not entered | No effect; refuse or retry admission through its owner. |
| Entry committed, outcome not established | Entered/unresolved | Owner retains attempt; reconcile or settle, never infer safe retry from an intent. |
| Effect has known outcome, mandatory receipt append fails | Preserve known applied/failed outcome | Evidence pending; owner retains receipt-only retry obligation and withholds body release while its required evidence is incomplete. An authorized observer sees outcome plus evidence-pending, never an ordinary retryable execution failure. |
| Required terminal evidence committed | Preserve known outcome | Disclosure still requires current permission; evidence does not grant publication. |
| Crash loses unpersisted outcome knowledge | Unknown after recovery | Reconcile against the effect owner/destination; no fabricated failure, success or cancellation. |

Before an effect, its existing owner must have durable custody sufficient to
recover pending settlement/evidence obligations. After a known outcome, it retries
only the receipt operation, even after caller loss. If the outcome itself cannot
be persisted before a crash, recovery reports Unknown with the retained attempt
identity. It cannot claim receipt-only recovery knows the lost result. A pending
mandatory receipt blocks new protected uses of the result, not mechanical
cancellation or reconciliation. No independent security recovery queue decides
whether work runs again.

Authentication failures and denied decisions also require evidence when the
store is available. If it is unavailable, reject protected ingress/effects and
surface an explicit audit-unavailable operational fault. Do not claim that an
unrecorded denial was durably audited or record unverified identity as authenticated.
The complete-audit claim covers admitted protected work and durably recorded
decisions, not every packet that reached an unavailable process.

Exporters and the console consume projections of committed receipts. Export
lag or UI failure does not turn completed work into failure. A required receipt
commit is semantic; optional telemetry is advisory. Existing runtime event
projection and Elephant's mutable data-change journal retain their own purposes
and cannot be advertised as the complete security receipt store.

## Alternatives considered

| Alternative | Decision and reason |
| --- | --- |
| Extend only MobKit console ABAC | Rejected as the architecture: runtime tools, background work, alternate surfaces and disclosures remain outside its authority. |
| Carry caller attributes and let arbitrary host callbacks decide independently | Rejected without the common composition contract: it leaves coverage, obligations, revocation and audit semantics to each leaf. Host policy providers remain supported behind the shared contract. |
| Move all policy and lifecycle into Core or one security machine | Rejected: violates feature ownership and duplicates existing operation authorities. |
| Adopt Elephant's current Principal/evaluator unchanged | Rejected: actor identity, generic delegation, policy lifecycle and effect audit are missing; domain policy semantics must be retained. |
| Use only signed capabilities | Insufficient alone: useful transport evidence, but resource attributes, current policy, source restrictions and effect settlement still need owners. |
| Use only content taint or prompt instructions | Rejected: neither grants permission nor enforces recipient/resource-specific information use. |
| Select a policy language first | Deferred: engine/backend choices follow the required contracts and conformance evidence. The shared contract must not depend on unproven general policy implication. |

## Consequences

Positive: one explicit contract across surfaces; policy authoring stays flexible;
Elephant and Meerkat preserve domain ownership; delegated authority cannot grow
silently; source restrictions survive derived work; complete audit has a stated
meaning and failure mode; generated catalogs can ratchet enforcement coverage.

Costs: additional policy/resource lookups, durable writes, protocol and storage
changes, retained provenance, administrative lifecycle work, and explicit
unavailability for unsupported integrations. Whole-context isolation and buffered
output constrain initial UX. Remote freshness bounds limit revocation promises.
Data provenance increases storage and privacy obligations; access-controlled
retention must be specified. A shared evaluator reduces duplication but introduces
coordinated versioning and semantic conformance requirements.

Security claims remain bounded by the trusted computing base. Authorization
does not establish truth of retrieved information, defeat all prompt injection,
or confine arbitrary native code without an isolation profile.

## Implementation sequence and acceptance gates

1. Decide this ownership contract and threat model. Build the nonmergeable
   ownership tracer proposed in caller-context.md using actual current owners.
   Prove where existing injection is sufficient and identify missing seams.
2. Define feature-owned action/resource catalogs, required identity/authority
   carriers, monotonic restriction algebra, permit-entry contract and receipt
   durability profile. Extend existing principal/tool-policy types deliberately.
3. Implement one governed vertical slice with a deterministic provider and
   recorded sinks: authenticated person -> agent -> attenuated helper ->
   Elephant read -> authorized model route -> authorized reply. Include cold
   restart and a second independent caller. Do not advertise general coverage.
4. Add schedules, comms, history/memory/artifacts, administration and alternate
   surfaces under the same contract. Replace console authority with projections
   and shared operations. No permanent double evaluation with divergent policy.
5. Enable a profile only when every advertised operation and adapter passes its
   coverage and fault tests. Old work lacking provenance is explicitly migrated
   with current resource authorization or refused; historical requester identity
   is never invented. Retire old authority paths with a declared cutover.

The acceptance suite must assert both the decision and absence/presence of the
actual protected read, network send, mutation or publication. A denied response
after the sink already received bytes fails. Required cases:

| Attack or fault | Required observable evidence |
| --- | --- |
| Forged requester/issuer/actor or prompt claims | No authority minted; no protected sink reached. |
| Confused deputy, helper with broader capabilities, System continuation | Root constraints remain binding at the recipient/effect. |
| Empty restriction or unknown attributes | No widening; typed refusal distinguishes missing facts. |
| Same idempotency key under another caller/audience; lost acknowledgement | No foreign completion disclosure and no duplicate effect. |
| Cold restart, flow retry, cancelled waiter, queued schedule | Exact authority persists; new uses recheck current grants. |
| Revoke before/after entry or during stream/remote outage | Before-entry blocked; entered outcomes truthful; next disclosure bounded by the advertised freshness profile. |
| Tool arguments/resource/destination change after decision | Entry refuses the mismatched permit. |
| Model fallback, native tool, hook/exporter, raw blob/history endpoint | Same resource and disclosure contract or explicit unsupported refusal. |
| Denied source influences ranking, summary, cache, memory or later caller | No unauthorized projection/body/disclosure; source dependencies retained. |
| Label downgrade, broadened audience, policy-admin impersonation | Separate authority required and recorded. |
| Different requester/audience batches, including three-input conflict | Existing queue retains separate schedulable work; no authority collapse. |
| Audit unavailable before entry; crash after remote send; exporter failure | Refusal, explicit uncertain settlement, and observational degradation respectively. |
| HTTP/RPC/MCP/CLI/SDK/WASM and direct trusted-host composition | Same declared operations obey the same profile; absent paths do not claim support. |
| Cross-version adapter strips context or unknown obligation | Capability negotiation refuses before protected work. |
| Independent grant/policy/resource update after final observation, before entry | Participating production authority fences determine one winner; no test-only common lock; old executor cannot realize a new effect. |
| P1 -> P2 policy activation, restart, replayed P1, same revision with different bytes, explicit rollback | One active epoch; stale/different content refused; rollback receives a new authorized epoch. |
| Root/intermediate grant revoked while leaf is unexpired | Descendant reads, queued jobs and resumed work refuse within the declared bound. |
| Known sink success followed by mandatory receipt failure/caller cancellation | Known result plus evidence-pending survives where durable; receipt-only retry, no second effect; crash with lost result becomes explicit Unknown. |
| Successful async launch followed by failure, or deferred session commit refused | Launch is not terminal success; actual owner emits the terminal outcome. |
| Unchanged content revision with reclassified/tombstoned transitive source | Stored summaries/artifacts and cached output cannot authorize fresh use from historical evidence. |
| Provider-held cache contains secret absent from current request; uncited context reaches curator | Full dependency domain follows output; unauthorized model invocation or publication refuses. |
| Literal wiki service commissioning versus wildcard-only or subject-restricted manager | Authorized bounded service can process private inputs without granting requester direct reads; altered service parameters refuse. |
| Subject scope None/empty/A, complete/partial/unknown subjects, waiver and caller clearance combinations | Preserve Elephant truth table; disallowed named subjects remain denied; one contributor cannot transfer a waiver. |
| Protected envelope carries canary person/source ID | Enforcement can resolve dependency while prompts, visible tool JSON, UI, traces and ordinary audit queries omit denied metadata. |
| Governed bootstrap/restore/handoff without receipt capability | Refuse the profile before protected effects; no silent audit downgrade. |

Property tests must establish monotonic attenuation and policy combination.
Differential tests must preserve Elephant domain rules. Generated coverage gates
must reject any governed operation without a declaration. Adversarial review of
this ADR is design evidence only; implementation acceptance requires executed
fault and sink assertions against production ownership paths.

## Open implementation decisions

The following choices must be fixed before their corresponding implementation
slice is accepted; none permits an adapter to invent local semantics:

- Concrete crate/API boundaries and generated state/handoff changes, based on
  the tracer's existing-owner inventory.
- Supported policy backend and versioned attribute schemas; activation storage
  implements the single-owner/new-epoch rollback semantics decided above.
- Credential exchange format and key rotation/trust-domain federation, including
  maximum remote lease/revocation delay and replay protection.
- Receipt store atomicity/integrity scheme, historical evidence retention and
  deletion/redaction policy, implementing the custody/result matrix above.
- Later mixed-history, streaming, dynamic-audience and hostile-tool isolation
  profiles. They remain unavailable until independently specified and proven.

## References

Canonical doctrine: [Meerkat Dogma](../../architecture/meerkat-dogma.md) and
[commentary](../../architecture/meerkat-dogma-commentary.md). In particular,
singular authority, feature ownership, thin surfaces, explicit handoffs, and
semantic versus advisory failure govern this decision.

[mk-rpc]: https://github.com/lukacf/meerkat-mobkit/blob/e2795b5dbbabe224e2e912256d7b99e480b5c9c9/crates/meerkat-mobkit/src/http_console.rs#L2373
[mk-operator]: https://github.com/lukacf/meerkat-mobkit/blob/e2795b5dbbabe224e2e912256d7b99e480b5c9c9/crates/meerkat-mobkit/src/memory/coordinator.rs#L163
[mk-view]: https://github.com/lukacf/meerkat-mobkit/blob/e2795b5dbbabe224e2e912256d7b99e480b5c9c9/crates/meerkat-mobkit/src/access/controller.rs#L245
[mk-sse]: https://github.com/lukacf/meerkat-mobkit/blob/e2795b5dbbabe224e2e912256d7b99e480b5c9c9/crates/meerkat-mobkit/src/http_sse.rs#L339
[mk-blob]: https://github.com/lukacf/meerkat-mobkit/blob/e2795b5dbbabe224e2e912256d7b99e480b5c9c9/crates/meerkat-mobkit/src/http_console.rs#L3939
[mk-tool]: https://github.com/lukacf/meerkat/blob/54d14e91bd426fcaaafc156227b0b5ab0e23d9a6/crates/meerkat/src/factory.rs#L7613
[mk-native]: https://github.com/lukacf/meerkat/blob/54d14e91bd426fcaaafc156227b0b5ab0e23d9a6/crates/meerkat/src/factory.rs#L7212
[mk-input]: https://github.com/lukacf/meerkat/blob/54d14e91bd426fcaaafc156227b0b5ab0e23d9a6/crates/meerkat-runtime/src/input.rs#L34
[mk-context]: https://github.com/lukacf/meerkat/blob/54d14e91bd426fcaaafc156227b0b5ab0e23d9a6/crates/meerkat-core/src/agent.rs#L707
[mk-observer]: https://github.com/lukacf/meerkat/blob/54d14e91bd426fcaaafc156227b0b5ab0e23d9a6/crates/meerkat-core/src/tool_consequence_policy.rs#L597
[el-policy]: https://github.com/lukacf/elephant/blob/1aefaad2de68535ca5b588f58e8ed1ae1bf5b56a/crates/policy/src/engine.rs#L23
[el-context]: https://github.com/lukacf/elephant/blob/1aefaad2de68535ca5b588f58e8ed1ae1bf5b56a/crates/mcp/src/handlers/mod.rs#L86
[el-subjects]: https://github.com/lukacf/elephant/blob/1aefaad2de68535ca5b588f58e8ed1ae1bf5b56a/crates/knowledge-views/src/lib.rs#L1253
[el-wiki]: https://github.com/lukacf/elephant/blob/1aefaad2de68535ca5b588f58e8ed1ae1bf5b56a/crates/pipeline/src/wiki_editorial.rs#L1004
[el-empty]: https://github.com/lukacf/elephant/blob/1aefaad2de68535ca5b588f58e8ed1ae1bf5b56a/bin/elephant-api/src/authz.rs#L152
[el-relabel]: https://github.com/lukacf/elephant/blob/1aefaad2de68535ca5b588f58e8ed1ae1bf5b56a/crates/mcp/src/handlers/entities.rs#L844
