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
//! The compat machine is intentionally excluded from the canonical catalog
//! and TLC state space; it exists only for the protocol-codegen producer
//! lookup.

use crate::identity::{
    EffectVariantId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, MachineId, NamedTypeId,
    PhaseId, ProtocolId, TransitionId,
};
use crate::{
    EffectDisposition, EffectDispositionRule, EnumSchema, FieldSchema, InitSchema, MachineSchema,
    NamedTypeBinding, RustBinding, StateSchema, TypeRef, VariantSchema,
};

/// Minimal compat machine that hosts the `ops_barrier_satisfaction`
/// handoff protocol's producer declaration. Its only effect variant is
/// `WaitAllSatisfied { wait_request_id, operation_ids }`, annotated
/// with `handoff_protocol = Some("ops_barrier_satisfaction")`.
pub fn ops_barrier_bridge_machine() -> MachineSchema {
    MachineSchema {
        machine: MachineId::parse("OpsBarrierBridgeMachine").expect("valid machine slug"),
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
                phase: PhaseId::parse("Idle").expect("valid phase slug"),
                fields: vec![],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "OpsBarrierBridgeInput".into(),
            variants: vec![VariantSchema {
                name: EnumVariantId::parse("OpsBarrierSatisfied").expect("valid variant slug"),
                fields: vec![
                    FieldSchema {
                        name: FieldId::parse("wait_request_id").expect("valid field slug"),
                        ty: TypeRef::Named(
                            NamedTypeId::parse("WaitRequestId").expect("valid named-type slug"),
                        ),
                    },
                    FieldSchema {
                        name: FieldId::parse("operation_ids").expect("valid field slug"),
                        ty: TypeRef::Seq(Box::new(TypeRef::Named(
                            NamedTypeId::parse("OperationId").expect("valid named-type slug"),
                        ))),
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
                name: EnumVariantId::parse("WaitAllSatisfied").expect("valid variant slug"),
                fields: vec![
                    FieldSchema {
                        name: FieldId::parse("wait_request_id").expect("valid field slug"),
                        ty: TypeRef::Named(
                            NamedTypeId::parse("WaitRequestId").expect("valid named-type slug"),
                        ),
                    },
                    FieldSchema {
                        name: FieldId::parse("operation_ids").expect("valid field slug"),
                        ty: TypeRef::Seq(Box::new(TypeRef::Named(
                            NamedTypeId::parse("OperationId").expect("valid named-type slug"),
                        ))),
                    },
                ],
            }],
        },
        transitions: vec![],
        surface_only_inputs: vec![
            InputVariantId::parse("OpsBarrierSatisfied").expect("valid input-variant slug"),
        ],
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        ci_step_limit: None,
        effect_dispositions: vec![EffectDispositionRule {
            effect_variant: EffectVariantId::parse("WaitAllSatisfied")
                .expect("valid effect-variant slug"),
            disposition: EffectDisposition::External,
            handoff_protocol: Some(
                ProtocolId::parse("ops_barrier_satisfaction").expect("valid protocol slug"),
            ),
        }],
        named_types: vec![
            NamedTypeBinding::string("WaitRequestId"),
            NamedTypeBinding::string("OperationId"),
        ],
    }
}

fn variant(name: &str) -> VariantSchema {
    VariantSchema {
        name: EnumVariantId::parse(name).expect("valid variant slug"),
        fields: vec![],
    }
}
