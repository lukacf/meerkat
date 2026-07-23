//! Blob chapter: the `BlobStore` contract plus the dangling-reference shape.

use meerkat_core::blob::content_blob_id;
use meerkat_core::{BlobStoreError, ContentBlock, ImageData, Message, UserMessage};

use crate::factory::{BlobStoreFactory, SessionStoreFactory};
use crate::failure::{ConformanceFailure, Steps};
use crate::fixtures;

const CHAPTER: &str = "blobs";

/// Blob chapter: content-addressed round-trip, restart survival when
/// `is_persistent()`, delete/exists honesty, and bounded reads.
pub async fn blobs(factory: &dyn BlobStoreFactory) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let store = factory.open().await?;

    // Content round-trip with the canonical content-addressed identity.
    let step = "content_round_trip";
    let data = fixtures::TINY_PNG_BASE64;
    let blob_ref = steps.wrap(step, store.put_image("image/png", data).await)?;
    steps.ensure(
        step,
        blob_ref.blob_id == content_blob_id("image/png", data),
        "put_image must mint the canonical content-addressed blob id",
    )?;
    steps.ensure(
        step,
        blob_ref.blob_id.is_canonical_sha256(),
        "minted blob ids must be canonical sha256 form",
    )?;
    let payload = steps.wrap(step, store.get(&blob_ref.blob_id).await)?;
    steps.ensure(
        step,
        payload.data == data && payload.media_type == blob_ref.media_type,
        "blob payload must round-trip byte-exact",
    )?;
    steps.ensure(
        step,
        steps.wrap(step, store.exists(&blob_ref.blob_id).await)?,
        "exists must report true for a stored blob",
    )?;

    // Idempotent re-put of identical content.
    let step = "idempotent_put";
    let replayed = steps.wrap(step, store.put_image("image/png", data).await)?;
    steps.ensure(
        step,
        replayed.blob_id == blob_ref.blob_id,
        "re-putting identical content must mint the identical id",
    )?;

    // Bounded reads.
    let step = "bounded_read";
    steps.wrap(
        step,
        store
            .get_with_encoded_limit(&blob_ref.blob_id, data.len())
            .await,
    )?;
    match store
        .get_with_encoded_limit(&blob_ref.blob_id, data.len() - 1)
        .await
    {
        Err(BlobStoreError::ReadLimitExceeded { .. }) => {}
        Err(other) => {
            return Err(steps.fail(
                step,
                format!("undersized bounded read must fail with ReadLimitExceeded, got: {other}"),
            ));
        }
        Ok(_) => {
            return Err(steps.fail(step, "undersized bounded read must be rejected"));
        }
    }

    // Restart survival: `is_persistent` must be stable across handles, and a
    // persistent store must serve the blob through a reopened handle.
    //
    // Residual trust: the harness cannot restart the process, so a factory
    // whose "reopen" shares in-process state (the correct model for pure
    // in-memory backends) cannot expose an is_persistent() lie by itself.
    // The cross-handle writes below at least prove the two handles address
    // ONE backing (not a copy); genuine restart survival is only proven by
    // factories that reopen the durable medium.
    let step = "reopen_survival_if_persistent";
    let reopened = factory.open().await?;
    steps.ensure(
        step,
        reopened.is_persistent() == store.is_persistent(),
        "is_persistent must be stable across handles over the same storage",
    )?;
    if store.is_persistent() {
        let survived = steps.wrap(step, reopened.get(&blob_ref.blob_id).await)?;
        steps.ensure(
            step,
            survived.data == data,
            "a persistent blob store must serve stored blobs after reopen",
        )?;

        // Cross-handle identity: a write through the reopened handle is
        // served by the original, and a delete through the original is
        // observed by the reopened one.
        let cross = steps.wrap(
            step,
            reopened
                .put_image("image/png", fixtures::TINY_PNG_VARIANT_BASE64)
                .await,
        )?;
        let via_original = steps.wrap(step, store.get(&cross.blob_id).await)?;
        steps.ensure(
            step,
            via_original.data == fixtures::TINY_PNG_VARIANT_BASE64,
            "a blob written through a reopened handle must be served by the original handle — \
             the reopened handle is backed by different storage",
        )?;
        steps.wrap(step, store.delete(&cross.blob_id).await)?;
        steps.ensure(
            step,
            !steps.wrap(step, reopened.exists(&cross.blob_id).await)?,
            "a delete through the original handle must be observed by the reopened handle",
        )?;
    }

    // Delete/exists honesty: after delete the id is NotFound — never a
    // silent success — and delete is idempotent.
    let step = "delete_and_exists_honesty";
    steps.wrap(step, store.delete(&blob_ref.blob_id).await)?;
    steps.ensure(
        step,
        !steps.wrap(step, store.exists(&blob_ref.blob_id).await)?,
        "exists must report false after delete",
    )?;
    match store.get(&blob_ref.blob_id).await {
        Err(BlobStoreError::NotFound(_)) => {}
        Err(other) => {
            return Err(steps.fail(
                step,
                format!("get after delete must fail with NotFound, got: {other}"),
            ));
        }
        Ok(_) => {
            return Err(steps.fail(
                step,
                "get after delete must surface NotFound, never a silent success",
            ));
        }
    }
    steps.wrap(step, store.delete(&blob_ref.blob_id).await)?;
    Ok(())
}

