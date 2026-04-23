//! Compat bridging machine that hosts the supervisor-trust-edge
//! publication/revocation handoff protocols.
//!
//! Pattern mirror of `external_tool_surface_bridge` and
//! `ops_barrier_bridge`: `MeerkatMachine` already owns the supervisor
//! binding fact (`supervisor_binding_kind` + `supervisor_bound_*`), but
//! the companion trust-edge mutation in `meerkat-comms::Router` is a
//! separate shell step today. The `BindSupervisor` /
//! `AuthorizeSupervisor` / `RevokeSupervisor` transitions accept first;
//! the shell (`comms_drain`) then calls `router.add_trusted_peer` /
//! `remove_trusted_peer` as a separate step and rolls back the DSL on
//! trust-publication failure. That split ownership is row F2 in
//! `docs/wave-c-prep/state-scope-audit.md` — tracked by
//! `meerkat-runtime/src/meerkat_machine/dsl.rs` DSL comment `:114-134`.
//!
//! C-F2 formalises the step-lock as a generated obligation pair:
//!
//! - `supervisor_trust_publish` — producer emits
//!   `PublishSupervisorTrustEdge`; owner realises via
//!   `router.add_trusted_peer(...)`; owner feeds back one of
//!   `SupervisorTrustEdgePublished` (success) or
//!   `SupervisorTrustEdgePublishFailed` (failure, triggers DSL
//!   rollback).
//! - `supervisor_trust_revoke` — producer emits
//!   `RevokeSupervisorTrustEdge`; owner realises via
//!   `router.remove_trusted_peer(...)`; owner feeds back one of
//!   `SupervisorTrustEdgeRevoked` (success) or
//!   `SupervisorTrustEdgeRevokeFailed` (failure, logged — revoke is
//!   best-effort once the DSL has accepted the revocation).
//!
//! Intentionally excluded from the canonical catalog and TLC state
//! space; it exists only for the protocol-codegen producer lookup and
//! the seam-inventory handoff-protocol registration. The canonical
//! `MeerkatMachine` continues to own the authoritative
//! `supervisor_binding_*` state; this bridge mirrors the effect shape
//! so the composition schema has a producer with a
//! `handoff_protocol = Some(...)` disposition rule.

use crate::identity::{
    EffectVariantId, EnumVariantId, FieldId, InputVariantId, MachineId, NamedTypeId, PhaseId,
    ProtocolId,
};
use crate::{
    EffectDisposition, EffectDispositionRule, EnumSchema, FieldSchema, InitSchema, MachineSchema,
    NamedTypeBinding, RustBinding, StateSchema, TypeRef, VariantSchema,
};

