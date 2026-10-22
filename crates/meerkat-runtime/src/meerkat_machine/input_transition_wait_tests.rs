//! A cancellation that meets an input between lifecycle steps (neither queued
//! nor terminal, with no run to cancel) re-reads it when the input's next step
//! commits, not on a timer (#1655). The paused clock makes that observable:
//! no virtual time passes between the commit and the answer. A step committed
//! without the signal costs the cancellation its settle bound, never a false
//! settle error, and the subscription it takes under the driver lock it reads
//! with cannot miss a step committed by the lock's next holder.
//!
//! The persistent variants step and consume the input through the persistent
//! driver's inner driver, so they pin the delegation of the step subscription,
//! not a durable commit.
//!
//! The cancellations' settle deadline is a std-clock instant, which the paused
//! clock does not move. The paused-clock bound tests therefore cover the read
//! a timed-out wait falls through to, and the real-clock tests past the bound
//! cover a read that starts after the deadline. Under a paused clock, a
//! cancellation that never settles keeps re-parking (each wait auto-advances
//! about 5 s) until 5 real seconds pass, as the released 25 ms poll did.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::*;
use std::time::Duration;

use chrono::Utc;

fn keyed_prompt(key: &str) -> Input {
    Input::Prompt(crate::input::PromptInput {
        injected_context: Vec::new(),
        header: crate::input::InputHeader {
            ingress_context: None,
            retained_resume: None,
            authority_association: None,
            id: InputId::new(),
            timestamp: Utc::now(),
            source: crate::input::InputOrigin::Operator,
            durability: crate::input::InputDurability::Durable,
            visibility: crate::input::InputVisibility::default(),
            idempotency_key: Some(crate::identifiers::IdempotencyKey::new(key)),
            supersession_key: None,
            correlation_id: None,
        },
        content: "held between lifecycle steps".into(),
        typed_turn_appends: Vec::new(),
        turn_metadata: None,
    })
}

fn persistent_machine() -> Arc<MeerkatMachine> {
    let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
    Arc::new(
        MeerkatMachine::persistent(store, Arc::new(meerkat_store::MemoryBlobStore::new()))
            .expect("persistent machine"),
    )
}

async fn session_driver(
    machine: &MeerkatMachine,
    session_id: &SessionId,
) -> crate::meerkat_machine::driver::SharedDriver {
    machine
        .sessions
        .read()
        .await
        .get(session_id)
        .expect("registered session")
        .driver
        .clone()
}

/// Accept one keyed prompt (it stays queued: no executor serves the session)
/// and step it to applied with no run: the state a cancellation must wait out.
///
/// The admission hands the session mutation gate to its accepted boundary
/// work, which releases it on the process-owned runtime, out of the paused
/// clock's sight. The gate is taken once before the step, so that work is
/// done before any cancellation starts: a cancellation started while it still
/// held the gate would wait for the gate, not for the input.
async fn input_between_steps(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    key: &str,
) -> InputId {
    let input = keyed_prompt(key);
    let input_id = input.id().clone();
    <MeerkatMachine as SessionServiceRuntimeExt>::accept_input(machine, session_id, input)
        .await
        .expect("accept the prompt");
    take_the_gate_once(machine, session_id).await;
    session_driver(machine, session_id)
        .await
        .lock()
        .await
        .apply_without_run_for_test(&input_id)
        .expect("step the input to applied without a run");
    input_id
}

/// Take the session mutation gate and give it back at once. The gate is first
/// come, first served, so its holder and every waiter queued on it before
/// this call have had their turn.
async fn take_the_gate_once(machine: &MeerkatMachine, session_id: &SessionId) {
    drop(
        machine
            .session_mutation_gate(session_id)
            .await
            .expect("registered session gate")
            .lock_settled(SettledGateWait::TestFixture)
            .await,
    );
}

/// Let every other task run until it is idle. The paused clock advances only
/// then, so a cancellation started before this call has read the input and
/// parked by the time it returns, as long as it waits for nothing off this
/// runtime: `input_between_steps` leaves no admission work holding the gate.
async fn until_parked() {
    crate::tokio::time::sleep(Duration::from_millis(1)).await;
}

