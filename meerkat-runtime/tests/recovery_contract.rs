#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
//! Phase 0 external-boundary contract tests for runtime receipt/replay recovery.
//!
//! These exercise the real runtime store implementations and both runtime
//! drivers through one outside-in recovery matrix.

use std::sync::Arc;

use chrono::Utc;
use meerkat_core::BlobStore;
use meerkat_core::lifecycle::run_primitive::RunApplyBoundary;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
use meerkat_core::types::SessionId;
use meerkat_runtime::SessionServiceRuntimeExt;
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PromptInput,
};
use meerkat_runtime::input_state::{
    InputLifecycleState, InputState, InputStatePersistenceRecord, InputStateSeed,
    InputTerminalOutcome, StoredInputState,
};
use meerkat_runtime::runtime_state::RuntimeState;
use meerkat_runtime::store::{
    InMemoryRuntimeStore, RuntimeStore, SessionDelta, load_runtime_state,
};
use meerkat_runtime::traits::RuntimeDriver;
use meerkat_runtime::{EphemeralRuntimeDriver, MeerkatMachine, PersistentRuntimeDriver};
use meerkat_store::MemoryBlobStore;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

#[cfg(feature = "sqlite-store")]
use meerkat_runtime::store::SqliteRuntimeStore;

struct StoreHarness {
    name: &'static str,
    store: Arc<dyn RuntimeStore>,
    _tempdir: Option<TempDir>,
}

fn supported_store_harnesses() -> Vec<StoreHarness> {
    #[allow(unused_mut)]
    let mut harnesses = vec![StoreHarness {
        name: "memory",
        store: Arc::new(InMemoryRuntimeStore::new()),
        _tempdir: None,
    }];

    #[cfg(feature = "sqlite-store")]
    {
        let tempdir = TempDir::new().unwrap();
        let db_path = tempdir.path().join("runtime.sqlite3");
        let store = Arc::new(SqliteRuntimeStore::new(&db_path).unwrap());
        harnesses.push(StoreHarness {
            name: "sqlite",
            store,
            _tempdir: Some(tempdir),
        });
    }

    harnesses
}

fn memory_blob_store() -> Arc<dyn BlobStore> {
    Arc::new(MemoryBlobStore::new())
}

fn make_runtime_id(label: &str) -> LogicalRuntimeId {
    LogicalRuntimeId::new(format!("recovery-{label}-{}", Uuid::now_v7()))
}

fn make_prompt(text: &str) -> Input {
    Input::Prompt(PromptInput {
        injected_context: Vec::new(),
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Operator,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        content: text.into(),
        typed_turn_appends: Vec::new(),
        turn_metadata: None,
    })
}

fn make_session_snapshot() -> Vec<u8> {
    serde_json::to_vec(&meerkat_core::Session::new()).unwrap()
}

#[derive(serde::Serialize)]
struct CompactionCommitFingerprintFixture<'a> {
    selection: &'a meerkat_core::TranscriptRewriteSelection,
    original_span_digest: &'a str,
    replacement_digest: &'a str,
    messages_before: usize,
    messages_after: usize,
    actor: &'a Option<String>,
}

fn compaction_commit_fingerprint(commit: &meerkat_core::TranscriptRewriteCommit) -> String {
    use std::fmt::Write as _;

    let canonical = serde_json::to_vec(&CompactionCommitFingerprintFixture {
        selection: &commit.selection,
        original_span_digest: &commit.original_span_digest,
        replacement_digest: &commit.replacement_digest,
        messages_before: commit.messages_before,
        messages_after: commit.messages_after,
        actor: &commit.actor,
    })
    .unwrap();
    let mut fingerprint = String::from("sha256:");
    for byte in Sha256::digest(canonical) {
        write!(&mut fingerprint, "{byte:02x}").unwrap();
    }
    fingerprint
}

