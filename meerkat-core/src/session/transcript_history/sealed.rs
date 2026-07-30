//! Sealed proof-carrying transcript-history capability.

use super::graph::{
    TranscriptGraphPrefixAccumulator, TranscriptHistoryState, TranscriptRevisionBody,
    TranscriptRevisionEdge, TranscriptRewriteCommit, TranscriptRewritePrefixAccumulator,
    TranscriptRewriteRecord,
};
#[cfg(test)]
use super::validate::validate_transcript_history_state;
use crate::session::TranscriptEditError;

/// Checkpoint- or construction-proved compact transcript-history authority.
///
/// The compact state is singular: this seal never carries a revision-body
/// index or a parallel materialized graph. Construction is confined to the
/// Session module, which owns the checkpoint/content-validation marker and the
/// authorized rewrite producer.
#[derive(Clone, Debug)]
pub struct ValidatedTranscriptHistory {
    state: std::sync::Arc<TranscriptHistoryState>,
}

/// Borrowed, prefix-proved rewrite suffix.
///
/// The start prefix is supplied by the caller and matched against the
/// validator-bound per-edge accumulator before this value can exist. Consumers
/// can therefore authorize the exact pending occurrence suffix without
/// materializing any historical transcript body.
#[derive(Clone, Debug)]
pub struct ValidatedTranscriptRewriteSuffix<'a> {
    history: &'a ValidatedTranscriptHistory,
    start_index: usize,
    start_prefix: TranscriptRewritePrefixAccumulator,
    edges: Vec<std::sync::Arc<TranscriptRevisionEdge>>,
}

impl ValidatedTranscriptHistory {
    fn from_proved_state(state: std::sync::Arc<TranscriptHistoryState>) -> Self {
        Self { state }
    }

    /// Test-only structural seal. Production seals require checkpoint or
    /// authorized-construction evidence and enter through
    /// [`Self::adopt_session_validated`].
    #[cfg(test)]
    fn seal(state: std::sync::Arc<TranscriptHistoryState>) -> Result<Self, TranscriptEditError> {
        validate_transcript_history_state(&state)?;
        Ok(Self::from_proved_state(state))
    }

    #[cfg(test)]
    pub(in crate::session) fn seal_owned(
        state: TranscriptHistoryState,
    ) -> Result<Self, TranscriptEditError> {
        Self::seal(std::sync::Arc::new(state))
    }

    /// Adopt the exact graph covered by the Session's checkpoint/content seal.
    pub(in crate::session) fn adopt_session_validated(
        state: std::sync::Arc<TranscriptHistoryState>,
    ) -> Self {
        Self::from_proved_state(state)
    }

