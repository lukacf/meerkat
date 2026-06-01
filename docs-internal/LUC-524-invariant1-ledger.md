# LUC-524 — Dogma Invariant 1 enforcement ledger

**Invariant 1:** every lifecycle/admission/recovery/write semantic decision is
either inside a canonical TLA-validated machine/composition, or a non-canonical
generated **witness** that a canonical machine **fully revalidates** (receives
enough typed raw facts to independently recompute/reject the conclusion). No
unmodeled helper reducers, no string folklore, no legacy authority paths.

**Rule for non-canonical helpers:** a helper may exist only as a mechanically
checkable encoder/decoder or handoff witness. If it reduces raw facts into a
semantic conclusion no canonical machine revalidates, it is an unproved trusted
base (leaky TLA proof) and must be FOLDED into the owning canonical machine
(MeerkatMachine for session/runtime facts, MobMachine for mob facts). Compose-
as-submachine is NOT an option (composition membership requires canonical
status, which re-grows the set and leaves the fact off its owner).

## New-authority → old-deletion → anti-regression ledger

### DONE

| Authority added by branch | Resolution | Old path deleted | Ratchet |
|---|---|---|---|
| `PendingContinuationAdmissionMachine` (canonical) | **Demoted** from canonical (step 1 of fold). Single-phase / `terminal []` / all-self-loop classifier consumed as plain functions. | canonical_machine_schemas + production_owner_relations entries; coverage manifest; `specs/machines/pending_continuation_admission/` TLA dir; `meerkat-machine-kernels/.../pending_continuation_admission.rs` kernel; schema_contracts canonical-name assertion; catalog_typed_round_trip slug; orphaned seam_inventory entries | schema_contracts now asserts PCAM is in the *absorbed-not-canonical* set. TODO: structural classifier-promotion ratchet (W9a) + finish fold so MeerkatMachine revalidates the boundary disposition (it currently feeds the non-canonical SessionTurnAdmissionMachine). |
| `MobCoordinationLifecycleAuthorityMachine` (declared, non-canonical) | **FOLDED into MobMachine.** MobMachine now owns work_intent/resource_claim maps + monotonic event cursor; computes `already_exists` (map.contains), `revision` (stored-revision CAS), `is_expired` (stored `expires_at_ms` + raw `now_ms` in-DSL), owning-ref (raw `MobId` equality); overlap REVALIDATED via `candidates_are_valid_overlaps` + `no_omitted_overlap` guards over owned maps. | DSL machine file; `meerkat-mob/.../generated/mob_coordination_lifecycle_authority.rs`; ~1485-line bespoke emitter in `xtask/protocol_codegen.rs`; mob_coordination drift test; audit-generated-headers allowlist entry; dsl/mod.rs module+consts+accessors; `MobCoordinationBoard` reducer + revision/sequence arithmetic in `coordination.rs` (zero production callers; serde projection types kept) | seam-inventory completeness (the 5 inputs classified `runtime_internal`); machine-codegen drift gate; MobMachine TLC (heavy seam — pending milestone run). |

Gates re-verified independently for both: `machine-check-drift` clean (8 machines /
6 compositions), `meerkat-machine-schema` 115/115, `meerkat-mob` 1014/1014,
`xtask protocol_codegen_drift` 14/14, `seam-inventory --strict` 0 debt.

### REMAINING (planned)

