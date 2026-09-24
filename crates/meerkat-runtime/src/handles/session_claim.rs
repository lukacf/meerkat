//! Runtime-side session-id claim registry.
//!
//! Owned by [`crate::meerkat_machine::MeerkatMachine`] (one registry per
//! `MeerkatMachine` instance). Every live runtime-registered session that
//! also wires comms goes through this handle to reserve its session-id
//! identity, so the machine instance is the canonical owner of "this
//! session id is currently active" — no process-global statics in the
//! comms shell (dogma #2).
//!
//! The impl lives in [`meerkat_core::handles::DefaultSessionClaimRegistry`];
//! the runtime just re-exports it under a runtime-flavored name so
//! downstream surfaces do not need to know the trait's default concrete
//! type. `DefaultSessionClaimRegistry::global()` remains the process-scope
//! fallback for bare `AgentFactory` callers without a runtime.

pub use meerkat_core::handles::DefaultSessionClaimRegistry as RuntimeSessionClaimRegistry;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use meerkat_core::SessionId;
    use meerkat_core::handles::{SessionClaimError, SessionClaimHandle};

    use super::RuntimeSessionClaimRegistry;

    #[test]
    fn second_live_acquire_for_same_session_fails() {
        let registry: Arc<dyn SessionClaimHandle> = Arc::new(RuntimeSessionClaimRegistry::new());
        let sid = SessionId::new();
        let _claim = registry.clone().try_acquire(&sid).expect("first acquire");
        let err = registry.try_acquire(&sid).expect_err("second must fail");
        assert!(matches!(err, SessionClaimError::SessionIdentityInUse(_)));
    }

    #[test]
    fn drop_releases_claim() {
        let registry: Arc<dyn SessionClaimHandle> = Arc::new(RuntimeSessionClaimRegistry::new());
        let sid = SessionId::new();
        {
            let _claim = registry.clone().try_acquire(&sid).expect("first acquire");
        }
        let _claim2 = registry
            .try_acquire(&sid)
            .expect("acquire after drop should succeed");
    }
}
