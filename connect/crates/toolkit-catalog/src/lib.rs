//! Canonical generated owners for toolkit-owned application lifecycles.
//!
//! This crate supplies transition authority, not connector selection, policy,
//! credentials, WorkGraph execution, or readiness verification. Production
//! bridges validate concrete values and trusted owner observations before
//! admitting generated inputs, and persist prepared state plus handoffs under
//! their storage fence before realizing effects.

pub mod coverage;
pub mod dsl;
pub mod lock_order;
mod original_models;
mod wire;
pub use dsl::acquisition::*;
pub use dsl::original_offer::*;
pub use dsl::original_types::*;
pub use dsl::source_record::*;
pub use original_models::{
    OriginalModelArtifacts, OriginalRecoveryArtifacts, OriginalRecoveryProfile,
    component_lifetime_model_artifacts, original_offer_model_artifacts,
    original_offer_recovery_artifacts, source_record_model_artifacts,
};

/// Extract the source owner and its bounded vocabulary from the catalog.
pub fn source_record_schema() -> meerkat_machine_schema::MachineSchema {
    let mut schema = ToolkitSourceRecordMachineState::schema();
    schema.named_types.extend([
        meerkat_machine_schema::NamedTypeBinding::string_enum(
            "ToolkitSourceKind",
            ToolkitSourceKind::MODEL_VARIANTS,
        ),
        meerkat_machine_schema::NamedTypeBinding::string_enum(
            "ToolkitSourceReadiness",
            ToolkitSourceReadiness::MODEL_VARIANTS,
        ),
    ]);
    schema
}

/// Receipt provenance remains a bridge obligation; this schema owns no native verdict.
pub fn original_offer_schema() -> meerkat_machine_schema::MachineSchema {
    let mut schema = ToolkitOriginalOfferMachineState::schema();
    schema.named_types.extend([
        meerkat_machine_schema::NamedTypeBinding::string_enum(
            "ToolkitCoreObservation",
            ToolkitCoreObservation::MODEL_VARIANTS,
        ),
        meerkat_machine_schema::NamedTypeBinding::string_enum(
            "ToolkitDispatchAnswer",
            ToolkitDispatchAnswer::MODEL_VARIANTS,
        ),
        meerkat_machine_schema::NamedTypeBinding::string_enum(
            "ToolkitReceiptKind",
            ToolkitReceiptKind::MODEL_VARIANTS,
        ),
        meerkat_machine_schema::NamedTypeBinding::string_enum(
            "ToolkitWithdrawalCause",
            ToolkitWithdrawalCause::MODEL_VARIANTS,
        ),
    ]);
    schema
}

/// The compiled schema is extracted from the same canonical macro invocation
/// that produces the authority consumed by the production bridge.
pub fn acquisition_schema() -> meerkat_machine_schema::MachineSchema {
    let mut schema = ToolkitAcquisitionMachineState::schema();
    // The outcome effect uses the same generated phase enum. Derive its formal
    // named binding from that enum schema rather than maintaining another list.
    let variants = schema
        .state
        .phase
        .variants
        .iter()
        .map(|variant| variant.name.as_str())
        .collect::<Vec<_>>();
    schema
        .named_types
        .push(meerkat_machine_schema::NamedTypeBinding::string_enum(
            &schema.state.phase.name,
            &variants,
        ));
    schema
}

/// Generate bounded model artifacts. Rendering does not execute TLC or prove a
/// production bridge; verification must record those separate checks.
pub fn acquisition_model_artifacts() -> Result<(String, String, String), Box<dyn std::error::Error>>
{
    use meerkat_machine_schema::identity::{FieldId, InputVariantId};
    use meerkat_machine_schema::{InputFieldDomain, InputFieldDomainKind};

    let mut schema = acquisition_schema();
    // Every listed input is guarded by expected_revision == self.revision in
    // the canonical DSL. Declare the echoed state domain explicitly so model
    // exploration can continue after revision 2; names alone select no domain.
    for input in [
        "Admit",
        "RecordSelection",
        "ResolveReady",
        "ReportUnavailable",
        "ReportFailure",
        "Cancel",
        "Expire",
        "AcknowledgeContinuation",
    ] {
        schema.input_field_domains.push(InputFieldDomain {
            input: InputVariantId::parse(input)?,
            field: FieldId::parse("expected_revision")?,
            domain: InputFieldDomainKind::StateField(FieldId::parse("revision")?),
        });
    }
    // Selected requests reach a terminal revision of at least 3. Extend this
    // field's default 0..2 domain without changing generated Rust semantics.
    schema.input_field_domains.push(InputFieldDomain {
        input: InputVariantId::parse("AcknowledgeContinuation")?,
        field: FieldId::parse("terminal_revision")?,
        domain: InputFieldDomainKind::AdditionalValues((3..=8).collect()),
    });
    schema.validate()?;
    Ok((
        meerkat_machine_codegen::render_machine_semantic_model(&schema)?,
        meerkat_machine_codegen::render_machine_ci_cfg(&schema, false)?,
        meerkat_machine_codegen::render_machine_ci_cfg(&schema, true)?,
    ))
}

pub use dsl::component_lifetime::*;

/// Same generated native lifetime schema consumed by NativeHostLifetime.
/// Extraction/validation is separate from model-checking and bridge tests.
pub fn component_lifetime_schema() -> meerkat_machine_schema::MachineSchema {
    ToolkitComponentLifetimeMachineState::schema()
}

#[cfg(test)]
mod component_lifetime_tests {
    use super::*;

    #[test]
    fn component_lifetime_closure_requires_exact_observed_settlement_and_never_reopens() {
        component_lifetime_schema().validate().unwrap();
        let mut owner = ToolkitComponentLifetimeMachineAuthority::new();
        owner
            .apply(ComponentLifetimeInput::Stage {
                incarnation: "incarnation".into(),
                component: "component".into(),
                activation_digest: "digest".into(),
            })
            .unwrap();
        owner
            .apply(ComponentLifetimeInput::Open {
                incarnation: "incarnation".into(),
                expected_revision: owner.state().revision,
            })
            .unwrap();
        owner
            .apply(ComponentLifetimeInput::BeginClose {
                incarnation: "incarnation".into(),
                expected_revision: owner.state().revision,
                close_id: "close".into(),
                reason: "shutdown".into(),
                quarantine: false,
            })
            .unwrap();
        assert_eq!(
            owner.state().lifetime_phase,
            ComponentLifetimePhase::Closing
        );
        assert!(
            owner
                .apply(ComponentLifetimeInput::ConfirmSettlement {
                    incarnation: "other".into(),
                    expected_revision: owner.state().revision,
                    close_id: "close".into(),
                })
                .is_err()
        );
        assert_eq!(
            owner.state().lifetime_phase,
            ComponentLifetimePhase::Closing
        );
        owner
            .apply(ComponentLifetimeInput::ConfirmSettlement {
                incarnation: "incarnation".into(),
                expected_revision: owner.state().revision,
                close_id: "close".into(),
            })
            .unwrap();
        assert_eq!(owner.state().lifetime_phase, ComponentLifetimePhase::Closed);
        assert!(
            owner
                .apply(ComponentLifetimeInput::Open {
                    incarnation: "incarnation".into(),
                    expected_revision: owner.state().revision,
                })
                .is_err()
        );
    }
}
