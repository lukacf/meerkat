use super::*;
use meerkat_core::execution_scope::{
    ExecutionAdmissionCommitRef, ScopedEffectClaimId, ScopedEffectClaimRecord,
    ScopedEffectPolicyRevision, ScopedEffectStartPermit, ScopedEffectTarget, ScopedRunAuthority,
};
use meerkat_core::ops::OperationId;
use meerkat_core::{ToolExecutionPolicy, ToolMutationClass};
use sha2::{Digest, Sha256};
use std::num::NonZeroU64;

pub(super) fn claim_record_digest(
    claim: &ScopedEffectClaimRecord<ScopedEffectTarget>,
) -> Result<[u8; 32], serde_json::Error> {
    let mut digest = Sha256::new();
    digest.update(b"meerkat.live-effect-claim.v1\0");
    digest.update(serde_json::to_vec(&(
        &claim.claim_id,
        &claim.scope_id,
        &claim.request_id,
        &claim.effect_id,
        &claim.target,
        &claim.executor,
        &claim.input_id,
        &claim.run_id,
        &claim.grant,
        &claim.candidate_policy_revision,
        claim.commit.revision,
    ))?);
    Ok(digest.finalize().into())
}

pub(super) fn read_retained_claim(
    state: &dsl::LiveRequestMachineState,
    head_revision: u64,
    key: &str,
) -> Result<ScopedEffectClaimRecord<ScopedEffectTarget>, LiveRequestAuthorityError> {
    let invalid = LiveRequestAuthorityError::InvalidEffectFeedback;
    let claim: ScopedEffectClaimRecord<ScopedEffectTarget> = serde_json::from_str(
        state
            .claim_records
            .get(key)
            .ok_or_else(|| invalid("missing exact retained claim"))?,
    )?;
    let request = claim.request_id.to_string();
    let run = claim.run_id.to_string();
    let scope: meerkat_core::execution_scope::RunEffectScopeRecord = serde_json::from_str(
        state
            .run_scope_records
            .get(&run)
            .ok_or_else(|| invalid("missing retained scope"))?,
    )?;
    claim
        .validate_scope_binding(claim.scope_id, &scope, &claim.target)
        .map_err(|_| invalid("claim does not bind retained scope"))?;
    if claim.claim_id.as_uuid().to_string() != key
        || claim.commit.digest != claim_record_digest(&claim)?
        || claim.commit.revision.get() > head_revision
        || state.run_requests.get(&run) != Some(&request)
        || state.run_inputs.get(&run) != Some(&claim.input_id.to_string())
        || state.run_admission_commits.get(&run)
            != Some(&serde_json::to_string(&scope.admission_commit)?)
        || state.run_scopes.get(&run) != Some(&claim.scope_id.as_uuid().to_string())
        || state.claim_requests.get(key) != Some(&request)
        || state.claim_runs.get(key) != Some(&run)
        || state.claim_effects.get(key) != Some(&claim.effect_id.to_string())
        || state.claim_targets.get(key) != Some(&serde_json::to_string(&claim.target)?)
        || state.claim_kinds.get(key) != Some(&claim.target.kind())
        || state.claim_policy_revisions.get(key)
            != Some(&serde_json::to_string(&claim.candidate_policy_revision)?)
    {
        return Err(invalid(
            "record does not identify the exact committed claim",
        ));
    }
    Ok(claim)
}

/// Trusted-host ordinary-policy observation. This is not an effect permit.
/// The fence must hold this exact evaluated policy current through publication.
/// No default revision, permissive default fence, or deserialization is provided.
pub struct LiveEffectPolicyObservation {
    policy: OrdinaryEffectPolicy,
    revision: ScopedEffectPolicyRevision,
    currentness: Arc<dyn RuntimeStoreWriteFence>,
    source: LiveEffectPolicySource,
}

enum OrdinaryEffectPolicy {
    Tools(ToolExecutionPolicy),
    ModelRequest,
}

enum LiveEffectPolicySource {
    TrustedHost,
    Evaluated {
        run_id: meerkat_core::RunId,
        target: ScopedEffectTarget,
    },
}

impl LiveEffectPolicyObservation {
    pub fn from_dispatch_evaluation(
        evaluation: meerkat_core::EvaluatedToolExecutionPolicy,
    ) -> Result<Self, RuntimeStoreError> {
        let target = evaluation
            .target()
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let revision = evaluation
            .revision()
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        Ok(Self {
            policy: OrdinaryEffectPolicy::Tools(evaluation.policy().clone()),
            revision,
            source: LiveEffectPolicySource::Evaluated {
                run_id: evaluation.run_id().clone(),
                target,
            },
            currentness: Arc::new(DispatcherPolicyFence(evaluation)),
        })
    }

