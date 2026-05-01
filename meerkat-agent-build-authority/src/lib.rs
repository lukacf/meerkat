//! Internal typed authority for factory-owned agent construction.
//!
//! This crate exists so `meerkat-core` can require a concrete capability type
//! without re-exporting a minting API from `meerkat_core::agent`. The facade
//! factory owns private source markers and stamps a source-coupled witness;
//! core accepts only authority values carrying that witness.

use std::any::TypeId;

/// Capability proving that the caller is entering core construction from a
/// factory-owned policy path.
#[derive(Debug, Clone, Copy)]
pub struct AgentFactoryBuildAuthority {
    #[allow(dead_code)]
    guard_type: TypeId,
    #[allow(dead_code)]
    source_type: TypeId,
    #[allow(dead_code)]
    witness_type: TypeId,
}

impl AgentFactoryBuildAuthority {
    /// Validate that this value was minted for the linked facade factory marker
    /// types.
    #[doc(hidden)]
    pub fn is_canonical_factory_authority(&self) -> bool {
        private::is_canonical_factory_authority(self)
    }
}

mod private {
    use super::AgentFactoryBuildAuthority;

    pub(super) fn is_canonical_factory_authority(authority: &AgentFactoryBuildAuthority) -> bool {
        authority.guard_type != authority.source_type
            && authority.witness_type == authority.source_type
    }
}

#[cfg(test)]
mod tests {
    use super::AgentFactoryBuildAuthority;
    use std::any::TypeId;

    struct TestFactoryAuthorityGuard;
    struct TestFactoryAuthoritySource;
    struct ForgedAuthorityGuard;
    struct ForgedAuthoritySource;
    fn authority_from_source<G: 'static, T: 'static>() -> AgentFactoryBuildAuthority {
        AgentFactoryBuildAuthority {
            guard_type: TypeId::of::<G>(),
            source_type: TypeId::of::<T>(),
            witness_type: TypeId::of::<T>(),
        }
    }

    fn authority_from_parts<G: 'static, T: 'static, W: 'static>() -> AgentFactoryBuildAuthority {
        AgentFactoryBuildAuthority {
            guard_type: TypeId::of::<G>(),
            source_type: TypeId::of::<T>(),
            witness_type: TypeId::of::<W>(),
        }
    }

    #[test]
    fn registered_factory_source_validates() {
        let authority =
            authority_from_source::<TestFactoryAuthorityGuard, TestFactoryAuthoritySource>();

        assert!(authority.is_canonical_factory_authority());
    }

    #[test]
    fn facade_stamped_authority_uses_local_witness_for_validation() {
        let authority = authority_from_source::<TestFactoryAuthorityGuard, ForgedAuthoritySource>();

        assert!(authority.is_canonical_factory_authority());
    }

    #[test]
    fn facade_stamped_authority_does_not_delegate_guard_to_linker_symbol() {
        let authority = authority_from_source::<ForgedAuthorityGuard, TestFactoryAuthoritySource>();

        assert!(authority.is_canonical_factory_authority());
    }

    #[test]
    fn mismatched_factory_witness_is_rejected() {
        let authority = authority_from_parts::<
            TestFactoryAuthorityGuard,
            TestFactoryAuthoritySource,
            ForgedAuthoritySource,
        >();

        assert!(!authority.is_canonical_factory_authority());
    }
}
