//! [`Session::with_inline_images_externalized`] over the transcript shapes a
//! compacted session whose images were written inline reaches a conversion
//! with: camera snapshots in tool results, an image in a runtime notice and
//! in a user message, held in the anchor, in rewrite replacements, in the
//! parent advance a later compaction swallowed, and in the live tail.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;
use crate::session_store::strand_layout_for_retained_history;
use crate::types::{
    AssistantBlock, BlockAssistantMessage, ContentBlock, ImageData, StopReason, SystemNoticeBlock,
    SystemNoticeKind, SystemNoticeMessage, ToolResult, UserMessage,
};

fn inline_png(data: &str) -> ContentBlock {
    ContentBlock::Image {
        media_type: "image/png".to_string(),
        data: ImageData::Inline {
            data: data.to_string(),
        },
    }
}

/// Base64 of a short payload distinct per `index`.
fn photo(index: usize) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(format!("PNG photo {index}"))
}

/// One camera turn: a user request, a tool call whose result carries two
/// snapshots, a runtime notice carrying a third, and the reply.
fn push_camera_turn(session: &mut Session, turn: usize, photos: &mut Vec<String>) {
    let call_id = format!("toolu_{turn}");
    let snapshots = [photo(photos.len()), photo(photos.len() + 1)];
    let notice = photo(photos.len() + 2);
    photos.extend(snapshots.iter().cloned());
    photos.push(notice.clone());
    session.push(Message::User(UserMessage::text(format!(
        "check the cameras ({turn})"
    ))));
    session.push(Message::BlockAssistant(BlockAssistantMessage::new(
        vec![AssistantBlock::ToolUse {
            id: call_id.clone(),
            name: "camera_snapshot".to_string(),
            args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
            meta: None,
        }],
        StopReason::ToolUse,
    )));
    session.push(Message::tool_results(vec![ToolResult::with_blocks(
        call_id,
        vec![
            ContentBlock::Text {
                text: "two snapshots".to_string(),
            },
            inline_png(&snapshots[0]),
            inline_png(&snapshots[1]),
        ],
        false,
    )]));
    session.push(Message::SystemNotice(SystemNoticeMessage::with_block(
        SystemNoticeKind::Generic,
        None,
        SystemNoticeBlock::RuntimeNotice {
            category: "motion".to_string(),
            detail: Some("the porch camera saw motion".to_string()),
            payload: None,
            content: vec![inline_png(&notice)],
        },
    )));
    session.push(Message::BlockAssistant(BlockAssistantMessage::new(
        vec![AssistantBlock::Text {
            text: format!("all quiet ({turn})"),
            meta: None,
        }],
        StopReason::EndTurn,
    )));
}

/// A session compacted `compactions` times, each compaction swallowing a
/// camera turn appended after the previous one and keeping a user photo in
/// its replacement, with one more photo in the live tail, as the persisted
/// WholeBlob document a conversion decodes. Returns it with every photo it
/// holds, in the order the transform first meets them.
fn compacted_session_with_inline_images(compactions: usize) -> (Session, Vec<String>) {
    let mut session = Session::new();
    let mut photos = Vec::new();
    for turn in 0..compactions {
        push_camera_turn(&mut session, turn, &mut photos);
        let kept = photo(photos.len());
        photos.push(kept.clone());
        let end = session.messages().len();
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end },
                vec![
                    Message::User(UserMessage::text(format!("summary {turn}"))),
                    Message::User(UserMessage::with_blocks(vec![inline_png(&kept)])),
                ],
                TranscriptRewriteReason::new("compaction"),
                Some("test".to_string()),
                None,
            )
            .unwrap();
    }
    let tail = photo(photos.len());
    photos.push(tail.clone());
    session.push(Message::User(UserMessage::with_blocks(vec![inline_png(
        &tail,
    )])));
    if compactions > 4 {
        assert!(
            session
                .retire_transcript_history(crate::TranscriptHistoryRetention::default())
                .unwrap()
                > 0
        );
    }
    let document = serde_json::to_vec(&session).unwrap();
    let decoded = Session::decode_whole_blob_document(&document)
        .unwrap()
        .into_session();
    (decoded, photos)
}

