# LiveRequestMachine

_Generated from the Rust machine catalog. Do not edit by hand._

- Version: `25`
- Rust owner: `self` / `catalog::dsl::live_request`

## State
- Phase enum: `Ready`
- `ingress_open`: `Bool`
- `grant_id`: `String`
- `grant_generation`: `u64`
- `grant_expiry`: `u64`
- `grant_revoked`: `Bool`
- `executor_binding`: `String`
- `grant_record`: `String`
- `grant_profile_revision`: `String`
- `grant_evidence`: `Set<LiveRequestEvidenceKind>`
- `grant_mutations`: `Set<ToolMutationClass>`
- `grant_tools_restricted`: `Bool`
- `grant_tools`: `Set<String>`
- `grant_max_requests`: `u64`
- `grant_max_concurrent_requests`: `u64`
- `grant_max_effects`: `u64`
- `grant_max_tokens`: `u64`
- `grant_max_duration_ms`: `u64`
- `grant_admitted_requests`: `Set<String>`
- `grant_active_requests`: `Set<String>`
- `request_ids`: `Set<String>`
- `request_phases`: `Map<String, LiveRequestPhase>`
- `request_sources`: `Map<String, String>`
- `source_requests`: `Map<String, String>`
- `source_refusals`: `Map<String, LiveSourceRefusal>`
- `source_refusal_payloads`: `Map<String, String>`
- `request_payloads`: `Map<String, String>`
- `request_evidence`: `Map<String, LiveRequestEvidenceKind>`
- `request_grants`: `Map<String, String>`
- `request_grant_records`: `Map<String, String>`
- `request_generations`: `Map<String, u64>`
- `request_executors`: `Map<String, String>`
- `request_inputs`: `Map<String, String>`
- `request_admission_commits`: `Map<String, String>`
- `request_ingress_generations`: `Map<String, u64>`
- `admitted_requests`: `Set<String>`
- `request_completion_obligations`: `Set<String>`
- `request_credit_records`: `Map<String, u64>`
- `request_credit_bytes`: `Map<String, u64>`
- `request_credit_snapshot_ceiling`: `Map<String, u64>`
- `request_credit_spent_records`: `Map<String, u64>`
- `request_credit_spent_bytes`: `Map<String, u64>`
- `request_terminal_sequences`: `Map<String, u64>`
- `request_terminal_digests`: `Map<String, String>`
- `request_ordinary_completion_digests`: `Map<String, String>`
- `request_runs`: `Map<String, String>`
- `run_requests`: `Map<String, String>`
- `run_inputs`: `Map<String, String>`
- `run_admission_commits`: `Map<String, String>`
- `run_scopes`: `Map<String, String>`
- `scope_runs`: `Map<String, String>`
- `bound_requests`: `Set<String>`
- `run_scope_records`: `Map<String, String>`
- `run_callback_records`: `Map<String, String>`
- `run_callback_receipts`: `Map<String, String>`
- `run_callback_claims`: `Map<String, Set<String>>`
- `run_callback_sequences`: `Map<String, u64>`
- `run_callback_digests`: `Map<String, String>`
- `run_predecessors`: `Map<String, String>`
- `run_ordinals`: `Map<String, u64>`
- `run_successors`: `Map<String, String>`
- `run_continuation_inputs`: `Map<String, String>`
- `run_continuation_admission_commits`: `Map<String, String>`
- `run_continuation_result_digests`: `Map<String, String>`
- `run_continuation_stage_credits`: `Map<String, u64>`
- `run_callback_application_claimed`: `Map<String, Bool>`
- `request_parents`: `Map<String, String>`
- `cancelled_requests`: `Set<String>`
- `remaining_effects`: `Map<String, u64>`
- `request_known_tokens`: `Map<String, u64>`
- `claim_ids`: `Set<String>`
- `claim_requests`: `Map<String, String>`
- `claim_runs`: `Map<String, String>`
- `claim_targets`: `Map<String, String>`
- `claim_records`: `Map<String, String>`
- `claim_effects`: `Map<String, String>`
- `claim_kinds`: `Map<String, ScopedEffectKind>`
- `claim_chains`: `Map<String, String>`
- `claim_attempts`: `Map<String, u64>`
- `claim_retry_eligible`: `Map<String, Bool>`
- `chain_latest_claims`: `Map<String, String>`
- `claim_known_tokens`: `Map<String, u64>`
- `claim_accounting_status`: `Map<String, ScopedTokenAccountingStatus>`
- `claim_accounting_records`: `Map<String, String>`
- `claim_policy_revisions`: `Map<String, String>`
- `claim_phases`: `Map<String, LiveEffectPhase>`
- `spent_effects`: `Set<String>`
- `completion_credit_schema`: `LiveCompletionCreditSchema`
- `claim_credit_records`: `Map<String, u64>`
- `claim_credit_bytes`: `Map<String, u64>`
- `claim_credit_minimum_record_charge`: `Map<String, u64>`
- `claim_credit_maximum_record_charge`: `Map<String, u64>`
- `claim_credit_snapshot_ceiling`: `Map<String, u64>`
- `claim_credit_spent_records`: `Map<String, u64>`
- `claim_credit_spent_bytes`: `Map<String, u64>`
- `claim_terminal_sequences`: `Map<String, u64>`
- `claim_terminal_digests`: `Map<String, String>`
- `source_cancellations`: `Map<String, LiveRequestCancellationReason>`

