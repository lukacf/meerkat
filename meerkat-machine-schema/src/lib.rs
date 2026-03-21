pub mod catalog;
mod composition;
mod machine;

pub use catalog::{
    CodeAnchor, CompositionCoverageManifest, MachineCoverageManifest, ScenarioCoverage,
    SemanticCoverageEntry, canonical_composition_coverage_manifests, canonical_composition_schemas,
    canonical_machine_coverage_manifests, canonical_machine_schemas,
    comms_drain_lifecycle_composition, comms_drain_lifecycle_machine,
    external_tool_bundle_composition, external_tool_surface_machine, flow_run_machine,
    input_lifecycle_machine, mob_bundle_composition, mob_lifecycle_machine,
    mob_orchestrator_machine, ops_lifecycle_machine, ops_peer_bundle_composition,
    ops_runtime_bundle_composition, peer_comms_machine, peer_runtime_bundle_composition,
    runtime_control_machine, runtime_ingress_machine, runtime_pipeline_composition,
    turn_execution_machine,
};
pub use composition::{
    ActorKind, ActorPriority, ActorSchema, ClosurePolicy, CompositionInvariant,
    CompositionInvariantKind, CompositionSchema, CompositionSchemaError, CompositionStateLimits,
    CompositionWitness, CompositionWitnessField, CompositionWitnessInput, CompositionWitnessState,
    CompositionWitnessTransition, CompositionWitnessTransitionOrder, EffectHandoffProtocol,
    EntryInput, FeedbackFieldBinding, FeedbackFieldSource, FeedbackInputRef, MachineInstance,
    ProtocolGenerationMode, ProtocolHelperReturnShape, ProtocolRustBinding, Route,
    RouteBindingSource, RouteDelivery, RouteFieldBinding, RouteTarget, SchedulerRule,
};
pub use machine::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    FieldType, Guard, HelperSchema, InitSchema, InputMatch, InvariantSchema, MachineSchema,
    MachineSchemaError, Quantifier, RustBinding, StateSchema, TransitionSchema, TypeRef, Update,
    VariantSchema,
};
