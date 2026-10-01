//! Exact mathematical attenuation data, never authentication or grant authority.

use serde::{Deserialize, Serialize};

use crate::constraints::{DelegationFailure, ExecutionRestrictions};

/// Immutable inputs and the exact result of the existing child restriction algebra.
///
/// Construction and decoding establish only the relationship among these values.
/// The grant owner must independently bind `parent()` to its actual retained
/// parent, validate the issuer and ancestors, and enforce current authorization.
/// Serialization exposes protected policy data and requires authorized custody.
/// Debug output deliberately omits all values.
///
/// Fields cannot be constructed independently of the algebra:
///
/// ```compile_fail
/// use meerkat_authorization_contracts::{constraints::ExecutionRestrictions,
///     derived_child::DerivedChildRestrictions};
/// let value = ExecutionRestrictions::unrestricted();
/// let forged = DerivedChildRestrictions {
///     parent: value.clone(), requested: value.clone(), effective: value,
/// };
/// ```
///
/// Accessors cannot mutate the retained values:
///
/// ```compile_fail
/// use meerkat_authorization_contracts::{constraints::ExecutionRestrictions,
///     derived_child::DerivedChildRestrictions};
/// let mut derived = DerivedChildRestrictions::new(
///     ExecutionRestrictions::unrestricted(), ExecutionRestrictions::unrestricted(),
/// ).unwrap();
/// *derived.effective() = ExecutionRestrictions::unrestricted();
/// ```
///
/// Field projection does not provide mutable access or a proof constructor:
///
/// ```compile_fail
/// use meerkat_authorization_contracts::{constraints::ExecutionRestrictions,
///     derived_child::DerivedChildRestrictions};
/// let mut derived = DerivedChildRestrictions::new(
///     ExecutionRestrictions::unrestricted(), ExecutionRestrictions::unrestricted(),
/// ).unwrap();
/// derived.effective = ExecutionRestrictions::unrestricted();
/// ```
///
/// Missing data has no default relationship:
///
/// ```compile_fail
/// use meerkat_authorization_contracts::derived_child::DerivedChildRestrictions;
/// let derived = DerivedChildRestrictions::default();
/// ```
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct DerivedChildRestrictions {
    view: DerivedChildRestrictionsView,
}

/// Read-only projection for canonical generated guards. Constructing or mutating
/// a separate view cannot construct or mutate a checked attenuation value.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct DerivedChildRestrictionsView {
    pub parent: ExecutionRestrictions,
    pub requested: ExecutionRestrictions,
    pub effective: ExecutionRestrictions,
}

impl std::ops::Deref for DerivedChildRestrictions {
    type Target = DerivedChildRestrictionsView;

    fn deref(&self) -> &Self::Target {
        &self.view
    }
}

impl std::fmt::Debug for DerivedChildRestrictionsView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DerivedChildRestrictionsView")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for DerivedChildRestrictions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DerivedChildRestrictions")
            .finish_non_exhaustive()
    }
}

impl DerivedChildRestrictions {
    /// Compute the exact child values without consulting any authority owner.
    ///
    /// # Errors
    /// Returns the existing algebra's error for exhausted or unresolved parent
    /// delegation depth. Other unresolved facts remain in the effective value.
    pub fn new(
        parent: ExecutionRestrictions,
        requested: ExecutionRestrictions,
    ) -> Result<Self, DelegationFailure> {
        let effective = parent.for_child(&requested)?;
        Ok(Self {
            view: DerivedChildRestrictionsView {
                parent,
                requested,
                effective,
            },
        })
    }

    #[must_use]
    pub fn parent(&self) -> &ExecutionRestrictions {
        &self.parent
    }

    #[must_use]
    pub fn requested(&self) -> &ExecutionRestrictions {
        &self.requested
    }

    #[must_use]
    pub fn effective(&self) -> &ExecutionRestrictions {
        &self.effective
    }
}

impl<'de> Deserialize<'de> for DerivedChildRestrictions {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            parent: ExecutionRestrictions,
            requested: ExecutionRestrictions,
            effective: ExecutionRestrictions,
        }

        let wire = Wire::deserialize(deserializer)?;
        let derived = Self::new(wire.parent, wire.requested).map_err(D::Error::custom)?;
        if derived.effective != wire.effective {
            return Err(D::Error::custom(
                "child restrictions differ from the exact attenuation result",
            ));
        }
        Ok(derived)
    }
}

#[cfg(test)]
mod tests;