/// The dangling-reference shape.
///
/// A persisted session referencing `ImageData::Blob { blob_id }` whose blob
/// is absent must (a) still load as a session — the reference is opaque to
/// the session store — and (b) surface `BlobStoreError::NotFound` when the
/// blob is fetched. A blob store that answers a dangling reference with a
/// silent success (or an empty payload) corrupts hydration.
pub async fn dangling_blob_reference(
    sessions: &dyn SessionStoreFactory,
    blobs: &dyn BlobStoreFactory,
) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    const STEP: &str = "dangling_blob_reference";
    let session_store = sessions.open().await?;
    let blob_store = blobs.open().await?;

    // A canonical id whose content was never stored.
    let dangling_id = content_blob_id("image/png", "bm90LXN0b3JlZC1jb25mb3JtYW5jZQ==");
    steps.ensure(
        STEP,
        !steps.wrap(STEP, blob_store.exists(&dangling_id).await)?,
        "fixture error: the fabricated blob id must not exist",
    )?;

    let mut message = UserMessage::text("turn with a blob-backed image");
    message.content.push(ContentBlock::Image {
        media_type: "image/png".to_string(),
        data: ImageData::Blob {
            blob_id: dangling_id.clone(),
        },
    });
    let mut session = fixtures::session_with_texts(&[]);
    session.push(Message::User(message));
    steps.wrap(STEP, session_store.save(&session).await)?;

    let loaded = steps
        .wrap(STEP, session_store.load(session.id()).await)?
        .ok_or_else(|| steps.fail(STEP, "a session with a blob reference must load"))?;
    let loaded_ref = loaded
        .messages()
        .iter()
        .find_map(|message| match message {
            Message::User(user) => user.content.iter().find_map(|block| block.image_blob_ref()),
            _ => None,
        })
        .ok_or_else(|| {
            steps.fail(
                STEP,
                "the persisted transcript must preserve the ImageData::Blob reference",
            )
        })?;
    steps.ensure(
        STEP,
        loaded_ref.1 == &dangling_id,
        "the persisted blob reference must round-trip exactly",
    )?;

    match blob_store.get(&dangling_id).await {
        Err(BlobStoreError::NotFound(missing)) => {
            steps.ensure(
                STEP,
                missing == dangling_id,
                "NotFound must name the missing blob id",
            )?;
        }
        Err(other) => {
            return Err(steps.fail(
                STEP,
                format!("a dangling blob reference must surface NotFound, got: {other}"),
            ));
        }
        Ok(_) => {
            return Err(steps.fail(
                STEP,
                "a dangling blob reference must surface NotFound on get — never a silent \
                 success",
            ));
        }
    }
    Ok(())
}
