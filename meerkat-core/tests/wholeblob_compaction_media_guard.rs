//! Reproduction probe for the HomeCore WholeBlob wedge: a compaction that
//! rebuilt its retained rows from the model-hydrated (inline image) copies,
//! followed by the checkpoint's media externalization, must still decode
//! through the current-envelope ingress guard.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(feature = "test-support")]

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use meerkat_core::Session;
use meerkat_core::blob::{BlobId, BlobPayload, BlobRef, BlobStore, BlobStoreError};
use meerkat_core::image_content::{MissingBlobBehavior, hydrate_messages_for_execution};
use meerkat_core::types::{ContentBlock, ImageData, Message, UserMessage};

#[derive(Default)]
struct ContentAddressedBlobStore {
    blobs: Mutex<HashMap<BlobId, BlobPayload>>,
}

#[async_trait]
impl BlobStore for ContentAddressedBlobStore {
    async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
        let media_type = meerkat_core::image_generation::MediaType::canonical_str(media_type);
        let blob_id = meerkat_core::blob::content_blob_id(&media_type, data);
        self.blobs.lock().expect("blob store mutex").insert(
            blob_id.clone(),
            BlobPayload {
                blob_id: blob_id.clone(),
                media_type: media_type.clone(),
                data: data.to_string(),
            },
        );
        Ok(BlobRef {
            blob_id,
            media_type,
        })
    }

    async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
        self.blobs
            .lock()
            .expect("blob store mutex")
            .get(blob_id)
            .cloned()
            .ok_or_else(|| BlobStoreError::NotFound(blob_id.clone()))
    }

    async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
        self.blobs.lock().expect("blob store mutex").remove(blob_id);
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        true
    }
}

fn image_user_message(text: &str) -> Message {
    let mut user = UserMessage::text(text);
    user.content.push(ContentBlock::Image {
        media_type: "image/png".to_string(),
        data: ImageData::Inline {
            data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB".to_string(),
        },
    });
    Message::User(user)
}

fn round_trip(session: &Session) -> Result<Session, String> {
    let bytes = serde_json::to_vec(session).map_err(|error| error.to_string())?;
    serde_json::from_slice::<Session>(&bytes).map_err(|error| error.to_string())
}

/// Shape under test: retained rows were externalized by an earlier
/// checkpoint, the compaction rebuilt them from the hydrated model copies,
/// and the next checkpoint externalized the whole document again.
#[tokio::test]
async fn compaction_over_hydrated_rows_survives_checkpoint_externalization() {
    let store = ContentAddressedBlobStore::default();
    let mut session = Session::new();
    session.append_system_message("system prompt");
    for turn in 0..6 {
        session.push(Message::User(UserMessage::text(format!("question {turn}"))));
    }
    session.push(image_user_message("look at this"));
    session.push(Message::User(UserMessage::text("and then")));

    // Earlier checkpoint: the image row is blob-backed on the live session.
    session
        .externalize_media(&store, 0)
        .await
        .expect("externalize before compaction");
    round_trip(&session).expect("externalized baseline decodes");

    // Model view: hydrated copies of the same rows.
    let mut hydrated = session.messages().to_vec();
    hydrate_messages_for_execution(&store, &mut hydrated, MissingBlobBehavior::Error)
        .await
        .expect("hydrate for the model");

    // Compaction rebuild from the hydrated copies: system verbatim, one
    // summary, then the retained tail including the (now inline) image row.
    let mut replacement = vec![hydrated[0].clone()];
    replacement.push(Message::User(UserMessage::compaction_summary(
        "[Compaction summary] earlier questions",
    )));
    replacement.extend(hydrated[5..].iter().cloned());
    session
        .stage_validated_compaction_for_test(replacement, 12)
        .expect("compaction commits");
    round_trip(&session).expect("compacted session decodes before the checkpoint");

    // Next checkpoint externalizes the whole document again.
    session
        .externalize_media(&store, 0)
        .await
        .expect("externalize after compaction");
    let decoded = round_trip(&session);
    assert!(
        decoded.is_ok(),
        "compacted + externalized WholeBlob must decode, got: {}",
        decoded.err().unwrap_or_default()
    );
}