/// Commit the input's terminal: consume it, as a completed run's boundary
/// does. Returns the paused-clock instant of the commit.
async fn consume(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    input_id: &InputId,
) -> crate::tokio::time::Instant {
    session_driver(machine, session_id)
        .await
        .lock()
        .await
        .consume_inputs_for_test(std::slice::from_ref(input_id), &RunId::new())
        .expect("consume the applied input");
    crate::tokio::time::Instant::now()
}

/// The cancellations' settle bound.
const SETTLE: Duration = Duration::from_secs(5);

/// What the admission's accepted boundary work still holds when a bound row
/// starts its cancellation. The admission hands the session mutation gate to
/// that work, which releases it on the process-owned runtime, out of the
/// paused clock's sight.
#[derive(Clone, Copy)]
enum BoundaryWork {
    /// Nothing the row arranges: the work runs as it does.
    Unheld,
    /// The work keeps the gate until the row's unsignalled commit, or until
    /// `SETTLE * 10` of paused time has passed since it started.
    HeldUntilTheCommit,
}

/// Hold the next admission's accepted boundary work, and with it the session
/// mutation gate, until the returned sender fires or `SETTLE * 10` of paused
/// time has passed since the work started.
fn hold_next_boundary_work(machine: &MeerkatMachine) -> crate::tokio::sync::oneshot::Sender<()> {
    let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
    *machine
        .test_accepted_boundary_work_hold
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((entered_tx, release_rx));
    let (commit_tx, commit_rx) = crate::tokio::sync::oneshot::channel::<()>();
    crate::tokio::spawn(async move {
        let _ = entered_rx.await;
        crate::tokio::select! {
            _ = commit_rx => {}
            () = crate::tokio::time::sleep(SETTLE * 10) => {}
        }
        let _ = release_tx.send(());
    });
    commit_tx
}

/// Release the held boundary work after the row's commit, then take the gate
/// once, so a cancellation queued on it has had its turn.
async fn release_after_the_commit(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    commit: crate::tokio::sync::oneshot::Sender<()>,
) {
    let _ = commit.send(());
    take_the_gate_once(machine, session_id).await;
}

/// Commit the input's terminal without the step signal: apply the generated
/// consume straight on the shared machine authority, as a lifecycle apply
/// outside the driver's commit point would.
async fn consume_without_signal(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    input_id: &InputId,
) {
    let authority = session_driver(machine, session_id)
        .await
        .lock()
        .await
        .shared_dsl_authority();
    let mut authority = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    dsl::MeerkatMachineMutator::apply(
        &mut *authority,
        dsl::MeerkatMachineInput::ConsumeInput {
            input_id: input_id.to_string(),
        },
    )
    .expect("consume on the shared machine authority");
}

/// Publish a member residency for the session, as a mob's host does before it
/// cancels a tracked input.
async fn resident_member(
    machine: &Arc<MeerkatMachine>,
    session_id: &SessionId,
) -> meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
    let member = meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
        mob_id: "input-transition-wait".to_string(),
        agent_identity: "worker".to_string(),
        host_id: "host-a".to_string(),
        binding_generation: 1,
        member_session_id: session_id.to_string(),
        generation: 0,
        fence_token: 1,
    };
    let residency = machine
        .begin_member_residency_update(session_id.clone())
        .await
        .commit(member.clone(), None)
        .await
        .expect("publish the member residency");
    drop(residency);
    member
}

async fn exact_cancellation_waits_for_the_committed_step(machine: Arc<MeerkatMachine>) {
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let input_id = input_between_steps(&machine, &session_id, "exact-between-steps").await;

    // The owned body that `cancel_input_if_present` runs on the process-owned
    // cleanup runtime, driven here on the test's own runtime so the paused
    // clock governs its waits.
    let cancel = crate::tokio::spawn({
        let machine = Arc::clone(&machine);
        let session_id = session_id.clone();
        let input_id = input_id.clone();
        async move {
            machine
                .cancel_input_if_present_owned(
                    &session_id,
                    &input_id,
                    "cancelled between steps".to_string(),
                )
                .await
        }
    });
    until_parked().await;
    assert!(
        !cancel.is_finished(),
        "an input between steps holds the cancellation until a step commits"
    );

    let committed = consume(&machine, &session_id, &input_id).await;
    let observed = cancel
        .await
        .expect("cancellation task")
        .expect("the cancellation settles on the committed terminal");
    assert!(observed, "the consumed input is observed");
    assert_eq!(
        crate::tokio::time::Instant::now() - committed,
        Duration::ZERO,
        "the committed terminal, not a timer, orders the re-read"
    );
}

