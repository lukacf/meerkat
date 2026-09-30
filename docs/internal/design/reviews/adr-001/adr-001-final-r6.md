# ADR-001 r6 bounded delta review

Final verdict: GREEN for the repaired hashes recorded in the closure below. D1 is closed. The preliminary finding is preserved as review history.

Initial verdict: ISSUE - one authority-domain scope clarification was needed before freezing. No other core weakening or domain loophole was found in the r5-to-r6 delta. This was not a request to reopen the architecture.

Reviewed files and exact hashes:

- docs/internal/design/adr-001-runtime-security.md: 69c687a748effff1f11312c2b54883818851fb12a9c9458db6b1063d92b40d04
- docs/internal/design/governed-deployment-profiles.md: 3ccb3321128c5eb68b5873756ffcfc8b847a91c20732fb8289dfd5d23ec5dfad

Compared only against frozen candidate-r5.md and candidate-r5-profiles.md, with surrounding sections inspected to resolve meaning. No source or ADR edits, builds, tests or bus messages.

## D1 - classify each authority/state, not an entire mixed-ownership service

Location: ADR lines 395-411, specifically 402-403. Related conformance row: deployment profiles line 195.

The new rule correctly says domain classification follows canonical ownership and entry/revocation state, not hostname, storage packaging or consumption. However, the sentence "If a service's grant or revocation state is actually owned by Meerkat" is broader than the intended authority-scoped rule.

Counterexample: Elephant independently owns its durable resource-policy state and resource-entry fence, while it validates an authenticated parent delegation issued by Meerkat. The parent grant's owner remains Meerkat. The current sentence can be read to classify all of Elephant as same-domain because one grant in its effective conjunction is Meerkat-owned. This contradicts the preceding independence rule and wrongly turns evidence consumption into a merger of authorities.

Smallest repair: replace the service-wide sentence with authority-scoped language, for example:

> Domain classification applies to each canonical authority and its state. A separate service handle or adapter that exposes Meerkat-owned grant or revocation state remains a same-domain participant for that state; code or storage packaging does not establish an independent authority. An independently owned Elephant resource-policy and entry authority remains independent when it consumes a Meerkat-issued delegation. The parent grant remains Meerkat-owned and fenced in Meerkat's domain; cross-domain consumption of its evidence follows the declared authenticated protocol and freshness bound, without weakening that owner-side fence.

In the profile row, test an "adapter presenting Meerkat-owned grant/epoch state as an independent authority" rather than implicitly classifying a whole service. Add or incorporate the mixed case: an independent Elephant resource authority consuming a Meerkat parent grant retains both owners and their respective entry/freshness obligations.

This preserves both desired outcomes:

1. A wrapper or new deployment around Meerkat-owned mutable grant state cannot claim independence to replace the required fence with a lease.
2. An independent resource authority does not merge into Meerkat merely by accepting Meerkat-authenticated delegation evidence. Its separate domain does not let Meerkat skip its own parent-grant fence.

## Other delta checks

- ADR line 231 explicitly restores refusal for unsupported identity/audience hydration and release. No relaxation found.
- ADR lines 341-345 still require same-domain participants to fence and prohibit adapter reclassification. Lines 386-393 retain destination entry, declared freshness and fail-closed unknown freshness.
- ADR lines 405-411 preserve in-process tool-policy provider and replica fencing, including rolling deployments. Notifications remain hints unless an acknowledged ordering protocol prevents stale entry.
- ADR line 753 extends measured acceptance budgets to the lowest-resource advertised host class. This tightens the enablement gate.
- Profile lines 178-188 bind applicability to reachable governed behavior, not profile naming. Omitting an optional label cannot exempt a reachable warehouse, send or imported-attribute path from conformance.
- Profile lines 192-194 make classification/identity/audience refusal, segment compatibility and all reachable destination guarantees explicit Core rows. Restricted ingestion does not bypass a missing/unmapped authority; this agrees with profile lines 51-54.

After D1's wording is scoped to the specific canonical authority/state, this bounded delta is GREEN. No further architecture or product-contract change is requested.

## Final closure after D1 repair

Final verdict: GREEN. Verified the exact repaired files:

- docs/internal/design/adr-001-runtime-security.md: 2c3744f703ea0f58f3cbd6cd02103e0539eadb5eafac4cd6a48de34f0a38c6fe
- docs/internal/design/governed-deployment-profiles.md: 05ed1bd4202f4d53d5b7a5f83af052c6806a1809a6b8e152a060df74e294cc53

ADR lines 402-406 now assign domains per canonical authority/state contract. A handle to Meerkat-owned grant or revocation state stays same-domain for that state. Consuming a Meerkat parent grant explicitly does not transfer ownership of Elephant's resource policy or entry. Lines 408-414 continue to require same-domain provider/replica fencing and prohibit replacing that fence with an independent-domain lease.

Deployment-profile line 195 now tests the false claim by a handle, while explicitly preserving independent Elephant resource authority when it consumes a Meerkat parent grant. This closes the mixed-ownership counterexample without giving wrappers a way to avoid the Meerkat owner-side fence.

The full r5-to-final diff was checked again and contains only the previously reviewed bounded changes plus this repair. No remaining core weakening or domain-classification loophole was found in this delta. No further repair is requested. This is design-delta review evidence only; production conformance and performance gates remain required.
