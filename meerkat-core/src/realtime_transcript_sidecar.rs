//! Authenticated realtime-transcript component sidecar.
//!
//! [`SessionRealtimeTranscriptState`] remains the in-memory reducer projection
//! used by live-channel code and the exceptional WholeBlob envelope. A
//! HeadCanonical session persists the typed operations that produced that
//! projection instead:
//!
//! - ordinary provider observations use the existing
//!   [`RealtimeTranscriptEvent`] value unchanged;
//! - the bounded pending-user-content recovery slot has explicit stage/clear
//!   records;
//! - [`RealtimeTranscriptSidecarRecord::SnapshotV1`] is reserved for the
//!   one-time 0.8.10 activation and explicit rewrite/recovery rebases.
//!
//! Exact record bytes are folded into the shared component-event prefix as
//! they are produced. Therefore an ordinary boundary only serializes, hashes,
//! and carries events since the acknowledged predecessor.

use crate::generated::session_document::RealtimeUserContentBlobStageDisposition;
use crate::realtime_transcript::{
    PendingRealtimeUserContentBlob, RealtimeTranscriptEvent, RealtimeUserContentApplyOutcome,
};
use crate::realtime_transcript_revision::{
    self, RealtimeTranscriptApplyCommit, RealtimeTranscriptShellError,
    SessionRealtimeTranscriptState,
};
use crate::session_component_sidecar::{
    ComponentEventPrefixAuthority, PreparedComponentEventSuffix, SerializedComponentEvent,
    SessionComponentKind, VerifiedComponentEventSequence,
};
use crate::types::SessionId;
use std::sync::Arc;

/// Schema of canonical realtime component-event bytes.
pub const REALTIME_TRANSCRIPT_SIDECAR_EVENT_SCHEMA_V1: u16 = 1;

/// Exceptional operation that authorizes a full realtime projection reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RealtimeTranscriptSnapshotReasonV1 {
    /// One-time conversion of the supported 0.8.10 inline projection.
    Activation0_8_10,
    /// Explicit typed transcript rewrite.
    TranscriptRewrite,
    /// Machine-authorized recovery/proved-history projection replacement.
    RecoveryRebase,
}

/// One typed mutation of the realtime-transcript reducer projection.
///
/// The enum is intentionally closed. Adding a variant requires a new component
/// event schema; old readers must never reinterpret an unfamiliar operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RealtimeTranscriptSidecarRecord {
    /// Full, machine-validated projection used only at an explicit rebase.
    SnapshotV1 {
        reason: RealtimeTranscriptSnapshotReasonV1,
        /// Exact sequence at which this reset is placed. Replay refuses a
        /// snapshot moved to any other prefix, even if its payload is valid.
        base_event_count: u64,
        state: SessionRealtimeTranscriptState,
    },
    /// Ordinary provider observation. The domain event is the durable record;
    /// there is no parallel persistence DTO.
    EventV1 { event: RealtimeTranscriptEvent },
    /// Install the bounded image-blob recovery anchor.
    PendingUserContentBlobStagedV1 {
        pending: PendingRealtimeUserContentBlob,
    },
    /// Clear one exact invalid recovery anchor after generated authority
    /// classifies the supplied request relative to that occupied slot.
    PendingUserContentBlobClearedV1 {
        expected_pending: PendingRealtimeUserContentBlob,
        request: Option<PendingRealtimeUserContentBlob>,
    },
}

