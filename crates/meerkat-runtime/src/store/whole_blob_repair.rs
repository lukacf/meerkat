//! Sanctioned recovery for a committed WholeBlob document that the
//! current-envelope decoder refuses because its live rows no longer preserve
//! the graph-proved audited endpoint.
//!
//! The repair never invents or drops a transcript row. It re-anchors the
//! document on its live rows: the audited transcript graph, its rewrite prefix
//! authority, and the pending compaction projection intents riding in the
//! metadata are removed so the document decodes again as a session whose
//! audit history restarts at the next rewrite. Every message the store holds
//! is preserved byte for byte, the store authority advances through the
//! ordinary compare-and-swap, and the report states exactly what was dropped.

use std::sync::Arc;

use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
use meerkat_core::types::Message;
use meerkat_core::{
    AuditedEndpointDivergence, SESSION_COMPACTION_PROJECTION_INTENTS_KEY,
    SESSION_TRANSCRIPT_HISTORY_STATE_KEY, SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY, Session,
    TranscriptHistoryState, audited_endpoint_divergence,
};

use super::{
    PreparedWholeBlobSnapshotCas, RuntimeStore, RuntimeStoreError, WholeBlobSnapshotCasOutcome,
    WholeBlobStoreAuthority,
};
use crate::identifiers::LogicalRuntimeId;

/// What the repair decided or did.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WholeBlobRepairAction {
    /// The committed document decodes; nothing to repair.
    NoRepairNeeded,
    /// Diagnose-only run: the re-anchored document was built and verified but
    /// not committed.
    WouldReanchor,
    /// The re-anchored document was committed under a new store authority.
    Reanchored,
    /// The repair refused to act; the reason names what would be lost or what
    /// could not be read.
    Refused(String),
}

/// One pending compaction projection intent the repair removed from the
/// document metadata. The store-owned outbox row is left untouched so the
/// runtime's ordinary finalization still indexes the discarded content.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DroppedCompactionIntent {
    pub parent_revision: String,
    pub revision: String,
}

/// Audit record of an explicitly accepted shorter-than-endpoint re-anchor.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AcceptedShorterLive {
    pub live_row_count: usize,
    pub endpoint_row_count: usize,
}

/// Facts about one repair attempt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WholeBlobAuditedEndpointRepairReport {
    pub runtime_id: String,
    pub session_id: String,
    pub base_store_revision: u64,
    pub base_blob_sha256: String,
    /// Why the current decoder refused the committed body, verbatim.
    pub decode_error: Option<String>,
    /// The audited-endpoint relation computed from the graph and rows read
    /// out of the refused document, when both could be read.
    pub divergence: Option<AuditedEndpointDivergence>,
    /// Errors reading the graph or the rows out of the refused document.
    pub read_errors: Vec<String>,
    pub live_row_count: usize,
    /// Rewrite edges the dropped graph carried (audit history, not rows).
    pub graph_edges_dropped: usize,
    pub dropped_intents: Vec<DroppedCompactionIntent>,
    /// Set only when the operator explicitly accepted re-anchoring on a live
    /// transcript shorter than its audited endpoint (audit record).
    pub accepted_shorter: Option<AcceptedShorterLive>,
    pub action: WholeBlobRepairAction,
    /// Store authority after an applied repair.
    pub committed_store_revision: Option<u64>,
    pub committed_blob_sha256: Option<String>,
}

fn metadata_object(
    document: &mut serde_json::Value,
) -> Result<&mut serde_json::Map<String, serde_json::Value>, String> {
    document
        .get_mut("metadata")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| "WholeBlob document has no metadata object".to_string())
}