fn make_session_with_compaction_intent() -> (
    meerkat_core::Session,
    meerkat_core::CompactionProjectionIntent,
) {
    let mut session = meerkat_core::Session::new();
    session.push(meerkat_core::types::Message::User(
        meerkat_core::types::UserMessage::text("verbose context one"),
    ));
    session.push(meerkat_core::types::Message::User(
        meerkat_core::types::UserMessage::text("verbose context two"),
    ));
    let parent = session.transcript_revision().unwrap();
    session
        .commit_transcript_rewrite(
            meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
            vec![meerkat_core::types::Message::User(
                meerkat_core::types::UserMessage::compaction_summary("compacted context"),
            )],
            meerkat_core::TranscriptRewriteReason::new("compaction"),
            Some("runtime-store-conformance".to_string()),
            Some(parent),
        )
        .unwrap();

    // The persisted selection is the compaction-specific wire variant used by
    // runtime-store validation. Keep this fixture in the shared suite so both
    // backends exercise the exact same transaction input.
    let mut encoded = serde_json::to_value(&session).unwrap();
    encoded["metadata"][meerkat_core::SESSION_TRANSCRIPT_HISTORY_STATE_KEY]["commits"][0]["selection"] = serde_json::json!({
        "type": "compaction_message_range",
        "range": { "start": 0, "end": 2 }
    });
    let mut session: meerkat_core::Session = serde_json::from_value(encoded).unwrap();
    let commit = session
        .transcript_history_state()
        .unwrap()
        .unwrap()
        .commits
        .last()
        .unwrap()
        .clone();
    let commit_fingerprint = compaction_commit_fingerprint(&commit);
    let intent = meerkat_core::CompactionProjectionIntent {
        projection: serde_json::from_value(serde_json::json!({
            "session_id": session.id(),
            "parent_revision": &commit.parent_revision,
            "revision": &commit.revision,
            "commit_fingerprint": commit_fingerprint,
        }))
        .unwrap(),
        summary_tokens: 5,
        messages_before: 2,
        messages_after: 1,
    };
    session
        .add_compaction_projection_intent(intent.clone())
        .unwrap();
    (session, intent)
}

fn make_receipt(
    run_id: RunId,
    contributing_input_ids: Vec<InputId>,
    sequence: u64,
) -> RunBoundaryReceipt {
    RunBoundaryReceipt {
        run_id,
        boundary: RunApplyBoundary::RunStart,
        contributing_input_ids,
        conversation_digest: None,
        message_count: 0,
        sequence,
    }
}

fn stamp_runtime_metadata(state: &mut InputState, input: &Input) {
    let policy = meerkat_runtime::DefaultPolicyTable::resolve(input, true);
    let policy_version = policy.policy_version;
    state.runtime_semantics = Some(
        meerkat_runtime::ingress_types::RuntimeInputSemantics::try_from_generated_admission(
            input, true,
        )
        .expect("generated admission semantics"),
    );
    state.policy = Some(meerkat_runtime::input_state::PolicySnapshot {
        version: policy_version,
        decision: policy,
    });
}

fn applied_pending_state(input: &Input, run_id: &RunId, sequence: u64) -> StoredInputState {
    let mut state = InputState::new_accepted(input.id().clone());
    state.persisted_input = Some(input.clone());
    state.durability = Some(InputDurability::Durable);
    stamp_runtime_metadata(&mut state, input);
    // Simulate Accepted → Queued → Staged → Applied → AppliedPendingConsumption
    // by seeding the DSL-owned phase + run association alongside the shell.
    // The recovery path normalises these to a recovered phase based on the
    // persisted boundary receipt; the history chain is not material to
    // recovery.
    StoredInputState {
        state,
        seed: InputStateSeed {
            phase: InputLifecycleState::AppliedPendingConsumption,
            last_run_id: Some(run_id.clone()),
            last_boundary_sequence: Some(sequence),
            terminal_outcome: None,
            attempt_count: 1,
            admission_sequence: None,
            recovery_lane: Some(meerkat_core::types::HandlingMode::Queue),
        },
    }
}

