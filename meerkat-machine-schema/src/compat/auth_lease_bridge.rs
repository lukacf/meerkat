//! Compat bridging machine that hosts the `auth_lease_lifecycle_publication`
//! handoff protocol's producer annotation.
//!
//! Pattern mirror of `ops_barrier_bridge` and `supervisor_trust_bridge`:
//! the canonical `AuthMachine` (`catalog::dsl::auth_machine`) already
//! emits `EmitLifecycleEvent { new_state }` as an `External`-disposition
//! effect on every lifecycle-phase transition. The runtime's
//! `RuntimeAuthLeaseHandle` owner observes those events to drive the
//! per-binding projection consumed by provider callbacks and the
//! refresh/reauth policy.
//!
//! That seam is informative (AuthMachine's phase is already the
//! authoritative fact; the owner does not feed any state back into the
//! DSL), so the handoff protocol is modelled as an `EffectExtractor`
//! with zero declared feedback inputs. The composition declaration's
//! role is to close the seam in the closed-world composition registry,
//! not to request a feedback contract.
//!
//! The canonical `AuthMachine` cannot itself host the
//! `handoff_protocol = Some(...)` disposition annotation today — the
//! `machine!` DSL macro grammar does not express the annotation. Rather
//! than extend the macro across the workspace for this single-field
//! informative seam, this compat bridge mirrors the effect shape
//! (`PublishLifecycleEvent { new_state: AuthLifecyclePhase }`) and
//! carries the handoff annotation. A Route in
//! `auth_lease_bundle_composition` declaratively binds the canonical
//! AuthMachine `EmitLifecycleEvent` to the bridge's mirror input so the
//! producer path is covered by the composition registry.
//!
//! Intentionally excluded from the canonical catalog and TLC state
//! space; it exists only for the protocol-codegen producer lookup and
//! the composition registry's cross-machine coverage.

use crate::identity::{
    EffectVariantId, EnumVariantId, FieldId, InputVariantId, MachineId, NamedTypeId, PhaseId,
    ProtocolId,
};
use crate::{
    EffectDisposition, EffectDispositionRule, EnumSchema, FieldSchema, InitSchema, MachineSchema,
    NamedTypeBinding, RustBinding, StateSchema, TypeRef, VariantSchema,
};

/// Minimal compat machine hosting the `auth_lease_lifecycle_publication`
/// handoff protocol's producer annotation. The single effect variant,
/// `PublishLifecycleEvent { new_state }`, mirrors the canonical
/// `AuthMachine::EmitLifecycleEvent` payload shape. The matching
/// `MirrorLifecycleEvent` input lets the composition Route carry the
/// canonical AuthMachine effect into the bridge so the Route's
/// producer/consumer validation closes.
pub fn auth_lease_bridge_machine() -> MachineSchema {
    MachineSchema {
        machine: MachineId::parse("AuthLeaseBridgeMachine").expect("valid machine slug"),
        version: 1,
        rust: RustBinding {
            // The effect is actually emitted by the canonical AuthMachine
            // at `meerkat-machine-schema/src/catalog/dsl/auth_machine.rs`;
            // this binding points at the runtime owner (`handles::auth_lease`)
            // that realises the publication obligation.
            crate_name: "meerkat-runtime".into(),
            module: "handles::auth_lease".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "AuthLeaseBridgePhase".into(),
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
            name: "AuthLeaseBridgeInput".into(),
            variants: vec![VariantSchema {
                name: EnumVariantId::parse("MirrorLifecycleEvent").expect("valid variant slug"),
                fields: vec![FieldSchema {
                    name: FieldId::parse("new_state").expect("valid field slug"),
                    ty: TypeRef::Named(
                        NamedTypeId::parse("AuthLifecyclePhase").expect("valid named-type slug"),
                    ),
                }],
            }],
        },
        signals: EnumSchema {
            name: "AuthLeaseBridgeSignal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "AuthLeaseBridgeEffect".into(),
            variants: vec![VariantSchema {
                name: EnumVariantId::parse("PublishLifecycleEvent").expect("valid variant slug"),
                fields: vec![FieldSchema {
                    name: FieldId::parse("new_state").expect("valid field slug"),
                    ty: TypeRef::Named(
                        NamedTypeId::parse("AuthLifecyclePhase").expect("valid named-type slug"),
                    ),
                }],
            }],
        },
        transitions: vec![],
        surface_only_inputs: vec![
            InputVariantId::parse("MirrorLifecycleEvent").expect("valid input-variant slug"),
        ],
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        ci_step_limit: None,
        effect_dispositions: vec![EffectDispositionRule {
            effect_variant: EffectVariantId::parse("PublishLifecycleEvent")
                .expect("valid effect-variant slug"),
            disposition: EffectDisposition::External,
            handoff_protocol: Some(
                ProtocolId::parse("auth_lease_lifecycle_publication").expect("valid protocol slug"),
            ),
        }],
        named_types: vec![NamedTypeBinding::type_path(
            "AuthLifecyclePhase",
            "crate::catalog::dsl::auth_machine::AuthLifecyclePhase",
        )],
    }
}

fn variant(name: &str) -> VariantSchema {
    VariantSchema {
        name: EnumVariantId::parse(name).expect("valid variant slug"),
        fields: vec![],
    }
}