/// Control: the same compaction rebuilt from the live (blob-backed) rows.
#[tokio::test]
async fn compaction_over_live_rows_survives_checkpoint_externalization() {
    let store = ContentAddressedBlobStore::default();
    let mut session = Session::new();
    session.append_system_message("system prompt");
    for turn in 0..6 {
        session.push(Message::User(UserMessage::text(format!("question {turn}"))));
    }
    session.push(image_user_message("look at this"));
    session.push(Message::User(UserMessage::text("and then")));
    session
        .externalize_media(&store, 0)
        .await
        .expect("externalize before compaction");

    let live = session.messages().to_vec();
    let mut replacement = vec![live[0].clone()];
    replacement.push(Message::User(UserMessage::compaction_summary(
        "[Compaction summary] earlier questions",
    )));
    replacement.extend(live[5..].iter().cloned());
    session
        .stage_validated_compaction_for_test(replacement, 12)
        .expect("compaction commits");
    session
        .externalize_media(&store, 0)
        .await
        .expect("externalize after compaction");
    let decoded = round_trip(&session);
    assert!(
        decoded.is_ok(),
        "control must decode, got: {}",
        decoded.err().unwrap_or_default()
    );
}

/// Shape under test: the image arrives inline during the turn, the compaction
/// commits while it is still inline, and the checkpoint externalizes it.
#[tokio::test]
async fn compaction_over_inline_image_then_externalize_survives_ingress() {
    let store = ContentAddressedBlobStore::default();
    let mut session = Session::new();
    session.append_system_message("system prompt");
    for turn in 0..6 {
        session.push(Message::User(UserMessage::text(format!("question {turn}"))));
    }
    session.push(image_user_message("fresh inline image this turn"));
    session.push(Message::User(UserMessage::text("and then")));

    let live = session.messages().to_vec();
    let mut replacement = vec![live[0].clone()];
    replacement.push(Message::User(UserMessage::compaction_summary(
        "[Compaction summary] earlier questions",
    )));
    replacement.extend(live[5..].iter().cloned());
    session
        .stage_validated_compaction_for_test(replacement, 12)
        .expect("compaction commits");
    session
        .externalize_media(&store, 0)
        .await
        .expect("externalize after compaction");
    let decoded = round_trip(&session);
    assert!(
        decoded.is_ok(),
        "inline-then-externalized must decode, got: {}",
        decoded.err().unwrap_or_default()
    );
}

use meerkat_core::types::{SystemNoticeKind, SystemNoticeMessage};

fn background_job_notice(text: &str) -> Message {
    Message::SystemNotice(SystemNoticeMessage::new(
        SystemNoticeKind::BackgroundJob,
        text,
    ))
}

/// After a compaction and an in-prefix externalization, a synthetic refresh
/// that would strip inside the audited endpoint is refused by the Session
/// itself rather than corrupting the document.
#[tokio::test]
async fn synthetic_refresh_inside_audited_endpoint_is_refused_not_corrupting() {
    let store = ContentAddressedBlobStore::default();
    let mut session = Session::new();
    session.append_system_message("system prompt");
    for turn in 0..6 {
        session.push(Message::User(UserMessage::text(format!("question {turn}"))));
    }
    session.push(background_job_notice("job 1 running"));
    session.push(image_user_message("fresh inline image this turn"));
    session.push(Message::User(UserMessage::text("and then")));

    let live = session.messages().to_vec();
    let mut replacement = vec![live[0].clone()];
    replacement.push(Message::User(UserMessage::compaction_summary(
        "[Compaction summary] earlier questions",
    )));
    replacement.extend(live[5..].iter().cloned());
    session
        .stage_validated_compaction_for_test(replacement, 12)
        .expect("compaction commits");
    session
        .externalize_media(&store, 0)
        .await
        .expect("externalize after compaction");
    let refused = session.replace_synthetic_notices(SystemNoticeKind::BackgroundJob, vec![]);
    assert!(
        refused.is_err(),
        "a strip inside the audited endpoint must be refused"
    );
    round_trip(&session).expect("the refused refresh left the document decodable");
}

