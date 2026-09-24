//! Agent comms helpers.

use crate::agent::{Agent, AgentLlmClient, AgentSessionStore, AgentToolDispatcher, CommsRuntime};

impl<C, T, S> Agent<C, T, S>
where
    C: AgentLlmClient + ?Sized + 'static,
    T: AgentToolDispatcher + ?Sized + 'static,
    S: AgentSessionStore + ?Sized + 'static,
{
    /// Get the comms runtime, if enabled.
    pub fn comms(&self) -> Option<&dyn CommsRuntime> {
        self.comms_runtime.as_deref()
    }

    /// Get a shared handle to the comms runtime, if enabled.
    pub fn comms_arc(&self) -> Option<std::sync::Arc<dyn CommsRuntime>> {
        self.comms_runtime.clone()
    }

    pub fn observed_comms_sender(&self) -> Option<std::sync::Arc<super::ObservedCommsSender>> {
        Some(std::sync::Arc::new(super::ObservedCommsSender::new(
            self.comms_runtime.clone()?,
            std::sync::Arc::clone(&self.post_commit_hooks),
        )))
    }

    /// Send through the configured comms runtime and project a successful peer
    /// delivery onto the observe-only post-commit hook surface.
    pub async fn send_comms(
        &self,
        command: crate::comms::CommsCommand,
    ) -> Result<crate::comms::SendReceipt, crate::comms::SendError> {
        let sender = self.observed_comms_sender().ok_or_else(|| {
            crate::comms::SendError::Unsupported("comms runtime is not configured".to_string())
        })?;
        sender.send(command).await
    }
}
