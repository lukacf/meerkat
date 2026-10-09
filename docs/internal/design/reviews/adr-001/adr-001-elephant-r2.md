# ADR-001 r2 - Elephant and federation adversarial re-review

Verdict: GREEN for architecture review and sharing with the project owners. No remaining material blocker found in this review's Elephant, distributed-grant, information-provenance and security-audit scope. This is not implementation acceptance or proof that any currently running integration provides the proposed guarantees.

Reviewed exact file: (operator-retained path)/docs/internal/design/adr-001-runtime-security.md
SHA-256: 44882a1ba3c8ea36d9a44c13df6c4b52b246c459dacc93437ef188ea7ac055ca
Length: 568 lines.
Elephant source baseline: 1aefaad2de68535ca5b588f58e8ed1ae1bf5b56a.
Review method: re-read revised normative contracts and acceptance gates against the r1 counterexamples and previously inspected source. Static review only. No source/ADR edits and no tests executed.

## Closure of r1 findings

| Finding | Disposition | Exact repair and why it closes the counterexample |
| --- | --- | --- |
| E1, historical revisions survive present source restriction changes | Closed | Lines 291-298 distinguish content from live security identity, require every transitive dependency's current access/classification/tombstone epoch or a covering bounded lease, refuse missing dependencies, and make deletion revoking by default absent an explicit surviving grant. Line 517 adds the unchanged-revision/reclassified-ancestor sink test. Old content receipts cannot now be treated as permission to replay cached summaries. |
| E2, root-read intersection conflicts with privileged wiki service delegation | Closed | Lines 191-201 define the typed commissioning operation and independently issued service mandate, preserve both actors, prohibit private intermediates/credentials/internal permits from escaping, and require separate publication authorization. Lines 339-345 map actual Elephant manage:wiki behavior into that contract without granting the requester direct read. Lines 177-181 distinguish ancestor-dependent grants from intentionally independent service mandates. Line 519 requires positive and negative commissioning tests. |
| E3, generic empty-set rule breaks subjectless waiver semantics | Closed | Lines 169-172 make monotonicity about effective permission rather than generic attribute-set emptiness. Lines 361-369 preserve None versus Some(empty), require record waiver plus caller clearance, retain the named-subject restriction and forbid transferring the waiver from one contributor. Line 520 states the domain truth-table test. This matches the inspected predicate and restrictive join. |
| E4, protected result metadata leaks dependencies and people | Closed | Lines 272-278 split recipient-visible payload/metadata from the enforcement-only control envelope, allow opaque references, forbid automatic prompt/tool/log/audit exposure and require separate disclosure authorization. Line 521 adds a canary test across all named sinks. |

## Additional adversarial checks

- Root revocation while an exchanged Elephant leaf token remains unexpired: lines 177-181 plus 244-249 require transitive validity within a declared bound; leaf expiry alone no longer passes.
- Reclassification races after a local version read but before a protected disclosure: lines 221-230 require participating-authority fences or an explicitly bounded lease, not an operation-local lock masquerading as a global fence. This also covers the current-state dependency requirement from E1.
- A model omits the sensitive evidence from its citations but uses it to choose a tool argument: lines 286-289 attach all observed and control-influencing context to answers, summaries, tool arguments and artifacts. Omitted citations do not reduce the dependency set.
- A provider retains hidden prior context while local request bodies appear clean: lines 300-304 explicitly include provider-held sessions/caches and refuse unknown remote dependencies.
- Elephant authorizes a read, but the receiving runtime's governing receipt cannot be committed: lines 129-133 and 387-401 require the governed audited composition and durable entry/effect evidence; the receiver cannot silently switch to unaudited work. Remote response and local decision correlation remain required at 354-356.
- Remote mutation succeeds, then its response is lost and local evidence persistence fails: lines 403-426 preserve the distinction among known result, evidence-pending and genuinely unknown result, and keep retry authority with the real operation owner. The ADR does not claim distributed exactly-once effects.
- A subjectless waiver is combined with a record naming a disallowed subject: lines 366-369 retain the named-subject check and require every contributor to authorize the waiver. The waiver does not become a universal exemption.
- Wiki commissioning by a wildcard-only manager or a subject-limited manager: lines 339-345 plus 519 retain literal manage:wiki and unrestricted-subject requirements. The privilege is scoped to a service operation rather than converted into general resource clearance.

## Implementation reminders, not new ADR blockers

The following are concrete entries to include when implementation owners build the already-required differential corpus and protocol conformance suite. They do not require enlarging the first profile or delaying architecture review.

1. Elephant has intentional or historical surface differences that an evaluator migration must inventory explicitly: HTTP event/timeline aliases in bin/elephant-api/src/authz.rs:112-124 versus MCP explicit event scopes at crates/mcp/src/authz.rs:97; ordinary wildcard scope expansion versus literal manage:wiki; raw-document/source implications; and first matching clearance behavior in crates/policy/src/engine.rs:126-130. An implementation must label any intended semantic change instead of calling it conformance preservation.
2. The federation protocol still needs exact issuer and holder binding, audience, chain-size/depth limits, key rotation and clock/freshness rules before it is enabled. The ADR appropriately leaves the wire format open while making missing support fail closed. Use adversarial replay with the same valid grant presented by the wrong workload, not only invalid-signature tests.
3. The transitive source closure may be expensive. Any batch certificate or opaque dependency handle must be issued by the relevant authority and bind the complete source set plus current security epochs; it cannot be a stale client-side optimization that skips dependencies. The existing first-profile restrictions give room to start without a clever cache.
4. Independent wiki service mandates require explicit policy on which changes revoke them. Their intended independence from a commissioning user's ordinary read grants does not make the service grant immune to its own replacement, space revocation, processor-policy change or output restrictions.

## Scope of GREEN

GREEN means the revised text contains coherent, fail-closed answers to the material counterexamples raised in this review and is ready for project-owner challenge. Every concrete protocol, source-attribution implementation, feature coverage claim, effect fence and sink assertion remains subject to the implementation acceptance gates. No production security claim was verified by this review.