/// The writer-side guard: a live transcript that diverges from its audited
/// endpoint is refused at encode time with the same relation the ingress
/// guard enforces, and the divergence report names the first bad row.
#[test]
fn writer_guard_refuses_a_document_the_ingress_guard_would_refuse() {
    let mut session = Session::new();
    session.append_system_message("system prompt");
    for turn in 0..4 {
        session.push(Message::User(UserMessage::text(format!("question {turn}"))));
    }
    let live = session.messages().to_vec();
    let mut replacement = vec![live[0].clone()];
    replacement.push(Message::User(UserMessage::compaction_summary(
        "[Compaction summary] earlier questions",
    )));
    replacement.extend(live[3..].iter().cloned());
    session
        .stage_validated_compaction_for_test(replacement, 12)
        .expect("compaction commits");
    assert_eq!(
        session
            .audited_endpoint_divergence()
            .expect("coherence check"),
        None,
        "a freshly compacted session is coherent"
    );
    session
        .to_persisted_artifact()
        .expect("coherent session encodes");

    // Manufacture the divergence the guard exists for: mutate a row inside
    // the audited endpoint without any audit.
    let mut tampered = session.messages().to_vec();
    tampered[1] = Message::User(UserMessage::text("tampered row inside the endpoint"));
    session.replace_messages_unaudited_for_test(tampered);
    let divergence = session
        .audited_endpoint_divergence()
        .expect("coherence check")
        .expect("tampered session diverges");
    assert_eq!(
        divergence.kind,
        meerkat_core::AuditedEndpointDivergenceKind::LivePrefixDiverges
    );
    assert_eq!(divergence.first_divergent_row, Some(1));
    let refused = session
        .to_persisted_artifact()
        .expect_err("writer guard refuses the divergent document");
    assert!(
        refused
            .to_string()
            .contains("refusing to persist a WholeBlob document"),
        "unexpected error: {refused}"
    );
    // The ingress guard agrees on the same document when written unchecked.
    let unchecked = serde_json::to_vec(&session).expect("plain serde still serializes");
    let ingress = Session::from_persisted_bytes(&unchecked).expect_err("ingress refuses");
    assert!(
        ingress
            .to_string()
            .contains("live transcript does not preserve the graph-proved audited endpoint"),
        "unexpected ingress error: {ingress}"
    );
}

/// Control: the same refresh without the externalization in between keeps
/// the committed row prefix and therefore retains the notice.
#[tokio::test]
async fn synthetic_refresh_after_compaction_without_externalization_decodes() {
    let mut session = Session::new();
    session.append_system_message("system prompt");
    for turn in 0..6 {
        session.push(Message::User(UserMessage::text(format!("question {turn}"))));
    }
    session.push(background_job_notice("job 1 running"));
    session.push(Message::User(UserMessage::text("and then")));

    let live = session.messages().to_vec();
    let mut replacement = vec![live[0].clone()];
    replacement.push(Message::User(UserMessage::compaction_summary(
        "[Compaction summary] earlier questions",
    )));
    replacement.extend(live[5..].iter().cloned());
    session
        .stage_validated_compaction_for_test(replacement, 12)
        .expect("compaction commits");
    session
        .replace_synthetic_notices(SystemNoticeKind::BackgroundJob, vec![])
        .expect("synthetic refresh");
    let decoded = round_trip(&session);
    assert!(
        decoded.is_ok(),
        "control must decode, got: {}",
        decoded.err().unwrap_or_default()
    );
}

use meerkat_core::types::{AssistantBlock, BlockAssistantMessage, StopReason, ToolResult};

fn tool_turn(session: &mut Session, user: Message, tool_id: &str, reply: &str) {
    session.push(user);
    session.push(Message::BlockAssistant(BlockAssistantMessage::new(
        vec![AssistantBlock::ToolUse {
            id: tool_id.to_string(),
            name: "lookup".to_string(),
            args: serde_json::value::RawValue::from_string("{\"q\":\"x\"}".to_string())
                .expect("valid args"),
            meta: None,
        }],
        StopReason::ToolUse,
    )));
    session.push(Message::tool_results(vec![ToolResult::new(
        tool_id.to_string(),
        format!("result for {tool_id}"),
        false,
    )]));
    session.push(Message::BlockAssistant(BlockAssistantMessage::new(
        vec![AssistantBlock::Text {
            text: reply.to_string(),
            meta: None,
        }],
        StopReason::EndTurn,
    )));
}

