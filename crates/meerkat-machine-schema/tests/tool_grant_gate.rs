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
    AgentRuntimeId, FenceToken, Generation, MeerkatMachineAuthority, MeerkatMachineEffect,
    MeerkatMachineInput, MeerkatMachineInputVariant, MeerkatMachineMutator, MeerkatMachineSignal,
    MeerkatMachineState, MeerkatMachineTransition, MeerkatMachineTransitionError,
    MeerkatMachineTransitionTrigger, RunId, SessionId, SessionResidencyOwner, ToolName,
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

fn servers(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|server| (*server).to_string()).collect()
}

fn replace_gate(
    authority: &mut MeerkatMachineAuthority,
    gated: &[&str],
) -> Result<MeerkatMachineTransition, MeerkatMachineTransitionError> {
    install_gate(authority, gated, &[], false, &[], &[])
}

/// A build's whole gate: its names, its servers, the unconfigured-servers
/// mode, the servers it configures ungated and the role entries it ignored.
fn install_gate(
    authority: &mut MeerkatMachineAuthority,
    gated: &[&str],
    gated_servers: &[&str],
    gates_unconfigured_servers: bool,
    ungated_servers: &[&str],
    not_mounted_names: &[&str],
) -> Result<MeerkatMachineTransition, MeerkatMachineTransitionError> {
    MeerkatMachineMutator::apply(
        authority,
        MeerkatMachineInput::ReplaceToolGrantGate {
            gated: names(gated),
            gated_servers: servers(gated_servers),
            gates_unconfigured_servers,
            ungated_servers: servers(ungated_servers),
            not_mounted_names: names(not_mounted_names),
        },
    )
}

fn extend(
    authority: &mut MeerkatMachineAuthority,
    server: &str,
    gated: &[&str],
) -> Result<MeerkatMachineTransition, MeerkatMachineTransitionError> {
    MeerkatMachineMutator::apply(
        authority,
        MeerkatMachineInput::ExtendToolGrantGate {
            server: server.to_string(),
            gated: names(gated),
        },
    )
}

fn stage_gated(
    authority: &mut MeerkatMachineAuthority,
    server: &str,
) -> Result<MeerkatMachineTransition, MeerkatMachineTransitionError> {
    MeerkatMachineMutator::apply(
        authority,
        MeerkatMachineInput::SurfaceStageGatedAdd {
            surface_id: server.to_string(),
            now_ms: 1,
        },
    )
}

fn refused_by(
    result: Result<MeerkatMachineTransition, MeerkatMachineTransitionError>,
    input: MeerkatMachineInputVariant,
) {
    match result {
        Err(MeerkatMachineTransitionError::GuardRejected { trigger, .. }) => {
            assert_eq!(trigger, MeerkatMachineTransitionTrigger::Input(input));
        }
        other => panic!("{input:?} must be refused: {other:?}"),
    }
}

fn invariant_refuses(state: MeerkatMachineState, invariant: &str) {
    match MeerkatMachineAuthority::recover_from_state(state) {
        Err(MeerkatMachineTransitionError::RecoveredStateInvariantRejected {
            invariant: rejected,
            ..
        }) => assert_eq!(rejected, invariant),
        Err(other) => panic!("{invariant} must refuse the state: {other:?}"),
        Ok(_) => panic!("{invariant} must refuse the state, but it recovered"),
    }
}

fn stage_recorded(transition: &MeerkatMachineTransition, server: &str) -> bool {
    transition.effects().iter().any(|effect| {
        matches!(
            effect,
            MeerkatMachineEffect::SurfaceStageRecorded { surface_id, .. } if surface_id == server
        )
    })
}

