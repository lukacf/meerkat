mod compositions;
mod coverage;
pub mod dsl;

use crate::{CompositionSchema, MachineSchema};

// Canonical exposures for the two-kernel cutover
pub use compositions::{
    auth_lease_bundle_composition, compat_composition_schemas, meerkat_mob_seam_composition,
    schedule_bundle_composition, schedule_mob_bundle_composition,
    schedule_runtime_bundle_composition,
};
pub use coverage::{
    CodeAnchor, CompositionCoverageManifest, MachineCoverageManifest, ScenarioCoverage,
    SemanticCoverageEntry, canonical_composition_coverage_manifests,
    canonical_machine_coverage_manifests,
};

pub fn canonical_machine_schemas() -> Vec<MachineSchema> {
    vec![
        dsl::dsl_meerkat_machine(),
        dsl::dsl_mob_machine(),
        dsl::dsl_schedule_lifecycle_machine(),
        dsl::dsl_occurrence_lifecycle_machine(),
        dsl::dsl_auth_machine(),
    ]
}

pub fn canonical_composition_schemas() -> Vec<CompositionSchema> {
    vec![
        meerkat_mob_seam_composition(),
        schedule_bundle_composition(),
        schedule_runtime_bundle_composition(),
        schedule_mob_bundle_composition(),
        auth_lease_bundle_composition(),
    ]
}
