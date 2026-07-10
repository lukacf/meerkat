use std::collections::BTreeSet;
use std::io::Read as _;
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(target_arch = "wasm32")]
use crate::tokio::sync::{OwnedSemaphorePermit, Semaphore};
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::blob::{BlobId, BlobStore, BlobStoreError};
use crate::realtime_transcript::RealtimeTranscriptEvent;
use crate::session::SessionDeferredTurnState;
use crate::types::{ContentBlock, ContentInput, ImageData, Message, SystemNoticeBlock};

/// Maximum decoded user-image bytes hydrated into a realtime provider
/// projection at once.
///
/// Each individual live image is already capped by
/// [`crate::live_adapter::MAX_LIVE_IMAGE_BYTES`]. Reconnect projection must
/// additionally bound the aggregate because canonical history may contain
/// many independently valid images. Two maximum-size images keeps image +
/// audio turns composable without allowing an unbounded history replay to
/// materialize every realm blob into memory at once.
pub const MAX_REALTIME_USER_IMAGE_PROJECTION_BYTES: usize =
    crate::live_adapter::MAX_LIVE_IMAGE_BYTES * 2;

/// Process-wide custody reserved while a canonical realtime history is
/// hydrated and expanded into provider seed events.
///
/// The 40 MiB decoded image ceiling can transiently exist as blob payloads,
/// hydrated base64 message strings, provider `data:` URLs, and a serialized
/// WebSocket frame. Reserving 256 MiB before hydration bounds that complete
/// amplification window instead of beginning accounting only after the live
/// adapter has already been constructed.
pub const REALTIME_OPEN_PROJECTION_MEMORY_BUDGET_BYTES: usize = 256 * 1024 * 1024;

/// Failure acquiring process-wide realtime open-projection custody.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RealtimeOpenProjectionAdmissionError {
    #[error(
        "realtime open projection memory is backpressured ({reservation_bytes} bytes requested from a {capacity_bytes}-byte process budget)"
    )]
    Backpressured {
        capacity_bytes: usize,
        reservation_bytes: usize,
    },
    #[error(
        "invalid realtime open projection admission limits: capacity={capacity_bytes}, reservation={reservation_bytes}"
    )]
    InvalidLimits {
        capacity_bytes: usize,
        reservation_bytes: usize,
    },
}

/// One owned reservation spanning hydration through provider seed ACK.
///
/// The permit is deliberately non-cloneable. A cloneable
/// [`RealtimeOpenProjectionLeaseSlot`] shares one take-once instance across
/// cloned open configs instead.
pub struct RealtimeOpenProjectionLease {
    _permit: OwnedSemaphorePermit,
    reserved_bytes: usize,
}

impl std::fmt::Debug for RealtimeOpenProjectionLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealtimeOpenProjectionLease")
            .field("reserved_bytes", &self.reserved_bytes)
            .finish_non_exhaustive()
    }
}

impl RealtimeOpenProjectionLease {
    #[must_use]
    pub fn reserved_bytes(&self) -> usize {
        self.reserved_bytes
    }
}

/// Cloneable take-once carrier for a realtime open-projection lease.
///
/// Every clone points at the same slot. The first factory invocation takes the
/// lease and holds it locally through provider seed acknowledgement. A
/// concurrent or later reuse sees an empty slot and must acquire fresh process
/// custody, so retaining the original config cannot accidentally pin or reuse
/// an old reservation.
#[derive(Clone, Default)]
pub struct RealtimeOpenProjectionLeaseSlot {
    inner: Arc<Mutex<Option<RealtimeOpenProjectionLease>>>,
}

impl std::fmt::Debug for RealtimeOpenProjectionLeaseSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealtimeOpenProjectionLeaseSlot")
            .field("available", &self.is_available())
            .finish()
    }
}

impl RealtimeOpenProjectionLeaseSlot {
    #[must_use]
    pub fn new(lease: RealtimeOpenProjectionLease) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Some(lease))),
        }
    }

    #[must_use]
    pub fn take(&self) -> Option<RealtimeOpenProjectionLease> {
        match self.inner.lock() {
            Ok(mut guard) => guard.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        }
    }

    #[must_use]
    pub fn is_available(&self) -> bool {
        match self.inner.lock() {
            Ok(guard) => guard.is_some(),
            Err(poisoned) => poisoned.into_inner().is_some(),
        }
    }
}

