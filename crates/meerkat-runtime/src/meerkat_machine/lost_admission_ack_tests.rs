//! An input admission whose store committed the admission delta and then
//! lost the acknowledgement degrades its session (`admission_commit`). The
//! input is durably admitted, the caller got a typed refusal, and nothing
//! else may come to settle the session: a host can refuse the retry before
//! it reaches the runtime, and an unregister refuses at its first durable
//! write. The idle runtime loop then stayed attached, never published its
//! teardown handoff, and the reload-required discard waited for that handoff
//! forever.
//!
//! The admission now hands off the degraded gate settlement as it leaves,
//! as an admission past its commit deadline does, and a reload hands it off
//! for a loop that is still attached, so the reload completes and the
//! committed input stays pending in the cold successor. The rows cover both
//! admission write paths: inline, and the owned child task a store that
//! declares a commit outcome deadline (the SQLite store) writes in.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::tests::RuntimeCommitAtomicityStore;
use super::*;
use std::sync::atomic::Ordering;
use std::time::Duration;

use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;

use crate::store::{DEFAULT_COMMIT_OUTCOME_DEADLINE, InMemoryRuntimeStore};

/// How long an admission, a settlement or a hand-off may take to answer.
const ANSWER_BOUND: Duration = Duration::from_secs(5);
/// How long a reload may take to settle. Before the fix it never did: the
/// bound turns that hang into a failure.
const RELOAD_BOUND: Duration = Duration::from_secs(10);

const LOST_ACK_TEXT: &str = "an admission whose acknowledgement is lost";
const LOST_ACK_KEY: &str = "lost-admission-acknowledgement";

/// The two ways an admission writes its delta.
#[derive(Clone, Copy, Debug)]
enum AdmissionWritePath {
    /// The store declares no commit outcome deadline: the admission writes
    /// inline, holding M and the driver.
    Inline,
    /// The store declares the default commit outcome deadline, as the SQLite
    /// store does: the admission writes in an owned child task that hands M
    /// and the driver back with the outcome.
    OwnedChild,
}

/// A persistent machine over `inner` through `write_path`. `inner` stays
/// the test's handle on the store: its scripted acknowledgement loss and its
/// rows are the machine's.
fn machine_over(
    inner: &InMemoryRuntimeStore,
    write_path: AdmissionWritePath,
) -> Arc<MeerkatMachine> {
    let store: Arc<dyn RuntimeStore> = match write_path {
        AdmissionWritePath::Inline => Arc::new(inner.clone()) as Arc<dyn RuntimeStore>,
        AdmissionWritePath::OwnedChild => {
            let store = RuntimeCommitAtomicityStore::pass_through(Arc::new(inner.clone()));
            store.declare_commit_outcome_deadline(DEFAULT_COMMIT_OUTCOME_DEADLINE);
            Arc::new(store) as Arc<dyn RuntimeStore>
        }
    };
    Arc::new(
        MeerkatMachine::persistent(store, Arc::new(meerkat_store::MemoryBlobStore::new()))
            .expect("persistent machine"),
    )
}

/// Gives the session an idle runtime loop and finishes every run at once.
struct IdleExecutor;

#[async_trait::async_trait]
impl CoreExecutor for IdleExecutor {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        Ok(CoreApplyOutput::with_untyped_snapshot(
            RunBoundaryReceiptDraft {
                run_id,
                boundary: RunApplyBoundary::RunStart,
                contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                conversation_digest: None,
                message_count: 0,
            },
            None,
            None,
        ))
    }

    async fn cancel_after_boundary(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(&mut self, _reason: String) -> Result<(), CoreExecutorError> {
        Ok(())
    }
}

async fn registered_with_runtime_loop(machine: &Arc<MeerkatMachine>) -> SessionId {
    let session_id = SessionId::new();
    machine
        .register_session_with_executor(session_id.clone(), Box::new(IdleExecutor))
        .await
        .expect("register a session with a runtime loop");
    assert!(
        runtime_loop_attached(machine, &session_id).await,
        "the fixture needs an attached runtime loop"
    );
    session_id
}

async fn runtime_loop_attached(machine: &MeerkatMachine, session_id: &SessionId) -> bool {
    let sessions = machine.sessions.read().await;
    let entry = sessions.get(session_id).expect("registered session entry");
    !matches!(entry.attachment_slot, RuntimeLoopAttachmentSlot::Empty)
}

async fn teardown_slot(
    machine: &MeerkatMachine,
    session_id: &SessionId,
) -> Arc<crate::runtime_loop::RuntimeLoopTeardownSlot> {
    let sessions = machine.sessions.read().await;
    sessions
        .get(session_id)
        .expect("registered session entry")
        .runtime_loop_teardown
        .clone()
        .expect("the runtime loop's teardown slot")
}

