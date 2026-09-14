use super::*;
use crate::live_ledger::source::LiveSourceRow;
use crate::live_source::{LiveSourceDisposition, LiveSourceEntryRecord};
use meerkat_core::InputId;
use meerkat_core::live_execution::request::{
    LiveRequestCancelIntent, LiveRequestCancellationReason,
};

pub(crate) struct CommittedLiveSourceCancellation {
    pub intent: LiveRequestCancelIntent,
    pub target: Option<LiveSourceCancellationTarget>,
}

pub(crate) struct LiveSourceCancellationTarget {
    pub input_id: InputId,
    pub request_id: meerkat_core::ops::OperationId,
}

pub(in crate::live_ledger) struct PreparedLiveSourceCancellation {
    result: CommittedLiveSourceCancellation,
    pub(in crate::live_ledger) commit: PreparedLiveLedgerCommit,
    fence: LiveRequestTimeFence,
}

fn invalid(reason: &'static str) -> LiveRequestAuthorityError {
    LiveRequestAuthorityError::InvalidSourceCancellation(reason)
}

impl LiveRequestStoreOwner {
    pub(crate) async fn pending_source_cancellations(
        &self,
    ) -> Result<Vec<LiveRequestCancelIntent>, LiveRequestAuthorityError> {
        let ops = self
            .store
            .live_ledger_ops()
            .ok_or(LiveRequestAuthorityError::Unsupported)?;
        let Some(head) = ops.load_live_head(&self.session_id).await? else {
            return Ok(Vec::new());
        };
        head.validate_payload()?;
        if head.reference.session_id != self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        let owner = dsl::LiveRequestMachineAuthority::recover_from_state(
            crate::generated::live_request_state::decode(&head.payload.request_snapshot)?,
        )?;
        let mut pending = Vec::new();
        for request in &owner.state().admitted_requests {
            let source = owner
                .state()
                .request_sources
                .get(request)
                .ok_or_else(|| invalid("admitted request has no source"))?;
            let Some(reason) = owner.state().source_cancellations.get(source) else {
                continue;
            };
            let mut candidate = owner.prepare_authority();
            let transition = dsl::LiveRequestMachineMutator::apply(
                &mut candidate,
                dsl::LiveRequestInput::CancelSource {
                    source: source.clone(),
                    reason: *reason,
                },
            )?;
            match transition.effects() {
                [
                    dsl::LiveRequestEffect::SourceCancellationRetained {
                        source: observed,
                        reason: observed_reason,
                    },
                ] if observed == source && observed_reason == reason => {}
                [
                    dsl::LiveRequestEffect::SourceCancellationRetained {
                        source: observed,
                        reason: observed_reason,
                    },
                    dsl::LiveRequestEffect::RequestCancellationRequired { request_id },
                ] if observed == source && observed_reason == reason && request_id == request => {
                    let intent = LiveRequestCancelIntent {
                        source: serde_json::from_str(source)?,
                        reason: *reason,
                    };
                    if intent.source.session_id() != &self.session_id {
                        return Err(LiveRequestAuthorityError::SessionMismatch);
                    }
                    let request_id =
                        meerkat_core::ops::OperationId(uuid::Uuid::parse_str(request).map_err(
                            |_| invalid("generated cancellation request identity is malformed"),
                        )?);
                    if let Some(hold) = self.observe_cancelled_callback_hold(&request_id).await? {
                        super::request_completion::LiveRequestCompletionProgress::CancelledCallbackHeld(hold)
                            .trace_recovery_hold(&self.session_id, &request_id);
                        continue;
                    }
                    pending.push(intent);
                }
                _ => return Err(invalid("generated cancellation recovery changed identity")),
            }
        }
        Ok(pending)
    }

