//! The wasm32 form of the realm admission guard.
//!
//! On native targets [`AdmissionGuard`] is the proof that an operation holds
//! its realm admission fences (`realm_admission.rs`). A wasm32 build has no
//! realm filesystem and no native build, so there is nothing to fence: this
//! form exists only so that target-neutral code (the token-store and
//! credential-coordinator signatures) has one signature on every target. Its
//! one constructor is [`AdmissionGuard::none`], a guard over no fence, which
//! only stores with no fence (in memory, ephemeral) accept.
//!
//! Keep this file to exactly the names target-neutral code uses, each with
//! the native signature; the native form is the contract.

use std::sync::Arc;

/// Proof that the current operation holds every fence of its set; on wasm32
/// there are none.
///
/// It is not `Clone`: an operation shares it by `Arc` and passes it down as
/// `&AdmissionGuard`, as on native targets.
#[derive(Debug)]
pub struct AdmissionGuard {
    _no_fences: (),
}

impl AdmissionGuard {
    /// A guard over no fence. Only stores with no fence (in memory,
    /// ephemeral), which never ask whether a guard covers anything, accept
    /// it; it covers no fence.
    pub fn none() -> Arc<Self> {
        Arc::new(Self { _no_fences: () })
    }
}