/// Process-wide admission owner for realtime history hydration + provider
/// seeding.
#[derive(Clone)]
pub struct RealtimeOpenProjectionAdmission {
    semaphore: Arc<Semaphore>,
    capacity_bytes: usize,
    reservation_bytes: usize,
}

impl std::fmt::Debug for RealtimeOpenProjectionAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RealtimeOpenProjectionAdmission")
            .field("capacity_bytes", &self.capacity_bytes)
            .field("reservation_bytes", &self.reservation_bytes)
            .finish_non_exhaustive()
    }
}

impl RealtimeOpenProjectionAdmission {
    /// Build an isolated admission owner. Production callers normally use
    /// [`Self::global`]; explicit owners support embedded hosts and focused
    /// concurrency tests without sharing global test state.
    pub fn new(
        capacity_bytes: usize,
        reservation_bytes: usize,
    ) -> Result<Self, RealtimeOpenProjectionAdmissionError> {
        if capacity_bytes == 0
            || reservation_bytes == 0
            || reservation_bytes > capacity_bytes
            || u32::try_from(capacity_bytes).is_err()
            || u32::try_from(reservation_bytes).is_err()
        {
            return Err(RealtimeOpenProjectionAdmissionError::InvalidLimits {
                capacity_bytes,
                reservation_bytes,
            });
        }
        Ok(Self {
            semaphore: Arc::new(Semaphore::new(capacity_bytes)),
            capacity_bytes,
            reservation_bytes,
        })
    }

    /// Canonical process-wide owner. One worst-case image-bearing open is
    /// admitted at a time; no payload-bearing waiter is queued.
    #[must_use]
    pub fn global() -> &'static Self {
        static ADMISSION: OnceLock<RealtimeOpenProjectionAdmission> = OnceLock::new();
        ADMISSION.get_or_init(|| RealtimeOpenProjectionAdmission {
            semaphore: Arc::new(Semaphore::new(REALTIME_OPEN_PROJECTION_MEMORY_BUDGET_BYTES)),
            capacity_bytes: REALTIME_OPEN_PROJECTION_MEMORY_BUDGET_BYTES,
            reservation_bytes: REALTIME_OPEN_PROJECTION_MEMORY_BUDGET_BYTES,
        })
    }

    pub fn try_acquire(
        &self,
    ) -> Result<RealtimeOpenProjectionLease, RealtimeOpenProjectionAdmissionError> {
        let permits = u32::try_from(self.reservation_bytes).map_err(|_| {
            RealtimeOpenProjectionAdmissionError::InvalidLimits {
                capacity_bytes: self.capacity_bytes,
                reservation_bytes: self.reservation_bytes,
            }
        })?;
        let permit = Arc::clone(&self.semaphore)
            .try_acquire_many_owned(permits)
            .map_err(|_| RealtimeOpenProjectionAdmissionError::Backpressured {
                capacity_bytes: self.capacity_bytes,
                reservation_bytes: self.reservation_bytes,
            })?;
        Ok(RealtimeOpenProjectionLease {
            _permit: permit,
            reserved_bytes: self.reservation_bytes,
        })
    }
}

/// Failure preparing canonical user images for a realtime provider replay.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RealtimeUserImageHydrationError {
    #[error(transparent)]
    BlobStore(#[from] BlobStoreError),
    #[error(
        "realtime user-image projection exceeds the {max_bytes}-byte cumulative decoded limit (attempted {attempted_bytes} bytes)"
    )]
    CumulativeBudgetExceeded {
        max_bytes: usize,
        attempted_bytes: usize,
    },
    #[error("realtime user image {context} contains invalid base64: {detail}")]
    InvalidBase64 { context: String, detail: String },
    #[error(
        "realtime user image blob integrity mismatch: expected {expected_blob_id}, store returned {returned_blob_id}, hydrated content resolves to {computed_blob_id}"
    )]
    BlobIdentityMismatch {
        expected_blob_id: BlobId,
        returned_blob_id: BlobId,
        computed_blob_id: BlobId,
    },
}

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

