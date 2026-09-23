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
    let report = repair_whole_blob_audited_endpoint(&store, &runtime_id, false)
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
    let report = repair_whole_blob_audited_endpoint(&store, &runtime_id, true)
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
    let again = repair_whole_blob_audited_endpoint(&store, &runtime_id, true)
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
