use meerkat_core::types::SessionId;

use crate::meerkat_machine::MeerkatMachine;
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
}

pub(crate) struct UserInterruptAuthority(());

impl UserInterruptAuthority {
    pub(super) fn new() -> Self {
        Self(())
    }
}
