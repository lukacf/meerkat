//! Persisted-session recovery helpers.
//!
//! Populated by W1-D (`RecoveredCreateRequest`,
//! `RecoveryRuntimeBindingMode`) and W2-B (`load_persisted_session`,
//! `recovered_create_request_*`, `recovery_overrides_from_turn`,
//! `cleanup_recovered_runtime_if_new`).

use meerkat_core::service::CreateSessionRequest;

/// Result of `recovered_create_request*`: a [`CreateSessionRequest`]
/// reconstructed from a persisted session, plus a flag indicating
/// whether the recovery process registered a fresh runtime binding for
/// the session (so callers can roll back the registration on
/// pre-run-apply failure via `cleanup_recovered_runtime_if_new`).
#[derive(Debug)]
pub struct RecoveredCreateRequest {
    /// Reconstructed create request that surfaces feed back into the
    /// session service.
    pub request: CreateSessionRequest,
    /// Whether the recovery flow registered a *new* runtime binding for
    /// this session (i.e. it was not already live in the runtime).
    pub runtime_was_registered: bool,
}

/// How recovery should bind the persisted session into the runtime.
///
/// - `Authoritative` — register the recovered session with full machine
///   authority (default; used when the operator explicitly resumes).
/// - `LocalResources` — bind only local-resource projections, leaving
///   canonical authority to a peer (used when the recovered session is
///   observed during a peer-driven flow rather than a user-initiated
///   resume).
#[derive(Clone, Copy, Debug)]
pub enum RecoveryRuntimeBindingMode {
    /// Full authority: this process owns the session machine.
    Authoritative,
    /// Local-resource binding only; another node owns the machine.
    LocalResources,
}
