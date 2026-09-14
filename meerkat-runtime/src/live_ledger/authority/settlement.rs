use super::*;
use crate::live_ledger::completion::{
    LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES, LiveCompletionEvent, LiveCompletionRecord,
    LiveCompletionText, LivePhysicalEffectOutcome,
};
use crate::live_ledger::transcript::LiveLedgerFormatV1;
use meerkat_core::execution_scope::{
    ScopedEffectClaimRecord, ScopedEffectNotStartedProof, ScopedEffectTarget,
};
use meerkat_core::live_execution::request::LiveSourceKey;
use meerkat_core::live_observation::LiveObservationSeq;
use meerkat_core::ops::OperationId;

pub(crate) enum LiveEffectFeedback {
    Observed {
        claim: ScopedEffectClaimRecord<ScopedEffectTarget>,
        outcome: LivePhysicalEffectOutcome,
        token_accounting: meerkat_core::execution_scope::ScopedEffectTokenAccounting,
    },
    NotStarted(ScopedEffectNotStartedProof<ScopedEffectTarget>),
}

impl LiveEffectFeedback {
    pub(crate) fn request_id(&self) -> &OperationId {
        match self {
            Self::Observed { claim, .. } => &claim.request_id,
            Self::NotStarted(proof) => &proof.claim().request_id,
        }
    }

    pub(crate) fn session_id(&self) -> &SessionId {
        match self {
            Self::Observed { claim, .. } => &claim.executor.session_id,
            Self::NotStarted(proof) => &proof.claim().executor.session_id,
        }
    }
}

pub(in crate::live_ledger) struct PreparedLiveEffectSettlement {
    session_id: SessionId,
    preparation: SettlementPreparation,
}

enum SettlementPreparation {
    Observed(LiveObservationSeq),
    Append {
        sequence: LiveObservationSeq,
        commit: Box<PreparedLiveLedgerCommit>,
        fence: Box<LiveRequestTimeFence>,
    },
}

impl LiveRequestStoreOwner {
    pub(crate) async fn settle_effect(
        &self,
        feedback: LiveEffectFeedback,
        diagnostic: LiveCompletionText<LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES>,
    ) -> Result<LiveObservationSeq, LiveRequestAuthorityError> {
        let prepared = self.prepare_effect_settlement(feedback, diagnostic).await?;
        self.commit_effect_settlement(prepared).await
    }

