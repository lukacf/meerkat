//! The family's implementation over the host's rules: the launch policy
//! attached, a request normalized for its requester, the host's hooks, the
//! consent its rule gives, and the commit through the executor seam.
//!
//! This file holds, so far, the slot a mob's identity runtime keeps its
//! family in ([`IdentityFamilySlot`]).

use std::sync::{Arc, PoisonError, RwLock};

use super::family::IdentityFamily;

/// The family of changes and reads of one mob's long-term agents, once a
/// launch that has an identity policy attached it. The mob's identity
/// runtime holds the slot, and every surface reads it at each call: a
/// holder built before the family exists (an agent's tool surface) sees it
/// once it is attached. Empty on a launch without an identity policy, and
/// again once the mob stopped.
#[derive(Clone, Default)]
pub struct IdentityFamilySlot(Arc<RwLock<Option<Arc<dyn IdentityFamily>>>>);

impl IdentityFamilySlot {
    /// Attach `family`: every surface of the mob calls it from now on.
    pub fn attach(&self, family: Arc<dyn IdentityFamily>) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Some(family);
    }

    /// The attached family, if any.
    pub fn get(&self) -> Option<Arc<dyn IdentityFamily>> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Detach the family: the mob stopped.
    pub fn clear(&self) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl std::fmt::Debug for IdentityFamilySlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IdentityFamilySlot")
            .field("attached", &self.get().is_some())
            .finish()
    }
}
