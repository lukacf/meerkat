# ADR-001 information and identity review, draft r1

Verdict: GREEN for this review scope. No independently confirmed material design gap. This is a design verdict, not implementation acceptance or evidence that current binaries enforce the proposal.

Reviewed file: `(operator-retained path)/docs/internal/design/adr-001-runtime-security.md`, draft with 444 lines, on 2026-09-30.

Source baseline verified locally: Meerkat `54d14e91bd426fcaaafc156227b0b5ab0e23d9a6`, matching ADR lines 50-52. I read the canonical Meerkat dogma and the commentary for singular authority, mechanical projections, and provider/policy seams. I did not edit the ADR or repository source, run builds, or execute security tests.

## Material findings

None. There are no I1-style RED findings in this scope.

The draft does not merely state that ABAC exists. It assigns authorities, defines conjunctive decision composition, requires typed failure for missing authority or unsupported enforcement, scopes the initial profile, and ties its claims to future sink assertions. Several attractive counterexamples below would violate explicit text already present. Reporting them as missing requirements would be a false positive.

## Adversarial cases and disposition

### 1. Authenticated issuer claims another domain's identity or clearance

Attack: a valid token from issuer A names a principal in domain B, or a legitimate Elephant resource service returns a clearance assertion for an unrelated user. An implementation that accepts any signed claim from any registered issuer would elevate authority.

The draft rejects this. ADR lines 73-75 assign identity and security attributes to their respective authorities. Lines 104-110 restrict resource providers to registered domains and forbid minting unrelated identities or lowering another source's restrictions. Lines 131-135 require issuer, audience, lifetime, proof, canonical trust-domain-qualified identity, and separate workload/human authentication. The Elephant protocol explicitly validates the issuer's right to attest each claim at lines 276-281.

The draft leaves the credential exchange format and key rotation for implementation at lines 415-416. That is a legitimate deferred mechanism, not absence of the trust rule. Source confirmation: `crates/meerkat-core/src/auth/principal.rs:26-42` and `:69-92` provide typed principal references and acting-on-behalf-of relationships, but do not themselves authenticate an issuer. The ADR correctly does not present these existing structs as proof of authentication.

Disposition: covered. The acceptance test at line 385 should include a correctly signed but wrong-domain/wrong-claim-authority token, not only a bad signature.

### 2. A privileged helper becomes a confused deputy

Attack: Alice has a bounded read mandate, delegates to a service agent with a broad credential, and the helper uses its ambient credential to read or publish data outside Alice's authority. Another variant labels the continuation System to discard the human restrictions.

ADR lines 88-94 require the conjunction of original mandate, delegation restrictions, executor ceiling, current policy, resource policy, destination policy, and obligations. Lines 137-160 preserve actor/requester distinction and original provenance, constrain delegation monotonically, and explicitly refuse transfer of unrelated executor authority. Lines 162-168 require separately authorized service and recurring mandates. Lines 276-281 prevent a general static service token from substituting for the caller.

Disposition: covered by an explicit rule, with a concrete sink test at line 386. The rule is about the permission actually used, so recording Alice's name in a receipt while calling Elephant as an unrestricted service would fail it.

### 3. Mutable attributes or caller-controlled labels create clearance

Attack: the caller sets a mutable label, purpose field, or JSON attribute to a privileged value; a leaf adapter turns missing or stale data into an unrestricted scope; a record's subject entity is treated as its authenticated reader.

ADR lines 75, 91-100, and 172-178 require authoritative resource resolution, versioned feature-owned declarations, authorized attribute sources, freshness evidence, and refusal for unknown authority. Lines 152-157 distinguish absence, unknown, unrestricted, and empty scope. Lines 286-288 explicitly separate knowledge subjects from authenticated principals.

Disposition: covered. An adapter that trusts unverified tool JSON contradicts lines 175-178; it is not an implementation choice left open by the ADR.

### 4. Hidden records influence ranking without returning their bodies

Attack: a service filters denied hits after global top-K scoring, returns a score normalized by hidden records, or lets denied results change visible pagination. No secret body is returned, but an observer can infer a denied record's presence or properties.

ADR lines 218-222 expressly constrain records, counts, scores, citations, errors, and pagination even when query execution uses a separate service mandate. Line 393 names denied-source influence on ranking and requires no unauthorized projection or disclosure. The exclusion of timing and traffic analysis at lines 112-117 does not exclude direct changes to returned rankings or scores.

Disposition: covered. A safe implementation may evaluate a caller-authorized candidate set or carry the relevant dependency to a release decision; simple post-filtering is not automatically conformant. The ADR need not select a search algorithm to reject the counterexample.

### 5. Summary, embedding, or cache launders a revoked source

Attack: a secret appears in a transcript, is summarized or embedded, and then its original message is removed. A later caller can read the summary or retrieve a cached answer. Another variant claims that an uncited source did not contribute even though the model saw it.

ADR lines 224-238 require resource-authoritative provenance, the conjunction of source restrictions, dependency union, explicit unknown contributors, separate declassification, and dependency revalidation. Lines 240-260 require authorization of the selected context and final release, initially using a complete context domain; they specifically reject deleting a forbidden message after it influenced a summary. Line 393 names summaries, caches, memory, and later callers.

Source confirmation: the existing compaction contract at `crates/meerkat-core/src/compact.rs:154-178` identifies the rebuilt summary and exact retained/discarded transcript positions. The validator at `crates/meerkat-core/src/agent/compact.rs:186-193` forbids retaining a prior compaction summary as if it were an ordinary retained row, and `:223-226` requires a disposition for every source row. These are usable provenance seams, not a present authorization guarantee. The ADR does not confuse them with proof that confidentiality dependencies already survive compaction.

