// @generated — protocol helpers for `surface_snapshot_alignment`
// Composition: meerkat_mob_seam, Producer: meerkat, Effect: RefreshVisibleSurfaceSet
// Closure policy: AckRequired
// Liveness: eventual snapshot acknowledgement under surface host liveness

use meerkat_core::handles::{
    DslTransitionError, ExternalToolSurfaceEffect, ExternalToolSurfaceHandle,
};

#[derive(Debug, Clone)]
pub struct SurfaceSnapshotAlignmentObligation {
    pub snapshot_epoch: u64,
}

pub fn extract_obligations(
    effects: &[ExternalToolSurfaceEffect],
) -> Vec<SurfaceSnapshotAlignmentObligation> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            ExternalToolSurfaceEffect::RefreshVisibleSurfaceSet { snapshot_epoch } => {
                Some(SurfaceSnapshotAlignmentObligation {
                    snapshot_epoch: *snapshot_epoch,
                })
            }
            _ => None,
        })
        .collect()
}

pub fn submit_surface_snapshot_aligned(
    handle: &(impl ExternalToolSurfaceHandle + ?Sized),
    obligation: SurfaceSnapshotAlignmentObligation,
) -> Result<(), DslTransitionError> {
    handle.snapshot_aligned(obligation.snapshot_epoch)
}
