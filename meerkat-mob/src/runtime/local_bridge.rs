//! `LocalMobRuntimeBridge` — local implementation of `MobMemberRuntimeBridge`
//! that forwards to `MeerkatMachine` for in-process mob members.

use crate::error::MobError;
use crate::runtime::bridge::MobMemberRuntimeBridge;
use crate::runtime::bridge_protocol::{
    BridgeAck, BridgeBindResponse, BridgeDeliveryOutcome, BridgeDeliveryResponse,
    BridgeDestroyResponse, BridgeMemberRuntimeState, BridgeObservationResponse,
    BridgePeerConnectivity, BridgePeerSpec, BridgeRetireResponse,
};
use async_trait::async_trait;
use meerkat_core::types::{ContentInput, HandlingMode, SessionId};
use meerkat_runtime::MeerkatMachine;
#[allow(unused_imports)]
use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
use std::sync::Arc;

/// Local bridge implementation that forwards to an in-process `MeerkatMachine`.
pub struct LocalMobRuntimeBridge {
    machine: Arc<MeerkatMachine>,
    session_id: SessionId,
}

impl LocalMobRuntimeBridge {
    pub fn new(machine: Arc<MeerkatMachine>, session_id: SessionId) -> Self {
        Self {
            machine,
            session_id,
        }
    }
}

fn runtime_state_to_bridge(state: meerkat_runtime::RuntimeState) -> BridgeMemberRuntimeState {
    match state {
        meerkat_runtime::RuntimeState::Initializing => BridgeMemberRuntimeState::Initializing,
        meerkat_runtime::RuntimeState::Idle => BridgeMemberRuntimeState::Idle,
        meerkat_runtime::RuntimeState::Attached => BridgeMemberRuntimeState::Attached,
        meerkat_runtime::RuntimeState::Running => BridgeMemberRuntimeState::Running,
        meerkat_runtime::RuntimeState::Retired => BridgeMemberRuntimeState::Retired,
        meerkat_runtime::RuntimeState::Stopped => BridgeMemberRuntimeState::Stopped,
        meerkat_runtime::RuntimeState::Destroyed => BridgeMemberRuntimeState::Destroyed,
        _ => BridgeMemberRuntimeState::Idle,
    }
}

#[async_trait]
impl MobMemberRuntimeBridge for LocalMobRuntimeBridge {
    async fn bind_member(
        &self,
        _expected_peer_id: &str,
        _expected_address: &str,
    ) -> Result<BridgeBindResponse, MobError> {
        // Local members don't need binding — they're already in-process.
        Err(MobError::Internal(
            "bind_member is not supported for local members".to_string(),
        ))
    }

    async fn authorize_supervisor(&self) -> Result<BridgeAck, MobError> {
        // Local members don't need supervisor authorization.
        Ok(BridgeAck { ok: true })
    }

    async fn revoke_supervisor(&self) -> Result<BridgeAck, MobError> {
        // Local members don't need supervisor revocation.
        Ok(BridgeAck { ok: true })
    }

    async fn deliver_member_input(
        &self,
        input_id: &str,
        content: ContentInput,
        handling_mode: HandlingMode,
    ) -> Result<BridgeDeliveryResponse, MobError> {
        use meerkat_runtime::input::{
            Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention,
            PeerInput,
        };

        let (body, blocks) = match content {
            ContentInput::Text(body) => (body, None),
            ContentInput::Blocks(blocks) => {
                let body = meerkat_core::types::text_content(&blocks);
                (body, Some(blocks))
            }
        };

        let input = Input::Peer(PeerInput {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: InputOrigin::Operator,
                durability: InputDurability::Durable,
                visibility: InputVisibility::default(),
                idempotency_key: Some(meerkat_runtime::identifiers::IdempotencyKey::new(
                    input_id.to_string(),
                )),
                supersession_key: None,
                correlation_id: None,
            },
            convention: Some(PeerConvention::Message),
            body,
            blocks,
            handling_mode: match handling_mode {
                HandlingMode::Queue => None,
                mode => Some(mode),
            },
        });

