# ADR-001 adversarial review record

This record accompanies [ADR-001](adr-001-runtime-security.md). It records
design review, not implementation acceptance, security certification, or
authorization to deploy. The ADR remains Proposed; Luka is the decision owner.

Final result: all four requested project reviewers cleared both r6 documents
with no remaining material findings in their reviewed scopes. The final internal
delta review also cleared both hashes. Original rejections and their resolutions
are retained below; design clearance is not implementation acceptance.

## Artifacts and method

The author inspected PR #1333 and pinned Meerkat, MobKit and Elephant source
baselines recorded in the ADR. Independent reviewers were instructed to seek
concrete counterexamples, cite the violated authority or information contract,
and distinguish missing architecture from deferred implementation detail.
Reviewers could reject the proposal. The author reconciled findings and sent
the complete revision back for another adversarial pass.

| Revision | SHA-256 | Purpose |
| --- | --- | --- |
| r1 | `7806f38fd70f37362ef6f9ae143ab2511b0fdadae5a9cb4dcd7bb3b0616f8073` | Initial 444-line draft reviewed by three subagents. |
| r2 | `44882a1ba3c8ea36d9a44c13df6c4b52b246c459dacc93437ef188ea7ac055ca` | 568-line revision incorporating the first round. |
| r3 | `a242a21407fbf52d2ca5615edfdd6b5649b40fa6f73c959d4857eddd8e2db87e` | 904-line revision addressing project-owner findings. |
| r4 ADR | `246ba64f62ba1cdf8022a27da0f275664dc1d4e0e4034dbe24114ab9f7f814b3` | Runtime ADR after final precision and ownership edits. |
| r4 profiles | `b2e39c432201ce3fe2f4df4f6a53e83193f0cdfdeefd22a0590c85f82c68dbe9` | Conditional deployment profiles extracted without weakening obligations. |
| r5 ADR | `dae4e6a4fab44b103ced096cd730a54668a4d617a78b72dd10c2ed30f88dd0b5` | Adds explicit intent-sensitivity limit and deferred selective carry-forward. |
| r5 profiles | `20392d72d41cb7f7c1703080a94300db4ddb14efe103f0a0bea38c38aab3dbf9` | Clarifies account release, adoption, classifier isolation, quarantine alarms and every conformance row scope. |
| r6 ADR | `2c3744f703ea0f58f3cbd6cd02103e0539eadb5eafac4cd6a48de34f0a38c6fe` | Final per-authority domain definition, admission refusal and constrained-host budget requirements. |
| r6 profiles | `05ed1bd4202f4d53d5b7a5f83af052c6806a1809a6b8e152a060df74e294cc53` | Final reachability-based profile applicability and four explicit Core cases. |

The [frozen r2](reviews/adr-001/candidate-r2.md) and
[frozen r3](reviews/adr-001/candidate-r3.md) candidates preserve exact bytes
supplied to project-owner reviewers. Their relative links retain the
original ADR location as their base; use the main ADR for navigation.
Hashes identify the ADR bytes only, excluding this record and reviewer reports.
Reports preserve the reviewer's own scope and verdict; GREEN does not imply
that the current code implements the proposed controls.

## Subagent round 1

| Review | Verdict | Report |
| --- | --- | --- |
| Authority, lifecycle, revocation and audit | RED | [Full report](reviews/adr-001/adr-001-authority-r1.md) |
| Identity, information flow and bypass paths | GREEN with precision improvements | [Full report](reviews/adr-001/adr-001-information-r1.md) |
| Elephant policy compatibility and federation | RED | [Full report](reviews/adr-001/adr-001-elephant-r1.md) |