/// Externalize byte-bearing realtime user content before it enters the
/// transcript reducer.
///
/// This ordering is load-bearing for causal staging: a user image may wait in
/// `realtime_transcript_state` behind an unmaterialized predecessor. If the
/// event entered the reducer inline, persistence would serialize the full
/// base64 payload into session metadata because message-only normalization
/// cannot see staged segments.
pub async fn externalize_realtime_transcript_event(
    blob_store: &dyn BlobStore,
    event: &mut RealtimeTranscriptEvent,
) -> Result<(), BlobStoreError> {
    if let RealtimeTranscriptEvent::UserContentFinal { content, .. } = event {
        externalize_content_blocks(blob_store, content).await?;
    }
    Ok(())
}

/// Hydrate only user-message images needed by the realtime provider history
/// projector, enforcing an aggregate decoded-byte budget across every image
/// occurrence (including repeated references to the same blob).
///
/// Tool-result and system-notice images are deliberately left blob-backed:
/// realtime history projection does not send them as `input_image` parts, so
/// hydrating them would consume memory without changing provider context.
pub async fn hydrate_user_images_for_realtime_projection(
    blob_store: &dyn BlobStore,
    messages: &mut [Message],
    max_decoded_bytes: usize,
) -> Result<(), RealtimeUserImageHydrationError> {
    hydrate_user_images_for_realtime_projection_with_usage(blob_store, messages, max_decoded_bytes)
        .await
        .map(|_| ())
}

/// Hydrate realtime user images and return the full canonical decoded-byte
/// usage independently of any later provider seed selection.
pub async fn hydrate_user_images_for_realtime_projection_with_usage(
    blob_store: &dyn BlobStore,
    messages: &mut [Message],
    max_decoded_bytes: usize,
) -> Result<usize, RealtimeUserImageHydrationError> {
    struct HydratedImage {
        message_index: usize,
        block_index: usize,
        media_type: String,
        data: String,
    }

    let mut decoded_total = 0usize;
    let mut hydrated_images = Vec::new();

    for (message_index, message) in messages.iter().enumerate() {
        let Message::User(user) = message else {
            continue;
        };
        for (block_index, block) in user.content.iter().enumerate() {
            let ContentBlock::Image { media_type, data } = block else {
                continue;
            };

            let remaining_decoded_bytes = max_decoded_bytes.saturating_sub(decoded_total);
            let max_encoded_bytes = max_base64_bytes_for_decoded(remaining_decoded_bytes);
            let (decoded_len, hydrated) = match data {
                ImageData::Inline { data } => {
                    if data.len() > max_encoded_bytes {
                        return Err(cumulative_budget_error(
                            max_decoded_bytes,
                            decoded_total,
                            Some(data.len()),
                        ));
                    }
                    (
                        decoded_realtime_image_len(
                            data,
                            "inline content".to_string(),
                            decoded_total,
                            max_decoded_bytes,
                        )?,
                        None,
                    )
                }
                ImageData::Blob { blob_id } => {
                    let payload = match blob_store
                        .get_with_encoded_limit(blob_id, max_encoded_bytes)
                        .await
                    {
                        Ok(payload) => payload,
                        Err(BlobStoreError::ReadLimitExceeded {
                            actual_encoded_bytes,
                            ..
                        }) => {
                            return Err(cumulative_budget_error(
                                max_decoded_bytes,
                                decoded_total,
                                actual_encoded_bytes,
                            ));
                        }
                        Err(error) => return Err(error.into()),
                    };
                    let computed_blob_id = crate::blob::content_blob_id(media_type, &payload.data);
                    if payload.blob_id != *blob_id || computed_blob_id != *blob_id {
                        return Err(RealtimeUserImageHydrationError::BlobIdentityMismatch {
                            expected_blob_id: blob_id.clone(),
                            returned_blob_id: payload.blob_id,
                            computed_blob_id,
                        });
                    }
                    (
                        decoded_realtime_image_len(
                            &payload.data,
                            format!("blob {blob_id}"),
                            decoded_total,
                            max_decoded_bytes,
                        )?,
                        Some(payload.data),
                    )
                }
            };
            let attempted_bytes = decoded_total.checked_add(decoded_len).ok_or(
                RealtimeUserImageHydrationError::CumulativeBudgetExceeded {
                    max_bytes: max_decoded_bytes,
                    attempted_bytes: usize::MAX,
                },
            )?;
            if attempted_bytes > max_decoded_bytes {
                return Err(RealtimeUserImageHydrationError::CumulativeBudgetExceeded {
                    max_bytes: max_decoded_bytes,
                    attempted_bytes,
                });
            }
            decoded_total = attempted_bytes;

            if let Some(data) = hydrated {
                hydrated_images.push(HydratedImage {
                    message_index,
                    block_index,
                    media_type: media_type.clone(),
                    data,
                });
            }
        }
    }

    for hydrated in hydrated_images {
        let Some(Message::User(user)) = messages.get_mut(hydrated.message_index) else {
            return Err(BlobStoreError::Internal(
                "realtime image hydration plan lost its user-message target".to_string(),
            )
            .into());
        };
        let Some(block) = user.content.get_mut(hydrated.block_index) else {
            return Err(BlobStoreError::Internal(
                "realtime image hydration plan lost its content-block target".to_string(),
            )
            .into());
        };
        *block = ContentBlock::Image {
            media_type: hydrated.media_type,
            data: ImageData::Inline {
                data: hydrated.data,
            },
        };
    }
    Ok(decoded_total)
}

