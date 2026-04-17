//! Runtime impl of [`meerkat_core::handles::AuthLeaseHandle`] (Phase 1.5-rev).
//!
//! Routes the auth-lease trait methods to the corresponding DSL inputs on a
//! shared per-session [`MeerkatMachineAuthority`]. Snapshots read the DSL state
//! back out through the same authority.

use std::sync::Arc;

use meerkat_core::handles::{AuthLeaseHandle, AuthLeaseSnapshot, DslTransitionError};

use super::HandleDslAuthority;
use crate::meerkat_machine::dsl as mm_dsl;

/// Runtime-backed [`AuthLeaseHandle`] impl.
#[derive(Debug)]
pub struct RuntimeAuthLeaseHandle {
    dsl: Arc<HandleDslAuthority>,
}

impl RuntimeAuthLeaseHandle {
    /// Construct a handle backed by the session's shared DSL authority.
    pub fn new(dsl: Arc<HandleDslAuthority>) -> Self {
        Self { dsl }
    }

    /// Construct a handle backed by an ephemeral DSL authority.
    pub fn ephemeral() -> Self {
        Self::new(Arc::new(HandleDslAuthority::ephemeral()))
    }
}

impl AuthLeaseHandle for RuntimeAuthLeaseHandle {
    fn acquire_lease(&self, binding_key: &str, expires_at: u64) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::AcquireAuthLease {
                binding_key: binding_key.to_string(),
                expires_at,
            },
            "AuthLeaseHandle::acquire_lease",
        )
    }

    fn mark_expiring(&self, binding_key: &str) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::MarkAuthExpiring {
                binding_key: binding_key.to_string(),
            },
            "AuthLeaseHandle::mark_expiring",
        )
    }

    fn begin_refresh(&self, binding_key: &str) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::BeginAuthRefresh {
                binding_key: binding_key.to_string(),
            },
            "AuthLeaseHandle::begin_refresh",
        )
    }

    fn complete_refresh(
        &self,
        binding_key: &str,
        new_expires_at: u64,
        now: u64,
    ) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::CompleteAuthRefresh {
                binding_key: binding_key.to_string(),
                new_expires_at,
                now,
            },
            "AuthLeaseHandle::complete_refresh",
        )
    }

    fn refresh_failed(&self, binding_key: &str, permanent: bool) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::AuthRefreshFailed {
                binding_key: binding_key.to_string(),
                permanent,
            },
            "AuthLeaseHandle::refresh_failed",
        )
    }

    fn mark_reauth_required(&self, binding_key: &str) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::MarkReauthRequired {
                binding_key: binding_key.to_string(),
            },
            "AuthLeaseHandle::mark_reauth_required",
        )
    }

    fn release_lease(&self, binding_key: &str) -> Result<(), DslTransitionError> {
        self.dsl.apply_input(
            mm_dsl::MeerkatMachineInput::ReleaseAuthLease {
                binding_key: binding_key.to_string(),
            },
            "AuthLeaseHandle::release_lease",
        )
    }

    fn snapshot(&self, binding_key: &str) -> AuthLeaseSnapshot {
        let state = self.dsl.snapshot_state();
        let key = binding_key.to_string();
        let state_str = if state.auth_valid_leases.contains(&key) {
            Some("valid".to_string())
        } else if state.auth_expiring_leases.contains(&key) {
            Some("expiring".to_string())
        } else if state.auth_refreshing_leases.contains(&key) {
            Some("refreshing".to_string())
        } else if state.auth_reauth_required_leases.contains(&key) {
            Some("reauth_required".to_string())
        } else {
            None
        };
        let expires_at = state.auth_expires_at.get(&key).copied();
        AuthLeaseSnapshot {
            state: state_str,
            expires_at,
        }
    }
}