/// Every serialized row and graph edge a layout of `session` writes.
fn laid_out_bytes(session: &Session) -> Result<Vec<Vec<u8>>, crate::SessionStoreError> {
    let history = session
        .validated_transcript_history_state()
        .expect("a validated graph");
    let retained = strand_layout_for_retained_history(session, history.as_ref())?;
    let layout = retained.layout;
    let mut bytes = layout.serialized_anchor;
    for rewrite in layout.rewrites {
        bytes.push(rewrite.serialized_graph_edge);
        bytes.extend(rewrite.serialized_parent_suffix);
        bytes.extend(rewrite.serialized_replacement);
    }
    bytes.extend(layout.serialized_tail);
    Ok(bytes)
}

fn commits(session: &Session) -> Vec<TranscriptRewriteCommit> {
    session
        .transcript_history_state()
        .unwrap()
        .expect("a compacted session has a graph")
        .commits()
        .cloned()
        .collect()
}

fn assert_externalizes(compactions: usize) {
    let (source, photos) = compacted_session_with_inline_images(compactions);
    let source_refusal = laid_out_bytes(&source).expect_err("inline images refuse the layout");
    assert!(
        source_refusal.to_string().contains("carries inline media"),
        "{source_refusal}"
    );

    let externalized = source
        .with_inline_images_externalized()
        .unwrap()
        .expect("the session holds inline images");
    // Every distinct image, once, in the order the transform met it, named
    // by its content address.
    let retired_photos = if compactions > 4 {
        // The bodies of the retired occurrences are gone; their photos were
        // never in the retained graph.
        let retained = source
            .transcript_history_state()
            .unwrap()
            .unwrap()
            .retired_count();
        assert!(retained > 0);
        Some(retained)
    } else {
        None
    };
    let payload_data = externalized
        .payloads
        .iter()
        .map(|payload| payload.data.clone())
        .collect::<Vec<_>>();
    for payload in &externalized.payloads {
        assert_eq!(payload.media_type, "image/png");
        assert_eq!(
            payload.blob_id,
            crate::content_blob_id(&payload.media_type, &payload.data)
        );
    }
    match retired_photos {
        None => assert_eq!(payload_data, photos),
        Some(_) => {
            // A re-anchored graph holds a suffix of the photos: the retained
            // ones in order, ending with the tail photo.
            assert!(!payload_data.is_empty());
            let start = photos.len() - payload_data.len();
            assert_eq!(payload_data, photos[start..]);
        }
    }

    let session = &externalized.session;
    let document = serde_json::to_string(session).unwrap();
    for data in &payload_data {
        assert!(!document.contains(data.as_str()), "{data} is still inline");
    }
    assert_eq!(
        session.transcript_revision().unwrap(),
        source.transcript_revision().unwrap()
    );
    assert_eq!(commits(session), commits(&source));
    assert_eq!(
        session
            .transcript_history_state()
            .unwrap()
            .unwrap()
            .rewrite_prefix(),
        source
            .transcript_history_state()
            .unwrap()
            .unwrap()
            .rewrite_prefix()
    );
    for bytes in laid_out_bytes(session).expect("the externalized session lays out") {
        let row = String::from_utf8(bytes).unwrap();
        for data in &payload_data {
            assert!(!row.contains(data.as_str()), "a laid-out row holds {data}");
        }
    }
    // Every retained revision materializes, its images as blob references.
    for commit in commits(session).iter().skip(retired_photos.unwrap_or(0)) {
        for revision in [&commit.parent_revision, &commit.revision] {
            let rows = session
                .retained_transcript_revision_rows(revision)
                .unwrap()
                .expect("a retained revision");
            let rows = serde_json::to_string(rows.messages()).unwrap();
            for data in &payload_data {
                assert!(!rows.contains(data.as_str()), "revision {revision}");
            }
        }
    }
    // Deterministic, and a fixed point.
    assert_eq!(
        serde_json::to_value(
            &source
                .with_inline_images_externalized()
                .unwrap()
                .unwrap()
                .session
        )
        .unwrap(),
        serde_json::to_value(session).unwrap()
    );
    assert!(session.with_inline_images_externalized().unwrap().is_none());
}