    pub(in crate::live_ledger) async fn prepare_source_cancellation(
        &self,
        intent: LiveRequestCancelIntent,
    ) -> Result<PreparedLiveSourceCancellation, LiveRequestAuthorityError> {
        if intent.source.session_id() != &self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        let ops = self
            .store
            .live_ledger_ops()
            .ok_or(LiveRequestAuthorityError::Unsupported)?;
        let before = ops.load_live_head(&self.session_id).await?;
        let source_before = ops.lookup_live_source(&intent.source).await?;
        let owner = match &before {
            Some(head) => {
                head.validate_payload()?;
                if head.reference.session_id != self.session_id {
                    return Err(LiveRequestAuthorityError::SessionMismatch);
                }
                dsl::LiveRequestMachineAuthority::recover_from_state(
                    crate::generated::live_request_state::decode(&head.payload.request_snapshot)?,
                )?
            }
            None => dsl::LiveRequestMachineAuthority::new(),
        };
        let source = serde_json::to_string(&intent.source)?;
        let state = owner.state();
        let request = state.source_requests.get(&source);
        let retained = state.source_cancellations.get(&source);
        let source_record = source_before
            .as_ref()
            .map(LiveSourceRow::record)
            .transpose()?;
        match &source_record {
            None if request.is_none() && retained.is_none() => {}
            Some(LiveSourceEntryRecord::CancellationOnly { intent: original })
                if request.is_none()
                    && retained == Some(&original.reason)
                    && original.source == intent.source => {}
            Some(LiveSourceEntryRecord::Reservation { record })
                if request == Some(&record.request_id().to_string())
                    && record.source() == &intent.source
                    && record.cancellation() == retained
                    && state.request_payloads.get(&record.request_id().to_string())
                        == Some(&serde_json::to_string(&record.frozen_digest().map_err(
                            |error| RuntimeStoreError::ReadFailed(error.to_string()),
                        )?)?) => {}
            Some(LiveSourceEntryRecord::Reservation { record })
                if request.is_none()
                    && record.source() == &intent.source
                    && record.cancellation() == retained
                    && matches!(record.disposition(), LiveSourceDisposition::Refused { reason }
                        if state.source_refusals.get(&source) == Some(reason))
                    && state.source_refusal_payloads.get(&source)
                        == Some(&serde_json::to_string(&record.frozen_digest().map_err(
                            |error| RuntimeStoreError::ReadFailed(error.to_string()),
                        )?)?) => {}
            _ => {
                let current = ops.load_live_head(&self.session_id).await?;
                if current.as_ref().map(|head| &head.reference)
                    != before.as_ref().map(|head| &head.reference)
                {
                    return Err(LiveRequestAuthorityError::NotNewlyCommitted(Box::new(
                        LiveLedgerCommitOutcome::Conflict {
                            current: current.map(|head| head.reference),
                        },
                    )));
                }
                return Err(invalid("source row and generated source owner disagree"));
            }
        }
        let command = dsl::LiveRequestInput::CancelSource {
            source: source.clone(),
            reason: retained.copied().unwrap_or(intent.reason),
        };
        let mut candidate = owner.prepare_authority();
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, command.clone())?;
        let Some(dsl::LiveRequestEffect::SourceCancellationRetained {
            source: echoed_source,
            reason,
        }) = transition.effects().first()
        else {
            return Err(invalid("generated source cancellation receipt is missing"));
        };
        if echoed_source != &source
            || candidate.state().source_cancellations.get(&source) != Some(reason)
        {
            return Err(invalid(
                "generated source cancellation receipt changed identity",
            ));
        }
        let cancellation_required = match transition.effects() {
            [_] => None,
            [
                _,
                dsl::LiveRequestEffect::RequestCancellationRequired { request_id },
            ] if request == Some(request_id) => Some(request_id),
            _ => return Err(invalid("generated cancellation target changed identity")),
        };
        let intent = LiveRequestCancelIntent {
            source: intent.source,
            reason: *reason,
        };
        let replacement = match source_record {
            Some(LiveSourceEntryRecord::Reservation { record }) => {
                LiveSourceEntryRecord::Reservation {
                    record: Box::new(record.with_cancellation(intent.reason)),
                }
            }
            _ => LiveSourceEntryRecord::CancellationOnly {
                intent: intent.clone(),
            },
        };
        let target = match cancellation_required {
            Some(request) if state.admitted_requests.contains(request) => {
                let input = super::request_completion::current_request_input(state, request)
                    .ok_or_else(|| invalid("admitted cancellation has no exact input"))?;
                Some(LiveSourceCancellationTarget {
                    input_id: InputId(uuid::Uuid::parse_str(input).map_err(|_| {
                        invalid("generated cancellation input identity is malformed")
                    })?),
                    request_id: meerkat_core::ops::OperationId(
                        uuid::Uuid::parse_str(request).map_err(|_| {
                            invalid("generated cancellation request identity is malformed")
                        })?,
                    ),
                })
            }
            _ => None,
        };
        let commit = PreparedLiveLedgerCommit::from_request_transition(
            &self.session_id,
            before.as_ref(),
            &candidate,
        )?
        .with_source_mutation(source_before.as_ref(), LiveSourceRow::encode(&replacement)?)?;
        let fence = LiveRequestTimeFence {
            predecessor: owner,
            input: command,
            expected_snapshot: Arc::clone(&commit.successor().payload.request_snapshot),
            clock: Arc::clone(&self.clock),
        };
        Ok(PreparedLiveSourceCancellation {
            result: CommittedLiveSourceCancellation { intent, target },
            commit,
            fence,
        })
    }

    pub(crate) async fn cancel_source(
        &self,
        intent: LiveRequestCancelIntent,
    ) -> Result<CommittedLiveSourceCancellation, LiveRequestAuthorityError> {
        let mut attempts_remaining = 8;
        loop {
            let result = match self.prepare_source_cancellation(intent.clone()).await {
                Ok(prepared) => prepared.commit(self.store.as_ref()).await,
                Err(error) => Err(error),
            };
            match result {
                Err(LiveRequestAuthorityError::NotNewlyCommitted(outcome))
                    if attempts_remaining > 1
                        && matches!(
                            *outcome,
                            LiveLedgerCommitOutcome::Conflict { .. }
                                | LiveLedgerCommitOutcome::SourceConflict { .. }
                        ) =>
                {
                    attempts_remaining -= 1;
                    tracing::debug!(
                        session_id = %self.session_id, attempts_remaining,
                        "repreparing uncommitted Live source cancellation after owner conflict"
                    );
                    tokio::task::yield_now().await;
                }
                result => return result,
            }
        }
    }
}

