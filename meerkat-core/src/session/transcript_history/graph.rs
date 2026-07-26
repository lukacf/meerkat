//! Typed transcript revision-graph value types and their maintenance.
//!
//! Extracted verbatim from `session.rs`; the extraction commit changes
//! no behaviour, only where the code lives.

use super::decode_memo::{
    TRANSCRIPT_GRAPH_FACT_HEAL_PROBE_CURRENT, TRANSCRIPT_GRAPH_FACT_VALIDATED,
    TranscriptGraphValidationMode, memoized_validated_transcript_graph,
    record_transcript_graph_heal_probe, record_validated_transcript_graph,
    transcript_graph_heal_probe_is_memoized, transcript_graph_shape_key,
};
use super::heal::{heal_legacy_compaction_rewrite_semantics, heal_legacy_revision_strings};
use super::validate::{
    revision_body_extends_head, validate_transcript_history_state,
    validate_transcript_rewrite_record,
};
use crate::session::{
    TranscriptEditError, TranscriptRewriteReason, TranscriptRewriteSelection,
    transcript_messages_digest,
};
use crate::time_compat::SystemTime;
use crate::types::Message;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

/// Immutable rewrite commit that advances a session transcript head.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptRewriteCommit {
    pub parent_revision: String,
    pub revision: String,
    pub selection: TranscriptRewriteSelection,
    pub original_span_digest: String,
    pub replacement_digest: String,
    pub messages_before: usize,
    pub messages_after: usize,
    pub reason: TranscriptRewriteReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[cfg_attr(feature = "schema", schemars(with = "SchemaSystemTime"))]
    pub committed_at: SystemTime,
}

/// Immutable transcript revision body retained by the session-local graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptRevisionBody {
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_revision: Option<String>,
    #[cfg_attr(feature = "schema", schemars(with = "Vec<serde_json::Value>"))]
    pub messages: Vec<Message>,
    #[cfg_attr(feature = "schema", schemars(with = "SchemaSystemTime"))]
    pub created_at: SystemTime,
}

#[cfg(feature = "schema")]
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[schemars(rename = "SystemTime")]
struct SchemaSystemTime {
    secs_since_epoch: u64,
    nanos_since_epoch: u32,
}

/// Self-contained append-only transcript rewrite record.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptRewriteRecord {
    pub commit: TranscriptRewriteCommit,
    pub parent_body: TranscriptRevisionBody,
    pub revision_body: TranscriptRevisionBody,
}

impl<'de> Deserialize<'de> for TranscriptRewriteRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        struct Wire {
            commit: TranscriptRewriteCommit,
            parent_body: TranscriptRevisionBody,
            revision_body: TranscriptRevisionBody,
        }
        let wire = Wire::deserialize(deserializer)?;
        let mut revisions = vec![wire.parent_body, wire.revision_body];
        let mut commits = vec![wire.commit];
        heal_legacy_revision_strings(&mut revisions, &mut commits, None)
            .map_err(serde::de::Error::custom)?;
        heal_legacy_compaction_rewrite_semantics(&mut commits, &revisions);
        let mut revisions = revisions.into_iter();
        let parent_body = revisions
            .next()
            .ok_or_else(|| serde::de::Error::custom("rewrite record lost its parent body"))?;
        let revision_body = revisions
            .next()
            .ok_or_else(|| serde::de::Error::custom("rewrite record lost its revision body"))?;
        let commit = commits
            .into_iter()
            .next()
            .ok_or_else(|| serde::de::Error::custom("rewrite record lost its commit"))?;
        Ok(Self {
            commit,
            parent_body,
            revision_body,
        })
    }
}

impl TranscriptRewriteRecord {
    pub fn new(
        commit: TranscriptRewriteCommit,
        parent_body: TranscriptRevisionBody,
        revision_body: TranscriptRevisionBody,
    ) -> Result<Self, TranscriptEditError> {
        validate_transcript_rewrite_record(&commit, &parent_body, &revision_body)?;
        Ok(Self {
            commit,
            parent_body,
            revision_body,
        })
    }
}

/// Typed session-local transcript revision graph state.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptHistoryState {
    pub head: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<TranscriptRewriteCommit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revisions: Vec<TranscriptRevisionBody>,
    /// Digest-format generation of the revision strings. Documents stamped
    /// `>= 2` were written by the content-addressed digest format, so decode
    /// skips the per-decode legacy-heal probe (a full-transcript hash);
    /// absent/0 means unknown provenance and the probe runs once — the next
    /// save persists the marker. A compatibility convenience, not an
    /// integrity boundary (checkpoint stamps own integrity).
    #[serde(default, skip_serializing_if = "digest_format_is_unknown")]
    pub digest_format: u32,
}