#[test]
fn inline_images_in_a_whole_graph_become_blob_references_and_lay_out() {
    assert_externalizes(2);
}

#[test]
fn inline_images_in_a_re_anchored_graph_become_blob_references_and_lay_out() {
    assert_externalizes(6);
}

#[test]
fn a_session_without_inline_images_is_left_alone() {
    let mut session = Session::new();
    session.push(Message::User(UserMessage::text("no photos here")));
    assert!(session.with_inline_images_externalized().unwrap().is_none());
}

/// A content-addressed append (an occurrence whose parent rows were
/// rematerialized after its base, here with the base's images already
/// externalized) keeps its kind and commit, but its parent row prefix is
/// re-derived to extend the preceding endpoint, as the layout requires;
/// its parent materializes with its images as blob references.
#[test]
fn a_content_addressed_append_is_re_derived_from_its_preceding_endpoint() {
    let (source, _) = compacted_session_with_inline_images(2);
    let state = source.transcript_history_state().unwrap().unwrap();
    let mut records = commits(&source)
        .into_iter()
        .map(|commit| TranscriptRewriteRecord {
            parent_body: state.materialize_revision(&commit.parent_revision).unwrap(),
            revision_body: state.materialize_revision(&commit.revision).unwrap(),
            commit,
            digest_format: 0,
        })
        .collect::<Vec<_>>();
    // The second occurrence's parent holds its base's rows with their images
    // already externalized: the same content, different row bytes.
    let base_len = records[0].revision_body.messages.len();
    let mut rematerialized = crate::image_content::InlineImageCollector::default();
    let mut rematerialized_any = false;
    for message in &mut records[1].parent_body.messages[..base_len] {
        rematerialized_any |= rematerialized.externalize(message);
    }
    assert!(rematerialized_any, "the base rows hold an inline image");
    let graph = TranscriptHistoryState::from_rewrite_records(records)
        .unwrap()
        .expect("a graph");
    assert_eq!(
        graph.parent_transitions()[1],
        TranscriptRewriteParentTransition::ContentAddressedAppend
    );
    // As decoded, its parent row prefix is the rematerialized rows' own
    // baseline, which does not extend the preceding endpoint.
    let appended_rows = |edge: &TranscriptRevisionEdge| {
        edge.parent_advance()
            .appended()
            .iter()
            .map(|message| serde_json::to_vec(message).unwrap())
            .collect::<Vec<_>>()
    };
    let decoded_edges = graph.edges();
    assert_ne!(
        decoded_edges[1].parent_row_prefix(),
        &decoded_edges[0]
            .result_witness()
            .row_prefix()
            .extend_serialized_rows(&appended_rows(&decoded_edges[1]))
            .unwrap()
    );

    let mut collector = crate::image_content::InlineImageCollector::default();
    let externalized = graph
        .with_bodies_rewritten(&mut |message| collector.externalize(message))
        .unwrap()
        .expect("the graph holds inline images");
    assert_eq!(
        externalized.parent_transitions(),
        graph.parent_transitions()
    );
    assert_eq!(
        externalized.commits().cloned().collect::<Vec<_>>(),
        graph.commits().cloned().collect::<Vec<_>>()
    );
    let edges = externalized.edges();
    assert_eq!(
        edges[1].parent_row_prefix(),
        &edges[0]
            .result_witness()
            .row_prefix()
            .extend_serialized_rows(&appended_rows(&edges[1]))
            .unwrap()
    );
    let parent = externalized
        .materialize_revision(edges[1].parent_revision())
        .unwrap();
    let parent = serde_json::to_string(&parent.messages).unwrap();
    for payload in collector.into_payloads() {
        assert!(!parent.contains(payload.data.as_str()));
    }
    assert!(
        parent.contains("\"blob_id\""),
        "the parent names its images by blob id"
    );
}

