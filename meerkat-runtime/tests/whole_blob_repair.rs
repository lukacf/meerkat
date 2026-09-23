//! WholeBlob audited-endpoint recovery: a committed document the current
//! decoder refuses is diagnosed and re-anchored on its live rows without
//! losing a message, through the ordinary store compare-and-swap.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use meerkat_core::Session;
use meerkat_core::lifecycle::RunId;
use meerkat_core::types::{Message, UserMessage};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::store::memory::InMemoryRuntimeStore;
use meerkat_runtime::store::whole_blob_repair::{
    WholeBlobRepairAction, repair_whole_blob_audited_endpoint,
};
use meerkat_runtime::store::{
    PreparedWholeBlobProvisionalTail, RuntimeStore, WholeBlobStoreAuthority,
};
use meerkat_runtime::store::{RuntimeStoreError, SerializedSessionSnapshot, SqliteRuntimeStore};

fn compacted_session() -> Session {
    let mut session = Session::new();
    session.append_system_message("system prompt");
    for turn in 0..5 {
        session.push(Message::User(UserMessage::text(format!("question {turn}"))));
    }
    let live = session.messages().to_vec();
    let mut replacement = vec![live[0].clone()];
    replacement.push(Message::User(UserMessage::compaction_summary(
        "[Compaction summary] earlier questions",
    )));
    replacement.extend(live[4..].iter().cloned());
    session
        .stage_validated_compaction_for_test(replacement, 12)
        .expect("compaction commits");
    session.push(Message::User(UserMessage::text("after compaction")));
    session
}

/// Mutate one row inside the audited endpoint without any audit, then encode
/// with plain serde so the writer-side guard is bypassed, exactly as a buggy
/// writer would have done.
fn wedged_document(session: &mut Session) -> (Vec<u8>, Vec<Message>) {
    let mut tampered = session.messages().to_vec();
    tampered[1] = Message::User(UserMessage::text("row inside the endpoint, rewritten"));
    session.replace_messages_unaudited_for_test(tampered.clone());
    let bytes = serde_json::to_vec(session).expect("plain serde encodes");
    (bytes, tampered)
}

#[tokio::test]
async fn wedged_wholeblob_is_diagnosed_and_reanchored_without_losing_rows() {
    let store = InMemoryRuntimeStore::new();
    let mut session = compacted_session();
    let session_id = session.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let (bytes, live_rows) = wedged_document(&mut session);
    let injected: WholeBlobStoreAuthority = store
        .inject_committed_whole_blob_bytes_for_test(&runtime_id, &session_id, bytes)
        .await
        .expect("inject wedged body");

    // The ordinary read refuses exactly like production did.
    let refused = store
        .session_authority_ops()
        .load_committed_whole_blob_snapshot(&runtime_id)
        .await
        .expect_err("wedged body must be refused");
    assert!(
        refused
            .to_string()
            .contains("live transcript does not preserve the graph-proved audited endpoint"),
        "unexpected refusal: {refused}"
    );

    // Diagnose only.
    let report = repair_whole_blob_audited_endpoint(&store, &runtime_id, false, false)
        .await
        .expect("diagnose");
    assert_eq!(report.action, WholeBlobRepairAction::WouldReanchor);
    assert_eq!(report.base_store_revision, injected.store_revision());
    assert_eq!(report.live_row_count, live_rows.len());
    let divergence = report.divergence.expect("divergence is reported");
    assert_eq!(divergence.first_divergent_row, Some(1));
    assert_eq!(
        report.dropped_intents.len(),
        1,
        "the pending compaction intent is named"
    );
    assert_eq!(report.graph_edges_dropped, 1);
    assert!(
        store
            .session_authority_ops()
            .load_committed_whole_blob_snapshot(&runtime_id)
            .await
            .is_err(),
        "diagnose must not write"
    );

    // Apply.
    let report = repair_whole_blob_audited_endpoint(&store, &runtime_id, true, false)
        .await
        .expect("apply");
    assert_eq!(report.action, WholeBlobRepairAction::Reanchored);
    let committed_revision = report.committed_store_revision.expect("new revision");
    assert!(committed_revision > injected.store_revision());
    let recovered = store
        .session_authority_ops()
        .load_committed_whole_blob_snapshot(&runtime_id)
        .await
        .expect("recovered body decodes")
        .expect("recovered body exists");
    assert_eq!(
        recovered.session().messages(),
        live_rows.as_slice(),
        "no row lost or changed"
    );
    assert_eq!(recovered.authority().store_revision(), committed_revision);
    assert_eq!(
        recovered
            .session()
            .compaction_projection_intents()
            .expect("intents readable")
            .len(),
        0
    );

    // Idempotent: a decodable body reports nothing to do.
    let again = repair_whole_blob_audited_endpoint(&store, &runtime_id, true, false)
        .await
        .expect("second run");
    assert_eq!(again.action, WholeBlobRepairAction::NoRepairNeeded);
}

