mod compositions;
mod coverage;
pub(crate) mod dsl;

use crate::{CompositionSchema, MachineSchema};

pub use compositions::{
    meerkat_mob_seam_composition, schedule_bundle_composition, schedule_mob_bundle_composition,
    schedule_runtime_bundle_composition,
};
pub use coverage::{
    CodeAnchor, CompositionCoverageManifest, MachineCoverageManifest, ScenarioCoverage,
    SemanticCoverageEntry, canonical_composition_coverage_manifests,
    canonical_machine_coverage_manifests,
};

/// DSL-generated schema for MeerkatMachine.
pub fn meerkat_machine() -> MachineSchema {
    dsl::dsl_meerkat_machine()
}

/// DSL-generated schema for MobMachine.
pub fn mob_machine() -> MachineSchema {
    dsl::dsl_mob_machine()
}

/// DSL-generated schema for OccurrenceLifecycleMachine.
pub fn occurrence_lifecycle_machine() -> MachineSchema {
    dsl::dsl_occurrence_lifecycle_machine()
}

/// DSL-generated schema for ScheduleLifecycleMachine.
pub fn schedule_lifecycle_machine() -> MachineSchema {
    dsl::dsl_schedule_lifecycle_machine()
}

pub fn canonical_machine_schemas() -> Vec<MachineSchema> {
    vec![
        meerkat_machine(),
        mob_machine(),
        schedule_lifecycle_machine(),
        occurrence_lifecycle_machine(),
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