/// A blob store that keeps each payload under the id it is put by.
#[derive(Default)]
struct PayloadStore(std::sync::Mutex<std::collections::BTreeMap<crate::BlobId, (String, String)>>);

#[async_trait::async_trait]
impl crate::BlobStore for PayloadStore {
    async fn put_image(
        &self,
        media_type: &str,
        data: &str,
    ) -> Result<crate::BlobRef, crate::BlobStoreError> {
        let blob_id = crate::content_blob_id(media_type, data);
        self.0
            .lock()
            .unwrap()
            .insert(blob_id.clone(), (media_type.to_string(), data.to_string()));
        Ok(crate::BlobRef {
            blob_id,
            media_type: media_type.to_string(),
        })
    }

    async fn get(
        &self,
        blob_id: &crate::BlobId,
    ) -> Result<crate::BlobPayload, crate::BlobStoreError> {
        let (media_type, data) = self
            .0
            .lock()
            .unwrap()
            .get(blob_id)
            .cloned()
            .ok_or_else(|| crate::BlobStoreError::NotFound(blob_id.clone()))?;
        Ok(crate::BlobPayload {
            blob_id: blob_id.clone(),
            media_type,
            data,
        })
    }

    async fn delete(&self, blob_id: &crate::BlobId) -> Result<(), crate::BlobStoreError> {
        self.0.lock().unwrap().remove(blob_id);
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        false
    }
}

fn image(media_type: &str, data: &str) -> ContentBlock {
    ContentBlock::Image {
        media_type: media_type.to_string(),
        data: ImageData::Inline {
            data: data.to_string(),
        },
    }
}

/// A camera tool call and its result carrying two snapshots, one under a
/// non-canonical media type, and an incoming comms notice carrying a third.
fn camera_rows(turn: usize) -> Vec<Message> {
    let call_id = format!("toolu_{turn}");
    let comms: SystemNoticeBlock = serde_json::from_value(serde_json::json!({
        "type": "comms",
        "kind": "message",
        "direction": "incoming",
        "content": [image("image/jpeg", &photo(3 * turn + 2))],
    }))
    .unwrap();
    vec![
        Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::ToolUse {
                id: call_id.clone(),
                name: "camera_snapshot".to_string(),
                args: serde_json::value::RawValue::from_string("{}".to_string()).unwrap(),
                meta: None,
            }],
            StopReason::ToolUse,
        )),
        Message::tool_results(vec![ToolResult::with_blocks(
            call_id,
            vec![
                image("image/PNG", &photo(3 * turn)),
                image("image/png", &photo(3 * turn + 1)),
            ],
            false,
        )]),
        Message::SystemNotice(SystemNoticeMessage::with_block(
            SystemNoticeKind::Generic,
            None,
            comms,
        )),
    ]
}

/// Six rows of text, the shape each compaction of the loop leaves behind.
fn compaction_replacement(turn: usize) -> Vec<Message> {
    let mut rows = vec![Message::User(UserMessage::text(format!("summary {turn}")))];
    for exchange in 0..2 {
        rows.push(Message::User(UserMessage::text(format!(
            "question {turn}.{exchange}"
        ))));
        rows.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: format!("answer {turn}.{exchange}"),
                meta: None,
            }],
            StopReason::EndTurn,
        )));
    }
    rows.push(Message::User(UserMessage::text(format!("next {turn}"))));
    rows
}