/// The registered machine in each phase the grant path stages in: Idle, then
/// Attached once a runtime binds, then Running once a run starts.
fn authorities_in_staging_phases() -> Vec<(&'static str, MeerkatMachineAuthority)> {
    let attach = |authority: &mut MeerkatMachineAuthority| {
        MeerkatMachineMutator::apply(
            authority,
            MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: AgentRuntimeId("runtime-1".to_string()),
                fence_token: FenceToken(1),
                generation: Some(Generation(1)),
                runtime_epoch_id: None,
                session_id: SessionId("session-1".to_string()),
                residency_owner: SessionResidencyOwner::Session,
            },
        )
        .expect("a runtime binds");
    };
    let idle = registered_authority();
    let mut attached = registered_authority();
    attach(&mut attached);
    let mut running = registered_authority();
    attach(&mut running);
    MeerkatMachineMutator::apply(
        &mut running,
        MeerkatMachineInput::Prepare {
            session_id: SessionId("session-1".to_string()),
            run_id: RunId("run-1".to_string()),
        },
    )
    .expect("a run starts");
    vec![("Idle", idle), ("Attached", attached), ("Running", running)]
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
        Err(other) => panic!("open names outside the gate must not recover: {other:?}"),
        Ok(_) => panic!("open names outside the gate must not recover, but the state recovered"),
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

/// A build's gate installs its servers, the unconfigured-servers mode, the
/// servers it configures ungated and the role entries it ignored, and closes
/// every open name. A gate whose ignored names it also gates, or whose
/// ungated servers are gated servers, is refused whole, and the generated
/// invariants refuse a torn state that says either.
#[test]
fn replace_installs_the_server_gate_and_refuses_overlapping_sets() {
    let mut authority = registered_authority();
    replace_gate(&mut authority, &["shell"]).expect("a names-only gate");
    apply_open(&mut authority, &["shell"]).expect("a gated name opens");

    let installed = install_gate(
        &mut authority,
        &["deploy"],
        &["tickets"],
        true,
        &["notes"],
        &["shell"],
    )
    .expect("the build installs its server gate");
    assert!(refreshes_visible_surface(&installed));
    let state = authority.state();
    assert_eq!(state.tool_grant_gated_names, names(&["deploy"]));
    assert_eq!(state.tool_grant_gated_servers, servers(&["tickets"]));
    assert!(state.tool_grant_gates_unconfigured_servers);
    assert_eq!(state.tool_grant_ungated_servers, servers(&["notes"]));
    assert_eq!(state.tool_grant_not_mounted_names, names(&["shell"]));
    assert!(
        state.tool_grant_open_names.is_empty(),
        "a new build starts closed"
    );

    // An ignored name the gate also gates, and an ungated server the gate
    // also names: each gate is refused whole.
    for (gated, gated_servers, ungated, ignored) in [
        (&["shell"][..], &[][..], &[][..], &["shell"][..]),
        (&[][..], &["tickets"][..], &["tickets"][..], &[][..]),
    ] {
        let before = authority.state().clone();
        refused_by(
            install_gate(&mut authority, gated, gated_servers, true, ungated, ignored),
            MeerkatMachineInputVariant::ReplaceToolGrantGate,
        );
        assert_eq!(*authority.state(), before, "a refused gate changes nothing");
    }

    let mut torn = authority.state().clone();
    torn.tool_grant_not_mounted_names = names(&["deploy"]);
    invariant_refuses(torn, "tool_grant_not_mounted_are_not_gated");
    let mut torn = authority.state().clone();
    torn.tool_grant_ungated_servers = servers(&["tickets"]);
    invariant_refuses(torn, "tool_grant_ungated_servers_are_not_gated");
}

