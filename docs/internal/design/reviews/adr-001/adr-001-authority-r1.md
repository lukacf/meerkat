# ADR-001 r1 authority review

Verdict: RED.

Target: `/Users/luka/.codex/worktrees/security-adr/meerkat/docs/internal/design/adr-001-runtime-security.md`, 444-line draft reviewed against Meerkat commit `54d14e91bd426fcaaafc156227b0b5ab0e23d9a6`.

This is a static architectural review. No builds, tests, fault injections, or exploit demonstrations were run. The ADR is explicitly a proposal; findings below concern its normative contract and the implementation assumptions used to justify that contract. They are not claims that the proposed security feature already exists. Source paths below are relative to the Meerkat checkout above.

## Findings

| ID | Severity | Status | Root defect |
| --- | --- | --- | --- |
| A1 | High | New, design blocker | Local entry serialization does not define an ordering with independent grant, policy, and resource authorities. |
| A2 | High | New, design blocker | Mandatory receipt failure after a real effect has no complete outcome/recovery contract, and tool dispatch completion is not effect completion. |
| A3 | Medium | New | Active policy version, activation, and rollback authority are missing from the ownership contract. |
| A4 | Medium | New | An undefined `governed audited` profile makes mandatory audit guarantees conditional without defining the condition. |

## A1 - A local operation lock is not a revocation fence across authorities

**ADR evidence:** lines 71-90 assign grant validity, resource facts, operation entry, and effective decision to separate owners. Lines 188-200 then promise that checking current generations in an owner-serialized entry transition orders revocation against the effect. Only the cross-host case receives an explicit bounded-freshness exception at lines 202-208. Lines 390-391 test the promised result without specifying the required authority coordination.

**Dogma:** Authority Is Singular; Generated Machines Own Canonical Change; Shells, Stores, and Projections Are Mechanical. Canonical doctrine lines 105-119 requires a typed authority/handoff path for enabledness and effect closure. Commentary lines 390-409 requires the realizing owner, feedback inputs, correlation fields, and closure policy to be named.

**Counterexample:** One process contains a grant owner G, resource owner R, and tool operation owner O. O validates generation G7 and resource version R12. G independently commits revocation G8. O then commits its entry transition and dispatches. No unrelated await is necessary: two threads and two independent owner locks suffice. O serialized its own transition exactly as written, but the revocation committed before entry and the effect still started. An immutable snapshot or a second read only moves the race unless something holds the relevant authority stable through the linearization point. The same problem exists if a mutable resource binding changes after its version is checked.

**Implemented evidence:** `crates/meerkat-core/src/tool_consequence_policy.rs:577-594` makes the policy provider the snapshot/current-version owner. `:969-994` obtains a generation and snapshot, awaits evaluation, and returns `Ok(())` for Allow; it does not produce a cross-owner entry fence. `crates/meerkat-core/src/tool_execution_policy.rs:581-595` first awaits dispatch admission, then awaits consequence evaluation, then dispatches. That existing admission callback is too early to be adopted as the final security entry point unchanged. The current store has a useful precedent: `crates/meerkat-runtime/src/store/mod.rs:9013-9026` explicitly requires retaining an external authority fence and target transaction through commit. None of this proves that the proposed security fence is implemented.

**Required repair:** Specify the semantic coordination requirement now, without freezing Rust API names. Every authority whose update must beat local entry must participate in a named entry composition/handoff that validates and holds its generation through the single entry commit, or expose an equivalent atomic compare/reservation protocol. Explicitly define the entry linearization point and the point at which revocation is acknowledged as effective. Authorities unable to join this ordering must use an explicitly bounded lease/freshness profile, even when located in the same process; locality alone is not atomicity. The effect owner consumes the resulting exact authorization. The evaluator must not become the executor or a universal security machine. Require all slow preparation/evaluation before the final fence and require the actual bound resource to remain covered at realization.

**Acceptance test:** Independently schedule G, R, and O with barriers after every observation and immediately before durable entry. Commit G8 or R13 after O's last read but before entry. Assert zero protected sink calls for a revocation/version change that won the defined ordering. Also test entry winning first, cancellation during the commit, owner restart, and an old executor resuming after replacement. Assert exact typed outcomes and no duplicate realization. A test that serializes revoke and entry through the same test-only mutex does not establish the production contract.

**Decision classification:** Blocking semantic decision. Lock implementation, type names, and exact crate placement can remain open. The ordering and participants cannot be left implicit in the word `fenced`.

## A2 - A completed effect followed by receipt failure is an unmodeled terminal state

