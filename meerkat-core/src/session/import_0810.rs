//! Explicit, one-time importer for the exact Meerkat 0.8.10 Session envelope.
//!
//! Released checkpoint and transcript-witness formats are deliberately absent
//! from ordinary [`Session`] deserialization. This module is the sole boundary
//! allowed to interpret them. A successful import proves the frozen released
//! evidence at most once, strips every legacy proof carrier, and returns a
//! domain Session plus a non-cloneable receipt that a store must consume while
//! atomically adopting the imported state under its own physical authority.

use super::*;
use crate::types::SystemMessage;
use std::collections::{BTreeMap, BTreeSet};

mod frozen_checkpoint;

const RELEASED_SESSION_ENVELOPE_VERSION: u32 = 2;
const RELEASED_CHECKPOINT_STAMP_KEY: &str = "session_checkpoint_stamp_v1";
const RELEASED_RUNTIME_CHECKPOINT_PROVENANCE_KEY: &str = "session_runtime_checkpoint_provenance_v1";
const RELEASED_TRANSCRIPT_HISTORY_WITNESS_KEY: &str =
    "session_transcript_history_checkpoint_digest_v1";
const RELEASED_SYSTEM_CONTEXT_STATE_KEY: &str = "session_system_context_state";
const RELEASED_SYSTEM_CONTEXT_RENDER_LABEL: &str = "[Runtime System Context]";

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct FrozenSystemContextState0810 {
    #[serde(default)]
    pending: Vec<FrozenSystemContextAppend0810>,
    #[serde(default)]
    applied: Vec<FrozenSystemContextAppend0810>,
    #[serde(default)]
    seen: BTreeMap<String, FrozenSeenSystemContextKey0810>,
    #[serde(default)]
    active_turn_pending_keys: BTreeSet<String>,
    #[serde(default)]
    active_turn_pending_indices: BTreeSet<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct FrozenSystemContextAppend0810 {
    content: crate::lifecycle::run_primitive::CoreRenderable,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    source_kind: FrozenSystemContextSource0810,
    #[serde(default)]
    peer_response_terminal: Option<crate::handles::PeerResponseTerminalFact>,
    accepted_at: SystemTime,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct FrozenSeenSystemContextKey0810 {
    content: crate::lifecycle::run_primitive::CoreRenderable,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    source_kind: FrozenSystemContextSource0810,
    #[serde(default)]
    peer_response_terminal: Option<crate::handles::PeerResponseTerminalFact>,
    state: FrozenSeenSystemContextState0810,
}

#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FrozenSystemContextSource0810 {
    #[default]
    Normal,
    RuntimeSteer,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FrozenSeenSystemContextState0810 {
    Pending,
    Applied,
}

pub(crate) fn is_released_checkpoint_metadata_key(key: &str) -> bool {
    matches!(
        key,
        RELEASED_CHECKPOINT_STAMP_KEY
            | RELEASED_RUNTIME_CHECKPOINT_PROVENANCE_KEY
            | RELEASED_TRANSCRIPT_HISTORY_WITNESS_KEY
    )
}

pub(super) fn contains_released_checkpoint_metadata(
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    metadata
        .keys()
        .any(|key| is_released_checkpoint_metadata_key(key))
}

/// Why the exact released document may be adopted.
///
/// `FrozenCheckpointVerified` is self-contained evidence from a stamped
/// 0.8.10 document. `StoreAuthorizationRequired` deliberately is not:
/// unstamped 0.8.10 rows may be imported only when the backend consumes the
/// receipt in the same operation that proves the released physical store
/// schema, the exact source row/blob identity, and installs a store-issued
/// current authority. Envelope bytes alone cannot distinguish a graph-less
/// unstamped 0.8.10 Session from current graph-less domain state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Released0810ImportEvidence {
    FrozenCheckpointVerified,
    StoreAuthorizationRequired,
}

/// Single-use evidence for one exact 0.8.10 import.
///
/// The type is intentionally not `Clone`. Reading its fields is sufficient to
/// compare it with a backend-issued source authority, but adoption must take
/// ownership of the receipt so no second store transition can reuse it.
#[derive(Debug)]
#[must_use = "released import receipts must be consumed by one store-adoption transaction"]
pub struct Released0810ImportReceipt {
    session_id: SessionId,
    source_document_sha256: [u8; 32],
    evidence: Released0810ImportEvidence,
}

impl Released0810ImportReceipt {
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub const fn source_document_sha256(&self) -> &[u8; 32] {
        &self.source_document_sha256
    }

    #[must_use]
    pub const fn evidence(&self) -> Released0810ImportEvidence {
        self.evidence
    }
}

/// Domain state and the one receipt authorizing its store adoption.
#[derive(Debug)]
#[must_use = "released imported state must be adopted through its single-use receipt"]
pub struct ImportedReleased0810Session {
    session: Session,
    receipt: Released0810ImportReceipt,
}

impl ImportedReleased0810Session {
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }

    #[must_use]
    pub fn receipt(&self) -> &Released0810ImportReceipt {
        &self.receipt
    }

    /// Split the imported domain state from the single-use adoption receipt.
    #[must_use]
    pub fn into_parts(self) -> (Session, Released0810ImportReceipt) {
        (self.session, self.receipt)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Released0810ImportError {
    #[error("released 0.8.10 session document is malformed: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error(
        "released importer accepts exact session envelope version {expected}, observed {observed}"
    )]
    EnvelopeVersion { expected: u32, observed: u32 },
    #[error("released importer refuses current transcript-history wire")]
    CurrentTranscriptHistory,
    #[error("released importer refuses current-only metadata `{0}`")]
    CurrentMetadata(&'static str),
    #[error("released checkpoint verification failed: {0}")]
    Checkpoint(String),
    #[error("released transcript-history import failed: {0}")]
    TranscriptHistory(String),
}

/// Import one exact Meerkat 0.8.10 Session document.
///
/// This is intentionally not part of [`Deserialize`] or
/// [`Session::from_persisted_bytes`]. Current loaders never inspect a legacy
/// schema: backend activation first classifies an exact released physical row,
/// invokes this importer once, and consumes the returned receipt while
/// replacing that row with store-owned current authority.
pub fn import_released_0810_session(
    serialized: &[u8],
) -> Result<ImportedReleased0810Session, Released0810ImportError> {
    let source_document_sha256 = sha256_key(serialized);
    let mut deserializer = serde_json::Deserializer::from_slice(serialized);
    let serde_repr = SessionSerde::deserialize(&mut deserializer)?;
    deserializer.end()?;
    if serde_repr.version != RELEASED_SESSION_ENVELOPE_VERSION {
        return Err(Released0810ImportError::EnvelopeVersion {
            expected: RELEASED_SESSION_ENVELOPE_VERSION,
            observed: serde_repr.version,
        });
    }

    let mut session = released_session_from_serde(serde_repr)?;
    let history_kind = transcript_history_wire_kind(&session.metadata)
        .map_err(|error| Released0810ImportError::TranscriptHistory(error.to_string()))?;
    if matches!(history_kind, Some(TranscriptHistoryWireKind::Current)) {
        return Err(Released0810ImportError::CurrentTranscriptHistory);
    }
    if session
        .metadata
        .contains_key(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY)
    {
        return Err(Released0810ImportError::CurrentMetadata(
            SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY,
        ));
    }

    let stamped = session.metadata.contains_key(RELEASED_CHECKPOINT_STAMP_KEY);
    let imported_history = frozen_checkpoint::verify(&session, stamped)
        .map_err(Released0810ImportError::Checkpoint)?;

    if let Some(history) = imported_history {
        install_imported_history(&mut session, history)?;
    }
    adopt_released_system_context_into_transcript(&mut session)?;
    strip_released_checkpoint_metadata(&mut session);
    session.version = SESSION_VERSION;

    let evidence = if stamped {
        Released0810ImportEvidence::FrozenCheckpointVerified
    } else {
        Released0810ImportEvidence::StoreAuthorizationRequired
    };
    let session_id = session.id().clone();
    Ok(ImportedReleased0810Session {
        session,
        receipt: Released0810ImportReceipt {
            session_id,
            source_document_sha256,
            evidence,
        },
    })
}

/// Retire the 0.8.10 out-of-band prompt projection while the frozen envelope
/// is already verified but before current store authority is installed.
///
/// The released shape did not retain an original transcript position for
/// applied prompt context. Its only honest ordered conversion point is this
/// adoption boundary. Every entry remains distinct and ordered; no System
/// content is coalesced, replaced, or normalized.
fn adopt_released_system_context_into_transcript(
    session: &mut Session,
) -> Result<(), Released0810ImportError> {
    let Some(encoded) = session.metadata.remove(RELEASED_SYSTEM_CONTEXT_STATE_KEY) else {
        return Ok(());
    };
    let state: FrozenSystemContextState0810 = serde_json::from_value(encoded)?;
    validate_released_system_context_state_0810(&state)?;

    // The released state partition is itself the ordering witness: every
    // applied entry crossed an earlier model boundary, while `pending`
    // contains only entries accepted after the last such boundary. Each
    // partition preserves admission order. Concatenating applied then pending
    // therefore reconstructs the only ordering the 0.8.10 representation
    // retained, without sorting by a wall clock that may move backwards.
    let appends = state.applied.into_iter().chain(state.pending);
    for append in appends {
        // A runtime steer is owned by its pending RuntimeStore input and is
        // re-armed from that input on recovery. The Session copy was never
        // durable conversation data.
        if append.source_kind == FrozenSystemContextSource0810::RuntimeSteer {
            continue;
        }

        if append.peer_response_terminal.is_some() {
            let mut notice = append.content.into_system_notice_message();
            notice.created_at = append.accepted_at.into();
            let already_present = session.messages().iter().any(|message| {
                matches!(
                    message,
                    Message::SystemNotice(existing)
                        if existing.kind == notice.kind
                            && existing.body == notice.body
                            && existing.blocks == notice.blocks
                )
            });
            if !already_present {
                session.push(Message::SystemNotice(notice));
            }
            continue;
        }

        if matches!(
            &append.content,
            crate::lifecycle::run_primitive::CoreRenderable::SystemNotice { .. }
        ) {
            let mut notice = append.content.into_system_notice_message();
            notice.created_at = append.accepted_at.into();
            session.push(Message::SystemNotice(notice));
            continue;
        }

        let rendered =
            render_released_system_context_block_0810(&append.content, append.source.as_deref());
        // Import never applies NEW ingress idempotency semantics to already
        // accepted 0.8.10 facts. Every stored entry becomes one distinct
        // ordinary System row, even when content or identity repeats.
        session.push(Message::System(SystemMessage::with_identity_at(
            rendered,
            append.source,
            append.idempotency_key,
            append.accepted_at.into(),
        )));
    }

    // These were auxiliary indices for the retired sidecar. Deserializing
    // them above makes unknown released shapes fail closed; successful
    // adoption intentionally carries none of them into current Session state.
    let _ = (
        state.seen,
        state.active_turn_pending_keys,
        state.active_turn_pending_indices,
    );
    Ok(())
}

fn validate_released_system_context_state_0810(
    state: &FrozenSystemContextState0810,
) -> Result<(), Released0810ImportError> {
    let invalid = |message: String| {
        Released0810ImportError::Malformed(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            message,
        )))
    };
    let mut keyed: BTreeMap<
        String,
        (
            FrozenSeenSystemContextState0810,
            &FrozenSystemContextAppend0810,
        ),
    > = BTreeMap::new();
    for (lifecycle, appends) in [
        (
            FrozenSeenSystemContextState0810::Applied,
            state.applied.as_slice(),
        ),
        (
            FrozenSeenSystemContextState0810::Pending,
            state.pending.as_slice(),
        ),
    ] {
        for append in appends {
            let Some(key) = append.idempotency_key.as_ref() else {
                continue;
            };
            if keyed.insert(key.clone(), (lifecycle, append)).is_some() {
                return Err(invalid(format!(
                    "released 0.8.10 system-context state repeats idempotency key `{key}`"
                )));
            }
        }
    }
    if keyed.len() != state.seen.len() {
        return Err(invalid(
            "released 0.8.10 system-context seen index does not cover the exact keyed entries"
                .to_string(),
        ));
    }
    for (key, seen) in &state.seen {
        let Some((lifecycle, append)) = keyed.get(key) else {
            return Err(invalid(format!(
                "released 0.8.10 system-context seen key `{key}` has no stored append"
            )));
        };
        if *lifecycle != seen.state
            || append.content != seen.content
            || append.source != seen.source
            || append.source_kind != seen.source_kind
            || append.peer_response_terminal != seen.peer_response_terminal
        {
            return Err(invalid(format!(
                "released 0.8.10 system-context seen key `{key}` contradicts its stored append"
            )));
        }
    }
    if state
        .active_turn_pending_indices
        .iter()
        .any(|index| usize::try_from(*index).map_or(true, |index| index >= state.pending.len()))
    {
        return Err(invalid(
            "released 0.8.10 active-turn pending index is out of bounds".to_string(),
        ));
    }
    for key in &state.active_turn_pending_keys {
        let Some((FrozenSeenSystemContextState0810::Pending, _)) = keyed.get(key) else {
            return Err(invalid(format!(
                "released 0.8.10 active-turn key `{key}` is not pending"
            )));
        };
    }
    Ok(())
}