#[tokio::test(start_paused = true)]
async fn ephemeral_exact_cancellation_waits_for_the_committed_step() {
    exact_cancellation_waits_for_the_committed_step(Arc::new(MeerkatMachine::ephemeral())).await;
}

#[tokio::test(start_paused = true)]
async fn persistent_exact_cancellation_waits_for_the_committed_step() {
    exact_cancellation_waits_for_the_committed_step(persistent_machine()).await;
}

async fn tracked_cancellation_waits_for_the_committed_step(machine: Arc<MeerkatMachine>) {
    const KEY: &str = "tracked-between-steps";
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let member = resident_member(&machine, &session_id).await;
    let input_id = input_between_steps(&machine, &session_id, KEY).await;

    let cancel = crate::tokio::spawn({
        let machine = Arc::clone(&machine);
        let session_id = session_id.clone();
        async move {
            machine
                .cancel_tracked_input_for_member_incarnation(&session_id, KEY, &member)
                .await
        }
    });
    until_parked().await;
    assert!(
        !cancel.is_finished(),
        "an input between steps holds the tracked cancellation until a step commits"
    );

    let committed = consume(&machine, &session_id, &input_id).await;
    cancel
        .await
        .expect("tracked cancellation task")
        .expect("the tracked cancellation settles on the committed terminal");
    assert_eq!(
        crate::tokio::time::Instant::now() - committed,
        Duration::ZERO,
        "the committed terminal, not a timer, orders the tracked re-read"
    );
}

#[tokio::test(start_paused = true)]
async fn ephemeral_tracked_cancellation_waits_for_the_committed_step() {
    tracked_cancellation_waits_for_the_committed_step(Arc::new(MeerkatMachine::ephemeral())).await;
}

#[tokio::test(start_paused = true)]
async fn persistent_tracked_cancellation_waits_for_the_committed_step() {
    tracked_cancellation_waits_for_the_committed_step(persistent_machine()).await;
}

async fn exact_cancellation_reads_again_at_the_bound(
    machine: Arc<MeerkatMachine>,
    boundary_work: BoundaryWork,
) {
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let commit = match boundary_work {
        BoundaryWork::Unheld => None,
        BoundaryWork::HeldUntilTheCommit => Some(hold_next_boundary_work(&machine)),
    };
    let input_id = input_between_steps(&machine, &session_id, "exact-unsignalled").await;

    let cancel = crate::tokio::spawn({
        let machine = Arc::clone(&machine);
        let session_id = session_id.clone();
        let input_id = input_id.clone();
        async move {
            machine
                .cancel_input_if_present_owned(
                    &session_id,
                    &input_id,
                    "cancelled between steps".to_string(),
                )
                .await
        }
    });
    until_parked().await;
    consume_without_signal(&machine, &session_id, &input_id).await;
    if let Some(commit) = commit {
        release_after_the_commit(&machine, &session_id, commit).await;
    }
    crate::tokio::time::sleep(SETTLE / 2).await;
    assert!(
        !cancel.is_finished(),
        "a terminal committed without the signal does not wake the cancellation"
    );

    let observed = cancel
        .await
        .expect("cancellation task")
        .expect("the read after the bound finds the terminal: no false settle error");
    assert!(observed, "the consumed input is observed");
}

