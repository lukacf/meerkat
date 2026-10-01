# Governed deployment profiles for ADR-001

## Status and ownership

Proposed, 2026-09-30. This is the deployment companion to
[ADR-001](adr-001-runtime-security.md), reviewed as part of the same proposal.
It assigns application policy choices to their actual owners rather than to
Meerkat. It does not describe shipping features or authorize a deployment.

The runtime contract remains in the ADR. The conditions below are normative
when a deployment claims the corresponding profile. Optional classification,
legacy adoption or reserved notification profiles require explicit enablement,
authority and conformance evidence. Absence of support refuses the operation;
it cannot silently select trusted-embedded behavior. Anonymous-human, physical,
dynamic-membership and live-voice profiles remain unavailable initially.

The examples explain Homecore, OB3 and Toolkit requirements without assigning
those products' policy semantics to Meerkat. Applications choose concrete
principals, audiences, source classes, relationship rules and transformation
grants; the common runtime verifies and preserves their declared authority.

## Authority over imported attributes

Sheet owner cells, pasted links, directory heuristics and imported labels are
suggestions unless their write path performs the required grant/release
decision. A field may authoritatively describe a value without its editor being
entitled to grant access through that value. Subscription and allowlist changes
must name an authorized grantor and preserve revocation dependencies.

## Identity assurance and exposure

Assurance distinguishes an authenticated human from an authenticated device or
service carrying an unidentified human request, and from explicitly supported
anonymous ingress. An unidentified speaker is not the device owner. Its ceiling
comes from an ingress-authority anonymous-access policy. Audience contracts can
represent explicit principals, external membership or physical exposure;
a device/location does not prove who can hear or see it. Physical release must
cover possible unidentified observers. Anonymous-human, physical-exposure and
dynamic-membership profiles remain unavailable initially; missing identity or
audience support refuses protected hydration and release.

## Conversation and shared contexts

A declared message-resource authority assigns each admitted human or peer
message a policy-backed classification, processing constraints and disclosure
audience from authenticated ingress facts. The transport adapter conveys that
authority; it does not invent access rights. Sender requests may narrow bounds
but cannot override restrictions on known copied or derived content. Agent
ownership and authentication alone do not confer disclosure rights. A private
message's default can permit a reply to its sender without exposing it to an
agent owner, operator or monitoring channel. Missing classification authority
refuses governed admission. An unmapped message class also refuses; only a
mapped class with an authorized restricted-ingestion policy can await
classification inside that restricted context. Independently authored fresh input is classified
as a new resource; this does not claim inference of every source in a human's
external knowledge.

The ADR's audience-bound admission gate runs before a contribution enters the
shared context. In a household calendar, an adults-only item must be refused or
routed to an adults-only partition before a context shared with children sees
it. The same logical calendar agent can retain its identity across partitions
and fresh context generations, under the existing binding owner.

Triage partitions inputs using authorized metadata before shared inference,
or uses independently isolated model contexts for compatible partitions. A
mixed-context model's outputs retain the entire dependency domain; separate
JSON objects do not prove disjoint provenance. Logical agents can own multiple
context partitions without combining incompatible requester authority.

## Stored and external disclosures

An authenticated principal's individually controlled external account is eligible
for the initial stable-recipient profile under an explicit release contract.
The account-binding authority verifies the principal/account relationship;
the policy expressly permits that provider's retention and the principal's own
account-authorized devices. This is not inferred from an address or successful
API send. Revocation or loss of the binding blocks new sends, but cannot recall
retained copies. Shared accounts and multi-member groups do not qualify merely
because one member owns the credential; they require their actual audience
contract. Compromised or unknowable account control does not prove a fixed
audience. Implementation still requires the destination conformance evidence.

Writes to application databases, warehouses, object stores, logs and exports are
protected disclosures or custody transfers. Either every plaintext read path
(including other applications, direct IAM access and exports) enforces retained
restrictions, or the write authorizes the effective reader audience. Persisting
an envelope beside readable plaintext is insufficient. Encryption with keys
confined to the enforcing boundary can support controlled custody.

Channel membership at send time does not authorize future readers of retained
history. Such destinations need enforceable subsequent access control or an
explicit release policy covering retention and evolving membership, including
future entrants. The latter deliberately grants broader release, not revocable
per-reader protection. Monitoring copies, test redirects, subscriptions and
alternate destinations each require their own disclosure authorization. A
standing grant may supply it; adapter configuration alone cannot. The initial
profile does not support dynamic audiences, so these product paths remain
unavailable until that destination profile is specified and proven.

The initial profile therefore cannot be advertised as sufficient for a product
whose operation requires ungoverned database readers or changing channel
audiences. Those paths need their own proven destination profile or an explicitly
authorized broader release contract; existing product behavior is not evidence
that the requirement is satisfied.

## Classification and legacy adoption

Two explicit transformations address input whose classification or historical
provenance cannot be obtained mechanically. Neither is implicit on ordinary
read, write, import, backup possession or system administration:

