# Provider-owned extension documents

Status: implemented candidate, validation pending, 2026-10-05. The independent
implementation and adversarial review are ongoing; this document does not
claim passing runtime acceptance or a released upstream dependency.

## Decision

Meta supplies durable, access-controlled extension documents through the
deployment's configured storage provider. Each optional extension package owns
its commands, payload schema and evaluation. Enabling an extension does not
create a second database configuration or expose a generic state-editing tool
to agents.

Implement the generic contract and reference SQLite backend in a small
`meta-extension-state` crate with no Meerkat dependency. Meta's optional
adapter resolves runtime authority and provides the configured store. This
allows fast storage/policy tests without compiling the agent runtime.
Meta's dependency on that crate and its runtime factory adapter are behind
an optional feature that is off by default. Omitting the feature introduces no
document store open, tool definitions, prompts or runtime initialization.

## Existing seams and gaps

- `crates/meerkat-meta/src/storage_provider.rs:93` lists the current store
  set. There is no generic extension document store.
- `storage_provider.rs:141` is the composite provider boundary. Its existing
  fail-closed rule requires declared durable storage or an explicit error.
- `src/blob_store.rs:60` stores bytes but has no document listing or revision
  compare-and-swap. Blobs alone cannot implement concurrent mutable documents.
- `src/runtime/metadata.rs:306` owns subscription cursors and idle-retirement
  records. It is not an application-data store.
- `src/unified_runtime/builder.rs:375` registers named tool bundles selected
  through profile `tools.rust_bundles`. Existing registration expects a ready
  dispatcher; a factory is needed to receive provider-opened services.
- `src/unified_runtime/builder.rs:1240` notes that bootstrap can revive members
  before identity activation. Binding authority only after bootstrap returns
  is therefore too late.
- Meerkat `ToolDispatchContext.origin_session_id()` is filled by the runner.
  Forks can reuse the parent's external-tools `Arc`. A dispatcher must resolve
  the caller for every call, never capture the parent at construction.
- Durable `fork_source` and some `spawned_by` provenance exist upstream.
  Ordinary spawned children and fresh delegates lack complete durable parent
  provenance. Console labels are not an authority substitute.

## Document and storage contract

A record has a provider-minted nonreused document ID, package namespace,
realm, title, content type, schema version, opaque payload bytes, owner, ACL,
opaque revision, and timestamps. Payload interpretation and schema evolution
belong to the extension package, not the document store.
Realm and namespace are bound by the host service, not supplied by tool input.
Names are display metadata and are never document identity.

The backend exposes create, get, filtered/paginated list, conditional content
replacement, conditional access change, conditional ownership transfer, and
conditional deletion. Content, ACL, owner and revision form one atomic record.
Every mutation except create requires the current revision. Every committed
mutation, including ACL changes, advances it. Deleted IDs cannot be recreated.

Each entry point receives trusted host access context. The common crate owns
one policy evaluator; backend implementations invoke it against the current
record inside their mutation transaction before comparing and replacing the
record. SQLite uses a write transaction, not a read-authorize-write sequence
on separate connections. Remote backends must provide equivalent atomicity.

The extension evaluates and serializes an edit before conditional commit. A
concurrent edit, ACL change, or transfer makes that commit conflict or become unauthorized.
Failed validation or commit leaves the stored document untouched.

Mutation request IDs are scoped by realm, namespace and actual principal.
The fingerprint binds the canonical original agent request after parsing and
default normalization, before loading or interpreting the document payload.
It includes the requested operation, document, expected revision, data changes
and requested result selection. It must not be computed from a recalculated
payload whose value can change after the first commit.

The adapter first asks the service for an authorized prior receipt using that
request ID and fingerprint, before loading a document or rejecting a stale
expected revision. A completed identical retry returns the original receipt.
Otherwise evaluation proceeds and the mutation transaction checks the receipt
again before authorizing/comparing/committing, so concurrent duplicate calls
also have exactly one effect. Different requests reusing an ID fail. The
transaction stores the fingerprint and successful receipt with the document
commit. A retry after lost authorization does not reveal document content.
Receipts, including minimal deletion receipts, persist across restart.
Retention must not silently weaken the promised retry window.

