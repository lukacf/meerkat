//! Session checkpointing trait for periodic persistence.
//!
//! Host-mode agents run indefinitely, so the normal post-turn persistence path
//! (`PersistentSessionService::persist_full_session`) never fires after the
//! initial `create_session`. This trait allows the agent to checkpoint the
//! session after each host-mode interaction.

use crate::session::Session;
use async_trait::async_trait;

/// Periodic session persistence hook.
///
/// Implementations clone what they need from the borrowed `Session`.
/// Errors are logged, not propagated — a checkpoint failure should not
/// kill the agent loop.
#[async_trait]
pub trait SessionCheckpointer: Send + Sync {
    /// Save a snapshot of the current session state.
    async fn checkpoint(&self, session: &Session);
}
