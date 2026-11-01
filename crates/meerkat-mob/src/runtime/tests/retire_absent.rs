//! The retire of an identity that has no roster incarnation is MobMachine's
//! answer, `RetireAbsent`, in every phase, never the handle's own. The handle
//! reads the roster only to choose the command: a retire that finds no
//! incarnation asks the actor, which applies `RetireAbsent` in its own step
//! and answers what the machine answers. Nothing changes while the identity
//! maps to no runtime, and an identity the machine still maps is refused: a
//! spawn of it committed its membership after the retire read the roster, and
//! the handle answers that refusal as the spawn still in progress, retryable.

use super::*;
use crate::machines::mob_machine as mob_dsl;
use crate::runtime::handle::retire_absent_refusal;

/// Bound for test steps that must happen promptly; it only turns a hang into
/// a failure.
const STEP: Duration = Duration::from_secs(10);

fn retire_absent(identity: &AgentIdentity) -> mob_dsl::MobMachineInput {
    mob_dsl::MobMachineInput::RetireAbsent {
        agent_identity: mob_dsl::AgentIdentity::from_domain(identity),
    }
}

fn machine_state(handle: &MobHandle) -> mob_dsl::MobMachineState {
    handle.machine_state_watch_rx.borrow().clone()
}

fn maps(state: &mob_dsl::MobMachineState, identity: &AgentIdentity) -> bool {
    state
        .identity_to_runtime
        .contains_key(&mob_dsl::AgentIdentity::from_domain(identity))
}

async fn retire(handle: &MobHandle, identity: &AgentIdentity) -> Result<(), MobError> {
    tokio::time::timeout(STEP, handle.retire(identity.clone()))
        .await
        .expect("retire answers")
}

/// A fresh machine authority taken into `phase` the way a mob takes itself
/// there: the lifecycle quiesce for the phase's intent, then its input.
fn authority_in(phase: mob_dsl::MobPhase) -> mob_dsl::MobMachineAuthority {
    let mut authority = mob_dsl::MobMachineAuthority::new();
    let (intent, input) = match phase {
        mob_dsl::MobPhase::Running => return authority,
        mob_dsl::MobPhase::Stopped => (
            mob_dsl::PlacedCompletionLifecycleIntentKind::Stop,
            mob_dsl::MobMachineInput::Stop,
        ),
        mob_dsl::MobPhase::Completed => (
            mob_dsl::PlacedCompletionLifecycleIntentKind::Complete,
            mob_dsl::MobMachineInput::Complete,
        ),
        mob_dsl::MobPhase::Destroyed => (
            mob_dsl::PlacedCompletionLifecycleIntentKind::Destroy,
            mob_dsl::MobMachineInput::Destroy,
        ),
    };
    mob_dsl::MobMachineMutator::apply(
        &mut authority,
        mob_dsl::MobMachineInput::BeginPlacedCompletionLifecycleQuiesce { intent },
    )
    .expect("begin the lifecycle quiesce");
    mob_dsl::MobMachineMutator::apply(&mut authority, input).expect("enter the phase");
    assert_eq!(authority.state().lifecycle_phase, phase);
    authority
}

/// MobMachine answers `RetireAbsent` in every phase, without an effect or a
/// state change. Before the Completed and Destroyed arms it refused the
/// input there, while the handle answered `Ok(())` on its own.
#[test]
fn mob_machine_answers_retire_absent_in_every_phase() {
    let absent = AgentIdentity::from("retire-absent-every-phase");
    for phase in [
        mob_dsl::MobPhase::Running,
        mob_dsl::MobPhase::Stopped,
        mob_dsl::MobPhase::Completed,
        mob_dsl::MobPhase::Destroyed,
    ] {
        let mut authority = authority_in(phase);
        let before = authority.state().clone();
        let transition = mob_dsl::MobMachineMutator::apply(&mut authority, retire_absent(&absent))
            .unwrap_or_else(|error| {
                panic!("MobMachine answers RetireAbsent in {phase:?}: {error:?}")
            });
        assert_eq!(transition.from_phase, phase);
        assert_eq!(transition.to_phase, phase, "RetireAbsent keeps the phase");
        assert!(
            transition.effects().is_empty(),
            "RetireAbsent emits nothing in {phase:?}: {:?}",
            transition.effects()
        );
        assert!(
            *authority.state() == before,
            "RetireAbsent changes no machine state in {phase:?}"
        );
    }
}

