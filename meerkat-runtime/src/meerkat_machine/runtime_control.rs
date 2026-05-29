use super::*;

#[cfg(not(target_arch = "wasm32"))]
type AcceptInputWithCompletionFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<
                    (AcceptOutcome, Option<crate::completion::CompletionHandle>),
                    RuntimeDriverError,
                >,
            > + Send
            + 'a,
    >,
>;

#[cfg(target_arch = "wasm32")]
type AcceptInputWithCompletionFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<
                    (AcceptOutcome, Option<crate::completion::CompletionHandle>),
                    RuntimeDriverError,
                >,
            > + 'a,
    >,
>;

#[cfg(feature = "live")]
fn dsl_live_channel_status_from_observation(
    status: &meerkat_core::live_adapter::LiveAdapterStatus,
) -> (
    crate::meerkat_machine::dsl::LiveChannelPublicStatus,
    Option<crate::meerkat_machine::dsl::LiveChannelDegradationReason>,
    Option<String>,
) {
    use crate::meerkat_machine::dsl::{
        LiveChannelDegradationReason as DslReason, LiveChannelPublicStatus as DslStatus,
    };
    use meerkat_core::live_adapter::LiveAdapterStatus;

    match status {
        LiveAdapterStatus::Idle => (DslStatus::Idle, None, None),
        LiveAdapterStatus::Opening => (DslStatus::Opening, None, None),
        LiveAdapterStatus::Ready => (DslStatus::Ready, None, None),
        LiveAdapterStatus::Closing => (DslStatus::Closing, None, None),
        LiveAdapterStatus::Closed => (DslStatus::Closed, None, None),
        LiveAdapterStatus::Degraded { reason } => {
            let (reason, detail) = dsl_live_channel_degradation_reason(reason);
            (DslStatus::Degraded, Some(reason), detail)
        }
        other => (
            DslStatus::Degraded,
            Some(DslReason::Unknown),
            Some(format!("{other:?}")),
        ),
    }
}

#[cfg(feature = "live")]
fn dsl_live_channel_degradation_reason(
    reason: &meerkat_core::live_adapter::LiveDegradationReason,
) -> (
    crate::meerkat_machine::dsl::LiveChannelDegradationReason,
    Option<String>,
) {
    use crate::meerkat_machine::dsl::LiveChannelDegradationReason as DslReason;
    use meerkat_core::live_adapter::LiveDegradationReason;

    match reason {
        LiveDegradationReason::RateLimited => (DslReason::RateLimited, None),
        LiveDegradationReason::ProviderThrottled => (DslReason::ProviderThrottled, None),
        LiveDegradationReason::NetworkUnstable => (DslReason::NetworkUnstable, None),
        LiveDegradationReason::Other { detail } => {
            (DslReason::Other, Some(detail.clone().into_owned()))
        }
        other => (DslReason::Unknown, Some(format!("{other:?}"))),
    }
}

#[cfg(feature = "live")]
fn dsl_live_command_kind(
    kind: meerkat_live::LiveCommandAcceptanceKind,
) -> crate::meerkat_machine::dsl::LiveCommandPublicKind {
    match kind {
        meerkat_live::LiveCommandAcceptanceKind::SendInput => {
            crate::meerkat_machine::dsl::LiveCommandPublicKind::SendInput
        }
        meerkat_live::LiveCommandAcceptanceKind::CommitInput => {
            crate::meerkat_machine::dsl::LiveCommandPublicKind::CommitInput
        }
        meerkat_live::LiveCommandAcceptanceKind::Interrupt => {
            crate::meerkat_machine::dsl::LiveCommandPublicKind::Interrupt
        }
        meerkat_live::LiveCommandAcceptanceKind::TruncateAssistantOutput => {
            crate::meerkat_machine::dsl::LiveCommandPublicKind::TruncateAssistantOutput
        }
    }
}

#[cfg(feature = "live")]
fn dsl_live_command_rejection_reason(
    error: &meerkat_live::LiveAdapterHostError,
) -> crate::meerkat_machine::dsl::LiveCommandRejectionReason {
    use crate::meerkat_machine::dsl::LiveCommandRejectionReason as DslReason;
    use meerkat_live::LiveAdapterHostError;

    match error {
        LiveAdapterHostError::ChannelNotFound(_) => DslReason::ChannelNotFound,
        LiveAdapterHostError::NoAdapter(_) => DslReason::NoAdapter,
        LiveAdapterHostError::ChannelNotReady(_, _) => DslReason::ChannelNotReady,
        LiveAdapterHostError::UnsupportedCommand(_) => DslReason::UnsupportedCommand,
        LiveAdapterHostError::AdapterError(_) => DslReason::AdapterError,
        _ => DslReason::InternalHostError,
    }
}

#[cfg(feature = "live")]
fn dsl_live_channel_request_rejection_reason(
    error: &meerkat_live::LiveAdapterHostError,
) -> crate::meerkat_machine::dsl::LiveChannelRequestRejectionReason {
    use crate::meerkat_machine::dsl::LiveChannelRequestRejectionReason as DslReason;
    use meerkat_live::LiveAdapterHostError;

    match error {
        LiveAdapterHostError::ChannelNotFound(_) => DslReason::ChannelNotFound,
        LiveAdapterHostError::NoAdapter(_) => DslReason::NoAdapter,
        _ => DslReason::InternalHostError,
    }
}

