//! Internal typed authority for factory-owned agent construction.
//!
//! This crate exists so `meerkat-core` can require a concrete capability type
//! without re-exporting a minting API from `meerkat_core::agent`. The facade
//! factory depends on this crate directly and owns the value passed across the
//! core build seam.

use std::num::NonZeroUsize;

/// Capability proving that the caller is entering core construction from a
/// factory-owned policy path.
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct AgentFactoryBuildAuthority {
    seal: NonZeroUsize,
}

impl AgentFactoryBuildAuthority {
    /// Validate that this value was minted by the internal authority bridge.
    #[doc(hidden)]
    pub fn is_canonical_factory_authority(&self) -> bool {
        self.seal == private::canonical_factory_seal()
    }
}

mod private {
    use std::num::NonZeroUsize;

    pub const CANONICAL_FACTORY_SEAL_VALUE: usize = 0x6d_6b_74_21;

    #[allow(unsafe_code)]
    pub(super) const fn canonical_factory_seal() -> NonZeroUsize {
        // SAFETY: the canonical factory seal constant is non-zero by
        // construction.
        unsafe { NonZeroUsize::new_unchecked(CANONICAL_FACTORY_SEAL_VALUE) }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentFactoryBuildAuthority, private};
    use std::num::NonZeroUsize;

    #[allow(unsafe_code)]
    fn authority_from_seal(seal: NonZeroUsize) -> AgentFactoryBuildAuthority {
        // SAFETY: `AgentFactoryBuildAuthority` is a transparent wrapper around
        // the non-zero seal value.
        unsafe { std::mem::transmute::<NonZeroUsize, AgentFactoryBuildAuthority>(seal) }
    }

    #[test]
    fn canonical_seal_validates() {
        let authority = authority_from_seal(private::canonical_factory_seal());

        assert!(authority.is_canonical_factory_authority());
    }

    #[test]
    fn non_canonical_seal_is_rejected() {
        let authority = authority_from_seal(NonZeroUsize::MIN);

        assert!(!authority.is_canonical_factory_authority());
    }
}