Missing and unauthorized documents have the same public error. Check access
before disclosing revision conflicts. Listing filters by current access before
returning rows, counts or cursors; it has bounded page and payload sizes. No
unfiltered document IDs or ACL membership appear in pagination metadata.

## Ownership and data audiences

The caller context is created by the host and is not deserializable from agent
arguments. It contains a verified realm and principal, current mob memberships,
managed mobs, realm administration, verified lineage, and applicable host policy.
Extension tools must reject caller-supplied authority fields. Labels and
transcripts never establish any of these facts.

| Owner | Direct management authority | Implicit data audience |
|---|---|---|
| Agent | That stable principal, subject to host policy | The owner and its permitted fork lineage |
| Mob | Host-attested managers of that mob | Managers only; ordinary membership needs an explicit grant |
| Realm | Host-attested realm administrators | Administrators only; realm membership needs an explicit grant |

Agent ownership is the creation default. A caller can create for itself;
creating for a mob or realm requires the corresponding management authority.
Ownership, realm-wide access, and current mob membership are distinct facts.

Direct owner-management authority permits sharing, ACL changes, transfer and
deletion. Reader and Editor grants never permit those actions. Editor permits
document-data edits; Reader permits reads. Inheritance never carries management.
An administrator's action still intersects host ABAC and tool execution policy.
There is no implicit bypass because the store or package runs as a privileged
host process.

Explicit grant audiences are an agent principal, a mob membership audience, or
the realm audience. Agent audiences have reach `SelfOnly`, `Forks`, or
`Descendants`; normal grants default to `Forks`. The agent owner's implicit
read/edit audience also defaults to `Forks`. An owner can change this reach.
Mob/realm membership grants are evaluated from current trusted membership;
they do not automatically extend to agents outside that membership.

Transfer is a conditional owner-authorized operation. The new owner is
validated in the same realm. Existing explicit grants remain unless the same
atomic request changes them; the old implicit owner audience disappears. A
transfer receipt must make this effect clear. No session disposal implicitly
transfers or deletes documents.

## Forks, children, ceilings and revocation

`Forks` matches the addressed principal and a path consisting only of verified
fork edges. `Descendants` additionally accepts verified spawn edges. A child
created by ordinary spawning needs an explicit direct grant or an audience
whose reach includes descendants. A fork uses the original document ID and
current ACL; no document or ACL is copied at fork time.

For every inherited route, compute the minimum of the source grant role and
every delegation ceiling along the verified path. Then intersect the result
with current runtime/tool policy and host ABAC for the actual caller and action.
A Reader cannot become an Editor through another fork. Do not clone Meerkat's
tool policy into an extension-specific policy language: the host evaluates its existing typed
policy and passes the permitted operation ceiling to the generic service.

Removing a grant revokes that grant and every access route derived from it on
the next call. Independent direct grants can still permit access. To remove
all access for a branch, owner administration supports a deny for an agent
audience with `Descendants` reach. Deny overrides every matching direct or
inherited data grant. Read denial also denies edits; edit denial leaves reads.
Direct owner administration can remove a denial; inherited access cannot.
Changing the owner's reach to `SelfOnly` revokes its implicit inherited routes.

Current ACL evaluation and the write are in the same storage transaction, so
an ACL revocation cannot race with an already-authorized stale write. If the
edit commits first it was authorized before revocation; if revocation commits
first the edit is refused. Already returned data cannot be retroactively erased.

Lineage must be complete enough to prove both an allow and the absence of a
matching subtree denial. Missing, cyclic, truncated, inconsistent or stale
provenance fails closed for delegation. Legacy unknown ancestry must not be
treated as a proven root to bypass a subtree denial.
Immutable creation provenance distinguishes explicit host `Root`, verified
`Spawn` or `Fork`, native `Successor`, current-runtime `Unproven`, and
`LegacyUnknown`. Generic construction defaults to `Unproven`; only a trusted
host root constructor or a captured native source witness establishes proof. Only runtime-attested `Root` proves an empty ancestor chain;
an absent old field is `LegacyUnknown`, not `Root`. Unsupported or incomplete
ancestry produces a typed authority-unavailable result inside the host and an
appropriate non-disclosing tool failure, never a silently reduced allowlist.

