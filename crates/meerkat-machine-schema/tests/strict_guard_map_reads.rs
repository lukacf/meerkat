//! Strict map reads in guards (#1811): a guard's value-projected map read
//! never defaults. Before, the generated mutator read an absent key as the
//! type's default (`unwrap_or_default`) while TLA+ projected it to "none",
//! so an unseeded session document took the `Active` arm in Rust (projecting
//! a runtime checkpoint, archiving and retiring a runtime) where TLC and the
//! fail-closed protocol authority refused. Every executor now refuses.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_machine_schema::Expr;
use meerkat_machine_schema::catalog::dsl::dsl_session_document_machine_production_schema;
use meerkat_machine_schema::catalog::dsl::session_document::{
    SessionArchiveRuntimeObservation, SessionDocumentEffect, SessionDocumentInput,
    SessionDocumentLifecycle, SessionDocumentMachineAuthority, SessionDocumentMachineMutator,
    SessionDocumentMachineTransitionError, SessionId,
};

fn absent_key_field(
    result: Result<impl std::fmt::Debug, SessionDocumentMachineTransitionError>,
) -> &'static str {
    match result {
        Err(SessionDocumentMachineTransitionError::AbsentMapKey { field, .. }) => field,
        other => panic!("expected an absent-key refusal, got {other:?}"),
    }
}

#[test]
fn an_unseeded_session_is_refused_not_defaulted() {
    let mut authority = SessionDocumentMachineAuthority::new();

    let projection = SessionDocumentMachineMutator::apply(
        &mut authority,
        SessionDocumentInput::ResolveRuntimeCheckpointProjection {
            session_id: SessionId::from("unseeded"),
        },
    );
    assert_eq!(absent_key_field(projection), "session_lifecycle_terminal");

    let archive = SessionDocumentMachineMutator::apply(
        &mut authority,
        SessionDocumentInput::ArchiveSessionDocument {
            session_id: SessionId::from("unseeded"),
            runtime_backed: true,
            durable_document_present: true,
            runtime_observation: SessionArchiveRuntimeObservation::RetirementRequired,
        },
    );
    assert_eq!(absent_key_field(archive), "session_lifecycle_terminal");
    assert!(
        authority.state().session_lifecycle_terminal.is_empty(),
        "a refused archive records nothing"
    );

    let first_turn = SessionDocumentMachineMutator::apply(
        &mut authority,
        SessionDocumentInput::MarkSessionInitialTurnPending {
            session_id: SessionId::from("unseeded"),
        },
    );
    assert_eq!(absent_key_field(first_turn), "session_first_turn_phase");
}

#[test]
fn a_seeded_session_reads_its_stored_value() {
    let mut authority = SessionDocumentMachineAuthority::new();
    SessionDocumentMachineMutator::apply(
        &mut authority,
        SessionDocumentInput::RecoverSessionLifecycleTerminal {
            session_id: SessionId::from("s"),
            terminal: SessionDocumentLifecycle::Archived,
        },
    )
    .expect("recover-seed");
    let transition = SessionDocumentMachineMutator::apply(
        &mut authority,
        SessionDocumentInput::ResolveRuntimeCheckpointProjection {
            session_id: SessionId::from("s"),
        },
    )
    .expect("a seeded session resolves");
    let rendered = format!("{:?}", transition.effects());
    assert!(
        rendered.contains("IgnoreArchived"),
        "an Archived session never projects: {rendered}"
    );
    assert!(
        !transition
            .effects()
            .iter()
            .any(|effect| matches!(effect, SessionDocumentEffect::RuntimeCheckpointProjectionResolved { disposition } if format!("{disposition:?}") == "Project")),
        "an Archived session never projects"
    );
}

fn contains_map_value(expr: &Expr) -> bool {
    format!("{expr:?}").contains("MapValue {")
}

#[test]
fn the_schema_carries_the_strict_read_for_tla_and_the_authorities() {
    let schema = dsl_session_document_machine_production_schema();
    for name in [
        "ResolveRuntimeCheckpointProjectionActive",
        "ArchiveSessionDocumentActive",
        "MarkSessionInitialTurnPendingInactiveOrPending",
    ] {
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.name.as_str() == name)
            .unwrap_or_else(|| panic!("transition {name}"));
        assert!(
            transition
                .guards
                .iter()
                .any(|guard| contains_map_value(&guard.expr)),
            "{name} must read its map strictly"
        );
    }
}
