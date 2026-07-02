use std::collections::BTreeSet;

use crate::blob::{BlobId, BlobStore, BlobStoreError};
use crate::session::SessionDeferredTurnState;
use crate::types::{ContentBlock, ContentInput, ImageData, Message, SystemNoticeBlock};

/// Policy for durable blobs that are missing at hydration time.
///
/// There is intentionally NO `Default`: every hydration seam must choose its
/// behavior explicitly. Anything that feeds the model (LLM execution,
/// deferred-turn continuation input) must use `Error` — a missing blob there
/// is a typed terminal fault, never silently rewritten prompt text.
/// `HistoricalPlaceholder` is only for read-side transcript/projection
/// rendering, where an evicted blob must still display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingBlobBehavior {
    /// A missing durable blob is a hard fault: hydration returns the typed
    /// `BlobStoreError::NotFound` so the caller can terminalize the turn.
    Error,
    /// A missing durable blob degrades to a textual placeholder, for
    /// read-side rendering of history whose blob may have been evicted.
    HistoricalPlaceholder,
}

pub async fn externalize_content_blocks(
    blob_store: &dyn BlobStore,
    blocks: &mut [ContentBlock],
) -> Result<(), BlobStoreError> {
    for block in blocks.iter_mut() {
        if let ContentBlock::Image {
            media_type,
            data: ImageData::Inline { data },
        } = block
        {
            let blob_ref = blob_store.put_image(media_type, data).await?;
            *block = ContentBlock::Image {
                media_type: blob_ref.media_type,
                data: ImageData::Blob {
                    blob_id: blob_ref.blob_id,
                },
            };
        }
    }
    Ok(())
}

