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
#[cfg(feature = "sqlite-store")]
use meerkat_core::{IncrementalSessionStore as _, SessionStore as _};
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
    InMemoryRuntimeStore, RuntimeStore, SerializedSessionSnapshot, load_runtime_state,
};
use meerkat_runtime::traits::RuntimeDriver;
use meerkat_runtime::{EphemeralRuntimeDriver, MeerkatMachine, PersistentRuntimeDriver};
use meerkat_store::MemoryBlobStore;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uuid::Uuid;

#[cfg(feature = "sqlite-store")]
use meerkat_runtime::store::{
    PreparedHeadCanonicalProvisionalTail, PreparedRuntimeSessionCommit, SqliteRuntimeStore,
};
#[cfg(feature = "sqlite-store")]
use meerkat_store::SqliteSessionStore;

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

/// Test-only supported-floor encoder. Full-body materialization is deliberate
/// here: it constructs exact released 0.8.10 ingress bytes, never a current
/// runtime persistence path.
fn encode_as_released_0810_compaction_fixture(
    session: &meerkat_core::Session,
) -> serde_json::Value {
    let history = session
        .validated_transcript_history_state()
        .unwrap()
        .expect("fixture rewrite graph exists");
    assert_eq!(history.commit_count(), 1, "fixture has one rewrite");
    let commit = history.last_commit().expect("fixture rewrite commit");
    let (start, end) = commit.selection.bounds();
    let mut released_commit = serde_json::to_value(commit).unwrap();
    released_commit
        .as_object_mut()
        .unwrap()
        .remove("rewrite_generation");
    released_commit["selection"] = serde_json::json!({
        "type": "compaction_message_range",
        "range": { "start": start, "end": end }
    });
    let released_graph = serde_json::json!({
        "head": history.head(),
        "commits": [released_commit],
        "revisions": [
            history.materialize_revision(&commit.parent_revision).unwrap(),
            history.materialize_revision(&commit.revision).unwrap(),
        ],
        "digest_format": history.digest_format(),
    });
    let mut encoded = serde_json::to_value(session).unwrap();
    encoded["metadata"][meerkat_core::SESSION_TRANSCRIPT_HISTORY_STATE_KEY] = released_graph;
    encoded
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
    let encoded = encode_as_released_0810_compaction_fixture(&session);
    let mut session: meerkat_core::Session = serde_json::from_value(encoded).unwrap();
    let commit = session
        .validated_transcript_history_state()
        .unwrap()
        .unwrap()
        .last_commit()
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

/// A durably accepted content row never bound to any run: the current
/// never-started-input shape. Recovery must retain it for ordinary redelivery,
/// not infer consumption from its presence beside a completed candidate.
fn accepted_unbound_state(input: &Input) -> StoredInputState {
    let mut state = InputState::new_accepted(input.id().clone());
    state.persisted_input = Some(input.clone());
    state.durability = Some(InputDurability::Durable);
    stamp_runtime_metadata(&mut state, input);
    StoredInputState {
        state,
        seed: InputStateSeed {
            recovery_lane: Some(meerkat_core::types::HandlingMode::Queue),
            ..InputStateSeed::new_accepted()
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
                Some(SerializedSessionSnapshot {
                    session_snapshot: make_session_snapshot().into(),
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
                Some(SerializedSessionSnapshot {
                    session_snapshot: make_session_snapshot().into(),
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
                Some(SerializedSessionSnapshot {
                    session_snapshot: pending_snapshot.clone().into(),
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
                Some(SerializedSessionSnapshot {
                    session_snapshot: pending_snapshot.clone().into(),
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
                SerializedSessionSnapshot {
                    session_snapshot: pending_snapshot.into(),
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
async fn recovery_contract_normalizes_every_dead_process_phase_to_fresh_idle() {
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
            machine
                .register_session(session_id.clone())
                .await
                .expect("cold registration must replace the dead process shell");
            assert_eq!(
                machine.runtime_state(&session_id).await.unwrap(),
                RuntimeState::Idle,
                "{}: persisted {recovered_state} is observation of a dead process, not restorable authority",
                harness.name
            );
            assert_eq!(
                load_runtime_state(harness.store.as_ref(), &runtime_id)
                    .await
                    .unwrap(),
                Some(RuntimeState::Idle),
                "{}: persisted {recovered_state} must converge to a fresh unbound Idle shell",
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
                Some(SerializedSessionSnapshot {
                    session_snapshot: make_session_snapshot().into(),
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

/// Level 3 — the durable-tail recovery boundary is all-or-nothing.
///
/// A recovered durable tail commits through the SAME `atomic_apply` boundary
/// as an ordinary completed run: the recovered session snapshot (revision N+1
/// content over the committed revision N head), the recovered run's boundary
/// receipt, and the input-state terminalization records become visible
/// TOGETHER — and a stale pre-recovery replay makes NOTHING visible.
#[tokio::test]
async fn atomic_apply_recovery_boundary_is_all_or_nothing() {
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, Message, StopReason, UserMessage,
    };

    for harness in supported_store_harnesses() {
        let name = harness.name;
        let runtime_id = make_runtime_id(name);

        // Committed authority head at revision N: the last boundary that
        // actually committed before shutdown.
        let mut committed = meerkat_core::Session::new();
        committed.push(Message::User(UserMessage::text(
            "committed turn".to_string(),
        )));
        let committed_snapshot = serde_json::to_vec(&committed).unwrap();
        harness
            .store
            .atomic_apply(
                &runtime_id,
                Some(SerializedSessionSnapshot {
                    session_snapshot: committed_snapshot.clone().into(),
                }),
                make_receipt(RunId::new(), vec![], 0),
                vec![],
                Some(committed.id().clone()),
            )
            .await
            .unwrap();

        // The durable tail: revision N+1 content — a completed turn whose
        // boundary commit lost the race with shutdown.
        let mut recovered = committed.clone();
        recovered.push(Message::BlockAssistant(BlockAssistantMessage {
            blocks: vec![AssistantBlock::Text {
                text: "durable tail reply".to_string(),
                meta: None,
            }],
            stop_reason: StopReason::EndTurn,
            identity: meerkat_core::types::TranscriptMessageIdentity::default(),
            created_at: meerkat_core::types::message_timestamp_now(),
        }));
        let recovered_snapshot = serde_json::to_vec(&recovered).unwrap();

        let recovered_run = RunId::new();
        let recovered_input = InputId::new();
        let recovered_receipt = RunBoundaryReceipt {
            run_id: recovered_run.clone(),
            boundary: RunApplyBoundary::RunStart,
            contributing_input_ids: vec![recovered_input.clone()],
            conversation_digest: Some(format!("{name}-recovered-boundary-digest")),
            message_count: recovered.messages().len(),
            sequence: 1,
        };
        // Input-state terminalization: the recovered run's input closes
        // Consumed. Recovery TERMINALIZES the original input; it never
        // requeues it.
        let terminalized = StoredInputState {
            state: InputState::new_accepted(recovered_input.clone()),
            seed: InputStateSeed {
                phase: InputLifecycleState::Consumed,
                last_run_id: Some(recovered_run.clone()),
                last_boundary_sequence: Some(1),
                terminal_outcome: Some(InputTerminalOutcome::Consumed),
                attempt_count: 1,
                admission_sequence: None,
                recovery_lane: None,
            },
        };

        harness
            .store
            .atomic_apply(
                &runtime_id,
                Some(SerializedSessionSnapshot {
                    session_snapshot: recovered_snapshot.clone().into(),
                }),
                recovered_receipt.clone(),
                vec![persistable(terminalized)],
                Some(recovered.id().clone()),
            )
            .await
            .unwrap_or_else(|err| {
                panic!("{name}: the recovered boundary must commit atomically: {err}")
            });

        // ALL visible together: snapshot, receipt, terminalized input.
        assert_eq!(
            harness
                .store
                .load_session_snapshot(&runtime_id)
                .await
                .unwrap(),
            Some(Arc::new(recovered_snapshot.clone())),
            "{name}: recovered snapshot must be the durable head"
        );
        assert_eq!(
            harness
                .store
                .load_boundary_receipt(&runtime_id, &recovered_run, 1)
                .await
                .unwrap(),
            Some(recovered_receipt),
            "{name}: the recovered run's boundary receipt must be durable"
        );
        let rows = harness
            .store
            .load_input_states_strict(&runtime_id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "{name}: exactly the terminalized input row");
        assert_eq!(rows[0].state.input_id, recovered_input);
        assert_eq!(rows[0].seed.phase, InputLifecycleState::Consumed);
        assert_eq!(
            rows[0].seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed),
            "{name}: recovery must terminalize, not requeue"
        );
        assert_eq!(rows[0].seed.last_run_id, Some(recovered_run));

        // Failure case: a stale writer replays the PRE-recovery snapshot
        // (revision N) with a fresh receipt and input record. The store's
        // supersession check must reject the WHOLE boundary — snapshot,
        // receipt, AND input row stay invisible.
        let stale_run = RunId::new();
        let stale_prompt = make_prompt("stale replay input");
        let stale_input = stale_prompt.id().clone();
        let error = match harness
            .store
            .atomic_apply(
                &runtime_id,
                Some(SerializedSessionSnapshot {
                    session_snapshot: committed_snapshot.clone().into(),
                }),
                make_receipt(stale_run.clone(), vec![stale_input.clone()], 2),
                vec![persistable(applied_pending_state(
                    &stale_prompt,
                    &stale_run,
                    2,
                ))],
                Some(recovered.id().clone()),
            )
            .await
        {
            Ok(()) => panic!("{name}: a stale pre-recovery replay must be rejected"),
            Err(err) => err,
        };
        assert!(
            matches!(
                error,
                meerkat_runtime::store::RuntimeStoreError::SessionSnapshotSuperseded { .. }
            ),
            "{name}: expected SessionSnapshotSuperseded, got {error:?}"
        );
        assert_eq!(
            harness
                .store
                .load_session_snapshot(&runtime_id)
                .await
                .unwrap(),
            Some(Arc::new(recovered_snapshot)),
            "{name}: the recovered head must be retained"
        );
        assert_eq!(
            harness
                .store
                .load_boundary_receipt(&runtime_id, &stale_run, 2)
                .await
                .unwrap(),
            None,
            "{name}: the rejected boundary's receipt must not be visible"
        );
        let rows = harness
            .store
            .load_input_states_strict(&runtime_id)
            .await
            .unwrap();
        assert_eq!(
            rows.len(),
            1,
            "{name}: the rejected boundary's input row must not be visible"
        );
        assert_eq!(rows[0].state.input_id, recovered_input);
    }
}

#[tokio::test]
async fn whole_blob_provisional_recovery_promotes_store_owned_candidate() {
    use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, Message, StopReason, TranscriptMessageIdentity,
        UserMessage,
    };
    use meerkat_runtime::recovery::{DurableTailRecoveryOutcome, recover_durable_tail};
    use meerkat_runtime::store::{PreparedRuntimeSessionCommit, PreparedWholeBlobProvisionalTail};

    let store = InMemoryRuntimeStore::new();
    let run_id = RunId::new();
    let mut committed = meerkat_core::Session::new();
    committed.push(Message::User(UserMessage::text("committed input")));
    let session_id = committed.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let committed_authority = store
        .commit_prepared_session_boundary(
            &runtime_id,
            PreparedRuntimeSessionCommit::snapshot_only(
                BoundSessionCommit::sealed(Arc::new(committed.clone())).unwrap(),
            ),
        )
        .await
        .unwrap()
        .authority()
        .and_then(|authority| authority.whole_blob())
        .cloned()
        .expect("initial boundary returns WholeBlob store authority");

    let mut candidate = committed;
    candidate.push(Message::BlockAssistant(BlockAssistantMessage {
        blocks: vec![AssistantBlock::Text {
            text: "durable whole-blob reply".to_string(),
            meta: None,
        }],
        stop_reason: StopReason::EndTurn,
        identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
        created_at: meerkat_core::types::message_timestamp_now(),
    }));
    let candidate = BoundSessionCommit::sealed(Arc::new(candidate)).unwrap();
    let provisional = store
        .write_prepared_whole_blob_provisional_tail(
            &runtime_id,
            PreparedWholeBlobProvisionalTail::prepare(
                committed_authority,
                run_id.clone(),
                1,
                &candidate,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(provisional.candidate_sequence(), 1);

    let DurableTailRecoveryOutcome::Committed {
        recovered,
        boundary_sequence,
        ..
    } = recover_durable_tail(&store, &session_id)
        .await
        .expect("store-owned WholeBlob provisional tail recovers")
    else {
        panic!("WholeBlob provisional tail must commit");
    };
    assert_eq!(boundary_sequence, 1);
    let Some(Message::BlockAssistant(last)) = recovered.messages().last() else {
        panic!("recovered WholeBlob tail must retain the assistant reply");
    };
    assert_eq!(last.identity.run_id.as_ref(), Some(&run_id));
    assert!(
        store
            .load_whole_blob_provisional_tail(&runtime_id)
            .await
            .unwrap()
            .is_none(),
        "successful recovery promotes and clears the provisional candidate"
    );
}

#[tokio::test]
async fn whole_blob_interrupted_recovery_installs_one_sealed_repair_artifact() {
    use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, Message, StopReason, ToolResult,
        TranscriptMessageIdentity, UserMessage,
    };
    use meerkat_runtime::recovery::{DurableTailRecoveryOutcome, recover_durable_tail};
    use meerkat_runtime::store::{PreparedRuntimeSessionCommit, PreparedWholeBlobProvisionalTail};

    let store = InMemoryRuntimeStore::new();
    let run_id = RunId::new();
    let mut committed = meerkat_core::Session::new();
    committed.push(Message::User(UserMessage::text("invoke durable tool")));
    let session_id = committed.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let committed_authority = store
        .commit_prepared_session_boundary(
            &runtime_id,
            PreparedRuntimeSessionCommit::snapshot_only(
                BoundSessionCommit::sealed(Arc::new(committed.clone())).unwrap(),
            ),
        )
        .await
        .unwrap()
        .authority()
        .and_then(|authority| authority.whole_blob())
        .cloned()
        .expect("initial boundary returns WholeBlob store authority");

    let mut interrupted = committed;
    interrupted.push(Message::BlockAssistant(BlockAssistantMessage {
        blocks: vec![AssistantBlock::ToolUse {
            id: "durable-call".to_string(),
            name: "durable_tool".to_string(),
            args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
            meta: None,
        }],
        stop_reason: StopReason::ToolUse,
        identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
        created_at: meerkat_core::types::message_timestamp_now(),
    }));
    interrupted.push(Message::tool_results(vec![ToolResult::new(
        "durable-call".to_string(),
        "durable result".to_string(),
        false,
    )]));
    let interrupted_message_count = interrupted.messages().len();
    let candidate = BoundSessionCommit::sealed(Arc::new(interrupted)).unwrap();
    store
        .write_prepared_whole_blob_provisional_tail(
            &runtime_id,
            PreparedWholeBlobProvisionalTail::prepare(committed_authority, run_id, 1, &candidate)
                .unwrap(),
        )
        .await
        .unwrap();

    let DurableTailRecoveryOutcome::Committed { recovered, .. } =
        recover_durable_tail(&store, &session_id)
            .await
            .expect("interrupted WholeBlob candidate recovers through sealed repair")
    else {
        panic!("interrupted WholeBlob candidate must commit");
    };
    assert_eq!(recovered.messages().len(), interrupted_message_count + 1);
    assert!(
        matches!(recovered.messages().last(), Some(Message::SystemNotice(_))),
        "repair appends exactly one deterministic interruption notice"
    );
    let stored = store
        .load_session_snapshot(&runtime_id)
        .await
        .unwrap()
        .expect("repaired WholeBlob body is committed");
    let stored = meerkat_core::Session::from_persisted_bytes(stored.as_slice()).unwrap();
    assert_eq!(
        stored.messages(),
        recovered.messages(),
        "the store installs the exact sealed repair artifact returned by recovery"
    );
    assert!(
        store
            .load_whole_blob_provisional_tail(&runtime_id)
            .await
            .unwrap()
            .is_none(),
        "repaired recovery consumes the exact provisional candidate"
    );
}

#[tokio::test]
async fn whole_blob_recovery_uses_latest_same_run_candidate_sequence() {
    use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, Message, StopReason, ToolResult,
        TranscriptMessageIdentity, UserMessage,
    };
    use meerkat_runtime::recovery::{DurableTailRecoveryOutcome, recover_durable_tail};
    use meerkat_runtime::store::{PreparedRuntimeSessionCommit, PreparedWholeBlobProvisionalTail};

    let store = InMemoryRuntimeStore::new();
    let run_id = RunId::new();
    let mut committed = meerkat_core::Session::new();
    committed.push(Message::User(UserMessage::text("invoke durable tool")));
    let session_id = committed.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let committed_authority = store
        .commit_prepared_session_boundary(
            &runtime_id,
            PreparedRuntimeSessionCommit::snapshot_only(
                BoundSessionCommit::sealed(Arc::new(committed.clone())).unwrap(),
            ),
        )
        .await
        .unwrap()
        .authority()
        .and_then(|authority| authority.whole_blob())
        .cloned()
        .expect("initial boundary returns WholeBlob store authority");

    let mut first_candidate = committed;
    first_candidate.push(Message::BlockAssistant(BlockAssistantMessage {
        blocks: vec![AssistantBlock::ToolUse {
            id: "durable-call".to_string(),
            name: "durable_tool".to_string(),
            args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
            meta: None,
        }],
        stop_reason: StopReason::ToolUse,
        identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
        created_at: meerkat_core::types::message_timestamp_now(),
    }));
    let first = store
        .write_prepared_whole_blob_provisional_tail(
            &runtime_id,
            PreparedWholeBlobProvisionalTail::prepare(
                committed_authority.clone(),
                run_id.clone(),
                1,
                &BoundSessionCommit::sealed(Arc::new(first_candidate.clone())).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.candidate_sequence(), 1);

    let mut latest_candidate = first_candidate;
    latest_candidate.push(Message::tool_results(vec![ToolResult::new(
        "durable-call".to_string(),
        "durable result".to_string(),
        false,
    )]));
    latest_candidate.push(Message::BlockAssistant(BlockAssistantMessage {
        blocks: vec![AssistantBlock::Text {
            text: "latest durable reply".to_string(),
            meta: None,
        }],
        stop_reason: StopReason::EndTurn,
        identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
        created_at: meerkat_core::types::message_timestamp_now(),
    }));
    let latest = store
        .write_prepared_whole_blob_provisional_tail(
            &runtime_id,
            PreparedWholeBlobProvisionalTail::prepare(
                committed_authority,
                run_id,
                2,
                &BoundSessionCommit::sealed(Arc::new(latest_candidate.clone())).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(latest.candidate_sequence(), 2);

    let DurableTailRecoveryOutcome::Committed { recovered, .. } =
        recover_durable_tail(&store, &session_id)
            .await
            .expect("latest same-run WholeBlob candidate recovers")
    else {
        panic!("latest same-run WholeBlob candidate must commit");
    };
    assert_eq!(
        recovered.messages().last(),
        latest_candidate.messages().last(),
        "recovery promotes the latest candidate, not the first provisional body"
    );
    assert!(
        store
            .load_whole_blob_provisional_tail(&runtime_id)
            .await
            .unwrap()
            .is_none(),
        "latest candidate promotion consumes the provisional authority"
    );
}

/// The HeadCanonical recovery profile uses one SQLite transaction to own
/// runtime authority, exact physical rows/head CAS, the 0.8.10 receipt
/// migration, lifecycle/input fences, and the recovered boundary.
///
/// This also pins the duplicate-turn regression fixed in
/// `observe_candidate_run_inputs`: encountering one genuinely unbound
/// content row must not discard other rows that exact run-binding evidence
/// proved were consumed by the adopted tail.
#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn head_canonical_recovery_uses_only_store_owned_source_and_migrates_floor_receipt() {
    use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
    use meerkat_core::session_store::PreparedHeadCanonicalMutation;
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, Message, StopReason, UserMessage,
    };
    use meerkat_runtime::recovery::{DurableTailRecoveryOutcome, recover_durable_tail};

    let tempdir = TempDir::new().unwrap();
    let db_path = tempdir.path().join("head-canonical-recovery.sqlite3");
    let store = SqliteRuntimeStore::new_head_canonical(&db_path).unwrap();
    let session_store = SqliteSessionStore::open(&db_path).unwrap();
    let candidate_run = RunId::new();

    let mut committed = meerkat_core::Session::new();
    committed.push(Message::User(UserMessage::text(
        "committed turn".to_string(),
    )));
    let session_id = committed.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);

    let mutation = PreparedHeadCanonicalMutation::prepare(&committed, None).unwrap();
    let document = BoundSessionCommit::sealed(Arc::new(committed.clone()))
        .unwrap()
        .with_head_canonical_mutation(mutation)
        .unwrap();
    let committed_authority = store
        .commit_prepared_session_boundary(
            &runtime_id,
            PreparedRuntimeSessionCommit::snapshot_only(document),
        )
        .await
        .unwrap()
        .authority()
        .and_then(|authority| authority.head_canonical())
        .cloned()
        .expect("initial boundary returns HeadCanonical store authority");

    let staged_prompt = make_prompt("staged for the candidate run");
    let receipt_prompt = make_prompt("named by the committed mid-run receipt");
    let retained_prompt = make_prompt("genuinely unbound retained input");
    let staged_id = staged_prompt.id().clone();
    let receipt_id = receipt_prompt.id().clone();
    let retained_id = retained_prompt.id().clone();

    // This is the exact supported-floor shape: a real sequence-1 receipt
    // whose conversation digest field was absent. Recovery must derive that
    // digest from its store-owned physical transcript and enrich this exact
    // row inside the eventual recovery transaction.
    let floor_receipt = RunBoundaryReceipt {
        run_id: candidate_run.clone(),
        boundary: RunApplyBoundary::RunCheckpoint,
        contributing_input_ids: vec![receipt_id.clone()],
        conversation_digest: None,
        message_count: committed.messages().len(),
        sequence: 1,
    };
    store
        .commit_prepared_session_boundary(
            &runtime_id,
            PreparedRuntimeSessionCommit::success(
                None,
                floor_receipt,
                vec![
                    persistable(applied_pending_state(&staged_prompt, &candidate_run, 1)),
                    persistable(accepted_unbound_state(&receipt_prompt)),
                    persistable(accepted_unbound_state(&retained_prompt)),
                ],
                Some(session_id.clone()),
            ),
        )
        .await
        .unwrap();
    let retained_before = store
        .load_input_state(&runtime_id, &retained_id)
        .await
        .unwrap()
        .unwrap();
    for input_id in [&staged_id, &receipt_id] {
        assert!(
            store
                .load_input_state(&runtime_id, input_id)
                .await
                .unwrap()
                .unwrap()
                .state
                .persisted_input
                .is_some(),
            "pre-recovery staged/accepted contributors must retain attribution and redelivery bytes"
        );
    }

    // Advance only the physical canonical head. Runtime authority deliberately
    // remains at `committed`, exactly modelling the lost boundary race.
    let mut physical_head = committed.clone();
    physical_head.push(Message::BlockAssistant(BlockAssistantMessage {
        blocks: vec![AssistantBlock::Text {
            text: "durable tail reply".to_string(),
            meta: None,
        }],
        stop_reason: StopReason::EndTurn,
        identity: meerkat_core::types::TranscriptMessageIdentity::default()
            .with_run_id(candidate_run.clone()),
        created_at: meerkat_core::types::message_timestamp_now(),
    }));
    let observed_head = session_store
        .load_head(&session_id)
        .await
        .unwrap()
        .expect("committed physical head");
    let physical_mutation =
        PreparedHeadCanonicalMutation::prepare(&physical_head, Some(observed_head)).unwrap();
    let provisional = store
        .write_prepared_head_canonical_provisional_tail(
            &runtime_id,
            PreparedHeadCanonicalProvisionalTail::prepare(
                committed_authority,
                candidate_run.clone(),
                physical_mutation.successor_head(),
                physical_mutation.successor_head_token(),
                &physical_head,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(provisional.candidate_sequence(), 1);
    session_store
        .apply_prepared_head_canonical_mutation(&physical_mutation)
        .await
        .unwrap();

    // The public API accepts only the store and session identity. Every
    // Authority, row, class, run, digest, provisional sequence, and CAS facts
    // come from the opaque store-owned source.
    let outcome = recover_durable_tail(&store, &session_id)
        .await
        .unwrap_or_else(|error| panic!("head-canonical recovery must commit: {error}"));
    let recovered = match outcome {
        DurableTailRecoveryOutcome::Committed {
            disposition,
            boundary_sequence,
            recovered,
        } => {
            assert_eq!(format!("{disposition:?}"), "CommitCompletedRetainInputs");
            assert_eq!(boundary_sequence, 2);
            *recovered
        }
        other => panic!("head-canonical recovery did not commit: {other:?}"),
    };
    let conversation_digest = recovered.transcript_content_digest().unwrap();

    for (label, input_id) in [
        ("staging-bound", &staged_id),
        ("receipt-bound", &receipt_id),
    ] {
        let stored = store
            .load_input_state(&runtime_id, input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.seed.phase,
            InputLifecycleState::Consumed,
            "the {label} row was proved consumed and must be terminalized"
        );
        assert_eq!(
            stored.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        assert_eq!(stored.seed.last_run_id, Some(candidate_run.clone()));
        assert_eq!(stored.seed.last_boundary_sequence, Some(2));
        assert!(
            stored.state.persisted_input.is_none(),
            "the {label} row committed its recovery receipt and must retire redelivery bytes"
        );
    }

    let retained_after = store
        .load_input_state(&runtime_id, &retained_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained_after.seed, retained_before.seed);
    assert_eq!(retained_after.seed.last_run_id, None);
    assert_eq!(retained_after.seed.terminal_outcome, None);
    assert_eq!(
        serde_json::to_value(retained_after.state.persisted_input.as_ref()).unwrap(),
        serde_json::to_value(retained_before.state.persisted_input.as_ref()).unwrap(),
        "accepted-unconsumed content remains byte-for-byte redeliverable"
    );
    assert!(retained_after.state.persisted_input.is_some());

    // Reopen the durable store: payload retirement must not erase terminal
    // facts, while the unrelated accepted input still carries its exact
    // redelivery material.
    let restarted_store = SqliteRuntimeStore::new_head_canonical(&db_path).unwrap();
    for input_id in [&staged_id, &receipt_id] {
        let restarted = restarted_store
            .load_input_state(&runtime_id, input_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restarted.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(
            restarted.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        assert!(restarted.state.persisted_input.is_none());
    }
    let restarted_retained = restarted_store
        .load_input_state(&runtime_id, &retained_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restarted_retained.seed.phase, InputLifecycleState::Accepted);
    assert!(restarted_retained.state.persisted_input.is_some());

    let migrated_floor_receipt = store
        .load_boundary_receipt(&runtime_id, &candidate_run, 1)
        .await
        .unwrap()
        .expect("supported-floor receipt remains durable");
    assert_eq!(
        migrated_floor_receipt.conversation_digest,
        Some(
            recovered
                .transcript_prefix_digest(committed.messages().len())
                .unwrap()
        )
    );
    let recovered_receipt = store
        .load_boundary_receipt(&runtime_id, &candidate_run, 2)
        .await
        .unwrap()
        .expect("recovered receipt is durable");
    assert_eq!(
        sorted_id_strings(recovered_receipt.contributing_input_ids.clone()),
        sorted_id_strings(vec![staged_id, receipt_id])
    );
    assert_eq!(
        recovered_receipt.conversation_digest,
        Some(conversation_digest)
    );
    assert_eq!(recovered_receipt.message_count, recovered.messages().len());

    let stored_recovered = session_store
        .load(&session_id)
        .await
        .unwrap()
        .expect("recovered physical head");
    assert_eq!(
        meerkat_core::session_store::session_projection_cas_token(&stored_recovered).unwrap(),
        meerkat_core::session_store::session_projection_cas_token(&recovered).unwrap()
    );
    let authority = store
        .load_session_boundary_authority(&runtime_id)
        .await
        .unwrap()
        .expect("recovered runtime authority");
    let authority = authority
        .head_canonical()
        .expect("recovered authority remains HeadCanonical");
    assert_eq!(
        authority.committed_head_token(),
        meerkat_core::session_head_cas_token(authority.boundary_head()).unwrap()
    );
    assert_eq!(
        load_runtime_state(&store, &runtime_id).await.unwrap(),
        Some(RuntimeState::Idle)
    );

    let aligned = recover_durable_tail(&store, &session_id)
        .await
        .expect("an already-committed recovery must converge");
    let DurableTailRecoveryOutcome::AlreadyAligned { recovered: aligned } = aligned else {
        panic!("equal store-owned authority/head must return AlreadyAligned");
    };
    assert_eq!(
        meerkat_core::session_store::session_projection_cas_token(aligned.as_ref()).unwrap(),
        meerkat_core::session_store::session_projection_cas_token(&recovered).unwrap()
    );
    assert!(
        store
            .load_boundary_receipt(&runtime_id, &candidate_run, 3)
            .await
            .unwrap()
            .is_none(),
        "benign alignment must not mint a phantom recovery boundary"
    );
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn head_canonical_incomplete_intent_is_discarded_without_advancing_the_session() {
    use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
    use meerkat_core::session_store::PreparedHeadCanonicalMutation;
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, Message, StopReason, TranscriptMessageIdentity,
        UserMessage,
    };
    use meerkat_runtime::recovery::{DurableTailRecoveryOutcome, recover_durable_tail};

    let tempdir = TempDir::new().unwrap();
    let db_path = tempdir
        .path()
        .join("head-canonical-incomplete-intent.sqlite3");
    let store = SqliteRuntimeStore::new_head_canonical(&db_path).unwrap();
    let session_store = SqliteSessionStore::open(&db_path).unwrap();
    let run_id = RunId::new();

    let mut committed = meerkat_core::Session::new();
    committed.push(Message::User(UserMessage::text("committed input")));
    let session_id = committed.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let root = PreparedHeadCanonicalMutation::prepare(&committed, None).unwrap();
    let document = BoundSessionCommit::sealed(Arc::new(committed.clone()))
        .unwrap()
        .with_head_canonical_mutation(root)
        .unwrap();
    let committed_authority = store
        .commit_prepared_session_boundary(
            &runtime_id,
            PreparedRuntimeSessionCommit::snapshot_only(document),
        )
        .await
        .unwrap()
        .authority()
        .and_then(|authority| authority.head_canonical())
        .cloned()
        .expect("root HeadCanonical authority");

    let mut candidate = committed.clone();
    candidate.push(Message::BlockAssistant(BlockAssistantMessage {
        blocks: vec![AssistantBlock::Text {
            text: "candidate that was never physically applied".to_string(),
            meta: None,
        }],
        stop_reason: StopReason::EndTurn,
        identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
        created_at: meerkat_core::types::message_timestamp_now(),
    }));
    let observed = session_store
        .load_head(&session_id)
        .await
        .unwrap()
        .expect("committed physical head");
    let candidate_mutation =
        PreparedHeadCanonicalMutation::prepare(&candidate, Some(observed)).unwrap();
    assert!(
        PreparedHeadCanonicalProvisionalTail::prepare(
            committed_authority.clone(),
            run_id.clone(),
            candidate_mutation.successor_head(),
            "head-cas:wrong-token",
            &candidate,
        )
        .is_err(),
        "a caller cannot mint an intent whose token differs from its exact successor head"
    );
    let incomplete = store
        .write_prepared_head_canonical_provisional_tail(
            &runtime_id,
            PreparedHeadCanonicalProvisionalTail::prepare(
                committed_authority,
                run_id,
                candidate_mutation.successor_head(),
                candidate_mutation.successor_head_token(),
                &candidate,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(incomplete.candidate_sequence(), 1);

    let DurableTailRecoveryOutcome::AlreadyAligned { recovered } =
        recover_durable_tail(&store, &session_id)
            .await
            .expect("recovery discards an unapplied first intent and reloads")
    else {
        panic!("unapplied first intent must converge to the committed base");
    };
    assert_eq!(recovered.messages(), committed.messages());
    assert!(
        store
            .load_head_canonical_provisional_tail(&runtime_id)
            .await
            .unwrap()
            .is_none(),
        "the exact unapplied intent is removed"
    );
}

#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn head_canonical_recovery_uses_latest_same_run_physical_candidate() {
    use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
    use meerkat_core::session_store::PreparedHeadCanonicalMutation;
    use meerkat_core::types::{
        AssistantBlock, BlockAssistantMessage, Message, StopReason, ToolResult,
        TranscriptMessageIdentity, UserMessage,
    };
    use meerkat_runtime::recovery::{DurableTailRecoveryOutcome, recover_durable_tail};

    let tempdir = TempDir::new().unwrap();
    let db_path = tempdir
        .path()
        .join("head-canonical-latest-same-run.sqlite3");
    let store = SqliteRuntimeStore::new_head_canonical(&db_path).unwrap();
    let session_store = SqliteSessionStore::open(&db_path).unwrap();
    let run_id = RunId::new();

    let mut committed = meerkat_core::Session::new();
    committed.push(Message::User(UserMessage::text("invoke durable tool")));
    let session_id = committed.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let root = PreparedHeadCanonicalMutation::prepare(&committed, None).unwrap();
    let document = BoundSessionCommit::sealed(Arc::new(committed.clone()))
        .unwrap()
        .with_head_canonical_mutation(root)
        .unwrap();
    let committed_authority = store
        .commit_prepared_session_boundary(
            &runtime_id,
            PreparedRuntimeSessionCommit::snapshot_only(document),
        )
        .await
        .unwrap()
        .authority()
        .and_then(|authority| authority.head_canonical())
        .cloned()
        .expect("root HeadCanonical authority");

    let mut first_candidate = committed;
    first_candidate.push(Message::BlockAssistant(BlockAssistantMessage {
        blocks: vec![AssistantBlock::ToolUse {
            id: "durable-call".to_string(),
            name: "durable_tool".to_string(),
            args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
            meta: None,
        }],
        stop_reason: StopReason::ToolUse,
        identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
        created_at: meerkat_core::types::message_timestamp_now(),
    }));
    let observed = session_store
        .load_head(&session_id)
        .await
        .unwrap()
        .expect("root physical head");
    let first_mutation =
        PreparedHeadCanonicalMutation::prepare(&first_candidate, Some(observed)).unwrap();
    let first = store
        .write_prepared_head_canonical_provisional_tail(
            &runtime_id,
            PreparedHeadCanonicalProvisionalTail::prepare(
                committed_authority.clone(),
                run_id.clone(),
                first_mutation.successor_head(),
                first_mutation.successor_head_token(),
                &first_candidate,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    session_store
        .apply_prepared_head_canonical_mutation(&first_mutation)
        .await
        .unwrap();
    assert_eq!(first.candidate_sequence(), 1);

    let observed = session_store
        .load_head(&session_id)
        .await
        .unwrap()
        .expect("first physical head");
    let mut latest_candidate = session_store
        .load(&session_id)
        .await
        .unwrap()
        .expect("first physical materialization");
    latest_candidate.push(Message::tool_results(vec![ToolResult::new(
        "durable-call".to_string(),
        "durable result".to_string(),
        false,
    )]));
    latest_candidate.push(Message::BlockAssistant(BlockAssistantMessage {
        blocks: vec![AssistantBlock::Text {
            text: "latest durable reply".to_string(),
            meta: None,
        }],
        stop_reason: StopReason::EndTurn,
        identity: TranscriptMessageIdentity::default().with_run_id(run_id.clone()),
        created_at: meerkat_core::types::message_timestamp_now(),
    }));
    let latest_mutation =
        PreparedHeadCanonicalMutation::prepare(&latest_candidate, Some(observed)).unwrap();
    let latest = store
        .write_prepared_head_canonical_provisional_tail(
            &runtime_id,
            PreparedHeadCanonicalProvisionalTail::prepare(
                committed_authority,
                run_id.clone(),
                latest_mutation.successor_head(),
                latest_mutation.successor_head_token(),
                &latest_candidate,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    session_store
        .apply_prepared_head_canonical_mutation(&latest_mutation)
        .await
        .unwrap();
    assert_eq!(latest.candidate_sequence(), 2);
    assert_eq!(
        latest.physical_store_revision(),
        first.physical_store_revision() + 1
    );

    let DurableTailRecoveryOutcome::Committed {
        recovered,
        boundary_sequence,
        ..
    } = recover_durable_tail(&store, &session_id)
        .await
        .expect("latest same-run physical candidate recovers")
    else {
        panic!("latest same-run physical candidate must commit");
    };
    assert_eq!(boundary_sequence, 1);
    assert_eq!(
        recovered.messages().last(),
        latest_candidate.messages().last(),
        "recovery commits the latest physical candidate, not the first provisional write"
    );
    assert!(
        store
            .load_head_canonical_provisional_tail(&runtime_id)
            .await
            .unwrap()
            .is_none(),
        "successful recovery promotes and clears the provisional authority"
    );
}

#[cfg(feature = "sqlite-store")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedRealmFixtureManifest {
    schema_version: u32,
    fixture_id: String,
    data_classification: String,
    producer: ReleasedRealmFixtureProducer,
    realm: ReleasedRealmFixtureLayout,
    files: Vec<ReleasedRealmFixtureFile>,
    expected: ReleasedRealmFixtureExpectations,
}

#[cfg(feature = "sqlite-store")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedRealmFixtureProducer {
    artifact_origin: String,
    product: String,
    meerkat_version: String,
    binary_name: String,
    binary_version_output: String,
    binary_sha256: String,
    source_release: String,
    capture_receipt_path: String,
    capture_receipt_sha256: String,
}

#[cfg(feature = "sqlite-store")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedRealmFixtureLayout {
    root: String,
    realm_id: String,
    manifest: String,
    sqlite_database: String,
    pre_upgrade_ledgers: Vec<ReleasedRealmFixtureLedger>,
}

#[cfg(feature = "sqlite-store")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedRealmFixtureLedger {
    domain: String,
    version: i64,
}

#[cfg(feature = "sqlite-store")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedRealmFixtureFile {
    path: String,
    bytes: u64,
    sha256: String,
}

#[cfg(feature = "sqlite-store")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedRealmFixtureExpectations {
    sessions: Vec<ReleasedRealmFixtureSession>,
    runtime_snapshots: Vec<ReleasedRealmFixtureRuntimeSnapshot>,
    consumed_inputs: Vec<ReleasedRealmFixtureConsumedInput>,
}

#[cfg(feature = "sqlite-store")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedRealmFixtureSession {
    session_id: String,
    strand: String,
    message_count: usize,
    head_revision: String,
    rewrite_count: usize,
    messages: Vec<ReleasedRealmFixtureMessage>,
}

#[cfg(feature = "sqlite-store")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedRealmFixtureMessage {
    sequence: i64,
    bytes: u64,
    sha256: String,
}

#[cfg(feature = "sqlite-store")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedRealmFixtureConsumedInput {
    runtime_id: String,
    input_id: String,
    last_run_id: String,
    last_boundary_sequence: u64,
    bytes: u64,
    sha256: String,
}

#[cfg(feature = "sqlite-store")]
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasedRealmFixtureRuntimeSnapshot {
    runtime_id: String,
    session_id: String,
    bytes: u64,
    sha256: String,
}

#[cfg(feature = "sqlite-store")]
fn released_realm_fixture_relative(raw: &str) -> std::path::PathBuf {
    use std::path::Component;

    let path = std::path::Path::new(raw);
    assert!(!path.as_os_str().is_empty(), "fixture path is empty");
    assert!(!path.is_absolute(), "fixture path must be relative: {raw}");
    assert!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_))),
        "fixture path is not normalized: {raw}"
    );
    path.to_path_buf()
}

#[cfg(feature = "sqlite-store")]
fn released_realm_fixture_sha256(path: &std::path::Path) -> String {
    use std::fmt::Write as _;
    use std::io::Read as _;

    let mut file = std::fs::File::open(path)
        .unwrap_or_else(|error| panic!("cannot hash fixture file {}: {error}", path.display()));
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .unwrap_or_else(|error| panic!("cannot read fixture file {}: {error}", path.display()));
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut encoded = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut encoded, "{byte:02x}").unwrap();
    }
    encoded
}

#[cfg(feature = "sqlite-store")]
fn collect_released_realm_fixture_files(
    corpus: &std::path::Path,
    directory: &std::path::Path,
    files: &mut std::collections::BTreeSet<String>,
) {
    for entry in std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("cannot list fixture {}: {error}", directory.display()))
    {
        let entry = entry.unwrap_or_else(|error| panic!("cannot read fixture entry: {error}"));
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)
            .unwrap_or_else(|error| panic!("cannot stat fixture {}: {error}", path.display()));
        assert!(
            !metadata.file_type().is_symlink(),
            "released fixture contains symlink {}",
            path.display()
        );
        if metadata.is_dir() {
            collect_released_realm_fixture_files(corpus, &path, files);
        } else {
            assert!(
                metadata.is_file(),
                "released fixture contains a non-regular entry {}",
                path.display()
            );
            let relative = path
                .strip_prefix(corpus)
                .expect("fixture traversal stayed under corpus")
                .to_string_lossy()
                .replace('\\', "/");
            if relative != "fixture-manifest.json" {
                assert!(files.insert(relative), "duplicate fixture path");
            }
        }
    }
}

#[cfg(feature = "sqlite-store")]
fn released_realm_fixture_ledger(database: &std::path::Path) -> Vec<(String, i64)> {
    use rusqlite::OpenFlags;

    let connection = rusqlite::Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap_or_else(|error| {
        panic!(
            "cannot open pre-upgrade fixture database {} read-only: {error}",
            database.display()
        )
    });
    let mut statement = connection
        .prepare("SELECT domain, version FROM meerkat_schema ORDER BY domain")
        .unwrap_or_else(|error| {
            panic!(
                "fixture database {} has no readable meerkat_schema ledger: {error}",
                database.display()
            )
        });
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query pre-upgrade schema ledger")
        .collect::<Result<Vec<_>, _>>()
        .expect("decode pre-upgrade schema ledger");
    rows
}

#[cfg(feature = "sqlite-store")]
#[derive(Debug)]
struct ReleasedRealmFixtureRawSession {
    session_id: String,
    strand: String,
    message_count: usize,
    head_revision: String,
    rewrite_count: usize,
    messages: Vec<(i64, Vec<u8>)>,
}

#[cfg(feature = "sqlite-store")]
fn released_realm_fixture_sessions(
    database: &std::path::Path,
) -> Vec<ReleasedRealmFixtureRawSession> {
    use rusqlite::OpenFlags;

    let connection = rusqlite::Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let mut head_statement = connection
        .prepare(
            "SELECT session_id, strand, message_count, head_revision, rewrite_count \
             FROM session_heads ORDER BY session_id",
        )
        .unwrap();
    let heads = head_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    heads
        .into_iter()
        .map(
            |(session_id, strand, message_count, head_revision, rewrite_count)| {
                let mut message_statement = connection
                    .prepare(
                        "SELECT seq, message_json FROM session_strand_messages \
                         WHERE session_id = ?1 AND strand = ?2 \
                           AND typeof(message_json) = 'blob' ORDER BY seq",
                    )
                    .unwrap();
                let messages = message_statement
                    .query_map((&session_id, &strand), |row| Ok((row.get(0)?, row.get(1)?)))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                ReleasedRealmFixtureRawSession {
                    session_id,
                    strand,
                    message_count: usize::try_from(message_count)
                        .expect("released message_count must be non-negative"),
                    head_revision,
                    rewrite_count: usize::try_from(rewrite_count)
                        .expect("released rewrite_count must be non-negative"),
                    messages,
                }
            },
        )
        .collect()
}

#[cfg(feature = "sqlite-store")]
fn released_realm_fixture_runtime_snapshots(database: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    use rusqlite::OpenFlags;

    let connection = rusqlite::Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let columns = connection
        .prepare("PRAGMA table_info(runtime_session_snapshots)")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        columns,
        vec![
            (0, "runtime_id".to_string(), "TEXT".to_string(), 0, None, 1),
            (
                1,
                "session_snapshot".to_string(),
                "BLOB".to_string(),
                1,
                None,
                0,
            ),
        ],
        "runtime_session_snapshots is not the exact released 0.8.10 schema"
    );
    let authority_table: i64 = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master \
             WHERE type = 'table' AND name = 'runtime_session_authority')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        authority_table, 0,
        "pre-upgrade fixture already contains 0.8.11 runtime authority"
    );
    let mut statement = connection
        .prepare(
            "SELECT runtime_id, session_snapshot FROM runtime_session_snapshots \
             WHERE typeof(session_snapshot) = 'blob' ORDER BY runtime_id",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    rows
}

#[cfg(feature = "sqlite-store")]
fn released_realm_fixture_consumed_inputs(
    database: &std::path::Path,
) -> Vec<(String, String, Vec<u8>)> {
    use rusqlite::OpenFlags;

    let connection = rusqlite::Connection::open_with_flags(
        database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let mut statement = connection
        .prepare(
            "SELECT runtime_id, input_id, state_json FROM runtime_input_states \
             WHERE typeof(state_json) = 'blob' ORDER BY runtime_id, input_id",
        )
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[cfg(feature = "sqlite-store")]
fn released_realm_fixture_bytes_sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut encoded, "{byte:02x}").unwrap();
    }
    encoded
}

#[cfg(feature = "sqlite-store")]
fn released_realm_fixture_post_ledger_version(database: &std::path::Path, domain: &str) -> i64 {
    let connection = rusqlite::Connection::open(database).unwrap();
    let version: i64 = connection
        .query_row(
            "SELECT version FROM meerkat_schema WHERE domain = ?1",
            [domain],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| {
            panic!(
                "post-upgrade database {} has no {domain} ledger: {error}",
                database.display()
            )
        });
    version
}

/// Release-boundary evidence, not a current-source fixture generator.
///
/// The committed corpus is a clean synthetic realm written by the published
/// `rkat` 0.8.10 binary. This test binds its producer and every input byte
/// before opening a temporary copy with the current session/runtime stores,
/// then proves the session head, message sequence, runtime snapshot, and
/// consumed-input facts through public reads. A missing corpus is an explicit
/// release blocker; this never falls back to constructing old bytes with
/// current code.
#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn released_0_8_10_rkat_realm_upgrades_without_losing_durable_state() {
    use meerkat_core::{IncrementalSessionStore as _, SessionStore as _};
    use meerkat_runtime::store::RuntimeStore as _;
    use std::collections::{BTreeMap, BTreeSet};

    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/v0_8_10_released_realm/corpus");
    let manifest_path = corpus.join("fixture-manifest.json");
    let manifest_bytes = std::fs::read(&manifest_path).unwrap_or_else(|error| {
        panic!(
            "RELEASE BLOCKER: released 0.8.10 realm fixture is absent or unreadable at {}: {error}; \
             use tests/fixtures/v0_8_10_released_realm/import_released_fixture.py with the \
             published rkat 0.8.10 artifact, never current-source generated bytes",
            manifest_path.display()
        )
    });
    let manifest: ReleasedRealmFixtureManifest = serde_json::from_slice(&manifest_bytes)
        .unwrap_or_else(|error| panic!("released fixture manifest is invalid: {error}"));

    assert_eq!(manifest.schema_version, 1);
    assert_eq!(
        manifest.fixture_id,
        "meerkat-0.8.10-rkat-released-synthetic"
    );
    assert_eq!(
        manifest.data_classification, "synthetic_non_production",
        "production-redacted data is forbidden in the release fixture"
    );
    assert_eq!(manifest.producer.artifact_origin, "published_release");
    assert_eq!(manifest.producer.product, "rkat");
    assert_eq!(manifest.producer.meerkat_version, "0.8.10");
    assert_eq!(
        manifest.producer.binary_version_output, "rkat 0.8.10",
        "fixture was not written by the published Meerkat 0.8.10 CLI"
    );
    assert_eq!(manifest.producer.binary_name, "rkat");
    assert_eq!(
        manifest.producer.binary_sha256,
        "7a60f631c78cf6abc5abb523b503b86e752abeb13ae05d100f85164679435815",
        "fixture must remain bound to the reviewed published macOS arm64 binary"
    );
    for (label, digest) in [
        ("producer binary", &manifest.producer.binary_sha256),
        ("capture receipt", &manifest.producer.capture_receipt_sha256),
    ] {
        assert_eq!(digest.len(), 64, "{label} SHA-256 has the wrong length");
        assert!(
            digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
            "{label} SHA-256 is not lowercase hexadecimal"
        );
    }
    let receipt_relative = released_realm_fixture_relative(&manifest.producer.capture_receipt_path);
    assert_eq!(
        receipt_relative,
        std::path::Path::new("provenance/capture-receipt.json")
    );
    let receipt_path = corpus.join(&receipt_relative);
    assert_eq!(
        released_realm_fixture_sha256(&receipt_path),
        manifest.producer.capture_receipt_sha256,
        "committed capture receipt bytes differ from their provenance digest"
    );
    let receipt_value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).expect("read capture receipt"))
            .expect("capture receipt must be JSON");
    let receipt = receipt_value
        .as_object()
        .expect("capture receipt must be an object");
    assert_eq!(
        receipt.get("data_classification").and_then(|v| v.as_str()),
        Some("synthetic_non_production")
    );
    assert_eq!(
        receipt.get("producer").and_then(|v| v.as_str()),
        Some("meerkat-release")
    );
    assert_eq!(
        receipt.get("meerkat_version").and_then(|v| v.as_str()),
        Some("0.8.10")
    );
    assert_eq!(
        receipt
            .get("writer_binary_version_output")
            .and_then(|value| value.as_str()),
        Some(manifest.producer.binary_version_output.as_str())
    );
    assert_eq!(
        receipt
            .get("writer_binary_sha256")
            .and_then(|value| value.as_str()),
        Some(manifest.producer.binary_sha256.as_str())
    );
    assert!(
        !receipt
            .get("writer_binary_path")
            .and_then(|value| value.as_str())
            .expect("capture receipt must bind the immutable writer path")
            .is_empty()
    );
    assert_eq!(
        manifest.producer.source_release, "https://github.com/lukacf/meerkat/releases/tag/v0.8.10",
        "fixture source is not the public Meerkat 0.8.10 release"
    );
    assert_eq!(
        receipt
            .get("source_release")
            .and_then(|value| value.as_str()),
        Some(manifest.producer.source_release.as_str())
    );
    assert_eq!(
        receipt
            .get("current_source_build")
            .and_then(|v| v.as_bool()),
        Some(false),
        "current-source generated fixtures are forbidden"
    );
    assert_eq!(
        receipt.get("sanitization_method").and_then(|v| v.as_str()),
        Some("synthetic_inputs_before_capture"),
        "production redaction is not released-writer evidence"
    );
    assert_eq!(
        receipt.get("clean_shutdown").and_then(|v| v.as_bool()),
        Some(true),
        "the upgrade corpus must be a clean-shutdown image"
    );
    assert_eq!(
        receipt.get("release_asset").and_then(|v| v.as_str()),
        Some("rkat-0.8.10-aarch64-apple-darwin.tar.gz")
    );
    assert_eq!(
        receipt.get("release_asset_sha256").and_then(|v| v.as_str()),
        Some("97501fa6bc078b344315e91981240f4b66a8d2f64c26f4575b04c74df73b5db7")
    );

    let realm_root_relative = released_realm_fixture_relative(&manifest.realm.root);
    let realm_manifest_relative = released_realm_fixture_relative(&manifest.realm.manifest);
    let sqlite_relative = released_realm_fixture_relative(&manifest.realm.sqlite_database);
    assert_eq!(realm_root_relative, std::path::Path::new("realm"));
    assert_eq!(
        realm_manifest_relative,
        realm_root_relative.join("realm_manifest.json")
    );
    assert_eq!(
        sqlite_relative,
        realm_root_relative.join("sessions.sqlite3"),
        "released rkat SQLite profile must remain co-tenant"
    );

    let expected_files = manifest
        .files
        .iter()
        .map(|file| {
            let relative = released_realm_fixture_relative(&file.path);
            assert!(
                relative.starts_with(&realm_root_relative) || relative == receipt_relative,
                "fixture file is outside the declared realm/provenance roots: {}",
                file.path
            );
            assert_eq!(file.sha256.len(), 64, "invalid file SHA-256");
            let path = corpus.join(&relative);
            let metadata = std::fs::symlink_metadata(&path).unwrap_or_else(|error| {
                panic!("fixture file {} is missing: {error}", path.display())
            });
            assert!(metadata.is_file() && !metadata.file_type().is_symlink());
            assert_eq!(metadata.len(), file.bytes, "fixture byte count changed");
            assert_eq!(
                released_realm_fixture_sha256(&path),
                file.sha256,
                "fixture file digest changed: {}",
                file.path
            );
            file.path.clone()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        expected_files.len(),
        manifest.files.len(),
        "fixture manifest contains duplicate file paths"
    );
    let mut actual_files = BTreeSet::new();
    collect_released_realm_fixture_files(&corpus, &corpus, &mut actual_files);
    assert_eq!(
        actual_files, expected_files,
        "fixture corpus contains unbound or missing files"
    );

    let persisted_manifest: meerkat_store::RealmManifest =
        serde_json::from_slice(&std::fs::read(corpus.join(&realm_manifest_relative)).unwrap())
            .expect("released realm manifest must decode under 0.8.11");
    assert_eq!(
        persisted_manifest.realm.to_string(),
        manifest.realm.realm_id
    );
    assert_eq!(
        persisted_manifest.backend,
        meerkat_store::RealmBackend::Sqlite
    );

    let mut expected_ledgers = Vec::new();
    for row in &manifest.realm.pre_upgrade_ledgers {
        assert!(row.version > 0);
        expected_ledgers.push((row.domain.clone(), row.version));
    }
    assert!(
        manifest
            .realm
            .pre_upgrade_ledgers
            .iter()
            .any(|row| row.domain == "session-store" && row.version == 2)
    );
    assert!(
        manifest
            .realm
            .pre_upgrade_ledgers
            .iter()
            .any(|row| row.domain == "runtime-store" && row.version == 1)
    );
    assert!(
        manifest
            .realm
            .pre_upgrade_ledgers
            .iter()
            .any(|row| row.domain == "schedule-store" && row.version == 1)
    );
    expected_ledgers.sort();
    assert_eq!(
        released_realm_fixture_ledger(&corpus.join(&sqlite_relative)),
        expected_ledgers,
        "pre-upgrade ledger differs from released fixture manifest"
    );

    let raw_sessions = released_realm_fixture_sessions(&corpus.join(&sqlite_relative));
    assert_eq!(
        raw_sessions.len(),
        manifest.expected.sessions.len(),
        "session expectations must bind the exact released head set"
    );
    let expected_sessions = manifest
        .expected
        .sessions
        .iter()
        .map(|session| (session.session_id.as_str(), session))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        expected_sessions.len(),
        manifest.expected.sessions.len(),
        "duplicate session expectation"
    );
    for raw in &raw_sessions {
        let expected = expected_sessions
            .get(raw.session_id.as_str())
            .expect("released session head identity is undeclared");
        assert_eq!(raw.strand, expected.strand);
        assert_eq!(raw.message_count, expected.message_count);
        assert_eq!(raw.head_revision, expected.head_revision);
        assert_eq!(raw.rewrite_count, expected.rewrite_count);
        assert_eq!(
            raw.messages.len(),
            expected.messages.len(),
            "message expectations do not bind the exact released sequence"
        );
        for ((sequence, bytes), expected_message) in raw.messages.iter().zip(&expected.messages) {
            assert_eq!(*sequence, expected_message.sequence);
            assert_eq!(bytes.len() as u64, expected_message.bytes);
            assert_eq!(
                released_realm_fixture_bytes_sha256(bytes),
                expected_message.sha256
            );
        }
    }

    let raw_runtime_snapshots =
        released_realm_fixture_runtime_snapshots(&corpus.join(&sqlite_relative));
    assert_eq!(
        raw_runtime_snapshots.len(),
        manifest.expected.runtime_snapshots.len(),
        "runtime snapshot expectations must bind the exact released row set"
    );
    let expected_runtime_snapshots = manifest
        .expected
        .runtime_snapshots
        .iter()
        .map(|snapshot| (snapshot.runtime_id.as_str(), snapshot))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        expected_runtime_snapshots.len(),
        manifest.expected.runtime_snapshots.len(),
        "duplicate runtime snapshot expectation"
    );
    let expected_session_ids = manifest
        .expected
        .sessions
        .iter()
        .map(|session| session.session_id.as_str())
        .collect::<BTreeSet<_>>();
    assert!(
        manifest
            .expected
            .runtime_snapshots
            .iter()
            .all(|snapshot| expected_session_ids.contains(snapshot.session_id.as_str())),
        "runtime snapshot expectations name a session outside the exact head set"
    );
    for (runtime_id, bytes) in &raw_runtime_snapshots {
        let expected = expected_runtime_snapshots
            .get(runtime_id.as_str())
            .expect("released runtime snapshot identity is undeclared");
        assert_eq!(bytes.len() as u64, expected.bytes);
        assert_eq!(released_realm_fixture_bytes_sha256(bytes), expected.sha256);
        let snapshot: serde_json::Value =
            serde_json::from_slice(bytes).expect("released runtime snapshot must be session JSON");
        assert_eq!(
            snapshot.get("version").and_then(|value| value.as_u64()),
            Some(2)
        );
        assert_eq!(
            snapshot.get("id").and_then(|value| value.as_str()),
            Some(expected.session_id.as_str())
        );
    }

    let raw_consumed_inputs =
        released_realm_fixture_consumed_inputs(&corpus.join(&sqlite_relative));
    assert_eq!(
        raw_consumed_inputs.len(),
        manifest.expected.consumed_inputs.len(),
        "consumed-input expectations must bind the exact released row set"
    );
    let expected_consumed_inputs = manifest
        .expected
        .consumed_inputs
        .iter()
        .map(|input| ((input.runtime_id.as_str(), input.input_id.as_str()), input))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        expected_consumed_inputs.len(),
        manifest.expected.consumed_inputs.len(),
        "duplicate consumed-input expectation"
    );
    for (runtime_id, input_id, bytes) in &raw_consumed_inputs {
        let expected = expected_consumed_inputs
            .get(&(runtime_id.as_str(), input_id.as_str()))
            .expect("released input identity is undeclared");
        assert_eq!(bytes.len() as u64, expected.bytes);
        assert_eq!(released_realm_fixture_bytes_sha256(bytes), expected.sha256);
        let state: serde_json::Value =
            serde_json::from_slice(bytes).expect("released input state must be JSON");
        assert_eq!(
            state.get("current_state").and_then(|value| value.as_str()),
            Some("consumed")
        );
        assert_eq!(
            state.get("last_run_id").and_then(|value| value.as_str()),
            Some(expected.last_run_id.as_str())
        );
        assert_eq!(
            state
                .get("last_boundary_sequence")
                .and_then(|value| value.as_u64()),
            Some(expected.last_boundary_sequence)
        );
        assert_eq!(
            state
                .pointer("/terminal_outcome/outcome_type")
                .and_then(|value| value.as_str()),
            Some("consumed")
        );
        assert!(
            !state
                .get("persisted_input")
                .expect("released consumed input must carry its source payload")
                .is_null(),
            "raw corpus verification binds the unchanged 0.8.10 bytes before activation"
        );
    }

    let temporary = TempDir::new().unwrap();
    for file in &manifest.files {
        let relative = released_realm_fixture_relative(&file.path);
        let target = temporary.path().join(&relative);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(corpus.join(relative), target).unwrap();
    }
    let sqlite_database = temporary.path().join(&sqlite_relative);

    let first_session_store = SqliteSessionStore::open(&sqlite_database)
        .expect("0.8.11 must open and migrate the released 0.8.10 session database");
    let first_runtime_store = SqliteRuntimeStore::new_head_canonical(&sqlite_database)
        .expect("0.8.11 must open and migrate the released 0.8.10 runtime database");
    let mut post_upgrade_ledgers = BTreeMap::new();
    for row in manifest
        .realm
        .pre_upgrade_ledgers
        .iter()
        .filter(|row| matches!(row.domain.as_str(), "session-store" | "runtime-store"))
    {
        let version = released_realm_fixture_post_ledger_version(&sqlite_database, &row.domain);
        assert!(
            version >= row.version,
            "post-upgrade {} ledger regressed from released v{} to v{version}",
            row.domain,
            row.version
        );
        assert!(
            post_upgrade_ledgers
                .insert(row.domain.clone(), version)
                .is_none(),
            "duplicate fixture ledger identity"
        );
    }
    drop(first_session_store);
    drop(first_runtime_store);

    let session_store = SqliteSessionStore::open(&sqlite_database)
        .expect("the migrated session database must reopen idempotently");
    let runtime_store = SqliteRuntimeStore::new_head_canonical(&sqlite_database)
        .expect("the migrated runtime database must reopen idempotently");
    for (domain, expected_version) in &post_upgrade_ledgers {
        assert_eq!(
            released_realm_fixture_post_ledger_version(&sqlite_database, domain),
            *expected_version,
            "reopening the upgraded store reran or changed the {domain} migration ledger"
        );
    }
    for expected in &manifest.expected.runtime_snapshots {
        let runtime_id = LogicalRuntimeId::new(expected.runtime_id.clone());
        let authority = runtime_store
            .load_session_boundary_authority(&runtime_id)
            .await
            .unwrap_or_else(|error| panic!("post-upgrade authority load failed: {error}"))
            .expect("0.8.11 did not activate current authority for released runtime snapshot");
        assert_eq!(authority.session_id().to_string(), expected.session_id);
    }

    assert!(!manifest.expected.sessions.is_empty());
    for expected in &manifest.expected.sessions {
        let session_id = SessionId::parse(&expected.session_id).expect("fixture session UUID");
        let session = session_store
            .load(&session_id)
            .await
            .unwrap_or_else(|error| panic!("post-upgrade session load failed: {error}"))
            .expect("fixture session disappeared during upgrade");
        assert_eq!(session.messages().len(), expected.message_count);
        assert_eq!(
            session.transcript_content_digest().unwrap(),
            expected.head_revision
        );
        let commits = session_store
            .load_rewrite_commits(&session_id)
            .await
            .unwrap_or_else(|error| panic!("post-upgrade rewrite load failed: {error}"));
        assert_eq!(commits.len(), expected.rewrite_count);
    }

    assert!(!manifest.expected.consumed_inputs.is_empty());
    for expected in &manifest.expected.consumed_inputs {
        let runtime_id = LogicalRuntimeId::new(expected.runtime_id.clone());
        let input_id =
            InputId::from_uuid(Uuid::parse_str(&expected.input_id).expect("fixture input UUID"));
        let stored = runtime_store
            .load_input_state(&runtime_id, &input_id)
            .await
            .unwrap_or_else(|error| panic!("post-upgrade input load failed: {error}"))
            .expect("consumed input disappeared during upgrade");
        assert_eq!(stored.seed.phase, InputLifecycleState::Consumed);
        assert_eq!(
            stored.seed.last_run_id.as_ref().map(ToString::to_string),
            Some(expected.last_run_id.clone())
        );
        assert_eq!(
            stored.seed.last_boundary_sequence,
            Some(expected.last_boundary_sequence)
        );
        assert_eq!(
            stored.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        );
        assert!(
            stored.state.persisted_input.is_none(),
            "exact 0.8.10 activation must retire closed terminal redelivery bytes"
        );
        assert!(
            runtime_store
                .load_session_boundary_authority(&runtime_id)
                .await
                .unwrap()
                .is_some(),
            "HeadCanonical runtime authority disappeared during upgrade"
        );
    }
}
