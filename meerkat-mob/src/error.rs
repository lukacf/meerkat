//! Typed error hierarchy for mob operations.
//!
//! [`MobError`] is the canonical error type returned by all [`MobService`](crate::service::MobService)
//! methods and store operations.

use crate::run::RunStatus;
use crate::validate::Diagnostic;

/// Error type for mob operations.
///
/// All [`MobService`](crate::service::MobService) methods return
/// `Result<T, MobError>`. Variants cover spec management, run lifecycle,
/// dispatch, collection, namespace, resolver, session, comms, and store
/// errors.
#[derive(Debug, thiserror::Error)]
pub enum MobError {
    /// The requested mob spec was not found.
    #[error("mob spec not found: {mob_id}")]
    SpecNotFound {
        /// Mob ID that was not found.
        mob_id: String,
    },

    /// The mob spec failed validation.
    #[error("mob spec validation failed: {count} diagnostic(s)", count = diagnostics.len())]
    SpecValidation {
        /// Validation diagnostics.
        diagnostics: Vec<Diagnostic>,
    },

    /// Optimistic concurrency conflict on spec update.
    #[error("spec conflict: expected revision {expected}, actual {actual}")]
    SpecConflict {
        /// Expected generation/revision.
        expected: u64,
        /// Actual generation/revision.
        actual: u64,
    },

    /// The requested run was not found.
    #[error("run not found: {run_id}")]
    RunNotFound {
        /// Run ID that was not found.
        run_id: String,
    },

    /// Invalid run status transition.
    #[error("invalid transition: {from:?} -> {to:?}")]
    InvalidTransition {
        /// Current status.
        from: RunStatus,
        /// Attempted target status.
        to: RunStatus,
    },

    /// Step dispatch failed.
    #[error("dispatch failed for step {step_id}: {reason}")]
    DispatchFailed {
        /// Step that failed to dispatch.
        step_id: String,
        /// Failure reason.
        reason: String,
    },

    /// Collection timed out before the required number of responses arrived.
    #[error("collection timeout for step {step_id}: collected {collected}/{expected}")]
    CollectionTimeout {
        /// Step that timed out.
        step_id: String,
        /// Number of responses collected.
        collected: usize,
        /// Number of responses expected.
        expected: usize,
    },

    /// Namespace configuration error.
    #[error("namespace error: {0}")]
    NamespaceError(String),

    /// Resolver error.
    #[error("resolver error: {0}")]
    ResolverError(String),

    /// Wrapped session service error.
    #[error("session error: {0}")]
    SessionError(#[from] meerkat_core::service::SessionError),

    /// Wrapped comms send error.
    #[error("comms error: {0}")]
    CommsError(#[from] meerkat_core::comms::SendError),

    /// Store operation error.
    #[error("store error: {0}")]
    StoreError(String),

    /// Internal/unexpected error.
    #[error("internal error: {0}")]
    Internal(String),
}