fn render_released_system_context_block_0810(
    content: &crate::lifecycle::run_primitive::CoreRenderable,
    source: Option<&str>,
) -> String {
    let mut rendered = String::from(RELEASED_SYSTEM_CONTEXT_RENDER_LABEL);
    if let Some(source) = source {
        rendered.push_str("\nsource: ");
        rendered.push_str(source);
    }
    rendered.push_str("\n\n");
    rendered.push_str(content.render_text().trim());
    rendered
}

fn released_session_from_serde(
    serde_repr: SessionSerde,
) -> Result<Session, Released0810ImportError> {
    let mut metadata = serde_repr.metadata;
    let realtime_transcript = match metadata.remove(SESSION_REALTIME_TRANSCRIPT_STATE_KEY) {
        Some(value) => {
            let state = serde_json::from_value(value)?;
            SessionRealtimeTranscriptProjection::from_inline_snapshot(&serde_repr.id, state)
                .map_err(|error| {
                    Released0810ImportError::Malformed(serde_json::Error::io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        error.to_string(),
                    )))
                })?
        }
        None => SessionRealtimeTranscriptProjection::empty(&serde_repr.id),
    };
    Ok(Session {
        version: RELEASED_SESSION_ENVELOPE_VERSION,
        id: serde_repr.id,
        messages: TranscriptMessages::from_vec(serde_repr.messages),
        created_at: serde_repr.created_at,
        updated_at: serde_repr.updated_at,
        metadata,
        realtime_transcript: Box::new(realtime_transcript),
        history_caches: Box::default(),
        transcript_history_metadata_validation:
            TranscriptHistoryMetadataValidation::RequiresValidation,
        usage: serde_repr.usage,
    })
}