## Inputs
- `Activate`(grant_id: String, generation: u64, expires_at: u64, executor: String, record: String, profile_revision: String, evidence: Set<LiveRequestEvidenceKind>, mutations: Set<ToolMutationClass>, tools_restricted: Bool, tools: Set<String>, max_requests: u64, max_concurrent_requests: u64, max_effects: u64, max_tokens: u64, max_duration_ms: u64, now: u64)
- `Reserve`(request_id: String, source: String, payload: String, evidence: LiveRequestEvidenceKind, profile_revision: String, parent_scope: String, grant_id: String, generation: u64, executor: String, now: u64, credit_records: u64, credit_bytes: u64, snapshot_ceiling: u64, content_complete: Bool, content_discontinuous: Bool, content_empty: Bool, content_fits: Bool)
- `Admit`(request_id: String, source: String, payload: String, input_id: String, admission_commit: String, profile_revision: String, ingress_generation: u64, source_ingress_open: Bool, credit_records: u64, credit_bytes: u64, snapshot_ceiling: u64, now: u64)
- `ObserveAdmission`(request_id: String, source: String, payload: String, input_id: String, admission_commit: String, grant_id: String, generation: u64, executor: String, ingress_generation: u64)
- `Stage`(request_id: String, input_id: String, admission_commit: String, run_id: String, scope_id: String, scope_record: String, executor: String, profile_revision: String, now: u64)
- `RestoreScope`(request_id: String, input_id: String, admission_commit: String, run_id: String, scope_id: String, scope_record: String, parent_scope: String, executor: String, profile_revision: String, now: u64)
- `AdmitCallbackContinuation`(request_id: String, run_id: String, callback_record: String, result_digest: String, input_id: String, admission_commit: String, executor: String, profile_revision: String, stage_credit_bytes: u64, now: u64)
- `ObserveCallbackContinuation`(request_id: String, run_id: String, callback_record: String, result_digest: String, input_id: String, admission_commit: String)
- `StageCallbackContinuation`(request_id: String, previous_run_id: String, input_id: String, admission_commit: String, result_digest: String, run_id: String, scope_id: String, scope_record: String, executor: String, profile_revision: String, now: u64)
- `ClaimCallbackApplication`(request_id: String, run_id: String, input_id: String, admission_commit: String, scope_id: String, scope_record: String, callback_record: String, result_digest: String, executor: String, profile_revision: String, now: u64)
- `ClaimEffect`(request_id: String, input_id: String, admission_commit: String, run_id: String, scope_id: String, scope_record: String, parent_scope: String, executor: String, claim_id: String, claim_record: String, effect_id: String, chain_id: String, attempt: u64, target: String, kind: ScopedEffectKind, tool: String, mutation: ToolMutationClass, profile_revision: String, policy_revision: String, policy_permits: Bool, credit_schema: LiveCompletionCreditSchema, credit_records: u64, credit_bytes: u64, minimum_record_charge: u64, maximum_record_charge: u64, snapshot_ceiling: u64, available_records: u64, available_bytes: u64, now: u64)
- `SettleEffect`(claim_id: String, request_id: String, run_id: String, target: String, outcome: LiveEffectPhase, completion_records: u64, completion_bytes: u64, completion_sequence: u64, completion_digest: String, local_noninvocation_proven: Bool, token_accounting_status: ScopedTokenAccountingStatus, token_accounting_record: String, observed_tokens: u64)
- `Cancel`(request_id: String)
- `Revoke`(grant_id: String, generation: u64)
- `FenceExecutor`(executor: String)
- `CloseIngress`
- `Suspend`(request_id: String, run_id: String, callback_record: String, ordinary_completion_digest: String, callback_claims: Set<String>, spent_records: Map<String, u64>, spent_bytes: Map<String, u64>, completion_sequence: u64, completion_digest: String)
- `ObserveCallbackSuspension`(request_id: String, run_id: String, callback_record: String, ordinary_completion_digest: String, callback_claims: Set<String>, completion_digest: String)
- `Complete`(request_id: String, run_id: String, input_id: String, ordinary_completion_digest: String, completion_records: u64, completion_bytes: u64, completion_sequence: u64, completion_digest: String)
- `ObserveRequestCompletion`(request_id: String, run_id: String, input_id: String, ordinary_completion_digest: String)
- `ObserveEffectSettlement`(claim_id: String, request_id: String, run_id: String, target: String, outcome: LiveEffectPhase, completion_digest: String)
- `ResolveModelAttempt`(request_id: String, run_id: String, scope_id: String, chain_id: String, now: u64)
- `ResolveInputRecovery`(request_id: String, input_id: String, run_id: String, source: String, observed_phase: LiveRecoveryInputPhase, boundary_committed: Bool, application_evidence: LiveRecoveryApplicationEvidence, purpose: LiveInputRecoveryPurpose, runtime_run_current: Bool)
- `CompleteRunless`(request_id: String, input_id: String, ordinary_completion_digest: String, completion_records: u64, completion_bytes: u64, completion_sequence: u64, completion_digest: String)
- `ObserveRunlessCompletion`(request_id: String, input_id: String, ordinary_completion_digest: String)
- `CancelSource`(source: String, reason: LiveRequestCancellationReason)
- `CompleteUnstagedContinuation`(request_id: String, previous_run_id: String, input_id: String, ordinary_completion_digest: String, completion_records: u64, completion_bytes: u64, completion_sequence: u64, completion_digest: String)
- `ObserveUnstagedContinuationCompletion`(request_id: String, previous_run_id: String, input_id: String, ordinary_completion_digest: String)
- `ObserveCancelledCallback`(request_id: String, run_id: String, callback_record: String, ordinary_completion_digest: String, callback_claims: Set<String>, completion_digest: String, reason: LiveRequestCancellationReason)