    pub(crate) fn shares_exact_state_with(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.state, &other.state)
    }

    /// Prove the exact ordered occurrence suffix after `start_prefix`.
    pub fn prove_commit_suffix_after(
        &self,
        start_prefix: &TranscriptRewritePrefixAccumulator,
    ) -> Result<ValidatedTranscriptRewriteSuffix<'_>, TranscriptEditError> {
        let start_index = usize::try_from(start_prefix.occurrence_count()).map_err(|_| {
            TranscriptEditError::HistoryStateMalformed(
                "rewrite-prefix occurrence count exceeds this platform".to_string(),
            )
        })?;
        if start_index > self.state.commit_count() {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "rewrite prefix contains {start_index} occurrences, graph contains {}",
                self.state.commit_count()
            )));
        }
        let bound = if start_index == 0 {
            TranscriptRewritePrefixAccumulator::empty()
        } else {
            self.state
                .edge(start_index - 1)
                .map(TranscriptRevisionEdge::rewrite_prefix)
                .cloned()
                .ok_or_else(|| {
                    TranscriptEditError::HistoryStateMalformed(
                        "rewrite prefix cannot address compact graph".to_string(),
                    )
                })?
        };
        if &bound != start_prefix {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "rewrite prefix does not bind the first {start_index} graph occurrences"
            )));
        }
        Ok(ValidatedTranscriptRewriteSuffix {
            history: self,
            start_index,
            start_prefix: start_prefix.clone(),
            edges: self.state.edge_suffix(start_index).ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(
                    "rewrite suffix cannot address compact graph tail".to_string(),
                )
            })?,
        })
    }

    /// Prove a suffix from its exact first occurrence.
    ///
    /// Used by missing-receipt repair: the predecessor prefix comes from the
    /// sealed preceding edge, not from caller-authored bytes.
    pub fn prove_commit_suffix_starting_with(
        &self,
        first: &TranscriptRewriteCommit,
    ) -> Result<ValidatedTranscriptRewriteSuffix<'_>, TranscriptEditError> {
        let start_index = self.exact_rewrite_commit_index(first)?;
        let start_prefix = if start_index == 0 {
            TranscriptRewritePrefixAccumulator::empty()
        } else {
            self.state
                .edge(start_index - 1)
                .map(|edge| edge.rewrite_prefix().clone())
                .ok_or_else(|| {
                    TranscriptEditError::HistoryStateMalformed(
                        "rewrite suffix lost its preceding occurrence prefix".to_string(),
                    )
                })?
        };
        Ok(ValidatedTranscriptRewriteSuffix {
            history: self,
            start_index,
            start_prefix,
            edges: self.state.edge_suffix(start_index).ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(
                    "rewrite suffix cannot address compact graph tail".to_string(),
                )
            })?,
        })
    }

    /// Explicit user-restore projection to the latest occurrence of a child
    /// revision. Parent-advance fractional states are intentionally excluded.
    pub fn project_at_revision(&self, revision: &str) -> Result<Self, TranscriptEditError> {
        let index = self
            .state
            .edges()
            .iter()
            .rposition(|edge| edge.revision() == revision)
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "revision {revision} is not an audited child endpoint"
                ))
            })?;
        self.project_at_edge_count(index + 1)
    }

    /// Explicit projection immediately after this exact rewrite occurrence.
    pub fn project_at_rewrite_commit(
        &self,
        commit: &TranscriptRewriteCommit,
    ) -> Result<Self, TranscriptEditError> {
        let index = self.exact_rewrite_commit_index(commit)?;
        self.project_at_edge_count(index + 1)
    }

    /// Explicitly materialize this occurrence's parent for user restore/audit.
    ///
    /// This does not mint a fractional `TranscriptHistoryState`.
    pub fn materialize_rewrite_parent(
        &self,
        commit: &TranscriptRewriteCommit,
    ) -> Result<TranscriptRevisionBody, TranscriptEditError> {
        let index = self.exact_rewrite_commit_index(commit)?;
        self.state.materialize_occurrence_parent(index)
    }

    /// Explicitly materialize this exact occurrence's child for user
    /// restore/audit. Unlike content-addressed lookup this remains unambiguous
    /// when semantic revision labels recur (`A -> B -> A`).
    pub fn materialize_rewrite_child(
        &self,
        commit: &TranscriptRewriteCommit,
    ) -> Result<TranscriptRevisionBody, TranscriptEditError> {
        let index = self.exact_rewrite_commit_index(commit)?;
        self.state.materialize_occurrence_child(index)
    }

    /// Explicitly materialize a historical content revision.
    pub fn materialize_revision(
        &self,
        revision: &str,
    ) -> Result<TranscriptRevisionBody, TranscriptEditError> {
        self.state.materialize_revision(revision)
    }

    fn exact_rewrite_commit_index(
        &self,
        commit: &TranscriptRewriteCommit,
    ) -> Result<usize, TranscriptEditError> {
        let index = commit
            .rewrite_generation
            .checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "rewrite occurrence generation {} cannot address this graph",
                    commit.rewrite_generation
                ))
            })?;
        let Some(bound) = self.state.commit(index) else {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "rewrite occurrence generation {} is outside the proved graph",
                commit.rewrite_generation
            )));
        };
        if bound != commit {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "rewrite occurrence generation {} does not match the proved graph commit",
                commit.rewrite_generation
            )));
        }
        Ok(index)
    }

    fn project_at_edge_count(&self, edge_count: usize) -> Result<Self, TranscriptEditError> {
        let state = self.state.proved_prefix(edge_count)?;
        Ok(Self::from_proved_state(std::sync::Arc::new(state)))
    }

    pub(in crate::session) fn into_state(self) -> TranscriptHistoryState {
        std::sync::Arc::try_unwrap(self.state).unwrap_or_else(|shared| (*shared).clone())
    }

    #[must_use]
    pub fn proves_record(&self, record: &TranscriptRewriteRecord) -> bool {
        super::graph::record_is_proved_by(Some(self), record)
    }

    #[must_use]
    pub fn final_audited_live_tail_base(&self) -> Option<usize> {
        self.state.last_commit().map(|commit| commit.messages_after)
    }

    /// Final exact rewrite occurrence proved by this seal.
    ///
    /// This inherent forwarding method is intentional: function-item call
    /// sites such as `Option::and_then(ValidatedTranscriptHistory::last_commit)`
    /// cannot use method lookup through `Deref`.
    #[must_use]
    pub fn last_commit(&self) -> Option<&TranscriptRewriteCommit> {
        self.state.last_commit()
    }

    #[must_use]
    pub fn state(&self) -> &TranscriptHistoryState {
        &self.state
    }

    #[must_use]
    pub fn shared(&self) -> std::sync::Arc<TranscriptHistoryState> {
        std::sync::Arc::clone(&self.state)
    }
}

impl ValidatedTranscriptRewriteSuffix<'_> {
    #[must_use]
    pub fn start_prefix(&self) -> &TranscriptRewritePrefixAccumulator {
        &self.start_prefix
    }

    #[must_use]
    pub fn end_prefix(&self) -> &TranscriptRewritePrefixAccumulator {
        self.history.state.rewrite_prefix()
    }

    #[must_use]
    pub fn edges(&self) -> &[std::sync::Arc<TranscriptRevisionEdge>] {
        &self.edges
    }

    pub fn commits(&self) -> impl ExactSizeIterator<Item = &TranscriptRewriteCommit> {
        self.edges().iter().map(|edge| edge.commit())
    }

    #[must_use]
    pub fn graph_prefix(&self) -> &TranscriptGraphPrefixAccumulator {
        self.history.state.graph_prefix()
    }
}

impl std::ops::Deref for ValidatedTranscriptHistory {
    type Target = TranscriptHistoryState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}