| Finding | Revision in r2 |
| --- | --- |
| A1: an operation-local lock cannot fence independent policy, grant and resource owners | Define participating-authority composition, generation stability through the entry commit, revocation acknowledgment, crash release, and explicit bounded leases when participation is unavailable. |
| A2: effect success followed by audit failure can erase truth or trigger a duplicate; dispatch success can merely mean async launch | Separate execution, evidence and disclosure states. Couple attempt consumption, owner state and intent; preserve known outcomes and receipt-only retry custody, and report Unknown after lost outcome knowledge. Actual effect owners settle detached/deferred work. |
| A3: policy activation and rollback had no singular owner | Assign one domain authority, durable monotonic activation epochs, immutable revision content and authorized rollback as a new activation. |
| A4: wording allowed a silently unaudited governed profile | Make durable evidence mandatory in governed; persist and negotiate the profile, and refuse unsupported bootstrap, restore or handoff. |
| E1: immutable source revision could outlive current restriction or deletion | Require current security state for every transitive dependency or a covering bounded lease. Historical evidence is not fresh authority; deletion revokes retained use absent an explicit surviving grant. |
| E2: pure requester-read intersection breaks Elephant's privileged wiki service | Specify authorized commissioning and an independent bounded service mandate, preserving causal requester and actor without granting direct access to private intermediates. Publication remains separately authorized. |
| E3: generic empty-scope rule breaks the subjectless waiver predicate | Preserve None versus Some(empty), named-subject checks, explicit record waiver plus caller clearance, and contributor-wide validity of the permission-bearing exception. |
| E4: security metadata can itself disclose denied sources or people | Separate protected control envelopes from recipient-visible projections; metadata release requires its own authorization. |
| N1-N3: explicit ancestor validity, provider-held context and conservative dependency attribution | Require live validity of all necessary grant ancestors, include remote model caches/sessions, and carry the complete observed/control-influencing context into derived outputs. |

The acceptance matrix now names sink and fault tests for each repair. These
tests are implementation obligations, not tests executed for this document.

## Subagent round 2

| Review | Verdict | Report |
| --- | --- | --- |
| Authority, lifecycle, revocation and audit | GREEN; A1-A4 retired | [Full report](reviews/adr-001/adr-001-authority-r2.md) |
| Identity, information flow and bypass paths | GREEN; N1-N3 closed | [Full report](reviews/adr-001/adr-001-information-r2.md) |
| Elephant policy compatibility and federation | GREEN; E1-E4 closed | [Full report](reviews/adr-001/adr-001-elephant-r2.md) |

## Subagent round 3

All three reviewers reread the exact r3 bytes and returned GREEN for design
coherence and further project review. No mandatory text repairs remained.

- [Authority and recovery](reviews/adr-001/adr-001-authority-r3.md)
- [Information and identity](reviews/adr-001/adr-001-information-r3.md)
- [Federation and domain compatibility](reviews/adr-001/adr-001-elephant-r3.md)

## Project-owner review via agent bus

Requested reviewers: GCP Meerkat/MobKit lead (`claude-gcp-lead`), Homecore
developer (`homecore`), OB3 developer (`ob3`), and Meerkat Toolkit developer
(`toolkit-codex-local`). Each reviewer received the complete frozen r2 candidate via the GCS-backed
agent bus on 2026-09-30, from `codex-security-adr-mac`. All four sends succeeded.
The requests specified the exact hash, asked for counterexamples and minimum
repairs, and did not disclose the subagent verdicts.

| Reviewer | Role | Status |
| --- | --- | --- |
| `claude-gcp-lead` | GCP Meerkat/MobKit lead | [RED on r2](reviews/adr-001/gcp-lead-r2.md); F1-F9 resolved in the dispositions below. |
| `homecore` | Homecore developer | [RED on r2](reviews/adr-001/homecore-r2.md); F1-F10 dispositioned below. F5 closed as an accepted limitation: anonymous, physical and dynamic profiles remain unavailable. |
| `ob3` | OB3 developer | [RED on r2](reviews/adr-001/ob3-r2.md); F1-F7 resolved in the dispositions below. |
| `toolkit-codex-local` | Meerkat Toolkit developer | [GREEN on r2](reviews/adr-001/toolkit-r2.md), with implementation acceptance requirements. |

Responses, findings and revisions are recorded here rather than inferred from
delivery or silence.

### GCP lead findings and draft r3 dispositions

