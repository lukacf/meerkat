//! Maintenance-mode session-service support.
//!
//! Offline maintenance verbs (`rkat storage migrate`) drive the persistent
//! service's machine-owned storage machinery — bulk legacy-checkpoint
//! adoption in particular — without ever building an agent. The
//! [`MaintenanceAgentBuilder`] satisfies the service's builder seam by
//! refusing construction typed, which keeps maintenance compositions honest:
//! any code path that would materialize an agent under the maintenance
//! fence is a bug and surfaces as one.

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::mpsc;
use async_trait::async_trait;
use meerkat_core::event::AgentEvent;
use meerkat_core::service::{CreateSessionRequest, SessionError};
use meerkat_core::types::RunResult;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;

use crate::ephemeral::{SessionAgent, SessionAgentBuilder};

/// Uninhabited session agent: the builder refuses before a value can exist,
/// so every trait method is statically unreachable (`match *self {}`).
pub enum RefusingSessionAgent {}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionAgent for RefusingSessionAgent {
    async fn run_with_events(
        &mut self,
        _prompt: meerkat_core::types::ContentInput,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<RunResult, meerkat_core::error::AgentError> {
        match *self {}
    }

    fn set_skill_references(&mut self, _refs: Option<Vec<meerkat_core::skills::SkillKey>>) {
        match *self {}
    }

    fn set_turn_tool_overlay(
        &mut self,
        _overlay: Option<meerkat_core::service::TurnToolOverlay>,
    ) -> Result<(), meerkat_core::error::AgentError> {
        match *self {}
    }

    fn hot_swap_llm_identity(
        &mut self,
        _client: std::sync::Arc<dyn meerkat_core::AgentLlmClient>,
        _identity: meerkat_core::SessionLlmIdentity,
        _request_policy: meerkat_core::SessionLlmRequestPolicy,
    ) -> Result<(), meerkat_core::error::AgentError> {
        match *self {}
    }

    fn cancel(&mut self) {
        match *self {}
    }

    fn session_id(&self) -> meerkat_core::SessionId {
        match *self {}
    }

    fn snapshot(&self) -> crate::ephemeral::SessionSnapshot {
        match *self {}
    }

    fn session_clone(
        &self,
    ) -> Result<meerkat_core::Session, meerkat_core::SystemContextStateError> {
        match *self {}
    }

    fn observed_session_tail(&self) -> crate::ephemeral::ObservedSessionTailKind {
        match *self {}
    }

    fn apply_runtime_system_context(
        &mut self,
        _appends: &[meerkat_core::PendingSystemContextAppend],
    ) {
        match *self {}
    }

    fn system_context_state(&self) -> meerkat_core::SystemContextStateHandle {
        match *self {}
    }
}

/// Builder for maintenance compositions: refuses to build agents, typed.
#[derive(Debug, Clone, Copy, Default)]
pub struct MaintenanceAgentBuilder;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl SessionAgentBuilder for MaintenanceAgentBuilder {
    type Agent = RefusingSessionAgent;

    async fn build_agent(
        &self,
        _req: &CreateSessionRequest,
        _event_tx: mpsc::Sender<AgentEvent>,
    ) -> Result<Self::Agent, SessionError> {
        Err(SessionError::Unsupported(
            "maintenance session service builds no agents".to_string(),
        ))
    }
}
