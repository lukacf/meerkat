use crate::MobError;
use crate::store::SupervisorAuthorityRecord;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{CommsCommand, InputStreamMode, PeerName, SendReceipt, TrustedPeerSpec};
use meerkat_core::interaction::{InteractionContent, PeerInputCandidate};
use meerkat_core::types::HandlingMode;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
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
        Ok(Arc::new(runtime))
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

    pub(crate) async fn supervisor_spec(&self) -> Result<TrustedPeerSpec, MobError> {
        let authority = self.authority().await;
        TrustedPeerSpec::new(
            self.participant_name.clone(),
            authority.public_peer_id,
            format!("inproc://{}", self.participant_name),
        )
        .map_err(|error| MobError::WiringError(format!("invalid supervisor spec: {error}")))
    }

    /// Send a typed bridge command and return the raw JSON response.
    pub(crate) async fn send_bridge_command(
        &self,
        recipient: &TrustedPeerSpec,
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

    pub(crate) async fn request_json<T: serde::Serialize>(
        &self,
        recipient: &TrustedPeerSpec,
        intent: &str,
        payload: &T,
        timeout: Duration,
    ) -> Result<serde_json::Value, MobError> {
        let _request_guard = self.request_lock.lock().await;
        let runtime = self.runtime().await;
        runtime.add_trusted_peer(recipient.clone()).await?;
        let to = PeerName::new(recipient.name.clone()).map_err(|error| {
            MobError::WiringError(format!(
                "invalid supervisor recipient name '{}': {error}",
                recipient.name
            ))
        })?;
        let params = serde_json::to_value(payload).map_err(|error| {
            MobError::Internal(format!("serialize supervisor payload: {error}"))
        })?;
        let receipt = runtime
            .send(CommsCommand::PeerRequest {
                to,
                intent: intent.to_string(),
                params,
                handling_mode: HandlingMode::Queue,
                stream: InputStreamMode::None,
            })
            .await?;
        let SendReceipt::PeerRequestSent { envelope_id, .. } = receipt else {
            return Err(MobError::Internal(
                "unexpected receipt for supervisor request".to_string(),
            ));
        };
        self.wait_for_response(&runtime, envelope_id, timeout).await
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

        let started = std::time::Instant::now();
        loop {
            let drained = runtime.drain_peer_input_candidates().await;
            if let Some(result) = self
                .buffer_and_extract(drained, request_envelope_id)
                .await?
            {
                return Ok(result);
            }

            let remaining = timeout.saturating_sub(started.elapsed());
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

    fn response_value(
        candidate: &PeerInputCandidate,
        request_envelope_id: uuid::Uuid,
    ) -> Result<Option<serde_json::Value>, MobError> {
        let InteractionContent::Response {
            in_reply_to,
            status,
            result,
        } = &candidate.interaction.content
        else {
            return Ok(None);
        };

        if in_reply_to.0 != request_envelope_id {
            return Ok(None);
        }

        match status {
            meerkat_core::interaction::ResponseStatus::Completed
            | meerkat_core::interaction::ResponseStatus::Failed => Ok(Some(result.clone())),
            meerkat_core::interaction::ResponseStatus::Accepted => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use meerkat_core::PeerInputClass;
    use meerkat_core::interaction::{InboxInteraction, InteractionId, ResponseStatus};

    fn response_candidate(
        request_envelope_id: uuid::Uuid,
        status: ResponseStatus,
        result: serde_json::Value,
    ) -> PeerInputCandidate {
        PeerInputCandidate {
            interaction: InboxInteraction {
                id: InteractionId(uuid::Uuid::new_v4()),
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
            class: PeerInputClass::Response,
            lifecycle_peer: None,
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
}