/// Diagnose, and optionally repair, the committed WholeBlob document of one
/// runtime session whose current decode fails the audited-endpoint guard.
///
/// With `apply == false` the repaired document is built and verified but not
/// written. With `apply == true` it is committed through the store's
/// compare-and-swap against the observed authority, so a concurrent writer
/// makes the repair report a conflict instead of clobbering anything.
///
/// A `LiveShorterThanEndpoint` divergence is the one case where re-anchoring
/// on the live rows drops content the audited endpoint proves existed. Apply
/// refuses it unless `accept_shorter` is set explicitly; the report and the
/// audit record then echo both row counts.
pub async fn repair_whole_blob_audited_endpoint(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    apply: bool,
    accept_shorter: bool,
) -> Result<WholeBlobAuditedEndpointRepairReport, RuntimeStoreError> {
    let Some((bytes, authority)) = store
        .session_authority_ops()
        .load_committed_whole_blob_bytes(runtime_id)
        .await?
    else {
        return Err(RuntimeStoreError::NotFound(format!(
            "no committed WholeBlob body for runtime {}",
            runtime_id.0
        )));
    };
    let mut report = WholeBlobAuditedEndpointRepairReport {
        runtime_id: runtime_id.0.clone(),
        session_id: authority.session_id().to_string(),
        base_store_revision: authority.store_revision(),
        base_blob_sha256: authority.blob_sha256().to_string(),
        decode_error: None,
        divergence: None,
        read_errors: Vec::new(),
        live_row_count: 0,
        graph_edges_dropped: 0,
        dropped_intents: Vec::new(),
        accepted_shorter: None,
        action: WholeBlobRepairAction::NoRepairNeeded,
        committed_store_revision: None,
        committed_blob_sha256: None,
    };
    let decode_error = match Session::decode_whole_blob_document(bytes.as_ref()) {
        Ok(document) => {
            report.live_row_count = document.session().messages().len();
            return Ok(report);
        }
        Err(error) => error.to_string(),
    };
    report.decode_error = Some(decode_error);

    // Read the rows and the graph independently of the refusing decoder.
    let mut document: serde_json::Value = serde_json::from_slice(bytes.as_ref())
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
    let live_rows: Vec<Message> = match document.get("messages") {
        Some(value) => match serde_json::from_value(value.clone()) {
            Ok(rows) => rows,
            Err(error) => {
                report
                    .read_errors
                    .push(format!("live rows do not decode: {error}"));
                report.action = WholeBlobRepairAction::Refused(
                    "the live rows themselves do not decode; nothing can be preserved safely"
                        .to_string(),
                );
                return Ok(report);
            }
        },
        None => {
            report.action =
                WholeBlobRepairAction::Refused("WholeBlob document has no messages".to_string());
            return Ok(report);
        }
    };
    report.live_row_count = live_rows.len();
    let metadata = match metadata_object(&mut document) {
        Ok(metadata) => metadata,
        Err(error) => {
            report.action = WholeBlobRepairAction::Refused(error);
            return Ok(report);
        }
    };
    if let Some(graph) = metadata.get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY) {
        match serde_json::from_value::<TranscriptHistoryState>(graph.clone()) {
            Ok(state) => {
                report.graph_edges_dropped = state.commits().count();
                match audited_endpoint_divergence(&state, &live_rows) {
                    Ok(divergence) => report.divergence = divergence,
                    Err(error) => report.read_errors.push(format!(
                        "audited endpoint could not be materialized: {error}"
                    )),
                }
            }
            Err(error) => report
                .read_errors
                .push(format!("transcript graph does not decode: {error}")),
        }
    }
    if let Some(intents) = metadata.get(SESSION_COMPACTION_PROJECTION_INTENTS_KEY) {
        match serde_json::from_value::<Vec<meerkat_core::CompactionProjectionIntent>>(
            intents.clone(),
        ) {
            Ok(intents) => {
                report.dropped_intents = intents
                    .iter()
                    .map(|intent| DroppedCompactionIntent {
                        parent_revision: intent.projection.parent_revision().to_string(),
                        revision: intent.projection.revision().to_string(),
                    })
                    .collect();
            }
            Err(error) => report.read_errors.push(format!(
                "compaction projection intents do not decode: {error}"
            )),
        }
    }

    // Re-anchor on the live rows: drop the audit graph, its prefix authority
    // and the intents that require it. Nothing else in the document changes.
    metadata.remove(SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
    metadata.remove(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
    metadata.remove(SESSION_COMPACTION_PROJECTION_INTENTS_KEY);
    let repaired_bytes = serde_json::to_vec(&document)
        .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
    let repaired = match Session::decode_whole_blob_document(&repaired_bytes) {
        Ok(decoded) => decoded.into_session(),
        Err(error) => {
            report.action = WholeBlobRepairAction::Refused(format!(
                "re-anchored document still does not decode: {error}"
            ));
            return Ok(report);
        }
    };
    if repaired.messages().len() != live_rows.len() || repaired.messages() != live_rows.as_slice() {
        report.action = WholeBlobRepairAction::Refused(format!(
            "re-anchored document would change the live rows ({} vs {}); refusing",
            repaired.messages().len(),
            live_rows.len()
        ));
        return Ok(report);
    }
    if repaired.id() != authority.session_id() {
        report.action = WholeBlobRepairAction::Refused(
            "re-anchored document names a different session than the store authority".to_string(),
        );
        return Ok(report);
    }
    if !apply {
        report.action = WholeBlobRepairAction::WouldReanchor;
        return Ok(report);
    }
    if let Some(divergence) = &report.divergence
        && divergence.kind == meerkat_core::AuditedEndpointDivergenceKind::LiveShorterThanEndpoint
    {
        if !accept_shorter {
            report.action = WholeBlobRepairAction::Refused(format!(
                "live transcript ({} rows) is shorter than the audited endpoint ({} rows): re-anchoring would drop audited content; re-run with accept_shorter to confirm",
                divergence.live_row_count, divergence.endpoint_row_count
            ));
            return Ok(report);
        }
        report.accepted_shorter = Some(AcceptedShorterLive {
            live_row_count: divergence.live_row_count,
            endpoint_row_count: divergence.endpoint_row_count,
        });
    }
    let carrier = BoundSessionCommit::sealed(Arc::new(repaired)).map_err(|error| {
        RuntimeStoreError::WriteFailed(format!("failed to seal the re-anchored document: {error}"))
    })?;
    let prepared = PreparedWholeBlobSnapshotCas::prepare(authority, carrier)?;
    match store
        .session_authority_ops()
        .commit_prepared_whole_blob_snapshot_cas(runtime_id, prepared)
        .await?
    {
        WholeBlobSnapshotCasOutcome::Committed(committed) => {
            report.committed_store_revision = Some(committed.store_revision());
            report.committed_blob_sha256 = Some(committed.blob_sha256().to_string());
            report.action = WholeBlobRepairAction::Reanchored;
        }
        WholeBlobSnapshotCasOutcome::Conflict => {
            report.action = WholeBlobRepairAction::Refused(
                "store authority moved while repairing; re-run against the new revision"
                    .to_string(),
            );
        }
    }
    Ok(report)
}

/// Convenience for callers holding a concrete authority pair.
pub fn describe_authority(authority: &WholeBlobStoreAuthority) -> String {
    format!(
        "store_revision {} blob {}",
        authority.store_revision(),
        authority.blob_sha256()
    )
}