| Finding | Disposition |
| --- | --- |
| F1: shared evaluator release coupling | Accept. First profile shares independently versioned contracts and conformance vectors, not evaluator code. Domain policies remain distinct; equal decisions are required only for equal semantic inputs and policy domains. |
| F2: unnamed entry owner and allegedly forbidden durable journal | Accept the ownership precision, reject the claim that extending canonical persistence necessarily creates a second authority. Name MeerkatMachine, ToolDispatchAdmission as its integration seam, operation/detached owners and completion/outbox contracts. Require generated models, retain pre-entry evidence for protected reads and disclosures, and permit physical group commit. |
| F3: revoked source poisons long-lived context; unbounded rechecks | Accept the missing recovery path. Add a fresh context segment with no inherited content, not a sanitizing summary. Allow snapshot/change protocols with coverage proofs and gap recovery; notifications alone do not establish freshness. |
| F4: evidence-pending continuation has no named lifecycle state | Accept. Add proposed orthogonal EvidencePending obligations to existing MeerkatMachine, exact receipt-proof transitions and dependent-continuation guards. A healthy store at entry cannot eliminate a later live append failure. |
| F5: identity/grant sprawl | Accept. Choose trust-domain-qualified PrincipalRef and specify domain mapping obligations without collapsing capability grants into user identity. |
| F6: requester must be nested inside InputOrigin | Reject the proposed conflation, accept the need for one binding. Immediate hop/origin and retained original requester are different facts. Atomically bind them at admission, validate peer and requester evidence separately, and treat legacy authority as unavailable. |
| F7: MobKit still owns protected operations | Accept. Make MobKit enforcement explicit; projection-only applies to facts owned elsewhere. |
| F8: realtime/live path | Accept explicit coverage. Initial buffered profile refuses live attachment before provider connection or context seeding. |
| F9: relation to PR #1333 options | Accept. State the resolved placement and retain peer wiring as a transport/topology control. |

The GCP lead acknowledged that irreversible information release needs durable
pre-release custody and that admission-time store health does not prevent a
post-effect append failure. Its follow-up retained the request for a named
evidence-pending state in the existing generated owner. The later exact-candidate verdicts below record acceptance of these repairs.

### OB3 findings and draft r3 dispositions

| Finding | Disposition |
| --- | --- |
| F1: unclassified human conversation | Accept. A declared message-resource authority supplies policy-backed ingress labels and sender narrowing. Agent ownership is not automatic audience membership. |
| F2: persisted product output bypasses runtime disclosure | Accept. Database/warehouse writes are disclosure or controlled custody; envelopes alone do not govern direct plaintext readers. |
| F3: persistent multi-contributor session has no clean boundary | Accept. Add the fresh-segment reset shared with the GCP repair; no old content, state or late effects transfer implicitly. |
| F4: attribute writers may accidentally grant access | Accept. Access-conferring field writes require grant/release authority; imported labels and heuristics are advisory otherwise. |
| F5: dynamic channels and monitoring copies | Accept the explicit product contract, reject send-time membership alone as sufficient for future readers. Require subsequent-read enforcement or explicit broader release semantics; dynamic audience support remains outside the initial profile. Copies and redirects each require authorization. |
| F6: admin-through-agent confused deputy | Accept explicit statement. Addressing/owning/administering an agent grants no right to contributors' private context. |
| F7: timed-out receipt append may have committed | Accept. Add append-level unresolved outcome, immutable idempotent identity, reconciliation and proof before dependent continuation. |

The draft also incorporates source-service disclosure limits, cross-process
incarnation fencing, receipt group commit, performance budgets and the existing
generator/WASM compatibility requirements. Independent Elephant fixes remain
identified follow-up work; this documentation task does not apply them.

### Homecore findings and draft r3 dispositions

