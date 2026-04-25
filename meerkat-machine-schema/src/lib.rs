pub mod catalog;
/// Compatibility-only absorbed machine schemas retained for generated kernel
/// consumers during the two-kernel collapse. These are intentionally excluded
/// from the canonical registry.
pub mod compat;
mod composition;
pub mod identity;
mod machine;
pub mod types;

pub use identity::{NamedTypeBinding, RustTypeAtom};
pub use types::{CommsRuntimeId, McpServerId, MobId, PeerCorrelationId};

pub use catalog::{
    CodeAnchor, CompositionCoverageManifest, MachineCoverageManifest, ScenarioCoverage,
    SemanticCoverageEntry, canonical_composition_coverage_manifests, canonical_composition_schemas,
    canonical_machine_coverage_manifests, canonical_machine_schemas, compat_composition_schemas,
    meerkat_mob_seam_composition,
};
pub use compat::{
    auth_lease_bridge_machine, external_tool_surface_bridge_machine, flow_frame_machine,
    flow_run_machine, loop_iteration_machine, mob_destroy_session_ingress_bridge_machine,
    ops_barrier_bridge_machine, supervisor_trust_bridge_machine,
};
pub use composition::{
    ActorKind, ActorPriority, ActorSchema, ClosurePolicy, CompositionDriver,
    CompositionDriverRustBinding, CompositionInvariant, CompositionInvariantKind,
    CompositionSchema, CompositionSchemaError, CompositionStateLimits, CompositionTransactionPlan,
    CompositionWitness, CompositionWitnessField, CompositionWitnessInput, CompositionWitnessState,
    CompositionWitnessTransition, CompositionWitnessTransitionOrder, DriverDispatchRoute,
    EffectHandoffProtocol, EntryInput, FeedbackFieldBinding, FeedbackFieldSource, FeedbackInputRef,
    MachineInstance, ProtocolGenerationMode, ProtocolHandleArgKey, ProtocolHelperReturnShape,
    ProtocolRustBinding, Route, RouteBindingSource, RouteDelivery, RouteFieldBinding, RouteTarget,
    RouteTargetKind, RouteTargetSelector, RouteVariantId, SchedulerRule, WatchedEffect,
};
pub use machine::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    FieldType, Guard, HelperSchema, InitSchema, InputMatch, InvariantSchema, MachineSchema,
    MachineSchemaError, Quantifier, RustBinding, StateSchema, TransitionSchema, TriggerKind,
    TriggerMatch, TypeRef, Update, VariantSchema,
};

#[cfg(test)]
mod tests {
    use super::{Update, canonical_machine_coverage_manifests, canonical_machine_schemas};
    use crate::catalog::dsl::dsl_schedule_lifecycle_machine as schedule_lifecycle_machine;

    #[test]
    fn schedule_and_occurrence_machines_are_registered_in_canonical_catalog() {
        let machine_names: Vec<_> = canonical_machine_schemas()
            .into_iter()
            .map(|schema| schema.machine)
            .collect();
        let coverage_names: Vec<_> = canonical_machine_coverage_manifests()
            .into_iter()
            .map(|manifest| manifest.machine)
            .collect();

        assert!(
            machine_names
                .iter()
                .any(|name| name.as_str() == "ScheduleLifecycleMachine"),
            "schedule lifecycle machine must be a canonical schema"
        );
        assert!(
            machine_names
                .iter()
                .any(|name| name.as_str() == "OccurrenceLifecycleMachine"),
            "occurrence lifecycle machine must be a canonical schema"
        );
        assert!(
            coverage_names
                .iter()
                .any(|name| name.as_str() == "ScheduleLifecycleMachine"),
            "schedule lifecycle machine must have coverage metadata"
        );
        assert!(
            coverage_names
                .iter()
                .any(|name| name.as_str() == "OccurrenceLifecycleMachine"),
            "occurrence lifecycle machine must have coverage metadata"
        );
    }

    #[test]
    fn schedule_delete_transitions_bump_revision_and_supersede_pending_occurrences() {
        let machine = schedule_lifecycle_machine();

        for transition_name in ["DeleteActive", "DeletePaused"] {
            let transition = machine
                .transitions
                .iter()
                .find(|transition| transition.name.as_str() == transition_name);

            assert!(transition.is_some(), "missing {transition_name} transition");
            let Some(transition) = transition else {
                return;
            };

            assert!(
                transition.updates.iter().any(|update| matches!(
                    update,
                    Update::Increment { field, amount } if field.as_str() == "revision" && *amount == 1
                )),
                "{transition_name} should advance the revision"
            );
            assert!(
                transition
                    .emit
                    .iter()
                    .any(|effect| effect.variant.as_str() == "SupersedePendingOccurrences"),
                "{transition_name} should supersede older pending occurrences"
            );
        }
    }

    #[test]
    fn canonical_registry_excludes_compat_flow_machine_schemas() {
        let machine_names: Vec<_> = canonical_machine_schemas()
            .into_iter()
            .map(|schema| schema.machine)
            .collect();

        for compat_name in ["FlowRunMachine", "FlowFrameMachine", "LoopIterationMachine"] {
            assert!(
                !machine_names
                    .iter()
                    .any(|name| name.as_str() == compat_name),
                "{compat_name} should remain compat-only, not canonical"
            );
        }
    }
}