/// Failure to prepare, replay, or acknowledge the realtime component sidecar.
#[derive(Debug, thiserror::Error)]
pub enum RealtimeTranscriptSidecarError {
    #[error("realtime component event serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("realtime component reducer rejected durable event: {0}")]
    Reducer(#[from] RealtimeTranscriptShellError),
    #[error("realtime component prefix is invalid: {0}")]
    Prefix(String),
    #[error("realtime component sidecar is incoherent: {0}")]
    Incoherent(String),
}

/// Full reducer projection plus the small authenticated persistence state.
///
/// This is boxed by [`crate::Session`] so sessions that never use realtime
/// still pay one pointer. Session clones share the accumulated projection
/// through `Arc`; the HeadCanonical prepare/commit path touches only the
/// bounded pending suffix.
#[derive(Debug, Clone)]
pub(crate) struct SessionRealtimeTranscriptProjection {
    /// Shared immutable projection. Ordinary `Session` clones remain O(1);
    /// the single actor-owned writer mutates through `Arc::make_mut`.
    state: Arc<SessionRealtimeTranscriptState>,
    /// Whether WholeBlob serialization must carry the full projection.
    present: bool,
    acknowledged_prefix: ComponentEventPrefixAuthority,
    pending_events: Vec<SerializedComponentEvent>,
}

/// Fully validated and serialized exceptional projection rebase.
///
/// Preparing can fail; applying cannot. Session rewrite code prepares this
/// before mutating transcript/history authority and installs it only after
/// every other fallible step has succeeded.
pub(crate) struct PreparedRealtimeTranscriptRebase {
    state: SessionRealtimeTranscriptState,
    event: SerializedComponentEvent,
}

impl SessionRealtimeTranscriptProjection {
    pub(crate) fn empty(session_id: &SessionId) -> Self {
        Self {
            state: Arc::new(SessionRealtimeTranscriptState::default()),
            present: false,
            acknowledged_prefix: ComponentEventPrefixAuthority::empty(
                session_id.clone(),
                SessionComponentKind::Realtime,
            ),
            pending_events: Vec::new(),
        }
    }

    /// Decode the 0.8.10 inline metadata projection and park its exact
    /// SnapshotV1 activation event. Nothing is acknowledged until the
    /// HeadCanonical activation transaction commits.
    pub(crate) fn from_inline_snapshot(
        session_id: &SessionId,
        state: SessionRealtimeTranscriptState,
    ) -> Result<Self, RealtimeTranscriptSidecarError> {
        let state = realtime_transcript_revision::restore_realtime_transcript_state(state)?;
        let snapshot = RealtimeTranscriptSidecarRecord::SnapshotV1 {
            reason: RealtimeTranscriptSnapshotReasonV1::Activation0_8_10,
            base_event_count: 0,
            state: state.clone(),
        };
        let pending_event = serialize_record(&snapshot)?;
        Ok(Self {
            state: Arc::new(state),
            present: true,
            acknowledged_prefix: ComponentEventPrefixAuthority::empty(
                session_id.clone(),
                SessionComponentKind::Realtime,
            ),
            pending_events: vec![pending_event],
        })
    }

    #[must_use]
    pub(crate) fn state(&self) -> &SessionRealtimeTranscriptState {
        self.state.as_ref()
    }

    #[must_use]
    pub(crate) fn whole_blob_projection(&self) -> Option<&SessionRealtimeTranscriptState> {
        self.present.then_some(self.state.as_ref())
    }

    #[must_use]
    pub(crate) fn acknowledged_prefix(&self) -> &ComponentEventPrefixAuthority {
        &self.acknowledged_prefix
    }

    #[must_use]
    pub(crate) fn is_pristine(&self) -> bool {
        !self.present
            && self.acknowledged_prefix.event_count() == 0
            && self.pending_events.is_empty()
    }

    pub(crate) fn successor_prefix(
        &self,
    ) -> Result<ComponentEventPrefixAuthority, RealtimeTranscriptSidecarError> {
        self.acknowledged_prefix
            .extend_serialized_events(&self.pending_events)
            .map_err(|error| RealtimeTranscriptSidecarError::Prefix(error.to_string()))
    }

    pub(crate) fn prepare_suffix(
        &self,
    ) -> Result<Option<PreparedComponentEventSuffix>, RealtimeTranscriptSidecarError> {
        if self.pending_events.is_empty() {
            return Ok(None);
        }
        PreparedComponentEventSuffix::prepare(
            self.acknowledged_prefix.clone(),
            self.pending_events.clone(),
        )
        .map(Some)
        .map_err(|error| RealtimeTranscriptSidecarError::Prefix(error.to_string()))
    }

