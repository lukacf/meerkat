//! Individual completion and whole-mob idle are different observations.
//!
//! This disposable fixture uses the existing MUTATING `close_if_idle`
//! control. It does not implement or qualify the proposed pure observer.
//! The queued-input control is reused from retained db4869f2e9; this row
//! additionally completes independent member A while member B remains at
//! attempt zero behind the existing runtime queue-authority hook.
//! No global, remote, child-mob or schedule qualification follows. Member
//! status is printed as Idle/work0/Unknown, never asserted as healthy idle.
use super::*;
use crate::runtime::admission_close::CloseIfIdleOutcome;
use meerkat_runtime::{InputLifecycleState, InputTerminalOutcome, SessionServiceRuntimeExt as _};

const STEP: Duration = Duration::from_secs(10);

fn turn_driven_worker(name: &str) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new(ProfileName::from("worker"), AgentIdentity::from(name));
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

#[tokio::test]
async fn individual_completion_does_not_make_queued_work_idle() {
    let (handle, service) = create_test_mob(with_unique_mob_id(
        sample_definition(),
        "whole-idle-individual-completion",
    ))
    .await;
    service.set_return_exact_run_result(true);
    let finished_member = AgentIdentity::from("whole-idle-finished-member");
    let queued_member = AgentIdentity::from("whole-idle-queued-member");
    for member in [&finished_member, &queued_member] {
        handle
            .spawn_spec(turn_driven_worker(member.as_str()))
            .await
            .expect("seat turn-driven member without a kickoff");
    }
    let finished_session = handle
        .resolve_bridge_session_id(&finished_member)
        .await
        .expect("A has a bridge session");
    let queued_session = handle
        .resolve_bridge_session_id(&queued_member)
        .await
        .expect("B has a bridge session");
    assert_ne!(finished_session, queued_session);
    let adapter = MobSessionService::acquire_runtime_adapter(service.as_ref(), None)
        .expect("acquire existing runtime")
        .expect("runtime-backed test mob");
    // This selects the real member-status fallback: Idle with the last
    // recorded work count and Unknown health. It does not forge queue state.
    service.fail_execution_snapshots_for(&queued_session);
    let (queue_entered, queue_release) =
        adapter.arm_runtime_loop_before_queue_authority_test_hook(queued_session.clone());
    let queued_prompt = "B was accepted before A completed";
    let queued_input =
        meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(queued_prompt, None));
    let queued_id = queued_input.id().clone();
    let (queued_admission, queued_completion) = adapter
        .accept_input_with_completion(&queued_session, queued_input)
        .await
        .expect("admit B");
    let entered = tokio::time::timeout(STEP, queue_entered).await;

    let finished_prompt = "A completed while B remained queued";
    let finished_input =
        meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(finished_prompt, None));
    let finished_id = finished_input.id().clone();
    assert_ne!(finished_id, queued_id);
    // B's hook stays held throughout A's admission, exact completion and
    // settlement, and every observation below. One bound detects deadlocks.
    let held = tokio::time::timeout(STEP, async {
        let (finished_admission, completion) = adapter
            .accept_input_with_completion(&finished_session, finished_input)
            .await
            .expect("admit A independently of B's held queue");
        let completed = completion
            .expect("A admission registers exact completion")
            .wait()
            .await;
        adapter.wait_current_run_settled(&finished_session).await;
        (
            finished_admission,
            completed,
            adapter.input_state(&finished_session, &finished_id).await,
            adapter.input_state(&queued_session, &queued_id).await,
            adapter.current_run(&queued_session).await,
            adapter.runtime_state(&queued_session).await,
            handle.member_status(&queued_member).await,
            adapter.session_has_active_inputs(&finished_session).await,
            adapter.session_has_active_inputs(&queued_session).await,
            handle.close_if_idle().await,
            service.recorded_start_turn_prompts().await,
            handle.admission_closed().borrow().is_none(),
        )
    })
    .await;

    // Release even if the held observation timed out, so failed assertions
    // cannot leave this test's runtime blocked. No polling or extra prompt.
    let released = queue_release.send(());
    let completed_b = match queued_completion {
        Some(completion) => Some(tokio::time::timeout(STEP, completion.wait()).await),
        None => None,
    };
    let settled_b =
        tokio::time::timeout(STEP, adapter.wait_current_run_settled(&queued_session)).await;
    let after = tokio::time::timeout(STEP, async {
        (
            adapter.input_state(&finished_session, &finished_id).await,
            adapter.input_state(&queued_session, &queued_id).await,
            adapter.session_has_active_inputs(&finished_session).await,
            adapter.session_has_active_inputs(&queued_session).await,
            service.recorded_start_turn_prompts().await,
        )
    })
    .await;
    // This closes admission on the disposable mob. It is not a pure read.
    let closed = tokio::time::timeout(STEP, handle.close_if_idle()).await;
    let shutdown = tokio::time::timeout(STEP, handle.shutdown()).await;
    shutdown
        .expect("shutdown answers")
        .expect("shutdown succeeds");

    assert!(queued_admission.is_accepted());
    entered
        .expect("B reaches queue hook")
        .expect("queue hook signal");
    released.expect("B hook receiver was alive at release");
    let (
        accepted_a,
        completed_a,
        terminal_a,
        queued_b,
        run_b,
        phase_b,
        member_b,
        active_a_before_close,
        active_b_before_close,
        busy,
        prompts,
        open,
    ) = held.expect("A completes and observations answer while B's queue hook remains held");
    println!(
        "WHOLE_IDLE_HELD a_input={finished_id} b_input={queued_id} a_completion={completed_a:?} a_state={terminal_a:?} b_state={queued_b:?} b_current_run={run_b:?} b_runtime={phase_b:?} member={member_b:?} active_a_before_close={active_a_before_close} active_b_before_close={active_b_before_close} deciding_close={busy:?} prompts={prompts:?} admission_open={open}"
    );
    assert!(accepted_a.is_accepted());
    assert!(
        !active_a_before_close,
        "A has no native active input before the deciding close"
    );
    assert!(
        active_b_before_close,
        "B still has native active input before the deciding close"
    );
    match completed_a.expect("A exact completion authority answers") {
        meerkat_runtime::CompletionOutcome::Completed(result) => {
            assert_eq!(result.text, finished_prompt);
        }
        other => panic!("A must complete successfully: {other:?}"),
    }
    let terminal_a = terminal_a.expect("read A").expect("A is retained");
    assert_eq!(terminal_a.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(terminal_a.seed.attempt_count, 1);
    assert_eq!(
        terminal_a.seed.terminal_outcome,
        Some(InputTerminalOutcome::Consumed)
    );
    let queued_b = queued_b.expect("read B").expect("B is retained");
    assert_eq!(queued_b.seed.phase, InputLifecycleState::Queued);
    assert_eq!(queued_b.seed.attempt_count, 0);
    assert_eq!(queued_b.seed.last_run_id, None);
    assert_eq!(queued_b.seed.terminal_outcome, None);
    assert_eq!(run_b, None);
    assert!(matches!(
        phase_b.expect("read B runtime phase"),
        meerkat_runtime::RuntimeState::Idle | meerkat_runtime::RuntimeState::Attached
    ));
    let member_b = member_b.expect("B status answers");
    assert_eq!(member_b.current_bridge_session_id(), Some(&queued_session));
    let progress = member_b
        .progress
        .expect("runtime fallback supplies progress");
    assert_eq!(
        progress.run_state,
        super::super::handle::MemberRunState::Idle
    );
    assert_eq!(progress.in_flight_work, 0);
    assert_eq!(
        progress.health,
        super::super::handle::MemberHealthClass::Unknown
    );
    assert_eq!(
        prompts,
        vec![(finished_session.clone(), finished_prompt.to_string())]
    );
    assert!(open, "A's completion did not permit closing with B queued");
    match busy.expect("deciding close answers") {
        CloseIfIdleOutcome::Busy(busy) => {
            assert_eq!(
                busy.runs, 1,
                "only B's native queued input keeps this mob busy"
            );
            assert_eq!(busy.roster_transitions, 0);
            assert_eq!(busy.parked_admission, 0);
            assert_eq!(busy.foreground_owners, 0);
        }
        other => panic!("A completed, but B's accepted queue must prevent idle close: {other:?}"),
    }
    match completed_b
        .expect("B admission registered completion")
        .expect("release alone resolves B completion")
        .expect("B completion authority answers")
    {
        meerkat_runtime::CompletionOutcome::Completed(result) => {
            assert_eq!(result.text, queued_prompt);
        }
        other => panic!("B must complete successfully after release: {other:?}"),
    }
    settled_b.expect("B's exact run settles");
    let (final_a, final_b, active_a, active_b, final_prompts) =
        after.expect("final observations answer");
    println!(
        "WHOLE_IDLE_SETTLED a_state={final_a:?} b_state={final_b:?} active_a={active_a} active_b={active_b} prompts={final_prompts:?} final_deciding_close={closed:?}"
    );
    for state in [final_a, final_b] {
        let state = state.expect("read final input").expect("input is retained");
        assert_eq!(state.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(state.seed.attempt_count, 1);
        assert_eq!(
            state.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
    }
    assert!(!active_a);
    assert!(!active_b);
    assert_eq!(
        final_prompts,
        vec![
            (finished_session, finished_prompt.to_string()),
            (queued_session, queued_prompt.to_string()),
        ]
    );
    assert_eq!(
        closed
            .expect("one final close answers")
            .expect("close succeeds"),
        CloseIfIdleOutcome::Closed
    );
}

/// Queued input is work even when no run has begun and the member's status
/// reads Idle with its last recorded in-flight count of 0. Unlike the
/// held-start-turn row, this holds the existing runtime hook before queue
/// authority and staging. The failing optional
/// execution snapshot selects the real runtime-phase fallback for status;
/// no idle snapshot or input lifecycle facts are fabricated by the fixture.
#[tokio::test]
async fn whole_mob_idle_queued_unattempted_input_outlives_idle_member_snapshot() {
    use meerkat_runtime::{
        InputLifecycleState, InputTerminalOutcome, SessionServiceRuntimeExt as _,
    };

    let (handle, service) = create_test_mob(with_unique_mob_id(
        sample_definition(),
        "whole-idle-queued-unattempted",
    ))
    .await;
    let identity = AgentIdentity::from("whole-idle-queued-member");
    service.set_return_exact_run_result(true);
    handle
        .spawn_spec(turn_driven_worker(identity.as_str()))
        .await
        .expect("seat one turn-driven member without a kickoff");
    let session = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("the member has a bridge session");
    let adapter = MobSessionService::acquire_runtime_adapter(service.as_ref(), None)
        .expect("acquire the existing runtime")
        .expect("the test mob is runtime-backed");
    service.fail_execution_snapshots_for(&session);

    let (queue_entered, queue_release) =
        adapter.arm_runtime_loop_before_queue_authority_test_hook(session.clone());
    let prompt = "one queued input beyond an idle member snapshot";
    let input = meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(prompt, None));
    let input_id = input.id().clone();
    let (accepted, completion) = adapter
        .accept_input_with_completion(&session, input)
        .await
        .expect("accept the exact input");
    let entered = tokio::time::timeout(STEP, queue_entered).await;

    // The runtime holds its finalization boundary here, but not queue or
    // driver authority. These ordinary observations must finish before it
    // is released. One deadline turns a lock-order mistake into a failure.
    let held_observation = tokio::time::timeout(STEP, async {
        (
            adapter.input_state(&session, &input_id).await,
            adapter.current_run(&session).await,
            adapter.runtime_state(&session).await,
            handle.member_status(&identity).await,
            handle.close_if_idle().await,
            service.recorded_start_turn_prompts().await,
            handle.admission_closed().borrow().is_none(),
        )
    })
    .await;

    // Release alone permits this input to run. No second prompt, polling
    // loop, manufactured machine transition or second authority is used.
    let released = queue_release.send(());
    let completed = match completion {
        Some(completion) => Some(tokio::time::timeout(STEP, completion.wait()).await),
        None => None,
    };
    let settled = tokio::time::timeout(STEP, adapter.wait_current_run_settled(&session)).await;
    let after_observation = tokio::time::timeout(STEP, async {
        (
            adapter.input_state(&session, &input_id).await,
            adapter.session_has_active_inputs(&session).await,
            service.recorded_start_turn_prompts().await,
        )
    })
    .await;
    // This existing API decides a close. It characterizes the final idle
    // boundary by closing admission.
    let closed = tokio::time::timeout(STEP, handle.close_if_idle()).await;
    let shutdown = tokio::time::timeout(STEP, handle.shutdown()).await;

    shutdown
        .expect("shutdown answers")
        .expect("shutdown succeeds");
    assert!(accepted.is_accepted());
    entered
        .expect("the queue hook is reached")
        .expect("queue hook signal");
    released.expect("the queue hook receiver is alive at release");
    let (queued, run, runtime_phase, member, busy, prompts_before, stayed_open) =
        held_observation.expect("all observations finish while the queue hook is held");
    let queued = queued
        .expect("read the input")
        .expect("the accepted input exists");
    assert_eq!(queued.seed.phase, InputLifecycleState::Queued);
    assert_eq!(queued.seed.attempt_count, 0);
    assert_eq!(queued.seed.last_run_id, None);
    assert_eq!(queued.seed.terminal_outcome, None);
    assert_eq!(run, None);
    assert!(matches!(
        runtime_phase.expect("read actual runtime phase"),
        meerkat_runtime::RuntimeState::Idle | meerkat_runtime::RuntimeState::Attached
    ));
    let member = member.expect("the seated member status answers");
    assert_eq!(member.current_bridge_session_id(), Some(&session));
    let progress = member.progress.expect("runtime fallback supplies progress");
    assert_eq!(
        progress.run_state,
        super::super::handle::MemberRunState::Idle
    );
    assert_eq!(progress.in_flight_work, 0);
    assert_eq!(
        progress.health,
        super::super::handle::MemberHealthClass::Unknown
    );
    assert!(
        prompts_before.is_empty(),
        "no turn has reached the session service"
    );
    assert!(stayed_open);
    match busy.expect("the busy observation answers") {
        CloseIfIdleOutcome::Busy(busy) => {
            assert_eq!(busy.runs, 1, "only native queued input keeps the mob busy");
            assert_eq!(busy.roster_transitions, 0);
            assert_eq!(busy.parked_admission, 0);
            assert_eq!(busy.foreground_owners, 0);
        }
        other => panic!("queued input must prevent idle close: {other:?}"),
    }
    let completed = completed
        .expect("acceptance registered a completion")
        .expect("release alone resolves the completion")
        .expect("completion authority answers");
    match completed {
        meerkat_runtime::CompletionOutcome::Completed(result) => assert_eq!(result.text, prompt),
        other => panic!("the queued input must complete successfully: {other:?}"),
    }
    settled.expect("the exact run settles");
    let (terminal, active_after, prompts_after) =
        after_observation.expect("terminal observations finish");
    let terminal = terminal
        .expect("read terminal input")
        .expect("input is retained");
    assert_eq!(terminal.seed.phase, InputLifecycleState::Consumed);
    assert_eq!(
        terminal.seed.terminal_outcome,
        Some(InputTerminalOutcome::Consumed)
    );
    assert_eq!(terminal.seed.attempt_count, 1);
    assert!(!active_after);
    assert_eq!(prompts_after, vec![(session, prompt.to_string())]);
    assert_eq!(
        closed
            .expect("one final close answers")
            .expect("close succeeds"),
        CloseIfIdleOutcome::Closed
    );
}
