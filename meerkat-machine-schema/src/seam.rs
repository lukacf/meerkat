//! Schema-owned seam classification for effect dispositions.
//!
//! Every effect disposition declares how its ownership boundary behaves. This
//! classification lives with the effect on the catalog DSL (the `seam` clause of
//! a `disposition` declaration) and is emitted onto the generated
//! [`EffectDispositionRule`](crate::EffectDispositionRule), so the seam-inventory
//! audit reads the classification straight off the schema instead of a
//! hand-maintained side table.

use std::fmt;

/// Classification of an effect's ownership boundary characteristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeamClassification {
    /// Effect is fully internal to the machine — no owner realization needed.
    /// Examples: state projection, local bookkeeping.
    NoOwnerRealization,
    /// Effect requires owner/shell to realize it, but no feedback is expected.
    /// The machine emits and moves on; correctness does not depend on acknowledgment.
    OwnerRealizationOnly,
    /// Effect requires owner realization AND the owner must feed back into the
    /// machine (or another composed machine) for the lifecycle to close.
    /// This is the seam that needs a formal handoff protocol.
    OwnerRealizationPlusFeedback,
    /// Effect is a terminal/result signal whose surface representation must align
    /// with machine truth. Divergence here means the API lies about outcomes.
    SurfaceResultAlignment,
}

impl fmt::Display for SeamClassification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOwnerRealization => write!(f, "no-owner-realization"),
            Self::OwnerRealizationOnly => write!(f, "owner-realization-only"),
            Self::OwnerRealizationPlusFeedback => write!(f, "owner-realization-plus-feedback"),
            Self::SurfaceResultAlignment => write!(f, "surface-result-alignment"),
        }
    }
}