## Signals

## Effects
- `ActivationChanged`(grant_id: String, generation: u64, record: String)
- `SourceReserved`(request_id: String, source: String, payload: String)
- `SourceRefused`(source: String, payload: String, reason: LiveSourceRefusal)
- `InputAdmitted`(request_id: String, input_id: String, admission_commit: String)
- `AdmissionObserved`(request_id: String, input_id: String, admission_commit: String)
- `CallbackContinuationAdmitted`(request_id: String, run_id: String, input_id: String, admission_commit: String, result_digest: String)
- `CallbackContinuationObserved`(request_id: String, run_id: String, input_id: String, admission_commit: String, result_digest: String)
- `CallbackApplicationClaimed`(request_id: String, run_id: String, scope_id: String, callback_record: String, result_digest: String)
- `RunScopeBound`(request_id: String, run_id: String, scope_id: String)
- `ScopeRestored`(request_id: String, run_id: String, scope_id: String)
- `ModelAttemptResolved`(chain_id: String, attempt: u64)
- `ModelTokenBudgetExhausted`(used: u64, limit: u64)
- `RequestCompletionObserved`(request_id: String, completion_sequence: u64, completion_digest: String)
- `EffectStartClaimed`(claim_id: String, request_id: String, run_id: String, target: String)
- `EffectSettled`(claim_id: String, outcome: LiveEffectPhase)
- `RequestCancellationRequired`(request_id: String)
- `GrantRevoked`(grant_id: String, generation: u64)
- `ExecutorFenced`(executor: String)
- `IngressClosed`
- `RequestSuspended`(request_id: String, run_id: String)
- `RequestCompleted`(request_id: String, run_id: String)
- `EffectSettlementObserved`(claim_id: String, outcome: LiveEffectPhase, completion_sequence: u64, completion_digest: String)
- `InputRecoveryResolved`(request_id: String, input_id: String, run_id: String, disposition: LiveInputRecoveryDisposition)
- `RunlessRequestCompleted`(request_id: String, input_id: String)
- `SourceCancellationRetained`(source: String, reason: LiveRequestCancellationReason)
- `CancelledCallbackHeld`(request_id: String, run_id: String, reason: LiveRequestCancellationReason)