fn persistable(stored: StoredInputState) -> InputStatePersistenceRecord {
    let mut driver = EphemeralRuntimeDriver::new(make_runtime_id("persistence-record"));
    driver
        .recover_input_state_persistence_record(stored)
        .expect("test input-state seed should pass generated recovery authority")
}

fn sorted_id_strings(ids: impl IntoIterator<Item = InputId>) -> Vec<String> {
    let mut ids = ids.into_iter().map(|id| id.to_string()).collect::<Vec<_>>();
    ids.sort();
    ids
}

async fn retire_runtime(
    driver: &mut PersistentRuntimeDriver,
) -> Result<meerkat_runtime::RetireReport, meerkat_runtime::RuntimeDriverError> {
    let pending = driver.active_input_ids().len();
    Ok(meerkat_runtime::RetireReport {
        inputs_abandoned: 0,
        inputs_pending_drain: pending,
    })
}

#[tokio::test]
async fn recovery_store_contract_applies_machine_owned_receipts_across_supported_backends() {
    for harness in supported_store_harnesses() {
        let runtime_id = make_runtime_id(harness.name);
        let run_id = RunId::new();
        let first = make_prompt("first contribution");
        let second = make_prompt("second contribution");
        let first_id = first.id().clone();
        let second_id = second.id().clone();
        let receipt = RunBoundaryReceipt {
            run_id: run_id.clone(),
            boundary: RunApplyBoundary::RunStart,
            contributing_input_ids: vec![first_id.clone(), second_id.clone()],
            conversation_digest: Some(format!("{}-machine-digest", harness.name)),
            message_count: 2,
            sequence: 0,
        };

        harness
            .store
            .atomic_apply(
                &runtime_id,
                Some(SessionDelta {
                    session_snapshot: make_session_snapshot(),
                }),
                receipt.clone(),
                vec![
                    persistable(applied_pending_state(&first, &run_id, 0)),
                    persistable(applied_pending_state(&second, &run_id, 0)),
                ],
                None,
            )
            .await
            .unwrap();

        assert_eq!(
            receipt.sequence, 0,
            "{}: first authoritative receipt should start at sequence zero",
            harness.name
        );
        assert_eq!(
            receipt.contributing_input_ids,
            vec![first_id.clone(), second_id.clone()],
            "{}: authoritative receipt should preserve contributor order",
            harness.name
        );
        assert!(
            receipt.conversation_digest.is_some(),
            "{}: receipt should preserve the machine-owned digest",
            harness.name
        );
        assert_eq!(
            receipt.message_count, 2,
            "{}: receipt should preserve the machine-owned message count",
            harness.name
        );

        let loaded_receipt = harness
            .store
            .load_boundary_receipt(&runtime_id, &run_id, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded_receipt, receipt,
            "{}: stored receipt should round-trip without drift",
            harness.name
        );

        let first_state = harness
            .store
            .load_input_state(&runtime_id, &first_id)
            .await
            .unwrap()
            .unwrap();
        let second_state = harness
            .store
            .load_input_state(&runtime_id, &second_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            first_state.seed.last_run_id,
            Some(run_id.clone()),
            "{}: first contributor should record the authoritative run id",
            harness.name
        );
        assert_eq!(
            first_state.seed.last_boundary_sequence,
            Some(0),
            "{}: first contributor should record the authoritative boundary sequence",
            harness.name
        );
        assert_eq!(
            second_state.seed.last_run_id,
            Some(run_id.clone()),
            "{}: second contributor should record the authoritative run id",
            harness.name
        );
        assert_eq!(
            second_state.seed.last_boundary_sequence,
            Some(0),
            "{}: second contributor should record the authoritative boundary sequence",
            harness.name
        );

        let second_receipt = harness
            .store
            .atomic_apply(
                &runtime_id,
                Some(SessionDelta {
                    session_snapshot: make_session_snapshot(),
                }),
                RunBoundaryReceipt {
                    run_id: run_id.clone(),
                    boundary: RunApplyBoundary::Immediate,
                    contributing_input_ids: vec![second_id.clone()],
                    conversation_digest: Some(format!("{}-second-digest", harness.name)),
                    message_count: 1,
                    sequence: 1,
                },
                vec![persistable(applied_pending_state(&second, &run_id, 1))],
                None,
            )
            .await;
        assert!(
            second_receipt.is_ok(),
            "{}: second machine-owned atomic apply should succeed",
            harness.name
        );
        let second_receipt = harness
            .store
            .load_boundary_receipt(&runtime_id, &run_id, 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            second_receipt.sequence, 1,
            "{}: the durable store should preserve the next machine-owned receipt sequence",
            harness.name
        );
    }
}