#[tokio::test]
async fn writer_side_guard_refuses_a_divergent_provisional_candidate() {
    let store = InMemoryRuntimeStore::new();
    let mut session = compacted_session();
    let session_id = session.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let coherent = serde_json::to_vec(&session).expect("coherent body encodes");
    let base: WholeBlobStoreAuthority = store
        .inject_committed_whole_blob_bytes_for_test(&runtime_id, &session_id, coherent)
        .await
        .expect("base authority");
    // Coherent candidates are accepted.
    PreparedWholeBlobProvisionalTail::prepare_from_session(base.clone(), RunId::new(), 1, &session)
        .expect("coherent candidate is prepared");
    let (_bytes, _rows) = wedged_document(&mut session);
    let refused =
        PreparedWholeBlobProvisionalTail::prepare_from_session(base, RunId::new(), 2, &session)
            .expect_err("divergent candidate is refused before any store write");
    assert!(
        refused
            .to_string()
            .contains("refusing to persist a WholeBlob document"),
        "unexpected error: {refused}"
    );
}

/// Item 1: a live transcript SHORTER than its audited endpoint is the one
/// shape where re-anchoring drops audited content. Apply refuses it unless
/// the operator explicitly accepts, and the acceptance is echoed with both
/// row counts.
#[tokio::test]
async fn shorter_than_endpoint_requires_explicit_acceptance() {
    let store = InMemoryRuntimeStore::new();
    let mut session = compacted_session();
    let session_id = session.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let mut shorter = session.messages().to_vec();
    shorter.truncate(2);
    session.replace_messages_unaudited_for_test(shorter.clone());
    let bytes = serde_json::to_vec(&session).expect("plain serde encodes");
    store
        .inject_committed_whole_blob_bytes_for_test(&runtime_id, &session_id, bytes)
        .await
        .expect("inject");

    let diagnosed = repair_whole_blob_audited_endpoint(&store, &runtime_id, false, false)
        .await
        .expect("diagnose");
    let divergence = diagnosed.divergence.expect("divergence");
    assert_eq!(
        divergence.kind,
        meerkat_core::AuditedEndpointDivergenceKind::LiveShorterThanEndpoint
    );
    assert_eq!(diagnosed.action, WholeBlobRepairAction::WouldReanchor);

    let refused = repair_whole_blob_audited_endpoint(&store, &runtime_id, true, false)
        .await
        .expect("apply without acceptance");
    match &refused.action {
        WholeBlobRepairAction::Refused(reason) => {
            assert!(
                reason.contains("shorter than the audited endpoint"),
                "{reason}"
            );
            assert!(
                reason.contains(&format!("({} rows)", shorter.len())),
                "{reason}"
            );
        }
        other => panic!("expected refusal, got {other:?}"),
    }
    assert!(
        store
            .session_authority_ops()
            .load_committed_whole_blob_snapshot(&runtime_id)
            .await
            .is_err(),
        "refused apply must not write"
    );

    let accepted = repair_whole_blob_audited_endpoint(&store, &runtime_id, true, true)
        .await
        .expect("apply with acceptance");
    assert_eq!(accepted.action, WholeBlobRepairAction::Reanchored);
    let record = accepted.accepted_shorter.expect("audit record");
    assert_eq!(record.live_row_count, shorter.len());
    assert_eq!(record.endpoint_row_count, divergence.endpoint_row_count);
    let recovered = store
        .session_authority_ops()
        .load_committed_whole_blob_snapshot(&runtime_id)
        .await
        .expect("decodes")
        .expect("exists");
    assert_eq!(recovered.session().messages(), shorter.as_slice());
}

/// Item 3: the committed-body read surfaces the typed store error, not a
/// generic read failure, so hosts can route to the repair.
#[tokio::test]
async fn refused_committed_body_is_the_typed_audited_endpoint_error() {
    let store = InMemoryRuntimeStore::new();
    let mut session = compacted_session();
    let session_id = session.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let (bytes, _rows) = wedged_document(&mut session);
    store
        .inject_committed_whole_blob_bytes_for_test(&runtime_id, &session_id, bytes)
        .await
        .expect("inject");
    let error = store
        .session_authority_ops()
        .load_committed_whole_blob_snapshot(&runtime_id)
        .await
        .expect_err("refused");
    assert!(
        matches!(error, RuntimeStoreError::AuditedEndpointDivergence { .. }),
        "expected the typed divergence, got {error:?}"
    );
}