## Invariants
- `refused_sources_never_mint_requests`
- `request_completion_obligations_have_exact_lifetimes`
- `request_completion_credits_are_complete`
- `request_record_fields_have_one_owner`
- `activated_grant_is_complete_and_bounded`
- `active_grant_requests_have_exact_admission`
- `current_requests_retain_the_activated_ceiling`
- `admission_and_run_scope_fields_are_complete`
- `request_identity_joins_are_exact`
- `callback_continuations_form_one_exact_acyclic_run_chain`
- `callback_continuation_admission_is_complete_and_one_shot`
- `callback_application_claims_require_continuation_lineage`
- `claims_retain_exact_identity_and_spent_effects`
- `observed_token_accounting_is_complete`
- `effect_attempt_chains_are_exact`
- `completion_credits_are_bounded_and_settlement_is_exact`
- `claim_identity_joins_are_exact`
- `callback_suspensions_retain_exact_run_membership`
- `open_ingress_requires_a_live_grant`

## Transitions
### `ResolveUnboundInputRecovery`
- From: `Ready`
- On: `ResolveInputRecovery`(request_id, input_id, run_id, source, observed_phase, boundary_committed, application_evidence, purpose, runtime_run_current)
- Guards:
  - ``
- Emits: `InputRecoveryResolved`
- To: `Ready`

### `ResolveBoundInputRecovery`
- From: `Ready`
- On: `ResolveInputRecovery`(request_id, input_id, run_id, source, observed_phase, boundary_committed, application_evidence, purpose, runtime_run_current)
- Guards:
  - ``
- Emits: `InputRecoveryResolved`
- To: `Ready`

### `ActivateFreshGrant`
- From: `Ready`
- On: `Activate`(grant_id, generation, expires_at, executor, record, profile_revision, evidence, mutations, tools_restricted, tools, max_requests, max_concurrent_requests, max_effects, max_tokens, max_duration_ms, now)
- Guards:
  - ``
- Emits: `ActivationChanged`
- To: `Ready`

### `ReserveNewSource`
- From: `Ready`
- On: `Reserve`(request_id, source, payload, evidence, profile_revision, parent_scope, grant_id, generation, executor, now, credit_records, credit_bytes, snapshot_ceiling, content_complete, content_discontinuous, content_empty, content_fits)
- Guards:
  - ``
- Emits: `SourceReserved`
- To: `Ready`

### `RefuseNewSource`
- From: `Ready`
- On: `Reserve`(request_id, source, payload, evidence, profile_revision, parent_scope, grant_id, generation, executor, now, credit_records, credit_bytes, snapshot_ceiling, content_complete, content_discontinuous, content_empty, content_fits)
- Guards:
  - ``
- Emits: `SourceRefused`
- To: `Ready`

### `AdmitReservedInput`
- From: `Ready`
- On: `Admit`(request_id, source, payload, input_id, admission_commit, profile_revision, ingress_generation, source_ingress_open, credit_records, credit_bytes, snapshot_ceiling, now)
- Guards:
  - ``
- Emits: `InputAdmitted`
- To: `Ready`

### `ObserveCommittedAdmission`
- From: `Ready`
- On: `ObserveAdmission`(request_id, source, payload, input_id, admission_commit, grant_id, generation, executor, ingress_generation)
- Guards:
  - ``
- Emits: `AdmissionObserved`
- To: `Ready`

### `StageAdmittedRun`
- From: `Ready`
- On: `Stage`(request_id, input_id, admission_commit, run_id, scope_id, scope_record, executor, profile_revision, now)
- Guards:
  - ``
- Emits: `RunScopeBound`
- To: `Ready`

### `AdmitExactCallbackContinuation`
- From: `Ready`
- On: `AdmitCallbackContinuation`(request_id, run_id, callback_record, result_digest, input_id, admission_commit, executor, profile_revision, stage_credit_bytes, now)
- Guards:
  - ``
- Emits: `CallbackContinuationAdmitted`
- To: `Ready`

### `ObserveExactCallbackContinuation`
- From: `Ready`
- On: `ObserveCallbackContinuation`(request_id, run_id, callback_record, result_digest, input_id, admission_commit)
- Guards:
  - ``
- Emits: `CallbackContinuationObserved`
- To: `Ready`

