//! Executor parity for strict guard reads (#1811). SessionDocumentMachine is
//! driven in production through the protocol-codegen authority
//! (`meerkat_core::session_document`) and in tests through the `machine!`
//! mutator. A guard read of an unseeded session must refuse the input in
//! both, never default, and a seeded session must resolve the same way.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_core::session_document as authority;
use meerkat_machine_schema::catalog::dsl::session_document as dsl;

fn projection_disposition_name(effects: &[authority::SessionDocumentEffect]) -> String {
    effects
        .iter()
        .find_map(|effect| match effect {
            authority::SessionDocumentEffect::RuntimeCheckpointProjectionResolved {
                disposition,
            } => Some(format!("{disposition:?}")),
            _ => None,
        })
        .expect("projection effect")
}

#[test]
fn both_executors_refuse_an_unseeded_session() {
    let mut production = authority::SessionDocumentMachineAuthority::new();
    let mut mutator = dsl::SessionDocumentMachineAuthority::new();

    assert!(
        production
            .resolve_runtime_checkpoint_projection(authority::SessionDocumentKey::new("unseeded"))
            .is_err(),
        "the protocol authority refuses an unseeded projection"
    );
    assert!(matches!(
        dsl::SessionDocumentMachineMutator::apply(
            &mut mutator,
            dsl::SessionDocumentInput::ResolveRuntimeCheckpointProjection {
                session_id: dsl::SessionId::from("unseeded"),
            },
        ),
        Err(dsl::SessionDocumentMachineTransitionError::AbsentMapKey { .. })
    ));

    assert!(
        production
            .archive_session_document(
                authority::SessionDocumentKey::new("unseeded"),
                true,
                true,
                authority::SessionArchiveRuntimeObservation::RetirementRequired,
            )
            .is_err(),
        "the protocol authority refuses an unseeded archive"
    );
    assert!(matches!(
        dsl::SessionDocumentMachineMutator::apply(
            &mut mutator,
            dsl::SessionDocumentInput::ArchiveSessionDocument {
                session_id: dsl::SessionId::from("unseeded"),
                runtime_backed: true,
                durable_document_present: true,
                runtime_observation: dsl::SessionArchiveRuntimeObservation::RetirementRequired,
            },
        ),
        Err(dsl::SessionDocumentMachineTransitionError::AbsentMapKey { .. })
    ));

    assert!(
        production
            .mark_session_initial_turn_pending(authority::SessionDocumentKey::new("unseeded"))
            .is_err(),
        "the protocol authority refuses an unseeded first turn"
    );
    assert!(matches!(
        dsl::SessionDocumentMachineMutator::apply(
            &mut mutator,
            dsl::SessionDocumentInput::MarkSessionInitialTurnPending {
                session_id: dsl::SessionId::from("unseeded"),
            },
        ),
        Err(dsl::SessionDocumentMachineTransitionError::AbsentMapKey { .. })
    ));
}

#[test]
fn both_executors_resolve_a_seeded_session_alike() {
    for (production_terminal, mutator_terminal, expected) in [
        (
            authority::SessionDocumentLifecycle::Active,
            dsl::SessionDocumentLifecycle::Active,
            "Project",
        ),
        (
            authority::SessionDocumentLifecycle::Archived,
            dsl::SessionDocumentLifecycle::Archived,
            "IgnoreArchived",
        ),
    ] {
        let mut production = authority::SessionDocumentMachineAuthority::new();
        production
            .recover_session_lifecycle_terminal(
                authority::SessionDocumentKey::new("s"),
                production_terminal,
            )
            .expect("seed the protocol authority");
        let effects = production
            .resolve_runtime_checkpoint_projection(authority::SessionDocumentKey::new("s"))
            .expect("a seeded session resolves");
        assert_eq!(projection_disposition_name(&effects), expected);

        let mut mutator = dsl::SessionDocumentMachineAuthority::new();
        dsl::SessionDocumentMachineMutator::apply(
            &mut mutator,
            dsl::SessionDocumentInput::RecoverSessionLifecycleTerminal {
                session_id: dsl::SessionId::from("s"),
                terminal: mutator_terminal,
            },
        )
        .expect("seed the mutator");
        let transition = dsl::SessionDocumentMachineMutator::apply(
            &mut mutator,
            dsl::SessionDocumentInput::ResolveRuntimeCheckpointProjection {
                session_id: dsl::SessionId::from("s"),
            },
        )
        .expect("a seeded session resolves");
        assert!(
            format!("{:?}", transition.effects()).contains(expected),
            "the mutator resolves {expected} like the protocol authority"
        );
    }
}
