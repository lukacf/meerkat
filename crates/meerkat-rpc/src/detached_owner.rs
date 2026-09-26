//! Plain-session owner revival for detached mob jobs.
//!
//! A top-level RPC session that calls `council` owns the council's detached
//! completion. The runtime retires an idle session's executor routinely, so
//! by the time the council ends the session may not be live. This host makes
//! it live again through the same executor attachment the session's own next
//! turn takes, so the completion is admitted and wakes it once (see
//! [`meerkat_mob_mcp::DetachedOwnerHost`]).

use std::sync::{Arc, Weak};

use meerkat_core::types::SessionId;
use meerkat_mob_mcp::{DetachedOwnerError, DetachedOwnerHost};

use crate::session_runtime::SessionRuntime;

/// [`DetachedOwnerHost`] for the sessions an RPC [`SessionRuntime`] serves.
///
/// Holds the runtime weakly: the runtime owns the mob state this host is
/// installed on.
pub struct RpcDetachedOwnerHost {
    runtime: Weak<SessionRuntime>,
}

impl RpcDetachedOwnerHost {
    pub fn new(runtime: &Arc<SessionRuntime>) -> Self {
        Self {
            runtime: Arc::downgrade(runtime),
        }
    }
}

#[async_trait::async_trait]
impl DetachedOwnerHost for RpcDetachedOwnerHost {
    async fn ensure_owner_live(&self, session_id: &SessionId) -> Result<(), DetachedOwnerError> {
        let runtime = self
            .runtime
            .upgrade()
            .ok_or_else(|| DetachedOwnerError::Failed {
                detail: "the RPC session runtime has shut down".to_string(),
            })?;
        runtime
            .ensure_runtime_executor(session_id)
            .await
            .map_err(|error| {
                if error.code == crate::error::SESSION_NOT_FOUND {
                    DetachedOwnerError::OwnerGone {
                        detail: error.message,
                    }
                } else {
                    DetachedOwnerError::Failed {
                        detail: error.message,
                    }
                }
            })
    }
}