    /// Restore a HeadCanonical projection only after the store has verified
    /// exact row contiguity, canonical bytes, and the expected prefix root.
    pub(crate) fn from_verified_sequence(
        session_id: &SessionId,
        sequence: &VerifiedComponentEventSequence,
    ) -> Result<Self, RealtimeTranscriptSidecarError> {
        if sequence.session_id() != session_id
            || sequence.component() != SessionComponentKind::Realtime
            || sequence.base_seq() != 0
        {
            return Err(RealtimeTranscriptSidecarError::Incoherent(
                "verified realtime sequence has the wrong session, component, or base".to_string(),
            ));
        }
        // The proof-bearing sequence is the only durable ingress to the
        // component reducer; raw store rows cannot reach this closure.
        let state = sequence.replay(
            SessionRealtimeTranscriptState::default(),
            |state, sequence, event| {
                let record = event
                    .decode_payload::<RealtimeTranscriptSidecarRecord>(
                        REALTIME_TRANSCRIPT_SIDECAR_EVENT_SCHEMA_V1,
                    )
                    .map_err(|error| RealtimeTranscriptSidecarError::Prefix(error.to_string()))?;
                apply_record(state, sequence, record)
            },
        )?;
        let state = realtime_transcript_revision::restore_realtime_transcript_state(state)?;
        Ok(Self {
            state: Arc::new(state),
            present: sequence.successor().event_count() > 0,
            acknowledged_prefix: sequence.successor().clone(),
            pending_events: Vec::new(),
        })
    }

    /// Advance the in-memory prefix only after the exact prepared successor is
    /// acknowledged durable.
    pub(crate) fn acknowledge_suffix(
        &mut self,
        prepared: &PreparedComponentEventSuffix,
        committed: &ComponentEventPrefixAuthority,
    ) -> Result<(), RealtimeTranscriptSidecarError> {
        if self.pending_events.is_empty()
            && prepared.component() == SessionComponentKind::Realtime
            && prepared.successor() == committed
            && &self.acknowledged_prefix == committed
        {
            // Reply-loss retry after the exact successor was already adopted.
            // The successor root cryptographically binds the prepared bytes.
            return Ok(());
        }
        if prepared.component() != SessionComponentKind::Realtime
            || prepared.predecessor() != &self.acknowledged_prefix
            || prepared.events() != self.pending_events.as_slice()
            || prepared.successor() != committed
        {
            return Err(RealtimeTranscriptSidecarError::Incoherent(
                "realtime component acknowledgement does not match the parked exact suffix"
                    .to_string(),
            ));
        }
        self.acknowledged_prefix = committed.clone();
        self.pending_events.clear();
        Ok(())
    }

    /// Apply one ordinary provider event and retain its canonical bytes.
    ///
    /// The event is serialized before reducer mutation, so serialization
    /// failure cannot leave an unpersistable in-memory successor.
    pub(crate) fn apply_event(
        &mut self,
        event: RealtimeTranscriptEvent,
    ) -> Result<(RealtimeTranscriptApplyCommit, bool), RealtimeTranscriptSidecarError> {
        let record = RealtimeTranscriptSidecarRecord::EventV1 {
            event: event.clone(),
        };
        let serialized = serialize_record(&record)?;
        let commit = realtime_transcript_revision::apply_realtime_transcript_event(
            Arc::make_mut(&mut self.state),
            event,
        )?;
        let rejected = user_content_event_rejected(&commit);
        if !rejected {
            self.present = true;
            self.pending_events.push(serialized);
        }
        Ok((commit, !rejected))
    }

    /// Stage the bounded recovery anchor. Exact retries do not mint duplicate
    /// sidecar events.
    pub(crate) fn stage_pending_user_content_blob(
        &mut self,
        pending: PendingRealtimeUserContentBlob,
    ) -> Result<RealtimeUserContentBlobStageDisposition, RealtimeTranscriptSidecarError> {
        let record = RealtimeTranscriptSidecarRecord::PendingUserContentBlobStagedV1 {
            pending: pending.clone(),
        };
        let serialized = serialize_record(&record)?;
        let disposition = realtime_transcript_revision::stage_pending_realtime_user_content_blob(
            Arc::make_mut(&mut self.state),
            pending,
        )?;
        if disposition == RealtimeUserContentBlobStageDisposition::StageNew {
            self.present = true;
            self.pending_events.push(serialized);
        }
        Ok(disposition)
    }

