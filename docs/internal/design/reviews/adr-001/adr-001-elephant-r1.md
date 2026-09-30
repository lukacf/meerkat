# ADR-001 r1 - Elephant and federation adversarial review

Verdict: RED. The direction is sound, but the current-source authorization contract and the privileged service-handoff contract need explicit decisions before the federation can be implemented consistently. Two additional precision fixes are required to support the stated semantic-preservation claim.

Reviewed ADR: /Users/luka/.codex/worktrees/security-adr/meerkat/docs/internal/design/adr-001-runtime-security.md, r1, 444 lines.
Reviewed Elephant: /Users/luka/src/Elephant at 1aefaad2de68535ca5b588f58e8ed1ae1bf5b56a.
Review mode: static source and design inspection only. No source or ADR edits; no builds/tests. This review file is the sole output mutation.

## E1 - P1 - Historical provenance versions can outlive current source restrictions

ADR lines: 172-185, 224-238, 283-288. In particular, lines 237-238 say persistent dependencies permit revalidation, but do not require current security state for every retained dependency. Lines 173 and 181 bind resource versions without distinguishing immutable content version from mutable access-control state.

Counterexample: Elephant source revision R is readable by Alice at T1. Meerkat stores its content, security envelope and local decision receipt in an artifact and summary. At T2 Elephant reclassifies R's source, removes Alice, or tombstones the source, while R's immutable content revision remains available. At T3 Meerkat validates the exact historical content version and a still-valid runtime mandate, then releases the cached summary. Every named content version and receipt is authentic, but Alice receives data after source permission was withdrawn. A remote lease contract alone does not say which security epoch the lease covers.

Source evidence: Elephant retains immutable knowledge revisions and complete dependency inventories (crates/types/src/records/knowledge_view.rs:448-460). Projection reads currently authorize the view and revision objects, not a generalized fresh dependency closure (crates/mcp/src/handlers/knowledge_views.rs:1978-2002). The ADR is proposing a stronger contract, so the current code does not fill this gap implicitly.

Required repair: distinguish content identity/revision from current authorization state. Each source authority must define the live access-policy/classification/tombstone epoch for an immutable revision and whether deletion revokes retained-copy use. Before use or disclosure, validate every transitive dependency against that current state, or an explicitly bounded authority-issued lease covering it. Missing dependency, deleted authority record or unavailable mandatory authority is not equivalent to a public record. Keep immutable historical policy evidence for audit only; it must not authorize present use. An explicit surviving archival/release grant may define an exception, but a retained revision does not.

Acceptance test: acquire R; produce summary S and artifact A; change only R's source classification or tombstone epoch; restart Meerkat; attempt to use S in a model request, read A, and disclose a cached output. Assert no protected sink receives content without a currently valid covering grant. Include an ancestor dependency changed beneath an unchanged immediate summary revision and a remote-authority outage.

## E2 - P1 - Root-rights conjunction and Elephant's privileged wiki delegation need an explicit bridge

ADR lines: 88-94, 137-160, 162-168, 264-281. Lines 273-274 promise preservation of special service grants, while lines 88-90 and 159-160 can be implemented as requiring the original human's direct rights on every resource.

Counterexample: an Elephant administrator has explicit manage:wiki with unrestricted subject scope, but not ordinary read clearances for every security envelope. Current Elephant deliberately authorizes that user to commission the wiki service, which processes all security envelopes and publishes separately protected results. If the integration intersects the human's direct read rights on every service read, legitimate wiki creation is denied. If it bypasses those checks by replacing the human with a static privileged service token, the model has acquired a generic confused-deputy path. The present text rejects the second result but does not specify the third, correct authority transition.

Source evidence: crates/mcp/src/handlers/knowledge_views.rs:31-48 explicitly declares a space-wide service delegation, denies restricted subjects, and requires literal manage:wiki rather than wildcard. docs/active-architecture.md:124-138 says that grant covers all security envelopes and is separate from ordinary entity read/write. crates/pipeline/src/wiki_editorial.rs:1004-1051 stores a space-bound live grant and checks current activity.

Required repair: specify a typed privileged-service handoff. The original requester must be authorized to commission the exact service operation; an independently authorized service mandate bounds its internal reads/processors/publications. The root operation ceiling constrains that handoff, but need not imply the requester can directly read each private input. Preserve requester provenance and prohibit returning private source bodies, unrestricted service credentials or broadly reusable internal permits. Distinguish current service-grant validity from the commissioning user's direct resource entitlement. Name this as the way special service grants fit the conjunction rule, not an undocumented adapter exception.

