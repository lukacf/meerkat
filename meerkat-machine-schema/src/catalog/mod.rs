mod comms_drain_lifecycle;
mod compositions;
mod coverage;
mod external_tool_surface;
mod flow_run;
mod input_lifecycle;
mod mob_helper_result_anchor;
mod mob_lifecycle;
mod mob_member_lifecycle_anchor;
mod mob_orchestrator;
mod mob_runtime_bridge_anchor;
mod mob_wiring_anchor;
mod ops_lifecycle;
mod peer_comms;
mod runtime_control;
mod runtime_ingress;
mod turn_execution;

use crate::{CompositionSchema, MachineSchema};

pub use comms_drain_lifecycle::comms_drain_lifecycle_machine;
pub use compositions::{
    comms_drain_lifecycle_composition, continuation_runtime_bundle_composition,
    external_tool_bundle_composition, mob_bundle_composition, ops_peer_bundle_composition,
    ops_runtime_bundle_composition, peer_runtime_bundle_composition, runtime_pipeline_composition,
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
/// Observation anchors hold statements about what the composition has seen while
/// canonical mob ownership schemas continue to evolve. They are not the final
/// owners of these routes and only exist to keep the instrumentation explicit.
pub use mob_helper_result_anchor::mob_helper_result_anchor_machine;
pub use mob_lifecycle::mob_lifecycle_machine;
pub use mob_member_lifecycle_anchor::mob_member_lifecycle_anchor_machine;
pub use mob_orchestrator::mob_orchestrator_machine;
pub use mob_runtime_bridge_anchor::mob_runtime_bridge_anchor_machine;
pub use mob_wiring_anchor::mob_wiring_anchor_machine;
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
        // Observation anchors keep ownership-extraction routes explicit in the
        // formal catalog while canonical mob ownership is still being modeled
        // in future lifecycle/bridge/wiring/helper classifier machines. These
        // anchors are not the canonical control points for those domains.
        mob_member_lifecycle_anchor_machine(),
        mob_runtime_bridge_anchor_machine(),
        mob_wiring_anchor_machine(),
        mob_helper_result_anchor_machine(),
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
        comms_drain_lifecycle_composition(),
    ]
}