pub async fn hydrate_content_blocks(
    blob_store: &dyn BlobStore,
    blocks: &mut [ContentBlock],
    missing_behavior: MissingBlobBehavior,
) -> Result<(), BlobStoreError> {
    for block in blocks.iter_mut() {
        let Some((media_type, blob_id)) = block.image_blob_ref() else {
            continue;
        };
        match blob_store.get(blob_id).await {
            Ok(payload) => {
                *block = ContentBlock::Image {
                    media_type: media_type.to_string(),
                    data: ImageData::Inline { data: payload.data },
                };
            }
            Err(BlobStoreError::NotFound(missing)) => match missing_behavior {
                MissingBlobBehavior::Error => return Err(BlobStoreError::NotFound(missing)),
                MissingBlobBehavior::HistoricalPlaceholder => {
                    *block = ContentBlock::Text {
                        text: format!("[image unavailable: {missing}]"),
                    };
                }
            },
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

pub async fn externalize_messages_from(
    blob_store: &dyn BlobStore,
    messages: &mut [Message],
    start: usize,
) -> Result<(), BlobStoreError> {
    for msg in messages.iter_mut().skip(start) {
        match msg {
            Message::User(user) => {
                externalize_content_blocks(blob_store, &mut user.content).await?;
            }
            Message::ToolResults { results, .. } => {
                for result in results.iter_mut() {
                    externalize_content_blocks(blob_store, &mut result.content).await?;
                }
            }
            Message::SystemNotice(notice) => {
                externalize_system_notice_blocks(blob_store, &mut notice.blocks).await?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub async fn hydrate_messages_for_execution(
    blob_store: &dyn BlobStore,
    messages: &mut [Message],
    missing_behavior: MissingBlobBehavior,
) -> Result<(), BlobStoreError> {
    for msg in messages.iter_mut() {
        match msg {
            Message::User(user) => {
                hydrate_content_blocks(blob_store, &mut user.content, missing_behavior).await?;
            }
            Message::ToolResults { results, .. } => {
                for result in results.iter_mut() {
                    hydrate_content_blocks(blob_store, &mut result.content, missing_behavior)
                        .await?;
                }
            }
            Message::SystemNotice(notice) => {
                hydrate_system_notice_blocks(blob_store, &mut notice.blocks, missing_behavior)
                    .await?;
            }
            _ => {}
        }
    }
    Ok(())
}

async fn externalize_system_notice_blocks(
    blob_store: &dyn BlobStore,
    blocks: &mut [SystemNoticeBlock],
) -> Result<(), BlobStoreError> {
    for block in blocks {
        match block {
            SystemNoticeBlock::Comms { content, .. }
            | SystemNoticeBlock::ExternalEvent { content, .. } => {
                externalize_content_blocks(blob_store, content).await?;
            }
            _ => {}
        }
    }
    Ok(())
}

async fn hydrate_system_notice_blocks(
    blob_store: &dyn BlobStore,
    blocks: &mut [SystemNoticeBlock],
    missing_behavior: MissingBlobBehavior,
) -> Result<(), BlobStoreError> {
    for block in blocks {
        match block {
            SystemNoticeBlock::Comms { content, .. }
            | SystemNoticeBlock::ExternalEvent { content, .. } => {
                hydrate_content_blocks(blob_store, content, missing_behavior).await?;
            }
            _ => {}
        }
    }
    Ok(())
}

pub async fn externalize_content_input(
    blob_store: &dyn BlobStore,
    input: &mut ContentInput,
) -> Result<(), BlobStoreError> {
    if let ContentInput::Blocks(blocks) = input {
        externalize_content_blocks(blob_store, blocks).await?;
    }
    Ok(())
}

pub async fn hydrate_content_input(
    blob_store: &dyn BlobStore,
    input: &mut ContentInput,
    missing_behavior: MissingBlobBehavior,
) -> Result<(), BlobStoreError> {
    if let ContentInput::Blocks(blocks) = input {
        hydrate_content_blocks(blob_store, blocks, missing_behavior).await?;
    }
    Ok(())
}

pub async fn externalize_deferred_turn_state(
    blob_store: &dyn BlobStore,
    state: &mut SessionDeferredTurnState,
) -> Result<(), BlobStoreError> {
    if let Some(prompt) = state.pending_initial_prompt_mut_for_blob_rewrite() {
        externalize_content_input(blob_store, &mut prompt.prompt).await?;
    }
    for pending in state.pending_tool_results_mut_for_blob_rewrite() {
        for result in &mut pending.results {
            externalize_content_blocks(blob_store, &mut result.content).await?;
        }
    }
    Ok(())
}

pub async fn hydrate_deferred_turn_state(
    blob_store: &dyn BlobStore,
    state: &mut SessionDeferredTurnState,
    missing_behavior: MissingBlobBehavior,
) -> Result<(), BlobStoreError> {
    if let Some(prompt) = state.pending_initial_prompt_mut_for_blob_rewrite() {
        hydrate_content_input(blob_store, &mut prompt.prompt, missing_behavior).await?;
    }
    for pending in state.pending_tool_results_mut_for_blob_rewrite() {
        for result in &mut pending.results {
            hydrate_content_blocks(blob_store, &mut result.content, missing_behavior).await?;
        }
    }
    Ok(())
}

pub fn collect_blob_ids_from_blocks(blocks: &[ContentBlock]) -> BTreeSet<BlobId> {
    blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Image {
                data: ImageData::Blob { blob_id },
                ..
            } => Some(blob_id.clone()),
            _ => None,
        })
        .collect()
}

pub fn collect_blob_ids_from_messages(messages: &[Message]) -> BTreeSet<BlobId> {
    let mut ids = BTreeSet::new();
    for message in messages {
        match message {
            Message::User(user) => ids.extend(collect_blob_ids_from_blocks(&user.content)),
            Message::ToolResults { results, .. } => {
                for result in results {
                    ids.extend(collect_blob_ids_from_blocks(&result.content));
                }
            }
            Message::SystemNotice(notice) => {
                for block in &notice.blocks {
                    match block {
                        SystemNoticeBlock::Comms { content, .. }
                        | SystemNoticeBlock::ExternalEvent { content, .. } => {
                            ids.extend(collect_blob_ids_from_blocks(content));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob::{BlobPayload, BlobRef};
    use crate::types::{
        SystemNoticeDirection, SystemNoticeKind, SystemNoticeMessage, SystemNoticePeer,
    };
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestBlobStore {
        blobs: Mutex<HashMap<BlobId, BlobPayload>>,
    }

    impl TestBlobStore {
        fn with_payload(payload: BlobPayload) -> Self {
            Self {
                blobs: Mutex::new(HashMap::from([(payload.blob_id.clone(), payload)])),
            }
        }
    }

    #[async_trait]
    impl BlobStore for TestBlobStore {
        async fn put_image(&self, media_type: &str, data: &str) -> Result<BlobRef, BlobStoreError> {
            let blob_id = BlobId::new(format!("sha256:test-{}", data.len()));
            let mut blobs = self.blobs.lock().map_err(|err| {
                BlobStoreError::Internal(format!("test blob store mutex poisoned: {err}"))
            })?;
            blobs.insert(
                blob_id.clone(),
                BlobPayload {
                    blob_id: blob_id.clone(),
                    media_type: media_type.to_string(),
                    data: data.to_string(),
                },
            );
            Ok(BlobRef {
                blob_id,
                media_type: media_type.to_string(),
            })
        }

        async fn get(&self, blob_id: &BlobId) -> Result<BlobPayload, BlobStoreError> {
            self.blobs
                .lock()
                .map_err(|err| {
                    BlobStoreError::Internal(format!("test blob store mutex poisoned: {err}"))
                })?
                .get(blob_id)
                .cloned()
                .ok_or_else(|| BlobStoreError::NotFound(blob_id.clone()))
        }

        async fn delete(&self, blob_id: &BlobId) -> Result<(), BlobStoreError> {
            self.blobs
                .lock()
                .map_err(|err| {
                    BlobStoreError::Internal(format!("test blob store mutex poisoned: {err}"))
                })?
                .remove(blob_id);
            Ok(())
        }

        fn is_persistent(&self) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn hydrates_blob_refs_inside_system_notice_comms_content()
    -> Result<(), Box<dyn std::error::Error>> {
        let blob_id = BlobId::new("sha256:poster");
        let store = TestBlobStore::with_payload(BlobPayload {
            blob_id: blob_id.clone(),
            media_type: "image/png".to_string(),
            data: "iVBORw0KGgo=".to_string(),
        });
        let mut messages = vec![Message::SystemNotice(SystemNoticeMessage::with_block(
            SystemNoticeKind::Comms,
            None,
            SystemNoticeBlock::Comms {
                kind: crate::types::CommsNoticeKind::Message,
                direction: SystemNoticeDirection::Incoming,
                peer: Some(SystemNoticePeer {
                    id: crate::comms::PeerId::new(),
                    display_name: Some("operator".to_string()),
                }),
                sender_taint: None,
                request_id: None,
                intent: None,
                status: None,
                summary: None,
                payload: None,
                content: vec![ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Blob {
                        blob_id: blob_id.clone(),
                    },
                }],
            },
        ))];

        hydrate_messages_for_execution(
            &store,
            &mut messages,
            MissingBlobBehavior::HistoricalPlaceholder,
        )
        .await?;

        let Message::SystemNotice(notice) = &messages[0] else {
            return Err(std::io::Error::other("expected system notice").into());
        };
        let SystemNoticeBlock::Comms { content, .. } = &notice.blocks[0] else {
            return Err(std::io::Error::other("expected comms block").into());
        };
        assert_eq!(
            content[0],
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Inline {
                    data: "iVBORw0KGgo=".to_string(),
                },
            }
        );
        Ok(())
    }
}
