use std::sync::Arc;

use meerkat_core::lifecycle::CoreExecutorInterruptHandle;
use meerkat_core::types::SessionId;

use crate::meerkat_machine::MeerkatMachine;
use crate::runtime_state::RuntimeState;
use crate::traits::RuntimeDriverError;

impl MeerkatMachine {
    pub async fn hard_cancel_current_run(
        &self,
        session_id: &SessionId,
        reason: impl Into<String>,
    ) -> Result<(), RuntimeDriverError> {
        self.interrupt_current_run_with_reason(session_id, reason.into())
            .await
    }

    pub(crate) async fn hard_cancel_current_run_authorized(
        &self,
        session_id: &SessionId,
        reason: String,
        _authority: UserInterruptAuthority,
    ) -> Result<(), RuntimeDriverError> {
        let handle = self.interrupt_handle_for(session_id).await?;
        handle.hard_cancel_current_run(reason).await.map_err(|err| {
            RuntimeDriverError::Internal(format!("failed to hard cancel run: {err}"))
        })
    }

    async fn interrupt_handle_for(
        &self,
        session_id: &SessionId,
    ) -> Result<Arc<dyn CoreExecutorInterruptHandle>, RuntimeDriverError> {
        let handle = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            entry.interrupt_handle()
        };

        let Some(handle) = handle else {
            let state = self
                .existing_session_runtime_state(session_id)
                .await
                .unwrap_or(RuntimeState::Destroyed);
            return Err(RuntimeDriverError::NotReady { state });
        };

        Ok(handle)
    }
}

pub(crate) struct UserInterruptAuthority(());

impl UserInterruptAuthority {
    pub(super) fn new() -> Self {
        Self(())
    }
}
