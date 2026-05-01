//! Internal typed authority for factory-owned agent construction.
//!
//! This crate exists so `meerkat-core` can require a concrete capability type
//! without re-exporting a minting API from `meerkat_core::agent`. The facade
//! factory owns a private source marker and registers that marker with this
//! crate; core accepts only authority values tied to that single marker.

use std::any::TypeId;

/// Capability proving that the caller is entering core construction from a
/// factory-owned policy path.
#[derive(Debug, Clone, Copy)]
#[repr(transparent)]
pub struct AgentFactoryBuildAuthority {
    source_type: TypeId,
}

impl AgentFactoryBuildAuthority {
    /// Validate that this value was minted for the registered facade factory
    /// marker type.
    #[doc(hidden)]
    pub fn is_canonical_factory_authority(&self) -> bool {
        private::canonical_factory_source_type()
            .is_some_and(|source_type| source_type == self.source_type)
    }
}

/// Registration for the facade-owned authority source marker.
///
/// Validation fails closed unless exactly one marker is registered in the
/// process. Additional downstream registrations cannot mint a second accepted
/// authority in a graph that already links the canonical facade.
#[doc(hidden)]
#[repr(transparent)]
pub struct AgentFactoryBuildAuthorityRegistration {
    source_type: fn() -> TypeId,
}

inventory::collect!(AgentFactoryBuildAuthorityRegistration);

mod private {
    use super::{AgentFactoryBuildAuthorityRegistration, TypeId};

    pub(super) fn canonical_factory_source_type() -> Option<TypeId> {
        let mut registrations =
            inventory::iter::<AgentFactoryBuildAuthorityRegistration>.into_iter();
        let source_type = (registrations.next()?.source_type)();
        if registrations.next().is_some() {
            return None;
        }
        Some(source_type)
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentFactoryBuildAuthority, AgentFactoryBuildAuthorityRegistration};
    use std::any::TypeId;

    struct TestFactoryAuthoritySource;
    struct ForgedAuthoritySource;

    fn test_factory_source_type() -> TypeId {
        TypeId::of::<TestFactoryAuthoritySource>()
    }

    #[allow(unsafe_code)]
    const fn authority_registration(
        source_type: fn() -> TypeId,
    ) -> AgentFactoryBuildAuthorityRegistration {
        // SAFETY: registration is a transparent wrapper around a source-type
        // provider function. Tests mirror the facade's private registration path.
        unsafe {
            std::mem::transmute::<fn() -> TypeId, AgentFactoryBuildAuthorityRegistration>(
                source_type,
            )
        }
    }

    inventory::submit! {
        authority_registration(test_factory_source_type)
    }

    fn authority_from_source<T: 'static>() -> AgentFactoryBuildAuthority {
        AgentFactoryBuildAuthority {
            source_type: TypeId::of::<T>(),
        }
    }

    #[test]
    fn registered_factory_source_validates() {
        let authority = authority_from_source::<TestFactoryAuthoritySource>();

        assert!(authority.is_canonical_factory_authority());
    }

    #[test]
    fn non_registered_factory_source_is_rejected() {
        let authority = authority_from_source::<ForgedAuthoritySource>();

        assert!(!authority.is_canonical_factory_authority());
    }
}