- **Bounded classifier service.** A typed mandate identifies source class,
  input scope, permitted processors and classification outcomes, decision
  authority and release semantics. Model proposals are committed only through
  that authority, with source version, mandate and model-attempt evidence.
  Any allowed outcome broader than the restrictive ingestion baseline is an
  explicit preauthorized release/trust assumption. A label lattice bounds the
  possible release; it does not prove correct classification or resistance to
  prompt injection. Deployments rejecting that discretion keep the baseline
  until a separately authorized transformation permits broadening. Restrictive
  proposals compose by actual permission semantics, never by unioning labels
  or creating permission-bearing exceptions. Incompatible restrictions reroute
  or refuse shared-context admission.
  The initial classifier profile uses a fresh per-item context with fixed
  authorized configuration and no previous transcript, memory or provider cache.
  Classification labels are derived outputs and retain all observed/control
  dependencies. A future batch profile carries the complete batch dependencies;
  sharing a source principal or class does not prove per-item independence.
  A mixed classifier context cannot issue an independent label for item B by
  omitting item A from its output.
- **Legacy bundle adoption.** An authority with explicit rights over a legacy
  corpus may adopt an immutable, enumerated bundle as a new governed resource.
  It records exact bytes/version, actor, purposes, processors, audience,
  retention, expiry, revocation and unknown historical authorship/dependencies.
  Its grant explicitly covers those unknown internal dependencies within the
  stated bounds; it does not reconstruct them or invent message requesters.
  Known source restrictions still bind unless their own release authority
  permits change. New additions require new authorization. Derived outputs
  depend on the adopted bundle, whose revocation blocks later use. Unadopted
  material remains refused. This cannot promise source-specific revocation for
  identities never recovered, and it grants no replay authority over old work.
  Store custody, operator/admin status and a generic guardianship relationship
  confer no adoption or release right. A proven per-person corpus retains that
  person's privacy floor absent an explicit applicable release grant. Broader
  release of a multi-person corpus must satisfy all applicable rights authorities,
  including participant grants or a specifically scoped representation grant
  where policy permits it. Neither a union of participants nor guessed membership
  establishes an audience entitled to every contribution. Unknown participants
  require explicit corpus-wide adoption authority covering that uncertainty;
  otherwise the bundle remains refused. This is the same authority standard as
  other release transformations, not an administration shortcut.

## Reserved notification evidence

A host may declare a reserved durable receipt backend for explicitly
preauthorized bounded notifications. The class fixes authenticated event sources,
destinations, payload fields, current authorization or a bounded mandate,
capacity and exhaustion behavior. It cannot hydrate arbitrary private context
or widen recipients. Evidence commits there before dispatch; the ordinary
operation owner retains attempt and settlement authority. Later reconciliation
preserves append identities and integrity evidence without scheduling effects
from spool contents. Ambiguous appends across backends are reconciled by the
same identity. If required authority or durable capacity is absent, the governed
notification refuses. This supports an independently provisioned alarm path;
it is neither an unaudited emergency bypass nor a life-safety delivery guarantee.

Quarantine does not relax this rule. A notification owner independent of the
reverted state may proceed only if a surviving rollback-resistant authority or
monotonic witness proves its current mandate and covers every required revocation
dependency. The witness must participate in the same-domain fence; a stale copy
or missing remote invalidation cannot certify validity. Any permitted cross-domain
lease still follows ADR section 4. If that proof or durable capacity is unavailable,
alarms are explicitly unavailable during quarantine. An external authority outage
is not permission to reuse an unvalidated snapshot mandate.

## Integration conformance cases

These cases instantiate ADR invariants and the optional profiles above. Core
cases apply to every deployment claiming the relevant operation; profile-specific
cases apply before that optional capability is enabled. They are implementation
acceptance requirements, not tests already executed by the design review.
The Scope column is mandatory: Core rows apply whenever that runtime operation
is supported. Profile rows name the controlling section and become mandatory
whenever that behavior is enabled, regardless of a deployment's chosen profile
name. The column never makes an unconditional ADR invariant optional.
Enabled means a reachable behavior in the governed runtime composition, not
merely a declared profile name. An application cannot omit the label to avoid
the warehouse, imported-attribute or other applicable conformance cases.