/// Wait until `condition` holds, for at most the answer bound.
async fn eventually<F, Fut>(what: &str, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + ANSWER_BOUND;
    while !condition().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A prompt that carries `key` as its idempotency key.
fn keyed_prompt(text: &str, key: &str) -> crate::Input {
    let mut input = crate::Input::Prompt(crate::PromptInput::new(text, None));
    input.header_mut().idempotency_key = Some(crate::identifiers::IdempotencyKey::new(key));
    input
}

fn hand_offs(machine: &MeerkatMachine) -> usize {
    machine
        .test_degraded_gate_settlement_hand_offs
        .load(Ordering::SeqCst)
}

/// Admit `input` while the store loses the acknowledgement of the commit
/// that admits it. The admission refuses typed, the session needs a reload
/// for `admission_commit`, and the store holds exactly the input, committed.
async fn admit_losing_the_acknowledgement(
    machine: &Arc<MeerkatMachine>,
    inner: &InMemoryRuntimeStore,
    session_id: &SessionId,
    input: crate::Input,
) {
    let input_id = input.id().clone();
    inner.lose_next_atomic_input_persist_acknowledgement();
    let admitted = tokio::time::timeout(
        ANSWER_BOUND,
        machine.accept_input_with_completion(session_id, input),
    )
    .await
    .expect("the admission answers");
    let refusal = match admitted {
        Ok((outcome, _)) => panic!("the admission answered a lost acknowledgement: {outcome:?}"),
        Err(error) => error,
    };
    assert!(
        matches!(refusal, RuntimeDriverError::RecoveryRepairBlocked { .. }),
        "the admission refuses typed: {refusal:?}"
    );
    let required = machine
        .durability_reload_required(session_id)
        .await
        .expect("the lost acknowledgement degraded the session");
    assert_eq!(required.operation, "admission_commit");
    assert_committed_input_kept(inner, session_id, &input_id).await;
}

/// The store holds exactly `input_id`, with its payload and no terminal
/// outcome: the pending original that a lost acknowledgement leaves behind.
async fn assert_committed_input_kept(
    inner: &InMemoryRuntimeStore,
    session_id: &SessionId,
    input_id: &InputId,
) {
    let rows = inner
        .load_input_states_strict(&LogicalRuntimeId::for_session(session_id))
        .await
        .expect("read the store's inputs");
    assert_eq!(rows.len(), 1, "the store holds exactly one input: {rows:?}");
    assert_eq!(&rows[0].state.input_id, input_id);
    assert!(
        rows[0].state.persisted_input.is_some(),
        "the committed input keeps its payload"
    );
    assert!(
        rows[0].seed.terminal_outcome.is_none(),
        "the committed input stays pending: {:?}",
        rows[0].seed.phase
    );
}

/// The admission hands the degraded settlement off as it leaves, without
/// any later caller: the settlement detaches the runtime loop once M frees,
/// and the loop hands its executor off, which is what a reload claims.
/// Before, nothing detached the idle loop and its handoff never came.
async fn lost_acknowledgement_hands_off_the_settlement(write_path: AdmissionWritePath) {
    let inner = InMemoryRuntimeStore::new();
    let machine = machine_over(&inner, write_path);
    let session_id = registered_with_runtime_loop(&machine).await;
    let slot = teardown_slot(&machine, &session_id).await;

    admit_losing_the_acknowledgement(
        &machine,
        &inner,
        &session_id,
        keyed_prompt(LOST_ACK_TEXT, LOST_ACK_KEY),
    )
    .await;
    assert_eq!(
        hand_offs(&machine),
        1,
        "the admission hands the degraded settlement off as it leaves"
    );
    eventually(
        "the handed-off settlement detaches the runtime loop",
        || {
            let machine = Arc::clone(&machine);
            let session_id = session_id.clone();
            async move { !runtime_loop_attached(&machine, &session_id).await }
        },
    )
    .await;
    eventually("the detached loop hands its executor off", || {
        let slot = Arc::clone(&slot);
        async move { slot.exit_handoff_is_settled() }
    })
    .await;
}

#[tokio::test]
async fn a_lost_admission_ack_hands_off_the_settlement_inline() {
    lost_acknowledgement_hands_off_the_settlement(AdmissionWritePath::Inline).await;
}

#[tokio::test]
async fn a_lost_admission_ack_hands_off_the_settlement_owned_child() {
    lost_acknowledgement_hands_off_the_settlement(AdmissionWritePath::OwnedChild).await;
}

/// A host's sequence: the admission loses its acknowledgement, and the
/// reload of the degraded registration settles within its bound. It
/// replaces the registration with a cold successor, the committed input
/// stays pending there, and a resubmission with the same idempotency key is
/// deduplicated against it: admitted exactly once. Before, the reload waited
/// for the idle loop's teardown handoff forever.
async fn reload_after_a_lost_acknowledgement_keeps_the_pending_input(
    write_path: AdmissionWritePath,
) {
    let inner = InMemoryRuntimeStore::new();
    let machine = machine_over(&inner, write_path);
    let session_id = registered_with_runtime_loop(&machine).await;
    let witness = machine
        .current_session_registration_witness(&session_id)
        .await
        .expect("exact registration");
    let input = keyed_prompt(LOST_ACK_TEXT, LOST_ACK_KEY);
    let input_id = input.id().clone();
    admit_losing_the_acknowledgement(&machine, &inner, &session_id, input).await;

    let disposition = tokio::time::timeout(
        RELOAD_BOUND,
        machine.recover_or_discard_reload_required_registration_if_current(&witness),
    )
    .await
    .expect("the reload settles: it does not wait for a loop nobody detaches")
    .expect("the reload runs");
    assert_eq!(
        disposition,
        ReloadRequiredRegistrationDisposition::Discarded,
        "the reload replaces the degraded registration"
    );
    let successor = machine
        .current_session_registration_witness(&session_id)
        .await
        .expect("the cold successor is registered");
    assert_ne!(
        successor, witness,
        "the reload publishes a distinct successor"
    );
    assert!(
        machine.is_durability_ready(&session_id).await,
        "the cold install is ready"
    );
    assert_committed_input_kept(&inner, &session_id, &input_id).await;

    let (resubmitted, _) = tokio::time::timeout(
        ANSWER_BOUND,
        machine
            .accept_input_with_completion(&session_id, keyed_prompt(LOST_ACK_TEXT, LOST_ACK_KEY)),
    )
    .await
    .expect("the resubmission answers")
    .expect("the resubmission is answered");
    assert!(
        matches!(
            &resubmitted,
            crate::accept::AcceptOutcome::Deduplicated { existing_id, .. }
                if existing_id == &input_id
        ),
        "the resubmission is deduplicated against the committed input: {resubmitted:?}"
    );
    assert_committed_input_kept(&inner, &session_id, &input_id).await;
}

#[tokio::test]
async fn a_reload_after_a_lost_admission_ack_keeps_the_pending_input_inline() {
    reload_after_a_lost_acknowledgement_keeps_the_pending_input(AdmissionWritePath::Inline).await;
}

#[tokio::test]
async fn a_reload_after_a_lost_admission_ack_keeps_the_pending_input_owned_child() {
    reload_after_a_lost_acknowledgement_keeps_the_pending_input(AdmissionWritePath::OwnedChild)
        .await;
}

/// A session degrades with its runtime loop attached and nobody settles it
/// (the fixture degrades it directly, as a failed durable write that returns
/// without the settlement does). The reload hands the degraded settlement
/// off itself, which detaches the loop, and settles within its bound.
/// Before, it waited for that loop's teardown handoff forever.
#[tokio::test]
async fn a_reload_detaches_a_runtime_loop_nobody_settled() {
    let inner = InMemoryRuntimeStore::new();
    let machine = machine_over(&inner, AdmissionWritePath::Inline);
    let session_id = registered_with_runtime_loop(&machine).await;
    let witness = machine
        .current_session_registration_witness(&session_id)
        .await
        .expect("exact registration");
    assert!(
        machine
            .force_session_durability_reload_required_for_test(&session_id)
            .await,
        "the fixture degrades the session"
    );
    assert!(
        runtime_loop_attached(&machine, &session_id).await,
        "nothing settled the degraded session"
    );
    assert_eq!(hand_offs(&machine), 0, "nothing handed a settlement off");

    let disposition = tokio::time::timeout(
        RELOAD_BOUND,
        machine.recover_or_discard_reload_required_registration_if_current(&witness),
    )
    .await
    .expect("the reload settles: it does not wait for a loop nobody detaches")
    .expect("the reload runs");
    assert_eq!(
        disposition,
        ReloadRequiredRegistrationDisposition::Discarded,
        "the reload replaces the degraded registration"
    );
    assert_eq!(
        hand_offs(&machine),
        1,
        "the reload handed the degraded settlement off"
    );
    assert_ne!(
        machine
            .current_session_registration_witness(&session_id)
            .await
            .expect("the cold successor is registered"),
        witness,
        "the reload publishes a distinct successor"
    );
    assert!(
        machine.is_durability_ready(&session_id).await,
        "the cold install is ready"
    );
}
