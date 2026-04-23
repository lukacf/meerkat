//! Compat bridging machine that hosts the `ops_barrier_satisfaction`
//! handoff protocol producer side.
//!
//! The runtime's canonical `MeerkatMachine` emits `WaitAllSatisfied`
//! (field-typed: `wait_request_id`, `operation_ids`) as a local-
//! disposition effect. The protocol, however, needs a producer schema
//! whose `EffectDispositionRule` carries `handoff_protocol = Some(...)`.
//! Rather than extend the DSL macro with handoff-annotation syntax — a
//! cross-crate change with no other callers — this bridge machine
//! mirrors the same effect shape and declares the handoff annotation.
//!
//! This is the same pattern `loop_iteration_machine` uses for
//! `flow_loop_until_evaluation`: a compat machine whose only job is to
//! host the cross-machine protocol declaration. The compat machine is
//! intentionally excluded from the canonical catalog and TLC state
//! space; it exists only for the protocol-codegen producer lookup.

use crate::{
    EffectDisposition, EffectDispositionRule, EnumSchema, FieldSchema, InitSchema, MachineSchema,
    RustBinding, StateSchema, TypeRef, VariantSchema,
};

/// Minimal compat machine that hosts the `ops_barrier_satisfaction`
/// handoff protocol's producer declaration. Its only effect variant is
/// `WaitAllSatisfied { wait_request_id, operation_ids }`, annotated
/// with `handoff_protocol = Some("ops_barrier_satisfaction")`.
pub fn ops_barrier_bridge_machine() -> MachineSchema {
    MachineSchema {
        machine: "OpsBarrierBridgeMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-core".into(),
            module: "ops_lifecycle".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "OpsBarrierBridgePhase".into(),
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
            name: "OpsBarrierBridgeInput".into(),
            variants: vec![VariantSchema {
                name: "OpsBarrierSatisfied".into(),
                fields: vec![
                    FieldSchema {
                        name: "wait_request_id".into(),
                        ty: TypeRef::Named("WaitRequestId".into()),
                    },
                    FieldSchema {
                        name: "operation_ids".into(),
                        ty: TypeRef::Seq(Box::new(TypeRef::Named("OperationId".into()))),
                    },
                ],
            }],
        },
        signals: EnumSchema {
            name: "OpsBarrierBridgeSignal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "OpsBarrierBridgeEffect".into(),
            variants: vec![VariantSchema {
                name: "WaitAllSatisfied".into(),
                fields: vec![
                    FieldSchema {
                        name: "wait_request_id".into(),
                        ty: TypeRef::Named("WaitRequestId".into()),
                    },
                    FieldSchema {
                        name: "operation_ids".into(),
                        ty: TypeRef::Seq(Box::new(TypeRef::Named("OperationId".into()))),
                    },
                ],
            }],
        },
        transitions: vec![],
        surface_only_inputs: vec!["OpsBarrierSatisfied".into()],
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        ci_step_limit: None,
        effect_dispositions: vec![EffectDispositionRule {
            effect_variant: "WaitAllSatisfied".into(),
            disposition: EffectDisposition::External,
            handoff_protocol: Some("ops_barrier_satisfaction".into()),
        }],
    }
}

fn variant(name: &str) -> VariantSchema {
    VariantSchema {
        name: name.into(),
        fields: vec![],
    }
}
