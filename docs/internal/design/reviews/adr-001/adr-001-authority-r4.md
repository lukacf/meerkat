# ADR-001 r4 bounded authority review

Verdict: GREEN. No material regression found in the requested deltas or the split into the deployment-profile companion. No mandatory revision requested.

Reviewed exact files:

- `docs/internal/design/adr-001-runtime-security.md`: 845 lines; SHA-256 `246ba64f62ba1cdf8022a27da0f275664dc1d4e0e4034dbe24114ab9f7f814b3`.
- `docs/internal/design/governed-deployment-profiles.md`: 177 lines; SHA-256 `b2e39c432201ce3fe2f4df4f6a53e83193f0cdfdeefd22a0590c85f82c68dbe9`.
- Compared with frozen `docs/internal/design/reviews/adr-001/candidate-r3.md`: 904 lines; SHA-256 `a242a21407fbf52d2ca5615edfdd6b5649b40fa6f73c959d4857eddd8e2db87e`.

Scope: requested delta review and preservation check only, not a new full architecture review or implementation acceptance. No source/ADR edits or tests were performed.

## Delta findings

1. Same-domain fences are now mandatory. Main ADR lines 340-344 require participation or refusal, explicitly including in-process authorities. An adapter cannot recategorize a local authority to obtain a lease escape. Lines 384-400 qualify weaker freshness to independent authority domains while retaining the multi-process/replica coordination rule. Dependency validation at 452-459 and snapshot/change validation at 529-536 explicitly refer back to section 4. I found no remaining independent local lease permission in those paths. This strengthens the accepted r3 contract.

2. The split retains binding effect. Main ADR lines 21-25 make the companion part of the proposal and forbid weakening runtime invariants. Each shortened section retains its runtime rule and links to the applicable companion contract. Companion lines 10-15 require authority and conformance evidence for optional profiles, prohibit silent trusted-embedded fallback, and retain unsupported initial-profile exclusions. Main lines 793-795 plus companion lines 141-144 distinguish operation-level acceptance from optional profile enablement; a core operation cannot evade its tests merely because the example moved to a companion.

3. Mechanical preservation checks passed. All 24 integration conformance rows are present unchanged in the companion. The moved identity-assurance, stored/external-disclosure, classification/legacy-adoption and reserved-notification blocks are present verbatim. All section anchors used by main-document companion links resolve. Conversation/triage rules retain authenticated ingress, no inferred owner disclosure right, pre-inference separation and complete mixed-context dependencies. Imported-attribute examples retain the core rule that controlling a value does not confer power to grant rights through it.

4. Required performance budgets now precede implementation. Main lines 736-741 require numeric p50/p95 overhead and minimum receipt throughput, a stated parallel attempt count and workload/environment assumptions; an unset budget blocks the slice. Existing measured vertical-slice validation remains. This does not relax per-attempt evidence or permit executing before group-committed evidence.

5. Initial live-voice exclusion is explicit. Main lines 719-720 state the product consequence directly; the pre-provider-connection refusal remains in section 5. Companion line 15 retains initial unavailability. Moving deployment content did not enable an attachment route through a different profile name.

6. Classification-pending admission is narrower and coherent. Main lines 492-496 and companion lines 51-54 distinguish a mapped message with an authorized restricted-ingestion policy from a missing authority or unmapped class. Only the former may await classification in that restricted context. This closes the reading that a generic restricted context can admit arbitrary ungoverned input.

7. Broad-service-credential limits now apply generally. The original three-sentence rule moved unchanged from Elephant federation to main lines 423-425. It still requires recipient-specific authorization or an explicitly releasable projection and denies any implication from service access to downstream read rights. Federation still composes its own authority with runtime authorization.

## Preserved authority and recovery boundaries

The delta does not remove or weaken the r3 requirements for canonical generated owners, orthogonal evidence obligations, known-outcome versus Unknown recovery, append identity/reconciliation, snapshot antirollback, fresh-reset generation routing, owner-store-loss fencing, or no replay from audit alone. The reserved-notification detail remains verbatim in companion lines 127-137, and main lines 679-682 expressly require the same durability, authorization, identity and owner-custody contract. Optional selection therefore creates no unaudited or audit-driven execution path.

This GREEN verdict applies to the two hashes above. Production ownership, fault/sink assertions and conformance gates remain outstanding implementation obligations.