    /// Clear the exact occupied recovery anchor. The record carries both the
    /// slot being removed and the request-relative fact the generated
    /// authority consumed, so replay cannot clear a different value.
    pub(crate) fn clear_invalid_pending_user_content_blob(
        &mut self,
        request: Option<&PendingRealtimeUserContentBlob>,
    ) -> Result<(), RealtimeTranscriptSidecarError> {
        let expected_pending =
            realtime_transcript_revision::pending_realtime_user_content_blob(self.state.as_ref())
                .ok_or_else(|| {
                RealtimeTranscriptSidecarError::Incoherent(
                    "authorized pending-blob clear observed an empty slot".to_string(),
                )
            })?;
        let record = RealtimeTranscriptSidecarRecord::PendingUserContentBlobClearedV1 {
            expected_pending,
            request: request.cloned(),
        };
        let serialized = serialize_record(&record)?;
        realtime_transcript_revision::clear_invalid_pending_realtime_user_content_blob(
            Arc::make_mut(&mut self.state),
            request,
        )?;
        self.present = true;
        self.pending_events.push(serialized);
        Ok(())
    }

    /// Replace the reducer projection after an explicit transcript rewrite or
    /// machine-authorized recovery rebase.
    pub(crate) fn prepare_rebase_snapshot(
        &self,
        state: SessionRealtimeTranscriptState,
        reason: RealtimeTranscriptSnapshotReasonV1,
    ) -> Result<PreparedRealtimeTranscriptRebase, RealtimeTranscriptSidecarError> {
        let state = realtime_transcript_revision::restore_realtime_transcript_state(state)?;
        let base_event_count = self.successor_prefix()?.event_count();
        let record = RealtimeTranscriptSidecarRecord::SnapshotV1 {
            reason,
            base_event_count,
            state: state.clone(),
        };
        let event = serialize_record(&record)?;
        Ok(PreparedRealtimeTranscriptRebase { state, event })
    }