        match self
            .machine
            .accept_input_without_wake(&self.session_id, input)
            .await
        {
            Ok(outcome) => {
                let response = match outcome {
                    meerkat_runtime::AcceptOutcome::Accepted { input_id: id, .. } => {
                        BridgeDeliveryResponse {
                            input_id: input_id.to_string(),
                            canonical_input_id: Some(id.to_string()),
                            outcome: BridgeDeliveryOutcome::Accepted,
                        }
                    }
                    meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. } => {
                        let existing_id = existing_id.to_string();
                        BridgeDeliveryResponse {
                            input_id: input_id.to_string(),
                            canonical_input_id: Some(existing_id.clone()),
                            outcome: BridgeDeliveryOutcome::Deduplicated {
                                existing_input_id: existing_id,
                            },
                        }
                    }
                    meerkat_runtime::AcceptOutcome::Rejected { reason } => BridgeDeliveryResponse {
                        input_id: input_id.to_string(),
                        canonical_input_id: None,
                        outcome: BridgeDeliveryOutcome::Rejected {
                            reason: reason.to_string(),
                        },
                    },
                    _ => BridgeDeliveryResponse {
                        input_id: input_id.to_string(),
                        canonical_input_id: None,
                        outcome: BridgeDeliveryOutcome::Rejected {
                            reason: "unexpected accept outcome".to_string(),
                        },
                    },
                };
                Ok(response)
            }
            Err(error) => Err(MobError::Internal(format!(
                "local deliver_member_input failed: {error}"
            ))),
        }
    }

    async fn observe_member(&self) -> Result<BridgeObservationResponse, MobError> {
        use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;

        let state = self
            .machine
            .runtime_state(&self.session_id)
            .await
            .map_err(|error| MobError::Internal(format!("observe_member failed: {error}")))?;

        let current_run_id = self
            .machine
            .meerkat_machine_spine_snapshot(&self.session_id)
            .await
            .and_then(|snapshot| {
                snapshot
                    .control
                    .current_run_id
                    .map(|run_id| run_id.to_string())
            });

        Ok(BridgeObservationResponse {
            phase: runtime_state_to_bridge(state),
            state: runtime_state_to_bridge(state),
            accepting_inputs: Some(state.can_accept_input()),
            current_run: current_run_id.clone(),
            current_run_id,
            peer_connectivity: Some(BridgePeerConnectivity::Reachable),
            last_error: None,
            observed_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    async fn interrupt_member(&self) -> Result<BridgeAck, MobError> {
        self.machine
            .interrupt_current_run(&self.session_id)
            .await
            .map_err(|error| {
                MobError::Internal(format!("local interrupt_member failed: {error}"))
            })?;
        Ok(BridgeAck { ok: true })
    }

    async fn retire_member(&self) -> Result<BridgeRetireResponse, MobError> {
        let report = self
            .machine
            .retire_runtime(&self.session_id)
            .await
            .map_err(|error| MobError::Internal(format!("local retire_member failed: {error}")))?;
        Ok(BridgeRetireResponse {
            inputs_abandoned: report.inputs_abandoned,
            inputs_pending_drain: report.inputs_pending_drain,
        })
    }

    async fn destroy_member(&self) -> Result<BridgeDestroyResponse, MobError> {
        use meerkat_runtime::identifiers::LogicalRuntimeId;
        use meerkat_runtime::traits::RuntimeControlPlane;

        let runtime_id = LogicalRuntimeId::new(self.session_id.to_string());
        let report = RuntimeControlPlane::destroy(self.machine.as_ref(), &runtime_id)
            .await
            .map_err(|error| MobError::Internal(format!("local destroy_member failed: {error}")))?;
        Ok(BridgeDestroyResponse {
            inputs_abandoned: report.inputs_abandoned,
        })
    }

    async fn wire_member(&self, _peer_spec: BridgePeerSpec) -> Result<BridgeAck, MobError> {
        // Local wiring is handled directly through comms runtime, not through
        // the bridge. This is a no-op for local members.
        Ok(BridgeAck { ok: true })
    }

    async fn unwire_member(&self, _peer_spec: BridgePeerSpec) -> Result<BridgeAck, MobError> {
        // Local unwiring is handled directly through comms runtime.
        Ok(BridgeAck { ok: true })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_bridge_observe_returns_idle_for_registered_session() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        machine.register_session(session_id.clone()).await;

        let bridge = LocalMobRuntimeBridge::new(machine, session_id);
        let observation = bridge.observe_member().await.unwrap();

        assert_eq!(observation.state, BridgeMemberRuntimeState::Idle);
        assert!(observation.current_run_id.is_none());
    }

    #[tokio::test]
    async fn local_bridge_retire_returns_report() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        machine.register_session(session_id.clone()).await;

        let bridge = LocalMobRuntimeBridge::new(machine, session_id);
        let report = bridge.retire_member().await.unwrap();

        assert_eq!(report.inputs_abandoned, 0);
        assert_eq!(report.inputs_pending_drain, 0);
    }

    #[tokio::test]
    async fn local_bridge_bind_is_unsupported() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();

        let bridge = LocalMobRuntimeBridge::new(machine, session_id);
        let result = bridge.bind_member("peer-1", "inproc://peer-1").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn local_bridge_authorize_is_noop() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();

        let bridge = LocalMobRuntimeBridge::new(machine, session_id);
        let ack = bridge.authorize_supervisor().await.unwrap();

        assert!(ack.ok);
    }
}