Durable application identities keep ownership across restore and respawn.
Their principal key is `(realm_id, application_agent_identity)`. Worker
principals are `(realm_id, mob_id, member_identity, member_creation_id)`, where
`member_creation_id` is a runtime-issued immutable creation token persisted
with the member-created event. A restored incarnation retains that token;
a newly created worker using the same name receives a new token. A native
adapter must not invent a replacement token from display labels or process
startup time. Exact originating session IDs prove creation bindings; they
are not the document owner key.
Rebinding an application identity to a successor is permitted only by the
identity authority's explicit continuity contract. Reusing an identity string
in an unrelated roster is not evidence of successor continuity.
Retaining verified historical provenance allows a fork to keep its granted
access after its parent's execution session ends. If that historical binding
cannot be proven, reject inherited access rather than borrowing a successor's
identity. Which existing upstream records suffice is an implementation gate.
Every intermediate link must retain its original principal and creation proof.
Missing intermediate proof refuses inherited access, even when the endpoints
have the same display name as known members. Removing a parent from the active
roster does not by itself remove a persisted creation edge or document grant.

## Provider capability and bootstrap sequence

Add a defaulted optional `MetaStorageProvider::open_extension_state(context)`
capability. The default returns typed `Unsupported`. This avoids a new required
field in every downstream `MetaRealmStoreSet` initializer. The result contains
the store and its durability declaration. Open it once when at least one
registered extension requires it, and bind package namespaces over that store.

The disk provider chooses its path through `MetaStorageLayout`. Non-disk
providers supply their own backend through the same configured provider.
Unsupported, failed-open or nonpersistent storage refuses activation of a
package that requires durability. There is no package-local SQLite or memory
fallback. Hosts without such packages do not open this capability.

The optional registration API:

```rust
builder.register_tool_bundle_factory(
    "durable-documents-test",
    ToolBundleRequirements::durable_documents("durable-documents-test", "documents_read", "documents_apply"),
    external_factory,
)
```

The factory receives a namespace-bound document service and an `Arc<dyn ToolCallerResolver>`, never a principal.
The resolver obtains `origin_session_id` on each dispatch, reads typed persisted
session/member bindings, validates the current caller binding, obtains durable
lineage, and intersects host ABAC. Lookup failures propagate; an empty roster
projection is not evidence of no ancestry.

Required order, owned entirely by Meta bootstrap:

1. Resolve the deployment's provider and all required document capabilities.
2. Create the deferred authority resolver and factory-built dispatchers; register
   bundles before any member is built, including restored members.
3. Construct the mob handle/session authority without releasing member execution.
4. Bind the resolver to those authorities and identity mapping, exactly once.
5. Only then permit revival, bootstrap inputs, identity reconciliation and new
   turns; publish the runtime as ready afterward.

This needs a pre-activation hook in `MobRuntime` bootstrap, and potentially in
Meerkat's build/resume path if its current ordering cannot supply step 3. A
late-bound resolver must refuse dispatch while unbound, but that refusal alone
is not an acceptable substitute for correct ordering: restored work must not
run and fail merely because registration races startup. Adopters must not wire
a mutable global, reconstruct the provider, or capture a parent-bound closure.

## Export and protected artifact handling

Raw document bytes stay behind the document service. Meta's existing
`/blobs/{id}` route is a bearer-capability surface, not document ACL enforcement.
Do not return its blob IDs or URLs for protected documents or their exports.
Protected exports are deferred from this contract. Extensions return bounded
explicit reads only; no artifact-route redesign is required for this release.
A future export may use a protected artifact handle only when every fetch checks the actual
principal, host policy and current document ACL. Reusing the physical blob
backend does not authorize using the public blob route.

## Required upstream work and acceptance gate