/// `RetireAbsent` answers only an identity that maps to no runtime: one the
/// machine still maps is refused, in a running mob and in a completed one
/// alike, while an absent identity of the same completed mob is answered.
#[test]
fn mob_machine_refuses_retire_absent_for_an_identity_it_maps() {
    let mapped = AgentIdentity::from("retire-absent-mapped");
    let absent = AgentIdentity::from("retire-absent-unmapped");
    let mut authority = mob_dsl::MobMachineAuthority::new();
    authority
        .apply_signal(mob_dsl::MobMachineSignal::RecoverRosterMember {
            agent_identity: mob_dsl::AgentIdentity::from_domain(&mapped),
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&AgentRuntimeId::initial(
                mapped.clone(),
            )),
            fence_token: mob_dsl::FenceToken::from_domain(FenceToken::new(1)),
            generation: mob_dsl::Generation::from_domain(crate::ids::Generation::INITIAL),
            profile_name: "worker".to_string(),
            runtime_mode: mob_dsl::SpawnPolicyRuntimeMode::TurnDriven,
            external_addressable: false,
        })
        .expect("recover a roster member");
    mob_dsl::MobMachineMutator::apply(&mut authority, retire_absent(&mapped))
        .expect_err("a running mob refuses RetireAbsent for an identity it maps");

    mob_dsl::MobMachineMutator::apply(
        &mut authority,
        mob_dsl::MobMachineInput::BeginPlacedCompletionLifecycleQuiesce {
            intent: mob_dsl::PlacedCompletionLifecycleIntentKind::Complete,
        },
    )
    .expect("begin the completion quiesce");
    mob_dsl::MobMachineMutator::apply(&mut authority, mob_dsl::MobMachineInput::Complete)
        .expect("complete the mob");
    assert_eq!(
        authority.state().lifecycle_phase,
        mob_dsl::MobPhase::Completed
    );
    assert!(
        authority
            .state()
            .identity_to_runtime
            .contains_key(&mob_dsl::AgentIdentity::from_domain(&mapped)),
        "completion leaves the recovered member mapped"
    );
    mob_dsl::MobMachineMutator::apply(&mut authority, retire_absent(&mapped))
        .expect_err("a completed mob refuses RetireAbsent for an identity it maps");
    mob_dsl::MobMachineMutator::apply(&mut authority, retire_absent(&absent))
        .expect("a completed mob answers RetireAbsent for an absent identity");
}

/// Control: a running and a stopped mob answer the retire of an identity
/// that was never a member through their RetireAbsent arms, and the public
/// retire answers `Ok(())` without changing the machine.
#[tokio::test]
async fn a_running_and_a_stopped_mob_answer_the_retire_of_an_absent_identity() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let absent = AgentIdentity::from("retire-absent-control");
    for phase in [MobState::Running, MobState::Stopped] {
        if phase == MobState::Stopped {
            handle.stop().await.expect("stop the mob");
        }
        assert_eq!(handle.status().await.expect("mob status"), phase);
        let effects = handle
            .apply_machine_input_effects(retire_absent(&absent))
            .await
            .unwrap_or_else(|error| {
                panic!("MobMachine answers RetireAbsent in {phase:?}: {error}")
            });
        assert!(
            effects.is_empty(),
            "RetireAbsent emits nothing: {effects:?}"
        );
        retire(&handle, &absent)
            .await
            .unwrap_or_else(|error| panic!("a {phase:?} mob retires an absent identity: {error}"));
        assert_eq!(handle.status().await.expect("mob status"), phase);
        assert!(!maps(&machine_state(&handle), &absent));
    }
}

/// A completed mob's retire of the member its completion retired is
/// MobMachine's answer, `RetireAbsentCompleted`: the same `Ok(())` the
/// handle used to give without asking the machine, which refused it.
#[tokio::test]
async fn a_completed_mob_answers_the_retire_of_an_absent_member_through_mob_machine() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let worker = AgentIdentity::from("retire-absent-completed");
    handle
        .spawn(ProfileName::from("worker"), worker.clone(), None)
        .await
        .expect("spawn the worker");
    handle.complete().await.expect("complete the mob");
    assert_eq!(
        handle.status().await.expect("mob status"),
        MobState::Completed
    );
    assert!(
        handle.roster.read().await.get(&worker).is_none(),
        "completion retired the worker"
    );
    assert!(!maps(&machine_state(&handle), &worker));

    let effects = handle
        .apply_machine_input_effects(retire_absent(&worker))
        .await
        .expect("MobMachine answers RetireAbsent in a completed mob");
    assert!(
        effects.is_empty(),
        "RetireAbsent emits nothing: {effects:?}"
    );
    retire(&handle, &worker)
        .await
        .expect("a completed mob retires its absent member");
    assert_eq!(
        machine_state(&handle).lifecycle_phase,
        mob_dsl::MobPhase::Completed
    );
}

