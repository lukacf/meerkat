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
        self.dispatch_user_interrupt(session_id, reason.into())
            .await
    }

    // `user_interrupt` is mounted as a private child of `dispatch_session`, so
    // this live-authority helper is visible only to the private admitted
    // interrupt path and not to peer-admission siblings.
    pub(super) async fn apply_user_interrupt_live_cancel(
        &self,
        session_id: &SessionId,
        reason: String,
    ) -> Result<(), RuntimeDriverError> {
        let authority = UserInterruptAuthority::new();
        self.hard_cancel_current_run_authorized(session_id, reason, authority)
            .await
    }

    async fn hard_cancel_current_run_authorized(
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

struct UserInterruptAuthority(());

impl UserInterruptAuthority {
    fn new() -> Self {
        Self(())
    }
}
