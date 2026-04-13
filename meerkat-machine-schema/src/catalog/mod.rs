mod compositions;
mod coverage;
mod meerkat_machine;
mod mob_machine;
mod occurrence_lifecycle;
mod peer_directory_reachability;
mod schedule_lifecycle;
mod session_tool_visibility;
mod session_turn_admission;

use crate::{CompositionSchema, MachineSchema};

// Canonical exposures for the two-kernel cutover
pub use compositions::{
    meerkat_mob_seam_composition, schedule_bundle_composition, schedule_mob_bundle_composition,
    schedule_runtime_bundle_composition,
};
pub use coverage::{
    CodeAnchor, CompositionCoverageManifest, MachineCoverageManifest, ScenarioCoverage,
    SemanticCoverageEntry, canonical_composition_coverage_manifests,
    canonical_machine_coverage_manifests,
};
pub use meerkat_machine::meerkat_machine;
pub use mob_machine::mob_machine;
pub use occurrence_lifecycle::occurrence_lifecycle_machine;
pub use peer_directory_reachability::peer_directory_reachability_machine;
pub use schedule_lifecycle::schedule_lifecycle_machine;
pub use session_tool_visibility::session_tool_visibility_machine;
pub use session_turn_admission::session_turn_admission_machine;

pub fn canonical_machine_schemas() -> Vec<MachineSchema> {
    vec![
        meerkat_machine(),
        mob_machine(),
        peer_directory_reachability_machine(),
        schedule_lifecycle_machine(),
        occurrence_lifecycle_machine(),
        session_tool_visibility_machine(),
        session_turn_admission_machine(),
    ]
}

pub fn canonical_composition_schemas() -> Vec<CompositionSchema> {
    vec![
        meerkat_mob_seam_composition(),
        schedule_bundle_composition(),
        schedule_runtime_bundle_composition(),
        schedule_mob_bundle_composition(),
    ]
}