    pub fn from_model_evaluation(
        evaluation: meerkat_core::execution_scope::EvaluatedModelRequestPolicy,
    ) -> Self {
        Self {
            policy: OrdinaryEffectPolicy::ModelRequest,
            revision: evaluation.revision(),
            source: LiveEffectPolicySource::Evaluated {
                run_id: evaluation.run_id().clone(),
                target: evaluation.target().clone(),
            },
            currentness: Arc::new(ModelRequestPolicyFence(evaluation)),
        }
    }

    pub fn new(
        policy: ToolExecutionPolicy,
        revision: NonZeroU64,
        currentness: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<Self, RuntimeStoreError> {
        let revision = ScopedEffectPolicyRevision::TrustedHost {
            ordinary_policy: policy
                .content_digest()
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?,
            revision,
        };
        Ok(Self {
            policy: OrdinaryEffectPolicy::Tools(policy),
            revision,
            currentness,
            source: LiveEffectPolicySource::TrustedHost,
        })
    }

    /// Bind an actual managed-policy evaluation to the host's resolved physical
    /// target. The host must supply the same call used to resolve that target;
    /// publication uses the retained policy owner's lock, not a revision reread.
    pub fn from_evaluated(
        policy: ToolExecutionPolicy,
        evaluation: meerkat_core::AllowedToolConsequenceEvaluation,
        call: meerkat_core::ToolCallView<'_>,
        target: ScopedEffectTarget,
    ) -> Result<Self, RuntimeStoreError> {
        let request = evaluation.request();
        if request.tool_call_id != call.id
            || request.tool_name.as_str() != call.name
            || request.arguments_json != call.args.get()
            || !matches!(&target, ScopedEffectTarget::ToolDispatch { call_id, tool, .. }
                if call_id == &request.tool_call_id && tool == &request.tool_name)
        {
            return Err(RuntimeStoreError::WriteFailed(
                "evaluated policy does not bind the resolved tool call".into(),
            ));
        }
        let run_id = request.run_id.clone().ok_or_else(|| {
            RuntimeStoreError::WriteFailed("evaluated policy has no scoped run identity".into())
        })?;
        let revision = NonZeroU64::new(evaluation.provenance().revision.0).ok_or_else(|| {
            RuntimeStoreError::WriteFailed("evaluated policy has a zero revision".into())
        })?;
        let revision = ScopedEffectPolicyRevision::Managed {
            ordinary_policy: policy
                .content_digest()
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?,
            provider_id: request.provider_id.clone(),
            policy_id: request.policy_id.clone(),
            generation: evaluation.generation(),
            revision,
            digest: evaluation.provenance().digest.clone(),
        };
        Ok(Self {
            policy: OrdinaryEffectPolicy::Tools(policy),
            revision,
            currentness: Arc::new(EvaluatedPolicyFence(evaluation)),
            source: LiveEffectPolicySource::Evaluated { run_id, target },
        })
    }
}

struct EvaluatedPolicyFence(meerkat_core::AllowedToolConsequenceEvaluation);

struct DispatcherPolicyFence(meerkat_core::EvaluatedToolExecutionPolicy);

struct ModelRequestPolicyFence(meerkat_core::execution_scope::EvaluatedModelRequestPolicy);

impl RuntimeStoreWriteFence for ModelRequestPolicyFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        self.0.publish_immutable(operation)?;
        Ok(RuntimeStoreWriteFenceOutcome::Applied)
    }
}

impl RuntimeStoreWriteFence for DispatcherPolicyFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        policy_publication_outcome(self.0.publish_if_current(operation))
    }
}

impl RuntimeStoreWriteFence for EvaluatedPolicyFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        policy_publication_outcome(self.0.publish_if_current(operation))
    }
}

fn policy_publication_outcome(
    outcome: Result<Result<(), RuntimeStoreError>, meerkat_core::PolicyPublicationError>,
) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
    match outcome {
        Ok(result) => {
            result?;
            Ok(RuntimeStoreWriteFenceOutcome::Applied)
        }
        Err(meerkat_core::PolicyPublicationError::NotCurrent) => {
            Ok(RuntimeStoreWriteFenceOutcome::Conflict {
                reason: "evaluated policy changed before Live effect publication".into(),
            })
        }
        Err(error) => Err(RuntimeStoreError::WriteFailed(error.to_string())),
    }
}

pub(in crate::live_ledger) struct PreparedLiveEffectClaim {
    scope: ScopedRunAuthority,
    claim: ScopedEffectClaimRecord<ScopedEffectTarget>,
    commit: PreparedLiveLedgerCommit,
    fence: CurrentLiveRequestFence,
}

