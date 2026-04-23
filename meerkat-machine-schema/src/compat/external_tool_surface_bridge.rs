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
    RustBinding, StateSchema, TypeRef, VariantSchema,
};

/// Minimal compat machine hosting the surface handoff protocols'
/// producer annotations.
pub fn external_tool_surface_bridge_machine() -> MachineSchema {
    MachineSchema {
        machine: "ExternalToolSurfaceBridgeMachine".into(),
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
                phase: "Idle".into(),
                fields: vec![],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "ExternalToolSurfaceBridgeInput".into(),
            variants: vec![
                VariantSchema {
                    name: "SnapshotAligned".into(),
                    fields: vec![FieldSchema {
                        name: "snapshot_epoch".into(),
                        ty: TypeRef::U64,
                    }],
                },
                VariantSchema {
                    name: "PendingSucceeded".into(),
                    fields: vec![
                        FieldSchema {
                            name: "surface_id".into(),
                            ty: TypeRef::Named("SurfaceId".into()),
                        },
                        FieldSchema {
                            name: "pending_task_sequence".into(),
                            ty: TypeRef::U64,
                        },
                        FieldSchema {
                            name: "staged_intent_sequence".into(),
                            ty: TypeRef::U64,
                        },
                    ],
                },
                VariantSchema {
                    name: "PendingFailed".into(),
                    fields: vec![
                        FieldSchema {
                            name: "surface_id".into(),
                            ty: TypeRef::Named("SurfaceId".into()),
                        },
                        FieldSchema {
                            name: "pending_task_sequence".into(),
                            ty: TypeRef::U64,
                        },
                        FieldSchema {
                            name: "reason".into(),
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
                    name: "RefreshVisibleSurfaceSet".into(),
                    fields: vec![FieldSchema {
                        name: "snapshot_epoch".into(),
                        ty: TypeRef::U64,
                    }],
                },
                VariantSchema {
                    name: "ScheduleSurfaceCompletion".into(),
                    fields: vec![
                        FieldSchema {
                            name: "surface_id".into(),
                            ty: TypeRef::Named("SurfaceId".into()),
                        },
                        FieldSchema {
                            name: "operation".into(),
                            ty: TypeRef::Named("SurfaceDeltaOperation".into()),
                        },
                        FieldSchema {
                            name: "pending_task_sequence".into(),
                            ty: TypeRef::U64,
                        },
                        FieldSchema {
                            name: "staged_intent_sequence".into(),
                            ty: TypeRef::U64,
                        },
                        FieldSchema {
                            name: "applied_at_turn".into(),
                            ty: TypeRef::Named("TurnNumber".into()),
                        },
                    ],
                },
            ],
        },
        transitions: vec![],
        surface_only_inputs: vec![
            "SnapshotAligned".into(),
            "PendingSucceeded".into(),
            "PendingFailed".into(),
        ],
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        ci_step_limit: None,
        effect_dispositions: vec![
            EffectDispositionRule {
                effect_variant: "RefreshVisibleSurfaceSet".into(),
                disposition: EffectDisposition::External,
                handoff_protocol: Some("surface_snapshot_alignment".into()),
            },
            EffectDispositionRule {
                effect_variant: "ScheduleSurfaceCompletion".into(),
                disposition: EffectDisposition::External,
                handoff_protocol: Some("surface_completion".into()),
            },
        ],
    }
}

fn variant(name: &str) -> VariantSchema {
    VariantSchema {
        name: name.into(),
        fields: vec![],
    }
}