| Finding | Disposition |
| --- | --- |
| F1: private input contaminates shared contexts | Accept audience-bound admission before hydration/transcript inclusion and pre-inference partitioning. Reject the inference that splitting one mixed model response creates disjoint dependencies. Logical agents may keep stable identity across context generations. |
| F2: historical household data cannot migrate | Add explicit, revocable adoption of an immutable legacy bundle by an authority entitled to authorize its use/release. The grant explicitly covers unknown internal dependencies without inventing identities, dropping known restrictions or claiming recovered historical audit. |
| F3: snapshots restore revoked authority | Accept. Activation must consult surviving rollback-resistant authority; copied epochs/credentials are insufficient. Clones are quarantined until new incarnation and processing authority are established. |
| F4: model-assisted classification is not expressible | Add a bounded classifier-service mandate. Any outcome broader than restrictive ingestion is an explicit preauthorized release/trust assumption; an allowed label alone does not prove correct classification or injection resistance. |
| F5: anonymous speakers and physical/external audiences | Add assurance and audience vocabulary, separating authenticated device from unidentified human. These profiles remain unsupported initially; device identity cannot invent human identity. |
| F6: model-relayed human approval | Accept deterministic authenticated ingress capture bound to displayed operation digest and approval generation, consumed through the existing ApprovalLifecycleMachine. |
| F7: alarms fail when ordinary audit storage is full | Permit a reserved durable receipt backend for a fixed, preauthorized notification class. Preserve the same pre-effect evidence, append identity and operation owner; no unaudited fallback or promised life-safety availability. |
| F8: store loss requires re-admission | Retain immutable association/recovery evidence. Require fencing old executors and proof of non-entry or destination reconciliation before owner-authorized re-admission; audit rows alone never become replay authority. |
| F9: relationship facts lack ownership | Add identity/relationship authority, typed scoped relations and revocation dependencies. Configuration can implement that authority if its write, activation and recovery contract is governed. |
| F10: tool catalog stale on resume | Refresh discovery under current metadata authorization. Reject a universal equivalence between discovery and execution: public metadata may remain visible while calls are denied. |

### Toolkit result

Toolkit cleared r2 for architectural compatibility and supplied implementation
acceptance requirements, with an [independent counterreview](reviews/adr-001/toolkit-independent-r2.md).
The draft explicitly covers credential ceremony isolation and independent
waiters sharing preparation; the acceptance suite retains real custom-extension
and deferred-effect obligations. Toolkit received the revised candidates because its previous verdict does not
certify later text.

### Exact r3 re-review

All four project reviewers received the complete frozen r3 candidate and their
finding dispositions via agent bus on 2026-09-30. The request asked them to
check both closure and regressions; no r2 verdict is treated as approval of r3.

| Reviewer | r3 result |
| --- | --- |
| GCP Meerkat/MobKit lead | [GREEN on r3](reviews/adr-001/gcp-lead-r3.md); four requested clarifications to apply before acceptance. |
| Homecore | [GREEN on r3](reviews/adr-001/homecore-r3.md); four nonblocking profile clarifications under reconciliation. |
| OB3 | [GREEN on r3](reviews/adr-001/ob3-r3.md); two nonblocking precision edits incorporated into r4. |
| Meerkat Toolkit | [GREEN on r3](reviews/adr-001/toolkit-r3.md); no new findings. |

### Final profile clarifications after r3

Homecore cleared the original blockers and raised four nonblocking precision
items. The next revision addresses them without weakening runtime invariants:

- Individually controlled external accounts have an explicit stable-principal
  release contract covering provider retention and authorized devices. Shared
  accounts/groups do not acquire that contract implicitly.
- Adoption rights cannot come from custody, operator/admin status or generic
  guardianship. Proven per-person corpora retain their privacy floor; broadening
  requires applicable release rights. Participant union does not prove access
  to every participant's contribution.
- Initial classifier attempts use fresh per-item contexts. Labels are derived
  outputs; source/class equality does not establish independence in a batch.
- Alarms during quarantine need surviving current-mandate evidence outside the
  reverted snapshot and ordinary same-domain fencing. Missing proof refuses
  alarms too; a reserve is not a revocation bypass.

R4 moved the deployment-specific contracts and integration cases to the
companion, required same-domain entry fences, made numeric budget declaration a
pre-implementation gate, clarified classification-pending admission, and stated
the initial no-live-voice consequence. Both [authority](reviews/adr-001/adr-001-authority-r4.md)
and [information](reviews/adr-001/adr-001-information-r4.md) delta reviews were
GREEN. The next exact-version review covers the Homecore clarifications above.

### Exact r5 pair review

