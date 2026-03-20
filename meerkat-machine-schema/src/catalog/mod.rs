mod comms_drain_lifecycle;
mod compositions;
mod coverage;
mod external_tool_surface;
mod flow_run;
mod input_lifecycle;
mod mob_lifecycle;
mod mob_orchestrator;
mod ops_lifecycle;
mod peer_comms;
mod runtime_control;
mod runtime_ingress;
mod turn_execution;

use crate::{CompositionSchema, MachineSchema};

pub use comms_drain_lifecycle::comms_drain_lifecycle_machine;
pub use compositions::{
    continuation_runtime_bundle_composition, external_tool_bundle_composition,
    mob_bundle_composition, ops_peer_bundle_composition, ops_runtime_bundle_composition,
    peer_runtime_bundle_composition, runtime_pipeline_composition,
    surface_event_runtime_bundle_composition,
};
pub use coverage::{
    CodeAnchor, CompositionCoverageManifest, MachineCoverageManifest, ScenarioCoverage,
    SemanticCoverageEntry, canonical_composition_coverage_manifests,
    canonical_machine_coverage_manifests,
};
pub use external_tool_surface::external_tool_surface_machine;
pub use flow_run::flow_run_machine;
pub use input_lifecycle::input_lifecycle_machine;
pub use mob_lifecycle::mob_lifecycle_machine;
pub use mob_orchestrator::mob_orchestrator_machine;
pub use ops_lifecycle::ops_lifecycle_machine;
pub use peer_comms::peer_comms_machine;
pub use runtime_control::runtime_control_machine;
pub use runtime_ingress::runtime_ingress_machine;
pub use turn_execution::turn_execution_machine;

pub fn canonical_machine_schemas() -> Vec<MachineSchema> {
    vec![
        input_lifecycle_machine(),
        runtime_control_machine(),
        runtime_ingress_machine(),
        ops_lifecycle_machine(),
        peer_comms_machine(),
        external_tool_surface_machine(),
        turn_execution_machine(),
        mob_lifecycle_machine(),
        flow_run_machine(),
        mob_orchestrator_machine(),
        comms_drain_lifecycle_machine(),
    ]
}

pub fn canonical_composition_schemas() -> Vec<CompositionSchema> {
    vec![
        runtime_pipeline_composition(),
        surface_event_runtime_bundle_composition(),
        continuation_runtime_bundle_composition(),
        external_tool_bundle_composition(),
        peer_runtime_bundle_composition(),
        ops_runtime_bundle_composition(),
        ops_peer_bundle_composition(),
        mob_bundle_composition(),
    ]
}
