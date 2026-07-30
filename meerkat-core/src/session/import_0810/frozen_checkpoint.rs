//! Frozen verifier for the exact released 0.8.10 checkpoint representation.
//!
//! Nothing in this module is exported. Current Session state has no checkpoint
//! stamp or transcript-witness concept; these types exist only long enough to
//! prove and strip one released envelope inside the explicit importer.

use super::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const RELEASED_SCHEMA_BASE: u32 = 1;
const RELEASED_SCHEMA_RECOVERED: u32 = 2;
const RELEASED_SCHEMA_WITNESS_V3: u32 = 3;
const WITNESS_DOMAIN_V3: &str = "meerkat/transcript-history-witness/v3";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReleasedCheckpointProvenance {
    SessionCreated,
    Forked,
    IntraTurnCheckpoint,
    RunBoundaryCommit,
    TranscriptRewrite,
    RecoveryMigration,
    RecoveredRunBoundaryCommit,
    RecoveredInterruptedBoundary,
    RecoveredLegacyBoundaryCommit,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ReleasedCheckpointAnchor {
    session_id: SessionId,
    lineage_id: String,
    generation: u64,
    checkpoint_revision: u64,
    digest: String,
    provenance: ReleasedCheckpointProvenance,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ReleasedCheckpointAuthorityBase {
    Absent,
    Legacy {
        source_blob_digest: String,
        observed_generation: u64,
        observed_checkpoint_revision: u64,
    },
    Typed {
        anchor: ReleasedCheckpointAnchor,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct ReleasedCheckpointStamp {
    schema_version: u32,
    session_id: SessionId,
    lineage_id: String,
    generation: u64,
    checkpoint_revision: u64,
    authority_base: ReleasedCheckpointAuthorityBase,
    digest: String,
    provenance: ReleasedCheckpointProvenance,
}

impl ReleasedCheckpointStamp {
    fn validate_for_session(&self, session_id: &SessionId) -> Result<(), String> {
        let provenance_floor = match self.provenance {
            ReleasedCheckpointProvenance::RecoveredRunBoundaryCommit
            | ReleasedCheckpointProvenance::RecoveredInterruptedBoundary
            | ReleasedCheckpointProvenance::RecoveredLegacyBoundaryCommit => {
                RELEASED_SCHEMA_RECOVERED
            }
            _ => RELEASED_SCHEMA_BASE,
        };
        if self.schema_version != provenance_floor
            && self.schema_version != RELEASED_SCHEMA_WITNESS_V3
        {
            return Err(format!(
                "released checkpoint schema {} is invalid for {:?}",
                self.schema_version, self.provenance
            ));
        }
        if &self.session_id != session_id {
            return Err(format!(
                "released checkpoint session {} differs from envelope session {}",
                self.session_id, session_id
            ));
        }
        validate_lineage(&self.lineage_id)?;
        validate_digest(&self.digest)?;

        match &self.authority_base {
            ReleasedCheckpointAuthorityBase::Absent => {
                if self.generation != 0
                    || self.checkpoint_revision != 0
                    || !matches!(
                        self.provenance,
                        ReleasedCheckpointProvenance::SessionCreated
                            | ReleasedCheckpointProvenance::Forked
                    )
                {
                    return Err(
                        "released absent authority is legal only for a generation-zero create/fork root"
                            .to_string(),
                    );
                }
            }
            ReleasedCheckpointAuthorityBase::Legacy {
                source_blob_digest,
                observed_generation,
                observed_checkpoint_revision,
            } => {
                validate_digest(source_blob_digest)?;
                if self.lineage_id != format!("session:{session_id}")
                    || self.generation != *observed_generation
                    || self.checkpoint_revision != *observed_checkpoint_revision
                    || self.provenance != ReleasedCheckpointProvenance::RecoveryMigration
                {
                    return Err(
                        "released legacy authority does not preserve its exact observed cursor"
                            .to_string(),
                    );
                }
            }
            ReleasedCheckpointAuthorityBase::Typed { anchor } => {
                anchor.validate_for_session(session_id, &self.lineage_id)?;
                if !matches!(
                    self.provenance,
                    ReleasedCheckpointProvenance::IntraTurnCheckpoint
                        | ReleasedCheckpointProvenance::RunBoundaryCommit
                        | ReleasedCheckpointProvenance::TranscriptRewrite
                        | ReleasedCheckpointProvenance::RecoveredRunBoundaryCommit
                        | ReleasedCheckpointProvenance::RecoveredInterruptedBoundary
                        | ReleasedCheckpointProvenance::RecoveredLegacyBoundaryCommit
                ) {
                    return Err(
                        "released typed authority has invalid successor provenance".to_string()
                    );
                }
                let expected_revision = anchor
                    .checkpoint_revision
                    .checked_add(1)
                    .ok_or_else(|| "released checkpoint revision overflow".to_string())?;
                if self.generation != anchor.generation
                    || self.checkpoint_revision != expected_revision
                {
                    return Err(
                        "released typed authority is not the exact anchor successor".to_string()
                    );
                }
            }
        }
        Ok(())
    }

    fn witness_format(&self) -> u32 {
        if self.schema_version == RELEASED_SCHEMA_WITNESS_V3 {
            3
        } else {
            2
        }
    }
}

impl ReleasedCheckpointAnchor {
    fn validate_for_session(&self, session_id: &SessionId, lineage_id: &str) -> Result<(), String> {
        if &self.session_id != session_id || self.lineage_id != lineage_id {
            return Err("released checkpoint anchor identity mismatch".to_string());
        }
        validate_lineage(&self.lineage_id)?;
        validate_digest(&self.digest)?;
        if self.provenance == ReleasedCheckpointProvenance::IntraTurnCheckpoint {
            return Err(
                "released intra-turn projection cannot be a checkpoint authority base".to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ReleasedTranscriptWitness {
    format: u32,
    digest: String,
}

impl ReleasedTranscriptWitness {
    fn parse(value: &serde_json::Value) -> Result<Self, String> {
        match value {
            serde_json::Value::String(digest) => {
                validate_digest(digest)?;
                Ok(Self {
                    format: 2,
                    digest: digest.clone(),
                })
            }
            serde_json::Value::Object(fields) => {
                if fields.len() != 3 {
                    return Err(
                        "released typed transcript witness has unknown or missing fields"
                            .to_string(),
                    );
                }
                let format = fields
                    .get("witness_format")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| {
                        "released typed transcript witness lacks witness_format".to_string()
                    })?;
                if !matches!(format, 2 | 3) {
                    return Err(format!(
                        "unsupported released transcript witness format {format}"
                    ));
                }
                let revision_format = fields
                    .get("revision_digest_format")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| {
                        "released typed transcript witness lacks revision_digest_format".to_string()
                    })?;
                if revision_format != TRANSCRIPT_DIGEST_FORMAT_CURRENT {
                    return Err(format!(
                        "unsupported released transcript revision format {revision_format}"
                    ));
                }
                let digest = fields
                    .get("digest")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| "released typed transcript witness lacks digest".to_string())?;
                validate_digest(digest)?;
                Ok(Self {
                    format,
                    digest: digest.to_string(),
                })
            }
            _ => Err(
                "released transcript witness must be a digest string or typed object".to_string(),
            ),
        }
    }
}

pub(super) fn verify(
    session: &Session,
    stamped: bool,
) -> Result<Option<Arc<TranscriptHistoryState>>, String> {
    let stamp_value = session.metadata.get(RELEASED_CHECKPOINT_STAMP_KEY);
    if stamp_value.is_some() != stamped {
        return Err("released import stamp classification differs from exact metadata".to_string());
    }
    let history = session.metadata.get(SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
    if history.is_some_and(|value| value.get("format").is_some()) {
        return Err("released importer refuses a current-format transcript graph".to_string());
    }
    let carried = session
        .metadata
        .get(RELEASED_TRANSCRIPT_HISTORY_WITNESS_KEY)
        .map(ReleasedTranscriptWitness::parse)
        .transpose()?;

    let stamp = stamp_value
        .map(|value| {
            serde_json::from_value::<ReleasedCheckpointStamp>(value.clone())
                .map_err(|error| error.to_string())
        })
        .transpose()?;
    if let Some(stamp) = &stamp {
        stamp.validate_for_session(session.id())?;
    }

    match (stamp.as_ref(), history) {
        (Some(stamp), Some(history)) => {
            let witness = released_history_digest(history, stamp.witness_format())?;
            cross_check_carried(history, carried.as_ref(), stamp.witness_format(), &witness)?;
            verify_document_digest(session, Some(&witness), &stamp.digest)?;
            let normalized =
                import_released_0810_history(history.clone()).map_err(|error| error.to_string())?;
            Ok(Some(Arc::new(normalized)))
        }
        (Some(stamp), None) => {
            if carried.is_some() {
                return Err(
                    "released graph-less checkpoint carries an orphan history witness".to_string(),
                );
            }
            verify_document_digest(session, None, &stamp.digest)?;
            Ok(None)
        }
        (None, Some(history)) => {
            if let Some(carried) = &carried {
                let computed = released_history_digest(history, carried.format)?;
                if carried.digest != computed {
                    return Err(format!(
                        "released transcript witness mismatch: carried {}, computed {}",
                        carried.digest, computed
                    ));
                }
            }
            let normalized =
                import_released_0810_history(history.clone()).map_err(|error| error.to_string())?;
            Ok(Some(Arc::new(normalized)))
        }
        (None, None) => {
            if carried.is_some() {
                return Err(
                    "released unstamped document carries an orphan history witness".to_string(),
                );
            }
            Ok(None)
        }
    }
}

fn cross_check_carried(
    history: &serde_json::Value,
    carried: Option<&ReleasedTranscriptWitness>,
    stamp_format: u32,
    stamp_witness: &str,
) -> Result<(), String> {
    let Some(carried) = carried else {
        return Ok(());
    };
    let computed = if carried.format == stamp_format {
        stamp_witness.to_string()
    } else {
        released_history_digest(history, carried.format)?
    };
    if carried.digest != computed {
        return Err(format!(
            "released transcript witness mismatch: carried {}, computed {}",
            carried.digest, computed
        ));
    }
    Ok(())
}

fn verify_document_digest(
    session: &Session,
    history_digest: Option<&str>,
    expected: &str,
) -> Result<(), String> {
    let actual = released_document_digest(session, history_digest)?;
    if expected != actual {
        return Err(format!(
            "released checkpoint digest mismatch: expected {expected}, got {actual}"
        ));
    }
    Ok(())
}

fn released_document_digest(
    session: &Session,
    history_digest: Option<&str>,
) -> Result<String, String> {
    let messages = canonicalize_messages_for_digest(session.messages());
    let mut metadata = session.metadata.clone();
    metadata.remove(RELEASED_CHECKPOINT_STAMP_KEY);
    metadata.remove(RELEASED_RUNTIME_CHECKPOINT_PROVENANCE_KEY);
    metadata.remove(RELEASED_TRANSCRIPT_HISTORY_WITNESS_KEY);
    metadata.remove(SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
    metadata.remove(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
    session
        .inject_realtime_whole_blob_projection(&mut metadata)
        .map_err(|error| error.to_string())?;
    if let Some(digest) = history_digest {
        metadata.insert(
            SESSION_TRANSCRIPT_HISTORY_STATE_KEY.to_string(),
            serde_json::json!({
                "semantic_checkpoint_history_digest_v1": digest,
            }),
        );
    }
    if let Some(deferred) = metadata.get_mut(SESSION_DEFERRED_TURN_STATE_KEY) {
        *deferred = canonicalize_released_deferred_turn_value(deferred)
            .map_err(|error| error.to_string())?;
    }
    let document = serde_json::to_value(SessionSerdeRef {
        version: RELEASED_SESSION_ENVELOPE_VERSION,
        id: &session.id,
        messages: &messages,
        created_at: &session.created_at,
        updated_at: &session.updated_at,
        metadata: &metadata,
        usage: &session.usage,
    })
    .map_err(|error| error.to_string())?;
    canonical_digest(&document)
}

fn released_history_digest(history: &serde_json::Value, format: u32) -> Result<String, String> {
    match format {
        2 => {
            let canonical =
                transcript_history::graph::canonicalize_released_0810_checkpoint_history(history)
                    .map_err(|error| error.to_string())?;
            canonical_digest(&canonical)
        }
        3 => released_history_digest_v3(history),
        other => Err(format!(
            "unsupported released transcript witness format {other}"
        )),
    }
}

fn released_history_digest_v3(history: &serde_json::Value) -> Result<String, String> {
    let object = history
        .as_object()
        .ok_or_else(|| "released transcript history must be an object".to_string())?;
    let head = object
        .get("head")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "released transcript history lacks head".to_string())?;
    let commits: Vec<TranscriptRewriteCommit> = object
        .get("commits")
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let mut commits_value = serde_json::to_value(commits).map_err(|error| error.to_string())?;
    if let Some(commits) = commits_value.as_array_mut() {
        for commit in commits {
            if let Some(fields) = commit.as_object_mut() {
                fields.remove("rewrite_generation");
            }
        }
    }
    let commits_digest = canonical_digest(&commits_value)?;

    let mut revision_ids: Vec<&str> = object
        .get("revisions")
        .map(|value| {
            value
                .as_array()
                .ok_or_else(|| "released transcript revisions must be an array".to_string())?
                .iter()
                .map(|body| {
                    body.get("revision")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| "released retained revision lacks revision id".to_string())
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    revision_ids.sort_unstable();
    revision_ids.dedup();
    let revision_ids = serde_json::Value::Array(
        revision_ids
            .into_iter()
            .map(|revision| serde_json::Value::String(revision.to_string()))
            .collect(),
    );
    let retained_revisions_digest = canonical_digest(&revision_ids)?;
    canonical_digest(&serde_json::json!({
        "domain": WITNESS_DOMAIN_V3,
        "revision_digest_format": TRANSCRIPT_DIGEST_FORMAT_CURRENT,
        "head_revision": head,
        "commits_digest": commits_digest,
        "retained_revisions_digest": retained_revisions_digest,
    }))
}

fn canonical_digest(value: &serde_json::Value) -> Result<String, String> {
    let mut bytes = Vec::new();
    crate::digest_observability::write_canonical_json(value, &mut bytes)
        .map_err(|error| error.to_string())?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn canonicalize_released_deferred_turn_value(
    value: &serde_json::Value,
) -> Result<serde_json::Value, serde_json::Error> {
    let mut state: SessionDeferredTurnState = serde_json::from_value(value.clone())?;
    if let Some(prompt) = state.pending_initial_prompt_mut_for_blob_rewrite()
        && let crate::types::ContentInput::Blocks(blocks) = &mut prompt.prompt
    {
        canonicalize_digest_image_blocks(blocks);
    }
    for pending in state.pending_tool_results_mut_for_blob_rewrite() {
        for result in &mut pending.results {
            canonicalize_digest_image_blocks(&mut result.content);
        }
    }
    serde_json::to_value(state)
}

fn validate_lineage(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("released checkpoint lineage must not be empty".to_string());
    }
    Ok(())
}

fn validate_digest(value: &str) -> Result<(), String> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(format!("invalid released checkpoint digest `{value}`"));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("invalid released checkpoint digest `{value}`"));
    }
    Ok(())
}
