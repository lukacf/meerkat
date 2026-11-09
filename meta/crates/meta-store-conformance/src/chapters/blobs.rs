//! `BinaryBlobStore` profile: content addressing, persistence honesty,
//! typed dangling references, and the bundled legacy filesystem layout.

use std::path::Path;

use base64::Engine as _;
use bytes::Bytes;
use meerkat_core::BlobId;
use meerkat_core::BlobStoreError;
use meerkat_meta::blob_store::{BinaryBlobStore, ObjectStoreBlobStore};
use meerkat_store_conformance::ConformanceFailure;
use sha2::{Digest, Sha256};

use crate::factory::BinaryBlobStoreFactory;
use crate::steps::Steps;

const CHAPTER: &str = "binary_blobs";

/// Binary-blob chapter.
///
/// `expect_persistent` pins `is_persistent()` honesty from the outside: a
/// factory wired to a persistent backend passes `Some(true)` (and the chapter
/// requires reopen survival), an explicitly ephemeral backend passes
/// `Some(false)` (the memory store must never claim persistence), `None`
/// skips the pin and derives the reopen expectation from the store's own
/// claim.
pub async fn binary_blobs(
    factory: &dyn BinaryBlobStoreFactory,
    expect_persistent: Option<bool>,
) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let store = factory.open().await?;

    // --- content-address round trip -------------------------------------------
    let step = "content_address_round_trip";
    let payload = Bytes::from_static(b"meta-conformance blob payload");
    let blob_ref = steps.wrap(step, store.put_bytes("image/png", payload.clone()).await)?;
    steps.ensure(
        step,
        is_canonical_sha256(blob_ref.blob_id.as_str()),
        format!(
            "minted blob ids must be canonical sha256 form, got {}",
            blob_ref.blob_id
        ),
    )?;
    let fetched = steps.wrap(step, store.get_bytes(&blob_ref.blob_id).await)?;
    steps.ensure(
        step,
        fetched.data == payload
            && fetched.media_type == "image/png"
            && fetched.size == payload.len() as u64,
        "blob payload must round-trip byte-exact with its media type and size",
    )?;

    // --- idempotent re-put -------------------------------------------------------
    let step = "idempotent_put";
    let replayed = steps.wrap(step, store.put_bytes("image/png", payload.clone()).await)?;
    steps.ensure(
        step,
        replayed.blob_id == blob_ref.blob_id,
        "re-putting identical content must mint the identical content-addressed id",
    )?;

    // --- is_persistent honesty across reopen ---------------------------------------
    let step = "is_persistent_honesty";
    if let Some(expected) = expect_persistent {
        steps.ensure(
            step,
            store.is_persistent() == expected,
            format!(
                "is_persistent() must answer honestly: composition pinned {expected}, store \
                 claims {} — a memory store claiming persistence is exactly the silent-loss \
                 hazard this flag exists to catch",
                store.is_persistent()
            ),
        )?;
    }
    let reopened = factory.open().await?;
    steps.ensure(
        step,
        reopened.is_persistent() == store.is_persistent(),
        "is_persistent must be stable across handles over the same storage",
    )?;
    if store.is_persistent() {
        let survived = steps.wrap(step, reopened.get_bytes(&blob_ref.blob_id).await)?;
        steps.ensure(
            step,
            survived.data == payload,
            "a store claiming persistence must serve stored blobs through a reopened handle",
        )?;
    }

    // --- dangling reference is a typed NotFound --------------------------------------
    let step = "dangling_reference_not_found";
    let dangling = BlobId::new(format!(
        "sha256:{:x}",
        Sha256::digest(b"meta-conformance: content never stored")
    ));
    match store.get_bytes(&dangling).await {
        Err(BlobStoreError::NotFound(missing)) => {
            steps.ensure(
                step,
                missing == dangling,
                "NotFound must name the missing blob id",
            )?;
        }
        Err(other) => {
            return Err(steps.fail(
                step,
                format!("a dangling blob reference must surface NotFound, got: {other}"),
            ));
        }
        Ok(_) => {
            return Err(steps.fail(
                step,
                "a dangling blob reference must surface NotFound on get — never a silent \
                 success or empty payload",
            ));
        }
    }

    // --- delete honesty ------------------------------------------------------------
    let step = "delete_then_not_found";
    steps.wrap(step, store.delete(&blob_ref.blob_id).await)?;
    match store.get_bytes(&blob_ref.blob_id).await {
        Err(BlobStoreError::NotFound(_)) => {}
        Err(other) => {
            return Err(steps.fail(
                step,
                format!("get after delete must fail with NotFound, got: {other}"),
            ));
        }
        Ok(_) => {
            return Err(steps.fail(step, "get after delete must surface NotFound"));
        }
    }
    // Deletes are idempotent: deleting an absent blob is not an error.
    steps.wrap(step, store.delete(&blob_ref.blob_id).await)?;
    Ok(())
}

/// Legacy filesystem-layout read fallback, scoped to the bundled
/// `ObjectStoreBlobStore::local` (the chapter fabricates that store's legacy
/// on-disk shape: `<root>/<first-2-hex>/<sha-hex>.json` holding
/// `{ "media_type", "data": <base64> }`, addressed by the legacy id that
/// hashed the base64 TEXT rather than the raw bytes).
pub async fn legacy_blob_layout(root: &Path) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter("legacy_blob_layout");
    const STEP: &str = "legacy_layout_read_fallback";

    let media_type = "image/png";
    let base64_text = "YWJj"; // "abc"
    let mut hasher = Sha256::new();
    hasher.update(media_type.as_bytes());
    hasher.update([0]);
    hasher.update(base64_text.as_bytes());
    let key = format!("{:x}", hasher.finalize());
    let legacy_id = BlobId::new(format!("sha256:{key}"));

    let prefix = key
        .get(0..2)
        .ok_or_else(|| steps.fail(STEP, "fixture error: sha hex must have a 2-char prefix"))?;
    let legacy_dir = root.join(prefix);
    steps.wrap(STEP, std::fs::create_dir_all(&legacy_dir))?;
    steps.wrap(
        STEP,
        std::fs::write(
            legacy_dir.join(format!("{key}.json")),
            serde_json::json!({ "media_type": media_type, "data": base64_text }).to_string(),
        ),
    )?;

    let store = steps.wrap(STEP, ObjectStoreBlobStore::local(root.to_path_buf()))?;
    let payload = steps.wrap(STEP, store.get_bytes(&legacy_id).await)?;
    let decoded = steps.wrap(
        STEP,
        base64::engine::general_purpose::STANDARD.decode(base64_text),
    )?;
    steps.ensure(
        STEP,
        payload.data == decoded && payload.media_type == media_type,
        "the legacy filesystem layout must be served decoded through get_bytes",
    )?;

    // The fallback must not shadow the canonical object layout.
    const CANONICAL_STEP: &str = "canonical_layout_unshadowed";
    let canonical = steps.wrap(
        CANONICAL_STEP,
        store
            .put_bytes("image/png", Bytes::from_static(b"canonical bytes"))
            .await,
    )?;
    let served = steps.wrap(CANONICAL_STEP, store.get_bytes(&canonical.blob_id).await)?;
    steps.ensure(
        CANONICAL_STEP,
        served.data == Bytes::from_static(b"canonical bytes"),
        "canonical-layout blobs must keep serving alongside the legacy fallback",
    )?;
    Ok(())
}

fn is_canonical_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}