**ADR evidence:** lines 188-200 preserve existing settlement and one-use execution; lines 247-251 separate execution from disclosure. Lines 309-320 require a durable pre-effect intent and append the real settlement, but specify refusal only before new effects and Unknown only for a crash or ambiguous timeout. Lines 322-326 say a required receipt commit is semantic. The fault matrix at line 396 covers pre-entry audit failure, crash after send, and optional exporter failure, but omits a known real effect followed by failure to commit its mandatory receipt. Lines 417-418 defer recovery reconciliation.

**Counterexample 1:** The pre-entry intent commits. A destination accepts a mutation and returns a definitive result. The mandatory settlement-receipt append fails while the process is still alive. Reporting ordinary operation failure invites a duplicate. Reporting ordinary success hides a required unmet security obligation. Replacing the known effect result with Unknown loses evidence. Retrying the effect to repair the audit duplicates it. The ADR does not select the distinct execution, evidence-durability, and disclosure states or identify which owner retains the obligation after waiter loss and restart.

**Counterexample 2:** A tool returns `Ok(ToolDispatchOutcome)` containing a detached operation, or a session effect that will only be committed at the turn boundary. A receipt attached to the outer dispatch gate records successful settlement before the actual protected work has settled. Later the detached operation fails or the session commit refuses. The advertised audit chain now contains false completion. One tool call can also authorize several later effects, so a single tool-call completion cannot stand in for all of them.

**Implemented evidence:** `crates/meerkat-core/src/tool_execution_policy.rs:566-573`, `:595-602`, and `:625-635` map any successful inner dispatch to `LiveBridgeEffectOutcome::Committed`, then await settlement with `?` before returning the inner result. A settlement failure can therefore replace that inner result with `ToolError`. This is an existing specialized live-bridge admission protocol, not a ready-made general security outcome protocol. `crates/meerkat-core/src/ops.rs:185-199` explicitly allows still-running `async_ops` and uncommitted `session_effects`; `:77-82` says the turn owner commits the latter after the batch. `crates/meerkat-runtime/src/store/mod.rs:8740-8746` already distinguishes a failed-but-applied turn and requires one atomic transaction for its snapshot, boundary receipt, lifecycle, and input/outbox state. The ADR must use those actual owner boundaries rather than treating dispatch return as universal settlement.

**Required repair:** Add an explicit state/result matrix covering at least not-entered, entered with unresolved effect, known effect outcome with evidence pending, evidence committed, and disclosure withheld. Physical effect truth, audit durability, and publication truth must remain separate typed facts. Explain which existing generated operation owner holds the pending evidence obligation, how it is persisted/recovered, and what an authorized caller observes. A successful receipt retry must never re-run the effect. An integrity/append failure must not turn a known applied effect into ordinary execution failure or falsely complete a still-running effect. Split launch/acceptance receipts from terminal effect receipts for asynchronous and deferred effects, preserving their owner-generated correlation. Specify the atomic relationship among attempt consumption, pre-entry intent, and the existing owner state; an intent-only record must not independently authorize recovery execution. Do not introduce a second lifecycle reducer or security recovery queue.

**Acceptance test:** Use a sink with observable idempotency and a controllable receipt store. Inject failure and caller cancellation before intent, after intent, after entry, after definitive sink success, during receipt append, and after append before acknowledgment. Restart at each persisted boundary. Assert sink count, preserved execution result, evidence state, single-use attempt identity, correct disclosure, and receipt-only recovery. Run separate cases for a synchronous mutation, detached operation, and deferred session effect. Force a terminal async failure after successful launch and assert that launch never appears as terminal success. Force a known committed mutation followed by audit failure and assert that no surface exposes an ordinary retryable `ToolError` as if nothing happened.

**Decision classification:** Blocking semantic decision. Storage engine and exact outbox representation may follow the tracer. Outcome classes, owner custody, and allowed recovery actions are architecture, not a backend choice.

## A3 - Policy authoring is named; active policy lifecycle is not

**ADR evidence:** lines 56-67 allocate decision semantics to the common capability and rules to applications. The ownership table at lines 71-80 names grant lifecycle, resource attributes, operation lifecycle, credential lifecycle, and receipt durability. It does not name who owns the active policy revision, its activation epoch, accepted-version floor, or rollback transition. Lines 88-94 nevertheless require current host/resource/destination policy. Lines 303-304 emit policy-change receipts. Lines 413-414 defer update validation and rollback semantics.

**Counterexample:** An application authors policy P2 to revoke an operation. The host evaluator still has P1 cached while a registry or policy store exposes P2. A reload reinstalls P1, or uses the same revision with different content. Both implementations can claim to be applying application-owned rules and the shared evaluation contract. The receipt can faithfully record whichever digest was used, but no named authority has determined which revision was legally active. This also prevents A1's generation check from having a well-defined policy epoch.