/// A destroy left incomplete has committed MobMachine's Destroyed phase and
/// keeps the actor serving its retry. The retire of an absent identity there
/// is MobMachine's answer, `RetireAbsentDestroyed`: the same `Ok(())` the
/// handle used to give without asking the machine, which refused it.
#[tokio::test]
async fn an_incompletely_destroyed_mob_answers_the_retire_of_an_absent_identity_through_mob_machine()
 {
    let definition = with_unique_mob_id(sample_definition(), "retire-absent-destroyed");
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let storage = MobStorage::with_events_and_runtime_metadata(
        events.clone(),
        Arc::new(InMemoryMobRuntimeMetadataStore::new()),
    );
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let handle = MobBuilder::new(definition, storage)
        .with_session_service(service)
        .create()
        .await
        .expect("create mob");
    events.fail_clear();
    let error = handle
        .destroy()
        .await
        .expect_err("a failed event clear leaves the destroy incomplete");
    assert!(
        matches!(
            error,
            crate::runtime::handle::MobDestroyError::Incomplete { .. }
        ),
        "expected an incomplete destroy, got {error:?}"
    );
    assert_eq!(
        machine_state(&handle).lifecycle_phase,
        mob_dsl::MobPhase::Destroyed,
        "the incomplete destroy committed MobMachine's Destroyed phase"
    );

    let absent = AgentIdentity::from("retire-absent-destroyed");
    let effects = handle
        .apply_machine_input_effects(retire_absent(&absent))
        .await
        .expect("MobMachine answers RetireAbsent in a destroyed mob");
    assert!(
        effects.is_empty(),
        "RetireAbsent emits nothing: {effects:?}"
    );
    retire(&handle, &absent)
        .await
        .expect("a destroyed mob still serving its destroy retires an absent identity");
    assert_eq!(
        machine_state(&handle).lifecycle_phase,
        mob_dsl::MobPhase::Destroyed
    );

    crash_stop_and_release_routes(handle).await;
}

/// The handle answers the retire of an identity its roster does not list
/// with MobMachine's verdict, never on its own. The test removes a live
/// member's roster anchor and leaves MobMachine mapping the member: the handle
/// finds no incarnation, so it asks `RetireAbsent`, and the machine refuses,
/// because the identity is not absent. The handle used to answer `Ok(())`
/// here without asking the machine. Outside a test, a retire meets that
/// refusal only when a spawn of the identity committed its membership after
/// the retire read the roster, so the handle answers it as that spawn still
/// in progress, `MobError::LifecycleOperationPending`, which is retryable.
/// With the anchor back, the member retires through its exact incarnation,
/// so the refusal started nothing.
#[tokio::test]
async fn the_handle_answers_an_unlisted_identity_with_mob_machine_verdict() {
    let (handle, _service) = create_test_mob(sample_definition()).await;
    let member = AgentIdentity::from("retire-absent-unlisted");
    handle
        .spawn(ProfileName::from("worker"), member.clone(), None)
        .await
        .expect("spawn the member");
    assert!(maps(&machine_state(&handle), &member));

    let listed = handle.roster.read().await.snapshot();
    {
        let mut roster = handle.roster.write().await;
        let mut unlisted = roster.snapshot();
        unlisted.remove(&member);
        *roster = crate::runtime::roster_authority::RosterAuthority::from_roster(unlisted);
    }

    let error = retire(&handle, &member)
        .await
        .expect_err("MobMachine refuses RetireAbsent for an identity it still maps");
    assert!(
        matches!(
            &error,
            MobError::LifecycleOperationPending { intent }
                if intent == "spawn of member retire-absent-unlisted"
        ),
        "the handle answers MobMachine's refusal as the spawn in progress: {error:?}"
    );
    assert!(
        maps(&machine_state(&handle), &member),
        "the refused retire retired nothing"
    );

    *handle.roster.write().await =
        crate::runtime::roster_authority::RosterAuthority::from_roster(listed);
    retire(&handle, &member)
        .await
        .expect("the listed member retires through its exact incarnation");
    assert!(!maps(&machine_state(&handle), &member));
    assert!(handle.roster.read().await.get(&member).is_none());
}

/// The handle answers MobMachine's refusal of `RetireAbsent`, which the actor
/// gives as `InvalidTransition` toward Running from whatever phase the mob is
/// in, as the spawn of the identity still in progress: the machine refuses
/// the input only for an identity it maps, and an identity it maps while the
/// roster lists no incarnation of it is one whose spawn committed its
/// membership after the retire read the roster. The answer is busy on the
/// wire and retryable in its data, and any other answer passes through
/// unchanged.
#[test]
fn the_handle_answers_the_refused_retire_absent_as_the_spawn_in_progress() {
    let member = AgentIdentity::from("retire-absent-racing-spawn");
    for from in [MobState::Running, MobState::Stopped, MobState::Completed] {
        let answer = retire_absent_refusal(
            &member,
            MobError::InvalidTransition {
                from,
                to: MobState::Running,
            },
        );
        assert!(
            matches!(
                &answer,
                MobError::LifecycleOperationPending { intent }
                    if intent == "spawn of member retire-absent-racing-spawn"
            ),
            "a {from:?} mob's refusal is the spawn in progress: {answer:?}"
        );
        assert_eq!(
            answer.wire_error_code(),
            Some(meerkat_contracts::ErrorCode::SessionBusy),
            "callers retry a busy answer: {answer:?}"
        );
        assert_eq!(
            answer
                .structured_data()
                .and_then(|data| data.get("retryable").cloned()),
            Some(serde_json::Value::Bool(true)),
            "the answer's data says it is retryable: {answer:?}"
        );
    }

    let other = retire_absent_refusal(&member, MobError::ActorCommandChannelClosed);
    assert!(
        matches!(other, MobError::ActorCommandChannelClosed),
        "any other answer passes through unchanged: {other:?}"
    );
}
