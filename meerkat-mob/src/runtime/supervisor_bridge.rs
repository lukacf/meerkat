use crate::MobError;
use crate::store::SupervisorAuthorityRecord;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, InputStreamMode, PeerId, PeerRoute, SendReceipt, TrustedPeerDescriptor,
};
use meerkat_core::interaction::{InteractionContent, PeerInputCandidate, TerminalityClass};
use meerkat_core::time_compat::{Duration, Instant};
use meerkat_core::types::HandlingMode;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

pub(crate) struct MobSupervisorBridge {
    participant_name: String,
    authority: RwLock<SupervisorAuthorityRecord>,
    runtime: RwLock<Arc<meerkat_comms::CommsRuntime>>,
    buffered_candidates: Mutex<VecDeque<PeerInputCandidate>>,
    /// Serializes bridge command requests so that at most one
    /// request/response round-trip is in flight on this supervisor bridge
    /// at any time. Held for the full duration of `send_bridge_command`
    /// from `add_trusted_peer` through reply receipt, which keeps the
    /// comms `PeerResponse` correlation free of concurrent interleaving
    /// and makes rotation / bind fallbacks deterministic.
    request_lock: Mutex<()>,
}

impl MobSupervisorBridge {
    pub(crate) fn new(
        mob_id: &crate::MobId,
        authority: SupervisorAuthorityRecord,
    ) -> Result<Self, MobError> {
        let participant_name = format!("{mob_id}/__mob_supervisor__");
        let runtime = Self::build_runtime(&participant_name, &authority)?;
        Ok(Self {
            participant_name,
            authority: RwLock::new(authority),
            runtime: RwLock::new(runtime),
            buffered_candidates: Mutex::new(VecDeque::new()),
            request_lock: Mutex::new(()),
        })
    }

