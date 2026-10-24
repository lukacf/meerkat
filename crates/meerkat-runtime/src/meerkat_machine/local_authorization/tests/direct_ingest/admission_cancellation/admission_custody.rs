//! API-dependent mechanical custody cut tests. RPC owns real staged seed cleanup.
use super::*;
use crate::input_admission_custody::{
    NativeInputAdmissionCompletion, NativeInputAdmissionCustody,
    NativeInputAdmissionSettlement as Settlement,
};
use crate::store::{InMemoryRuntimeStore, RuntimeStore};
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

type Settlements = Arc<StdMutex<Vec<Settlement>>>;

struct ObservedCustody(Settlements);

impl NativeInputAdmissionCustody for ObservedCustody {
    fn settle(self: Box<Self>, settlement: Settlement) -> Option<NativeInputAdmissionCompletion> {
        self.0.lock().unwrap().push(settlement);
        None
    }
}

fn custody() -> (Box<dyn NativeInputAdmissionCustody>, Settlements) {
    let observed = Arc::new(StdMutex::new(Vec::new()));
    (Box::new(ObservedCustody(observed.clone())), observed)
}

struct CompletionCustody {
    settlements: Settlements,
    cleanup_ran: Arc<AtomicBool>,
    completed: tokio::sync::oneshot::Sender<
        Result<
            crate::completion::CompletionCleanupObservation,
            crate::completion::CompletionWaitError,
        >,
    >,
}

impl NativeInputAdmissionCustody for CompletionCustody {
    fn settle(self: Box<Self>, settlement: Settlement) -> Option<NativeInputAdmissionCompletion> {
        self.settlements.lock().unwrap().push(settlement);
        if settlement != Settlement::Admitted {
            return None;
        }
        let Self {
            cleanup_ran,
            completed,
            ..
        } = *self;
        Some(Box::new(move |observation| {
            Box::pin(async move {
                cleanup_ran.store(true, Ordering::SeqCst);
                let _ = completed.send(observation);
                Ok(())
            })
        }))
    }
}

fn observed(log: &Settlements) -> Vec<Settlement> {
    log.lock().unwrap().clone()
}

async fn submit_with_custody(
    machine: Arc<MeerkatMachine>,
    witness: crate::RuntimeExecutorAttachmentWitness,
    input: Input,
    custody: Box<dyn NativeInputAdmissionCustody>,
) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError> {
    let replay = if input.header().idempotency_key.is_some() {
        crate::accept::InputReplayPolicy::ExactPrompt
    } else {
        crate::accept::InputReplayPolicy::KeyOnly
    };
    machine
        .accept_input_with_completion_for_attachment_and_replay_policy_with_custody(
            &witness, input, replay, custody,
        )
        .await
}

async fn witness(
    machine: &Arc<MeerkatMachine>,
    session: &SessionId,
) -> crate::RuntimeExecutorAttachmentWitness {
    machine
        .current_executor_attachment_witness(session)
        .await
        .expect("actual attachment")
}

#[tokio::test(flavor = "current_thread")]
async fn caller_cancel_before_credential_custody_settles_not_admitted() {
    let (machine, mut session, input, key) = fixture().await;
    let id = input.id().clone();
    let witness = witness(&machine, &session.session).await;
    let (custody, log) = custody();
    let mut acquisition = AcquisitionObservation::install(&id);
    let held = meerkat_core::acquire_auth_login_lifecycle_guard(&key).await;
    let caller = tokio::spawn(submit_with_custody(
        machine.clone(),
        witness,
        input.clone(),
        custody,
    ));
    let reached = tokio::time::timeout(BOUND, acquisition.next()).await;
    caller.abort();
    let joined = caller.await;
    drop(held);
    lease_fence(&key).await;
    assert!(
        matches!(reached, Ok(Some(Stage::Waiting))),
        "actual credential wait: {reached:?}"
    );
    assert!(matches!(joined, Err(error) if error.is_cancelled()));
    assert_eq!(observed(&log), vec![Settlement::NotAdmitted]);
    assert!(!row_exists(&machine, &session.session, &id).await);
    assert_eq!(session.calls.load(Ordering::SeqCst), 0);
    drop(acquisition);
    let accepted = submit(
        machine.clone(),
        session.session.clone(),
        session.runtime.clone(),
        input,
        Route::WithCompletion,
    )
    .await
    .expect("same real input remains admissible");
    assert!(matches!(accepted, AcceptOutcome::Accepted { .. }));
    complete_same_input(&machine, &mut session, &id, Route::WithCompletion).await;
}

