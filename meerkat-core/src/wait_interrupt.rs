//! Shared wait-interrupt types for the agent tool dispatcher.
//!
//! Moved to `meerkat-core` so the `AgentToolDispatcher` trait can reference
//! them without a reverse dependency on `meerkat-tools`.

use std::sync::Arc;

/// Interrupt signal for the wait tool.
#[derive(Debug, Clone)]
pub struct WaitInterrupt {
    /// Human-readable reason for the interrupt.
    pub reason: String,
}

/// Receiver alias, cfg-gated for wasm32 compatibility.
#[cfg(not(target_arch = "wasm32"))]
pub type WaitInterruptReceiver = tokio::sync::watch::Receiver<Option<WaitInterrupt>>;
#[cfg(target_arch = "wasm32")]
pub type WaitInterruptReceiver =
    tokio_with_wasm::alias::sync::watch::Receiver<Option<WaitInterrupt>>;

/// Result of [`AgentToolDispatcher::bind_wait_interrupt`].
///
/// On success, returns the rebound dispatcher. On failure, returns the
/// original dispatcher unchanged alongside the error kind, so forwarding
/// dispatchers (e.g. `ToolGateway`) can keep unsupported entries intact.
pub type WaitInterruptBindResult =
    Result<Arc<dyn crate::agent::AgentToolDispatcher>, WaitInterruptBindError>;

/// Error from [`AgentToolDispatcher::bind_wait_interrupt`].
#[derive(Debug, thiserror::Error)]
pub enum WaitInterruptBindError {
    /// The dispatcher does not support wait interrupt binding.
    #[error("dispatcher does not support wait interrupt binding")]
    Unsupported,
    /// The dispatcher Arc has multiple owners, cannot unwrap for rebinding.
    #[error("dispatcher Arc has multiple owners, cannot unwrap for rebinding")]
    SharedOwnership,
}