### `StageExactCallbackContinuation`
- From: `Ready`
- On: `StageCallbackContinuation`(request_id, previous_run_id, input_id, admission_commit, result_digest, run_id, scope_id, scope_record, executor, profile_revision, now)
- Guards:
  - ``
- Emits: `RunScopeBound`
- To: `Ready`

### `ClaimExactCallbackApplication`
- From: `Ready`
- On: `ClaimCallbackApplication`(request_id, run_id, input_id, admission_commit, scope_id, scope_record, callback_record, result_digest, executor, profile_revision, now)
- Guards:
  - ``
- Emits: `CallbackApplicationClaimed`
- To: `Ready`

### `RestoreExactRunningScope`
- From: `Ready`
- On: `RestoreScope`(request_id, input_id, admission_commit, run_id, scope_id, scope_record, parent_scope, executor, profile_revision, now)
- Guards:
  - ``
- Emits: `ScopeRestored`
- To: `Ready`

### `ClaimExactCurrentEffect`
- From: `Ready`
- On: `ClaimEffect`(request_id, input_id, admission_commit, run_id, scope_id, scope_record, parent_scope, executor, claim_id, claim_record, effect_id, chain_id, attempt, target, kind, tool, mutation, profile_revision, policy_revision, policy_permits, credit_schema, credit_records, credit_bytes, minimum_record_charge, maximum_record_charge, snapshot_ceiling, available_records, available_bytes, now)
- Guards:
  - ``
- Emits: `EffectStartClaimed`
- To: `Ready`

### `SettleClaimedEffect`
- From: `Ready`
- On: `SettleEffect`(claim_id, request_id, run_id, target, outcome, completion_records, completion_bytes, completion_sequence, completion_digest, local_noninvocation_proven, token_accounting_status, token_accounting_record, observed_tokens)
- Guards:
  - ``
- Emits: `EffectSettled`
- To: `Ready`

### `ResolveFreshModelAttempt`
- From: `Ready`
- On: `ResolveModelAttempt`(request_id, run_id, scope_id, chain_id, now)
- Guards:
  - ``
- Emits: `ModelAttemptResolved`
- To: `Ready`

### `ResolveConclusiveModelSuccessor`
- From: `Ready`
- On: `ResolveModelAttempt`(request_id, run_id, scope_id, chain_id, now)
- Guards:
  - ``
- Emits: `ModelAttemptResolved`
- To: `Ready`

### `ResolveExhaustedModelTokenBudget`
- From: `Ready`
- On: `ResolveModelAttempt`(request_id, run_id, scope_id, chain_id, now)
- Guards:
  - ``
- Emits: `ModelTokenBudgetExhausted`
- To: `Ready`

### `ObserveExactEffectSettlement`
- From: `Ready`
- On: `ObserveEffectSettlement`(claim_id, request_id, run_id, target, outcome, completion_digest)
- Guards:
  - ``
- Emits: `EffectSettlementObserved`
- To: `Ready`

### `CancelKnownRequest`
- From: `Ready`
- On: `Cancel`(request_id)
- Guards:
  - ``
- Emits: `RequestCancellationRequired`
- To: `Ready`

### `CancelUnadmittedRequest`
- From: `Ready`
- On: `Cancel`(request_id)
- Guards:
  - ``
- Emits: `RequestCancellationRequired`
- To: `Ready`

### `CancelUnreservedSource`
- From: `Ready`
- On: `CancelSource`(source, reason)
- Guards:
  - ``
- Emits: `SourceCancellationRetained`
- To: `Ready`

### `CancelReservedSource`
- From: `Ready`
- On: `CancelSource`(source, reason)
- Guards:
  - ``
- Emits: `SourceCancellationRetained`, `RequestCancellationRequired`
- To: `Ready`

### `CancelUnadmittedSource`
- From: `Ready`
- On: `CancelSource`(source, reason)
- Guards:
  - ``
- Emits: `SourceCancellationRetained`, `RequestCancellationRequired`
- To: `Ready`

### `CancelCompletedSource`
- From: `Ready`
- On: `CancelSource`(source, reason)
- Guards:
  - ``
- Emits: `SourceCancellationRetained`
- To: `Ready`

### `ObserveSourceCancellation`
- From: `Ready`
- On: `CancelSource`(source, reason)
- Guards:
  - ``
- Emits: `SourceCancellationRetained`
- To: `Ready`

