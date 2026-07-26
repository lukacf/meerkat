//! Sealed proof-carrying transcript-history capability.
//!
//! Extracted verbatim from `session.rs`; the extraction commit changes
//! no behaviour, only where the code lives.

use super::graph::TranscriptHistoryState;
use super::validate::validate_transcript_history_state;
use crate::session::TranscriptEditError;

/// A transcript revision graph carrying the proof that
/// [`validate_transcript_history_state`] holds for it.
///
/// Save guards parse one session's graph and then hand it to several
/// consumers, each of which used to re-run the whole-graph validator — a
/// canonicalize-and-hash pass over every retained body, O(document), once per
/// consumer — even when the caller had already established exactly that fact
/// one line earlier. Memoizing the validator does not fix this: a key cheap
/// enough to compute per call cannot bind the message bodies it claims to have
/// verified, so every attempt either re-keys on each turn (the graph
/// legitimately changes) or trusts something weaker than the bytes.
///
/// This type carries the proof on the value it proves. There is no key to
/// collide and nothing to substitute between the check and the use, so a
/// consumer that demands one is asking for evidence rather than for a promise.
/// Construction is confined to this module: [`Self::seal`] verifies, and the
/// private adopt path lifts a `Session`'s own validation marker over the graph
/// parsed from the very metadata value that marker describes.
#[derive(Clone, Debug)]
pub struct ValidatedTranscriptHistory(std::sync::Arc<TranscriptHistoryState>);

impl ValidatedTranscriptHistory {
    /// The verifying ingress: prove `state`, then seal it.
    pub fn seal(
        state: std::sync::Arc<TranscriptHistoryState>,
    ) -> Result<Self, TranscriptEditError> {
        validate_transcript_history_state(&state)?;
        Ok(Self(state))
    }

    /// [`Self::seal`] for a caller holding an owned parse.
    pub fn seal_owned(state: TranscriptHistoryState) -> Result<Self, TranscriptEditError> {
        Self::seal(std::sync::Arc::new(state))
    }

    /// Adopt a graph already covered by a `Session`'s `Validated` marker.
    ///
    /// Sound only when `state` was parsed from the metadata value that marker
    /// describes, which is why it is private to the module owning both the
    /// marker and the parse cache that keeps them in step.
    pub(in crate::session) fn adopt_session_validated(
        state: std::sync::Arc<TranscriptHistoryState>,
    ) -> Self {
        Self(state)
    }

    /// Adopt the output of the snapshot-compaction seam.
    ///
    /// Sound only for the exact graph value
    /// `compact_mechanical_revision_bodies_for` just returned from: the
    /// FullVerify mode ran [`validate_transcript_history_state`] on it, the
    /// DecodeMemoized hit substituted a graph that was fully validated before
    /// memo admission, and pruning preserves validity by construction. Private
    /// to the session module so no other caller can launder an unproven graph
    /// through this ingress.
    pub(in crate::session) fn adopt_compacted_snapshot(
        state: std::sync::Arc<TranscriptHistoryState>,
    ) -> Self {
        Self(state)
    }

    /// The proved graph.
    #[must_use]
    pub fn state(&self) -> &TranscriptHistoryState {
        &self.0
    }

    /// The proved graph as a shared handle.
    #[must_use]
    pub fn shared(&self) -> std::sync::Arc<TranscriptHistoryState> {
        std::sync::Arc::clone(&self.0)
    }
}

impl std::ops::Deref for ValidatedTranscriptHistory {
    type Target = TranscriptHistoryState;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
