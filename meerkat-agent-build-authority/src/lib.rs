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
    seal: &'static private::Seal,
}

impl AgentFactoryBuildAuthority {
    /// Validate that this value was minted by the internal authority bridge.
    #[doc(hidden)]
    pub fn is_canonical_factory_authority(&self) -> bool {
        std::ptr::eq(std::ptr::from_ref(self.seal), &raw const private::SEAL)
    }
}

#[allow(unsafe_code)]
mod private {
    #[derive(Debug)]
    pub struct Seal;

    pub static SEAL: Seal = Seal;

    #[unsafe(export_name = "__meerkat_agent_factory_build_authority_new")]
    pub extern "Rust" fn mint_agent_factory_build_authority() -> super::AgentFactoryBuildAuthority {
        super::AgentFactoryBuildAuthority { seal: &SEAL }
    }
}
