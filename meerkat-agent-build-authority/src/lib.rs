//! Internal typed authority for factory-owned agent construction.
//!
//! This crate exists so `meerkat-core` can require a concrete capability type
//! without re-exporting a minting API from `meerkat_core::agent`. The facade
//! factory owns private source markers and a validator for those markers; core
//! accepts only authority values that the linked facade validator recognizes.

use std::any::TypeId;

/// Capability proving that the caller is entering core construction from a
/// factory-owned policy path.
#[derive(Debug, Clone, Copy)]
pub struct AgentFactoryBuildAuthority {
    #[allow(dead_code)]
    guard_type: TypeId,
    #[allow(dead_code)]
    source_type: TypeId,
}

impl AgentFactoryBuildAuthority {
    /// Validate that this value was minted for the linked facade factory marker
    /// types.
    #[doc(hidden)]
    pub fn is_canonical_factory_authority(&self) -> bool {
        private::facade_validator(self)
    }
}

mod private {
    use super::AgentFactoryBuildAuthority;
    use std::ffi::c_void;

    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn __meerkat_agent_factory_build_authority_validate(authority: *const c_void) -> bool;
    }

    #[allow(unsafe_code)]
    pub(super) fn facade_validator(authority: &AgentFactoryBuildAuthority) -> bool {
        // SAFETY: the canonical facade provides this validator symbol. Graphs
        // that do not link the facade cannot satisfy this validation path.
        unsafe {
            __meerkat_agent_factory_build_authority_validate(
                std::ptr::from_ref(authority).cast::<c_void>(),
            )
        }
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

    pub(crate) fn test_factory_guard_type() -> TypeId {
        TypeId::of::<TestFactoryAuthorityGuard>()
    }

    pub(crate) fn test_factory_source_type() -> TypeId {
        TypeId::of::<TestFactoryAuthoritySource>()
    }

    fn authority_from_source<G: 'static, T: 'static>() -> AgentFactoryBuildAuthority {
        AgentFactoryBuildAuthority {
            guard_type: TypeId::of::<G>(),
            source_type: TypeId::of::<T>(),
        }
    }

    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    extern "C" fn __meerkat_agent_factory_build_authority_validate(
        authority: *const std::ffi::c_void,
    ) -> bool {
        let authority = {
            // SAFETY: test calls pass a reference-derived pointer from
            // `is_canonical_factory_authority`.
            unsafe { authority.cast::<AgentFactoryBuildAuthority>().as_ref() }
        };
        authority.is_some_and(|authority| {
            authority.guard_type == test_factory_guard_type()
                && authority.source_type == test_factory_source_type()
        })
    }

    #[test]
    fn registered_factory_source_validates() {
        let authority =
            authority_from_source::<TestFactoryAuthorityGuard, TestFactoryAuthoritySource>();

        assert!(authority.is_canonical_factory_authority());
    }

    #[test]
    fn non_registered_factory_source_is_rejected() {
        let authority = authority_from_source::<TestFactoryAuthorityGuard, ForgedAuthoritySource>();

        assert!(!authority.is_canonical_factory_authority());
    }

    #[test]
    fn non_registered_factory_guard_is_rejected() {
        let authority = authority_from_source::<ForgedAuthorityGuard, TestFactoryAuthoritySource>();

        assert!(!authority.is_canonical_factory_authority());
    }
}