/// Minimal compat machine hosting the supervisor-trust-edge handoff
/// protocols' producer annotations.
///
/// Two effects, both annotated with `handoff_protocol`:
/// - `PublishSupervisorTrustEdge` → `supervisor_trust_publish`
/// - `RevokeSupervisorTrustEdge` → `supervisor_trust_revoke`
///
/// Four feedback inputs surface-only: the two success acks and the two
/// failure acks. The owner (`supervisor_bridge_owner`) realises each
/// effect by calling into `meerkat-comms::Router` and emits the
/// matching feedback input through the generated protocol helper.
pub fn supervisor_trust_bridge_machine() -> MachineSchema {
    MachineSchema {
        machine: MachineId::parse("SupervisorTrustBridgeMachine").expect("valid machine slug"),
        version: 1,
        rust: RustBinding {
            // The effects are actually emitted from `comms_drain` against
            // `meerkat-comms::Router`; the bridge points at the runtime
            // module that owns the publication/revocation path.
            crate_name: "meerkat-runtime".into(),
            module: "comms_drain".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "SupervisorTrustBridgePhase".into(),
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
            name: "SupervisorTrustBridgeInput".into(),
            variants: vec![
                VariantSchema {
                    name: EnumVariantId::parse("SupervisorTrustEdgePublished")
                        .expect("valid variant slug"),
                    fields: vec![
                        FieldSchema {
                            name: FieldId::parse("peer_id").expect("valid field slug"),
                            ty: TypeRef::Named(
                                NamedTypeId::parse("SupervisorPeerId")
                                    .expect("valid named-type slug"),
                            ),
                        },
                        FieldSchema {
                            name: FieldId::parse("epoch").expect("valid field slug"),
                            ty: TypeRef::U64,
                        },
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("SupervisorTrustEdgePublishFailed")
                        .expect("valid variant slug"),
                    fields: vec![
                        FieldSchema {
                            name: FieldId::parse("peer_id").expect("valid field slug"),
                            ty: TypeRef::Named(
                                NamedTypeId::parse("SupervisorPeerId")
                                    .expect("valid named-type slug"),
                            ),
                        },
                        FieldSchema {
                            name: FieldId::parse("epoch").expect("valid field slug"),
                            ty: TypeRef::U64,
                        },
                        FieldSchema {
                            name: FieldId::parse("reason").expect("valid field slug"),
                            ty: TypeRef::String,
                        },
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("SupervisorTrustEdgeRevoked")
                        .expect("valid variant slug"),
                    fields: vec![
                        FieldSchema {
                            name: FieldId::parse("peer_id").expect("valid field slug"),
                            ty: TypeRef::Named(
                                NamedTypeId::parse("SupervisorPeerId")
                                    .expect("valid named-type slug"),
                            ),
                        },
                        FieldSchema {
                            name: FieldId::parse("epoch").expect("valid field slug"),
                            ty: TypeRef::U64,
                        },
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("SupervisorTrustEdgeRevokeFailed")
                        .expect("valid variant slug"),
                    fields: vec![
                        FieldSchema {
                            name: FieldId::parse("peer_id").expect("valid field slug"),
                            ty: TypeRef::Named(
                                NamedTypeId::parse("SupervisorPeerId")
                                    .expect("valid named-type slug"),
                            ),
                        },
                        FieldSchema {
                            name: FieldId::parse("epoch").expect("valid field slug"),
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
            name: "SupervisorTrustBridgeSignal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "SupervisorTrustBridgeEffect".into(),
            variants: vec![
                VariantSchema {
                    name: EnumVariantId::parse("PublishSupervisorTrustEdge")
                        .expect("valid variant slug"),
                    fields: vec![
                        FieldSchema {
                            name: FieldId::parse("peer_id").expect("valid field slug"),
                            ty: TypeRef::Named(
                                NamedTypeId::parse("SupervisorPeerId")
                                    .expect("valid named-type slug"),
                            ),
                        },
                        FieldSchema {
                            name: FieldId::parse("name").expect("valid field slug"),
                            ty: TypeRef::String,
                        },
                        FieldSchema {
                            name: FieldId::parse("address").expect("valid field slug"),
                            ty: TypeRef::String,
                        },
                        FieldSchema {
                            name: FieldId::parse("epoch").expect("valid field slug"),
                            ty: TypeRef::U64,
                        },
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("RevokeSupervisorTrustEdge")
                        .expect("valid variant slug"),
                    fields: vec![
                        FieldSchema {
                            name: FieldId::parse("peer_id").expect("valid field slug"),
                            ty: TypeRef::Named(
                                NamedTypeId::parse("SupervisorPeerId")
                                    .expect("valid named-type slug"),
                            ),
                        },
                        FieldSchema {
                            name: FieldId::parse("epoch").expect("valid field slug"),
                            ty: TypeRef::U64,
                        },
                    ],
                },
            ],
        },
        transitions: vec![],
        surface_only_inputs: vec![
            InputVariantId::parse("SupervisorTrustEdgePublished")
                .expect("valid input-variant slug"),
            InputVariantId::parse("SupervisorTrustEdgePublishFailed")
                .expect("valid input-variant slug"),
            InputVariantId::parse("SupervisorTrustEdgeRevoked").expect("valid input-variant slug"),
            InputVariantId::parse("SupervisorTrustEdgeRevokeFailed")
                .expect("valid input-variant slug"),
        ],
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        ci_step_limit: None,
        effect_dispositions: vec![
            EffectDispositionRule {
                effect_variant: EffectVariantId::parse("PublishSupervisorTrustEdge")
                    .expect("valid effect-variant slug"),
                disposition: EffectDisposition::External,
                handoff_protocol: Some(
                    ProtocolId::parse("supervisor_trust_publish").expect("valid protocol slug"),
                ),
            },
            EffectDispositionRule {
                effect_variant: EffectVariantId::parse("RevokeSupervisorTrustEdge")
                    .expect("valid effect-variant slug"),
                disposition: EffectDisposition::External,
                handoff_protocol: Some(
                    ProtocolId::parse("supervisor_trust_revoke").expect("valid protocol slug"),
                ),
            },
        ],
        named_types: vec![NamedTypeBinding::string("SupervisorPeerId")],
    }
}

fn variant(name: &str) -> VariantSchema {
    VariantSchema {
        name: EnumVariantId::parse(name).expect("valid variant slug"),
        fields: vec![],
    }
}