/// The shape of a long-lived household session written by a build that
/// kept tool-result images inline: one prune edit, then compactions, each
/// swallowing the camera rows appended after the previous one, ten of them
/// in a loop that takes the transcript from nine rows back to six, and the
/// graph retired to the default retention, so it is re-anchored. Every image
/// is in a parent advance or the live tail, none in a replacement. The
/// externalized copy keeps the transcript revision, every commit, the
/// rewrite prefix and each image's own media type string; it lays out with
/// no inline bytes; and every retained revision, hydrated from the
/// payloads, is the source's revision byte for byte.
#[tokio::test]
async fn a_re_anchored_household_history_with_tool_result_images_converts() {
    let mut session = Session::new();
    for (index, text) in ["hello", "prune me", "and me", "keep this"]
        .into_iter()
        .enumerate()
    {
        session.push(Message::User(UserMessage::text(text)));
        session.push(Message::BlockAssistant(BlockAssistantMessage::new(
            vec![AssistantBlock::Text {
                text: format!("reply {index}"),
                meta: None,
            }],
            StopReason::EndTurn,
        )));
    }
    session
        .commit_transcript_rewrite(
            TranscriptRewriteSelection::MessageRange { start: 2, end: 6 },
            vec![Message::User(UserMessage::text("pruned"))],
            TranscriptRewriteReason::new("prune"),
            Some("test".to_string()),
            None,
        )
        .unwrap();
    session.push(Message::User(UserMessage::text("show me the cameras")));
    for turn in 0..13 {
        session.push_batch(camera_rows(turn));
        let end = session.messages().len();
        session
            .commit_transcript_rewrite(
                TranscriptRewriteSelection::MessageRange { start: 0, end },
                compaction_replacement(turn),
                TranscriptRewriteReason::new("compaction"),
                Some("test".to_string()),
                None,
            )
            .unwrap();
    }
    session.push_batch(camera_rows(13));
    assert_eq!(commits(&session).len(), 14);
    assert!(
        session
            .retire_transcript_history(crate::TranscriptHistoryRetention::default())
            .unwrap()
            > 0
    );
    let source = Session::decode_whole_blob_document(&serde_json::to_vec(&session).unwrap())
        .unwrap()
        .into_session();
    let refusal = laid_out_bytes(&source).expect_err("inline images refuse the layout");
    assert!(
        refusal
            .to_string()
            .contains("compact rewrite delta carries inline media"),
        "{refusal}"
    );

    let externalized = source
        .with_inline_images_externalized()
        .unwrap()
        .expect("the session holds inline images");
    let media_types = externalized
        .payloads
        .iter()
        .map(|payload| payload.media_type.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        media_types,
        ["image/PNG", "image/jpeg", "image/png"]
            .into_iter()
            .collect()
    );
    for payload in &externalized.payloads {
        assert_eq!(
            payload.blob_id,
            crate::content_blob_id(&payload.media_type, &payload.data)
        );
    }
    let converted = &externalized.session;
    assert_eq!(
        converted.transcript_revision().unwrap(),
        source.transcript_revision().unwrap()
    );
    assert_eq!(commits(converted), commits(&source));
    assert_eq!(
        converted
            .transcript_history_state()
            .unwrap()
            .unwrap()
            .rewrite_prefix(),
        source
            .transcript_history_state()
            .unwrap()
            .unwrap()
            .rewrite_prefix()
    );
    for bytes in laid_out_bytes(converted).expect("the converted session lays out") {
        let row = String::from_utf8(bytes).unwrap();
        for payload in &externalized.payloads {
            assert!(!row.contains(payload.data.as_str()));
        }
    }

    let store = PayloadStore::default();
    for payload in &externalized.payloads {
        crate::BlobStore::put_image(&store, &payload.media_type, &payload.data)
            .await
            .unwrap();
    }
    let retired = source
        .transcript_history_state()
        .unwrap()
        .unwrap()
        .retired_count();
    let mut revisions = vec![source.transcript_revision().unwrap()];
    for commit in commits(&source).into_iter().skip(retired) {
        revisions.push(commit.parent_revision);
        revisions.push(commit.revision);
    }
    for revision in revisions {
        let mut rows = converted
            .retained_transcript_revision_rows(&revision)
            .unwrap()
            .expect("a retained revision")
            .messages()
            .to_vec();
        crate::image_content::hydrate_messages_for_execution(
            &store,
            &mut rows,
            crate::image_content::MissingBlobBehavior::Error,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_value(&rows).unwrap(),
            serde_json::to_value(
                source
                    .retained_transcript_revision_rows(&revision)
                    .unwrap()
                    .expect("a retained revision")
                    .messages()
            )
            .unwrap(),
            "revision {revision}"
        );
    }
}

