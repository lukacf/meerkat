# ADR-001 information and identity review, bounded r4 revision

Verdict: GREEN. The split into runtime ADR and deployment-profile companion preserves the reviewed information obligations and exceptional-transformation constraints. No material regression or implicit widening was found.

Reviewed on 2026-09-30 against `docs/internal/design/reviews/adr-001/candidate-r3.md`. I compared the full main-document diff, read the complete new companion, inspected the retained runtime clauses, and mechanically checked that every moved integration row remains present. No ADR, companion, or source edits were made. No builds or security tests were executed.

## Exact reviewed artifacts

| Artifact | Lines | SHA-256 |
| --- | --- | --- |
| `(operator-retained path)/docs/internal/design/adr-001-runtime-security.md` | 845 | `246ba64f62ba1cdf8022a27da0f275664dc1d4e0e4034dbe24114ab9f7f814b3` |
| `(operator-retained path)/docs/internal/design/governed-deployment-profiles.md` | 177 | `b2e39c432201ce3fe2f4df4f6a53e83193f0cdfdeefd22a0590c85f82c68dbe9` |

This verdict concerns the pair of artifacts at these hashes. The companion is part of the reviewed contract, not optional background explanation.

## Material findings

None. No I-series RED findings.

## Preservation and interaction checks

### The companion remains binding

Main ADR lines 21-25 make the companion part of the proposal and prohibit its conditional contracts from weakening runtime invariants. Main lines 538-543 explicitly bind selection of the exceptional transformations to their companion contracts and forbid implicit selection by ingestion, writing, administration, or model output.

Companion lines 10-15 make applicable conditions normative, require explicit enablement, authority, and conformance evidence for classifier/adoption/notification profiles, and preserve refusal rather than downgrade when support is absent. Companion lines 141-144 distinguish core-operation tests from optional-profile tests; main lines 793-795 explicitly retain both categories where applicable.

Mechanical comparison found all 25 integration-case rows from frozen r3 unchanged in the companion. No acceptance row was lost merely by moving it.

### Pending classification is not a missing-authority bypass

Main lines 410-415 preserve the requirement for a declared message-resource authority. Main lines 488-496 now distinguish an authority-mapped message whose classification is pending from a message with missing authority or an unmapped class. Only the former may enter its declared restricted-ingestion context. Companion lines 44-56 preserve the same distinction along with sender-default and known-source restrictions.

Counterexample checked: an unknown connector supplies private content without a registered classification authority and an adapter calls it pending classification to admit it. That path is explicitly refused; restricted ingestion is not a fallback. The revision strengthens r3's more general unclassified-input wording without preventing authorized classification of mapped inputs.

### Legacy adoption preserves its honest exception

Main lines 441-459 retain normal dependency preservation and current source-authority checks. Lines 538-543 make the bounded exceptional route explicit. Companion lines 113-123 preserve the full r3 adoption contract: explicit corpus authority, immutable enumerated bytes, actor and scope, retention/expiry/revocation, preserved unknown historical authorship/dependencies, explicit grant coverage for those unknowns, preservation of known source restrictions, new authorization for new bytes, derived dependency on the bundle, revocation of later use, no false source-specific revocation guarantee, and no replay authority for old work.

Counterexamples checked: using backup possession as adoption authority; silently declaring unknown sources known; importing a known restricted record to override its authority; appending new bytes under an old adoption; and replaying old work with fabricated requesters. None is permitted by the two-document contract.

The adoption remains an explicit trusted authorization assumption. It does not become a claim that the runtime reconstructed unknown provenance or established consent from unidentified historical actors. Moving it to the companion does not hide or loosen that distinction.

### Classifier broadening remains a release/trust decision

Companion lines 101-112 retain the typed source-scoped mandate, decision authority, source/model-attempt evidence, explicit description of broadening as preauthorized release, and the statement that a lattice does not prove classification correctness or resistance to prompt injection. Restrictive proposals still compose by permission semantics and cannot create permission-bearing exceptions.

Counterexample checked: an injected private email induces the classifier to choose an allowed broad label. The contract still calls that a possible semantic misclassification inside an explicitly authorized discretionary release range, not proof of safe classification. The acceptance case at companion line 162 still requires hard bounds and semantic error behavior to be assessed separately.

