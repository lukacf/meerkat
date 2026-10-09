# ADR-001 identity and information-flow re-review, draft r2

Verdict: GREEN. The reviewed architecture closes this scope without a material remaining design gap. There are no new I-series RED findings. This is design closure for the declared operating profiles, not implementation acceptance or a claim about deployed enforcement.

Reviewed artifact: `(operator-retained path)/docs/internal/design/adr-001-runtime-security.md`, 568 lines.

Verified SHA-256: `44882a1ba3c8ea36d9a44c13df6c4b52b246c459dacc93437ef188ea7ac055ca`.

Review date: 2026-09-30. No source or ADR edits, builds, or executed security tests. The existing canonical Meerkat dogma remains the review standard.

## Closure of r1 precision notes

| Prior note | Disposition | Exact r2 evidence |
| --- | --- | --- |
| N1: ancestor-only revocation | Closed | Lines 177-181 require effective validity of every required ancestor and distinguish independently issued service mandates. Line 514 requires the live-leaf/revoked-ancestor test for reads, queued jobs, and resumed work. |
| N2: provider-held context | Closed | Lines 300-304 explicitly include provider caches, persistent model sessions, and remote context, and refuse unknown dependencies. Line 518 requires a sentinel absent from the current local request. |
| N3: conservative dependency domain | Closed | Lines 280-289 attach the complete observed domain, including control-influencing input, to model/curator answers, summaries, tool arguments, and artifacts. Citations or relevance claims cannot narrow it. Line 518 tests the curator case. |

## Independent regression review

### Privileged-service commissioning does not become ordinary delegation escalation

The strongest new attack is to ask a privileged wiki service to read a restricted source under its own authority, then treat the causal requester as entitled to the private source or its outputs.

Lines 191-201 reject that sequence. Commissioning is a separate typed operation with two decisions: the requester's bounded right to commission, and the independently issued service mandate for internal processing. The handoff names service identity, purpose, resources, processors, outputs, recipients, lifetime, and revocation dependencies. It explicitly forbids returning private intermediates, credentials, or reusable internal permits and requires separate publication authorization.

That separation is consistent with lines 98-104 and 165-181. Ordinary delegated agent work retains the original constraints. Service internal processing derives its own authority from the explicitly commissioned service contract; it cannot silently replace ordinary delegation because an agent happens to have broader credentials. The requester's commissioning ceiling and the service mandate both remain mandatory at the handoff.

A service mandate that survives a human grant is also no longer ambiguous: lines 177-181 require it to be independently issued. The handoff must declare its revocation dependencies at lines 197-198. Thus revoking a human grant invalidates dependent service work; intentional independence must be part of a separately authorized service mandate rather than inferred from a dropped ancestor reference.

The concrete Elephant mapping at lines 339-345 preserves the literal `manage:wiki` and unrestricted-subject predicates and refuses parameter substitution. Static source verification at the pinned Elephant revision `1aefaad2de68535ca5b588f58e8ed1ae1bf5b56a` supports this mapping:

- `crates/mcp/src/handlers/knowledge_views.rs:31-48` requires a nonempty space, unrestricted subjects, and the literal `manage:wiki` scope on the enforcing path.
- `crates/mcp/src/handlers/knowledge_views.rs:2299-2317` explicitly covers wildcard insufficiency and rejection of subject-restricted managers.
- `crates/pipeline/src/wiki_editorial.rs:1004-1051` binds managed authority to a space and registered grant and checks live activity.
- `crates/pipeline/src/wiki_editorial.rs:2413-2439` uses the service authority for recurring graph processing.

These source facts explain why requester read permission and service processing permission cannot be collapsed into one predicate. They do not prove the new runtime federation exists. The revision makes that distinction correctly.

### Service output cannot launder source restrictions

Counterexample attempted: the service produces a harmless-looking summary, caches it, or places an answer in a tool result; the requester has permission to commission the service but lacks permission to read the contributing data.

The service exception grants internal processing, not unrestricted release. Lines 280-289 require conservative dependency propagation. Lines 291-298 require current access/classification/tombstone authority for each transitive source. Lines 310-314 require recipient-specific output authorization. Lines 198-200 and 343-344 retain separate controls on intermediates and publication. A separately authorized transformation at lines 284-285 is the only stated route to release under changed restrictions; ordinary service output does not imply such a transformation.