fn expect_decodes(session: &Session, step: &str) {
    if let Err(error) = round_trip(session) {
        panic!("step '{step}' produced an undecodable document: {error}");
    }
}

/// Compaction over the live rows: keep the system row, one summary, and
/// the last `keep_tail` rows (taken from `source`, which may be hydrated).
fn compact_keeping_tail(session: &mut Session, source: &[Message], keep_tail: usize, step: &str) {
    let mut replacement = vec![source[0].clone()];
    replacement.push(Message::User(UserMessage::compaction_summary(
        "[Compaction summary] earlier turns",
    )));
    replacement.extend(source[source.len() - keep_tail..].iter().cloned());
    session
        .stage_validated_compaction_for_test(replacement, 12)
        .unwrap_or_else(|error| panic!("compaction at step '{step}' refused: {error}"));
}

/// Multi-step lifecycle: three compactions, tool turns, inline images that
/// get externalized at checkpoints (inside and outside audited endpoints),
/// synthetic notice refreshes beyond the boundary, decode after every step.
#[tokio::test]
async fn long_lived_wholeblob_lifecycle_decodes_after_every_step() {
    let store = ContentAddressedBlobStore::default();
    let mut session = Session::new();
    session.append_system_message("system prompt");
    for turn in 0..3 {
        tool_turn(
            &mut session,
            Message::User(UserMessage::text(format!("q{turn}"))),
            &format!("t{turn}"),
            &format!("a{turn}"),
        );
    }
    session.push(background_job_notice("job A running"));
    expect_decodes(&session, "three text turns");
    session.externalize_media(&store, 0).await.expect("ext 1");
    expect_decodes(&session, "externalize 1");

    // Compaction 1 over live rows (keep last tool turn + the notice).
    let live = session.messages().to_vec();
    compact_keeping_tail(&mut session, &live, 5, "compaction 1");
    expect_decodes(&session, "compaction 1");

    // Turns after compaction 1, including an inline image turn and a notice refresh.
    tool_turn(
        &mut session,
        image_user_message("here is a photo"),
        "t3",
        "a3",
    );
    session
        .replace_synthetic_notices(
            SystemNoticeKind::BackgroundJob,
            vec![background_job_notice("job A finished; job B running")],
        )
        .expect("refresh beyond boundary");
    expect_decodes(&session, "post-compaction-1 turns and refresh");
    session.externalize_media(&store, 0).await.expect("ext 2");
    expect_decodes(&session, "externalize 2");

    // Compaction 2 rebuilt from hydrated copies (retained tail has the image row).
    let mut hydrated = session.messages().to_vec();
    hydrate_messages_for_execution(&store, &mut hydrated, MissingBlobBehavior::Error)
        .await
        .expect("hydrate");
    compact_keeping_tail(&mut session, &hydrated, 5, "compaction 2");
    expect_decodes(&session, "compaction 2 (hydrated rebuild)");
    session.externalize_media(&store, 0).await.expect("ext 3");
    expect_decodes(&session, "externalize 3 (inside endpoint)");

    tool_turn(
        &mut session,
        Message::User(UserMessage::text("after second compaction")),
        "t4",
        "a4",
    );
    expect_decodes(&session, "final turn");

    // Store-side finalization shape: decode, complete an intent, re-encode.
    let bytes = serde_json::to_vec(&session).expect("encode");
    let mut decoded = Session::from_persisted_bytes(&bytes).expect("decode");
    let intents = decoded
        .compaction_projection_intents()
        .expect("intents readable");
    assert_eq!(
        intents.len(),
        2,
        "two pending compaction intents ride along"
    );
    decoded
        .complete_compaction_projection_intent(&intents[0].projection)
        .expect("complete intent");
    expect_decodes(&decoded, "re-encode after finalization cleanup");
}