/// R1: a terminal committed without the step signal costs the exact
/// cancellation its settle bound, then the read after it answers.
#[tokio::test(start_paused = true)]
async fn ephemeral_exact_cancellation_reads_again_at_the_bound() {
    exact_cancellation_reads_again_at_the_bound(
        Arc::new(MeerkatMachine::ephemeral()),
        BoundaryWork::Unheld,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn persistent_exact_cancellation_reads_again_at_the_bound() {
    exact_cancellation_reads_again_at_the_bound(persistent_machine(), BoundaryWork::Unheld).await;
}

/// R1 behind the admission's boundary work: while that work holds the
/// session mutation gate, a cancellation waits for the gate, not for the
/// input. Held until the unsignalled commit, the cancellation would read the
/// input only after that commit and answer at once, as the unheld rows did
/// now and then. `input_between_steps` waits the work out, so here the hold
/// ends on its paused-time bound before the cancellation starts.
#[tokio::test(start_paused = true)]
async fn ephemeral_exact_cancellation_reads_again_at_the_bound_behind_boundary_work() {
    exact_cancellation_reads_again_at_the_bound(
        Arc::new(MeerkatMachine::ephemeral()),
        BoundaryWork::HeldUntilTheCommit,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn persistent_exact_cancellation_reads_again_at_the_bound_behind_boundary_work() {
    exact_cancellation_reads_again_at_the_bound(
        persistent_machine(),
        BoundaryWork::HeldUntilTheCommit,
    )
    .await;
}

async fn tracked_cancellation_reads_again_at_the_bound(
    machine: Arc<MeerkatMachine>,
    boundary_work: BoundaryWork,
) {
    const KEY: &str = "tracked-unsignalled";
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let member = resident_member(&machine, &session_id).await;
    let commit = match boundary_work {
        BoundaryWork::Unheld => None,
        BoundaryWork::HeldUntilTheCommit => Some(hold_next_boundary_work(&machine)),
    };
    let input_id = input_between_steps(&machine, &session_id, KEY).await;

    let cancel = crate::tokio::spawn({
        let machine = Arc::clone(&machine);
        let session_id = session_id.clone();
        async move {
            machine
                .cancel_tracked_input_for_member_incarnation(&session_id, KEY, &member)
                .await
        }
    });
    until_parked().await;
    consume_without_signal(&machine, &session_id, &input_id).await;
    if let Some(commit) = commit {
        release_after_the_commit(&machine, &session_id, commit).await;
    }
    crate::tokio::time::sleep(SETTLE / 2).await;
    assert!(
        !cancel.is_finished(),
        "a terminal committed without the signal does not wake the tracked cancellation"
    );

    cancel
        .await
        .expect("tracked cancellation task")
        .expect("the read after the bound finds the terminal: no false settle error");
}

/// R1: a terminal committed without the step signal costs the tracked
/// cancellation its settle bound, then the read after it answers.
#[tokio::test(start_paused = true)]
async fn ephemeral_tracked_cancellation_reads_again_at_the_bound() {
    tracked_cancellation_reads_again_at_the_bound(
        Arc::new(MeerkatMachine::ephemeral()),
        BoundaryWork::Unheld,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn persistent_tracked_cancellation_reads_again_at_the_bound() {
    tracked_cancellation_reads_again_at_the_bound(persistent_machine(), BoundaryWork::Unheld).await;
}

/// R1 for the tracked cancellation behind the admission's boundary work; see
/// `ephemeral_exact_cancellation_reads_again_at_the_bound_behind_boundary_work`.
#[tokio::test(start_paused = true)]
async fn ephemeral_tracked_cancellation_reads_again_at_the_bound_behind_boundary_work() {
    tracked_cancellation_reads_again_at_the_bound(
        Arc::new(MeerkatMachine::ephemeral()),
        BoundaryWork::HeldUntilTheCommit,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn persistent_tracked_cancellation_reads_again_at_the_bound_behind_boundary_work() {
    tracked_cancellation_reads_again_at_the_bound(
        persistent_machine(),
        BoundaryWork::HeldUntilTheCommit,
    )
    .await;
}

async fn exact_cancellation_after_the_bound_answers_a_settled_terminal(
    machine: Arc<MeerkatMachine>,
) {
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let input_id = input_between_steps(&machine, &session_id, "exact-after-the-bound").await;
    consume(&machine, &session_id, &input_id).await;

    let observed = machine
        .cancel_input_if_present_owned_before(
            &session_id,
            &input_id,
            "cancelled after the bound".to_string(),
            meerkat_core::time_compat::Instant::now(),
        )
        .await
        .expect("a terminal with nothing owed needs no wait, so a spent bound is no error");
    assert!(observed, "the consumed input is observed");
}

/// A read that starts after the settle deadline (the one a wait that reached
/// it falls through to) answers a terminal input with no runless terminal
/// owed its publication at once, instead of failing the publication drain's
/// spent deadline.
#[tokio::test]
async fn ephemeral_exact_cancellation_after_the_bound_answers_a_settled_terminal() {
    exact_cancellation_after_the_bound_answers_a_settled_terminal(Arc::new(
        MeerkatMachine::ephemeral(),
    ))
    .await;
}

#[tokio::test]
async fn persistent_exact_cancellation_after_the_bound_answers_a_settled_terminal() {
    exact_cancellation_after_the_bound_answers_a_settled_terminal(persistent_machine()).await;
}

/// A publication handle the held dispatch below never reaches: its start
/// permit is never sent.
struct UnreachedPublisher;

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutorPublicationHandle for UnreachedPublisher {
    async fn publish_interaction_terminals(
        &self,
        _events: &[meerkat_core::event::AgentEvent],
    ) -> Result<
        Vec<meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt>,
        meerkat_core::lifecycle::core_executor::CoreExecutorError,
    > {
        panic!("the held dispatch never starts its publication")
    }
}

/// Past the settle deadline, a consumed input whose session driver still has
/// a runless terminal dispatch pending keeps the drain: the cancellation
/// reports the publication outstanding, never success.
#[tokio::test]
async fn exact_cancellation_after_the_bound_reports_a_pending_dispatch() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let input_id = input_between_steps(&machine, &session_id, "exact-held-dispatch").await;
    consume(&machine, &session_id, &input_id).await;
    let (driver, completions, mutation_gate) = {
        let sessions = machine.sessions.read().await;
        let entry = sessions.get(&session_id).expect("registered session");
        (
            entry.driver.clone(),
            entry.completions.clone(),
            entry.mutation_gate.clone(),
        )
    };
    let (_dispatch_result, start_permit) = machine
        .prepare_runless_terminal_publication_dispatch(
            &driver,
            &completions,
            &mutation_gate,
            Arc::new(UnreachedPublisher),
        )
        .expect("hold a runless terminal dispatch slot");

    let outcome = machine
        .cancel_input_if_present_owned_before(
            &session_id,
            &input_id,
            "cancelled after the bound".to_string(),
            meerkat_core::time_compat::Instant::now(),
        )
        .await;
    // Unsent, the permit ends the held dispatch, which clears its slot.
    drop(start_permit);
    assert!(
        matches!(
            outcome,
            Err(RuntimeDriverError::RuntimeTerminalPublicationInProgress { .. })
        ),
        "a pending dispatch keeps the drain at the spent deadline: {outcome:?}"
    );
}

/// Past the settle deadline, an abandoned directed input whose committed
/// runless terminal carrier is not yet published keeps the drain: the
/// cancellation reports the carrier outstanding, never success.
#[tokio::test]
async fn exact_cancellation_after_the_bound_reports_an_unpublished_carrier() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    // A runless terminal carrier names the runtime placement it terminates.
    machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("runtime binding");
    let interaction = meerkat_core::time_compat::new_uuid_v7().to_string();
    let input = crate::mob_adapter::create_tracked_flow_step_input(
        "unpublished-carrier",
        meerkat_core::types::ContentInput::Text("abandoned before it runs".to_string()),
        "input-transition-wait-flow",
        None,
        &interaction,
    )
    .expect("directed input");
    let input_id = input.id().clone();
    {
        // Admitted on the driver itself: the machine's admission refuses
        // directed input to a session with no publication capability, and the
        // drain under test is the one without a publication handle.
        let driver = session_driver(&machine, &session_id).await;
        let mut driver = driver.lock().await;
        assert!(
            driver
                .as_driver_mut()
                .accept_input(input)
                .await
                .expect("accept the directed input")
                .is_accepted()
        );
        let prepared = driver
            .prepare_runless_runtime_terminated_interaction_outboxes(
                std::slice::from_ref(&input_id),
                "abandoned before it runs".to_string(),
            )
            .expect("stage the runless terminal carrier");
        driver
            .abandon_queued_input(&input_id, crate::input_state::InputAbandonReason::Cancelled)
            .await
            .expect("abandon the queued input");
        assert_eq!(
            crate::meerkat_machine::driver::DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared),
            Some(input_id.clone()),
            "the abandoned input owns an unpublished carrier"
        );
    }

    let outcome = machine
        .cancel_input_if_present_owned_before(
            &session_id,
            &input_id,
            "cancelled after the bound".to_string(),
            meerkat_core::time_compat::Instant::now(),
        )
        .await;
    assert!(
        matches!(
            &outcome,
            Err(RuntimeDriverError::Internal(reason))
                if reason.contains("did not converge before its deadline")
        ),
        "an unpublished carrier keeps the drain at the spent deadline: {outcome:?}"
    );
}

/// Queue a consume behind a cancellation on the session's driver lock, which
/// the test holds, then release it. The driver mutex is FIFO, so the
/// cancellation reads the input between steps first and the consumer commits
/// its terminal right after. Returns the paused-clock instant of that commit.
async fn consume_after_the_queued_read(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    input_id: &InputId,
    held: crate::tokio::sync::MutexGuard<'_, crate::meerkat_machine::driver::DriverEntry>,
) -> crate::tokio::time::Instant {
    let driver = session_driver(machine, session_id).await;
    let consumer = crate::tokio::spawn({
        let input_id = input_id.clone();
        async move {
            driver
                .lock()
                .await
                .consume_inputs_for_test(std::slice::from_ref(&input_id), &RunId::new())
                .expect("consume the applied input");
            crate::tokio::time::Instant::now()
        }
    });
    until_parked().await;
    assert!(
        !consumer.is_finished(),
        "the consumer queues on the held driver"
    );
    drop(held);
    consumer.await.expect("consumer task")
}

/// L2: the exact cancellation subscribes under the driver lock it reads the
/// input with, so the step the lock's next holder commits wakes it.
#[tokio::test(start_paused = true)]
async fn exact_cancellation_subscribes_under_its_read_lock() {
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let input_id = input_between_steps(&machine, &session_id, "exact-fifo").await;
    let driver = session_driver(&machine, &session_id).await;
    let held = driver.lock().await;

    let cancel = crate::tokio::spawn({
        let machine = Arc::clone(&machine);
        let session_id = session_id.clone();
        let input_id = input_id.clone();
        async move {
            machine
                .cancel_input_if_present_owned(
                    &session_id,
                    &input_id,
                    "cancelled between steps".to_string(),
                )
                .await
        }
    });
    until_parked().await;
    assert!(
        !cancel.is_finished(),
        "the cancellation queues on the held driver"
    );

    let committed = consume_after_the_queued_read(&machine, &session_id, &input_id, held).await;
    let observed = cancel
        .await
        .expect("cancellation task")
        .expect("the cancellation settles on the committed terminal");
    assert!(observed, "the consumed input is observed");
    assert_eq!(
        crate::tokio::time::Instant::now() - committed,
        Duration::ZERO,
        "the step committed right after the read wakes the cancellation"
    );
}

/// L2: the tracked cancellation subscribes under the driver lock it reads
/// the input with, so the step the lock's next holder commits wakes it.
#[tokio::test(start_paused = true)]
async fn tracked_cancellation_subscribes_under_its_read_lock() {
    const KEY: &str = "tracked-fifo";
    let machine = Arc::new(MeerkatMachine::ephemeral());
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register session");
    let member = resident_member(&machine, &session_id).await;
    let input_id = input_between_steps(&machine, &session_id, KEY).await;
    let driver = session_driver(&machine, &session_id).await;
    let held = driver.lock().await;

    let cancel = crate::tokio::spawn({
        let machine = Arc::clone(&machine);
        let session_id = session_id.clone();
        async move {
            machine
                .cancel_tracked_input_for_member_incarnation(&session_id, KEY, &member)
                .await
        }
    });
    until_parked().await;
    assert!(
        !cancel.is_finished(),
        "the tracked cancellation queues on the held driver"
    );

    let committed = consume_after_the_queued_read(&machine, &session_id, &input_id, held).await;
    cancel
        .await
        .expect("tracked cancellation task")
        .expect("the tracked cancellation settles on the committed terminal");
    assert_eq!(
        crate::tokio::time::Instant::now() - committed,
        Duration::ZERO,
        "the step committed right after the read wakes the tracked cancellation"
    );
}