Meerkat remains the owner of runtime creation provenance. Add durable,
runtime-issued parent provenance for ordinary `mob_spawn_member` and fresh
delegates, established before child activation and replayed after restart.
Reuse existing fork provenance rather than inventing a second fork truth.
Distinguish fork edges from spawn edges; `spawned_by` alone is insufficient.
Cover cross-mob delegates with typed endpoints and exact originating sessions.
No parent edge may come from model arguments, labels or inferred comms strings.
Acceptance requires end-to-end evidence for same-mob forks and ordinary spawns,
plus cross-mob delegation. A backend or runtime path unable to prove one of
these must report typed unsupported ancestry, not claim that the feature works
because another path passes.

The native resolver also needs a typed, error-preserving provenance read and a
pre-activation binding point. Missing upstream support is a release blocker for
the promised child/fork behavior, not an invitation to ship label-based fallback.

## Requirements and validation

| Requirement | Acceptance evidence |
|---|---|
| Uses deployment storage | Close all handles, reopen configured disk/remote provider, recover payload, ACL, owner, revision and retry receipt |
| Agent-private default | Another principal cannot list, read, edit, share, transfer, delete or infer existence |
| Least authority | Reader cannot edit; Editor cannot administer; Mob/Realm ownership does not silently grant member data access |
| Forks and children | Fork reads original ID; fresh child denied until direct/descendant grant; both behaviors survive restart |
| No amplification | Reader through two forks remains Reader; each host/delegation ceiling and ABAC denial wins |
| Revocation | Source-grant removal affects forks; subtree denial beats independent grants; ACL/write race has one valid ordering |
| Identity continuity | Restore preserves owner; worker name reuse and unrelated successor cannot inherit documents |
| Atomicity | Concurrent edits from one revision have one winner; ACL/content/owner changes advance the same revision |
| Retry integrity | Same request returns one receipt; changed request with same ID fails; delete/recreate cannot resurrect stale writes |
| Input trust | Fake realm/principal/ancestry/admin fields, labels and cross-namespace IDs cannot grant authority |
| Startup ordering | A restored member's first tool call has bound services and actual caller authority; shared fork dispatcher resolves child |
| Output protection | Bounded reads/listing reveal no hidden records; protected exports cannot bypass document ACL through `/blobs` |

## Root review conditions

The 2026-10-05 root review approved this contract with the following required
conditions, incorporated above:

1. Idempotency fingerprints identify the original request, and authorized
   receipt lookup precedes document evaluation or stale-revision rejection.
2. The Meta extension-state dependency and adapter remain default-off.
3. Persist proven root versus legacy-unknown provenance and require explicit
   durable-identity successor continuity.
4. Prove same-mob and cross-mob lineage paths; missing support fails typed.
5. v0.1 delivers bounded reads and defers protected export routes.

An agent other than the design author implements the reviewed change. Tests,
review and PR evidence must establish the contract rather than treating this
document as evidence of runtime behavior.

## Minimal file plan and alternatives

- New `crates/meta-extension-state`: typed documents/ACLs/revisions, policy
  evaluator, provider-facing contract, SQLite reference implementation and
  independent conformance tests. No domain-specific payload concepts or Meerkat
  dependencies.
- `storage_provider.rs`, `storage_layout.rs`, storage health/doctor/migration:
  optional capability, provider-owned path, durability and lifecycle integration.
- `unified_runtime/builder.rs` and `mob_handle_runtime.rs`: factory registration,
  dependency resolution and authority binding before execution.
- A small optional adapter module: typed dynamic caller/lineage resolver and
  intersection with host policy. No authority in extension payloads or schemas.
- Upstream Meerkat PR: durable ordinary-spawn/delegate provenance and any missing
  pre-activation/query seams. Meta integration follows its released contract.
- Extension repositories: domain engines, tool schemas and package factories
  consuming the service.

Rejected: independent package-local SQLite (splits deployment storage),
runtime/session metadata or semantic memory (wrong owner), raw blobs alone (no CAS/catalog/ACL),
copied fork ACLs (revocation divergence), and console labels as lineage
(untrusted and not durable). A required new realm-store-set field is viable but
unnecessarily source-breaking for providers that do not enable extensions.

## Native integration contract

