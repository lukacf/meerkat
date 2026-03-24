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
}