#[cfg(feature = "live")]
fn extract_live_websocket_token_admission(
    effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
    session_id: &str,
    channel_id: &str,
    token: &str,
    transition: &str,
) -> Result<LiveWebsocketTokenAdmissionAuthority, RuntimeDriverError> {
    effects
        .iter()
        .find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveWebsocketTokenAdmissionResolved {
                session_id: effect_session_id,
                channel_id: effect_channel_id,
                token: effect_token,
                admitted,
                rejection,
                public_error_class,
                sequence,
            } if effect_session_id == session_id
                && effect_channel_id == channel_id
                && effect_token == token =>
            {
                Some(LiveWebsocketTokenAdmissionAuthority {
                    admitted: *admitted,
                    rejection: *rejection,
                    public_error_class: *public_error_class,
                    sequence: *sequence,
                })
            }
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "{transition} for channel '{channel_id}' emitted no LiveWebsocketTokenAdmissionResolved effect"
            ))
        })
}

/// Machine-generated authority for runtime cleanup after a completion waiter
/// resolves. The action is projected from a generated DSL effect; surfaces use
/// this wrapper instead of matching completion outcomes locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCompletionCleanupAuthority {
    pub action: crate::meerkat_machine::dsl::RuntimeCompletionCleanupAction,
    pub pre_admission_action: crate::meerkat_machine::dsl::RuntimeCompletionPreAdmissionAction,
    pub outcome: crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome,
    pub live_session: crate::meerkat_machine::dsl::RuntimeCompletionLiveSessionObservation,
    pub archived_by_authority: bool,
}

impl RuntimeCompletionCleanupAuthority {
    pub fn requires_runtime_cleanup(self) -> bool {
        matches!(
            self.action,
            crate::meerkat_machine::dsl::RuntimeCompletionCleanupAction::CleanupRuntime
        )
    }

    pub fn releases_pre_admission(self) -> bool {
        matches!(
            self.pre_admission_action,
            crate::meerkat_machine::dsl::RuntimeCompletionPreAdmissionAction::ReleasePreAdmission
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeCompletionCleanupEffect {
    action: crate::meerkat_machine::dsl::RuntimeCompletionCleanupAction,
    pre_admission_action: crate::meerkat_machine::dsl::RuntimeCompletionPreAdmissionAction,
}

fn runtime_completion_cleanup_effect_from_effects(
    session_id: &SessionId,
    effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
) -> Result<RuntimeCompletionCleanupEffect, RuntimeDriverError> {
    let expected_session_id = crate::meerkat_machine::dsl::SessionId::from_domain(session_id);
    effects
        .iter()
        .find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::RuntimeCompletionCleanupResolved {
                session_id: effect_session_id,
                action,
                pre_admission_action,
            } if effect_session_id == &expected_session_id => Some(RuntimeCompletionCleanupEffect {
                action: *action,
                pre_admission_action: *pre_admission_action,
            }),
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "ResolveRuntimeCompletionCleanup for session '{session_id}' emitted no RuntimeCompletionCleanupResolved effect"
            ))
        })
}

/// Machine-generated authority for mechanical completion-waiter failures.
/// The generated effect owns both admission release and the public failure
/// class/reason; surfaces only map these closed values to transport envelopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCompletionWaitFailureAuthority {
    pub failure: crate::meerkat_machine::dsl::RuntimeCompletionWaitFailureObservation,
    pub pre_admission_action: crate::meerkat_machine::dsl::RuntimeCompletionPreAdmissionAction,
    pub public_error_class:
        crate::meerkat_machine::dsl::RuntimeCompletionWaitFailurePublicErrorClass,
    pub public_reason: crate::meerkat_machine::dsl::RuntimeCompletionWaitFailurePublicReason,
    pub resumable: bool,
}

impl RuntimeCompletionWaitFailureAuthority {
    pub fn releases_pre_admission(self) -> bool {
        matches!(
            self.pre_admission_action,
            crate::meerkat_machine::dsl::RuntimeCompletionPreAdmissionAction::ReleasePreAdmission
        )
    }
}

fn runtime_completion_wait_failure_authority_from_effects(
    session_id: &SessionId,
    failure: crate::meerkat_machine::dsl::RuntimeCompletionWaitFailureObservation,
    effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
) -> Result<RuntimeCompletionWaitFailureAuthority, RuntimeDriverError> {
    let expected_session_id = crate::meerkat_machine::dsl::SessionId::from_domain(session_id);
    effects
        .iter()
        .find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::RuntimeCompletionWaitFailureResolved {
                session_id: effect_session_id,
                failure: effect_failure,
                pre_admission_action,
                public_error_class,
                public_reason,
                resumable,
            } if effect_session_id == &expected_session_id && *effect_failure == failure => {
                Some(RuntimeCompletionWaitFailureAuthority {
                    failure: *effect_failure,
                    pre_admission_action: *pre_admission_action,
                    public_error_class: *public_error_class,
                    public_reason: *public_reason,
                    resumable: *resumable,
                })
            }
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "ResolveRuntimeCompletionWaitFailure for session '{session_id}' emitted no RuntimeCompletionWaitFailureResolved effect"
            ))
        })
}

impl MeerkatMachine {
    pub async fn resolve_runtime_completion_cleanup(
        &self,
        session_id: &SessionId,
        observation: crate::completion::CompletionCleanupObservation,
        archived_by_authority: bool,
        live_session: crate::meerkat_machine::dsl::RuntimeCompletionLiveSessionObservation,
    ) -> Result<RuntimeCompletionCleanupAuthority, RuntimeDriverError> {
        let observed_outcome = observation.observed_outcome();
        let input =
            crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveRuntimeCompletionCleanup {
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                observation_session_id: crate::meerkat_machine::dsl::SessionId::from_domain(
                    observation.owner_session_id(),
                ),
                observation_agent_runtime_id: observation.owner_agent_runtime_id().cloned(),
                observation_fence_token: observation.owner_fence_token(),
                observation_runtime_generation: observation.owner_runtime_generation(),
                observation_runtime_epoch_id: observation.owner_runtime_epoch_id().cloned(),
                outcome: observed_outcome,
                archived_by_authority,
                live_session,
            };
        let effects = self
            .preview_session_dsl_input(session_id, input, "ResolveRuntimeCompletionCleanup")
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        let cleanup_effect = runtime_completion_cleanup_effect_from_effects(session_id, &effects)?;
        Ok(RuntimeCompletionCleanupAuthority {
            action: cleanup_effect.action,
            pre_admission_action: cleanup_effect.pre_admission_action,
            outcome: observed_outcome,
            live_session,
            archived_by_authority,
        })
    }