### `ObservePendingSourceCancellation`
- From: `Ready`
- On: `CancelSource`(source, reason)
- Guards:
  - ``
- Emits: `SourceCancellationRetained`, `RequestCancellationRequired`
- To: `Ready`

### `RevokeCurrentGrant`
- From: `Ready`
- On: `Revoke`(grant_id, generation)
- Guards:
  - ``
- Emits: `GrantRevoked`
- To: `Ready`

### `FenceExecutorBinding`
- From: `Ready`
- On: `FenceExecutor`(executor)
- Guards:
  - ``
- Emits: `ExecutorFenced`
- To: `Ready`

### `CloseRequestIngress`
- From: `Ready`
- On: `CloseIngress`()
- Guards:
  - ``
- Emits: `IngressClosed`
- To: `Ready`

### `ObserveClosedRequestIngress`
- From: `Ready`
- On: `CloseIngress`()
- Guards:
  - ``
- Emits: `IngressClosed`
- To: `Ready`

### `SuspendRunningRequest`
- From: `Ready`
- On: `Suspend`(request_id, run_id, callback_record, ordinary_completion_digest, callback_claims, spent_records, spent_bytes, completion_sequence, completion_digest)
- Guards:
  - ``
- Emits: `RequestSuspended`
- To: `Ready`

### `ObserveExactCallbackSuspension`
- From: `Ready`
- On: `ObserveCallbackSuspension`(request_id, run_id, callback_record, ordinary_completion_digest, callback_claims, completion_digest)
- Guards:
  - ``
- Emits: `RequestSuspended`
- To: `Ready`

### `ObserveExactCancelledCallbackHold`
- From: `Ready`
- On: `ObserveCancelledCallback`(request_id, run_id, callback_record, ordinary_completion_digest, callback_claims, completion_digest, reason)
- Guards:
  - ``
- Emits: `CancelledCallbackHeld`
- To: `Ready`

### `CompleteRunningRequest`
- From: `Ready`
- On: `Complete`(request_id, run_id, input_id, ordinary_completion_digest, completion_records, completion_bytes, completion_sequence, completion_digest)
- Guards:
  - ``
- Emits: `RequestCompleted`
- To: `Ready`

### `ObserveCompletedRequest`
- From: `Ready`
- On: `ObserveRequestCompletion`(request_id, run_id, input_id, ordinary_completion_digest)
- Guards:
  - ``
- Emits: `RequestCompletionObserved`
- To: `Ready`

### `CompleteRunlessRequest`
- From: `Ready`
- On: `CompleteRunless`(request_id, input_id, ordinary_completion_digest, completion_records, completion_bytes, completion_sequence, completion_digest)
- Guards:
  - ``
- Emits: `RunlessRequestCompleted`
- To: `Ready`

### `ObserveCompletedRunlessRequest`
- From: `Ready`
- On: `ObserveRunlessCompletion`(request_id, input_id, ordinary_completion_digest)
- Guards:
  - ``
- Emits: `RequestCompletionObserved`
- To: `Ready`

### `CompleteUnstagedCallbackContinuation`
- From: `Ready`
- On: `CompleteUnstagedContinuation`(request_id, previous_run_id, input_id, ordinary_completion_digest, completion_records, completion_bytes, completion_sequence, completion_digest)
- Guards:
  - ``
- Emits: `RunlessRequestCompleted`
- To: `Ready`

### `ObserveCompletedUnstagedCallbackContinuation`
- From: `Ready`
- On: `ObserveUnstagedContinuationCompletion`(request_id, previous_run_id, input_id, ordinary_completion_digest)
- Guards:
  - ``
- Emits: `RequestCompletionObserved`
- To: `Ready`

## Coverage
### Code Anchors
- `live_request_catalog_bridge` (machine `LiveRequestMachine`): `meerkat-runtime/src/live_ledger/authority/dsl.rs` — catalog-derived runtime transition body; committed storage and physical effect realization are not claimed by this anchor

### Scenarios
- `generated_scope_restore_preserves_admission_won_close_and_exact_bindings` — production DSL rejects mismatched request/input/run/scope/digest/lineage/executor after recovery and permits an admitted run despite closed ingress
- `generated_claim_rechecks_revocation_after_policy_await` — an actual awaited candidate policy result cannot restore a revoked generated claim
- `claimed_effect_survives_revoke_and_recovery_without_resend_permission` — recovery retains the spent effect and accepts exact unknown feedback once after revoke
