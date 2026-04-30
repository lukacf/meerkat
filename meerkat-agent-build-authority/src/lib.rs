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
    pub const fn new_for_agent_factory() -> Self {
        Self { _private: () }
    }
}