#[tokio::test]
async fn compaction_outbox_transaction_contract_is_shared_across_supported_backends() {
    for harness in supported_store_harnesses() {
        assert!(
            harness.store.supports_compaction_projection_outbox(),
            "{}: supported runtime store must advertise the compaction outbox it implements",
            harness.name
        );

        let runtime_id = make_runtime_id(harness.name);
        let (session, intent) = make_session_with_compaction_intent();
        let pending_snapshot = serde_json::to_vec(&session).unwrap();
        harness
            .store
            .atomic_apply(
                &runtime_id,
                Some(SessionDelta {
                    session_snapshot: pending_snapshot.clone(),
                }),
                make_receipt(RunId::new(), Vec::new(), 1),
                Vec::new(),
                Some(session.id().clone()),
            )
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{}: compaction snapshot and outbox must commit atomically: {error}",
                    harness.name
                )
            });
        assert_eq!(
            harness
                .store
                .load_pending_compaction_projections(&runtime_id)
                .await
                .unwrap(),
            vec![intent.clone()],
            "{}: committed compaction intent must be recoverable from the outbox",
            harness.name
        );

        harness
            .store
            .mark_compaction_projection_finalized(&runtime_id, &intent.projection)
            .await
            .unwrap();
        harness
            .store
            .mark_compaction_projection_finalized(&runtime_id, &intent.projection)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "{}: compaction finalization acknowledgement must be idempotent: {error}",
                    harness.name
                )
            });
        assert!(
            harness
                .store
                .load_pending_compaction_projections(&runtime_id)
                .await
                .unwrap()
                .is_empty(),
            "{}: finalized compaction must leave no pending projection",
            harness.name
        );
        let finalized_snapshot = harness
            .store
            .load_session_snapshot(&runtime_id)
            .await
            .unwrap()
            .expect("finalized session snapshot");
        let finalized_session: meerkat_core::Session =
            serde_json::from_slice(&finalized_snapshot).unwrap();
        assert!(
            finalized_session
                .compaction_projection_intents()
                .unwrap()
                .is_empty(),
            "{}: outbox finalization and session intent removal must be one transaction",
            harness.name
        );

        let replay_run_id = RunId::new();
        let replay_error = harness
            .store
            .atomic_apply(
                &runtime_id,
                Some(SessionDelta {
                    session_snapshot: pending_snapshot.clone(),
                }),
                make_receipt(replay_run_id.clone(), Vec::new(), 2),
                Vec::new(),
                Some(session.id().clone()),
            )
            .await
            .expect_err("finalized compaction tombstone must reject stale atomic replay");
        assert!(
            replay_error
                .to_string()
                .contains("finalized compaction intent"),
            "{}: stale replay must fail for the finalized-intent reason, got {replay_error}",
            harness.name
        );
        assert!(
            harness
                .store
                .load_boundary_receipt(&runtime_id, &replay_run_id, 2)
                .await
                .unwrap()
                .is_none(),
            "{}: rejected replay must roll back its boundary receipt",
            harness.name
        );
        assert_eq!(
            harness
                .store
                .load_session_snapshot(&runtime_id)
                .await
                .unwrap(),
            Some(finalized_snapshot),
            "{}: rejected replay must preserve the finalized snapshot",
            harness.name
        );

        harness
            .store
            .commit_session_snapshot(
                &runtime_id,
                SessionDelta {
                    session_snapshot: pending_snapshot,
                },
            )
            .await
            .expect_err("non-boundary writes must not bypass a finalized compaction tombstone");
    }
}

