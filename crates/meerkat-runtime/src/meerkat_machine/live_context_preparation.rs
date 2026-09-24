//! Mechanical custody and provider I/O for generated historical-prefix bootstrap.

use super::{MeerkatMachine, dsl};
use crate::RuntimeDriverError;
use crate::live_execution::{
    LiveContextBootstrapAppendAuthority, LiveContextPreparationFailure,
    LiveContextPreparationLease, LiveContextPreparationStatus,
};
use crate::tokio;
use meerkat_core::{LiveAppendDeliveryOutcome, LiveChannelId, SessionId};
use sha2::{Digest, Sha256};

impl MeerkatMachine {
    /// Atomically stage empty media and reserve its captured prefix against
    /// concurrent commit projection. This lock ends before any provider I/O.
    pub async fn stage_experimental_live_execution_with_preparation(
        &self,
        session_id: &SessionId,
        channel_id: &LiveChannelId,
        reserved_cursor: u64,
    ) -> Result<
        (
            super::ExperimentalLiveExecutionStageAuthority,
            LiveContextPreparationLease,
        ),
        RuntimeDriverError,
    > {
        let gate = self.live_context_projection_gate(session_id);
        let _guard = gate.lock().await;
        let stage = self
            .stage_experimental_live_execution(session_id, channel_id, 0)
            .await?;
        let lease = self
            .begin_live_context_preparation(session_id, channel_id, reserved_cursor)
            .await?;
        Ok((stage, lease))
    }