fn decoded_realtime_image_len(
    data: &str,
    source: String,
    decoded_before: usize,
    max_decoded_bytes: usize,
) -> Result<usize, RealtimeUserImageHydrationError> {
    let mut decoder = base64::read::DecoderReader::new(
        data.as_bytes(),
        &base64::engine::general_purpose::STANDARD,
    );
    let mut buffer = [0u8; 8 * 1024];
    let mut decoded_len = 0usize;
    loop {
        let read = decoder.read(&mut buffer).map_err(|error| {
            RealtimeUserImageHydrationError::InvalidBase64 {
                context: source.clone(),
                detail: error.to_string(),
            }
        })?;
        if read == 0 {
            return Ok(decoded_len);
        }
        decoded_len = decoded_len.checked_add(read).ok_or(
            RealtimeUserImageHydrationError::CumulativeBudgetExceeded {
                max_bytes: max_decoded_bytes,
                attempted_bytes: usize::MAX,
            },
        )?;
        let attempted_bytes = decoded_before.saturating_add(decoded_len);
        if attempted_bytes > max_decoded_bytes {
            return Err(RealtimeUserImageHydrationError::CumulativeBudgetExceeded {
                max_bytes: max_decoded_bytes,
                attempted_bytes,
            });
        }
    }
}

fn max_base64_bytes_for_decoded(decoded_bytes: usize) -> usize {
    (decoded_bytes.saturating_add(2) / 3).saturating_mul(4)
}

fn cumulative_budget_error(
    max_decoded_bytes: usize,
    decoded_before: usize,
    actual_encoded_bytes: Option<usize>,
) -> RealtimeUserImageHydrationError {
    let attempted_bytes = actual_encoded_bytes
        .map(base64_decoded_len_upper_bound)
        .and_then(|decoded| decoded_before.checked_add(decoded))
        .unwrap_or_else(|| max_decoded_bytes.saturating_add(1));
    RealtimeUserImageHydrationError::CumulativeBudgetExceeded {
        max_bytes: max_decoded_bytes,
        attempted_bytes: attempted_bytes.max(max_decoded_bytes.saturating_add(1)),
    }
}