The frozen [r5 ADR](reviews/adr-001/candidate-r5.md) and
[r5 deployment profiles](reviews/adr-001/candidate-r5-profiles.md), with their
[machine-readable manifest](reviews/adr-001/candidate-r5-manifest.json), are the
r5 pair submitted to all four project reviewers. The scope column identifies
15 Core and 13 Profile conformance cases; enabled behavior cannot opt out by
using a different profile name.

The [authority review and final-hash addendum](reviews/adr-001/adr-001-authority-r5.md)
are GREEN. The [information review](reviews/adr-001/adr-001-information-r5.md)
cleared the same substantive text before the mechanical scope-column addition.

| Reviewer | r5 exact-pair verdict |
| --- | --- |
| GCP Meerkat/MobKit lead | [GREEN on both r5 hashes](reviews/adr-001/gcp-lead-r5.md). |
| Homecore | [GREEN on both r5 hashes](reviews/adr-001/homecore-r5.md); domain definition and Core test specificity carried into the final delta. |
| OB3 | [GREEN on both r5 hashes](reviews/adr-001/ob3-r5.md). |
| Meerkat Toolkit | Asked to combine the r5 review and final r6 delta into one final verdict. |

## Final r6 candidate and delta review

The final pair consists of the [frozen r6 ADR](reviews/adr-001/candidate-r6.md)
and [frozen r6 profiles](reviews/adr-001/candidate-r6-profiles.md), identified by
this [manifest](reviews/adr-001/candidate-r6-manifest.json). The complete
[r5-to-r6 patch](reviews/adr-001/candidate-r5-to-r6.patch) was supplied to all
four bus reviewers with the final hashes. No earlier verdict is silently
carried forward to different bytes.

The final delta resolves Homecore N5 and D1 from its [r4 review](reviews/adr-001/homecore-r4.md)
and subsequent clarification, the GCP lead's domain clarification,
and OB3's profile-applicability precision. Domains are determined for each
canonical authority and its state. A handle exposing Meerkat-owned grant state
must use its owner-side fence, while Elephant can retain independent resource
policy and entry authority when it consumes that grant. Reachable governed
behavior determines profile obligations. The matrix now contains 19 Core and
13 Profile cases, with explicit admission, destination and domain tests.

The [final internal delta review](reviews/adr-001/adr-001-final-r6.md) is GREEN
on both r6 hashes. It initially found that service-wide wording could collapse
Elephant's independent authority merely because it consumes a Meerkat parent
grant. The author applied the fact-scoped repair and the reviewer verified
closure against the final bytes; both the finding and closure are preserved.

| Reviewer | Final r6 verdict |
| --- | --- |
| GCP Meerkat/MobKit lead | [GREEN on both final hashes](reviews/adr-001/gcp-lead-r6.md); no remaining findings. |
| Homecore | [GREEN on both final hashes](reviews/adr-001/homecore-r6.md); no remaining material findings. |
| OB3 | [GREEN on both final hashes](reviews/adr-001/ob3-r6.md); no remaining material findings. |
| Meerkat Toolkit | [GREEN_DESIGN_ONLY on both final hashes](reviews/adr-001/toolkit-r6.md); no remaining material architectural findings in its reviewed scope. |

Toolkit also supplied an [independent review through r5](reviews/adr-001/toolkit-independent-r5.md).
Its final r5-to-r6 delta was reviewed by the Toolkit lead, not that subagent.
The final [internal r6 review](reviews/adr-001/adr-001-final-r6.md) was a separate
subagent check of that delta. Each report retains its actual scope.

## Validation and remaining limits

The final candidate passed `make docs-check`: 116 public pages and all 54
supporting script tests. The ADR and its companion are internal documents;
the additional local link, hash and whitespace checks cover these artifacts.
All 17 pinned source references resolve to the recorded commit, file and line.
Local links and anchors, exact snapshot hashes, whitespace and ASCII dash
punctuation passed for the authored documents. Raw reviewer messages retain
their original wording and punctuation.

Only documentation changed. No runtime security tests, live policy migration,
credential exchange or cross-host enforcement have been executed by this work.
The first implementation gate remains a production-owner tracer and one
governed end-to-end vertical slice with actual sink and fault assertions.