    pub(crate) fn apply_prepared_rebase(&mut self, prepared: PreparedRealtimeTranscriptRebase) {
        self.state = Arc::new(prepared.state);
        self.present = true;
        self.pending_events.push(prepared.event);
    }
}

fn serialize_record(
    record: &RealtimeTranscriptSidecarRecord,
) -> Result<SerializedComponentEvent, RealtimeTranscriptSidecarError> {
    SerializedComponentEvent::canonical_json(REALTIME_TRANSCRIPT_SIDECAR_EVENT_SCHEMA_V1, record)
        .map_err(|error| RealtimeTranscriptSidecarError::Prefix(error.to_string()))
}

fn apply_record(
    state: &mut SessionRealtimeTranscriptState,
    sequence: u64,
    record: RealtimeTranscriptSidecarRecord,
) -> Result<(), RealtimeTranscriptSidecarError> {
    match record {
        RealtimeTranscriptSidecarRecord::SnapshotV1 {
            reason,
            base_event_count,
            state: snapshot,
        } => {
            if base_event_count != sequence {
                return Err(RealtimeTranscriptSidecarError::Incoherent(format!(
                    "realtime snapshot declares base event {base_event_count} but occupies sequence {sequence}"
                )));
            }
            if reason == RealtimeTranscriptSnapshotReasonV1::Activation0_8_10 && sequence != 0 {
                return Err(RealtimeTranscriptSidecarError::Incoherent(
                    "0.8.10 realtime activation snapshot must be the first component event"
                        .to_string(),
                ));
            }
            *state = realtime_transcript_revision::restore_realtime_transcript_state(snapshot)?;
        }
        RealtimeTranscriptSidecarRecord::EventV1 { event } => {
            let commit =
                realtime_transcript_revision::apply_realtime_transcript_event(state, event)?;
            if user_content_event_rejected(&commit) {
                return Err(RealtimeTranscriptSidecarError::Incoherent(
                    "durable realtime event is one the producer reducer rejects".to_string(),
                ));
            }
        }
        RealtimeTranscriptSidecarRecord::PendingUserContentBlobStagedV1 { pending } => {
            let disposition =
                realtime_transcript_revision::stage_pending_realtime_user_content_blob(
                    state, pending,
                )?;
            if disposition != RealtimeUserContentBlobStageDisposition::StageNew {
                return Err(RealtimeTranscriptSidecarError::Incoherent(
                    "durable pending-blob stage does not create a new exact slot".to_string(),
                ));
            }
        }
        RealtimeTranscriptSidecarRecord::PendingUserContentBlobClearedV1 {
            expected_pending,
            request,
        } => {
            if realtime_transcript_revision::pending_realtime_user_content_blob(state)
                != Some(expected_pending)
            {
                return Err(RealtimeTranscriptSidecarError::Incoherent(
                    "durable pending-blob clear does not name the occupied exact slot".to_string(),
                ));
            }
            realtime_transcript_revision::clear_invalid_pending_realtime_user_content_blob(
                state,
                request.as_ref(),
            )?;
        }
    }
    Ok(())
}

fn user_content_event_rejected(commit: &RealtimeTranscriptApplyCommit) -> bool {
    matches!(
        commit.outcome.user_content.as_ref(),
        Some(
            RealtimeUserContentApplyOutcome::RejectedInvalidIdentity { .. }
                | RealtimeUserContentApplyOutcome::RejectedUnmaterializedPredecessor { .. }
                | RealtimeUserContentApplyOutcome::RejectedConflict { .. }
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::realtime_transcript::RealtimeTranscriptRole;
    use crate::session_component_sidecar::{
        StoredComponentEventRow, VerifiedComponentEventSequence,
    };

    fn acknowledge_pending(projection: &mut SessionRealtimeTranscriptProjection) {
        let prepared = projection
            .prepare_suffix()
            .expect("prepare")
            .expect("non-empty suffix");
        let successor = prepared.successor().clone();
        projection
            .acknowledge_suffix(&prepared, &successor)
            .expect("acknowledge exact suffix");
    }

    fn verified_sequence(
        prefix: ComponentEventPrefixAuthority,
        events: &[SerializedComponentEvent],
    ) -> VerifiedComponentEventSequence {
        let rows = events
            .iter()
            .enumerate()
            .map(|(offset, event)| {
                StoredComponentEventRow::new(offset as u64, event.bytes().to_vec())
            })
            .collect();
        VerifiedComponentEventSequence::verify_full(prefix, rows).expect("verify full")
    }

    fn pending_blob(key: &str, item_id: &str) -> PendingRealtimeUserContentBlob {
        let media_type = "image/png".to_string();
        PendingRealtimeUserContentBlob {
            idempotency_key: key.to_string(),
            item_id: item_id.to_string(),
            previous_item_id: None,
            content_index: 0,
            blob_id: crate::blob::content_blob_id(&media_type, "sidecar-test-image"),
            media_type,
        }
    }

    #[test]
    fn ordinary_event_suffix_contains_only_new_typed_event() {
        let session_id = SessionId::new();
        let mut projection = SessionRealtimeTranscriptProjection::from_inline_snapshot(
            &session_id,
            SessionRealtimeTranscriptState::default(),
        )
        .expect("activation snapshot");
        acknowledge_pending(&mut projection);

        let _ = projection
            .apply_event(RealtimeTranscriptEvent::ItemObserved {
                item_id: "item-1".to_string(),
                previous_item_id: None,
                role: RealtimeTranscriptRole::User,
                response_id: None,
            })
            .expect("apply ordinary event");
        let suffix = projection
            .prepare_suffix()
            .expect("prepare")
            .expect("one event");
        assert_eq!(suffix.base_seq(), 1);
        assert_eq!(suffix.events().len(), 1);
        assert!(matches!(
            suffix.events()[0]
                .decode_payload::<RealtimeTranscriptSidecarRecord>(
                    REALTIME_TRANSCRIPT_SIDECAR_EVENT_SCHEMA_V1
                )
                .expect("decode"),
            RealtimeTranscriptSidecarRecord::EventV1 {
                event: RealtimeTranscriptEvent::ItemObserved { .. }
            }
        ));
    }

    #[test]
    fn verified_exact_rows_rebuild_projection_and_prefix() {
        let session_id = SessionId::new();
        let mut producer = SessionRealtimeTranscriptProjection::from_inline_snapshot(
            &session_id,
            SessionRealtimeTranscriptState::default(),
        )
        .expect("activation snapshot");
        let _ = producer
            .apply_event(RealtimeTranscriptEvent::ItemObserved {
                item_id: "item-1".to_string(),
                previous_item_id: None,
                role: RealtimeTranscriptRole::User,
                response_id: None,
            })
            .expect("event");
        let prepared = producer.prepare_suffix().expect("prepare").expect("suffix");
        let verified = verified_sequence(prepared.successor().clone(), prepared.events());
        let restored =
            SessionRealtimeTranscriptProjection::from_verified_sequence(&session_id, &verified)
                .expect("restore");
        assert_eq!(
            restored.acknowledged_prefix(),
            prepared.successor(),
            "materialization must retain the exact proved root"
        );
        assert!(
            restored.prepare_suffix().expect("prepare").is_none(),
            "materialized durable rows leave no pending suffix"
        );
    }

    #[test]
    fn exact_pending_blob_retry_does_not_append_duplicate_event() {
        let session_id = SessionId::new();
        let mut projection = SessionRealtimeTranscriptProjection::empty(&session_id);
        let pending = pending_blob("image-key", "item-image");
        assert_eq!(
            projection
                .stage_pending_user_content_blob(pending.clone())
                .expect("stage"),
            RealtimeUserContentBlobStageDisposition::StageNew
        );
        assert_eq!(
            projection
                .stage_pending_user_content_blob(pending)
                .expect("reuse"),
            RealtimeUserContentBlobStageDisposition::ReuseExact
        );
        let suffix = projection
            .prepare_suffix()
            .expect("prepare")
            .expect("stage event");
        assert_eq!(suffix.events().len(), 1);
    }

    #[test]
    fn exact_acknowledgement_retry_is_idempotent_but_different_retry_fails() {
        let session_id = SessionId::new();
        let mut projection = SessionRealtimeTranscriptProjection::empty(&session_id);
        let _ = projection
            .apply_event(RealtimeTranscriptEvent::ItemObserved {
                item_id: "item-1".to_string(),
                previous_item_id: None,
                role: RealtimeTranscriptRole::User,
                response_id: None,
            })
            .expect("event");
        let prepared = projection
            .prepare_suffix()
            .expect("prepare")
            .expect("suffix");
        let committed = prepared.successor().clone();
        projection
            .acknowledge_suffix(&prepared, &committed)
            .expect("first acknowledgement");
        projection
            .acknowledge_suffix(&prepared, &committed)
            .expect("reply-loss retry of exact acknowledgement");

        let different =
            ComponentEventPrefixAuthority::empty(session_id, SessionComponentKind::Realtime);
        assert!(
            projection
                .acknowledge_suffix(&prepared, &different)
                .is_err(),
            "a different committed successor must not be laundered as a retry"
        );
    }

    #[test]
    fn activation_snapshot_moved_from_sequence_zero_is_rejected() {
        let session_id = SessionId::new();
        let activation = SessionRealtimeTranscriptProjection::from_inline_snapshot(
            &session_id,
            SessionRealtimeTranscriptState::default(),
        )
        .expect("activation");
        let activation_event = activation
            .prepare_suffix()
            .expect("prepare activation")
            .expect("activation suffix")
            .events()[0]
            .clone();

        let mut ordinary = SessionRealtimeTranscriptProjection::empty(&session_id);
        let _ = ordinary
            .apply_event(RealtimeTranscriptEvent::ItemObserved {
                item_id: "item-1".to_string(),
                previous_item_id: None,
                role: RealtimeTranscriptRole::User,
                response_id: None,
            })
            .expect("ordinary event");
        let ordinary_event = ordinary
            .prepare_suffix()
            .expect("prepare ordinary")
            .expect("ordinary suffix")
            .events()[0]
            .clone();
        let moved = PreparedComponentEventSuffix::prepare(
            ComponentEventPrefixAuthority::empty(
                session_id.clone(),
                SessionComponentKind::Realtime,
            ),
            vec![ordinary_event, activation_event],
        )
        .expect("syntactically valid moved sequence");
        let verified = verified_sequence(moved.successor().clone(), moved.events());
        assert!(
            SessionRealtimeTranscriptProjection::from_verified_sequence(&session_id, &verified)
                .is_err(),
            "replay must reject an activation reset moved behind an ordinary event"
        );
    }
}
