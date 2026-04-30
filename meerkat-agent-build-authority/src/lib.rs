//! Internal typed authority for factory-owned agent construction.
//!
//! This crate exists so `meerkat-core` can require a concrete capability type
//! without re-exporting a minting API from `meerkat_core::agent`. The facade
//! factory depends on this crate directly and owns the value passed across the
//! core build seam.

/// Capability proving that the caller is entering core construction from a
/// factory-owned policy path.
#[derive(Debug, Clone, Copy)]
pub struct AgentFactoryBuildAuthority {
    _private: (),
}

impl AgentFactoryBuildAuthority {
    /// Mint the authority for the canonical facade factory.
    ///
    /// This constructor intentionally lives outside `meerkat-core`; depending
    /// on `meerkat` must not feature-unify a safe minting API into the core
    /// public surface.
    ///
    /// # Safety
    ///
    /// Callers must only mint this authority from code paths that have already
    /// composed canonical `AgentFactory` policy metadata. The value is the
    /// internal proof passed to `meerkat-core`'s factory-policy finalizer; using
    /// it outside that factory-owned path bypasses the policy boundary.
    #[allow(unsafe_code)]
    pub const unsafe fn new_for_agent_factory() -> Self {
        Self { _private: () }
    }
}