Enable Meta's `extension-state` feature explicitly and register a factory with
`register_tool_bundle_factory("durable-documents-test", requirements, factory)`. Requirements
name its document namespace and the read/edit tools. The factory receives a
provider-owned `DocumentService` and a `ToolCallerResolver`; it resolves each
runtime dispatch before reads, mutations and receipt lookups. The reference
provider caps a payload at 8 MiB and identifiers/revisions at 512 UTF-8 bytes.
No protected export route is included in this version.

The current authority identifies the executing native agent. It does not prove
the original requester. The single `runtime_origin_session` adapter retains
Meerkat's runner-supplied dispatch context and is the replacement point for the
[tracked original-requester context work](https://github.com/lukacf/meerkat/issues/1646).
Native tool grants and current host policy intersect document ACL rights. A
document ACL can narrow those rights but cannot grant a tool denied by native
execution policy. Namespaced access subjects are `agent:<stable identity>` or
`worker:<creation UUID>`; resource identity selectors still receive the actual
stable application identity, and labels come from the current native roster.

A stable application `AgentIdentity` denotes the same logical agent across
reset, deletion and host re-registration. Applications must mint a different
stable identity for a different agent, even if its display name is reused.
Unrelated worker name reuse receives a distinct runtime-issued creation token
and cannot access its predecessor's documents. Historical exact-session identity
bindings are immutable and remain available after session retirement.

Initial native extension calls wait for the host's actual continuity binding
publication. This is a runtime-local readiness fence, never identity authority:
it carries no principal or grants, preserves the original dispatch context,
and does not alter general tool admission or provider-native tool capabilities.
A canceled publication returns authority unavailable. Existing historical
bindings remain readable while a replacement is pending or fails.

The native integration uses the Meerkat crate family with exact version
requirements and a committed lockfile. Applications consuming Meta through Git
pin the reviewed revision and resolve the same crate family. Additional child tool bundles are propagated into native delegated
mob composition so the same per-call resolver works there.

A failed stable-identity materialization reserves its exact host target until a
successful host retry publishes the actual session binding. It remains
unavailable rather than being reclassified as a Worker. Retained provisional
identity history provides the same exclusion after restart; decoding the
canonical native member name locates that history only in the identity bridge's
root mob and never assigns an Agent principal. Ordinary Worker name reuse on
unreserved targets keeps its independent creation-token behavior. A missing
source session policy makes inherited access unavailable. The native
`load_retained_session_metadata` authority reads the latest retained metadata
for that exact source session, including archived sessions. It does not assert
that the source is currently executing, or return a historical policy revision.
Missing typed metadata, binding, unsupported reads and failures remain unavailable;
a present typed record with no effective policy has native unrestricted
semantics. The native factory persists its conjunction of the declared profile
restriction and explicit spawn restriction in this effective field. The
separate `spawn_tool_access_policy` field is not the effective policy.

Inherited data access intersects the current source session policy and current
caller policy on every call. An explicit host-authorized widening of native
source policy may widen an otherwise valid document ACL route. Immutable
delegation floors belong to the tracked native authority work; this package
does not persist a private policy snapshot or claim original-requester proof.

Without an enabled host access controller, ABAC adds no further restriction;
private document ACLs and native tool policy still apply.

Since Meta 0.9.4 the identity history comes from Core's member history, which
keeps every binding Core committed for an identity whether or not the
extension is enabled; Meta keeps no history tables of its own. A history Core
cannot read refuses activation with `ExtensionAuthorityUnavailable` before the
extension store opens or its dispatchers are built, and an unreadable history
is never treated as evidence that sessions were Workers. Turning the
extension feature or its factory off opens no extension store or dispatcher
and leaves the stored documents in place.

Document durability does not widen native member lifetime. The current
Meta composition gives implicit delegate child mobs in-memory journals;
completing a delegate retires its helper and fork descendants. The cross-mob
acceptance fixture therefore checks live private/share/fork/revoke behavior
and durable original documents after truthful retirement. Persistent
root-mob member lineage and fork access use the configured durable native
mob journal. Restoring ephemeral delegate children is not a capability of
this integration, and no synthetic lineage is created to pretend otherwise.