#[tokio::test]
async fn recovery_persistent_driver_contract_replays_missing_receipts_and_persists_retire_across_supported_backends()
 {
    for harness in supported_store_harnesses() {
        let runtime_id = make_runtime_id(harness.name);
        let run_id = RunId::new();
        let first = make_prompt("first recovery replay");
        let second = make_prompt("second recovery replay");
        let first_id = first.id().clone();
        let second_id = second.id().clone();
        let expected_ids = sorted_id_strings(vec![first_id.clone(), second_id.clone()]);

        harness
            .store
            .persist_input_state(
                &runtime_id,
                &persistable(applied_pending_state(&first, &run_id, 0)),
            )
            .await
            .unwrap();
        harness
            .store
            .persist_input_state(
                &runtime_id,
                &persistable(applied_pending_state(&second, &run_id, 0)),
            )
            .await
            .unwrap();

        let mut driver = PersistentRuntimeDriver::new(
            runtime_id.clone(),
            harness.store.clone(),
            memory_blob_store(),
        );
        let report = driver.recover().await.unwrap();
        assert_eq!(
            report.inputs_recovered, 2,
            "{}: missing boundary receipts should recover both contributors for replay",
            harness.name
        );
        assert_eq!(
            sorted_id_strings(driver.active_input_ids()),
            expected_ids,
            "{}: both contributors should remain active after replay recovery",
            harness.name
        );

        for input_id in [&first_id, &second_id] {
            assert!(
                driver.input_state(input_id).is_some(),
                "{}: driver should expose recovered input state",
                harness.name
            );
            assert_eq!(
                driver.inner_ref().input_phase(input_id),
                Some(InputLifecycleState::Queued),
                "{}: missing receipts should roll applied contributors back to queued",
                harness.name
            );
            let stored = harness
                .store
                .load_input_state(&runtime_id, input_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                stored.seed.phase,
                InputLifecycleState::Queued,
                "{}: recovered replay state should be persisted back to the store",
                harness.name
            );
        }

        let replayed_ids = vec![
            driver.contract_dequeue_next_for_recovery_tests().unwrap().0,
            driver.contract_dequeue_next_for_recovery_tests().unwrap().0,
        ];
        assert!(
            driver.contract_dequeue_next_for_recovery_tests().is_none(),
            "{}: only the recovered contributors should be queued for replay",
            harness.name
        );
        assert_eq!(
            sorted_id_strings(replayed_ids),
            expected_ids,
            "{}: replay queue should contain exactly the recovered contributors",
            harness.name
        );

        let retire_report = retire_runtime(&mut driver).await.unwrap();
        assert_eq!(
            retire_report.inputs_pending_drain, 2,
            "{}: retire should preserve the replayable contributors for later drain",
            harness.name
        );

        drop(driver);
    }
}

