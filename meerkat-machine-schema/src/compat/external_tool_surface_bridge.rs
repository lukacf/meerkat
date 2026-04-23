//! Compat bridging machine that hosts the external-tool-surface
//! handoff protocols' producer-side declarations.
//!
//! Pattern mirror of `ops_barrier_bridge`: the runtime's hand-written
//! `ExternalToolSurfaceAuthority` emits the real effects
//! (`ScheduleSurfaceCompletion`, `RefreshVisibleSurfaceSet`, ...) but
//! the DSL macro used by canonical MeerkatMachine cannot annotate a
//! disposition with `handoff_protocol`. This bridge schema hosts the
//! annotation so `surface_completion` and `surface_snapshot_alignment`
//! can be declared as honest handoff protocols in the compat
//! composition.
//!
//! Intentionally excluded from the canonical catalog and TLC state
//! space; it exists only for the protocol-codegen producer lookup.

use crate::{
    EffectDisposition, EffectDispositionRule, EnumSchema, FieldSchema, InitSchema, MachineSchema,
    NamedTypeBinding, RustBinding, StateSchema, TypeRef, VariantSchema,
};
use crate::identity::{
    EffectVariantId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, MachineId,
    NamedTypeId, PhaseId, ProtocolId, TransitionId,
};

/// Minimal compat machine hosting the surface handoff protocols'
/// producer annotations.
pub fn external_tool_surface_bridge_machine() -> MachineSchema {
    MachineSchema {
        machine: MachineId::parse("ExternalToolSurfaceBridgeMachine").expect("valid machine slug"),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-mcp".into(),
            module: "external_tool_surface_authority".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "ExternalToolSurfaceBridgePhase".into(),
                variants: vec![variant("Idle")],
            },
            fields: vec![],
            init: InitSchema {
            phase: PhaseId::parse("Idle").expect("valid phase slug"),
                fields: vec![],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "ExternalToolSurfaceBridgeInput".into(),
            variants: vec![
                VariantSchema {
                name: EnumVariantId::parse("SnapshotAligned").expect("valid variant slug"),
                    fields: vec![FieldSchema {
                name: FieldId::parse("snapshot_epoch").expect("valid field slug"),
                        ty: TypeRef::U64,
                    }],
                },
                VariantSchema {
                name: EnumVariantId::parse("PendingSucceeded").expect("valid variant slug"),
                    fields: vec![
                        FieldSchema {
                name: FieldId::parse("surface_id").expect("valid field slug"),
                            ty: TypeRef::Named(NamedTypeId::parse("SurfaceId").expect("valid named-type slug")),
                        },
                        FieldSchema {
                name: FieldId::parse("pending_task_sequence").expect("valid field slug"),
                            ty: TypeRef::U64,
                        },
                        FieldSchema {
                name: FieldId::parse("staged_intent_sequence").expect("valid field slug"),
                            ty: TypeRef::U64,
                        },
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("PendingFailed").expect("valid variant slug"),
                    fields: vec![
                        FieldSchema {
                name: FieldId::parse("surface_id").expect("valid field slug"),
                            ty: TypeRef::Named(NamedTypeId::parse("SurfaceId").expect("valid named-type slug")),
                        },
                        FieldSchema {
                name: FieldId::parse("pending_task_sequence").expect("valid field slug"),
                            ty: TypeRef::U64,
                        },
                        FieldSchema {
                name: FieldId::parse("reason").expect("valid field slug"),
                            ty: TypeRef::String,
                        },
                    ],
                },
            ],
        },
        signals: EnumSchema {
            name: "ExternalToolSurfaceBridgeSignal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "ExternalToolSurfaceBridgeEffect".into(),
            variants: vec![
                VariantSchema {
                name: EnumVariantId::parse("RefreshVisibleSurfaceSet").expect("valid variant slug"),
                    fields: vec![FieldSchema {
                name: FieldId::parse("snapshot_epoch").expect("valid field slug"),
                        ty: TypeRef::U64,
                    }],
                },
                VariantSchema {
                name: EnumVariantId::parse("ScheduleSurfaceCompletion").expect("valid variant slug"),
                    fields: vec![
                        FieldSchema {
                name: FieldId::parse("surface_id").expect("valid field slug"),
                            ty: TypeRef::Named(NamedTypeId::parse("SurfaceId").expect("valid named-type slug")),
                        },
                        FieldSchema {
                name: FieldId::parse("operation").expect("valid field slug"),
                            ty: TypeRef::Named(NamedTypeId::parse("SurfaceDeltaOperation").expect("valid named-type slug")),
                        },
                        FieldSchema {
                name: FieldId::parse("pending_task_sequence").expect("valid field slug"),
                            ty: TypeRef::U64,
                        },
                        FieldSchema {
                name: FieldId::parse("staged_intent_sequence").expect("valid field slug"),
                            ty: TypeRef::U64,
                        },
                        FieldSchema {
                name: FieldId::parse("applied_at_turn").expect("valid field slug"),
                            ty: TypeRef::Named(NamedTypeId::parse("TurnNumber").expect("valid named-type slug")),
                        },
                    ],
                },
            ],
        },
        transitions: vec![],
        surface_only_inputs: vec![
            InputVariantId::parse("SnapshotAligned").expect("valid input-variant slug"),
            InputVariantId::parse("PendingSucceeded").expect("valid input-variant slug"),
            InputVariantId::parse("PendingFailed").expect("valid input-variant slug"),
        ],
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        ci_step_limit: None,
        effect_dispositions: vec![
            EffectDispositionRule {
                effect_variant: EffectVariantId::parse("RefreshVisibleSurfaceSet").expect("valid effect-variant slug"),
                disposition: EffectDisposition::External,
                handoff_protocol: Some(ProtocolId::parse("surface_snapshot_alignment").expect("valid protocol slug")),
            },
            EffectDispositionRule {
                effect_variant: EffectVariantId::parse("ScheduleSurfaceCompletion").expect("valid effect-variant slug"),
                disposition: EffectDisposition::External,
                handoff_protocol: Some(ProtocolId::parse("surface_completion").expect("valid protocol slug")),
            },
        ],
        named_types: vec![
            NamedTypeBinding::u64("TurnNumber"),
            NamedTypeBinding::string("SurfaceId"),
            NamedTypeBinding::string("SurfaceDeltaOperation"),
        ],
    }
}

fn variant(name: &str) -> VariantSchema {
    VariantSchema {
        name: EnumVariantId::parse(name).expect("valid variant slug"),
        fields: vec![],
    }
}
