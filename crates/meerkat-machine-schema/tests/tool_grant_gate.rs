//! The scoped tool grant gate in the generated `MeerkatMachine`.
//!
//! Every build replaces the gated names and closes every open name, and a
//! model-request boundary's open set may name only gated names. The machine
//! never reads a grant: the open set is the grant service's adjudication,
//! applied as an input. These tests drive the generated kernel directly, so
//! they hold whatever shell applies the inputs.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeSet;

use meerkat_machine_schema::catalog::dsl::meerkat_machine::{
    MeerkatMachineAuthority, MeerkatMachineEffect, MeerkatMachineInput, MeerkatMachineInputVariant,
    MeerkatMachineMutator, MeerkatMachineSignal, MeerkatMachineTransition,
    MeerkatMachineTransitionError, MeerkatMachineTransitionTrigger, SessionId, ToolName,
};

/// A registered, idle machine: the state a session's builds and boundaries
/// drive.
fn registered_authority() -> MeerkatMachineAuthority {
    let mut authority = MeerkatMachineAuthority::new();
    authority
        .apply_signal(MeerkatMachineSignal::Initialize)
        .expect("machine initializes");
    MeerkatMachineMutator::apply(
        &mut authority,
        MeerkatMachineInput::RegisterSession {
            session_id: SessionId("session-1".to_string()),
            runtime_epoch_id: None,
            initial_run_start_holds: BTreeSet::new(),
        },
    )
    .expect("session registers");
    authority
}

fn names(list: &[&str]) -> BTreeSet<ToolName> {
    list.iter().map(|name| ToolName::from(*name)).collect()
}

fn replace_gate(
    authority: &mut MeerkatMachineAuthority,
    gated: &[&str],
) -> Result<MeerkatMachineTransition, MeerkatMachineTransitionError> {
    MeerkatMachineMutator::apply(
        authority,
        MeerkatMachineInput::ReplaceToolGrantGate {
            gated: names(gated),
        },
    )
}

fn apply_open(
    authority: &mut MeerkatMachineAuthority,
    open: &[&str],
) -> Result<MeerkatMachineTransition, MeerkatMachineTransitionError> {
    MeerkatMachineMutator::apply(
        authority,
        MeerkatMachineInput::ApplyToolGrantOpenSet { open: names(open) },
    )
}

fn refreshes_visible_surface(transition: &MeerkatMachineTransition) -> bool {
    matches!(
        transition.effects(),
        [MeerkatMachineEffect::RefreshVisibleSurfaceSet { .. }]
    )
}

#[test]
fn apply_tool_grant_open_set_refuses_names_outside_the_gate() {
    let mut authority = registered_authority();
    replace_gate(&mut authority, &["shell", "shell_jobs"]).expect("the build installs its gate");

    let opened = apply_open(&mut authority, &["shell"]).expect("a gated name opens");
    assert!(
        refreshes_visible_surface(&opened),
        "an opened name changes the visible set"
    );
    assert_eq!(authority.state().tool_grant_open_names, names(&["shell"]));

    // A set that names an ungated name is refused whole: no arm matches, and
    // the gate stays exactly as it was.
    for refused in [&["read_file"][..], &["shell", "read_file"][..]] {
        let before = authority.state().clone();
        match apply_open(&mut authority, refused) {
            Err(MeerkatMachineTransitionError::GuardRejected { trigger, .. }) => assert_eq!(
                trigger,
                MeerkatMachineTransitionTrigger::Input(
                    MeerkatMachineInputVariant::ApplyToolGrantOpenSet
                )
            ),
            other => panic!("an ungated name must never open: {other:?}"),
        }
        assert_eq!(
            *authority.state(),
            before,
            "a refused open set changes nothing"
        );
    }

    // The same open set again is a no-op that refreshes nothing.
    let unchanged = apply_open(&mut authority, &["shell"]).expect("an unchanged set is admitted");
    assert!(unchanged.effects().is_empty());
    assert_eq!(authority.state().tool_grant_open_names, names(&["shell"]));

    // A boundary that finds no live grant closes every name again.
    let closed = apply_open(&mut authority, &[]).expect("the empty set is within every gate");
    assert!(refreshes_visible_surface(&closed));
    assert!(authority.state().tool_grant_open_names.is_empty());

    // The generated invariant refuses a torn state whose open names left the
    // gate.
    let mut torn = authority.state().clone();
    torn.tool_grant_open_names = names(&["read_file"]);
    match MeerkatMachineAuthority::recover_from_state(torn) {
        Err(MeerkatMachineTransitionError::RecoveredStateInvariantRejected {
            invariant, ..
        }) => assert_eq!(invariant, "tool_grant_open_names_are_gated"),
        other => panic!("open names outside the gate must not recover: {other:?}"),
    }
}

#[test]
fn replace_tool_grant_gate_closes_every_open_name() {
    let mut authority = registered_authority();
    let installed = replace_gate(&mut authority, &["deploy", "shell", "shell_jobs"])
        .expect("the build installs its gate");
    assert!(refreshes_visible_surface(&installed));
    apply_open(&mut authority, &["deploy", "shell"]).expect("gated names open");
    assert_eq!(
        authority.state().tool_grant_open_names,
        names(&["deploy", "shell"])
    );

    // The next build's gate replaces the gated names and closes every open
    // name, the ones it gates again included.
    replace_gate(&mut authority, &["shell"]).expect("a rebuild replaces the gate");
    assert_eq!(authority.state().tool_grant_gated_names, names(&["shell"]));
    assert!(
        authority.state().tool_grant_open_names.is_empty(),
        "a new build starts closed"
    );

    // A name the new gate dropped can no longer open.
    assert!(apply_open(&mut authority, &["deploy"]).is_err());
    apply_open(&mut authority, &["shell"]).expect("a name the new gate keeps opens again");

    // A build without a gate installs the empty gate: it clears the last
    // build's, and nothing can open.
    replace_gate(&mut authority, &[]).expect("a build without a gate clears it");
    assert!(authority.state().tool_grant_gated_names.is_empty());
    assert!(authority.state().tool_grant_open_names.is_empty());
    assert!(apply_open(&mut authority, &["shell"]).is_err());
    apply_open(&mut authority, &[]).expect("the empty set is within every gate");
}
