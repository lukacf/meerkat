//! Concrete RuntimeInputSink implementation.
//!
//! Routes host-mode comms interactions through the RuntimeSessionAdapter
//! instead of calling Agent::run() directly.

use std::sync::Arc;

use meerkat_core::agent::RuntimeInputSink;
use meerkat_core::interaction::InboxInteraction;
use meerkat_core::types::SessionId;

use crate::comms_bridge::interaction_to_peer_input;
use crate::identifiers::LogicalRuntimeId;
use crate::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, SystemGeneratedInput,
};
use crate::service_ext::SessionServiceRuntimeExt as _;
use crate::session_adapter::RuntimeSessionAdapter;

/// Routes host-mode comms interactions through the runtime adapter.
///
/// Awaits only admission (durable-before-ack), NOT execution completion —
/// the host loop continues immediately after the input is accepted.
pub struct RuntimeCommsInputSink {
    adapter: Arc<RuntimeSessionAdapter>,
    session_id: SessionId,
    runtime_id: LogicalRuntimeId,
}

impl RuntimeCommsInputSink {
    pub fn new(adapter: Arc<RuntimeSessionAdapter>, session_id: SessionId) -> Self {
        let runtime_id = LogicalRuntimeId::new(session_id.to_string());
        Self {
            adapter,
            session_id,
            runtime_id,
        }
    }
}

#[async_trait::async_trait]
impl RuntimeInputSink for RuntimeCommsInputSink {
    async fn accept_peer_input(&self, interaction: InboxInteraction) -> Result<(), String> {
        let input = interaction_to_peer_input(&interaction, &self.runtime_id);
        self.adapter
            .accept_input(&self.session_id, input)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn accept_continuation(&self) -> Result<(), String> {
        let input = Input::SystemGenerated(SystemGeneratedInput {
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: InputOrigin::System,
                durability: InputDurability::Ephemeral,
                visibility: InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            generator: "comms_host_continuation".into(),
            content: "continuation after terminal response injection".into(),
        });
        self.adapter
            .accept_input(&self.session_id, input)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}
