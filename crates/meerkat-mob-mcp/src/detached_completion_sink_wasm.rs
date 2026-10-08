//! The browser runtime has no durable continuation owner, so a detached
//! outcome can never be submitted: no sink exists, and fork_off and council
//! run in the turn (`NoContinuationOwner`).

use meerkat_core::SessionId;
use meerkat_core::event::BackgroundJobTerminalStatus;

use crate::detached_delivery::{
    DetachedCompletionDelivered, DetachedCompletionError, DetachedCompletionOwner,
};

/// Uninhabited: no value of it exists on this target.
#[derive(Debug, Clone)]
pub(crate) enum DetachedCompletionSink {}

impl DetachedCompletionSink {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn submit(
        &self,
        _owner: &DetachedCompletionOwner,
        _owner_session_id: &SessionId,
        _tool: &'static str,
        _job_id: &str,
        _status: BackgroundJobTerminalStatus,
        _outcome: &serde_json::Value,
        _result_digest: &str,
    ) -> Result<DetachedCompletionDelivered, DetachedCompletionError> {
        match *self {}
    }
}