fn digest_format_is_unknown(format: &u32) -> bool {
    *format == 0
}

/// The digest-format generation minted by [`transcript_messages_digest`].
pub(crate) const TRANSCRIPT_DIGEST_FORMAT_CURRENT: u32 = 2;

impl<'de> Deserialize<'de> for TranscriptHistoryState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        struct Wire {
            head: String,
            #[serde(default)]
            commits: Vec<TranscriptRewriteCommit>,
            #[serde(default)]
            revisions: Vec<TranscriptRevisionBody>,
            #[serde(default)]
            digest_format: u32,
        }
        let wire = Wire::deserialize(deserializer)?;
        let mut state = TranscriptHistoryState {
            head: wire.head,
            commits: wire.commits,
            revisions: wire.revisions,
            digest_format: wire.digest_format,
        };
        // Pre-parent-pointer v1 snapshots serialized each body as
        // {created_at,messages,revision}. When every non-root body lacks a
        // parent, the append order is the only lineage the old format
        // carried; reconstruct that exact linear order before digest healing
        // and full validation.
        if state.revisions.len() > 1
            && state
                .revisions
                .iter()
                .skip(1)
                .all(|body| body.parent_revision.is_none())
        {
            for index in 1..state.revisions.len() {
                let parent = state.revisions[index - 1].revision.clone();
                state.revisions[index].parent_revision = Some(parent);
            }
        }
        // Fast path: a graph stamped with the current digest format skips the
        // heal probe outright — the probe hashes the full head transcript,
        // which is decode-hot (every session load). Unstamped graphs (legacy
        // or pre-marker writers) pay the probe once per process per shape
        // (the bounded decode memo absorbs repeat decodes of unchanged
        // marker-less bytes); their next save persists the marker.
        let head_is_current = state.digest_format >= TRANSCRIPT_DIGEST_FORMAT_CURRENT
            || match state
                .revisions
                .iter()
                .find(|body| body.revision == state.head)
            {
                Some(head_body) => {
                    let probe_key = transcript_graph_shape_key(
                        TRANSCRIPT_GRAPH_FACT_HEAL_PROBE_CURRENT,
                        state.digest_format,
                        &state.head,
                        &state.commits,
                        &state.revisions,
                    );
                    if probe_key
                        .as_deref()
                        .is_some_and(transcript_graph_heal_probe_is_memoized)
                    {
                        true
                    } else {
                        let current = transcript_messages_digest(&head_body.messages)
                            .map_err(serde::de::Error::custom)?
                            == state.head;
                        // Only the idempotent outcome is memoizable: a
                        // stale-format head must keep healing on every
                        // decode until a save persists the healed strings.
                        if current && let Some(key) = probe_key {
                            record_transcript_graph_heal_probe(key);
                        }
                        current
                    }
                }
                None => true,
            };
        state.digest_format = TRANSCRIPT_DIGEST_FORMAT_CURRENT;
        if !head_is_current {
            let TranscriptHistoryState {
                head,
                commits,
                digest_format: _,
                revisions,
            } = &mut state;
            heal_legacy_revision_strings(revisions, commits, Some(head))
                .map_err(serde::de::Error::custom)?;
        }
        heal_legacy_compaction_rewrite_semantics(&mut state.commits, &state.revisions);
        Ok(state)
    }
}

impl TranscriptHistoryState {
    /// Drop mechanical append-head snapshots while preserving every body that
    /// is an endpoint of an audited rewrite plus the current live head.
    ///
    /// Ordinary appends previously accumulated a complete transcript body on
    /// every message mutation once any rewrite had occurred. Those bodies are
    /// not rewrite history and are never selected for restore. Repointing the
    /// live head directly at the latest rewrite endpoint keeps the existing
    /// full-body lineage validator intact after the intermediate append heads
    /// are removed.
    pub(crate) fn compact_mechanical_revision_bodies(&mut self) -> Result<(), TranscriptEditError> {
        self.compact_mechanical_revision_bodies_for(TranscriptGraphValidationMode::FullVerify)
    }