impl LiveRequestStoreOwner {
    pub(crate) async fn claim_effect(
        &self,
        scope: ScopedRunAuthority,
        effect_id: OperationId,
        target: ScopedEffectTarget,
        policy: LiveEffectPolicyObservation,
    ) -> Result<ScopedEffectStartPermit<ScopedEffectTarget>, LiveRequestAuthorityError> {
        let mut attempts_remaining = 8;
        loop {
            let prepared = self
                .prepare_effect_claim_candidate(
                    scope.clone(),
                    effect_id.clone(),
                    target.clone(),
                    &policy,
                )
                .await?;
            match self.commit_effect_claim(prepared).await {
                Err(LiveRequestAuthorityError::NotNewlyCommitted(outcome))
                    if attempts_remaining > 1
                        && matches!(*outcome, LiveLedgerCommitOutcome::Conflict { .. }) =>
                {
                    attempts_remaining -= 1;
                    tracing::debug!(
                        session_id = %self.session_id,
                        effect_id = %effect_id,
                        attempts_remaining,
                        "repreparing uncommitted Live effect claim after head conflict"
                    );
                    tokio::task::yield_now().await;
                }
                result => return result,
            }
        }
    }

    #[cfg(test)]
    pub(in crate::live_ledger) async fn prepare_effect_claim(
        &self,
        scope: ScopedRunAuthority,
        effect_id: OperationId,
        target: ScopedEffectTarget,
        policy: LiveEffectPolicyObservation,
    ) -> Result<PreparedLiveEffectClaim, LiveRequestAuthorityError> {
        self.prepare_effect_claim_candidate(scope, effect_id, target, &policy)
            .await
    }

