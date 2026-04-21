pub mod catalog;
//
// Canonical machine schemas live under `catalog`. The former flow/frame/loop
// compat helpers are no longer exported from this crate; the remaining
// runtime-local flow kernel schemas live under `meerkat_mob::runtime::flow_kernels::*`.
mod composition;
mod machine;
pub mod types;

pub use types::{CommsRuntimeId, McpServerId, MobId, PeerCorrelationId};

pub use catalog::{
    CodeAnchor, CompositionCoverageManifest, MachineCoverageManifest, ScenarioCoverage,
    SemanticCoverageEntry, canonical_composition_coverage_manifests, canonical_composition_schemas,
    canonical_machine_coverage_manifests, canonical_machine_schemas, meerkat_mob_seam_composition,
};
pub use composition::{
    ActorKind, ActorPriority, ActorSchema, ClosurePolicy, CompositionDriverRustBinding,
    CompositionInvariant, CompositionInvariantKind, CompositionSchema, CompositionSchemaError,
    CompositionStateLimits, CompositionTransactionPlan, CompositionWitness,
    CompositionWitnessField, CompositionWitnessInput, CompositionWitnessState,
    CompositionWitnessTransition, CompositionWitnessTransitionOrder, EffectHandoffProtocol,
    EntryInput, FeedbackFieldBinding, FeedbackFieldSource, FeedbackInputRef, MachineInstance,
    ProtocolGenerationMode, ProtocolHelperReturnShape, ProtocolRustBinding, Route,
    RouteBindingSource, RouteDelivery, RouteFieldBinding, RouteTarget, RouteTargetKind,
    RouteTargetSelector, SchedulerRule,
};
pub use machine::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    FieldType, Guard, HelperSchema, InitSchema, InputMatch, InvariantSchema, MachineSchema,
    MachineSchemaError, Quantifier, RustBinding, StateSchema, TransitionSchema, TriggerKind,
    TypeRef, Update, VariantSchema,
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
                .any(|name| name == "ScheduleLifecycleMachine"),
            "schedule lifecycle machine must be a canonical schema"
        );
        assert!(
            machine_names
                .iter()
                .any(|name| name == "OccurrenceLifecycleMachine"),
            "occurrence lifecycle machine must be a canonical schema"
        );
        assert!(
            coverage_names
                .iter()
                .any(|name| name == "ScheduleLifecycleMachine"),
            "schedule lifecycle machine must have coverage metadata"
        );
        assert!(
            coverage_names
                .iter()
                .any(|name| name == "OccurrenceLifecycleMachine"),
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
                .find(|transition| transition.name == transition_name);

            assert!(transition.is_some(), "missing {transition_name} transition");
            let Some(transition) = transition else {
                return;
            };

            assert!(
                transition.updates.iter().any(|update| matches!(
                    update,
                    Update::Increment { field, amount } if field == "revision" && *amount == 1
                )),
                "{transition_name} should advance the revision"
            );
            assert!(
                transition
                    .emit
                    .iter()
                    .any(|effect| effect.variant == "SupersedePendingOccurrences"),
                "{transition_name} should supersede older pending occurrences"
            );
        }
    }
}