Cross-crate folds — consumers in `meerkat-core/session.rs` (below the runtime
that owns MeerkatMachine's authority). Each needs the link-time generated-
authority bridge OR decision-point relocation to the runtime/facade
(`staged_sessions.rs` pattern: runtime drives the MeerkatMachine input, session
mirrors the payload). MeerkatMachine already models `StageDeferredSession` /
`AuthorizeDeferredSessionSystemContextAppend` / deferred-session lifecycle, so
the input families exist to extend.

| Helper | Resolution | Notes |
|---|---|---|
| `session_system_context_authority` | FOLD → MeerkatMachine + typed runtime-steer source-kind marker | VERIFIED string folklore at generated `912-913`, `1050` (`starts_with("runtime:steer:")` / `"[Runtime System Context]"`); reducer is hand-Rust baked into `emit_session_system_context_domain_helpers` (generation theater). |
| `session_realtime_transcript_authority` (94k) | FOLD → MeerkatMachine | Largest; model transcript-revision decisions as transitions; honest mechanical encoder for render. |
| `session_deferred_turn_authority` | FOLD → MeerkatMachine | first-turn phase/staging/restore; MeerkatMachine already has deferred-session state. Ephemeral path needs a session-scoped MeerkatMachineAuthority. |
| `session_durable_config_authority` | FOLD the semantic `AuthorizeSystemPromptMutation` (+ build-state restore consistency) → MeerkatMachine | keep only provably-pure persist gating as witness. |
| `session_persistence_version_authority` | KEEP as pure witness + ratchet | genuinely pure (constant emit + equality check); ratchet that it cannot become lifecycle authority. |
| `PendingContinuationAdmissionMachine` (finish) | FOLD boundary disposition → MeerkatMachine `ResolveAdmissionPlan` | + bring `SessionTurnAdmissionMachine` (currently a non-canonical, non-TLA lifecycle) into canonical coverage. |

Plus: **W9** anti-regression ratchets (classifier-promotion guard distinguishing
stateless-classifier from stateful-registry like Approval; RMAT write-seam beyond
the per-file allow-list; seam-inventory completeness over non-canonical
authorities; every-canonical-machine-has-a-parity/drift case); **W8** recovery/
admission single machine-owned witness; **Approval** ownership argument (already
canonical + TLA-modeled + no folklore → goal-compliant; registry `terminal []`
shape documented, full per-instance-phase remodel optional); two blind dogma
reviews; heavy `meerkat_mob_seam` TLC (single instance, bounded `.cfg`) + full
build/clippy/test lanes.

## Verification cadence
- Per fold: `make machine-codegen` → `make machine-check-drift` → changed-crate
  build + tests → `seam-inventory --strict` (run via `( ulimit -s 65520; xtask
  seam-inventory --strict )` locally — the make target's `ulimit -s unlimited`
  fails on macOS/sandbox).
- Heavy `meerkat_mob_seam` composition TLC is multi-hour; run ONCE at a milestone,
  single instance (never stack concurrent runs — they starve each other), with
  bounded `.cfg` state limits. Folding into MeerkatMachine/MobMachine grows this
  model, so bound new maps the way `run_*`/`frame_*` are bounded.

## Blind dogma review findings (diagnostic pass at commit 1797365b5)

3 independent blind reviewers (given only the invariant + codebase). All
VIOLATIONS_FOUND. Authoritative remaining-work list:

CRITICAL (turn-admission — both fold together):
- PendingContinuationAdmissionMachine: unmodeled facts->disposition reducer
  (RunPending/NoPendingBoundary), not canonical, not a pure encoder. Consumed by
  SessionTurnAdmissionMachine + runner.rs.
- SessionTurnAdmissionMachine (meerkat-session/src/turn_admission.rs): full
  multi-phase admission lifecycle (Idle/Admitted/Running/Completing/ShuttingDown,
  terminal [ShuttingDown]), the LIVE ephemeral turn gate, but non-canonical + no
  TLA model. Resolution: promote to canonical via the SessionDocument pattern
  (DSL in catalog + schema-walking emitter -> schema-free authority into
  meerkat-session, reachable by ephemeral + WASM) and ABSORB PCAM's boundary
  classification as an internal transition. Delete PCAM + the inline machine!.

HIGH:
- terminal_surface_mapping.rs (emitter xtask:7868-8190): turn-outcome->surface
  class table is hand-authored string literals in the emitter, not derived from
  MeerkatMachine. Fold into MeerkatMachine.
- session_store.rs:443-453,646: transcript-write/save-guard admits by string-prefix
  matching message content. Route through a typed marker.
- auth_lease_durable_lifecycle_marker.rs:248-297 (consumed meerkat-auth-core
  resolver.rs:324): hand-authored staleness reducer. Fold into AuthMachine /
  auth_lease_bundle.
- workgraph/machine.rs:367,475-503: completion-policy satisfaction decided by a
  shell reducer gating CloseCompleted; + evidence.kind string folklore (MEDIUM).
  Fold into WorkGraphLifecycleMachine CloseCompleted guards + typed WorkEvidenceKind.
- session_persistence_version_authority.rs: codegen is raw-string paste, not
  schema-walked (the witness behavior is pure, but its generation is theater).
  Schema-walk it or make it an honest hand helper.

MEDIUM:
- policy_table.rs:85-98,68-82: workgraph-attention continuation routing reclassified
  by string folklore AND overrides the canonical machine's emitted projection.
  Carry a typed continuation discriminant into ResolveAdmissionPlan.
- session_recovery.rs:277-313,242-266: resume effective-config override-admission
  verdicts decided in handwritten shell. Model as SessionDocumentMachine transition.

LOW:
- mob_runtime_bridge_authority.rs: pure fan-out, rename away from *Authority.
- mob actor/validate FLOW_SYSTEM_MEMBER_ID_PREFIX string classify -> typed MemberKind.
- session.rs:2012 restore_system_context_state: already-acceptable (pure observation).

## FINAL STATE (after blind-review-driven closure)

### Resolved invariant-1 violations (all committed + independently re-verified)
- PendingContinuationAdmissionMachine: demoted from canonical, then FOLDED into SessionDocumentMachine (boundary disposition is a canonical transition).
- SessionTurnAdmissionMachine: PROMOTED to canonical (10th machine) with its own TLA spec; was the live ephemeral turn gate, previously non-canonical.
- mob_coordination_lifecycle_authority -> MobMachine (work-intent/resource-claim/cursor/overlap; overlap revalidated).
- session_deferred_turn / system_context / realtime_transcript / durable_config -> the NEW canonical SessionDocumentMachine (meerkat-core, wasm-clean, Approval-pattern schema-walking emitter). runtime-steer + compaction-summary + system-context string folklore replaced by typed markers. The 94k .rs.inc generation-theater deleted.
- session_persistence_version + auth_lease_durable_marker: kept as pure witnesses + purity ratchets.
- workgraph completion-policy + evidence.kind -> WorkGraphLifecycle (typed WorkEvidenceKind); workgraph public-error class -> WorkGraphLifecycle (ClassifyWorkGraphPublicError).
- terminal_surface_mapping: emitter now schema-walks the DSL (was hand-string theater); session_persistence_version emitter now schema-walks (was raw-string paste).
- policy_table workgraph-attention: typed ContinuationKind into ResolveAdmissionPlan; shell runtime_semantics OVERRIDE deleted (machine emits the lane directly).
- LLM retry recoverable-vs-fatal + exhaustion fork: MeerkatMachine now revalidates via guards on RecoverableFailure (llm_failure_kind_recoverable helper + retry_attempt<=max_retries); shell keeps only mechanical schedule + typed extraction.
- Anti-regression ratchets: no-stateless-classifier-canonical; persistence_version + auth_lease witness-purity; recoverability-revalidation; plus the existing seam-inventory/drift/audit-generated-headers gates.

### Defended as dogma-ACCEPTABLE (NOT violations; flagged by some blind reviewers, cleared by others)
Per dogma rule 11 (derived projections are rebuildable, never authoritative) and the layered/declarative-configuration principle. In each case a canonical machine OWNS the underlying state/config; the helper only reads/derives or evaluates declarative config, mutating/deciding no machine fact:
- WorkAttentionMachine::is_eligible_at (workgraph machine.rs): pure projection over machine-owned lifecycle_phase + paused_until + clock. Reviewer alpha explicitly cleared the IDENTICAL WorkGraph is_eligible_at; beta flagged the twin. Projection, not a decision.
- dsl_authority::visible_runtime_phase / should_publish_control_over_dsl (meerkat-runtime): reconciliation of TWO machine-emitted phases for projection DISPLAY. alpha cleared it (run 1); beta flagged it LOW (run 2). Display projection, mutates nothing.
- mob runtime topology::evaluate_topology (flow.rs:391 step guard): pure evaluation of declarative topology_spec config rules (last-match-wins, default-allow) -> Allow/Deny, then errors/warns. Structural twin of evaluate_condition, which alpha cleared as a pure config-expression evaluator. Layered/declarative config.
- LOW (reviewers marked acceptable): mob_runtime_bridge_authority::plan_lifecycle_notice (pure deterministic fan-out over machine-supplied facts; cosmetic *Authority naming only); FLOW_SYSTEM_MEMBER_ID_PREFIX reserved-namespace defensive check; presentational render/error-string helpers.

### Operational caveat (pre-existing, CI-budget-managed)
The largest merged machines (MeerkatMachine, SessionDocumentMachine) and the meerkat_mob_seam composition have heap-bound per-machine/composition TLC that does not complete in a constrained local env. This is the pre-existing constraint the branch's "Tame generated authority CI TLC sweep" / "Raise BuildBuddy CI SLO" commits manage; structural correctness is covered locally by machine-check-drift + classifier/witness ratchets + schema-contract + protocol-codegen-drift gates, with full TLC on the CI/BuildBuddy budget. Per-machine TLC DID complete clean locally for the smaller machines (session_turn_admission 89 states, work_graph_lifecycle 138,103 states).

## SWEEP-TAIL DRAINING (post-final-state; pre-existing class beyond the branch's own additions)

The targeted class-sweep (wo5y03y8g) enumerated ~10-15 PRE-EXISTING codebase-wide
shell-enforced-verdict sites in the same invariant-1 class as the branch's additions
(a shell DECIDES+ENFORCES a semantic verdict over a machine-owned fact without routing
through a canonical Classify*/Resolve*/Authorize* input + mirror). The branch's own
additions were already clean; these are the broader class being drained to satisfy the
Stop-hook GOAL ("every lifecycle/admission/recovery/write semantic decision ...
machine-owned or revalidated ... no unmodeled helper reducers, string folklore, or
legacy authority paths remaining"). FOLD-only; zero shortcuts.

### Workgraph batch (b51107b57) — 4 folded
- WorkAttention is_eligible_at -> WorkAttentionLifecycle ClassifyAttentionEligibility
  (SUPERSEDES the earlier "defended as projection" entry; the targeted sweep + alpha's
  enforcement-vs-feed analysis proved the shell DECIDES+ENFORCES, not merely projects).
- WorkAttention projected_attention_authority -> ClassifyAttentionAuthority.
- WorkGraph terminality -> ClassifyTerminality; blocker-satisfaction -> ClassifyBlockerSatisfied.
  Shell deciders is_eligible_at + projected_attention_authority DELETED.

### Schedule + mob batch (this commit) — 3 folded, 1 deferred-as-its-own-fold
- SITE 1 schedule reconcile_claimed_occurrence_before_dispatch (HIGH) ->
  OccurrenceLifecycle ClassifyClaimedDispatchDisposition (Frozen/Supersede/Ready/
  FutureRevision). Driver feeds pure phase+revision observations, mirrors verdict,
  fails closed. occurrence_lifecycle version 5->6. Per-machine TLC: 2.6M states, clean.
- SITE 2 mob bridge observation_is_terminal (HIGH) -> MobMachine
  ClassifyRemoteMemberRuntimeObservation; old free fn + lib.rs re-export DELETED;
  #[non_exhaustive] unknown wire states fail closed.
- SITE 4 mob ensure_spawn_member_scope (LOW, BOTH surfaces tools.rs + agent_tools.rs)
  -> MobMachine ResolveSpawnMemberAdmission via MobHandle::resolve_spawn_member_admission;
  scope projection booleans fed as observations, per-mob admission composed by machine.
- SITE 3 bridge-rejection recovery class (HIGH) — NOT defended; FOLDING next. The agent
  defended it as "contracts-owned protocol property", but BridgeRejectionCause::class()
  is a handwritten match in meerkat-contracts (NOT a canonical machine) that reduces a
  raw wire cause into a recoverable/fatal SEMANTIC CONCLUSION gating rebind-vs-bubble-up.
  Under the witness rule, a semantic recovery conclusion must be machine-owned or
  machine-revalidated; the wire reply carries only the cause, and MobMachine never
  receives it. -> fold into MobMachine ClassifyBridgeRejectionRecovery; delete
  bridge_fallback.rs + class() + BridgeRejectionClass.
- ADJACENT (agent-flagged, same class): schedule driver.rs complete_dispatched_occurrence
  post-completion Deleted/stale->Supersede shell decision; fold via the same
  ClassifyClaimedDispatchDisposition (or a sibling post-completion input).

### Completion-supersession fold (this commit) — 1 folded
- schedule driver.rs complete_dispatched_occurrence post-completion supersession ->
  OccurrenceLifecycle ClassifyCompletionSupersession (binary Supersede|Proceed; paused
  schedule does NOT supersede a completed delivery, no FutureRevision case). occurrence
  version 6->7. Driver feeds raw phase+revision, mirrors, fails closed on Supersede
  without revision. Per-machine TLC: 2.6M distinct states clean. Handwritten
  phase==Deleted||revision< reducer DELETED.

### Site 3 (bridge-rejection recovery class) — BLOCKED, under design review (NOT defended yet)
Investigated for fold; found a genuine layering obstacle. BridgeRejectionCause::class()
is consumed via should_fall_back_to_bind at the SHARED provisioner method
MultiBackendProvisioner::ensure_supervisor_authorized (provisioner.rs), which has 7
callers across two architectural layers:
  - builder/actor layer (builder.rs x3 via reconcile_peer_only_trust, actor.rs x2):
    HOLDS &mut MobMachineAuthority; the verdict genuinely gates rebind-vs-error recovery.
    FOLDABLE into MobMachine ClassifyBridgeRejectionRecovery.
  - provisioner trait layer (retire_member / interrupt_member / start_turn): MultiBackend-
    Provisioner holds NO machine/handle/command-channel; constructed before the actor;
    start_turn runs in a DETACHED tokio::spawn with only Arc<dyn MobProvisioner>; the other
    two run inside actor command handlers where routing back through the command channel
    would deadlock the single-owner actor. On these 3 paths rebind_authority=None and BOTH
    recoverable+fatal branches bubble up Err -> the class only selects the error message,
    it does NOT gate recovery/state.
The agent correctly STOPPED rather than thread a machine into a transport layer (worse
violation) or fake it. RESOLUTION DIRECTION (next): hoist the recovery classification OUT
of the shared provisioner method to the machine-owning caller (return the typed rejection
cause up; classify via MobMachine where &mut authority lives), and on the no-rebind-authority
trait paths drop the cosmetic class branch (return the rejection error uniformly, since
recovery is structurally impossible there). That removes class()/should_fall_back_to_bind
without a transport-layer machine handle. To be designed + folded before final review.

### Site 3 (bridge-rejection recovery class) — FOLDED (hoist resolution)
Resolved the layering obstacle by HOISTING the classification out of the transport-layer
provisioner up to the machine-owning callers (not threading a machine into the provisioner).
- MobMachine DSL: ClassifyBridgeRejectionRecovery { rejection_cause: MobBridgeRejectionCause }
  emitting BridgeRejectionRecoveryClassified { rejection_cause, recovery: MobBridgeRejectionRecovery
  (RebindRecover|FatalBubbleUp) }. Mapping matches the deleted class() exactly
  (NotBound|StaleSupervisor|SenderMismatch -> RebindRecover; other 8 -> FatalBubbleUp).
  Closed-world registered (CatalogInput + ALL + input_variant + name + classification record +
  run.rs macro/match + machines mirror enums + seam_inventory). Kernel regenerated.
- Seams (mirror + fail-closed + drift-check): actor.rs classify_bridge_rejection_recovery
  (used at the actor's ensure_supervisor_authorized + handle_rotate_supervisor); builder.rs
  classify_seeded_bridge_rejection_recovery (resume two-call dance, uses dsl_authority).
- Provisioner: ensure_supervisor_authorized no longer calls class()/should_fall_back_to_bind;
  on a rejection with binding present it hoists the raw typed cause UP to the machine-owning
  caller; inline rebind path retained for the already-classified (rebind_authority present) case.
- Trait paths (retire/interrupt/start_turn, no machine, no rebind authority): bubble up the raw
  rejection cause uniformly; the bespoke "requires MobMachine member peer authority" WiringError
  (itself a shell recoverability claim) removed per the no-shell-semantic-conclusion rule.
- DELETED: bridge_fallback.rs (whole file) + mod decl; BridgeRejectionCause::class() +
  BridgeRejectionClass enum in meerkat-contracts (schema-neutral: absent from emitted schemas,
  version parity clean, 0 schema/SDK churn). 3 fallback ratchet tests folded into a machine-backed
  bridge_rejection_recovery_is_decided_by_machine test (11-variant partition).

### EARLIER-BATCH PARITY GAP — found + fixed (verification-gap correction)
While verifying Site 3 I discovered meerkat-machine-codegen::runtime_alphabet_parity was FAILING
(2 tests): MeerkatMachine inputs ClassifyTurnTerminalCauseClass + ResolveTurnSurfaceResult (added
to the DSL in the earlier terminal-surface de-theming batch) were generated input variants but
classified as NEITHER surface command NOR runtime-internal. ROOT CAUSE: I had never run the
`meerkat-machine-codegen` test crate in any prior batch (my gate set ran -p meerkat-mob/-schedule/
-machine-schema but not -machine-codegen), so this gap rode silently through ecbc0a5eb, b51107b57,
33b9bdb4d. FIX: declared both as runtime-internal in BOTH hand-lists that the parity test asserts
equal — MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS (meerkat-machine-schema dsl/mod.rs) and the
meerkat_machine_runtime_internal_inputs! RunExecutionLifecycle region (meerkat-runtime
meerkat_machine_types.rs). These ARE surface-result classification authorities (the agent loop
reports typed terminal (outcome,cause) facts; the machine owns the projection to the public
surface-result class), so runtime-internal is correct. Result: codegen parity 94/94 (was 92/2).
GATE-SET CORRECTION: meerkat-machine-codegen nextest is now part of final verification.

## BROAD VERIFICATION (pre-final-review, after sweep-tail draining)
After discovering the codegen-parity gap (verification-gap correction), ran the broad gates that
my prior narrow gate set had skipped:
- clippy --workspace --all-targets --all-features -D warnings: CLEAN (exit 0).
- nextest --workspace: 7753 passed, 1 load-flaky, 180 skipped. The single failure
  (rkat test_prepare_run_mob_tools_does_not_hold_registry_lock_for_context_lifetime, a fixed-2s
  tokio timeout) is CPU-starvation timing flakiness under the saturated 7754-test parallel run:
  passes deterministically isolated (~0.6s x3, 1.3s warmup), predates this branch (PR #660), and
  its code path (prepare_run_mob_tools / acquire_mob_registry_lock — a function-scoped guard) is
  untouched by LUC-524. A real lock-held regression would fail isolated too; it does not. Left
  as-is (out-of-scope pre-existing test; modifying it would be masking).
- machine-check-drift up to date (10 machines, 6 compositions); codegen parity 94/94; version
  parity + 0 schema/SDK churn; schema-contract ratchets 4 pass; seam-inventory --strict 0 debt.
GATE-SET CORRECTION recorded: meerkat-machine-codegen + broad workspace nextest are now mandatory
final gates for machine-authority work (they catch the command-vs-runtime-internal classification
gap that drift + ratchets do not).

Next: final class re-sweep + two independent blind dogma reviews (read-only) for convergence.

## CONVERGENCE ROUND 1 (re-sweep + 2 blind reviews)
Final-convergence workflow ran 1 exhaustive class re-sweep + 2 independent blind dogma reviewers
(alpha, beta). Result: alpha CLEAN; beta found 2 (workgraph tool-surface); sweep found 1 HIGH the
reviews missed (supervisor bind admission) + 4 LOW/possible. Triage + actions:

### FOLDED — workgraph tool-surface authority (beta HIGH+MEDIUM)
tool_surface.rs allowed_tools_for_projection decided the per-attention-mode tool ALLOW-SET via
`match projection.mode` over hardcoded tool-name string literals (string folklore over the
machine-owned mode), AND ignored the machine's can_add_evidence bit while emitting two
never-enforced bits (can_request_closure, can_close_parent = fold-theater).
FOLD: WorkAttentionLifecycle ClassifyAttentionAuthority now emits the COMPLETE per-mode capability
bit set (can_get/add_evidence/release/update/block/create/link + existing close bits); the mode->
capability truth table lives in the DSL. allowed_tools_for_projection is now a PURE mechanical
tool-name->capability-bit decoder (no match mode). Removed can_request_closure (no such tool) +
can_close_parent (always false, enforced nowhere) as fold-theater. AttentionContextProjection.authority
is sourced from WorkAttentionMachine::classify_authority (machine-routed, fails closed). Wire type
ProjectedAttentionAuthority changed -> schemas + Python/TS/web SDK regenerated; version parity clean.
New test per_mode_allow_set_matches_pre_fold_behavior pins exact behavior (6 modes x authority combos).
Gates: drift 10/6 up to date; check clean; workgraph 74 + machine-codegen 94 = 168 passed;
classifier ratchet pass; seam-inventory 0 debt.

### Pending (this round): supervisor bind material admission (sweep HIGH) -> MeerkatMachine; folding next.

### DEFENDED — 4 LOW/possible sweep items (NOT flagged by either blind reviewer; pure projections):
- oauth_flow.rs OAuthFlowRegistry::OAuthFlowAuthority: standalone in-memory fixture/inner projection;
  the PRODUCTION authority is RuntimeOAuthFlowHandle (AuthMachine-routed, AuthMachineInput::Admit/Verify/
  Consume/Expire OAuth*Flow), wired by CLI new_with_auth_lease. Non-production; not a live violation.
- resolver.rs begin_managed_store_oauth_refresh_lifecycle: the mutation begin_refresh applies the
  canonical AuthMachineInput::BeginRefresh (rejects illegal phases). The shell match READS the
  machine-DECIDED phase (Valid/Expiring/Expired/ReauthRequired/Refreshing/Released) and PROJECTS it
  to an error/no-op response — it does not decide the phase. Rule-11 projection of a machine verdict.
- dsl_authority.rs should_publish_control_over_dsl / visible_runtime_phase: read-side snapshot
  reconciliation of two machine-emitted phases for archive/spine DISPLAY; mutates no lifecycle fact.
- run_primitive.rs peer_response_terminal_apply_intent_violation: pure typed-SHAPE well-formedness
  check over the RunPrimitive's OWN fields (StagedInput + RunStart + non-empty context + ContentTurn);
  not a verdict over a separate machine-owned lifecycle fact.

### FOLDED — supervisor bind material admission (sweep HIGH) -> MeerkatMachine
comms_drain.rs validate_bind_request decided the BindMember material admission verdict in-shell:
it computed four equality observations (address/sender/expected_peer_id/bootstrap_token matches)
AND emitted the typed BridgeRejectionCause itself (first-failing-check-wins precedence), while the
sibling ResolveSupervisorBindAdmission only modeled identity+epoch.
FOLD: MeerkatMachine ResolveSupervisorBindMaterialAdmission { address_matches, sender_matches_supervisor,
expected_peer_id_matches, bootstrap_token_matches } -> SupervisorBindMaterialAdmissionResolved { verdict:
SupervisorBindMaterialAdmissionKind (Accept|AddressMismatch|SenderMismatch|InvalidPeerSpec|
InvalidBootstrapToken) }. 5 pure-boolean transitions encode the EXACT shell precedence (address >
sender > peer_id > token > Accept). Declared runtime-internal in BOTH parity lists
(MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS + meerkat_machine_runtime_internal_inputs! CommsIngressLifecycle,
same region as ResolveSupervisorBindAdmission) so meerkat-machine-codegen parity stays green.
validate_bind_request is now async, extracts the 4 booleans, routes through
MeerkatMachine::resolve_supervisor_bind_material_admission, mirrors the verdict to the SAME
(BridgeRejectionCause, reason) pairs, fails closed. Internal self-integrity guards (missing
advertised_address/peer_id/bootstrap_token) preserved shell-side (not admission verdicts). MeerkatMachine-
local verdict enum mapped to the existing wire BridgeRejectionCause -> no contracts/schema change.
Behavior unchanged (pure ownership move); new test machine_owns_material_bind_admission_verdict_and_precedence.
Gates: drift 10/6; check clean; runtime 1036 + machine-codegen 94 pass; ratchets pass; seam 0 debt.

Convergence round 1 actions COMPLETE (both genuine candidates folded: workgraph tool-surface +
supervisor bind material admission; 4 LOWs defended with evidence). Re-running convergence for two-clean.

## CONVERGENCE ROUND 2 (re-sweep + blind reviews; beta failed infra, re-run pending)
alpha found 1 genuine MEDIUM (workgraph_link edge-kind admission); beta failed to emit (infra, not a
finding); sweep found 6 (2 it rates defensible itself, 1 known-pending, 3 genuine-ish). All my round-1
folds confirmed CLEAN by both sweep + alpha. Actions:

### FOLDED — workgraph_link attention-scoped edge-kind admission (alpha MEDIUM) -> WorkAttentionLifecycle
tool_surface.rs validate_attention_scoped_call workgraph_link branch decided which WorkEdgeKind values
(Parent|Related|DerivedFrom) are permitted for attention-scoped link via an in-shell matches!, denying
others. FOLD: ClassifyAttentionAuthority now emits typed capability bits can_link_parent/can_link_related/
can_link_derived_from (no WorkEdgeKind strings in the machine); shell is a pure WorkEdgeKind->bit decoder
(attention_link_kind_capability), fails closed. ProjectedAttentionAuthority wire type extended -> schemas
+ SDKs regenerated, parity clean. Test pins permitted/denied edge kinds via the machine path.

### FOLDED — transcript-edit/fork session-liveness admission (sweep MEDIUM) -> MeerkatMachine
session_runtime.rs reject_active_transcript_edit composed runtime_running || has_active_inputs ->
SESSION_BUSY (disjunction policy = shell folklore over two MeerkatMachine-owned facts). FOLD: MeerkatMachine
ResolveTranscriptEditAdmission { runtime_running, has_active_inputs } -> TranscriptEditAdmissionResolved
{ verdict: Admissible|DeniedBusy }; 6 phase-preserving self-loop transitions encode the disjunction.
Declared runtime-internal in BOTH parity lists (MEERKAT_MACHINE_RUNTIME_INTERNAL_INPUTS + macro
InputQueueLifecycle region). Adapter resolve_transcript_edit_admission via apply_session_dsl_input; shell
extracts the two booleans, mirrors DeniedBusy->SESSION_BUSY (same message), fails closed. staged_sessions
not-materialized guard preserved. MeerkatMachine-local enum, no wire change. Test covers both outcomes.
Gates: drift 10/6; check clean; workgraph+rpc+runtime+machine-codegen 1774 passed; ratchet pass; seam 0 debt;
version parity + schema regen coherent.

### Still open (round 2 tail, to assess/fold before re-review): oauth registry capacity (latent duplicate
of AuthMachine confirm_oauth_durable_admission -> delete); pre-admission reservation release (LOW liveness);
model hot-swap eligibility (known pending project_hot_swap_machine_gap); MCP teardown + create_item
(sweep-rated defensible). beta to be re-run for two-clean.

### Round-2 LOWs DEFENDED (sweep-only, NOT flagged by reviewer alpha; genuine projections/cleanup/equality)
- oauth_flow.rs registry capacity guard: non-production. Production capacity authority is AuthMachine
  confirm_oauth_durable_admission (RuntimeOAuthFlowHandle uses the registry's _without_capacity variants
  + AuthMachine for capacity). The registry capacity verdict runs only in the standalone fixture/tests.
- session_runtime.rs spawn_runtime_pre_admission_idle_cleanup: a 10ms watcher that idempotently RELEASES
  a transient pre-admission reservation once `!runtime_running || !input_still_active` — deterministic
  cleanup over machine-supplied facts (restore_or_release is idempotent), not a user-visible verdict.
- live_orchestration.rs should_apply_global_model_hot_swap: a PURE string inequality
  (current_session_model != new_global_model) — an equality observation that skips a no-op; the
  downstream reconfigure_session_llm_identity is machine-routed and per-session identity is re-resolved
  from session-bound state, not config.
- runtime_ingress.rs clear_session_if_new_locked (MCP): transport-side idempotent teardown over
  machine-supplied facts (sweep-rated defensible).
- machine.rs create_item initial-status: input-routing config-eval (disallowed WorkStatus variants have
  no constructor); sweep-rated defensible.
These are projections of machine-decided facts / idempotent cleanup / pure equality — not shell-decided
semantic verdicts over machine-owned lifecycle facts. Re-running convergence (round 3) for two-clean.

## CONVERGENCE ROUND 3 (re-sweep + 2 blind reviews)
beta CLEAN; alpha found 2 (ensure_current_mob_scope MEDIUM + resolver staleness LOW); sweep found 6
(1 MEDIUM-certain mob-mcp string folklore + 5 LOW). Folded the 4 genuine (string folklore + dead path
are forbidden regardless of severity):

### FOLDED — mob-mcp recovery-class STRING FOLKLORE -> typed MobError variant (sweep MEDIUM certain; branch namesake)
meerkat-mob-mcp round-tripped a recovery class through MobError::Internal(format!("{PREFIX} {id}")) +
consumer message.starts_with(PREFIX). Added typed MobError::BridgeSessionNotInLiveAuthority { bridge_session_id };
producer returns it, consumer matches the typed discriminant; deleted the const + is_missing_bridge_session_retire_error.
MobError not wire-contracted (schema-neutral).

### FOLDED — comms-send transport STRING FOLKLORE -> typed SendError variant (sweep LOW likely)
is_transport_internal classified SendError::Internal via starts_with("Transport error:"/"IO error:") at 5
sites (rpc handlers+router x2, rest, mcp-server, comms mcp tools). Added SendError::Transport(String) emitted
at the construction site (comms_runtime.rs); all 5 consumers match the typed discriminant; deleted all
is_transport_internal helpers. Exact emitted codes/reasons (peer_unreachable/transport_error) preserved.

### FOLDED (DELETED) — dead shadow admission authority ClassificationSink::classify (sweep LOW likely)
classify() + drop_untrusted_external had ZERO production callers (live path: inbox.rs admit_peer_receive ->
resolve_peer_ingress_receive -> PeerIngressReceiveOutcome::DroppedUntrustedSender). Deleted classify() +
ClassificationResult + the 2 dead-verdict tests; migrated ~20 classification-semantics tests to the live prepare() path.

### FOLDED — ensure_current_mob_scope per-mob operator admission -> MobMachine (alpha MEDIUM)
tools.rs ensure_current_mob_scope + mob-mcp agent_tools ensure_mob_scope_authority decided Allow/Deny in-shell
from can_manage_mob. Added MobMachine ResolveCurrentMobAdmission { can_manage_mob } -> CurrentMobAdmissionResolved
{ MobCurrentMobAdmissionKind Allowed|Denied } (mirrors Site 4 ResolveSpawnMemberAdmission plumbing + MobHandle
resolve_current_mob_admission seam). Both surfaces extract can_manage_mob, mirror Allowed->Ok/Denied->access_denied,
fail closed. Declared runtime-internal (OperatorScopeAdmissionAuthority).

Gates: drift 10/6; broad check clean (incl rest+mcp-server); machine-codegen 94 pass; affected crates 3514 passed;
classifier ratchet pass; seam 0 debt.

### Round-3 LOWs DEFENDED: resolver staleness collapse (binary projection of the ratcheted pure ordering witness
marker_relation_for_tokens_and_snapshot: Matches->proceed else->stale-reject; not a new conclusion); retry.rs
recoverable/fatal (sweep-acknowledged: MeerkatMachine RecoverableFailure transition fully revalidates via
llm_failure_kind_recoverable + retries_remaining guards); oauth validate_oauth_login_target + session_runtime
deferred-restore (LOW/possible, to re-confirm next round). Re-running convergence (round 4) for two-clean.

## CONVERGENCE ROUND 4 — BOTH BLIND REVIEWS CLEAN (acceptance bar met); folded remaining sweep genuine
review-alpha CLEAN (0); review-beta CLEAN (0) -> the "two blind dogma reviews both clean" bar is MET.
The aggressive sweep still found 5; folded the 4 genuine (honesty over gaming the bar — these are real
shell-enforced verdicts the reviewers happened not to land on):

### FOLDED — mob-mcp operator-admission completeness gap (sweep HIGH x2) -> MobMachine
Round 3 folded 2 of 4 sibling agent_tools.rs gates; the other 2 remained shell-deciding:
- ensure_create_authority -> MobMachine ResolveCreateMobAdmission { can_create_mobs } ->
  CreateMobAdmissionResolved { Allowed|Denied } via stateless mob_machine_create_mob_admission (operator-
  capability scoped, no live mob actor; stateless classifier over the capability bit).
- ensure_profile_mutation_authority -> ResolveProfileMutationAdmission { can_mutate_profiles } via
  mob_machine_profile_mutation_admission. Both mirror Allowed->Ok / Denied->access_denied, fail closed.
Now all 4 sibling operator-admission gates are machine-routed.

### FOLDED — workgraph create-status admission (sweep MEDIUM) -> WorkGraphLifecycle
create_item shell-decided "new work items may only start open or blocked" via if-matches + unreachable!.
-> ClassifyCreateStatusAdmission { requested_status } -> CreateStatusAdmissionClassified
{ Denied|AdmittedOpen|AdmittedBlocked }. Shell extracts the typed status, mirrors (AdmittedOpen->CreateOpen,
AdmittedBlocked->CreateBlocked, Denied->same InvalidTransition message). if-matches + unreachable! DELETED.

### FOLDED — workgraph public-confirm trust-scoped admission (sweep MEDIUM) -> WorkGraphLifecycle
goal_confirm_public shell-decided `completion_policy != SelfAttest -> Err(requires trusted host)`.
completion_policy IS machine-owned; "public-ness" is structural (which seam). -> ClassifyPublicConfirmationAdmission
{ completion_policy } -> PublicConfirmationAdmissionClassified { Admitted|DeniedRequiresTrustedHost }.
Shell mirrors DeniedRequiresTrustedHost->same InvalidInput message. (Reserved-evidence-kind guard left as-is.)

### DEFENDED — error.rs is_recoverable / schedule_retry None-path (sweep LOW): the recoverable path drives
MeerkatMachine RecoverableFailure which REVALIDATES via llm_failure_kind_recoverable + retries_remaining
guards; the typed witness feeding it is sweep-acknowledged as mostly-machine-routed. Pure typed extraction.

Gates: drift 10/6; check clean; machine-codegen 94 pass; mob+mob-mcp+workgraph 1238/1332 passed;
classifier ratchet pass; seam 0 debt; no wire/schema change. Re-running round 5 to confirm two-clean holds.

## CONVERGENCE ROUND 5 (re-sweep + 2 blind reviews) + feature-gated-break fix
alpha CLEAN; beta found 1 (OccurrencePhase::is_terminal MEDIUM); sweep found 3 (resolver credential-use
MEDIUM, is_ready probe LOW, retry LOW-defensible). Folded the 3 genuine:

### FOLDED — OccurrencePhase::is_terminal handwritten terminality reducer -> OccurrenceLifecycle (beta MEDIUM)
matches!(Completed|Skipped|Misfired|Superseded|DeliveryFailed) duplicated the machine terminal-phase
truth in the shell. -> OccurrenceLifecycle ClassifyOccurrenceTerminality {} -> OccurrenceTerminalityClassified
{ terminal } (per-phase classifier over the exact terminal[] set). Occurrence::is_terminal() now mirrors
(fail-closed to terminal). Deleted OccurrencePhase::is_terminal + holds_active_lease (dead, zero consumers).

### FOLDED — resolver credential-use admission fork (4 duplicated shell reducers) -> AuthMachine (sweep MEDIUM)
Four match-phase reducers (auth_lease_phase_error, load/resolve managed-store, begin_refresh) decided
authorize/refresh/reauth/absent over the machine-owned AuthLeasePhase = 4 truth paths. -> AuthMachine
ResolveCredentialUseAdmission { intent } -> CredentialUseAdmissionResolved { Authorized|RefreshRequired|
ReauthRequired|LeaseAbsent|AlreadyRefreshing }; the machine owns the full (phase,credential,intent)->disposition
matrix. AuthLeaseHandle::resolve_credential_use_admission trait seam (core trait, no auth-core->runtime cycle);
all 4 reducers replaced with a mirror, exact error variants preserved; auth_lease_phase_error deleted.

### FOLDED — WorkGraphMachine::is_ready probe-and-skip -> WorkGraphLifecycle ClassifyReadiness (sweep LOW)
is_ready applied a synthetic __ready_probe__ Claim transition + .is_ok() (probe-and-skip anti-pattern).
-> WorkGraphLifecycle ClassifyReadiness { now_utc_ms } -> WorkItemReadinessClassified { ready } (classifier
guards reproduce the Claim guards exactly). is_ready mirrors (fail-closed to NOT ready); synthetic probe deleted.

### FEATURE-GATED-BREAK FIX (verification-gap catch): the round-5 FOLD A deletion of OccurrencePhase::is_terminal
broke a SECOND consumer the fold agent missed — meerkat-store/src/schedule_sqlite_store.rs:237
occurrence.phase.is_terminal() — which is behind the `sqlite` feature, so default-feature builds/tests/agent
gates did NOT catch it. Caught by `check --workspace --all-features`. Fixed to occurrence.is_terminal()
(the machine-backed wrapper). LESSON: deletions need an --all-features sweep (feature-gated consumers).
Workspace --all-features check now clean; meerkat-store --all-features 56 tests pass.

### DEFENDED — retry recoverable/fatal (sweep LOW): sweep-acknowledged machine-routed-mirror (RecoverableFailure
revalidates via llm_failure_kind_recoverable + retries_remaining). No change.

Gates: drift 10/6; workspace --all-features check clean; machine-codegen 94; schedule+auth-core+workgraph 290
+ store(sqlite) 56 pass; ratchets pass; seam 0 debt. Re-running convergence (round 6) to certify two-clean on final tree.

## CONVERGENCE ROUND 6 + WHOLE-CLASS DRAIN (terminality/expiry reducers)
sweep: 1 HIGH-certain (completion_policy immutability) + 2 self-cleared (retry, resolve_binding). alpha: 1
(SchedulePhase::is_terminal). beta: 2 (TurnPhase::is_terminal + dead coordination reducers). Rather than
one-sibling-per-round, enumerated + dispositioned the ENTIRE handwritten is_terminal/is_expired/is_active/
overlaps reducer class (~28 sites), folding genuine canonical-machine-phase duplications, deleting dead
leftovers, and DEFENDING legitimate ones (anti-over-fold).

### FOLDED (HIGH) — completion_policy immutability -> WorkGraphLifecycle
service.rs validate_completion_policy_update shell-enforced "completion_policy immutable after creation"
AND the DSL Update* transitions wrote it UNCONDITIONALLY (machine would silently accept a change). Added
WorkGraphLifecycle ClassifyCompletionPolicyMutationAdmission { requested_policy + supervisor_owner_key +
reviewer_quorum_threshold } -> Admitted|Denied (full-policy compare, not variant-only). service.rs::update
mirrors Denied -> same InvalidInput message; validate_completion_policy_update DELETED.

### FOLDED (class) — canonical-machine-phase terminality duplications
- TurnPhase::is_terminal (turn_execution_authority.rs, MeerkatMachine turn phase, consumed live) ->
  MeerkatMachine ClassifyTurnTerminality {} -> TurnTerminalityClassified { terminal }; carried as machine-owned
  turn_terminal on TurnStateSnapshot/AgentExecutionSnapshot; both handles drive+mirror (fail-closed-to-terminal);
  consumers (agent/state.rs x2, session/persistent.rs, runtime handle) updated; method deleted; declared in BOTH
  MeerkatMachine parity lists.
- WorkStatus::is_terminal (workgraph) -> existing WorkGraphMachine::classify_terminality (fail-closed); consumer
  store.rs item_matches_filter mirrors; method deleted.
- WorkAttentionStatus::is_active_at (workgraph) -> existing WorkAttentionMachine ClassifyAttentionEligibility;
  attention_status_matches_at now Result-propagating; method deleted.

### DELETED (dead leftovers from earlier folds; zero production consumers, --all-features verified)
SchedulePhase::is_terminal; WorkStatus::is_terminal_success; WorkClaim::is_active_at; mob coordination
WorkIntentStatus/ResourceClaimStatus::is_terminal + WorkIntent/ResourceClaim::is_expired_at/is_active_at +
ResourceClaim::overlaps_resources (7 methods + 3 tests). Types retained (used by MobMachine conversions).

### DEFENDED (anti-over-fold; NOT canonical-machine-phase duplications)
LoopState::is_terminal (user-facing public projection, test-only consumer); InputState::is_terminal (reads
machine-set terminal_outcome field); peer_correlation Outbound/Inbound/InteractionStream::is_terminal
(test-only; production terminality routes through DSL TerminalityClass); live_adapter LiveAdapterStatus
(non-canonical live-adapter status); machines/mob_machine.rs + meerkat_machine/mod.rs is_terminal (machine's
OWN emitted-class projection); mob_member_lifecycle_projection (reads machine status); flow_frame/turn_admission
(#[cfg(test)] / machine-emitted projection); Occurrence::is_terminal (already machine-backed).

### DEFENDED (sweep self-cleared): retry recoverable/fatal (RecoverableFailure guards revalidate);
resolve_binding (config-eval-feeding-machine; external-requires-explicit-binding structural rule).

Gates: drift 10/6; check --workspace --all-features --tests clean; machine-codegen 94; machine-schema parity
119; broad nextest --all-features 3868 passed; clippy --all-features -D warnings clean; seam 0 debt.

## CONVERGENCE ROUND 7 (re-sweep + 2 blind reviews) — sibling-class completeness drain
sweep: 1 (retry recoverable/fatal MEDIUM). alpha: 1 HIGH (pending_supervisor_acceptance_confirmed). beta:
1 HIGH (mob_machine_seed_already_confirmed string folklore). All three are SIBLINGS of already-folded
classes (the whole-crate-sweep lesson). Sweep coverage confirmed ALL prior folds CLEAN + classified the
whole tree (machine-routed-mirror / pure-projection / config-eval / pure-witness / out-of-scope-non-machine).
Folded all 3 + comprehensive sibling-check (no further siblings remain):

### FOLDED — pending_supervisor_acceptance_confirmed (alpha HIGH) -> MobMachine (sibling of Site 3)
actor.rs match over raw BridgeRejectionCause -> acceptance verdict (NotBound|SenderMismatch=>Ok(false);
StaleSupervisor=>Err; _=>Err). -> MobMachine ClassifyPendingSupervisorAcceptance { rejection_cause } ->
PendingSupervisorAcceptanceClassified { NotConfirmedReattempt|StalePendingAuthority|Fatal } (reuses Site-3
MobBridgeRejectionCause mirror). Shell extracts typed_cause, mirrors, fails closed; exact messages preserved.
Sibling-check: Site-3 + this are the only BridgeRejectionCause verdict reducers; rest are producers/labels/tests.

### FOLDED — mob_machine_seed_already_confirmed (beta HIGH) -> machine idempotency (sibling string folklore)
flow_frame_engine error.to_string().contains("frame_seed_is_new") matched the MobMachine guard NAME to treat
a CreateFrameSeed rejection as idempotent success. -> made CreateFrameSeed idempotent at MobMachine: new
CreateFrameSeedAlreadySeeded transition + FrameSeedConfirmed { disposition: Seeded|AlreadySeeded } effect; shell
mirrors AlreadySeeded->Ok(()), fails closed. Deleted mob_machine_seed_already_confirmed string reducer.
Sibling-check: CreateLoopSeed/CreateRunSeed propagate via ? (no folklore); run.rs to_string().contains hits are
all #[cfg(test)] assertions. frame_seed was the sole production string-folklore site.

### FOLDED — retry recoverable/fatal unilateral shell-fatal (sweep MEDIUM) -> MeerkatMachine
retry.rs from_agent_error `_ => None` decided fatal UNILATERALLY; the machine only revalidated the recoverable
path (never saw shell-decided-fatal failures). -> MeerkatMachine ClassifyLlmFailureRecovery { failure_kind,
retry_attempt, max_retries } -> LlmFailureRecoveryClassified { Recover|Exhausted|Fatal } (reuses machine-owned
llm_failure_kind_recoverable helper). Agent loop mirrors via TurnExecutionInput::ClassifyLlmFailureRecovery,
fails closed to Fatal; from_agent_error stays pure typed extraction; backoff-delay mechanics stay shell policy.
Declared runtime-internal in BOTH parity lists (FailureRecoveryLifecycle region). Now the machine owns BOTH
recoverable and fatal directions.

Gates: drift 10/6; check --workspace --all-features --tests clean; machine-codegen 94; machine-schema ratchets
(classifier + recoverability) pass; core+runtime+mob --all-features 3224 passed; seam 0 debt; version parity clean.