    /// Join the generated summary/tail barrier before publishing a delegation
    /// result. Strict channels have no bootstrap barrier.
    pub async fn wait_live_context_ready_for_results(
        &self,
        session_id: &SessionId,
        channel_id: &LiveChannelId,
    ) -> Result<(), RuntimeDriverError> {
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        if LiveContextPreparationStatus::from_state(&state, channel_id.as_str())
            .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            == LiveContextPreparationStatus::NotRequested
        {
            return Ok(());
        }
        drop(state);
        self.drain_live_context_outbox(session_id).await?;
        let lease = self
            .shared
            .live_context_preparation_leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(session_id.clone(), channel_id.clone()))
            .cloned();
        loop {
            let changed = lease
                .as_ref()
                .map(|lease| lease.cancellation.changed.notified());
            let (_, effects) = self
                .apply_session_dsl_input(
                    session_id,
                    dsl::MeerkatMachineInput::ObserveLiveContextDeliveryReadiness {
                        session_id: session_id.to_string(),
                        channel_id: channel_id.to_string(),
                    },
                    "ObserveLiveContextDeliveryReadiness",
                )
                .await
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
            let readiness = effects
                .as_slice()
                .iter()
                .find_map(|effect| match effect {
                    dsl::MeerkatMachineEffect::LiveContextDeliveryReadinessObserved {
                        session_id: session,
                        channel_id: channel,
                        readiness,
                    } if session == &session_id.to_string() && channel == channel_id.as_str() => {
                        Some(*readiness)
                    }
                    _ => None,
                })
                .ok_or_else(|| {
                    RuntimeDriverError::Internal(
                        "bootstrap barrier emitted no exact observation".into(),
                    )
                })?;
            match readiness {
                dsl::LiveContextDeliveryReadiness::Ready => return Ok(()),
                dsl::LiveContextDeliveryReadiness::Failed
                | dsl::LiveContextDeliveryReadiness::Revoked => {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: "bootstrap delivery barrier failed or was revoked".into(),
                    });
                }
                dsl::LiveContextDeliveryReadiness::Pending => {
                    self.drain_live_context_outbox_for_channel(session_id, channel_id)
                        .await?;
                    changed
                        .ok_or_else(|| {
                            RuntimeDriverError::Internal(
                                "pending bootstrap has no process-local job custody".into(),
                            )
                        })?
                        .await;
                }
            }
        }
    }

    pub async fn begin_live_context_preparation(
        &self,
        session_id: &SessionId,
        channel_id: &LiveChannelId,
        reserved_cursor: u64,
    ) -> Result<LiveContextPreparationLease, RuntimeDriverError> {
        let _guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        let channel = channel_id.as_str();
        let runtime_id = state
            .live_experimental_staged_runtime_by_channel
            .get(channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "preparation has no staged runtime".into(),
            })?;
        let fence = state
            .live_experimental_staged_fence_by_channel
            .get(channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "preparation has no staged fence".into(),
            })?;
        let generation = state
            .live_experimental_staged_generation_by_channel
            .get(channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "preparation has no staged generation".into(),
            })?;
        let lease = LiveContextPreparationLease {
            session_id: session_id.clone(),
            channel_id: channel_id.clone(),
            lease_id: uuid::Uuid::new_v4().to_string(),
            reserved_cursor,
            binding: crate::live_execution::LiveDelegationRuntimeBinding::new(
                session_id.clone(),
                channel_id.clone(),
                crate::identifiers::LogicalRuntimeId::new(runtime_id.0.clone()),
                fence.0,
                generation.0,
            ),
            cancellation: Default::default(),
        };
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                dsl::MeerkatMachineInput::BeginLiveContextPreparation {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.to_string(),
                    lease_id: lease.lease_id.clone(),
                    reserved_cursor,
                    runtime_id: runtime_id.clone(),
                    fence_token: *fence,
                    generation: *generation,
                },
                "BeginLiveContextPreparation",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        if !effects.as_slice().iter().any(|effect| {
            matches!(effect,
                dsl::MeerkatMachineEffect::LiveContextPreparationChanged {
                    session_id: session, channel_id: channel, lease_id, phase,
                } if session == &session_id.to_string() && channel == channel_id.as_str()
                    && lease_id == &lease.lease_id
                    && *phase == dsl::LiveContextPreparationPhase::Capturing
            )
        }) {
            return Err(RuntimeDriverError::Internal(
                "bootstrap capture emitted no exact lease".into(),
            ));
        }
        self.shared
            .live_context_preparation_leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((session_id.clone(), channel_id.clone()), lease.clone());
        Ok(lease)
    }

    /// Linearize source admission with ACK-cut recording in generated
    /// authority. No persistence, actor, provider, or projection I/O occurs.
    pub async fn record_live_context_observation(
        &self,
        lease: &LiveContextPreparationLease,
        observation_id: meerkat_core::LiveContextObservationId,
    ) -> Result<crate::live_execution::LiveContextObservationReceipt, RuntimeDriverError> {
        let binding = &lease.binding;
        let (_, effects) = self
            .apply_session_dsl_input(
                lease.session_id(),
                dsl::MeerkatMachineInput::RecordLiveContextObservation {
                    session_id: lease.session_id.to_string(),
                    channel_id: lease.channel_id.to_string(),
                    lease_id: lease.lease_id.clone(),
                    runtime_id: dsl::AgentRuntimeId::from_domain(binding.runtime_id()),
                    fence_token: dsl::FenceToken(binding.fence_token()),
                    generation: dsl::Generation(binding.generation()),
                    observation_id: observation_id.to_string(),
                    observation_namespace: observation_id.namespace().to_string(),
                    observation_channel_id: observation_id.channel_id().to_string(),
                },
                "RecordLiveContextObservation",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            if let dsl::MeerkatMachineEffect::LiveContextObservationRecorded {
                session_id,
                channel_id,
                lease_id,
                observation_id: recorded,
                ordinal,
                runtime_id,
                fence_token,
                generation,
            } = effect
                && session_id == &lease.session_id.to_string()
                && channel_id == lease.channel_id.as_str()
                && lease_id == &lease.lease_id
                && recorded == &observation_id.to_string()
                && runtime_id.0 == binding.runtime_id().0
                && fence_token.0 == binding.fence_token()
                && generation.0 == binding.generation()
            {
                return Ok(crate::live_execution::LiveContextObservationReceipt {
                    observation_id,
                    ordinal: *ordinal,
                });
            }
        }
        Err(RuntimeDriverError::Internal(
            "source admission emitted no exact receipt".into(),
        ))
    }

    /// Record the exact native ACK in the same intake order as source
    /// admissions, before waking any provider-send waiter.
    pub async fn record_live_context_bootstrap_ack_cut(
        &self,
        authority: &LiveContextBootstrapAppendAuthority,
    ) -> Result<(), RuntimeDriverError> {
        let lease = &authority.lease;
        let binding = &lease.binding;
        let (_, effects) = self
            .apply_session_dsl_input(
                lease.session_id(),
                dsl::MeerkatMachineInput::RecordLiveContextBootstrapAckCut {
                    session_id: lease.session_id.to_string(),
                    channel_id: lease.channel_id.to_string(),
                    lease_id: lease.lease_id.clone(),
                    runtime_id: dsl::AgentRuntimeId::from_domain(binding.runtime_id()),
                    fence_token: dsl::FenceToken(binding.fence_token()),
                    generation: dsl::Generation(binding.generation()),
                    append_id: authority.append_id.clone(),
                    content_digest: authority.content_digest.clone(),
                    reserved_cursor: lease.reserved_cursor,
                },
                "RecordLiveContextBootstrapAckCut",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        if effects.as_slice().iter().any(|effect| matches!(effect,
            dsl::MeerkatMachineEffect::LiveContextBootstrapAckCutRecorded {
                session_id, channel_id, lease_id, append_id, ..
            } if session_id == &lease.session_id.to_string() && channel_id == lease.channel_id.as_str()
                && lease_id == &lease.lease_id && append_id == &authority.append_id
        )) { return Ok(()); }
        Err(RuntimeDriverError::Internal(
            "bootstrap ACK emitted no exact cut receipt".into(),
        ))
    }

    pub async fn mark_live_context_preparation_generating(
        &self,
        lease: &LiveContextPreparationLease,
    ) -> Result<(), RuntimeDriverError> {
        self.apply_session_dsl_input(
            lease.session_id(),
            dsl::MeerkatMachineInput::GenerateLiveContextPreparation {
                session_id: lease.session_id.to_string(),
                channel_id: lease.channel_id.to_string(),
                lease_id: lease.lease_id.clone(),
            },
            "GenerateLiveContextPreparation",
        )
        .await
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        Ok(())
    }

    pub async fn fail_live_context_preparation(
        &self,
        lease: &LiveContextPreparationLease,
        reason: LiveContextPreparationFailure,
    ) -> Result<(), RuntimeDriverError> {
        self.apply_session_dsl_input(
            lease.session_id(),
            dsl::MeerkatMachineInput::FailLiveContextPreparation {
                session_id: lease.session_id.to_string(),
                channel_id: lease.channel_id.to_string(),
                lease_id: lease.lease_id.clone(),
                reason,
            },
            "FailLiveContextPreparation",
        )
        .await
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        lease.cancellation.cancel();
        Ok(())
    }

    pub async fn live_context_preparation_status(
        &self,
        session_id: &SessionId,
        channel_id: &LiveChannelId,
    ) -> Result<LiveContextPreparationStatus, RuntimeDriverError> {
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        if state
            .live_channel_session_by_channel
            .get(channel_id.as_str())
            != Some(&session_id.to_string())
            && !state
                .live_context_preparation_lease_by_channel
                .contains_key(channel_id.as_str())
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "bootstrap channel belongs to no exact session custody".into(),
            });
        }
        LiveContextPreparationStatus::from_state(&state, channel_id.as_str())
            .map_err(|error| RuntimeDriverError::Internal(error.to_string()))
    }

    pub async fn authorize_live_context_bootstrap_append(
        &self,
        lease: &LiveContextPreparationLease,
        summary: &str,
    ) -> Result<LiveContextBootstrapAppendAuthority, RuntimeDriverError> {
        let append_id = uuid::Uuid::new_v4().to_string();
        let digest = format!("{:x}", Sha256::digest(summary.as_bytes()));
        let (_, effects) = self
            .apply_session_dsl_input(
                lease.session_id(),
                dsl::MeerkatMachineInput::AuthorizeLiveContextBootstrapAppend {
                    session_id: lease.session_id.to_string(),
                    channel_id: lease.channel_id.to_string(),
                    lease_id: lease.lease_id.clone(),
                    append_id: append_id.clone(),
                    content_digest: digest.clone(),
                    reserved_cursor: lease.reserved_cursor,
                },
                "AuthorizeLiveContextBootstrapAppend",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            if let Some(authority) = LiveContextBootstrapAppendAuthority::from_generated_effect(
                lease, &append_id, &digest, effect,
            )
            .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(authority);
            }
        }
        Err(RuntimeDriverError::Internal(
            "bootstrap append emitted no exact authority".into(),
        ))
    }

    pub async fn resolve_live_context_bootstrap_append(
        &self,
        authority: &LiveContextBootstrapAppendAuthority,
        outcome: LiveAppendDeliveryOutcome,
    ) -> Result<(), RuntimeDriverError> {
        let observation = match outcome {
            LiveAppendDeliveryOutcome::Acknowledged => dsl::LiveContextAppendObservation::Delivered,
            LiveAppendDeliveryOutcome::Rejected => dsl::LiveContextAppendObservation::Rejected,
            LiveAppendDeliveryOutcome::Ambiguous => dsl::LiveContextAppendObservation::Ambiguous,
            LiveAppendDeliveryOutcome::InterruptedByClose => {
                dsl::LiveContextAppendObservation::InterruptedByClose
            }
        };
        let lease = &authority.lease;
        let mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(lease.session_id())
            .await?;
        let state = self
            .session_dsl_state(lease.session_id())
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            })?;
        let session_key = lease.session_id.to_string();
        // These are proposals, not deletion authority: the generated ACK
        // transition validates the exact complement and every companion value.
        let mut retained_cursors = state.live_context_queued_cursor_by_append.clone();
        if outcome == LiveAppendDeliveryOutcome::Acknowledged {
            retained_cursors.retain(|append, cursor| {
                state.live_context_queued_session_by_append.get(append) != Some(&session_key)
                    || *cursor > lease.reserved_cursor
            });
        }
        let mut retained_sessions = state.live_context_queued_session_by_append.clone();
        retained_sessions.retain(|append, _| retained_cursors.contains_key(append));
        let mut retained_digests = state.live_context_queued_digest_by_append.clone();
        retained_digests.retain(|append, _| retained_cursors.contains_key(append));
        let mut retained_commits = state.live_context_queued_commit_token_by_append.clone();
        retained_commits.retain(|append, _| retained_cursors.contains_key(append));
        let mut retained_dispositions = state.live_context_queued_disposition_by_append.clone();
        retained_dispositions.retain(|append, _| retained_cursors.contains_key(append));
        let mut retained_append_by_cursor = state.live_context_queued_append_by_cursor.clone();
        retained_append_by_cursor.retain(|_, append| retained_cursors.contains_key(append));
        self.apply_session_dsl_input(
            lease.session_id(),
            dsl::MeerkatMachineInput::ResolveLiveContextBootstrapAppend {
                session_id: session_key,
                channel_id: lease.channel_id.to_string(),
                lease_id: lease.lease_id.clone(),
                append_id: authority.append_id.clone(),
                content_digest: authority.content_digest.clone(),
                reserved_cursor: lease.reserved_cursor,
                observation,
                retained_sessions,
                retained_cursors,
                retained_digests,
                retained_commits,
                retained_dispositions,
                retained_append_by_cursor,
            },
            "ResolveLiveContextBootstrapAppend",
        )
        .await
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        if outcome == LiveAppendDeliveryOutcome::Acknowledged {
            self.shared
                .live_context_queued_rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(session, cursor), _| {
                    session != lease.session_id() || *cursor > lease.reserved_cursor
                });
            drop(mutation_guard);
            self.request_live_context_drain(lease.session_id(), lease.channel_id());
        }
        Ok(())
    }

    /// Wait for exact media activation without reserving a provider append,
    /// so the source owner can revalidate immediately before delivery.
    pub async fn wait_live_context_preparation_ready(
        &self,
        lease: &LiveContextPreparationLease,
    ) -> Result<(), RuntimeDriverError> {
        loop {
            let changed = lease.cancellation.changed.notified();
            let state = self
                .session_dsl_state(lease.session_id())
                .await
                .map_err(|reason| RuntimeDriverError::ValidationFailed {
                    reason: reason.to_string(),
                })?;
            if lease.cancellation.is_cancelled() {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: "bootstrap lease was cancelled".into(),
                });
            }
            // Media active and the user has spoken on this channel (a user
            // provider turn started or a client delegation was admitted):
            // startup history appended into silence is spoken aloud by the
            // provider, so the bootstrap waits for the conversation to exist.
            if state
                .live_execution_phase_by_channel
                .get(lease.channel_id.as_str())
                == Some(&dsl::LiveExecutionChannelPhase::Active)
                && state
                    .live_conversation_started_channels
                    .contains(lease.channel_id.as_str())
            {
                break;
            }
            changed.await;
        }
        let host = self
            .live_context_mirror_host()
            .ok_or_else(|| RuntimeDriverError::Internal("bootstrap host is unavailable".into()))?;
        tokio::select! {
            biased;
            () = lease.cancellation.cancelled() => Err(RuntimeDriverError::ValidationFailed {
                reason: "bootstrap lease was cancelled".into(),
            }),
            result = host.wait_bootstrap_control_ready(lease) => result.map_err(RuntimeDriverError::Internal),
        }
    }

    /// Generation is facade-owned. This method owns only the exact quiet
    /// provider append and feedback, never a projection lock across I/O.
    /// Summary text is opaque payload, not source provenance. Only the sealed
    /// lease supplies the reserved edge; no future-returned cursor or claimed
    /// source identity is admitted here.
    pub async fn deliver_live_context_preparation(
        &self,
        lease: &LiveContextPreparationLease,
        summary: String,
    ) -> Result<(), RuntimeDriverError> {
        self.wait_live_context_preparation_ready(lease).await?;
        let Some(host) = self.live_context_mirror_host() else {
            self.fail_live_context_preparation(
                lease,
                LiveContextPreparationFailure::DeliveryRejected,
            )
            .await?;
            return Err(RuntimeDriverError::Internal(
                "bootstrap host is unavailable".into(),
            ));
        };
        let authority = self
            .authorize_live_context_bootstrap_append(lease, &summary)
            .await?;
        let outcome = tokio::select! {
            biased;
            () = lease.cancellation.cancelled() => return Err(RuntimeDriverError::ValidationFailed {
                reason: "bootstrap lease was cancelled".into(),
            }),
            result = host.append_bootstrap_context(authority.clone(), summary) => result,
        };
        let (returned, outcome) = match outcome {
            Ok(result) => result,
            Err(error) => {
                // An I/O error is not evidence of rejection or non-delivery.
                self.resolve_live_context_bootstrap_append(
                    &authority,
                    LiveAppendDeliveryOutcome::Ambiguous,
                )
                .await?;
                return Err(RuntimeDriverError::Internal(error));
            }
        };
        if returned.lease.lease_id != authority.lease.lease_id
            || returned.append_id != authority.append_id
            || returned.content_digest != authority.content_digest
        {
            self.resolve_live_context_bootstrap_append(
                &authority,
                LiveAppendDeliveryOutcome::Ambiguous,
            )
            .await?;
            return Err(RuntimeDriverError::Internal(
                "bootstrap host returned another append's authority".into(),
            ));
        }
        self.resolve_live_context_bootstrap_append(&returned, outcome)
            .await
    }

    pub(super) fn realize_live_context_preparation_cancellation(
        &self,
        session_id: &SessionId,
        state: &dsl::MeerkatMachineState,
        effects: &super::DslTransitionEffects,
    ) {
        let mut leases = self
            .shared
            .live_context_preparation_leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        leases.retain(|(session, channel), lease| {
            if session != session_id {
                return true;
            }
            if effects.as_slice().iter().any(|effect| {
                !matches!(
                    effect,
                    dsl::MeerkatMachineEffect::LiveContextDeliveryReadinessObserved { .. }
                        | dsl::MeerkatMachineEffect::LiveContextAppendDeferred { .. }
                        | dsl::MeerkatMachineEffect::LiveContextAppendAlreadyCovered { .. }
                )
            }) {
                lease.cancellation.changed.notify_waiters();
            }
            match state
                .live_context_preparation_phase_by_channel
                .get(channel.as_str())
            {
                Some(dsl::LiveContextPreparationPhase::Failed) => {
                    lease.cancellation.cancel();
                    false
                }
                Some(dsl::LiveContextPreparationPhase::ProviderAcknowledged)
                    if state
                        .live_revoked_execution_channels
                        .contains(channel.as_str()) =>
                {
                    lease.cancellation.cancel();
                    false
                }
                _ => true,
            }
        });
    }

    /// Called only after exact unregister finalization has authorized removal
    /// of this session incarnation.
    pub(super) fn release_finalized_live_context_preparation_jobs(&self, session_id: &SessionId) {
        self.shared
            .live_context_preparation_leases
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(session, _), lease| {
                if session != session_id {
                    return true;
                }
                lease.cancellation.cancel();
                false
            });
    }
}