Acceptance test: a manager with literal manage:wiki, no broad direct read, and unrestricted subjects can commission the bounded service; the same manager cannot read private sources or intermediate context. Wildcard-only and subject-restricted managers cannot commission it. A prompt changing the service to export raw data, a different space, or a different recipient fails. Revoking/replacing the service grant fences future work and does not revive older ancestors.

## E3 - P2 - Preserve the subjectless waiver, not just the empty-set shape

ADR lines: 152-158, 224-238, 270-274, 290-295. The combination of an unqualified 'Empty intersections deny' and 'preserve empty subject restrictions as restrictive' leaves a concrete Elephant exception underspecified.

Counterexample: a principal has Some(empty subject_allowlist), plus handling clearance for handling:subjectless_ok. A record has no subjects and explicitly carries handling:subjectless_ok. After fixing the HTTP adapter's empty-to-None bug, Elephant's policy engine permits that record. A generic adapter that converts the empty subject set to universal deny changes legitimate semantics. Conversely, unioning all handling labels across a restricted record and a subjectless-waived record can transfer the waiver and expose the first record.

Source evidence: crates/policy/src/engine.rs:152-199 first requires handling clearance, then explicitly permits incomplete or empty subjects when the record has handling:subjectless_ok; otherwise listed subjects must all match the allowlist. crates/pipeline/src/wiki_editorial.rs:4591-4596 says this flag is a permission exception and requires it on both contributors. Existing tests at :6279-6294 pin non-transfer of the exception. bin/elephant-api/src/authz.rs:152-161 currently erases Some(empty), a separate bug correctly identified by the ADR.

Required repair: define the generic monotonicity rule over allowed operations/resources, not blindly over each domain's raw set fields. In Elephant, None means unrestricted subjects; Some(empty) admits no named subject, while an explicit authority-controlled subjectless waiver remains a separate policy predicate. Treat permission-bearing exceptions separately from restrictive caveats. If the desired policy is to abolish the exception, declare that as an intentional semantic change, not a differential-preserving migration.

Acceptance test: a compact truth table covering None, Some(empty), Some(A), complete/partial/unknown subjects, waiver present/absent, caller handling clearance present/absent, and a disallowed named subject even when waiver is present. Include two-record derivation where only one contributor has the waiver.

## E4 - P2 - Govern the result envelope itself, including dependency existence

ADR lines: 218-228, 283-288, 299-307. The draft protects returned records and existence-sensitive audit reasons, but does not explicitly separate a resource-authority control envelope from model/user-visible data.

Counterexample: an authorized derived wiki result has a dependency on an evidence source whose name, subject identity or classified compartment is not disclosable to the final viewer. Elephant returns the authorized text plus source IDs, subjects, labels and decision references as required by section 5. An MCP adapter serializes the entire governed envelope into the model's tool result or console output. The authorized text remains protected correctly, but metadata reveals the hidden source or person. Recursively attaching the same restrictions to the envelope does not solve which part the trusted runtime can inspect for enforcement while the recipient cannot.

Required repair: specify two carriers: recipient-authorized payload/metadata and a protected control envelope accessible only to the enforcement TCB. Source/dependency references may be opaque capabilities or authority-resolved IDs; they must not be automatically rendered, logged, placed in prompts or exposed as citations. Claims needed for policy do not automatically confer permission to observe their values. The source authority must authorize any public projection of these labels and references, and the runtime may fail closed without disclosing the hidden reason.

Acceptance test: provide a result whose protected dependency metadata contains a canary person/source identifier. Permit the payload through a legitimate governed projection while denying metadata disclosure. Assert the canary never reaches model prompt, tool-visible JSON, UI, trace/export or ordinary audit query, while the policy owner can still revalidate the opaque dependency.

## Non-duplicated supporting requirements

The independent information-flow review reportedly identified ancestor revocation, provider-held context inventory, and conservative full observed-context provenance. I agree. Their required form should include every ancestor grant's current validity (or an authority-issued bounded attestation), fresh audience/holder binding, and no re-interpretation of an ancestor's revocation as a leaf-only lease issue. Information dependencies must include all model-observed/control-influencing input, not just citations the model selected.

## Strengths and limits

The ADR correctly avoids selecting a policy language first, requires both resource and runtime decisions, preserves feature ownership, rejects static service-token substitution, distinguishes audit projections from required evidence, admits remote freshness limits and excludes unsupported mixed-context/streaming profiles. These are substantial improvements over a caller-only proposal. The RED verdict is about the four concrete contract holes above, not a request to broaden the first implementation slice or claim a complete verified system.