/// A model-request boundary gates the names a gated server provides now: the
/// gated names grow, a name the build ignored that such a server provides
/// becomes gated, and no name opens or closes. Only a server the gate gates
/// (by name, or by the mode outside its ungated servers) extends it, only a
/// registered session's, and only a new build's gate shrinks the names.
#[test]
fn extend_grows_the_gate_for_a_gated_server_only() {
    let mut authority = registered_authority();
    install_gate(
        &mut authority,
        &["deploy"],
        &["tickets"],
        false,
        &[],
        &["close_ticket"],
    )
    .expect("the build installs its gate");
    apply_open(&mut authority, &["deploy"]).expect("a gated name opens");

    let extended = extend(&mut authority, "tickets", &["open_ticket", "close_ticket"])
        .expect("a gated server's names join the gate");
    assert!(refreshes_visible_surface(&extended));
    let state = authority.state();
    assert_eq!(
        state.tool_grant_gated_names,
        names(&["close_ticket", "deploy", "open_ticket"])
    );
    assert!(
        state.tool_grant_not_mounted_names.is_empty(),
        "an ignored name a gated server provides becomes gated"
    );
    assert_eq!(
        state.tool_grant_open_names,
        names(&["deploy"]),
        "an extension opens nothing and closes nothing"
    );

    let unchanged = extend(&mut authority, "tickets", &["open_ticket"])
        .expect("names already gated are admitted");
    assert!(unchanged.effects().is_empty());

    // A server the gate does not gate never extends it.
    let before = authority.state().clone();
    refused_by(
        extend(&mut authority, "notes", &["read_note"]),
        MeerkatMachineInputVariant::ExtendToolGrantGate,
    );
    assert_eq!(*authority.state(), before);

    // With the mode on, a server outside the ungated ones extends it, and an
    // ungated one stays ungated.
    install_gate(&mut authority, &[], &[], true, &["notes"], &[]).expect("a mode gate");
    extend(&mut authority, "calendar", &["list_events"]).expect("the mode gates an unnamed server");
    refused_by(
        extend(&mut authority, "notes", &["read_note"]),
        MeerkatMachineInputVariant::ExtendToolGrantGate,
    );

    // Only a new build's gate shrinks the names.
    install_gate(&mut authority, &[], &[], false, &[], &[]).expect("an empty gate");
    assert!(authority.state().tool_grant_gated_names.is_empty());

    // Before a session registers, nothing extends.
    let mut unregistered = MeerkatMachineAuthority::new();
    unregistered
        .apply_signal(MeerkatMachineSignal::Initialize)
        .expect("machine initializes");
    assert!(extend(&mut unregistered, "tickets", &["open_ticket"]).is_err());
}

/// The grant path stages only a server the gate gates, in Idle, Attached and
/// Running, recording the stage's lineage as the ordinary stage does: a
/// named server, or with the mode on a server outside the ungated ones. The
/// ordinary stage still stages any server.
#[test]
fn surface_stage_gated_add_stages_a_gated_server_only() {
    for (phase, mut authority) in authorities_in_staging_phases() {
        install_gate(&mut authority, &[], &["tickets"], false, &[], &[])
            .unwrap_or_else(|error| panic!("{phase}: the build installs its gate: {error:?}"));
        let staged = stage_gated(&mut authority, "tickets")
            .unwrap_or_else(|error| panic!("{phase}: a named server stages: {error:?}"));
        assert!(stage_recorded(&staged, "tickets"), "{phase}: {staged:?}");
        refused_by(
            stage_gated(&mut authority, "notes"),
            MeerkatMachineInputVariant::SurfaceStageGatedAdd,
        );

        install_gate(&mut authority, &[], &[], true, &["notes"], &[])
            .unwrap_or_else(|error| panic!("{phase}: a mode gate: {error:?}"));
        let staged = stage_gated(&mut authority, "calendar")
            .unwrap_or_else(|error| panic!("{phase}: the mode gates an unnamed server: {error:?}"));
        assert!(stage_recorded(&staged, "calendar"), "{phase}: {staged:?}");
        refused_by(
            stage_gated(&mut authority, "notes"),
            MeerkatMachineInputVariant::SurfaceStageGatedAdd,
        );

        // The ordinary stage is unchanged: it stages a server the gate does
        // not gate.
        let ordinary = MeerkatMachineMutator::apply(
            &mut authority,
            MeerkatMachineInput::SurfaceStageAdd {
                surface_id: "notes".to_string(),
                now_ms: 1,
            },
        )
        .unwrap_or_else(|error| panic!("{phase}: the ordinary stage: {error:?}"));
        assert!(stage_recorded(&ordinary, "notes"), "{phase}: {ordinary:?}");
    }
}