impl PreparedLiveSourceCancellation {
    pub(in crate::live_ledger) async fn commit(
        self,
        store: &dyn RuntimeStore,
    ) -> Result<CommittedLiveSourceCancellation, LiveRequestAuthorityError> {
        let ops = store
            .live_ledger_ops()
            .ok_or(LiveRequestAuthorityError::Unsupported)?;
        match ops
            .commit_live_ledger(self.commit, Arc::new(self.fence))
            .await?
        {
            LiveLedgerCommitOutcome::Committed { .. } => Ok(self.result),
            outcome => Err(LiveRequestAuthorityError::NotNewlyCommitted(Box::new(
                outcome,
            ))),
        }
    }
}

/// Reserve the encoded growth of the intent map, cancelled-request set, and
/// existing source row before admitting work. The open ingress flag also
/// reserves its exact `true` -> `false` JSON growth. Neither needs a new row.
pub(in crate::live_ledger) fn reserved_charge(
    state: &dsl::LiveRequestMachineState,
) -> Result<crate::live_resources::LiveResourceCharge, RuntimeStoreError> {
    use crate::live_resources::LiveResourceCharge;
    let mut total = LiveResourceCharge {
        records: 0,
        encoded_bytes: u64::from(state.ingress_open),
    };
    for (source, request) in &state.source_requests {
        if state.source_cancellations.contains_key(source) {
            continue;
        }
        let mut ceiling = 0;
        for reason in [
            LiveRequestCancellationReason::OperatorRequested,
            LiveRequestCancellationReason::GrantRevoked,
            LiveRequestCancellationReason::SessionArchived,
            LiveRequestCancellationReason::ExplicitSupersession,
        ] {
            let bytes = serde_json::to_vec(&(
                (source, reason),
                [request],
                Some(reason),
                LiveSourceDisposition::CancelledWithoutRun { reason },
            ))
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?
            .len() as u64;
            ceiling = ceiling.max(bytes);
        }
        total = total
            .checked_add(LiveResourceCharge {
                records: 0,
                encoded_bytes: ceiling,
            })
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    }
    Ok(total)
}
