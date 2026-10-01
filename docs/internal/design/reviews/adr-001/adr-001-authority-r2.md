# ADR-001 r2 authority re-review

Verdict: GREEN for architectural acceptance. No remaining material authority finding from this review. This is not implementation approval, deployment approval, or evidence that the proposed guarantees are enforced.

Reviewed artifact: `/Users/luka/.codex/worktrees/security-adr/meerkat/docs/internal/design/adr-001-runtime-security.md`, 568 lines.

SHA-256: `44882a1ba3c8ea36d9a44c13df6c4b52b246c459dacc93437ef188ea7ac055ca`.

Implementation baseline remains Meerkat `54d14e91bd426fcaaafc156227b0b5ab0e23d9a6`. This was a static reread of the complete revised ADR against the canonical doctrine, implicated commentary, and the current-source evidence established in r1. No builds, tests, fault injections, or live integrations were run. The ADR and source were not edited.

## Finding dispositions

| ID | Prior severity | Status | Exact r2 evidence and reason |
| --- | --- | --- | --- |
| A1 | High | Retired | Lines 221-230 now require a named participating-authority composition, generation stability through one durable entry commit, explicit participants and linearization, revocation acknowledgment, crash release and cancellation/expiry. They explicitly reject an operation-local lock or last read as sufficient. Nonparticipating authorities require an explicit bounded lease even within one process; unsupported coordination refuses the claimed profile. Lines 231-242 preserve exact resource binding and distinguish entered work from new effects. Line 512 requires competing production-authority updates and rejects test-only synchronization. This closes the r1 two-owner race in the normative contract. |
| A2 | High | Retired | Lines 394-401 couple consumed attempt identity, owner state, and pre-effect evidence; forbid execution from an intent row alone; and distinguish dispatch/launch acceptance from actual async/deferred settlement. Lines 409-426 provide separate execution, evidence, and disclosure states. A known result survives append failure; its existing owner retains receipt-only retry custody; a crash that loses outcome knowledge truthfully yields Unknown. Pending evidence restricts protected result use without preventing mechanical reconciliation. Lines 515-516 require the exact post-effect audit-failure and delayed-effect tests missing from r1. |
| A3 | Medium | Retired | Line 75 names one policy-domain owner for active content, revision, activation epoch, and rollback. Lines 89-96 separate authoring and activation, require durable monotonic epochs, prohibit reuse of a revision with different bytes, make rollback a new authorized activation, and retain current provider ownership until explicit migration. The common evaluator cannot keep a second accepted-version map. Line 513 requires restart/replay/rollback conformance. Lines 537-538 leave storage implementation open while preserving the decided semantics. |
| A4 | Medium | Retired | Lines 129-138 make durable security evidence mandatory in governed, reject an implicit non-audited governed variant, and persist/negotiate profile identity. Lines 387-392 apply pre-effect durability to governed without a new qualifier. Lines 428-433 state the audit-outage behavior for ingress/denial without claiming every failed packet was durably audited. Line 522 explicitly tests bootstrap, restore, and handoff without receipt capability. |

## Additional adversarial checks

- **No universal security machine:** Lines 62-67, 75-81, 221-236, and 394-426 leave policy activation, grant state, resource state, operation realization, and evidence custody at their named owners. The entry composition coordinates their facts; it does not obtain arbitrary executor authority. A receipt store cannot choose a retry or reconstruct lifecycle from its own rows.
- **No hidden duplicate journal:** The revised distinction between operation-owner durable custody and append-only security evidence is coherent. An intent is evidence coupled to owner state, not a second execution queue. Receipt-only retry cannot authorize the protected effect again. The concrete storage/handoff implementation still has to prove this.
- **Crash truth is explicit:** The text does not claim a known-but-unpersisted outcome survives process death. It explicitly distinguishes live known outcome/evidence pending from recovered Unknown. This removes the tempting but false promise that every post-effect audit failure can always recover by appending the known result.
- **Revocation scope remains consistent:** Entered effects can settle under the declared entry contract; new attempts and disclosures reauthorize. Ancestor validity at lines 177-181 and live dependency epochs at lines 291-298 prevent a valid leaf token or immutable content revision from silently becoming permanent authority.
- **Service commissioning is bounded:** Lines 191-201 describe an explicit commissioning operation plus an independently issued service mandate, retain causal requester and actor identities, constrain service parameters, prohibit handing internal authority to the requester, and independently authorize publication. This is a declared handoff, not a generic privilege-elevation switch. It must remain distinct from ordinary descendant delegation in implementation.
- **Open decisions do not reopen the repaired semantics:** Lines 535-544 defer API, storage, credential, and later-profile details. They no longer leave policy rollback or receipt-result semantics unspecified. Unsupported implementations must refuse their claimed profile under the preceding contract.

## What GREEN does not establish

The current tool gate is still only existing implementation evidence. In the pinned source, `ExecutionPolicyGatedDispatcher` performs dispatch admission before asynchronous consequence evaluation and maps an inner successful dispatch to its specialized live-bridge result before fallible settlement. `ToolDispatchOutcome` can still carry running async operations and deferred session effects. R2 now explicitly requires an implementation to respect those different owners; the document itself changes none of that behavior.

The next mandatory proof remains the production-owner tracer and governed vertical slice at lines 474-490, followed by actual sink and fault assertions at lines 492-528. In particular:

1. Map every final entry to real grant, policy, resource, binding, and attempt owners. Demonstrate the selected coordination protocol with independently scheduled authorities, not a test-only common mutex.
2. Demonstrate durable attempt consumption and owner custody across cancellation, lost acknowledgments, stale executors, and crash recovery. A receipt writer attached to the existing tool callback is insufficient by itself.
3. Exercise synchronous, detached, and deferred effects separately. Verify that evidence-pending and recovered Unknown do not invite duplicate execution or leak withheld bodies.
4. Prove generated declarations constrain actual registration/dispatch, including dynamic bindings and alternate surfaces. A complete catalog is not evidence of a complete effect path.
5. Verify the declared migration/cutover on real durable versions and reject missing provenance or weaker restored profiles. The allowed refusal path must remain available when old data cannot be migrated truthfully.

These are enforcement obligations already required by r2, not new architectural blockers or requests to freeze APIs prematurely.
