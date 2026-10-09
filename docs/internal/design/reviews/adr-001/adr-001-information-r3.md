# ADR-001 information and identity review, candidate r3

Verdict: GREEN. The candidate is coherent enough to send to the project owners for their review. I found no material architecture contradiction or implicit widening in the reviewed information, identity, adoption, classifier, audience, partition, and reset contracts.

This is a design verdict for the declared profiles. It is not implementation acceptance, deployment approval, or evidence that the referenced product integrations already meet the contracts.

Reviewed file: `(operator-retained path)/docs/internal/design/adr-001-runtime-security.md`, 904 lines.

Verified SHA-256 before and after the full read: `a242a21407fbf52d2ca5615edfdd6b5649b40fa6f73c959d4857eddd8e2db87e`.

Verified Meerkat source baseline: `54d14e91bd426fcaaafc156227b0b5ab0e23d9a6`.

No ADR or source edits were made. No security tests, builds, or live-system operations were performed.

## Material findings

None. No I-series RED findings remain in this review scope.

## Detailed adversarial adjudication

### 1. Legacy adoption is an explicit authority assumption, not invented history

Relevant text: lines 440-458 and 547-573; acceptance at line 844.

Attempted counterexample: take an old transcript with unknown contributors, call it a newly classified resource, and thereby erase both old restrictions and the fact that its origins are unknown. Another variant imports a known restricted source inside a legacy bundle to override that source's authority.

The candidate rejects the implicit version of this operation. Lines 547-549 make the exceptional transformations explicit and forbid treating ordinary import, write, backup possession, or administration as authorization. Lines 563-568 require an authority with explicit rights over the corpus, immutable enumerated bytes, an attributed grant with bounded purposes/processors/audience/retention/expiry/revocation, and preserved unknown authorship/dependencies. The grant explicitly covers unknown internal dependencies; this is the stated exception to ordinary refusal, not a claim that missing provenance was recovered. Lines 569-573 preserve known source restrictions, make new bytes require new authorization, retain bundle dependency, preserve revocability, disclaim source-specific revocation for unknown identities, and deny replay authority over historical work.

The trust assumption is therefore honest: an authorized adopter assumes responsibility for the stated use of the exact corpus. The architecture does not prove that every historical author consented or that every original source has been identified. It records the exceptional authority on which use relies. A host that cannot supply that authority must leave the bundle refused. The general conjunction rule at lines 119-125 still applies to all known mandatory constraints.

Disposition: closed. There is no conflict between ordinary unknown-dependency refusal and this expressly bounded exception. There is no permission for a model or ordinary store administrator to manufacture it.

### 2. Automatic classification is not claimed to be injection-proof

Relevant text: lines 408-418, 426-449 and 547-562; acceptance at lines 831 and 845.

Attempted counterexample: a private email tells its classifier to choose `team`. That label is inside the permitted outcome set, so a lattice-only test passes although confidentiality broadens.

Lines 555-559 explicitly call every permitted broadening from the restrictive ingestion baseline a preauthorized release/trust assumption. The text says the lattice does not prove correct classification or resistance to prompt injection. A deployment rejecting that discretion retains the restrictive baseline. The classification is committed by the decision authority under a typed source-scoped mandate, with source and model-attempt evidence; the model itself is not made an issuer.

The remaining generic sentence that facts come from resource authority at lines 426-429 is consistent with this contract: the committed fact comes from the authority, which may choose to rely on a bounded model proposal and must own that choice. The acceptance gate at line 845 separates hard bounds from semantic misclassification rather than treating one as proof of the other.

Restrictive proposals cannot launder permission-bearing labels either: lines 560-562 require composition by real permission semantics, forbid creating exceptions, and refuse or reroute incompatible shared-context input. This is consistent with the subjectless-waiver rule at lines 616-622.

Disposition: closed. The residual classifier error risk is deliberately admitted and bounded, not falsely solved.

### 3. Shared-context admission prevents contamination before inference

Relevant text: lines 497-518; acceptance at line 843.

Attempted counterexample: a team calendar receives an leads-only appointment. It stores or hydrates the appointment, later discovers that non-lead members are among its intended recipients, and permanently restricts all subsequent responses. A triage variant runs one model over mixed private inputs and emits separate output objects with supposedly disjoint dependencies.

Lines 497-503 put intended audience and processing domain under an authority and require compatibility before context hydration or transcript admission. Incompatible input must be refused or routed elsewhere, and unclassified input starts in restricted ingestion. The new rule prevents this contamination path rather than relying solely on a late release denial.

Lines 505-509 require partitioning before shared inference or independently isolated contexts. They explicitly preserve complete dependency on mixed-context outputs and reject JSON separation as provenance proof. The first-profile prohibition on incompatible multi-principal batching remains at lines 511-518. Logical agents can own partitions without merging the requesters' authority.

Disposition: closed. The new admission condition improves availability without weakening per-use or final disclosure checks.

### 4. Reset does not launder user messages, summaries, metadata, or late results

Relevant text: lines 520-536; acceptance at lines 833 and 840.

Attempted counterexamples: copy old user messages but omit assistant messages; ask the tainted model for a clean summary; perform an empty-prefix fork that silently retains metadata; reuse a provider session after changing the local session ID; or route an old tool completion to the new segment using the stable agent name.

The candidate explicitly excludes old user and model messages, summaries, memory, artifacts, pending prompts, provider state, cache references, and content-bearing metadata. Ordinary fork/compaction is not reset. Imports require fresh protected reads with complete dependencies. The original mandate persists for continued work, while independent work requires independently authorized admission. Outstanding effects remain with their existing owners.