**Implemented evidence:** `crates/meerkat-core/src/tool_consequence_policy.rs:585-590` explicitly places snapshot ownership and rejection of older accepted revisions on the policy provider and deliberately forbids a second accepted-revision store in Meerkat. The ADR neither explicitly retains that ownership nor specifies its replacement for the new shared capability. Moving evaluation into a common crate must not quietly create a registry-owned accepted-version map beside the existing provider's map.

**Required repair:** Add a distinct policy-lifecycle ownership row. For each policy domain, name the authority for active revision/epoch, content identity, activation, rollback, and recovery. Separate authoring from activation and pure evaluation. State whether the existing provider remains that authority or lowers activation to a named generated composition/handoff. Rollback must be a newly authorized activation with its own epoch; restoring an old snapshot must not silently restore an old authority epoch. The common evaluator consumes exact active-policy evidence and owns no competing mutable current-policy store. The backend language and concrete API can stay deferred.

**Acceptance test:** Activate P1 then P2, restart, replay P1, supply P2's revision with changed content, roll back intentionally through the authorized path, and interrupt activation before and after its durable boundary. Assert a single active epoch, no silent privilege restoration, and receipts matching the exact legally active policy. Concurrent entry/activation must obey A1's selected ordering.

**Decision classification:** Ownership clarification required before acceptance. This does not require selecting a policy language or adding a god machine.

## A4 - Audit is mandatory only in a profile the ADR never defines

**ADR evidence:** lines 119-127 define two profiles, trusted-embedded and governed. The requirement at lines 30-34 includes reconstruction of every protected decision/effect. Line 310 then narrows the durable pre-effect requirement to `the governed audited profile`. No profile table, capability negotiation rule, or activation rule defines whether this is a synonym for governed, a third profile, or an optional audit dimension. The acceptance matrix at line 396 assumes pre-entry audit failure refuses execution.

**Counterexample:** An implementation enables `governed`, installs authorization and disclosure enforcement, and omits the durable receipt store because the host did not choose `governed audited`. It can comply with the only explicitly qualified durable-intent requirement while advertising the broader governed guarantees elsewhere. A restart may likewise reconstruct governed enforcement without reconstructing the undeclared audit capability. The ADR's missing-components rule cannot close this because the profile's required component set is ambiguous.

**Required repair:** Either make durable security evidence an unconditional part of governed and remove the extra qualifier, or explicitly define distinct profiles and their guarantee/capability matrix. Persist and negotiate the selected profile identity with the admitted work. An execution must not be restored or handed off under a weaker audit dimension. State the allowed behavior when authentication or denial receipts cannot be persisted without accidentally granting an effect.

**Acceptance test:** Attempt bootstrap, restore, handoff, and protected entry with each profile/capability combination and an absent or failing receipt store. Assert that every composition advertised as governed-with-audit refuses before protected effects, including after restart. If non-audited governed execution is intentionally allowed, assert that its public capability claim and receipts cannot be mistaken for the audited profile.

**Decision classification:** Small normative fix with a material enforcement consequence. It is not an optional wording nit.

## Scope limits and false fixes to reject

- A shared evaluator is not inherently a second authority or a god machine. It becomes one if it owns operation lifecycle, invents active policy state, or replays effects from security receipts. The repairs above must retain existing feature owners.
- A separate append-only receipt representation is not inherently a duplicate recovery journal. It becomes one if absence/presence of its rows decides execution, retry, completion, or recovery outside the owning operation protocol. The ADR must say how the two records are coupled, not merely forbid a second journal.
- Exact Rust type names, policy backend, storage engine, and cryptographic format can remain open. The findings demand ownership, ordering, outcome, and failure semantics before an implementation can choose those details consistently.
- The current gate and native-tool disable observations in ADR lines 45 and 434-435 are supported by `crates/meerkat/src/factory.rs:7212-7223` and `:7613-7640`. That evidence proves placement and a current disable condition, not general resource authorization, durable one-use permits, or complete audit.
- Generated declaration coverage cannot alone prove effect coverage. The tracer must enumerate actual owners, including dynamic tool bindings, async operations, deferred session effects, model attempts, and disclosure paths. Do not satisfy the tests with a wrapper or test-only lock that production owners bypass.
- Migration at lines 374-377 remains a proposal. An eventual cutover must separately test old-work refusal/migration, old binaries against new durable versions, retained summaries with unknown contributors, and restored work carrying its exact profile. No migration or security enforcement was established by this review.

## Review basis

Canonical doctrine used: `docs/architecture/meerkat-dogma.md`, especially lines 62-148, 224-253, 255-339, and its contract/generation rules. Commentary used: `docs/architecture/meerkat-dogma-commentary.md`, Chapters 1, 2, 3, 7, 8, and 9, especially the typed handoff, mechanical recovery, policy ownership, cancellation, and production-ratchet sections. `docs/internal/design/caller-context.md` was read as the current propagation companion, not treated as implementation evidence.

Repository content was not edited. This report is the only written artifact.