#[tokio::test(flavor = "current_thread")]
async fn caller_abort_after_custody_transfer_cannot_restore_owned_admission() {
    let (machine, mut session, input, key) = fixture().await;
    let id = input.id().clone();
    let witness = witness(&machine, &session.session).await;
    let mutation = machine
        .sessions
        .read()
        .await
        .get(&session.session)
        .unwrap()
        .mutation_gate
        .clone();
    let held = mutation
        .lock_settled(crate::meerkat_machine::SettledGateWait::TestFixture)
        .await;
    let (custody, log) = custody();
    let mut acquisition = AcquisitionObservation::install(&id);
    let caller = tokio::spawn(submit_with_custody(
        machine.clone(),
        witness,
        input,
        custody,
    ));
    let reached = tokio::time::timeout(BOUND, acquisition.next()).await;
    caller.abort();
    let joined = caller.await;
    let before_release = observed(&log);
    drop(held);
    lease_fence(&key).await;
    assert!(
        matches!(reached, Ok(Some(Stage::Acquired))),
        "actual custody handoff: {reached:?}"
    );
    assert!(matches!(joined, Err(error) if error.is_cancelled()));
    assert!(
        before_release.is_empty(),
        "caller drop cannot settle owned custody"
    );
    assert_eq!(observed(&log), vec![Settlement::Admitted]);
    assert!(row_exists(&machine, &session.session, &id).await);
    drop(acquisition);
    complete_same_input(&machine, &mut session, &id, Route::WithCompletion).await;
}

async fn configurable_fixture(
    store: Option<&InMemoryRuntimeStore>,
) -> (
    Arc<MeerkatMachine>,
    AttachedSession,
    Input,
    LeaseKey,
    Arc<MutableControllerAccount>,
    LocalAuthorizationPublication,
) {
    let (configuration, template, account, publication) = mutable_controller_configuration(true);
    let machine = Arc::new(
        match store {
            Some(store) => MeerkatMachine::persistent_with_local_grant_authorization(
                Arc::new(store.clone()),
                Some(Arc::new(meerkat_store::MemoryBlobStore::new())),
                configuration,
            ),
            None => MeerkatMachine::ephemeral().with_local_grant_authorization(configuration),
        }
        .unwrap(),
    );
    let session = AttachedSession::attach(&machine).await;
    let selection = selected(
        &template,
        "candidate-controller",
        &format!("custody-{}", session.session),
    );
    let key = LeaseKey::from_credential_identity(selection.credential());
    install_credential(&machine, &EphemeralTokenStore::new(), &selection).await;
    let input = input_for(&template, &session, selection);
    (machine, session, input, key, account, publication)
}

#[tokio::test(flavor = "current_thread")]
async fn current_policy_refusal_after_transfer_restores_not_admitted_custody() {
    let (machine, mut session, input, key, account, publication) = configurable_fixture(None).await;
    let id = input.id().clone();
    let witness = witness(&machine, &session.session).await;
    let mutation = machine
        .sessions
        .read()
        .await
        .get(&session.session)
        .unwrap()
        .mutation_gate
        .clone();
    let held = mutation
        .lock_settled(crate::meerkat_machine::SettledGateWait::TestFixture)
        .await;
    let (custody, log) = custody();
    let mut acquisition = AcquisitionObservation::install(&id);
    let caller = tokio::spawn(submit_with_custody(
        machine.clone(),
        witness,
        input.clone(),
        custody,
    ));
    let reached = tokio::time::timeout(BOUND, acquisition.next()).await;
    {
        let _change = publication.begin_owner_change().unwrap();
        account.enabled.store(false, Ordering::SeqCst);
    }
    drop(held);
    let result = tokio::time::timeout(BOUND, caller)
        .await
        .expect("bounded refusal")
        .unwrap();
    lease_fence(&key).await;
    assert!(matches!(reached, Ok(Some(Stage::Acquired))));
    assert!(
        matches!(&result, Err(RuntimeDriverError::InputRefused { refusal }) if refusal.kind() == meerkat_core::OperationRefusalKind::Denied),
        "actual current policy refusal: {result:?}"
    );
    assert_eq!(observed(&log), vec![Settlement::NotAdmitted]);
    assert!(!row_exists(&machine, &session.session, &id).await);
    assert_eq!(session.calls.load(Ordering::SeqCst), 0);
    drop(acquisition);
    {
        let _change = publication.begin_owner_change().unwrap();
        account.enabled.store(true, Ordering::SeqCst);
    }
    let accepted = submit(
        machine.clone(),
        session.session.clone(),
        session.runtime.clone(),
        input,
        Route::WithCompletion,
    )
    .await
    .unwrap();
    assert!(matches!(accepted, AcceptOutcome::Accepted { .. }));
    complete_same_input(&machine, &mut session, &id, Route::WithCompletion).await;
}