/// A WholeBlob session loaded without a rewrite graph holds only the lazy
/// row-lineage mark. An inline image appended after the load is moved to the
/// blob store in place, as a checkpoint and a compaction do before they bind
/// the rows: the scan changes that row, and the session keeps its row
/// lineage, so the next compaction's rewrite commits and the session store
/// accepts the compacted document over the checkpointed one.
#[tokio::test]
async fn a_mutating_media_scan_keeps_a_loaded_whole_blob_sessions_row_lineage() {
    let mut persisted = Session::new();
    persisted.push(Message::User(UserMessage::text("hello")));
    persisted.push(Message::BlockAssistant(BlockAssistantMessage::new(
        vec![AssistantBlock::Text {
            text: "hi".to_string(),
            meta: None,
        }],
        StopReason::EndTurn,
    )));
    let mut live = Session::decode_whole_blob_document(&serde_json::to_vec(&persisted).unwrap())
        .unwrap()
        .into_session();
    let snapshot = photo(0);
    live.push(Message::User(UserMessage::with_blocks(vec![inline_png(
        &snapshot,
    )])));

    let store = PayloadStore::default();
    live.externalize_media(&store, 0).await.unwrap();
    let scanned = serde_json::to_string(&live.messages()[2]).unwrap();
    assert!(
        !scanned.contains(snapshot.as_str()) && scanned.contains("\"blob_id\""),
        "the scan moved the appended image to the blob store: {scanned}"
    );
    let rows = u64::try_from(live.messages().len()).unwrap();
    assert_eq!(
        live.exact_message_row_prefix_at(rows),
        Some(
            crate::session_store::SessionMessageRowPrefixAccumulator::from_messages(
                live.messages()
            )
            .unwrap()
        ),
        "the mutating media scan keeps the loaded WholeBlob row lineage, over the scanned rows"
    );

    // The checkpoint writes the scanned rows; the next compaction folds the
    // first turn.
    let checkpointed = Session::decode_whole_blob_document(&serde_json::to_vec(&live).unwrap())
        .unwrap()
        .into_session();
    live.commit_transcript_rewrite(
        TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
        vec![Message::User(UserMessage::text("summary"))],
        TranscriptRewriteReason::new("compaction"),
        Some("test".to_string()),
        None,
    )
    .expect("the next compaction's rewrite commits");
    assert_eq!(commits(&live).len(), 1);

    let history = live
        .validated_transcript_history_state()
        .expect("history validation")
        .expect("the compaction's graph");
    let revision = live.transcript_content_digest().unwrap();
    let chain = crate::session_store::find_transcript_rewrite_commit_chain_extending_session(
        &history,
        &checkpointed,
        &revision,
    )
    .unwrap();
    assert_eq!(
        chain.map(|chain| chain.len()),
        Some(1),
        "the session store accepts the compacted document over the checkpointed one"
    );
    let reloaded = Session::decode_whole_blob_document(&serde_json::to_vec(&live).unwrap())
        .unwrap()
        .into_session();
    assert_eq!(reloaded.messages(), live.messages());
    assert_eq!(commits(&reloaded), commits(&live));
}