Lines 533-536 preserve the product's logical member identity while requiring the binding owner to activate a new context generation and fence routing. Old results remain bound to the old generation. This closes the late-result contamination case without discarding logical product identity.

Source confirmation: `crates/meerkat-core/src/session.rs:7765-7794` shows that `fork_at` copies the selected prefix and calls `fork_metadata_projection`; `:6006-6009` shows that the projection clones non-reserved metadata. `:7958-7981` provides the full-history fork. Those are useful mechanisms but not already the specified reset. The ADR correctly treats the clean reset as an owner-mediated extension rather than claiming these existing operations satisfy it.

Disposition: closed. No path in the proposed reset grants declassification or automatic release of retained content.

### 5. Stateful providers and live channels remain gated before disclosure

Relevant text: lines 460-472 and 520-536; acceptance at lines 821 and 840.

Attempted counterexample: a local context appears empty after reset but a remote cache, persistent model session, or live voice sideband contains the previous confidential conversation.

The inventory explicitly includes provider-held caches, persistent model sessions, and referenced remote context; unknown remote dependencies refuse use. Live attachments are explicitly unavailable in the initial buffered profile before connection or seeding. Reset cannot inherit those handles.

Source confirmation: `crates/meerkat-core/src/agent/runner.rs:1596-1602` states that Gemini explicit cached content can own the system instruction. `crates/meerkat-gemini/src/client.rs:671-673` lowers the remote cache reference. `crates/meerkat-openai/src/public_live.rs:728-743` sends the session configuration through `create_webrtc` before attaching the sideband. These source paths substantiate the need for pre-connect refusal and complete context inventory; the candidate now says so directly.

Disposition: closed. Support is deferred by refusal, not by dropping the rule.

### 6. Stored output, copies, and future readers have real disclosure semantics

Relevant text: lines 474-495; acceptance at lines 832 and 836.

Attempted counterexamples: store classified plaintext with a label column but let direct warehouse IAM readers bypass the application; send a private DM copy through a configured monitoring channel; authorize current channel members but allow later joiners to read retained history.

The candidate covers all plaintext read paths, including other applications, direct IAM access and exports, or requires authorization of the effective reader audience at the write. Labels beside accessible plaintext are expressly insufficient. Every secondary destination requires disclosure authorization. For retained channels, send-time membership does not authorize future readers; an enforceable subsequent access policy or explicit broader release policy must cover them. The latter is honestly identified as broader release rather than revocable per-reader control.

Dynamic audiences remain unsupported initially. Thus the future-destination contract does not silently enable a product path whose enforcement is still absent.

Disposition: closed.

### 7. Identity, service processing, discovery, and physical audiences remain distinct

Relevant text: lines 89-98, 132-153, 199-314, 589-622.

Attempted counterexamples: treat a device's credential as the speaker's identity; treat a lead's relationship or operator role as blanket data access; substitute a service token for the requester; infer tool execution authority from discoverability; or use a waiver-bearing source to relax another source.

The candidate retains canonical principal qualification and issuer validation, introduces an explicit owner for principal relationships without making them universal grants, and distinguishes unidentified speakers from device owners. Unsupported anonymous/physical profiles refuse hydration and release. Approval requires an authenticated approver event bound to the displayed operation, not a model's paraphrase. Service commissioning authorizes bounded internal processing but preserves separate publication authorization. Literal Elephant wiki scope, current service grant, subject restrictions and waiver propagation remain distinct.

The discovery clause is now semantically accurate. It authorizes metadata separately, refreshes under current evidence, and permits a public tool to remain listed while execution is denied. This is compatible with the existing list-preserving execution gate at `crates/meerkat-core/src/tool_execution_policy.rs:10-18`; it does not demand that one decision impersonate the other.

Disposition: closed.

### 8. Freshness, restoration, and evidence do not create indirect release bypasses

Relevant text: lines 185-195, 334-398, 451-458, 538-545 and 630-729.

Attempted counterexamples: miss a revocation notification and treat silence as permission; restore a pre-revocation snapshot; re-admit lost work from an audit row as if non-entry were proven; or classify a successful effect with a failed receipt append as a retryable failure.

The candidate requires authoritative coverage plus an entry barrier or explicit lease, not just event delivery. Lost coverage requires revalidation or refusal. Restored/cloned authority must be validated against a surviving rollback-resistant authority before protected hydration/effects. Recovery preserves unknown outcomes, checks original payload/authority, and does not give receipt rows execution power. Evidence-pending blocks dependent body use without changing known effect truth. The reserved notification path still requires durable evidence and bounded authorization before dispatch.

These conditions preserve the information-flow guarantees through exceptional paths. They do not replace them with optimistic recovery or an unaudited emergency exception.

Disposition: no material information-flow regression identified.

## Review boundary

The candidate now contains explicit exceptional authority for legacy adoption and discretionary classifier release. Project owners must decide whether to issue those grants for their own data and deployments. GREEN does not claim that they have already accepted those trust assumptions or that every product can use the initial profile unchanged.

The architecture also deliberately requires unsupported dynamic, physical, anonymous and live integrations to remain unavailable until their profiles are specified and proven. That constraint is honest and consistent with the proposed first-profile scope.

Implementation feasibility, owner-state modeling, per-operation overhead, and actual enforcement remain subject to the tracer and acceptance gates at lines 769-860. No further text-level information/identity repair is required before sending this candidate to the project owners.