    fn build_runtime(
        participant_name: &str,
        authority: &SupervisorAuthorityRecord,
    ) -> Result<Arc<meerkat_comms::CommsRuntime>, MobError> {
        let runtime = meerkat_comms::CommsRuntime::inproc_only_with_keypair_and_silent_intents(
            participant_name,
            None,
            authority.keypair(),
            Arc::new(HashSet::new()),
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "failed to construct mob supervisor comms runtime '{participant_name}': {error}"
            ))
        })?;
        let runtime = Arc::new(runtime);
        Self::install_supervisor_peer_request_response_authority(&runtime, participant_name)?;
        Ok(runtime)
    }

    /// Supervisor bridge runtimes intentionally issue semantic peer
    /// request/response commands outside a session surface. Give them an
    /// explicit ephemeral machine authority so comms never owns that lifecycle.
    fn install_supervisor_peer_request_response_authority(
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        participant_name: &str,
    ) -> Result<(), MobError> {
        let dsl = Arc::new(meerkat_runtime::HandleDslAuthority::ephemeral());
        dsl.apply_signal(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
            "mob_supervisor_bridge::initialize",
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "failed to initialize supervisor bridge authority '{participant_name}': {error}"
            ))
        })?;
        dsl.apply_input(
            meerkat_runtime::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                session_id: meerkat_runtime::meerkat_machine::dsl::SessionId::from(
                    participant_name.to_string(),
                ),
            },
            "mob_supervisor_bridge::register",
        )
        .map_err(|error| {
            MobError::Internal(format!(
                "failed to register supervisor bridge authority '{participant_name}': {error}"
            ))
        })?;

        runtime.install_peer_request_response_authority(
            meerkat_comms::PeerRequestResponseAuthority::new(
                Arc::new(meerkat_runtime::RuntimePeerInteractionHandle::new(
                    Arc::clone(&dsl),
                )),
                Arc::new(meerkat_runtime::RuntimeInteractionStreamHandle::new(dsl)),
            ),
        );
        Ok(())
    }

    pub(crate) async fn rotate(
        &self,
        authority: SupervisorAuthorityRecord,
    ) -> Result<(), MobError> {
        let runtime = Self::build_runtime(&self.participant_name, &authority)?;
        *self.authority.write().await = authority;
        *self.runtime.write().await = runtime;
        self.buffered_candidates.lock().await.clear();
        Ok(())
    }

    pub(crate) async fn authority(&self) -> SupervisorAuthorityRecord {
        self.authority.read().await.clone()
    }

    pub(crate) async fn runtime(&self) -> Arc<meerkat_comms::CommsRuntime> {
        self.runtime.read().await.clone()
    }

    pub(crate) async fn runtime_core(&self) -> Arc<dyn CoreCommsRuntime> {
        self.runtime().await as Arc<dyn CoreCommsRuntime>
    }

    pub(crate) async fn supervisor_spec(&self) -> Result<TrustedPeerDescriptor, MobError> {
        let authority = self.authority().await;
        let public_key = authority.keypair().public_key();
        TrustedPeerDescriptor::unsigned_with_pubkey(
            self.participant_name.clone(),
            authority.public_peer_id,
            *public_key.as_bytes(),
            format!("inproc://{}", self.participant_name),
        )
        .map_err(|error| MobError::WiringError(format!("invalid supervisor spec: {error}")))
    }

    /// Send a typed bridge command and return the raw JSON response.
    pub(crate) async fn send_bridge_command(
        &self,
        recipient: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        self.request_json(
            recipient,
            super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
            command,
            timeout,
        )
        .await
    }

    pub(crate) async fn send_bridge_command_as_authority(
        &self,
        authority: &SupervisorAuthorityRecord,
        recipient: &TrustedPeerDescriptor,
        command: &super::bridge_protocol::BridgeCommand,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        let _request_guard = self.request_lock.lock().await;
        let probe_participant_name = format!(
            "{}/pending-supervisor-probe/{}",
            self.participant_name,
            uuid::Uuid::new_v4()
        );
        let runtime = Self::build_runtime(&probe_participant_name, authority)?;
        runtime
            .add_trusted_peer(recipient.clone())
            .await
            .map_err(MobError::from)?;
        self.request_json_with_runtime(
            &runtime,
            recipient,
            super::bridge_protocol::SUPERVISOR_BRIDGE_INTENT,
            command,
            timeout,
        )
        .await
    }

    pub(crate) async fn trust_recipient(
        &self,
        recipient: &TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        let runtime = self.runtime().await;
        runtime
            .add_trusted_peer(recipient.clone())
            .await
            .map_err(MobError::from)
    }

    pub(crate) async fn request_json<T: serde::Serialize>(
        &self,
        recipient: &TrustedPeerDescriptor,
        intent: &str,
        payload: &T,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        let _request_guard = self.request_lock.lock().await;
        let runtime = self.runtime().await;
        self.request_json_with_runtime(&runtime, recipient, intent, payload, timeout)
            .await
    }

    async fn request_json_with_runtime<T: serde::Serialize>(
        &self,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        recipient: &TrustedPeerDescriptor,
        intent: &str,
        payload: &T,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        let to = PeerRoute::with_display_name(recipient.peer_id, recipient.name.clone());
        let params = serde_json::to_value(payload).map_err(|error| {
            MobError::Internal(format!("serialize supervisor payload: {error}"))
        })?;
        let receipt = runtime
            .send(CommsCommand::PeerRequest {
                to,
                intent: intent.to_string(),
                params,
                blocks: None,
                handling_mode: HandlingMode::Queue,
                stream: InputStreamMode::None,
            })
            .await?;
        let SendReceipt::PeerRequestSent { envelope_id, .. } = receipt else {
            return Err(MobError::Internal(
                "unexpected receipt for supervisor request".to_string(),
            ));
        };
        self.wait_for_response(runtime, envelope_id, timeout).await
    }

    async fn wait_for_response(
        &self,
        runtime: &Arc<meerkat_comms::CommsRuntime>,
        request_envelope_id: uuid::Uuid,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        if let Some(result) = self.take_buffered_response(request_envelope_id).await? {
            return Ok(result);
        }

        let deadline = BridgeRequestDeadline::start(timeout);
        loop {
            let drained = runtime.drain_peer_input_candidates().await;
            if let Some(result) = self
                .buffer_and_extract(drained, request_envelope_id)
                .await?
            {
                return Ok(result);
            }

            let remaining = deadline.remaining();
            if remaining.is_zero() {
                return Err(MobError::Internal(format!(
                    "supervisor request '{request_envelope_id}' timed out after {}ms",
                    timeout.as_millis()
                )));
            }

            if tokio::time::timeout(remaining, runtime.inbox_notify().notified())
                .await
                .is_err()
            {
                return Err(MobError::Internal(format!(
                    "supervisor request '{request_envelope_id}' timed out after {}ms",
                    timeout.as_millis()
                )));
            }
        }
    }

    async fn take_buffered_response(
        &self,
        request_envelope_id: uuid::Uuid,
    ) -> Result<Option<serde_json::Value>, MobError> {
        let mut buffered = self.buffered_candidates.lock().await;
        let mut retained = VecDeque::new();
        let mut matched = None;

        while let Some(candidate) = buffered.pop_front() {
            match Self::response_value(&candidate, request_envelope_id)? {
                Some(value) if matched.is_none() => matched = Some(value),
                _ => retained.push_back(candidate),
            }
        }

        *buffered = retained;
        Ok(matched)
    }

    async fn buffer_and_extract(
        &self,
        drained: Vec<PeerInputCandidate>,
        request_envelope_id: uuid::Uuid,
    ) -> Result<Option<serde_json::Value>, MobError> {
        let mut buffered = self.buffered_candidates.lock().await;
        let mut matched = None;

        for candidate in drained {
            match Self::response_value(&candidate, request_envelope_id)? {
                Some(value) if matched.is_none() => matched = Some(value),
                _ => buffered.push_back(candidate),
            }
        }

        Ok(matched)
    }

    /// Extract a terminal response value for the awaited request, if any.
    ///
    /// Bridge code does not interpret `ResponseStatus` directly. The
    /// ingress classifier stamps response terminality onto the candidate, and
    /// the bridge consumes that typed fact.
    fn response_value(
        candidate: &PeerInputCandidate,
        request_envelope_id: uuid::Uuid,
    ) -> Result<Option<serde_json::Value>, MobError> {
        let InteractionContent::Response {
            in_reply_to,
            status: _,
            result,
        } = &candidate.interaction.content
        else {
            return Ok(None);
        };

        if in_reply_to.0 != request_envelope_id {
            return Ok(None);
        }

        match candidate.response_terminality {
            Some(TerminalityClass::Terminal { .. }) => Ok(Some(result.clone())),
            Some(TerminalityClass::Progress) | None => Ok(None),
            _ => Ok(None),
        }
    }
}