/// Item 5, end to end on the sqlite store: a wedged body with a PENDING
/// compaction projection outbox row (the HomeCore shape), refused on load;
/// repaired; then the runtime's finalization sequence (load, clear the
/// intent from the compatibility checkpoint, commit, mark finalized) runs
/// with no second refusal and leaves the outbox empty.
#[tokio::test]
async fn sqlite_wedged_body_with_pending_outbox_repairs_and_finalizes_cleanly() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let store = SqliteRuntimeStore::new_whole_blob(tempdir.path().join("runtime.sqlite3"))
        .expect("whole-blob sqlite store");
    let mut session = compacted_session();
    let session_id = session.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let pending = session
        .compaction_projection_intents()
        .expect("intents readable");
    assert_eq!(pending.len(), 1, "the compaction left one pending intent");
    let (bytes, live_rows) = wedged_document(&mut session);
    let injected = store
        .inject_wedged_whole_blob_for_test(&runtime_id, &session_id, bytes, pending.clone())
        .await
        .expect("inject wedged body + pending outbox row");

    let refused = store
        .load_session_snapshot(&runtime_id)
        .await
        .expect_err("wedged body refused on load");
    assert!(
        matches!(refused, RuntimeStoreError::AuditedEndpointDivergence { .. }),
        "{refused:?}"
    );
    assert_eq!(
        store
            .load_pending_compaction_projections(&runtime_id)
            .await
            .expect("pending rows")
            .len(),
        1
    );

    let report = repair_whole_blob_audited_endpoint(&store, &runtime_id, true, false)
        .await
        .expect("repair");
    assert_eq!(report.action, WholeBlobRepairAction::Reanchored);
    assert_eq!(report.dropped_intents.len(), 1);
    assert!(report.committed_store_revision.expect("revision") > injected.store_revision());

    // Next load: the runtime's compaction reconciliation sequence for the
    // WholeBlob profile (runtime_loop::reconcile_loaded_compaction_projection_outbox).
    let intents = store
        .load_pending_compaction_projections(&runtime_id)
        .await
        .expect("pending rows still present for the runtime to finalize");
    assert_eq!(
        intents.len(),
        1,
        "the outbox row is left for normal finalization"
    );
    let snapshot = store
        .load_session_snapshot(&runtime_id)
        .await
        .expect("repaired body loads")
        .expect("exists");
    let mut cleaned: Session = serde_json::from_slice(snapshot.as_ref()).expect("decodes");
    for intent in &intents {
        cleaned
            .complete_compaction_projection_intent(&intent.projection)
            .expect("clearing an already-absent intent is a no-op");
    }
    let cleaned_bytes = serde_json::to_vec(&cleaned).expect("encode");
    store
        .commit_session_snapshot(
            &runtime_id,
            SerializedSessionSnapshot {
                session_snapshot: std::sync::Arc::new(cleaned_bytes),
            },
        )
        .await
        .expect("compatibility checkpoint commits");
    for intent in &intents {
        store
            .mark_compaction_projection_finalized(&runtime_id, &intent.projection)
            .await
            .expect("finalize the outbox row");
    }
    assert!(
        store
            .load_pending_compaction_projections(&runtime_id)
            .await
            .expect("pending rows")
            .is_empty(),
        "outbox is empty after finalization"
    );
    let final_snapshot = store
        .session_authority_ops()
        .load_committed_whole_blob_snapshot(&runtime_id)
        .await
        .expect("final body decodes")
        .expect("exists");
    assert_eq!(
        final_snapshot.session().messages(),
        live_rows.as_slice(),
        "no row lost"
    );
    assert!(
        final_snapshot
            .session()
            .compaction_projection_intents()
            .expect("intents")
            .is_empty()
    );
}

/// A document that decodes reports its real row count, so an operator can
/// tell "nothing to repair, N rows" from an empty store.
#[tokio::test]
async fn healthy_document_reports_its_decoded_row_count() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let store = SqliteRuntimeStore::new_whole_blob(tempdir.path().join("runtime.sqlite3"))
        .expect("whole-blob sqlite store");
    let mut session = Session::new();
    session.append_system_message("system prompt");
    for turn in 0..3 {
        session.push(Message::User(UserMessage::text(format!("question {turn}"))));
    }
    let session_id = session.id().clone();
    let runtime_id = LogicalRuntimeId::for_session(&session_id);
    let bytes = serde_json::to_vec(&session).expect("plain serde encodes");
    store
        .inject_wedged_whole_blob_for_test(&runtime_id, &session_id, bytes, Vec::new())
        .await
        .expect("commit a healthy body");
    let report = repair_whole_blob_audited_endpoint(&store, &runtime_id, false, false)
        .await
        .expect("diagnose");
    assert!(matches!(
        report.action,
        WholeBlobRepairAction::NoRepairNeeded
    ));
    assert_eq!(report.decode_error, None);
    assert_eq!(report.live_row_count, 4);
}