    pub(in crate::live_ledger) async fn prepare_effect_settlement(
        &self,
        feedback: LiveEffectFeedback,
        diagnostic: LiveCompletionText<LIVE_TERMINAL_DIAGNOSTIC_MAX_BYTES>,
    ) -> Result<PreparedLiveEffectSettlement, LiveRequestAuthorityError> {
        let invalid = LiveRequestAuthorityError::InvalidEffectFeedback;
        if feedback.session_id() != &self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        let (claim, outcome, token_accounting, local_noninvocation_proven) = match feedback {
            LiveEffectFeedback::Observed {
                claim,
                outcome,
                token_accounting,
            } => (claim, outcome, token_accounting, false),
            LiveEffectFeedback::NotStarted(proof) => (
                proof.into_claim(),
                LivePhysicalEffectOutcome::NotStarted,
                meerkat_core::execution_scope::ScopedEffectTokenAccounting::NotApplicable {},
                true,
            ),
        };
        let ops = self
            .store
            .live_ledger_ops()
            .ok_or(LiveRequestAuthorityError::Unsupported)?;
        let head = ops
            .load_live_head(&self.session_id)
            .await?
            .ok_or_else(|| invalid("missing Live request owner"))?;
        if head.reference.session_id != self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        head.validate_payload()?;
        let owner = dsl::LiveRequestMachineAuthority::recover_from_state(
            crate::generated::live_request_state::decode(&head.payload.request_snapshot)?,
        )?;
        let state = owner.state();
        let key = claim.claim_id.as_uuid().to_string();
        let request = claim.request_id.to_string();
        let retained = claim::read_retained_claim(state, head.reference.revision, &key)?;
        if retained != claim {
            return Err(invalid(
                "feedback does not identify the exact committed claim",
            ));
        }
        let source: LiveSourceKey = serde_json::from_str(
            state
                .request_sources
                .get(&request)
                .ok_or_else(|| invalid("missing retained source"))?,
        )?;
        if source.session_id() != &self.session_id {
            return Err(invalid("retained source belongs to another session"));
        }
        let next = head
            .reference
            .event_count
            .checked_add(1)
            .ok_or_else(|| invalid("completion sequence overflow"))?;
        let record = LiveCompletionRecord {
            format: LiveLedgerFormatV1::V1,
            session_id: self.session_id.clone(),
            channel_id: source.channel_id().clone(),
            sequence: LiveObservationSeq::new(next)
                .map_err(|_| invalid("invalid completion sequence"))?,
            event: LiveCompletionEvent::EffectTerminal {
                claim_id: OperationId(*claim.claim_id.as_uuid()),
                request_id: claim.request_id,
                outcome,
                token_accounting,
                diagnostic,
            },
        };
        let digest = effect_credits::completion_digest(&record)?;
        let phase = effect_credits::effect_phase(outcome);
        if state.claim_terminal_digests.get(&key) == Some(&digest) {
            let mut observed = owner.prepare_authority();
            let transition = dsl::LiveRequestMachineMutator::apply(
                &mut observed,
                dsl::LiveRequestInput::ObserveEffectSettlement {
                    claim_id: key.clone(),
                    request_id: request,
                    run_id: claim.run_id.to_string(),
                    target: serde_json::to_string(&claim.target)?,
                    outcome: phase,
                    completion_digest: digest.clone(),
                },
            )?;
            let [
                dsl::LiveRequestEffect::EffectSettlementObserved {
                    claim_id,
                    outcome: observed_phase,
                    completion_sequence,
                    completion_digest,
                },
            ] = transition.effects()
            else {
                return Err(invalid(
                    "generated settlement observation did not identify its record",
                ));
            };
            if claim_id != &key
                || *observed_phase != phase
                || completion_digest != &digest
                || *completion_sequence > head.reference.event_count
            {
                return Err(invalid("generated settlement observation changed identity"));
            }
            return Ok(PreparedLiveEffectSettlement {
                session_id: self.session_id.clone(),
                preparation: SettlementPreparation::Observed(
                    LiveObservationSeq::new(*completion_sequence)
                        .map_err(|_| invalid("invalid retained completion sequence"))?,
                ),
            });
        }
        if outcome == LivePhysicalEffectOutcome::NotStarted && !local_noninvocation_proven {
            return Err(invalid(
                "NotStarted requires custody of an unconsumed permit",
            ));
        }
        let charge = record
            .encode()
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?
            .charge();
        let input = dsl::LiveRequestInput::SettleEffect {
            claim_id: key.clone(),
            request_id: request,
            run_id: claim.run_id.to_string(),
            target: serde_json::to_string(&claim.target)?,
            outcome: phase,
            completion_records: charge.records,
            completion_bytes: charge.encoded_bytes,
            completion_sequence: record.sequence.get(),
            completion_digest: digest,
            local_noninvocation_proven,
            token_accounting_status: token_accounting.status(),
            token_accounting_record: serde_json::to_string(&token_accounting)?,
            observed_tokens: token_accounting.known_tokens().unwrap_or_default(),
        };
        let mut candidate = owner.prepare_authority();
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, input.clone())?;
        if !matches!(transition.effects(), [dsl::LiveRequestEffect::EffectSettled {
            claim_id, outcome: settled,
        }] if claim_id == &key && *settled == phase)
        {
            return Err(invalid(
                "generated settlement did not bind its exact outcome",
            ));
        }
        let sequence = record.sequence;
        let commit = PreparedLiveLedgerCommit::from_request_transition_with_completions(
            &self.session_id,
            Some(&head),
            &candidate,
            vec![record],
        )?;
        let fence = LiveRequestTimeFence {
            predecessor: owner,
            input,
            expected_snapshot: Arc::clone(&commit.successor().payload.request_snapshot),
            clock: Arc::clone(&self.clock),
        };
        Ok(PreparedLiveEffectSettlement {
            session_id: self.session_id.clone(),
            preparation: SettlementPreparation::Append {
                sequence,
                commit: Box::new(commit),
                fence: Box::new(fence),
            },
        })
    }

    pub(in crate::live_ledger) async fn commit_effect_settlement(
        &self,
        prepared: PreparedLiveEffectSettlement,
    ) -> Result<LiveObservationSeq, LiveRequestAuthorityError> {
        if prepared.session_id != self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        match prepared.preparation {
            SettlementPreparation::Observed(sequence) => Ok(sequence),
            SettlementPreparation::Append {
                sequence,
                commit,
                fence,
            } => {
                let expected = commit.successor().reference.clone();
                let ops = self
                    .store
                    .live_ledger_ops()
                    .ok_or(LiveRequestAuthorityError::Unsupported)?;
                let outcome = ops.commit_live_ledger(*commit, Arc::new(*fence)).await?;
                match outcome {
                    LiveLedgerCommitOutcome::Committed { head }
                    | LiveLedgerCommitOutcome::AlreadyCommitted { head }
                        if head == expected =>
                    {
                        Ok(sequence)
                    }
                    outcome => Err(LiveRequestAuthorityError::NotNewlyCommitted(Box::new(
                        outcome,
                    ))),
                }
            }
        }
    }
}