fn base64_decoded_len_upper_bound(encoded_bytes: usize) -> usize {
    encoded_bytes
        .saturating_add(3)
        .checked_div(4)
        .unwrap_or(usize::MAX)
        .saturating_mul(3)
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
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::blob::{BlobPayload, BlobRef};
    use crate::types::{
        SystemNoticeDirection, SystemNoticeKind, SystemNoticeMessage, SystemNoticePeer,
    };
    use async_trait::async_trait;
    use base64::Engine as _;
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

    #[test]
    fn realtime_image_decoder_stops_at_the_cumulative_budget_boundary() {
        let decoded = vec![0_u8; 64 * 1024];
        let encoded = base64::engine::general_purpose::STANDARD.encode(decoded);
        let error = decoded_realtime_image_len(&encoded, "test".to_string(), 0, 1)
            .expect_err("decoder must stop once the one-byte budget is crossed");
        let RealtimeUserImageHydrationError::CumulativeBudgetExceeded {
            max_bytes,
            attempted_bytes,
        } = error
        else {
            panic!("unexpected decoder error: {error}");
        };
        assert_eq!(max_bytes, 1);
        assert!(attempted_bytes > max_bytes);
        assert!(
            attempted_bytes < 64 * 1024,
            "decoder scanned the complete payload instead of aborting: {attempted_bytes}"
        );
    }

    #[tokio::test]
    async fn realtime_projection_reports_exact_repeated_blob_budget_overflow() {
        let blob_id = crate::blob::content_blob_id("image/png", "AQID");
        let store = TestBlobStore::with_payload(BlobPayload {
            blob_id: blob_id.clone(),
            media_type: "image/png".to_string(),
            data: "AQID".to_string(),
        });
        let mut messages = (0..3)
            .map(|_| {
                Message::User(crate::types::UserMessage::with_blocks(vec![
                    ContentBlock::Image {
                        media_type: "image/png".to_string(),
                        data: ImageData::Blob {
                            blob_id: blob_id.clone(),
                        },
                    },
                ]))
            })
            .collect::<Vec<_>>();

        let error = hydrate_user_images_for_realtime_projection(&store, &mut messages, 6)
            .await
            .expect_err("three three-byte images must exceed a six-byte budget");
        assert!(matches!(
            error,
            RealtimeUserImageHydrationError::CumulativeBudgetExceeded {
                max_bytes: 6,
                attempted_bytes: 9,
            }
        ));
        assert!(messages.iter().all(|message| matches!(
            message,
            Message::User(user)
                if matches!(
                    user.content.as_slice(),
                    [ContentBlock::Image {
                        data: ImageData::Blob { blob_id: current },
                        ..
                    }] if current == &blob_id
                )
        )));
    }

    #[tokio::test]
    async fn realtime_projection_rejects_blob_bytes_that_do_not_match_durable_identity() {
        let expected_blob_id = crate::blob::content_blob_id("image/png", "AQID");
        let store = TestBlobStore::with_payload(BlobPayload {
            blob_id: expected_blob_id.clone(),
            media_type: "image/png".to_string(),
            data: "BAUG".to_string(),
        });
        let mut messages = vec![Message::User(crate::types::UserMessage::with_blocks(vec![
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: ImageData::Blob {
                    blob_id: expected_blob_id.clone(),
                },
            },
        ]))];

        let error = hydrate_user_images_for_realtime_projection(&store, &mut messages, 1024)
            .await
            .expect_err("corrupt blob bytes must not replace canonical realtime image identity");
        assert!(matches!(
            error,
            RealtimeUserImageHydrationError::BlobIdentityMismatch {
                expected_blob_id: ref expected,
                returned_blob_id: ref returned,
                ref computed_blob_id,
            } if expected == &expected_blob_id
                && returned == &expected_blob_id
                && computed_blob_id != &expected_blob_id
        ));
        assert!(matches!(
            messages[0],
            Message::User(ref user)
                if matches!(
                    user.content.as_slice(),
                    [ContentBlock::Image {
                        data: ImageData::Blob { blob_id },
                        ..
                    }] if blob_id == &expected_blob_id
                )
        ));
    }

    #[test]
    fn realtime_open_projection_slot_is_take_once_across_config_clones() {
        let admission = RealtimeOpenProjectionAdmission::new(1024, 1024)
            .expect("isolated projection admission");
        let slot = RealtimeOpenProjectionLeaseSlot::new(
            admission.try_acquire().expect("first projection lease"),
        );
        let cloned = slot.clone();

        let lease = cloned.take().expect("one clone takes shared lease");
        assert!(slot.take().is_none(), "the original sees the emptied slot");
        assert!(matches!(
            admission.try_acquire(),
            Err(RealtimeOpenProjectionAdmissionError::Backpressured { .. })
        ));

        drop(lease);
        assert!(
            admission.try_acquire().is_ok(),
            "dropping the factory-local lease releases process custody"
        );
    }

    #[test]
    fn realtime_open_projection_admission_rejects_invalid_or_saturated_limits() {
        assert!(matches!(
            RealtimeOpenProjectionAdmission::new(0, 0),
            Err(RealtimeOpenProjectionAdmissionError::InvalidLimits { .. })
        ));
        let admission = RealtimeOpenProjectionAdmission::new(2048, 1024)
            .expect("two-slot projection admission");
        let first = admission.try_acquire().expect("first lease");
        let second = admission.try_acquire().expect("second lease");
        assert!(matches!(
            admission.try_acquire(),
            Err(RealtimeOpenProjectionAdmissionError::Backpressured { .. })
        ));
        drop((first, second));
        assert!(admission.try_acquire().is_ok());
    }
}
