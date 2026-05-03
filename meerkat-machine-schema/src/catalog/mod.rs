mod compositions;
mod coverage;
pub mod dsl;

use crate::{CompositionSchema, MachineSchema};
use crate::{RustBinding, identity::MachineId};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineProductionOwnerRelation {
    pub machine: MachineId,
    pub rust: RustBinding,
}

impl MachineProductionOwnerRelation {
    pub fn new(machine: &str, crate_name: &str, module: &str) -> Self {
        Self {
            #[allow(clippy::expect_used)]
            machine: MachineId::parse(machine).expect("valid machine id"),
            rust: RustBinding {
                crate_name: crate_name.to_owned(),
                module: module.to_owned(),
            },
        }
    }
}

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

pub fn canonical_machine_production_owner_relations() -> Vec<MachineProductionOwnerRelation> {
    vec![
        MachineProductionOwnerRelation::new(
            "MeerkatMachine",
            dsl::MEERKAT_MACHINE_PRODUCTION_RUST_CRATE,
            dsl::MEERKAT_MACHINE_PRODUCTION_RUST_MODULE,
        ),
        MachineProductionOwnerRelation::new(
            "AuthMachine",
            dsl::AUTH_MACHINE_PRODUCTION_RUST_CRATE,
            dsl::AUTH_MACHINE_PRODUCTION_RUST_MODULE,
        ),
        MachineProductionOwnerRelation::new(
            "MobMachine",
            dsl::MOB_MACHINE_PRODUCTION_RUST_CRATE,
            dsl::MOB_MACHINE_PRODUCTION_RUST_MODULE,
        ),
        MachineProductionOwnerRelation::new(
            "ScheduleLifecycleMachine",
            dsl::SCHEDULE_LIFECYCLE_PRODUCTION_RUST_CRATE,
            dsl::SCHEDULE_LIFECYCLE_PRODUCTION_RUST_MODULE,
        ),
        MachineProductionOwnerRelation::new(
            "OccurrenceLifecycleMachine",
            dsl::OCCURRENCE_LIFECYCLE_PRODUCTION_RUST_CRATE,
            dsl::OCCURRENCE_LIFECYCLE_PRODUCTION_RUST_MODULE,
        ),
    ]
}
