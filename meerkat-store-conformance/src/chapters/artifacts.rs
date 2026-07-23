//! Artifact chapter: `ArtifactStore` basics with `is_persistent` honesty.
//!
//! The in-tree `MemoryArtifactStore` is a documented silent-durability
//! hazard when composed into durable bundles — `is_persistent()` honesty is
//! the seam deployment tooling keys fail-closed durability on, so this
//! chapter pins it alongside the CRUD contract.

use meerkat_core::{
    ArtifactContentHandle, ArtifactError, ArtifactId, ArtifactListFilter, ArtifactRecord,
    ArtifactType,
};

use crate::factory::ArtifactStoreFactory;
use crate::failure::{ConformanceFailure, Steps};

const CHAPTER: &str = "artifacts";

/// Artifact chapter: put/get/list/delete round-trips, NotFound honesty, and
/// `is_persistent` stability (plus reopen survival when persistent).
pub async fn artifacts(factory: &dyn ArtifactStoreFactory) -> Result<(), ConformanceFailure> {
    let steps = Steps::chapter(CHAPTER);
    let store = factory.open().await?;

    let step = "put_get_round_trip";
    let artifact_id = steps.wrap(step, ArtifactId::new("conformance-artifact-1"))?;
    let body = "conformance artifact body";
    let record = steps.wrap(
        step,
        ArtifactRecord::new(
            artifact_id.clone(),
            ArtifactType::Text,
            "conformance artifact".to_string(),
            "text/plain".to_string(),
            body.len() as u64,
            None,
            ArtifactContentHandle::Opaque {
                handle: format!("opaque:{body}"),
                media_type: "text/plain".to_string(),
            },
        ),
    )?;
    let handle = steps.wrap(step, store.put(record.clone()).await)?;
    steps.ensure(
        step,
        handle == record.handle,
        "put must return the record's handle",
    )?;
    let fetched = steps.wrap(step, store.get(&artifact_id).await)?;
    steps.ensure(
        step,
        fetched == record,
        "artifact records must round-trip exactly",
    )?;

    let step = "list_filter";
    let listed = steps.wrap(step, store.list(ArtifactListFilter::default()).await)?;
    steps.ensure(
        step,
        listed.iter().any(|entry| entry.artifact_id == artifact_id),
        "an unfiltered list must include the stored artifact",
    )?;
    let filtered = steps.wrap(
        step,
        store
            .list(ArtifactListFilter {
                session_id: Some("no-such-session".to_string()),
                ..ArtifactListFilter::default()
            })
            .await,
    )?;
    steps.ensure(
        step,
        !filtered
            .iter()
            .any(|entry| entry.artifact_id == artifact_id),
        "a non-matching owner filter must exclude the artifact",
    )?;

    // is_persistent honesty: stable across handles; persistent stores serve
    // records through reopened handles.
    //
    // Residual trust: without a process restart, a shared-state "reopen"
    // (the correct model for in-memory stores) cannot expose an
    // is_persistent() lie; the cross-handle write/delete below at least
    // proves both handles address one backing. Genuine restart survival is
    // only proven by factories that reopen the durable medium.
    let step = "is_persistent_honesty";
    let reopened = factory.open().await?;
    steps.ensure(
        step,
        reopened.is_persistent() == store.is_persistent(),
        "is_persistent must be stable across handles over the same storage",
    )?;
    if store.is_persistent() {
        let survived = steps.wrap(step, reopened.get(&artifact_id).await)?;
        steps.ensure(
            step,
            survived == record,
            "a persistent artifact store must serve records after reopen",
        )?;

        // Cross-handle identity: write through the reopened handle, read and
        // delete through the original, observe the delete via the reopened.
        let cross_id = steps.wrap(step, ArtifactId::new("conformance-artifact-cross"))?;
        let cross_body = "cross-handle artifact body";
        let cross_record = steps.wrap(
            step,
            ArtifactRecord::new(
                cross_id.clone(),
                ArtifactType::Text,
                "cross-handle artifact".to_string(),
                "text/plain".to_string(),
                cross_body.len() as u64,
                None,
                ArtifactContentHandle::Opaque {
                    handle: format!("opaque:{cross_body}"),
                    media_type: "text/plain".to_string(),
                },
            ),
        )?;
        steps.wrap(step, reopened.put(cross_record.clone()).await)?;
        let via_original = steps.wrap(step, store.get(&cross_id).await)?;
        steps.ensure(
            step,
            via_original == cross_record,
            "an artifact written through a reopened handle must be served by the original \
             handle — the reopened handle is backed by different storage",
        )?;
        steps.wrap(step, store.delete(&cross_id).await)?;
        match reopened.get(&cross_id).await {
            Err(ArtifactError::NotFound(_)) => {}
            Err(other) => {
                return Err(steps.fail(
                    step,
                    format!(
                        "a delete through the original handle must be observed as NotFound by \
                         the reopened handle, got: {other}"
                    ),
                ));
            }
            Ok(_) => {
                return Err(steps.fail(
                    step,
                    "a delete through the original handle must be observed by the reopened handle",
                ));
            }
        }
    }

    let step = "delete_and_not_found_honesty";
    steps.wrap(step, store.delete(&artifact_id).await)?;
    match store.get(&artifact_id).await {
        Err(ArtifactError::NotFound(_)) => {}
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
    steps.wrap(step, store.delete(&artifact_id).await)?;
    Ok(())
}