#[tokio::test(flavor = "current_thread")]
async fn actual_acceptance_settles_before_completion_registration_and_executor_wake() {
    let (machine, mut session, input, _) = fixture().await;
    let id = input.id().clone();
    let witness = witness(&machine, &session.session).await;
    let completions = machine
        .sessions
        .read()
        .await
        .get(&session.session)
        .unwrap()
        .completions
        .clone();
    let held = completions.lock().await;
    let log = Arc::new(StdMutex::new(Vec::new()));
    let cleanup_ran = Arc::new(AtomicBool::new(false));
    let (completed, completion_observation) = tokio::sync::oneshot::channel();
    let custody = Box::new(CompletionCustody {
        settlements: log.clone(),
        cleanup_ran: cleanup_ran.clone(),
        completed,
    });
    let caller = tokio::spawn(submit_with_custody(
        machine.clone(),
        witness,
        input,
        custody,
    ));
    let settled = tokio::time::timeout(BOUND, async {
        loop {
            let value = observed(&log);
            if !value.is_empty() {
                break value;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    let calls_before_release = session.calls.load(Ordering::SeqCst);
    let cleaned_before_release = cleanup_ran.load(Ordering::SeqCst);
    // The actual acceptance occurred, but the real completion registry is held.
    // Aborting the caller cannot prevent the native-owned relay installation.
    caller.abort();
    let joined = caller.await;
    drop(held);
    assert_eq!(
        settled.expect("settlement must precede blocked real completion registration"),
        vec![Settlement::Admitted]
    );
    assert_eq!(
        calls_before_release, 0,
        "no executor wake precedes custody settlement"
    );
    assert!(
        !cleaned_before_release,
        "cleanup requires actual generated completion"
    );
    assert!(matches!(joined, Err(error) if error.is_cancelled()));
    complete_same_input(&machine, &mut session, &id, Route::WithCompletion).await;
    let observation = tokio::time::timeout(BOUND, completion_observation)
        .await
        .expect("real completion must reach owned cleanup")
        .expect("cleanup sender retained");
    assert!(
        observation.is_ok(),
        "actual generated cleanup observation: {observation:?}"
    );
    assert!(
        cleanup_ran.load(Ordering::SeqCst),
        "owned relay runs cleanup without caller"
    );
    assert_eq!(
        observed(&log),
        vec![Settlement::Admitted],
        "settle exactly once"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn real_persistent_commit_with_lost_acknowledgement_keeps_custody_uncertain() {
    let store = InMemoryRuntimeStore::new();
    let (machine, session, input, _, _, _) = configurable_fixture(Some(&store)).await;
    let id = input.id().clone();
    let witness = witness(&machine, &session.session).await;
    let (custody, log) = custody();
    store.lose_next_atomic_input_persist_acknowledgement();
    let result = tokio::time::timeout(
        BOUND,
        submit_with_custody(machine.clone(), witness, input, custody),
    )
    .await
    .expect("bounded ambiguous store return");
    assert!(
        matches!(
            result,
            Err(RuntimeDriverError::RecoveryRepairBlocked { .. })
        ),
        "real lost commit acknowledgement: {result:?}"
    );
    assert_eq!(
        observed(&log),
        vec![Settlement::Uncertain],
        "uncertain commit must never restore the deferred seed"
    );
    let rows = store
        .load_input_states_strict(&session.runtime)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "the actual memory store committed before reporting failure"
    );
    assert_eq!(rows[0].state.input_id, id);
    assert!(rows[0].state.persisted_input.is_some());
    assert_eq!(
        session.calls.load(Ordering::SeqCst),
        0,
        "unsettled durable owner remains fail-closed"
    );
}

/// The session-level custody accept (no attachment fence) settles the
/// supplied custody from the same native admission: an accepted keyed
/// governed prompt settles `Admitted`, and its exact replay deduplicates onto
/// the same input and settles `Admitted` under the exact-prompt contract. An
/// input without a stable key is refused before admission and settles
/// `NotAdmitted`.
#[tokio::test(flavor = "current_thread")]
async fn session_custody_accept_settles_from_native_admission_and_its_exact_replay() {
    let (machine, mut session, input, _key) = fixture().await;
    let id = input.id().clone();

    let (first, first_log) = custody();
    let (accepted, _) = machine
        .accept_input_with_completion_and_custody(&session.session, input.clone(), first)
        .await
        .expect("a keyed governed prompt is admitted");
    assert!(
        matches!(&accepted, AcceptOutcome::Accepted { input_id, .. } if *input_id == id),
        "{accepted:?}"
    );
    assert_eq!(observed(&first_log), vec![Settlement::Admitted]);

    let (replay, replay_log) = custody();
    let (deduplicated, _) = machine
        .accept_input_with_completion_and_custody(&session.session, input.clone(), replay)
        .await
        .expect("an exact replay is answered");
    assert!(
        matches!(&deduplicated, AcceptOutcome::Deduplicated { existing_id, .. } if *existing_id == id),
        "{deduplicated:?}"
    );
    assert_eq!(observed(&replay_log), vec![Settlement::Admitted]);

    let mut unkeyed = input.clone();
    unkeyed.header_mut().idempotency_key = None;
    let (refused, refused_log) = custody();
    assert!(
        machine
            .accept_input_with_completion_and_custody(&session.session, unkeyed, refused)
            .await
            .is_err(),
        "the exact-prompt contract needs a stable key"
    );
    assert_eq!(observed(&refused_log), vec![Settlement::NotAdmitted]);
    complete_same_input(&machine, &mut session, &id, Route::WithCompletion).await;
}