Main line 430, stating that governed facts come from resource authority rather than the LLM, remains consistent: the authority commits the classifier proposal under its explicit mandate. The model does not become a grant issuer.

### Partition, reset, and provider-state rules remain runtime invariants

Main lines 488-500 retain audience-bound admission before hydration/transcript write and complete dependencies for mixed-context outputs. Companion lines 58-68 preserve the product examples and the requirement for partitioning before shared inference. Separate JSON outputs still cannot prove independent dependencies.

Main lines 502-527 retain the complete-context first profile, unsupported mixed reuse/batching/streaming, fresh reset with no inherited user/model content or metadata, independent admission, exact ownership of old effects, and binding-generation fencing under a stable logical agent identity. No reset behavior was weakened or delegated to application convention.

Main lines 461-473 retain complete provider-held context inventory and refusal of initial-profile live attachments before connection or seeding. Companion lines 14-15 and main consequences lines 716-722 reinforce rather than relax the initial live-voice limitation.

Counterexamples checked: copying only old user messages on reset; silently keeping remote caches; late result delivery via stable agent name; and post-inference triage splitting. The retained runtime text still refuses each.

### Stored disclosure and future-reader conditions remain complete

Main lines 481-486 retain a general rule for persistent writes and external sends, require all plaintext read paths or authorized effective readership, include retention/future readers/secondary copies, and refuse unsupported guarantees. Companion lines 72-93 preserve the detailed warehouse, direct-IAM, encryption, channel-history, future-membership, monitoring-copy, and redirect cases.

Counterexamples checked: relying only on a label column while direct table readers bypass enforcement, and checking channel membership at send time while later joiners gain old history. Both remain prohibited without the corresponding explicit and enforceable custody/release contract. Initial dynamic audiences remain unavailable.

### Service-token disclosure is now correctly general

The broad-service-credential rule moved from Elephant federation to main lines 423-425. Its meaning is preserved: service access does not establish a downstream recipient's entitlement; recipient-specific authorization or an explicit releasable projection is still required. This now clearly applies to every source using broad service credentials, not only Elephant.

It remains consistent with bounded service commissioning: a service may have separate internal-processing authority, but that does not publish its private intermediates to the commissioning requester.

### Freshness is stricter within a coordination domain

Main lines 340-344 require same-domain authorities, including in-process authorities, to participate in the entry fence or refuse. Only explicitly independent authority domains may use bounded leases; an adapter cannot relabel a local authority to escape the rule. Lines 394-400 include all replicas sharing the resource/work owner and reject notification-only authority.

The downstream dependency clauses are updated consistently: main lines 452-455 and 529-536 refer back to leases permitted by section 4. They do not leave a second generic lease escape for source checks or incremental validation. No companion clause reintroduces same-domain leases.

Counterexample checked: retain an old local source epoch because the change-feed notification has not arrived and the adapter claims a short local lease. The pair of documents now explicitly refuses that construction. Cross-domain leases remain an openly weaker declared contract, not a hidden promise of global instantaneous revocation.

### Reserved notifications remain audited effects

Main lines 679-682 bind the reserved notification backend to the same durability, authorization, identity, and owner-custody contract. Companion lines 127-137 retain exact sources/destinations/payload bounds, current authorization or bounded mandate, capacity/exhaustion behavior, no arbitrary private-context hydration, evidence before dispatch, ordinary effect ownership, idempotent reconciliation, and refusal when authority or durable capacity is absent.

Counterexample checked: an exhausted primary store triggers an unrecorded alarm send, or a replay of spool records sends the alarm again. Neither is authorized. The profile still makes no life-safety delivery guarantee.

## Review conclusion

The bounded r4 revision is a structural split plus clarified or stronger runtime rules. The companion preserves application-specific policy choices while the main ADR retains the mandatory enforcement boundaries and explicitly binds moved contracts and conformance cases.

GREEN means this pair is coherent for further owner review. Optional transformation grants and deployment profiles still require explicit authority, enablement, and actual production-path evidence. Design review does not supply those approvals or implementation proofs.