#[derive(Debug)]
struct BridgeRequestDeadline {
    started: Instant,
    timeout: Duration,
}

impl BridgeRequestDeadline {
    fn start(timeout: Duration) -> Self {
        Self {
            started: Instant::now(),
            timeout,
        }
    }

    fn remaining(&self) -> Duration {
        Self::remaining_after(self.timeout, self.started.elapsed())
    }

    fn remaining_after(timeout: Duration, elapsed: Duration) -> Duration {
        timeout.saturating_sub(elapsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use meerkat_core::PeerIngressMachinePolicy;
    use meerkat_core::interaction::{InboxInteraction, InteractionId, ResponseStatus};

    fn response_candidate(
        request_envelope_id: uuid::Uuid,
        status: ResponseStatus,
        result: serde_json::Value,
    ) -> PeerInputCandidate {
        let id = InteractionId(uuid::Uuid::new_v4());
        let worker_peer_id = PeerId::from_uuid(
            uuid::Uuid::parse_str("018f6f79-7a82-7c4e-a552-a3b86f9630f5")
                .expect("valid worker route id"),
        );
        let classification = PeerIngressMachinePolicy::default().classify_response(status);
        PeerInputCandidate {
            interaction: InboxInteraction {
                id,
                from_route: Some(worker_peer_id),
                from: "worker-rt".to_string(),
                content: InteractionContent::Response {
                    in_reply_to: InteractionId(request_envelope_id),
                    status,
                    result,
                },
                rendered_text: "[worker-rt]: response".to_string(),
                handling_mode: HandlingMode::Queue,
                render_metadata: None,
            },
            ingress: meerkat_core::PeerIngressFact::peer(
                id,
                classification.class,
                meerkat_core::PeerIngressKind::Response,
                Some(meerkat_core::PeerIngressAuthDecision::Required),
                meerkat_core::PeerIngressIdentity::new(
                    worker_peer_id,
                    "worker-rt",
                    meerkat_core::PeerIngressConvention::Response {
                        in_reply_to: InteractionId(request_envelope_id),
                        status,
                    },
                ),
            ),
            lifecycle_peer: None,
            response_terminality: classification.response_terminality,
        }
    }

    #[test]
    fn response_value_completed_returns_payload() {
        let request_envelope_id = uuid::Uuid::new_v4();
        let expected = serde_json::json!({"ok": true});
        let candidate = response_candidate(
            request_envelope_id,
            ResponseStatus::Completed,
            expected.clone(),
        );

        let value = MobSupervisorBridge::response_value(&candidate, request_envelope_id)
            .expect("completed response should parse");

        assert_eq!(value, Some(expected));
    }

    #[test]
    fn response_value_accepted_is_not_terminal() {
        let request_envelope_id = uuid::Uuid::new_v4();
        let candidate = response_candidate(
            request_envelope_id,
            ResponseStatus::Accepted,
            serde_json::json!({"phase": "queued"}),
        );

        let value = MobSupervisorBridge::response_value(&candidate, request_envelope_id)
            .expect("accepted response should parse");

        assert!(
            value.is_none(),
            "Accepted is progress and must not satisfy wait_for_response"
        );
    }

    #[test]
    fn response_value_failed_returns_payload() {
        let request_envelope_id = uuid::Uuid::new_v4();
        let expected = serde_json::json!({"error": "boom"});
        let candidate = response_candidate(
            request_envelope_id,
            ResponseStatus::Failed,
            expected.clone(),
        );

        let value = MobSupervisorBridge::response_value(&candidate, request_envelope_id)
            .expect("failed response should parse");

        assert_eq!(
            value,
            Some(expected),
            "Failed is terminal and must surface the result payload"
        );
    }

    #[test]
    fn response_value_routes_all_statuses_through_canonical_classifier() {
        use meerkat_core::interaction::{TerminalityClass, classify_response_terminality};

        for status in [
            ResponseStatus::Accepted,
            ResponseStatus::Completed,
            ResponseStatus::Failed,
        ] {
            let request_envelope_id = uuid::Uuid::new_v4();
            let payload = serde_json::json!({"status": format!("{status:?}")});
            let candidate = response_candidate(request_envelope_id, status, payload.clone());

            let value = MobSupervisorBridge::response_value(&candidate, request_envelope_id)
                .expect("status-response should parse");

            match classify_response_terminality(status) {
                TerminalityClass::Terminal { .. } => assert_eq!(
                    value,
                    Some(payload),
                    "terminal classification must surface payload (status={status:?})"
                ),
                TerminalityClass::Progress => assert!(
                    value.is_none(),
                    "progress classification must not surface payload (status={status:?})"
                ),
                _ => panic!(
                    "TerminalityClass variant grew; update supervisor_bridge::response_value \
                     and this test before landing the new class"
                ),
            }
        }
    }

    #[test]
    fn response_value_for_unrelated_envelope_returns_none() {
        let awaited = uuid::Uuid::new_v4();
        let other = uuid::Uuid::new_v4();
        assert_ne!(awaited, other);
        let candidate = response_candidate(
            other,
            ResponseStatus::Completed,
            serde_json::json!({"ok": true}),
        );

        let value = MobSupervisorBridge::response_value(&candidate, awaited)
            .expect("mismatched envelope should still parse");

        assert!(
            value.is_none(),
            "response for a different envelope must not satisfy the current wait"
        );
    }

    #[test]
    fn bridge_request_deadline_saturates_elapsed_timeout() {
        assert_eq!(
            BridgeRequestDeadline::remaining_after(
                Duration::from_millis(25),
                Duration::from_millis(30)
            ),
            Duration::ZERO
        );
        assert_eq!(
            BridgeRequestDeadline::remaining_after(
                Duration::from_millis(25),
                Duration::from_millis(10)
            ),
            Duration::from_millis(15)
        );
    }
}