    async fn prepare_effect_claim_candidate(
        &self,
        scope: ScopedRunAuthority,
        effect_id: OperationId,
        target: ScopedEffectTarget,
        policy: &LiveEffectPolicyObservation,
    ) -> Result<PreparedLiveEffectClaim, LiveRequestAuthorityError> {
        if let LiveEffectPolicySource::Evaluated {
            run_id,
            target: evaluated_target,
        } = &policy.source
            && (run_id != &scope.record().run_id || evaluated_target != &target)
        {
            return Err(LiveRequestAuthorityError::ScopeNotCurrent(
                "evaluated policy does not bind the exact run and physical target",
            ));
        }
        let observed = self
            .observe_run_scope(scope.scope_id(), scope.record().clone())
            .await?;
        let budget = effect_credits::EffectCompletionBudget::for_kind(target.kind())?;
        let available = crate::live_resources::LIVE_LEDGER_MAX_CHARGE
            .checked_sub(observed.head.payload.used)
            .and_then(|charge| charge.checked_sub(observed.head.payload.reserved))
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let scope_policy =
            ToolExecutionPolicy::resolve(observed.scope.policy.tool_access().clone())
                .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let (tool, mutation, policy_permits) = match &target {
            ScopedEffectTarget::ToolDispatch { tool, mutation, .. } => (
                tool.to_string(),
                *mutation,
                matches!(&policy.policy, OrdinaryEffectPolicy::Tools(ordinary)
                    if ordinary.permits_call(tool.as_ref(), *mutation))
                    && scope_policy.permits_call(tool.as_ref(), *mutation)
                    && observed.scope.policy.allowed_mutations().contains(mutation),
            ),
            ScopedEffectTarget::ModelComputation { .. } => {
                (String::new(), ToolMutationClass::Unknown, true)
            }
            ScopedEffectTarget::DescendantAdmission { .. } => (
                String::new(),
                ToolMutationClass::Mutating,
                observed
                    .scope
                    .policy
                    .allowed_mutations()
                    .contains(&ToolMutationClass::Mutating),
            ),
        };
        let claim_id = ScopedEffectClaimId::from_uuid(uuid::Uuid::new_v4());
        let (chain_id, attempt) = match &target {
            ScopedEffectTarget::ModelComputation {
                request_id,
                attempt,
                ..
            } => (
                meerkat_core::execution_scope::model_attempt_chain_id(scope.scope_id(), request_id)
                    .to_string(),
                u64::from(*attempt),
            ),
            ScopedEffectTarget::ToolDispatch { .. }
            | ScopedEffectTarget::DescendantAdmission { .. } => (effect_id.to_string(), 0),
        };
        let target_record = serde_json::to_string(&target)?;
        let request_id = observed.scope.request_id.to_string();
        let run_id = observed.scope.run_id.to_string();
        let revision = observed
            .head
            .reference
            .revision
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .ok_or(LiveRequestAuthorityError::ScopeNotCurrent(
                "claim revision overflow",
            ))?;
        let mut claim = ScopedEffectClaimRecord {
            claim_id,
            scope_id: scope.scope_id(),
            request_id: observed.scope.request_id.clone(),
            effect_id: effect_id.clone(),
            target: target.clone(),
            executor: observed.scope.executor.clone(),
            input_id: observed.scope.input_id.clone(),
            run_id: observed.scope.run_id.clone(),
            grant: observed.scope.grant.clone(),
            candidate_policy_revision: policy.revision.clone(),
            commit: ExecutionAdmissionCommitRef {
                revision,
                digest: [0; 32],
            },
        };
        claim.commit.digest = claim_record_digest(&claim)?;
        let mut command = dsl::LiveRequestInput::ClaimEffect {
            request_id: request_id.clone(),
            input_id: observed.scope.input_id.to_string(),
            admission_commit: serde_json::to_string(&observed.scope.admission_commit)?,
            run_id: run_id.clone(),
            scope_id: observed.scope_id.as_uuid().to_string(),
            scope_record: serde_json::to_string(&observed.scope)?,
            parent_scope: String::new(),
            executor: serde_json::to_string(&observed.scope.executor)?,
            claim_id: claim_id.as_uuid().to_string(),
            claim_record: serde_json::to_string(&claim)?,
            effect_id: effect_id.to_string(),
            chain_id,
            attempt,
            target: target_record.clone(),
            kind: target.kind(),
            tool,
            mutation,
            profile_revision: observed.profile_revision,
            policy_revision: serde_json::to_string(&policy.revision)?,
            policy_permits,
            credit_schema: crate::live_ledger::completion_budget::CompletionCreditSchema::V1,
            credit_records: budget.envelope.total().records,
            credit_bytes: budget.envelope.total().encoded_bytes,
            minimum_record_charge: budget.minimum_record_charge,
            maximum_record_charge: budget.maximum_record_charge,
            snapshot_ceiling: budget.snapshot_ceiling,
            available_records: available.records,
            available_bytes: available.encoded_bytes,
            now: 0,
        };
        refresh_command_time(&mut command, self.clock.as_ref())?;
        let mut candidate = observed.owner.prepare_authority();
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, command.clone())?;
        if !matches!(transition.effects(), [dsl::LiveRequestEffect::EffectStartClaimed {
            claim_id: claimed, request_id: request, run_id: run, target: claimed_target,
        }] if claimed == &claim_id.as_uuid().to_string()
            && request == &request_id && run == &run_id && claimed_target == &target_record)
        {
            return Err(LiveRequestAuthorityError::ScopeNotCurrent(
                "generated effect claim did not bind its exact invocation",
            ));
        }
        let commit = PreparedLiveLedgerCommit::from_request_transition(
            &self.session_id,
            Some(&observed.head),
            &candidate,
        )?
        .with_execution_fence(&observed.input, observed.lifecycle)?;
        if commit.successor().reference.revision != revision.get() {
            return Err(LiveRequestAuthorityError::ScopeNotCurrent(
                "claim revision mismatch",
            ));
        }
        let fence = CurrentLiveRequestFence {
            time: LiveRequestTimeFence {
                predecessor: observed.owner,
                input: command,
                expected_snapshot: Arc::clone(&commit.successor().payload.request_snapshot),
                clock: Arc::clone(&self.clock),
            },
            registration: Arc::clone(&policy.currentness),
        };
        Ok(PreparedLiveEffectClaim {
            scope,
            claim,
            commit,
            fence,
        })
    }

    pub(in crate::live_ledger) async fn commit_effect_claim(
        &self,
        prepared: PreparedLiveEffectClaim,
    ) -> Result<ScopedEffectStartPermit<ScopedEffectTarget>, LiveRequestAuthorityError> {
        let ops = self
            .store
            .live_ledger_ops()
            .filter(|ops| ops.ledger_write_profile().supports_execution_fence())
            .ok_or(LiveRequestAuthorityError::Unsupported)?;
        if prepared.commit.session_id() != &self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        let expected = prepared.commit.successor().reference.clone();
        let outcome = ops
            .commit_live_ledger(prepared.commit, Arc::new(prepared.fence))
            .await?;
        if !matches!(&outcome, LiveLedgerCommitOutcome::Committed { head } if head == &expected) {
            return Err(LiveRequestAuthorityError::NotNewlyCommitted(Box::new(
                outcome,
            )));
        }
        stage::seal_committed_effect(&prepared.scope, prepared.claim)
            .map_err(|error| RuntimeStoreError::WriteFailed(error).into())
    }
}