    /// [`Self::compact_mechanical_revision_bodies`] with an explicit
    /// validation trust mode. Only the durable-document decode seam passes
    /// [`TranscriptGraphValidationMode::DecodeMemoized`]; typed mutation and
    /// serialization seams keep the unconditional full validation.
    ///
    /// MERGE NOTE (class2 integration): this composes the decode memo (which
    /// absorbs repeat decodes of unchanged marker-less documents) with the
    /// extracted pruning half below (which the append fast path calls with
    /// its own O(1) validity proof, skipping validation entirely). Both
    /// mechanisms are load-bearing; neither replaces the other.
    pub(crate) fn compact_mechanical_revision_bodies_for(
        &mut self,
        mode: TranscriptGraphValidationMode,
    ) -> Result<(), TranscriptEditError> {
        let validated_key = match mode {
            TranscriptGraphValidationMode::FullVerify => None,
            TranscriptGraphValidationMode::DecodeMemoized => transcript_graph_shape_key(
                TRANSCRIPT_GRAPH_FACT_VALIDATED,
                self.digest_format,
                &self.head,
                &self.commits,
                &self.revisions,
            ),
        };
        if let Some(key) = validated_key.as_deref()
            && let Some(proved) = memoized_validated_transcript_graph(key)
        {
            // The key is over the pre-prune shape; the value is the proven
            // post-prune graph. Substituting it serves content that was
            // fully validated in this process, so a hit skips validation
            // AND pruning without ever trusting the incoming bodies.
            *self = TranscriptHistoryState::clone(&proved);
            return Ok(());
        }
        validate_transcript_history_state(self)?;
        self.prune_mechanical_revision_bodies();
        if let Some(key) = validated_key {
            record_validated_transcript_graph(key, Arc::new(self.clone()));
        }
        Ok(())
    }

    /// The pruning half of [`Self::compact_mechanical_revision_bodies`],
    /// without the full graph validation.
    ///
    /// Callable ONLY when the graph's validity is already established: pruning
    /// drops bodies, so running it over an unvalidated graph could launder a
    /// corrupt body out of sight. The append fast path in
    /// `transcript_history_state_after_message_mutation` is the one caller,
    /// and it proves the two facts that pruning needs (previously validated
    /// graph, new head extends the previous head) before calling.
    pub(crate) fn prune_mechanical_revision_bodies(&mut self) {
        let mut retained = BTreeSet::from([self.head.clone()]);
        for commit in &self.commits {
            retained.insert(commit.parent_revision.clone());
            retained.insert(commit.revision.clone());
        }

        let head_is_audited_endpoint = self
            .commits
            .iter()
            .any(|commit| commit.parent_revision == self.head || commit.revision == self.head);
        if !head_is_audited_endpoint
            && let Some(last_commit) = self
                .commits
                .last()
                .filter(|commit| commit.revision != self.head)
            && let Some(head_body) = self
                .revisions
                .iter_mut()
                .find(|body| body.revision == self.head)
        {
            head_body.parent_revision = Some(last_commit.revision.clone());
        }

        let mut seen = BTreeSet::new();
        self.revisions
            .retain(|body| retained.contains(&body.revision) && seen.insert(body.revision.clone()));

        // The full graph was validated before any pruning, so corrupt bodies
        // cannot be laundered by dropping them. The transformation changes no
        // message, revision digest, commit, or audited endpoint: it only
        // de-duplicates bodies by revision, removes non-endpoint mechanical
        // bodies, and points an unaudited live head directly at the already
        // validated latest commit. Re-hashing every retained transcript here
        // would repeat the dominant snapshot cost without adding evidence.
    }
}

impl TranscriptHistoryState {
    /// Rebuild transcript revision graph state from append-only rewrite records.
    pub fn from_rewrite_records<I>(records: I) -> Result<Option<Self>, TranscriptEditError>
    where
        I: IntoIterator<Item = TranscriptRewriteRecord>,
    {
        let mut state: Option<Self> = None;
        for record in records {
            validate_transcript_rewrite_record(
                &record.commit,
                &record.parent_body,
                &record.revision_body,
            )?;
            let state = state.get_or_insert_with(|| Self {
                head: record.commit.parent_revision.clone(),
                commits: Vec::new(),
                revisions: Vec::new(),
                digest_format: TRANSCRIPT_DIGEST_FORMAT_CURRENT,
            });
            if record.commit.parent_revision != state.head {
                if revision_body_extends_head(&record.parent_body, &state.revisions, &state.head)? {
                    state.head = record.commit.parent_revision.clone();
                } else {
                    return Err(TranscriptEditError::HistoryStateMalformed(format!(
                        "rewrite record parent {} does not extend transcript head {}",
                        record.commit.parent_revision, state.head
                    )));
                }
            }
            if !state
                .revisions
                .iter()
                .any(|body| body.revision == record.parent_body.revision)
            {
                state.revisions.push(record.parent_body);
            }
            if !state
                .revisions
                .iter()
                .any(|body| body.revision == record.revision_body.revision)
            {
                state.revisions.push(record.revision_body);
            }
            state.head = record.commit.revision.clone();
            state.commits.push(record.commit);
        }
        Ok(state)
    }
}
