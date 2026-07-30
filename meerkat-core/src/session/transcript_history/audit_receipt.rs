//! Receipt-only durable evidence for an ordered transcript-rewrite suffix.
//!
//! The session's sealed transcript graph is the singular authority for retained
//! bodies. The event log records only the ordered occurrence identities and
//! their rolling prefix transition; it must never become a second
//! `rewrites × transcript` body store.

use serde::{Deserialize, Deserializer, Serialize};

use super::{TranscriptRewriteCommit, TranscriptRewritePrefixAccumulator};
use crate::session::TranscriptEditError;

/// Receipt for one non-empty ordered transcript-rewrite suffix.
///
/// The transition is self-verifying:
/// `start_prefix.extend(commits) == end_prefix`. Occurrence generations are
/// checked by [`TranscriptRewritePrefixAccumulator::extend`], so neither a gap
/// nor a duplicate can be hidden inside one receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptRewriteAuditReceiptBatch {
    start_prefix: TranscriptRewritePrefixAccumulator,
    commits: Vec<TranscriptRewriteCommit>,
    end_prefix: TranscriptRewritePrefixAccumulator,
}

impl TranscriptRewriteAuditReceiptBatch {
    /// Validate and construct one receipt transition.
    pub fn new(
        start_prefix: TranscriptRewritePrefixAccumulator,
        commits: Vec<TranscriptRewriteCommit>,
        end_prefix: TranscriptRewritePrefixAccumulator,
    ) -> Result<Self, TranscriptEditError> {
        if commits.is_empty() {
            return Err(TranscriptEditError::HistoryStateMalformed(
                "rewrite audit receipt batch cannot be empty".to_string(),
            ));
        }
        let mut rebuilt = start_prefix.clone();
        for commit in &commits {
            rebuilt = rebuilt
                .extend(commit)
                .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        }
        if rebuilt != end_prefix {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "rewrite audit receipt end prefix does not bind {} ordered occurrences",
                commits.len()
            )));
        }
        Ok(Self {
            start_prefix,
            commits,
            end_prefix,
        })
    }

    /// Prefix authority immediately before this batch.
    #[must_use]
    pub fn start_prefix(&self) -> &TranscriptRewritePrefixAccumulator {
        &self.start_prefix
    }

    /// Exact ordered logical occurrences.
    #[must_use]
    pub fn commits(&self) -> &[TranscriptRewriteCommit] {
        &self.commits
    }

    /// Prefix authority after the complete batch.
    #[must_use]
    pub fn end_prefix(&self) -> &TranscriptRewritePrefixAccumulator {
        &self.end_prefix
    }

    /// Consume the receipt into its exact ordered commits.
    #[must_use]
    pub fn into_commits(self) -> Vec<TranscriptRewriteCommit> {
        self.commits
    }
}

impl<'de> Deserialize<'de> for TranscriptRewriteAuditReceiptBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "snake_case")]
        struct Wire {
            start_prefix: TranscriptRewritePrefixAccumulator,
            commits: Vec<TranscriptRewriteCommit>,
            end_prefix: TranscriptRewritePrefixAccumulator,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.start_prefix, wire.commits, wire.end_prefix)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::{TranscriptRewriteReason, TranscriptRewriteSelection};
    use crate::time_compat::SystemTime;

    fn commit(generation: u64) -> TranscriptRewriteCommit {
        TranscriptRewriteCommit {
            rewrite_generation: generation,
            parent_revision: format!("parent-{generation}"),
            revision: format!("revision-{generation}"),
            selection: TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
            original_span_digest: format!("original-{generation}"),
            replacement_digest: format!("replacement-{generation}"),
            messages_before: 1,
            messages_after: 1,
            reason: TranscriptRewriteReason::new("test"),
            actor: None,
            committed_at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn generation_eighty_receipt_serializes_only_its_one_commit() {
        let start: TranscriptRewritePrefixAccumulator = serde_json::from_value(serde_json::json!({
            "occurrence_count": 79,
            "digest": format!("sha256:{}", "0".repeat(64)),
        }))
        .expect("synthetic proved prefix is canonical");
        let commit = commit(80);
        let end = start.extend(&commit).expect("generation 80 extends 79");
        let before = crate::transcript_rewrite_prefix_commit_serializations();
        let receipt = TranscriptRewriteAuditReceiptBatch::new(start, vec![commit], end)
            .expect("one-commit receipt validates");
        let after = crate::transcript_rewrite_prefix_commit_serializations();
        assert_eq!(receipt.commits().len(), 1);
        assert_eq!(
            after - before,
            1,
            "receipt construction must serialize only the supplied suffix"
        );
    }

    #[test]
    fn receipt_wire_rejects_unknown_identity_fields() {
        let start = TranscriptRewritePrefixAccumulator::empty();
        let commit = commit(1);
        let end = start.extend(&commit).expect("first commit extends empty");
        let mut wire = serde_json::to_value(
            TranscriptRewriteAuditReceiptBatch::new(start, vec![commit], end)
                .expect("receipt validates"),
        )
        .expect("receipt serializes");
        wire.as_object_mut().expect("receipt is an object").insert(
            "candidate_identity".to_string(),
            serde_json::json!("ignored"),
        );
        assert!(
            serde_json::from_value::<TranscriptRewriteAuditReceiptBatch>(wire).is_err(),
            "identity-bearing receipt wire must reject unknown fields"
        );
    }
}
