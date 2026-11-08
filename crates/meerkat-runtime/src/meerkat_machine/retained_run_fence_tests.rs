//! A host's settle of the run a session retained, on a host store writer
//! whose store fence another writer took. The writer's runtime loop meets the
//! lost fence and tears the session down, and that teardown's unregister
//! settles the run its registration retained. Only the store's fence keeps
//! those writes from ending the new owner's work, so the row reads the
//! session through a realm the new owner opens after the teardown.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_core::host_store::{AcquireMode, FenceTiming, HostStore as _};
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::core_executor::{
    BoundSessionCommit, CoreApplyOutput, CoreExecutor, CoreExecutorError,
};
use meerkat_core::lifecycle::run_primitive::{RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft;
use meerkat_core::types::{
    AssistantBlock, BlockAssistantMessage, Message, SessionId, StopReason,
    TranscriptMessageIdentity, UserMessage,
};

use super::MeerkatMachine;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_state::{
    InputLifecycleState, InputState, InputStatePersistenceRecord, InputStateSeed, StoredInputState,
};
use crate::store::spi::test_host::TestHost;
use crate::store::spi::{SpiRealm, SpiRealmConfig, SpiRuntimeStore};
use crate::store::{PreparedRuntimeSessionCommit, PreparedWholeBlobProvisionalTail, RuntimeStore};

/// Bound on one step of the row: it only turns a hang into a failure.
const STEP: Duration = Duration::from_secs(30);

/// Records every run it is asked to apply. A held session applies none.
#[derive(Clone, Default)]
struct RecordedApplies(Arc<Mutex<Vec<RunId>>>);

impl RecordedApplies {
    fn applied(&self) -> Vec<RunId> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl CoreExecutor for RecordedApplies {
    async fn apply(
        &mut self,
        run_id: RunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(run_id.clone());
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

/// The row of a kept run's input: staged for `run_id`, with what admission
/// stamps on it.
fn staged_row(input: &Input, run_id: &RunId) -> InputStatePersistenceRecord {
    let mut state = InputState::new_accepted(input.id().clone());
    state.persisted_input = Some(input.clone());
    state.durability = Some(crate::input::InputDurability::Durable);
    state.idempotency_key = input.header().idempotency_key.clone();
    state.directed_run_started_attribution =
        crate::input_state::DirectedRunStartedAttribution::from_input(input)
            .expect("the input's directed shape validates");
    let policy = crate::DefaultPolicyTable::resolve(input, true);
    state.runtime_semantics = Some(
        crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(input, true)
            .expect("generated admission semantics"),
    );
    state.policy = Some(crate::input_state::PolicySnapshot {
        version: policy.policy_version,
        decision: policy,
    });
    InputStatePersistenceRecord::stored_row_for_test(StoredInputState {
        state,
        seed: InputStateSeed {
            phase: InputLifecycleState::Staged,
            last_run_id: Some(run_id.clone()),
            last_boundary_sequence: None,
            terminal_outcome: None,
            attempt_count: 1,
            admission_sequence: None,
            recovery_lane: Some(meerkat_core::types::HandlingMode::Queue),
        },
    })
    .expect("a staged row passes the stored-seed authority")
}

/// Write through `store` what a graceful stop that kept a durable run leaves:
/// the session's committed base, the run's provisional tail (its first
/// round's staged call, its cursor binding the run's input), and the input's
/// row, staged for the run.
async fn a_kept_run(store: &dyn RuntimeStore) -> (SessionId, RunId, Input) {
    let input = Input::Prompt(crate::input::PromptInput::new("the run's prompt", None));
    let run_id = RunId::new();
    let mut committed = meerkat_core::Session::new();
    committed.push(Message::User(UserMessage::text("the run's prompt")));
    let session_id = committed.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let base = store
        .commit_prepared_session_boundary(
            &runtime_id,
            PreparedRuntimeSessionCommit::snapshot_only(
                BoundSessionCommit::sealed(Arc::new(committed.clone()))
                    .expect("seal the committed session"),
            ),
        )
        .await
        .expect("the committed base is written")
        .authority()
        .and_then(|authority| authority.whole_blob())
        .cloned()
        .expect("a host store realm commits a WholeBlob authority");
    let staged = BlockAssistantMessage {
        blocks: vec![AssistantBlock::ToolUse {
            id: "kept-call".to_string(),
            name: "durable_tool".to_string(),
            args: serde_json::value::RawValue::from_string("{}".to_string())
                .expect("raw tool arguments"),
            meta: None,
        }],
        stop_reason: Some(StopReason::ToolUse),
        identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
        created_at: meerkat_core::types::message_timestamp_now(),
        assistant_message_id: Some(
            serde_json::from_value(serde_json::Value::String(uuid::Uuid::now_v7().to_string()))
                .expect("an assistant message id"),
        ),
    };
    let mut round = committed;
    round
        .set_run_cursor(&meerkat_core::SessionRunCursor::new(
            run_id.clone(),
            vec![input.id().clone()],
            0,
            meerkat_core::RunResumePoint::ToolsPending {
                assistant_message: staged,
            },
        ))
        .expect("the run cursor is recorded");
    store
        .write_prepared_whole_blob_provisional_tail(
            &runtime_id,
            PreparedWholeBlobProvisionalTail::prepare(
                base,
                run_id.clone(),
                1,
                &BoundSessionCommit::sealed(Arc::new(round)).expect("seal the run's tail"),
            )
            .expect("the run's tail prepares"),
        )
        .await
        .expect("the run's tail is written");
    store
        .persist_input_state(&runtime_id, &staged_row(&input, &run_id))
        .await
        .expect("the run's input row is written");
    (session_id, run_id, input)
}

/// A host's settle on a host store writer whose fence another writer took is
/// refused, and the new owner's work stays whole. The writer holds the
/// session's run starts for its recovery, so its runtime loop parks with the
/// kept run marked. Another writer then takes the host store's fence, and
/// the writer's realm learns it at its next renew. The settle wakes the
/// loop, which meets the lost fence before it settles anything: it tells the
/// run's waiter that its input stays queued for the new owner and tears the
/// session down. The realm refuses every read and write of a writer that
/// lost its fence until it is reopened, so the teardown cannot record its
/// unregister, and the registration needs a durability reload that no
/// reload in this process can make. The call answers
/// `RuntimeDriverError::StaleAuthority`, the refusal of a session that leaves
/// this process, never `Settled` and never the reload refusal, and nothing
/// runs. A host's retry meets the registration the teardown left degraded
/// and answers the same, at the settle's first gate acquisition this time.
/// The teardown's unregister would settle the run its registration
/// retained, but no batch of this writer lands once its fence is lost: a
/// realm the new owner opens reads the run's input with no terminal and the
/// run's tail standing, which its own registration then recovers. The row of
/// this case over the in-memory store asserts only the refusal: that store
/// does not fence a writer that lost the fence. RED before: the call
/// answered the reload refusal, which sent a host to reload a registration
/// whose store refuses it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_settle_on_a_host_store_writer_that_lost_its_fence_leaves_the_new_owners_work() {
    let timing = FenceTiming::try_new(Duration::from_secs(1), Duration::from_secs(3))
        .expect("a valid fence timing");
    let host = Arc::new(TestHost::with_timing(timing));
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .expect("the writer's realm opens");
    let store: Arc<dyn RuntimeStore> = Arc::new(
        SpiRuntimeStore::open(Arc::clone(&realm))
            .await
            .expect("the writer's runtime store opens"),
    );
    let (session_id, run_id, input) = a_kept_run(store.as_ref()).await;
    let runtime_id = LogicalRuntimeId::for_session(&session_id);

    // The writer registers the session under the host's recovery hold, and
    // its runtime loop parks on the hold with the kept run marked.
    let machine = Arc::new(
        MeerkatMachine::persistent(
            Arc::clone(&store),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .expect("persistent machine"),
    );
    machine.stage_registration_run_start_hold_only(
        &session_id,
        crate::RunStartHoldReason::HostRecovery,
    );
    machine
        .register_session(session_id.clone())
        .await
        .expect("the session's runtime registers");
    assert_eq!(
        machine.resumable_run_id(&session_id).await,
        Some(run_id.clone()),
        "the registration keeps the run resumable"
    );
    machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("bind the runtime placement");
    let mut parks = machine.run_start_held_parks();
    let parked_before = *parks.borrow();
    let executor = RecordedApplies::default();
    machine
        .ensure_session_with_executor(session_id.clone(), Box::new(executor.clone()))
        .await
        .expect("attach the executor");
    tokio::time::timeout(STEP, parks.wait_for(|parks| *parks > parked_before))
        .await
        .expect("the runtime loop parks on the host's hold")
        .expect("the park counter stays open");

    // Another writer takes the fence; the writer's next renew is fenced out.
    host.acquire_fence(AcquireMode::Takeover)
        .await
        .expect("another writer takes the fence");
    tokio::time::timeout(STEP, async {
        while realm.realm_poisoned().is_none() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the writer's realm learns of the takeover at its next renew");
    let landed_at_takeover = host.commit_count();

    let refused = tokio::time::timeout(STEP, machine.settle_retained_run(&session_id))
        .await
        .expect("the settle answers once the session's waiters were told");
    assert!(
        matches!(
            refused,
            Err(crate::RuntimeDriverError::StaleAuthority { .. })
        ),
        "a settle on a writer whose store fence another writer took answers the stale-authority \
         refusal of a session that leaves this process, not {refused:?}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        executor.applied().is_empty(),
        "nothing runs in this process: {:?}",
        executor.applied()
    );
    // The host's retry meets the registration the teardown left degraded,
    // which no reload in this process can serve: the same refusal.
    assert!(
        machine
            .durability_reload_required(&session_id)
            .await
            .is_some(),
        "the writer's registration stays durability-degraded"
    );
    let retried = tokio::time::timeout(STEP, machine.settle_retained_run(&session_id))
        .await
        .expect("the retry answers");
    assert!(
        matches!(
            retried,
            Err(crate::RuntimeDriverError::StaleAuthority { .. })
        ),
        "a retry on a writer whose store realm lost its fence answers the stale-authority \
         refusal again, not {retried:?}"
    );
    assert_eq!(
        host.commit_count(),
        landed_at_takeover,
        "no batch of the writer that lost its fence lands, the teardown's included"
    );

    // The new owner's view (the takeover's lease is let go first, so an
    // exclusive open succeeds).
    host.expire_lease();
    let owner_realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .expect("the new owner's realm opens");
    let owner_store = SpiRuntimeStore::open(Arc::clone(&owner_realm))
        .await
        .expect("the new owner's runtime store opens");
    let row = owner_store
        .load_input_state(&runtime_id, input.id())
        .await
        .expect("read the run's input row")
        .expect("the run's input row stays");
    assert!(
        matches!((row.seed.phase, &row.seed.terminal_outcome), (_, None)),
        "the run's input stays queued for the new owner, with no terminal: {:?}",
        (row.seed.phase, &row.seed.terminal_outcome)
    );
    assert!(
        owner_store
            .load_whole_blob_provisional_tail(&runtime_id)
            .await
            .expect("read the provisional tail")
            .is_some(),
        "the kept run's tail stands for the new owner"
    );
}