fn install_imported_history(
    session: &mut Session,
    history: Arc<TranscriptHistoryState>,
) -> Result<(), Released0810ImportError> {
    let exact_live_prefix = history
        .derive_live_row_lineage_after_final_semantic_replay(session.messages())
        .map_err(|error| Released0810ImportError::TranscriptHistory(error.to_string()))?
        .ok_or_else(|| {
            Released0810ImportError::TranscriptHistory(
                "live transcript does not preserve the released audited endpoint".to_string(),
            )
        })?;
    let endpoint_prefix = history
        .final_endpoint_witness()
        .ok_or_else(|| {
            Released0810ImportError::TranscriptHistory(
                "imported graph has no final endpoint witness".to_string(),
            )
        })?
        .row_prefix()
        .clone();
    if !session.install_exact_message_row_lineage(endpoint_prefix, exact_live_prefix) {
        return Err(Released0810ImportError::TranscriptHistory(
            "failed to install imported message-row lineage".to_string(),
        ));
    }
    session
        .metadata
        .remove(SESSION_TRANSCRIPT_HISTORY_STATE_KEY);
    session
        .metadata
        .remove(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
    session
        .history_caches
        .shared_state
        .set(Arc::clone(&history));
    session.transcript_history_metadata_validation = TranscriptHistoryMetadataValidation::Validated;
    Ok(())
}

fn strip_released_checkpoint_metadata(session: &mut Session) {
    session.metadata.remove(RELEASED_CHECKPOINT_STAMP_KEY);
    session
        .metadata
        .remove(RELEASED_RUNTIME_CHECKPOINT_PROVENANCE_KEY);
    session
        .metadata
        .remove(RELEASED_TRANSCRIPT_HISTORY_WITNESS_KEY);
    session
        .metadata
        .remove(SESSION_TRANSCRIPT_REWRITE_PREFIX_AUTHORITY_KEY);
    if session.history_caches.shared_state.get().is_none() {
        session.transcript_history_metadata_validation =
            TranscriptHistoryMetadataValidation::Validated;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frozen_append(text: &str, source: Option<&str>, source_kind: &str) -> serde_json::Value {
        serde_json::json!({
            "content": {
                "type": "text",
                "text": text,
            },
            "source": source,
            "source_kind": source_kind,
            "accepted_at": SystemTime::now(),
        })
    }

    #[test]
    fn released_sidecar_adoption_preserves_existing_and_distinct_system_rows() {
        let mut session = Session::new();
        session.append_system_message("existing");
        session.set_metadata_unchecked_for_test(
            RELEASED_SYSTEM_CONTEXT_STATE_KEY,
            serde_json::json!({
                "applied": [
                    frozen_append("  duplicate  ", Some("first"), "normal"),
                    frozen_append("  duplicate  ", Some("first"), "normal"),
                ],
                "pending": [
                    frozen_append("   ", None, "normal"),
                    frozen_append("never durable", Some("steer"), "runtime_steer"),
                ],
            }),
        );

        adopt_released_system_context_into_transcript(&mut session)
            .expect("frozen sidecar should adopt");

        let systems = session
            .messages()
            .iter()
            .filter_map(|message| match message {
                Message::System(system) => Some(system),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(systems.len(), 4);
        assert_eq!(systems[0].content, "existing");
        assert_eq!(
            systems[1].content,
            "[Runtime System Context]\nsource: first\n\nduplicate"
        );
        assert_eq!(systems[2].content, systems[1].content);
        assert_eq!(systems[3].content, "[Runtime System Context]\n\n");
        assert_eq!(
            systems[1]
                .identity
                .as_ref()
                .and_then(|identity| identity.source.as_deref()),
            Some("first")
        );
        assert!(
            !session
                .metadata()
                .contains_key(RELEASED_SYSTEM_CONTEXT_STATE_KEY)
        );
    }

    #[test]
    fn released_keyed_sidecar_adoption_embeds_exact_cross_store_identity() {
        let accepted_at = serde_json::to_value(SystemTime::now()).expect("time");
        let content = serde_json::json!({
            "type": "text",
            "text": "  shared instruction  ",
        });
        let mut session = Session::new();
        session.set_metadata_unchecked_for_test(
            RELEASED_SYSTEM_CONTEXT_STATE_KEY,
            serde_json::json!({
                "applied": [{
                    "content": content.clone(),
                    "source": "shared-key",
                    "idempotency_key": "shared-key",
                    "accepted_at": accepted_at,
                }],
                "seen": {
                    "shared-key": {
                        "content": content,
                        "source": "shared-key",
                        "state": "applied",
                    },
                },
            }),
        );

        adopt_released_system_context_into_transcript(&mut session)
            .expect("keyed frozen sidecar should adopt");

        let Message::System(system) = &session.messages()[0] else {
            panic!("adopted row must be System");
        };
        assert_eq!(
            system.content,
            "[Runtime System Context]\nsource: shared-key\n\nshared instruction"
        );
        assert_eq!(
            system.identity.as_ref(),
            Some(&crate::types::SystemMessageIdentity {
                source: Some("shared-key".to_string()),
                idempotency_key: Some("shared-key".to_string()),
            })
        );
    }
}