Disposition: covered. At the initial profile's whole-context granularity, all data supplied to a model or curator must remain in the output's conservative dependency domain; the model's citation list cannot certify independence. A narrower dependency rule would require the separately specified later profile or authorized transformation.

### 6. Provider-held context is absent from the current request body

Attack: a fresh-looking local prompt references a remote cache or persistent provider session that still contains confidential context. Checking only locally serialized messages would miss data used by the model, and a cached answer could reach a newly unauthorized caller after source revocation.

This is a real inventory hazard in the existing source. `crates/meerkat-core/src/lifecycle/run_primitive.rs:785-787` exposes a Gemini cached-content resource name. `crates/meerkat-gemini/src/client.rs:671-673` serializes that reference. `crates/meerkat-core/src/agent/runner.rs:1596-1614` explicitly recognizes that provider-held cached content can own the request's system instruction.

ADR lines 212-238 include histories, injected context, caches, unknown dependencies, and source revalidation. Lines 240-245 require per-attempt data authorization and provider-route authorization; lines 253-257 require the complete selected context domain. An opaque remote cache with unknown contributing data cannot satisfy that requirement merely because its handle and current local messages are authorized. It must supply the required authority/provenance or be unavailable under lines 119-124.

Disposition: covered as an invariant. Add an explicit acceptance fixture so implementers do not confuse complete context with current request bytes. This is a nonblocking clarification described below, not a request to implement cache introspection before accepting the ADR.

### 7. Static destination becomes a larger downstream audience

Attack: a tool writes an artifact to a shared location, or publishes to a channel whose membership grows after admission. The service account or channel identifier is incorrectly treated as the only recipient. A group member added later obtains data even though the original set of recipients was authorized.

ADR lines 137-150 persist the explicit disclosure association. Lines 172-186 bind permits to the exact destination and contributing data. Lines 247-260 require actual recipient authorization and explicitly leave dynamic audiences unsupported in the first profile. The companion `caller-context.md:89` further states that a frozen audience cannot silently become current channel membership.

Disposition: covered for the initial profile. A destination whose reader set cannot be fixed or whose future readers cannot be governed is an unsupported dynamic audience; authenticating the storage processor alone does not make it supported. Later publication capabilities must specify the audience authority and membership-change behavior before enabling this case, as ADR lines 419-420 require.

### 8. Revoke the root but leave a child token nominally valid

Attack: G0 delegates to G1, G1 delegates to G2, and only G0 is revoked. The leaf has an unexpired token and unchanged local generation, so a destination validates only G2 and proceeds. A queued schedule or cold restart makes this easier to miss.

Read together, ADR lines 74, 88-94, 139, 147-160, and 188-208 reject this result: the original mandate remains an applicable constraint, delegation does not replace it, and new uses require current authority. Root provenance alone would not be sufficient, but the proposal requires both provenance and effective current constraints. The remote case is bounded by the declared freshness profile, rather than promising impossible global instantaneous revocation.

Disposition: covered in architectural meaning. The acceptance suite should explicitly distinguish ancestor-only revocation from leaf revocation. This is a valuable nonblocking precision improvement because a leaf-only implementation can superficially appear to satisfy a generic revocation test.

### 9. Trusted host or ambient shell bypasses the controller

Attack: an in-process plugin with direct store/network access or a model-directed shell command reads a secret outside the declared resource operations and sends it through ambient credentials. A tool-name ACL does not mediate the actual effect.

ADR lines 104-117 expressly place unrestricted native code inside the trusted boundary, require process/filesystem/network/credential confinement for adversarial containment, and deny a confinement claim based on a shell tool-name ACL. Lines 119-124 and 240-245 refuse unsupported governed paths. Lines 355-357 restate the bounded claim.

Disposition: covered. This is not a sandbox ADR, and it does not claim that wrapping a tool name makes arbitrary code safe. Its guarantees depend on trusted operation owners mediating the effects they advertise; hostile code must use a separately specified isolation profile or remain unsupported.

## Nonblocking precision improvements

These would make the future conformance suite less ambiguous. They are not material omissions requiring a RED verdict.

- N1, ancestor validity: add a sentence near lines 152-160 that a derived grant's effective validity depends on the live validity of every required ancestor, bounded by the declared remote freshness contract. Add an ancestor-only revocation case with root and intermediate revocations, leaf nominally valid, queued work, cold restart, and descendant schedule occurrences. Assert no new read/send after the relevant fence or remote bound.
- N2, complete model context: near lines 240-245, explicitly include provider-held caches, persistent model sessions, and referenced remote context in the context-use decision. Add a fixture where a remote cache holds a sentinel absent from current local messages; after source denial or a caller change, no provider invocation using that cache and no cached response release may occur. Refusal is an acceptable initial implementation.
- N3, conservative derivation: near lines 231-238, state directly that the initial whole-context profile attaches the complete selected dependency domain to model-produced summaries, answers, tool arguments, and artifacts, independent of model citations or claimed relevance. Add an uncited secret-in-context fixture and a host-supplied curator fixture. Neither may lose the dependency without the separately authorized transformation.

## Dogma assessment

The declared owners are compatible with singular authority and feature ownership. Ingress attests identity; grant authority owns validity; resource authorities own classification; shared composition combines constraints; existing operation owners retain effect entry and settlement. The policy engine is not made an executor or a universal lifecycle machine. Caches and the MobKit console remain projections. Existing typed principals are not promoted into authenticated claims merely because they serialize.

The implementation can still violate this architecture by guessing dependency sets, checking only a leaf grant, treating a remote context handle as harmless metadata, or letting a destination adapter choose its own audience semantics. The proposal forbids those results and requires production-path, actual-sink tests at lines 379-404. Those future tests are the correct next evidence. Their absence today is not a contradiction in a document explicitly marked Proposed at lines 5-8.