#[tokio::test]
async fn recovery_contract_preserves_durable_lifecycle_state_projection() {
    for harness in supported_store_harnesses() {
        for recovered_state in [
            RuntimeState::Retired,
            RuntimeState::Stopped,
            RuntimeState::Destroyed,
        ] {
            let session_id = SessionId::new();
            let runtime_id = LogicalRuntimeId::for_session(&session_id);
            let seeder = MeerkatMachine::persistent(harness.store.clone(), memory_blob_store());
            seeder
                .register_session(session_id.clone())
                .await
                .expect("register session");
            match recovered_state {
                RuntimeState::Retired => {
                    meerkat_runtime::RuntimeControlPlane::retire(&seeder, &runtime_id)
                        .await
                        .unwrap();
                }
                RuntimeState::Stopped => {
                    seeder
                        .stop_runtime_executor(&session_id, "seed stopped projection")
                        .await
                        .unwrap();
                }
                RuntimeState::Destroyed => {
                    meerkat_runtime::RuntimeControlPlane::destroy(&seeder, &runtime_id)
                        .await
                        .unwrap();
                }
                other => panic!("unexpected seeded projection state: {other}"),
            }
            drop(seeder);

            let machine = MeerkatMachine::persistent(harness.store.clone(), memory_blob_store());
            if recovered_state == RuntimeState::Destroyed {
                // Destroyed is terminal machine truth: cold re-registration is
                // rejected by the machine verdict instead of laundering the
                // terminal state into a successful registration.
                let err = machine
                    .register_session(session_id.clone())
                    .await
                    .expect_err("destroyed runtime must reject cold re-registration");
                assert!(
                    err.to_string().contains("Runtime destroyed"),
                    "{}: rejection must surface the destroyed terminal verdict, got {err:?}",
                    harness.name
                );
            } else {
                machine
                    .register_session(session_id.clone())
                    .await
                    .expect("register session");
                assert_eq!(
                    machine.runtime_state(&session_id).await.unwrap(),
                    recovered_state,
                    "{}: recovered {recovered_state} projection must remain machine lifecycle truth",
                    harness.name
                );
            }
            assert_eq!(
                load_runtime_state(harness.store.as_ref(), &runtime_id)
                    .await
                    .unwrap(),
                Some(recovered_state),
                "{}: recovered {recovered_state} projection must remain durable lifecycle truth after machine recovery",
                harness.name
            );
        }
    }
}

#[tokio::test]
async fn recovery_persistent_driver_contract_consumes_committed_boundary_contributors_across_supported_backends()
 {
    for harness in supported_store_harnesses() {
        let runtime_id = make_runtime_id(harness.name);
        let run_id = RunId::new();
        let first = make_prompt("first committed contribution");
        let second = make_prompt("second committed contribution");
        let first_id = first.id().clone();
        let second_id = second.id().clone();
        let receipt = make_receipt(run_id.clone(), vec![first_id.clone(), second_id.clone()], 0);

        harness
            .store
            .atomic_apply(
                &runtime_id,
                Some(SessionDelta {
                    session_snapshot: make_session_snapshot(),
                }),
                receipt.clone(),
                vec![
                    persistable(applied_pending_state(&first, &run_id, 0)),
                    persistable(applied_pending_state(&second, &run_id, 0)),
                ],
                None,
            )
            .await
            .unwrap();

        let mut driver = PersistentRuntimeDriver::new(
            runtime_id.clone(),
            harness.store.clone(),
            memory_blob_store(),
        );
        driver.recover().await.unwrap();

        assert!(
            driver.active_input_ids().is_empty(),
            "{}: committed contributors should not remain active after recovery",
            harness.name
        );
        assert!(
            driver.contract_dequeue_next_for_recovery_tests().is_none(),
            "{}: committed contributors should not be replayed after recovery",
            harness.name
        );
        assert_eq!(
            load_runtime_state(harness.store.as_ref(), &runtime_id)
                .await
                .unwrap(),
            Some(RuntimeState::Idle),
            "{}: recovery should persist the runtime back to an idle lifecycle state",
            harness.name
        );

        for input_id in [&first_id, &second_id] {
            assert_eq!(
                driver.inner_ref().input_phase(input_id),
                Some(InputLifecycleState::Consumed),
                "{}: committed contributors should recover as consumed",
                harness.name
            );
            assert_eq!(
                driver.inner_ref().input_terminal_outcome(input_id),
                Some(InputTerminalOutcome::Consumed),
                "{}: committed contributors should recover with a consumed terminal outcome",
                harness.name
            );

            let stored = harness
                .store
                .load_input_state(&runtime_id, input_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                stored.seed.phase,
                InputLifecycleState::Consumed,
                "{}: consumed recovery state should be persisted back to the store",
                harness.name
            );
        }

        let loaded_receipt = harness
            .store
            .load_boundary_receipt(&runtime_id, &run_id, 0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            loaded_receipt.contributing_input_ids,
            vec![first_id.clone(), second_id.clone()],
            "{}: committed receipt should preserve contributor ordering through recovery",
            harness.name
        );
    }
}