    pub async fn resolve_runtime_completion_wait_failure(
        &self,
        session_id: &SessionId,
        error: &crate::completion::CompletionWaitError,
    ) -> Result<RuntimeCompletionWaitFailureAuthority, RuntimeDriverError> {
        let failure = error.wait_failure_observation();
        let input =
            crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveRuntimeCompletionWaitFailure {
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                failure,
            };
        let effects = self
            .preview_session_dsl_input(session_id, input, "ResolveRuntimeCompletionWaitFailure")
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        runtime_completion_wait_failure_authority_from_effects(session_id, failure, &effects)
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_open_admission(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        llm_identity: &meerkat_core::SessionLlmIdentity,
    ) -> Result<LiveOpenAdmissionAuthority, RuntimeDriverError> {
        let channel_id_string = channel_id.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveOpenAdmission {
                    session_id: session_id.to_string(),
                    channel_id: channel_id_string.clone(),
                    llm_identity: crate::meerkat_machine::dsl::SessionLlmIdentity::from_domain(
                        llm_identity,
                    ),
                },
                "ResolveLiveOpenAdmission",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        let authority = effects.as_slice().iter().find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveOpenAdmissionResolved {
                session_id: effect_session_id,
                channel_id: effect_channel_id,
                bound_llm_identity,
                admitted,
                rejection,
                sequence,
            } if *effect_session_id == session_id.to_string()
                && *effect_channel_id == channel_id_string =>
            {
                Some(LiveOpenAdmissionAuthority::from_generated_effect(
                    session_id.clone(),
                    channel_id.clone(),
                    *admitted,
                    *rejection,
                    bound_llm_identity.clone(),
                    *sequence,
                ))
            }
            _ => None,
        });
        match authority {
            Some(authority) => authority.map_err(RuntimeDriverError::Internal),
            None => Err(RuntimeDriverError::Internal(format!(
                "ResolveLiveOpenAdmission for channel '{channel_id_string}' emitted no LiveOpenAdmissionResolved effect"
            ))),
        }
    }

    #[cfg(feature = "live")]
    pub async fn live_channel_bound_llm_identity(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> Result<Option<meerkat_core::SessionLlmIdentity>, RuntimeDriverError> {
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        state
            .live_channel_identity_by_channel
            .get(&channel_id.to_string())
            .cloned()
            .map(meerkat_core::SessionLlmIdentity::try_from)
            .transpose()
            .map_err(RuntimeDriverError::Internal)
    }

    #[cfg(feature = "live")]
    pub async fn abandon_live_open_admission(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> Result<(), RuntimeDriverError> {
        self.apply_session_dsl_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::AbandonLiveOpenAdmission {
                session_id: session_id.to_string(),
                channel_id: channel_id.to_string(),
            },
            "AbandonLiveOpenAdmission",
        )
        .await
        .map(|_| ())
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })
    }

    #[cfg(feature = "live")]
    pub async fn live_channel_is_active_for_session(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> bool {
        self.session_dsl_state(session_id)
            .await
            .ok()
            .and_then(|state| {
                state
                    .live_active_channel_by_session
                    .get(&session_id.to_string())
                    .cloned()
            })
            .is_some_and(|active| active == channel_id.to_string())
    }

    #[cfg(feature = "live")]
    pub async fn live_session_for_active_channel(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> Option<SessionId> {
        let channel_id = channel_id.to_string();
        let session_ids = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect::<Vec<_>>()
        };

        for session_id in session_ids {
            let Ok(state) = self.session_dsl_state(&session_id).await else {
                continue;
            };
            if state
                .live_channel_session_by_channel
                .get(&channel_id)
                .is_some_and(|owner| owner == &session_id.to_string())
            {
                return Some(session_id);
            }
        }
        None
    }

    /// Read-only routing projection over generated live channel status
    /// authority. Active channels route through the active binding; closed
    /// retained channels route through the machine-owned close result map.
    #[cfg(feature = "live")]
    pub async fn live_session_for_status_channel(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> Option<SessionId> {
        let channel_id = channel_id.to_string();
        let session_ids = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect::<Vec<_>>()
        };

        for session_id in session_ids {
            let Ok(state) = self.session_dsl_state(&session_id).await else {
                continue;
            };
            if state
                .live_channel_session_by_channel
                .get(&channel_id)
                .is_some_and(|owner| owner == &session_id.to_string())
                || state.live_close_status_by_channel.contains_key(&channel_id)
            {
                return Some(session_id);
            }
        }
        None
    }

    /// Read-only routing projection over generated WebRTC token-owner state.
    /// Admission still occurs only when the selected machine resolves the
    /// typed admission input.
    #[cfg(feature = "live")]
    pub async fn live_session_for_webrtc_token(&self, token: &str) -> Option<SessionId> {
        let session_ids = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect::<Vec<_>>()
        };

        for session_id in session_ids {
            let Ok(state) = self.session_dsl_state(&session_id).await else {
                continue;
            };
            if state.live_webrtc_token_channel_by_token.contains_key(token) {
                return Some(session_id);
            }
        }
        None
    }

    /// Read-only routing projection over generated WebSocket token-owner
    /// state. The token lookup selects which machine receives the admission
    /// input; it does not decide token validity or public result class.
    #[cfg(feature = "live")]
    pub async fn live_session_for_websocket_token(&self, token: &str) -> Option<SessionId> {
        let session_ids = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect::<Vec<_>>()
        };

        for session_id in session_ids {
            let Ok(state) = self.session_dsl_state(&session_id).await else {
                continue;
            };
            if state
                .live_websocket_token_channel_by_token
                .contains_key(token)
            {
                return Some(session_id);
            }
        }
        None
    }

    #[cfg(feature = "live")]
    pub async fn live_active_channel_for_session(
        &self,
        session_id: &SessionId,
    ) -> Option<meerkat_live::LiveChannelId> {
        self.session_dsl_state(session_id)
            .await
            .ok()
            .and_then(|state| {
                state
                    .live_active_channel_by_session
                    .get(&session_id.to_string())
                    .cloned()
            })
            .map(meerkat_live::LiveChannelId::new)
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_refresh_queued_result(
        &self,
        session_id: &SessionId,
        acceptance: &meerkat_live::LiveRefreshQueueAcceptance,
    ) -> Result<LiveRefreshResultAuthority, RuntimeDriverError> {
        let channel_id = acceptance.channel_id().to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveRefreshQueued {
                    channel_id: channel_id.clone(),
                    queue_acceptance_sequence: acceptance.acceptance_sequence(),
                },
                "RecordLiveRefreshQueued",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveRefreshResultResolved {
                    channel_id: effect_channel_id,
                    status,
                    refresh_enqueued,
                    sequence,
                    queue_acceptance_sequence,
                } if *effect_channel_id == channel_id => Some(LiveRefreshResultAuthority {
                    status: *status,
                    refresh_enqueued: *refresh_enqueued,
                    sequence: *sequence,
                    queue_acceptance_sequence: *queue_acceptance_sequence,
                }),
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveRefreshQueued for channel '{channel_id}' emitted no LiveRefreshResultResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_close_result(
        &self,
        session_id: &SessionId,
        observation: &meerkat_live::LiveChannelCloseObservation,
    ) -> Result<LiveCloseResultAuthority, RuntimeDriverError> {
        let channel_id = observation.channel_id().to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveCloseClosed {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.clone(),
                    close_observation_sequence: observation.close_sequence(),
                },
                "RecordLiveCloseClosed",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        let authority = effects.as_slice().iter().find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveCloseResultResolved {
                channel_id: effect_channel_id,
                status,
                closed,
                sequence,
                close_observation_sequence,
            } if *effect_channel_id == channel_id
                && *close_observation_sequence == observation.close_sequence() =>
            {
                Some(LiveCloseResultAuthority::from_generated_effect(
                    channel_id.clone(),
                    *status,
                    *closed,
                    *sequence,
                    *close_observation_sequence,
                ))
            }
            _ => None,
        });
        match authority {
            Some(authority) => authority.map_err(RuntimeDriverError::Internal),
            None => Err(RuntimeDriverError::Internal(format!(
                "RecordLiveCloseClosed for channel '{channel_id}' emitted no LiveCloseResultResolved effect"
            ))),
        }
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_command_result(
        &self,
        session_id: &SessionId,
        acceptance: &meerkat_live::LiveCommandQueueAcceptance,
    ) -> Result<LiveCommandResultAuthority, RuntimeDriverError> {
        let channel_id = acceptance.channel_id().to_string();
        let command = dsl_live_command_kind(acceptance.kind());
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveCommandAccepted {
                    channel_id: channel_id.clone(),
                    command,
                    command_acceptance_sequence: acceptance.acceptance_sequence(),
                },
                "RecordLiveCommandAccepted",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveCommandResultResolved {
                    channel_id: effect_channel_id,
                    command: effect_command,
                    accepted,
                    sequence,
                    command_acceptance_sequence,
                } if *effect_channel_id == channel_id
                    && *effect_command == command
                    && *command_acceptance_sequence == acceptance.acceptance_sequence() =>
                {
                    Some(LiveCommandResultAuthority {
                        command: *effect_command,
                        accepted: *accepted,
                        sequence: *sequence,
                        command_acceptance_sequence: *command_acceptance_sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveCommandAccepted for channel '{channel_id}' emitted no LiveCommandResultResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_command_rejection_result(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        command: crate::meerkat_machine::dsl::LiveCommandPublicKind,
        error: &meerkat_live::LiveAdapterHostError,
    ) -> Result<LiveCommandRejectionAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let rejection = dsl_live_command_rejection_reason(error);
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveCommandRejected {
                    channel_id: channel_id.clone(),
                    command,
                    rejection,
                },
                "RecordLiveCommandRejected",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveCommandRejectionResolved {
                    channel_id: effect_channel_id,
                    command: effect_command,
                    rejection: effect_rejection,
                    public_error_class,
                    sequence,
                } if *effect_channel_id == channel_id
                    && *effect_command == command
                    && *effect_rejection == rejection =>
                {
                    Some(LiveCommandRejectionAuthority {
                        command: *effect_command,
                        rejection: *effect_rejection,
                        public_error_class: *public_error_class,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveCommandRejected for channel '{channel_id}' emitted no LiveCommandRejectionResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_unbound_live_command_rejection_result(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        command: crate::meerkat_machine::dsl::LiveCommandPublicKind,
    ) -> Result<LiveCommandRejectionAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let rejection = crate::meerkat_machine::dsl::LiveCommandRejectionReason::ChannelNotFound;
        let effects = apply_dsl_transition_on_authority(
            &self.live_unbound_rejection_authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveCommandRejected {
                channel_id: channel_id.clone(),
                command,
                rejection,
            },
            "RecordLiveCommandRejected:UnboundChannel",
        )
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveCommandRejectionResolved {
                    channel_id: effect_channel_id,
                    command: effect_command,
                    rejection: effect_rejection,
                    public_error_class,
                    sequence,
                } if *effect_channel_id == channel_id
                    && *effect_command == command
                    && *effect_rejection == rejection =>
                {
                    Some(LiveCommandRejectionAuthority {
                        command: *effect_command,
                        rejection: *effect_rejection,
                        public_error_class: *public_error_class,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveCommandRejected for unbound channel '{channel_id}' emitted no LiveCommandRejectionResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_channel_request_rejection_result(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        request: crate::meerkat_machine::dsl::LiveChannelRequestPublicKind,
        error: &meerkat_live::LiveAdapterHostError,
    ) -> Result<LiveChannelRequestRejectionAuthority, RuntimeDriverError> {
        self.resolve_live_channel_request_rejection_reason_result(
            session_id,
            channel_id,
            request,
            dsl_live_channel_request_rejection_reason(error),
        )
        .await
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_channel_request_rejection_reason_result(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        request: crate::meerkat_machine::dsl::LiveChannelRequestPublicKind,
        rejection: crate::meerkat_machine::dsl::LiveChannelRequestRejectionReason,
    ) -> Result<LiveChannelRequestRejectionAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveChannelRequestRejected {
                    channel_id: channel_id.clone(),
                    request,
                    rejection,
                },
                "RecordLiveChannelRequestRejected",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveChannelRequestRejectionResolved {
                    channel_id: effect_channel_id,
                    request: effect_request,
                    rejection: effect_rejection,
                    public_error_class,
                    sequence,
                } if *effect_channel_id == channel_id
                    && *effect_request == request
                    && *effect_rejection == rejection =>
                {
                    Some(LiveChannelRequestRejectionAuthority {
                        request: *effect_request,
                        rejection: *effect_rejection,
                        public_error_class: *public_error_class,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveChannelRequestRejected for channel '{channel_id}' emitted no LiveChannelRequestRejectionResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_unbound_live_channel_request_rejection_result(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        request: crate::meerkat_machine::dsl::LiveChannelRequestPublicKind,
    ) -> Result<LiveChannelRequestRejectionAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let rejection =
            crate::meerkat_machine::dsl::LiveChannelRequestRejectionReason::ChannelNotFound;
        let effects = apply_dsl_transition_on_authority(
            &self.live_unbound_rejection_authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveChannelRequestRejected {
                channel_id: channel_id.clone(),
                request,
                rejection,
            },
            "RecordLiveChannelRequestRejected:UnboundChannel",
        )
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveChannelRequestRejectionResolved {
                    channel_id: effect_channel_id,
                    request: effect_request,
                    rejection: effect_rejection,
                    public_error_class,
                    sequence,
                } if *effect_channel_id == channel_id
                    && *effect_request == request
                    && *effect_rejection == rejection =>
                {
                    Some(LiveChannelRequestRejectionAuthority {
                        request: *effect_request,
                        rejection: *effect_rejection,
                        public_error_class: *public_error_class,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveChannelRequestRejected for unbound channel '{channel_id}' emitted no LiveChannelRequestRejectionResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn record_live_webrtc_token_issued(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        token: &str,
        issued_at_ms: u64,
        ttl_ms: u64,
    ) -> Result<LiveWebrtcTokenAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let token = token.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveWebrtcTokenIssued {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.clone(),
                    token: token.clone(),
                    issued_at_ms,
                    ttl_ms,
                },
                "RecordLiveWebrtcTokenIssued",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveWebrtcTokenIssued {
                    session_id: effect_session_id,
                    channel_id: effect_channel_id,
                    token: effect_token,
                    expires_at_ms,
                    sequence,
                } if *effect_session_id == session_id.to_string()
                    && *effect_channel_id == channel_id
                    && *effect_token == token =>
                {
                    Some(LiveWebrtcTokenAuthority {
                        token: effect_token.clone(),
                        expires_at_ms: *expires_at_ms,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveWebrtcTokenIssued for channel '{channel_id}' emitted no LiveWebrtcTokenIssued effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_webrtc_answer_admission(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        token: &str,
        observed_at_ms: u64,
    ) -> Result<LiveWebrtcAnswerAdmissionAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let token = token.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveWebrtcAnswerAdmission {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.clone(),
                    token: token.clone(),
                    observed_at_ms,
                },
                "ResolveLiveWebrtcAnswerAdmission",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveWebrtcAnswerAdmissionResolved {
                    session_id: effect_session_id,
                    channel_id: effect_channel_id,
                    token: effect_token,
                    admitted,
                    rejection,
                    public_error_class,
                    sequence,
                } if *effect_session_id == session_id.to_string()
                    && *effect_channel_id == channel_id
                    && *effect_token == token =>
                {
                    Some(LiveWebrtcAnswerAdmissionAuthority {
                        admitted: *admitted,
                        rejection: *rejection,
                        public_error_class: *public_error_class,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "ResolveLiveWebrtcAnswerAdmission for channel '{channel_id}' emitted no LiveWebrtcAnswerAdmissionResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_webrtc_answer_result(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        answer_observation_sequence: u64,
    ) -> Result<LiveWebrtcAnswerResultAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveWebrtcAnswerAccepted {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.clone(),
                    answer_observation_sequence,
                },
                "RecordLiveWebrtcAnswerAccepted",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveWebrtcAnswerResultResolved {
                    channel_id: effect_channel_id,
                    status,
                    answered,
                    sequence,
                    answer_observation_sequence: effect_observation_sequence,
                } if *effect_channel_id == channel_id
                    && *effect_observation_sequence == answer_observation_sequence =>
                {
                    Some(LiveWebrtcAnswerResultAuthority {
                        status: *status,
                        answered: *answered,
                        sequence: *sequence,
                        answer_observation_sequence: *effect_observation_sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveWebrtcAnswerAccepted for channel '{channel_id}' emitted no LiveWebrtcAnswerResultResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn record_live_websocket_token_issued(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        token: &str,
        issued_at_ms: u64,
        ttl_ms: u64,
    ) -> Result<LiveWebsocketTokenAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let token = token.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveWebsocketTokenIssued {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.clone(),
                    token: token.clone(),
                    issued_at_ms,
                    ttl_ms,
                },
                "RecordLiveWebsocketTokenIssued",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveWebsocketTokenIssued {
                    session_id: effect_session_id,
                    channel_id: effect_channel_id,
                    token: effect_token,
                    expires_at_ms,
                    sequence,
                } if *effect_session_id == session_id.to_string()
                    && *effect_channel_id == channel_id
                    && *effect_token == token =>
                {
                    Some(LiveWebsocketTokenAuthority {
                        token: effect_token.clone(),
                        expires_at_ms: *expires_at_ms,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveWebsocketTokenIssued for channel '{channel_id}' emitted no LiveWebsocketTokenIssued effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_websocket_token_admission(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        token: &str,
        observed_at_ms: u64,
    ) -> Result<LiveWebsocketTokenAdmissionAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let token = token.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveWebsocketTokenAdmission {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.clone(),
                    token: token.clone(),
                    observed_at_ms,
                },
                "ResolveLiveWebsocketTokenAdmission",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        extract_live_websocket_token_admission(
            effects.as_slice(),
            &session_id.to_string(),
            &channel_id,
            &token,
            "ResolveLiveWebsocketTokenAdmission",
        )
    }

    #[cfg(feature = "live")]
    pub async fn resolve_unbound_live_websocket_token_admission(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        token: &str,
        observed_at_ms: u64,
    ) -> Result<LiveWebsocketTokenAdmissionAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let token = token.to_string();
        let effects = apply_dsl_transition_on_authority(
            &self.live_unbound_rejection_authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveWebsocketTokenAdmission {
                session_id: String::new(),
                channel_id: channel_id.clone(),
                token: token.clone(),
                observed_at_ms,
            },
            "ResolveLiveWebsocketTokenAdmission:UnboundChannel",
        )
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        extract_live_websocket_token_admission(
            effects.as_slice(),
            "",
            &channel_id,
            &token,
            "ResolveLiveWebsocketTokenAdmission:UnboundChannel",
        )
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_channel_status_result(
        &self,
        session_id: &SessionId,
        observation: &meerkat_live::LiveChannelStatusObservation,
    ) -> Result<LiveChannelStatusAuthority, RuntimeDriverError> {
        let channel_id = observation.channel_id().to_string();
        let (status, degradation_reason, degradation_detail) =
            dsl_live_channel_status_from_observation(observation.status());
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveChannelStatus {
                    channel_id: channel_id.clone(),
                    status,
                    status_observation_sequence: observation.observation_sequence(),
                    degradation_reason,
                    degradation_detail: degradation_detail.clone(),
                },
                "RecordLiveChannelStatus",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        let authority = effects.as_slice().iter().find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveChannelStatusResolved {
                channel_id: effect_channel_id,
                status,
                sequence,
                status_observation_sequence,
                degradation_reason,
                degradation_detail,
            } if *effect_channel_id == channel_id
                && *status_observation_sequence == observation.observation_sequence() =>
            {
                Some(LiveChannelStatusAuthority::from_generated_effect(
                    effect_channel_id.clone(),
                    *status,
                    *sequence,
                    *status_observation_sequence,
                    *degradation_reason,
                    degradation_detail.clone(),
                ))
            }
            _ => None,
        });
        match authority {
            Some(Ok(authority)) => Ok(authority),
            Some(Err(reason)) => Err(RuntimeDriverError::Internal(reason)),
            None => Err(RuntimeDriverError::Internal(format!(
                "RecordLiveChannelStatus for channel '{channel_id}' emitted no LiveChannelStatusResolved effect"
            ))),
        }
    }

    pub(super) async fn cancel_after_boundary_inner(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let (effect_tx, boundary_handle, projected_effect, previous_snapshot, committed_snapshot) = {
            let Some(_gate_guard) = self.lock_current_session_mutation_gate(session_id).await
            else {
                return Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                });
            };
            let staged = match self
                .stage_session_dsl_transition(
                    session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::CancelAfterBoundary {
                        reason: "boundary cancel".to_string(),
                    },
                    "CancelAfterBoundary",
                )
                .await
            {
                Ok(staged) => staged,
                Err(_) => {
                    return Err(RuntimeDriverError::NotReady {
                        state: self
                            .existing_session_runtime_state(session_id)
                            .await
                            .unwrap_or(RuntimeState::Destroyed),
                    });
                }
            };
            let projected_effect =
                crate::effect::runtime_effect_projection_from_dsl_effects(&staged.effects)
                    .map_err(RuntimeDriverError::Internal)?;

            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (
                entry.effect_sender(),
                entry.boundary_handle(),
                projected_effect,
                staged.previous_snapshot,
                staged.committed_snapshot,
            )
        };

        if let Err(err) = self
            .dispatch_cancel_after_boundary_runtime_effect(
                session_id,
                effect_tx,
                boundary_handle,
                projected_effect,
                "CancelAfterBoundary",
            )
            .await
        {
            self.restore_session_dsl_state_if_current(
                session_id,
                committed_snapshot,
                previous_snapshot,
            )
            .await;
            return Err(err);
        }

        Ok(())
    }

    /// Stop the attached runtime executor through the out-of-band control
    /// channel. When no loop is attached yet, a stop command is applied directly
    /// against the driver so queued work is still terminated consistently.
    pub async fn stop_runtime_executor(
        &self,
        session_id: &SessionId,
        reason: impl Into<String>,
    ) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::StopRuntimeExecutor {
                session_id: session_id.clone(),
                reason: reason.into(),
            },
        )
        .await
        .map_err(MeerkatMachine::driver_error_from_command_error)
        .map(|_| ())
    }

    pub(super) async fn stop_runtime_executor_inner(
        &self,
        session_id: &SessionId,
        reason: String,
    ) -> Result<(), RuntimeDriverError> {
        let (driver, effect_tx, effect) = {
            let Some(gate) = self.session_mutation_gate(session_id).await else {
                return Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                });
            };
            let gate_guard = Arc::clone(&gate).lock_owned().await;
            let staged = self
                .stage_session_dsl_transition(
                    session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::StopRuntimeExecutor {
                        reason,
                    },
                    "StopRuntimeExecutor",
                )
                .await
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
            let projected_effect =
                crate::effect::runtime_effect_projection_from_dsl_effects(&staged.effects)
                    .map_err(RuntimeDriverError::Internal)?;

            let (driver, effect_tx) = {
                let sessions = self.sessions.read().await;
                let entry = sessions
                    .get(session_id)
                    .ok_or(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    })?;
                if !Arc::ptr_eq(&entry.mutation_gate, &gate) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                (entry.driver.clone(), entry.effect_sender())
            };
            drop(gate_guard);
            (driver, effect_tx, projected_effect.into_effect())
        };

        if let Some(effect_tx) = effect_tx
            && effect_tx.send(effect).await.is_ok()
        {
            let stopped = tokio::time::timeout(std::time::Duration::from_millis(200), async {
                loop {
                    let state = {
                        let sessions = self.sessions.read().await;
                        let entry =
                            sessions
                                .get(session_id)
                                .ok_or(RuntimeDriverError::NotReady {
                                    state: RuntimeState::Destroyed,
                                })?;
                        if !Arc::ptr_eq(&entry.driver, &driver) {
                            return Err(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            });
                        }
                        entry.control_snapshot().phase
                    };
                    match state {
                        RuntimeState::Stopped => return Ok(()),
                        RuntimeState::Destroyed => {
                            return Err(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            });
                        }
                        _ => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
                    }
                }
            })
            .await;
            match stopped {
                Ok(result) => result?,
                Err(_) => {
                    let authority = self
                        .session_dsl_authority(session_id)
                        .await
                        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                    let generated_stop_state = authority
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .state()
                        .clone();
                    if generated_stop_state.runtime_stop_deferred
                        || generated_stop_state.lifecycle_phase
                            == crate::meerkat_machine::dsl::MeerkatPhase::Stopped
                    {
                        return Ok(());
                    }
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: "StopRuntimeExecutor effect was accepted but generated authority did not reach stopped"
                            .to_string(),
                    });
                }
            }

            let _gate_guard = self
                .lock_current_session_driver_gate(session_id, &driver)
                .await?;
            let final_state = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(session_id)
                    .ok_or(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    })?
                    .control_snapshot()
                    .phase
            };
            if !matches!(final_state, RuntimeState::Stopped) {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "StopRuntimeExecutor effect completed without generated stopped authority: {final_state}"
                    ),
                });
            }

            return Ok(());
        }

        let (driver, _gate_guard) = self
            .current_session_driver_with_authority(session_id)
            .await?;
        let completions = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?
                .completions
                .clone()
        };
        crate::control_plane::terminalize_async_stop(&driver, Some(&completions)).await?;

        // No live effect sender was available for this stop path. Scrub any
        // dead attachment capabilities that may still be published.
        self.clear_dead_runtime_attachment(session_id).await;
        Ok(())
    }

    /// Accept an input and execute it synchronously through the runtime driver.
    ///
    /// Used by surfaces that need the request/response shape while still
    /// preserving v9 input lifecycle semantics.
    pub async fn accept_input_and_run<T, F, Fut>(
        &self,
        session_id: &SessionId,
        input: Input,
        op: F,
    ) -> Result<T, RuntimeDriverError>
    where
        F: FnOnce(RunId, meerkat_core::lifecycle::run_primitive::RunPrimitive) -> Fut,
        Fut: Future<Output = Result<(T, CoreApplyOutput), RuntimeDriverError>>,
    {
        let MeerkatMachineRunPrepared {
            input_id,
            run_id,
            primitive,
        } = match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Prepare {
                    session_id: session_id.clone(),
                    input,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::Prepared(prepared) => prepared,
            other => {
                return Err(RuntimeDriverError::Internal(format!(
                    "unexpected command result preparing Meerkat run: {other:?}"
                )));
            }
        };

        match op(run_id.clone(), primitive).await {
            Ok((result, output)) => {
                self.execute_meerkat_machine_command(
                    None,
                    MeerkatMachineCommand::Commit {
                        session_id: session_id.clone(),
                        input_id,
                        run_id,
                        output,
                    },
                )
                .await
                .map_err(MeerkatMachine::driver_error_from_command_error)?;
                Ok(result)
            }
            Err(err) => {
                self.execute_meerkat_machine_command(
                    None,
                    MeerkatMachineCommand::Fail {
                        session_id: session_id.clone(),
                        run_id,
                        failure: MeerkatMachineRunFailure::new(err.to_string()),
                    },
                )
                .await
                .map_err(MeerkatMachine::driver_error_from_command_error)?;
                Err(err)
            }
        }
    }

    /// Accept an input and return a completion handle that resolves when the
    /// input reaches a terminal state (Consumed or Abandoned).
    ///
    /// Returns `(AcceptOutcome, Option<CompletionHandle>)`:
    /// - `(Accepted, Some(handle))` — await handle for result
    /// - `(Accepted, None)` — input reached a terminal state during admission
    /// - `(Deduplicated, Some(handle))` — joined in-flight waiter
    /// - `(Deduplicated, None)` — input already terminal; no waiter needed
    /// - `(Rejected, _)` — returned as `Err(ValidationFailed)`
    pub async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError>
    {
        self.accept_input_with_completion_boxed(session_id, input)
            .await
    }

    pub fn accept_input_with_completion_boxed<'a>(
        &'a self,
        session_id: &'a SessionId,
        input: Input,
    ) -> AcceptInputWithCompletionFuture<'a> {
        let input_id = input.id().clone();
        self.accept_boxed_input_with_completion(session_id, Box::new(input), input_id)
    }

    pub fn accept_boxed_input_with_completion<'a>(
        &'a self,
        session_id: &'a SessionId,
        input: Box<Input>,
        _input_id: InputId,
    ) -> AcceptInputWithCompletionFuture<'a> {
        let session_id = session_id.clone();
        Box::pin(async move {
            let input = *input;
            match self
                .execute_meerkat_machine_ingress_command(
                    MeerkatMachineCommand::AcceptWithCompletion {
                        session_id: session_id.clone(),
                        input,
                        register_completion: true,
                    },
                )
                .await?
            {
                MeerkatMachineCommandResult::AcceptWithCompletion {
                    outcome,
                    handle,
                    admission_signal: _,
                } => Ok((outcome, handle)),
                other => Err(RuntimeDriverError::Internal(format!(
                    "unexpected command result for accept_input_with_completion: {other:?}"
                ))),
            }
        })
    }

    /// Accept an input but intentionally do not wake the runtime loop.
    ///
    /// This is reserved for explicitly queued-only surface contracts that
    /// stage work for the next turn boundary instead of waking an idle session
    /// immediately.
    pub async fn accept_input_without_wake(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::AcceptWithoutWake {
                    session_id: session_id.clone(),
                    input,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::AcceptOutcome(outcome) => Ok(outcome),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected command result for accept_input_without_wake: {other:?}"
            ))),
        }
    }

    /// Get the shared ops lifecycle registry for a session/runtime instance.
    pub async fn ops_lifecycle_registry(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::OpsLifecycleRegistry {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::OpsLifecycleRegistry(registry)) => registry,
            Ok(_) => {
                tracing::error!("ops_lifecycle_registry: unexpected command result variant");
                None
            }
            Err(_) => None,
        }
    }

    /// Prepare canonical runtime bindings for a session.
    ///
    /// This is the single canonical helper that replaces the hand-rolled
    /// `register_session()` + `ops_lifecycle_registry()` + manual threading
    /// dance. All runtime-backed surfaces should call this instead.
    ///
    /// The method is idempotent: if the session is already registered, it
    /// returns bindings from the existing entry. The epoch_id is stable
    /// across repeated calls for the same session.
    pub async fn prepare_bindings(
        &self,
        session_id: SessionId,
    ) -> Result<meerkat_core::SessionRuntimeBindings, RuntimeBindingsError> {
        match Box::pin(self.prepare_session_runtime_bindings(
            session_id.clone(),
            super::dispatch_session::SessionBindingPreparation::AuthoritativeRuntimeBinding,
        ))
        .await
        {
            Ok(MeerkatMachineCommandResult::Bindings(bindings)) => Ok(bindings),
            Ok(_) => {
                tracing::error!("prepare_bindings: unexpected command result variant");
                Err(RuntimeBindingsError::SessionNotFound(session_id))
            }
            Err(err) => Err(RuntimeBindingsError::PrepareFailed(
                session_id,
                err.to_string(),
            )),
        }
    }

    /// Prepare factory-consumable session runtime resources without emitting
    /// cross-machine binding signals.
    ///
    /// Mob provisioning uses this to pre-create the session-owned handle bundle
    /// before `MobMachine::Spawn` has committed the member runtime id. The
    /// authoritative mob binding is routed later through
    /// `RequestRuntimeBinding -> PrepareBindings`, which emits the typed
    /// `RuntimeBound` signal with the mob-owned `AgentRuntimeId` and fence.
    pub async fn prepare_local_session_bindings(
        &self,
        session_id: SessionId,
    ) -> Result<meerkat_core::SessionRuntimeBindings, RuntimeBindingsError> {
        match Box::pin(self.prepare_session_runtime_bindings(
            session_id.clone(),
            super::dispatch_session::SessionBindingPreparation::LocalSessionResources,
        ))
        .await
        {
            Ok(MeerkatMachineCommandResult::Bindings(bindings)) => Ok(bindings),
            Ok(_) => {
                tracing::error!(
                    "prepare_local_session_bindings: unexpected command result variant"
                );
                Err(RuntimeBindingsError::SessionNotFound(session_id))
            }
            Err(_) => Err(RuntimeBindingsError::SessionNotFound(session_id)),
        }
    }
}
