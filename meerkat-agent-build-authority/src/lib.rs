//! Retired typed authority marker for factory-owned agent construction.
//!
//! Earlier revisions used a public concrete authority value as the facade/core
//! bridge. That shape is intentionally inert: core no longer accepts this
//! token, so a downstream direct dependency cannot transmute one into the
//! factory-policy finalizer.

/// Retired marker retained only so stale downstream code fails at the core
/// finalizer signature rather than resolving a minting API.
#[derive(Debug, Clone, Copy)]
pub struct AgentFactoryBuildAuthority {
    _private: (),
}
