# ADR-001 r3 independent federation and identity review

Verdict: GREEN for returning this architecture candidate to the project reviewers. No material contradiction or missing security boundary found within this review's scope. This is design review evidence, not an implementation, performance, migration or security-conformance acceptance.

Reviewed candidate:

- File: /Users/luka/.codex/worktrees/security-adr/meerkat/docs/internal/design/adr-001-runtime-security.md
- Lines: 904
- SHA-256: a242a21407fbf52d2ca5615edfdd6b5649b40fa6f73c959d4857eddd8e2db87e
- Read all 904 lines. Hash was checked again after review and remained unchanged.
- Source baseline: Meerkat 54d14e91bd426fcaaafc156227b0b5ab0e23d9a6; Elephant 1aefaad2de68535ca5b588f58e8ed1ae1bf5b56a.
- Scope: canonical principal mapping, InputOrigin independence, shared protocol and versioning, Elephant ABAC and commissioning preservation, distributed validity, and contradictions introduced by additional product contracts.
- Static inspection only. No build, tests, source edits, ADR edits or bus messages.

## External review dispositions

### F1 - shared implementation release coupling: closed

ADR 60-83 explicitly makes the first integration a shared protocol and semantic corpus, not shared evaluator implementation. The protocol is independently versioned and incompatible semantics fail negotiation. Vectors compare the same normalized inputs, domain semantics and contract version; different policies may correctly decide differently. ADR 761-763 records the actual remaining cost: maintaining conformance and negotiated compatibility, rather than forced lockstep application upgrades.

This preserves independent resource-policy ownership. Elephant's existing policy crate depends on elephant-types, not Meerkat (Elephant crates/policy/Cargo.toml:9-14). A future code-sharing decision remains subject to independent compatibility and dependency rules rather than being smuggled into this first profile.

### F5 - principal/grant vocabulary: closed at architecture level

ADR 199-216 selects the existing PrincipalRef vocabulary with explicit trust-domain qualification, maps projections and domain capabilities, and rejects implicit governed Owner privilege. The mapping preserves meaningful differences:

- Existing PrincipalRef is an identity reference, currently kind plus id (Meerkat crates/meerkat-core/src/auth/principal.rs:69-84).
- ActingOnBehalfOf is a relationship, not an authenticated delegating grant (same file:86-99).
- AuthGrant performs exact principal/action/scope/delegation comparisons (same file:194-213). ADR 208 retains those checks while requiring separate live evidence.
- MobToolAuthorityContext explicitly is a capability contract, not an identity model (crates/meerkat-core/src/service/mod.rs:1021-1044).
- The principal and agent control lanes are explicitly distinct (crates/meerkat-mob/src/control_policy.rs:11-14), and the machine principal key already has one conversion owner (same file:333-340).
- ForkedParticipantGrant is an attach-admission result, not a universal principal or delegating token (crates/meerkat-mob/src/forked_participant/types.rs:803-819).
- Elephant policy::Principal is an authorization attribute view containing space, scopes, clearances, subjects and purpose (Elephant crates/policy/src/engine.rs:23-38).

The ADR now supplies the semantic mapping that the implementation tracer must prove. It does not incorrectly replace all domain grants with one universal grant machine.

### F6 - InputOrigin versus requester: correctly resolved without conflation

ADR 256-265 retains immediate origin and binds it atomically to one execution-authority association. Immediate peer authentication and retained requester/lineage verification are distinct. Legacy origin survives, but no authenticated authority is invented.

This is the correct source model. InputOrigin includes Operator, Peer, Flow, System and External (Meerkat crates/meerkat-runtime/src/input.rs:58-84). The comms bridge obtains the immediate canonical peer from classified ingress before creating the input (crates/meerkat-runtime/src/comms_bridge.rs:128-165). A detached completion is System-originated even when its causal work belongs to a human mandate (crates/meerkat-runtime/src/input.rs:347-373). Making source equal requester would erase one of these facts.

Counterexample considered: Alice commissions agent A, A forwards to peer B, and B's detached job delivers a System completion. R3 preserves Alice, each immediate origin, exact work identities and the grant lineage, with contradiction refusal and no System privilege expansion.

### F7 - MobKit operations: closed

ADR 63-66 and 137-140 retain MobKit enforcement for HTTP/SSE/blob and memory delivery. ADR 787-789 limits replacement by projections to Meerkat-owned facts. The common contract does not remove the operation owner or create two owners for the same policy fact.

### F9 - relation to caller-context alternatives: closed

ADR 10-19 states the selected placement and retains peer wiring as a transport/topology constraint. This makes the superseding relationship explicit without treating peer topology as information authorization.

## Elephant preservation and federation

No regression from the previous r2 closure found:

- ADR 577-587 retains Elephant resource authority and conformance-tested domain semantics.
- ADR 589-595 preserves literal manage:wiki commissioning, unrestricted-subject management and separate protected output authorization. Source: Elephant crates/mcp/src/handlers/knowledge_views.rs:31-48. The generic commissioning contract at ADR 304-314 allows bounded service processing without requiring the requester to read all internal inputs and without transferring reusable service authority.
- ADR 614-622 distinguishes Some(empty), None, named-subject membership, completeness and the explicit subjectless waiver. Source: Elephant crates/policy/src/engine.rs:152-199.
- The waiver cannot be unioned from one source into another. This matches Elephant crates/pipeline/src/wiki_editorial.rs:4591-4596. The record-policy join also preserves policy-specific handling rather than treating all policy identifiers as interchangeable (same file:4607-4621).
- ADR 597-602 requires claim-by-claim issuer authority, exact space and constrained credential exchange; authenticated transport does not authorize arbitrary Elephant clearances.
- ADR 604-612 preserves protected result envelopes and local receipts. Broad service credentials do not create downstream recipient entitlement.
- ADR 432-438 keeps sensitive metadata outside normal model/UI/log payloads unless separately disclosed. An opaque dependency handle remains resolvable by the trusted enforcement boundary.

Concrete attacks checked: a wildcard replacing manage:wiki; Some(empty) becoming unrestricted; a named disallowed subject slipping through subjectless_ok; one contributor supplying another's waiver; a Meerkat issuer asserting Elephant clearance outside its grant; a service credential becoming a human read token; and sensitive source IDs leaking through a safe-refusal path. All are ruled out by explicit contracts rather than an implied prompt rule.

## Added product contracts and feasibility

The new contracts are coherent with the core design:

1. Distributed validity remains bounded. ADR 334-343 requires actual participation for the claimed local fence; 384-398 declares remote freshness and replica/incarnation boundaries. Notifications alone cannot create currentness. ADR 538-545 requires snapshot coverage, cursor monotonicity, gap recovery and an entry barrier or explicit lease. A dropped update cannot silently preserve an old Allow.
2. Session recovery does not declassify. ADR 520-536 creates a genuinely fresh segment and fences generation routing; old work remains with its old owner. No old transcript, summaries, caches or late results are imported merely by using the same logical agent identity.
3. Source revocation still works. ADR 451-458 treats historical bytes/receipts separately from current security state. A new context answers independent work without silently reusing the revoked source.
4. Legacy adoption is an explicit authority transfer with stated limits. ADR 563-573 requires enumerated immutable bytes, rights over the corpus, declared treatment of unknown internals, retained known restrictions and bundle-level revocation. It expressly does not promise source-specific revocation for unknown sources or revive old execution authority.
5. Classifier discretion is explicit. ADR 551-562 requires an authorized outcome bound and identifies broadening as a preauthorized release assumption. It does not claim model classification is correct or immune to prompt injection. Restricted proposals compose by permissions, including exception semantics.
6. Product facts cannot mint rights casually. ADR 142-153 makes access-conferring attribute writes separately authorized; relationship evidence remains distinct from universal permission. ADR 408-418 gives messages an actual resource-policy owner. ADR 497-518 prevents mixed inference from erasing provenance by splitting output objects later.
7. Custody and audience remain real boundaries. ADR 480-495 covers direct plaintext readers, future retained-history audiences and secondary destinations. A label beside an unguarded warehouse row is not enforcement. Initial unsupported audience profiles refuse instead of pretending to provide recipient guarantees.
8. New durable storage is permitted without new execution authority. ADR 643-650 resolves the earlier journal contradiction. ADR 354-376 names existing session owners and proposed EvidencePending obligations; 669-710 distinguishes append uncertainty, known outcomes, lost outcomes and safe reconciliation. The receipt journal cannot independently schedule execution.
9. Restore and alarms do not create backdoors. ADR 185-195 requires a surviving current authority and clone admission; 712-722 permits a separately provisioned, preauthorized, durable alarm path with bounded capacity and no unaudited fallback.

The architecture is demanding but no longer claims that existing process-local admission alone provides these guarantees. ADR 771-785 requires generated composition modeling, production-owner tracing and measured latency/throughput/dependency-validation budgets before enablement. Exact implementation placement, schema and resource costs remain work to prove, not facts this review can certify.

## Implementation evidence still required, not new ADR blockers

- Exercise the same corpus across different application versions with both compatible and incompatible semantic profiles.
- Prove one trust-domain-qualified requester representation through restart and all control-lane mappings; reject ambiguous legacy identifiers rather than choosing an issuer heuristically.
- Exercise the Alice -> peer -> System lineage case and contradictory immediate-hop binding.
- Run the full Elephant scope/clearance/subject/completeness/waiver/commissioning matrix. Surface-specific scope normalization must be captured deliberately in the declared profile rather than accidentally homogenized.
- Use actual distributed or replica authority mechanisms for revoke-versus-entry and stale-incarnation tests; a test-only shared lock is insufficient.
- Check fresh reset and adoption with hidden source identifiers, provider-held context, outstanding work and unknown historical authorship.
- Measure costs at the stated workload budgets. Green design review is not proof that the mandatory durability profile meets production latency budgets.

No mandatory text repair is requested from this reviewer before circulating r3.