Execution can complete while release is denied. Nothing requires the service's successful internal work to be reported as failure or retried. This remains compatible with the execution/evidence/disclosure split at lines 409-426.

### The protected envelope no longer exposes provenance as public metadata

Counterexample attempted: a correctly filtered body still reveals a hidden person's identity, denied source existence, grant identifier, or classification through its provenance envelope, tool JSON, audit query, or denial reason.

Lines 272-278 now explicitly separate recipient-visible data from the TCB control envelope, treat those fields as potentially sensitive, permit opaque authority-resolved references, and require separate authorization of each public metadata projection. Line 521 tests a person/source canary across prompts, UI, tool JSON, traces, and ordinary audit queries. The same rule covers service status metadata whose count or identifier would reveal restricted internal processing. A status observer's authorization is not authorization to receive every hidden dependency.

Opaque handles do not become replacement authority: their resolution still belongs to the source/identity authorities under lines 73-80 and 266-269. If a required dependency cannot be resolved, lines 295-296 refuse use.

### Reclassification and deletion invalidate derived uses, not only body reloads

Counterexample attempted: an immutable source revision and old receipt remain available while the source is reclassified or deleted. A cached summary or artifact keeps its old label and continues to be used.

Lines 291-298 now distinguish immutable content identity from live security state and require each transitive dependency to meet the current authority epoch or an explicit lease covering that state. A surviving archival/release grant must be explicit; deletion otherwise revokes retained-copy use. Line 517 tests unchanged content revision with changed security/tombstone state.

This closes an easy implementation misreading of revision-based provenance. The control envelope can retain enough protected identity to perform the recheck without exposing that identity to the requester.

### The subjectless waiver is permission-bearing and cannot spread by union

Counterexample attempted: one source with `handling:subjectless_ok` and one source without it are combined; a union of handling labels causes the derived artifact to waive the second source's incomplete subject restriction.

Lines 363-369 reject that transfer. The document distinguishes an empty restricted allowlist from unrestricted subjects, retains named-subject membership checks, requires caller clearance for the waiver, and requires every contributing source to authorize the exception. The phrase about ordinary empty attribute sets at lines 169-172 therefore does not create a global empty-means-deny rule that would overwrite Elephant's domain predicate.

Static verification against `crates/policy/src/engine.rs:152-199` confirms the important truth-table details: handling clearance is checked first, the waiver permits incomplete or empty subjects, and named subjects are still checked against the allowlist. This r2 mapping is accurate. The separate label/declassification authorization remains required at lines 370-373.

### Entry, evidence-pending, and late release remain distinct

Counterexample attempted: authorization observes a valid grant, another owner revokes it before entry, and an operation-local lock is presented as sufficient fencing. Lines 221-230 now require participation by every relevant authority or a declared bounded lease; a final version read alone is explicitly insufficient. This strengthens the boundary relied on by information use and disclosure rather than adding a competing evaluator owner.

Counterexample attempted: the effect succeeds, the receipt append fails, and the caller retries the operation or receives the output through another path. Lines 394-426 preserve the actual operation owner's custody, forbid inventing retry safety, retain a receipt-only obligation after a known outcome, and block new protected uses of the result while mandatory evidence is pending. Authorized outcome observation remains distinct from result-body release. A crash that loses the outcome becomes Unknown instead of a fabricated receipt-only success.

No information-flow regression follows from making execution truth visible to an authorized observer: lines 272-278 still control the metadata projection and lines 310-314 still control body release.

## Remaining scope limits are explicit, not unfinished design findings

The first profile still uses whole-context authorization and buffered output. Mixed-confidentiality reuse, dynamic audiences, multi-principal batching, and partial streaming remain unavailable under lines 316-323. Hostile native tools still require a separately supported isolation profile under lines 122-127 and 543-544. Remote revocation retains a declared freshness bound under lines 244-250. None of the new service or waiver text creates an exception to those limits.

No further clarification is required for closure of this review scope. The next required evidence is the owner tracer and the production-path acceptance suite at lines 474-528, including actual protected sinks. Their execution remains future implementation work by the ADR's explicit status.
