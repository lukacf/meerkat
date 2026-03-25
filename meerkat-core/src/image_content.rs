use std::collections::BTreeSet;

use crate::blob::{BlobId, BlobStore, BlobStoreError};
use crate::types::{ContentBlock, ContentInput, ImageData, Message};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissingBlobBehavior {
    Error,
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
            Message::User(user) => externalize_content_blocks(blob_store, &mut user.content).await?,
            Message::ToolResults { results } => {
                for result in results.iter_mut() {
                    externalize_content_blocks(blob_store, &mut result.content).await?;
                }
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
            Message::ToolResults { results } => {
                for result in results.iter_mut() {
                    hydrate_content_blocks(blob_store, &mut result.content, missing_behavior)
                        .await?;
                }
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
            Message::ToolResults { results } => {
                for result in results {
                    ids.extend(collect_blob_ids_from_blocks(&result.content));
                }
            }
            _ => {}
        }
    }
    ids
}