| Scope | Attack or integration fault | Required observable evidence |
| --- | --- | --- |
| Core | Missing identity/audience support or missing/unmapped classification authority | Refuse protected admission, hydration and release before any sink; a restricted context is not an escape. |
| Core | Contribution conflicts with a segment's intended audience or processing domain | Refuse or partition before hydration, transcript write and inference; post-inference splitting cannot establish independent provenance. |
| Core | Any reachable persistent-write or external-send path lacks required destination guarantees | Refuse release regardless of declared profile labels; all plaintext readers and applicable retention semantics are covered. |
| Core | A handle claims independent ownership of Meerkat-owned grant/epoch state | Trusted composition rejects that claim; Elephant can consume a Meerkat parent grant while retaining independent resource authority, uses its declared protocol/lease, refuses expiry, and retains its classification when relocated. |
| [Profile: conversation](#conversation-and-shared-contexts) | Private conversational input with no external classification | Policy-backed ingress permits the sender's authorized reply; no disclosure to another user, agent owner or monitoring destination by default. |
| [Profile: stored disclosure](#stored-and-external-disclosures) | Restricted finding written to a warehouse; second app or direct table reader | Every plaintext path enforces the envelope or the full effective audience is authorized before write; storing labels alone fails. |
| Core | One revoked source in a 200-turn context, then reset | Old segment refuses protected use; fresh segment can answer independent input without old user/model messages, summaries, metadata or caches. No reset-induced duplicate effects or late-result injection. |
| Core | Delayed/dropped change notification, cursor rollback, broad policy update | Coverage gap triggers revalidation/refusal; no stale grant merely because no notification arrived. |
| [Profile: imported attributes](#authority-over-imported-attributes) | Sheet owner/link edit, heuristic identity or access-conferring label write | No grant/subscription/read entitlement without the required authority over that write. |
| [Profile: external disclosure](#stored-and-external-disclosures) | Retained channel history exposed to a new member; monitoring copy or redirect | Explicit destination/release contract covers the real audience or refuses; each secondary disclosure has attributable authorization. |
| Core | Admin addresses another person's agent or reads its transcript | No contributor's protected content without the applicable data grant. |
| Core | Receipt commits but acknowledgment is lost | One append identity, no duplicate effect, reconciliation establishes committed evidence without inventing an execution result. |
| Core | Old worker after replica takeover; twenty parallel attempts | Stale incarnation cannot enter; per-attempt evidence and obligations remain distinct through physical group commit. |
| Core | Live attachment, cache or old provider session during reset/bootstrap | Initial live profile refuses before connect; absent complete authorized context inventory, no provider invocation. |
| Core | Different evaluator/application versions | Shared vectors and semantic negotiation reject incompatible obligations; no forced lockstep application upgrade or silent weakening. |
| Core | Legacy origin or mapped control principal | Retain immediate origin while authority is explicitly unavailable; no implicit governed Owner/admin privilege. |
| [Profile: conversation](#conversation-and-shared-contexts) | Shared household context receives a private input; mixed triage batch | Refuse or partition before hydration/transcript write/inference; broad-audience work remains usable after compaction; post-inference splitting cannot erase dependencies. |
| [Profile: adoption](#classification-and-legacy-adoption) | Adopt an ungoverned history bundle, then revoke it | Only explicit authorized adoption enables its stated audience; historical requester remains unknown; new bytes and revoked bundle uses refuse. |
| [Profile: classifier](#classification-and-legacy-adoption) | Inject an instruction to classify private input broadly | Hard outcome bounds hold; measure semantic misclassification separately. A permitted broad label is not proof of correct confidentiality classification. |
| [Profile: exposure](#identity-assurance-and-exposure) | Guest uses authenticated device; visitor sees display | No implicit owner identity or private hydration; unsupported physical/anonymous profile refuses. |
| Core | Approval text appears in mail, peer output or model response | No grant without the approver's bound authenticated ingress event and exact displayed digest. |
| Core | Resume after discovery grant changes | Refresh metadata under current discovery authority; keep discovery and execution decisions separate. |
| Core | Shared credential preparation with two independently authorized waiters | Cancelling/revoking one cannot borrow the other's activation or revoke its separately owned credential; late callbacks remain attempt-bound. |
| Core | Login callback replay, account substitution or secret canary | Exact ceremony binding enforced; secrets absent from model, browser observation, transcript and ordinary event sinks. |
| [Profile: external account](#stored-and-external-disclosures) | Send to an individually controlled external account, then revoke its binding | Explicit retention/device policy and verified owner binding authorize the first send; new sends refuse after revocation; group accounts remain outside this class. |
| [Profile: adoption](#classification-and-legacy-adoption) | Adopt another person's corpus using only custody or admin status | Refuse without applicable adoption/release authority; participant union or generic guardianship does not supply it. |
| [Profile: classifier](#classification-and-legacy-adoption) | Item A precedes item B in a classifier context | A's dependencies remain on B's proposal; only an actually isolated per-item attempt can establish independent input scope. |
| [Profile: notification](#reserved-notification-evidence) | Restore while the primary authority is unavailable | Alarms proceed only with surviving current-mandate proof and durable evidence; otherwise quarantine refuses alarms as well as ordinary work. |
| Core | Revoke in state generation N+1, restore N, or start a cloned fixture | Surviving authority prevents resurrection; unauthorized clone cannot hydrate/call a provider; receipt history cannot fork silently. |
| [Profile: notification](#reserved-notification-evidence) | Exhaust ordinary receipt capacity and send a bounded alarm | Reserved backend commits exact evidence first; ordinary disclosures still refuse; exhaustion has no unaudited fallback or duplicate replay. |
| Core | Lose owner store before versus after an external effect | Fence old incarnation, distinguish proven non-entry from unknown, and re-admit only through owner authorization with preserved identity; no duplicate sink call. |
| Core | Revoke guardianship, membership or approver-for while work waits | Dependent new reads, approvals and queued work refuse within the declared bound. |

## Evidence

The [review record](adr-001-runtime-security-review.md) retains the product
counterexamples, exact reviewed candidates and finding dispositions. No profile
can substitute a successful design review for actual sink/fault evidence.
