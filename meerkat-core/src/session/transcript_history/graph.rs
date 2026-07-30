//! Typed transcript revision-graph value types and their maintenance.
//!
//! Extracted verbatim from `session.rs`; the extraction commit changes
//! no behaviour, only where the code lives.

use super::heal::{heal_legacy_compaction_rewrite_semantics, heal_legacy_revision_strings};
use super::sealed::ValidatedTranscriptHistory;
use super::validate::{validate_transcript_history_state, validate_transcript_rewrite_record};
use crate::session::{
    TranscriptEditError, TranscriptRewriteReason, TranscriptRewriteSelection,
    transcript_messages_digest,
};
use crate::session_store::SessionMessageRowPrefixAccumulator;
use crate::time_compat::SystemTime;
use crate::types::Message;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::sync::Arc;

static TRANSCRIPT_REWRITE_PREFIX_COMMIT_SERIALIZATIONS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Number of exact commit serializations performed by rolling-prefix
/// construction. A receipt-only replay authorization must not advance it.
#[must_use]
pub fn transcript_rewrite_prefix_commit_serializations() -> u64 {
    TRANSCRIPT_REWRITE_PREFIX_COMMIT_SERIALIZATIONS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Immutable rewrite commit that advances a session transcript head.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TranscriptRewriteCommit {
    /// Serialized occurrence identity within one session's rewrite lineage.
    ///
    /// Revision strings identify content, not occurrences: a legitimate
    /// rewrite can return to earlier content, and two occurrences can even
    /// carry byte-identical facts under a fixed/coarse clock. Current writers
    /// therefore mint a strict contiguous `1..=n` generation. `0` exists only
    /// as the decode marker for 0.8.10 audit rows/graphs, which are normalized
    /// as an ordered set before they can enter current authority.
    #[serde(default, skip_serializing_if = "rewrite_generation_is_unknown")]
    pub rewrite_generation: u64,
    /// Content-addressed audit label for the occurrence parent.
    ///
    /// Structural continuity is owned by the occurrence generation, compact
    /// parent advance, exact row lineage, and graph prefix. This semantic
    /// label is re-proved when the targeted parent is materialized; ordinary
    /// append/commit validation never re-hashes every historical body.
    pub parent_revision: String,
    /// Content-addressed audit label for the occurrence result, with the same
    /// lazy semantic-replay contract as [`Self::parent_revision`].
    pub revision: String,
    pub selection: TranscriptRewriteSelection,
    /// Audit label for the removed semantic span. Exact selection coordinates
    /// and row lineage are structural authority; this digest is checked on
    /// targeted/final semantic materialization.
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

/// Exact-byte relationship from one audited rewrite endpoint to the next
/// commit's parent.
///
/// This evidence is aligned one-for-one with [`TranscriptHistoryState::commits`]
/// and is validated against the retained bodies before a
/// [`ValidatedTranscriptHistory`] may expose it. Current writers emit only
/// `ExactAppend`; `ExactSplice` preserves a frozen same-cardinality
/// relationship decoded by the explicit 0.8.10 importer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum TranscriptRewriteParentTransition {
    ExactAppend,
    ExactSplice,
}

fn rewrite_generation_is_unknown(generation: &u64) -> bool {
    *generation == 0
}

/// Normalize one checkpoint-bound graph commit vector to current serialized
/// occurrence identities.
///
/// A 0.8.10 graph's commit vector is itself the proved semantic order,
/// including supported cycles such as `A -> B -> A`; assigning `1..=n` from
/// that vector preserves rather than rediscovers its meaning. Current vectors
/// must already carry the same strict contiguous sequence. Mixed zero/current
/// vectors have no graph-writer provenance and refuse fail-closed.
///
/// Returns `true` only when a 0.8.10 all-zero vector was normalized.
fn normalize_legacy_graph_rewrite_generations(
    commits: &mut [TranscriptRewriteCommit],
) -> Result<bool, TranscriptEditError> {
    if commits.is_empty() {
        return Ok(false);
    }
    let zero_count = commits
        .iter()
        .filter(|commit| commit.rewrite_generation == 0)
        .count();
    if zero_count == commits.len() {
        for (index, commit) in commits.iter_mut().enumerate() {
            commit.rewrite_generation = u64::try_from(index)
                .ok()
                .and_then(|index| index.checked_add(1))
                .ok_or_else(|| {
                    TranscriptEditError::HistoryStateMalformed(
                        "transcript rewrite generation exceeds u64".to_string(),
                    )
                })?;
        }
        return Ok(true);
    }
    if zero_count != 0 {
        return Err(TranscriptEditError::HistoryStateMalformed(
            "transcript rewrite generations mix 0.8.10 zero markers with current occurrence identities"
                .to_string(),
        ));
    }
    for (index, commit) in commits.iter().enumerate() {
        let expected = u64::try_from(index)
            .ok()
            .and_then(|index| index.checked_add(1))
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(
                    "transcript rewrite generation exceeds u64".to_string(),
                )
            })?;
        if commit.rewrite_generation != expected {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "transcript rewrite generation {} is not the expected contiguous occurrence {expected}",
                commit.rewrite_generation
            )));
        }
    }
    Ok(false)
}

const TRANSCRIPT_REWRITE_PREFIX_CHAIN_DOMAIN: &[u8] =
    b"meerkat.transcript-rewrite-prefix.chain.v1\0";
const TRANSCRIPT_REWRITE_PREFIX_STEP_DOMAIN: &[u8] = b"meerkat.transcript-rewrite-prefix.step.v1\0";

// Domain policy: v1 is frozen over canonical `TranscriptRewriteCommit` JSON,
// including `rewrite_generation`, plus the previous raw digest and big-endian
// payload length below. Any change to occurrence semantics, commit
// canonicalization, or step framing requires a new chain+step domain and a
// coordinated EventStore sidecar schema bump; never reinterpret v1 bytes.

/// Rolling, canonical identity of an ordered exact rewrite-commit prefix.
///
/// This is a semantic graph fact, not a replay-cursor assertion. It is carried
/// by the graph, folded into checkpoint authority, and independently matched
/// against the EventStore's receipt. One ordinary lineage-tail commit extends
/// the accumulator with one commit serialization; it never re-hashes the
/// accumulated prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptRewritePrefixAccumulator {
    occurrence_count: u64,
    digest: String,
}

impl<'de> Deserialize<'de> for TranscriptRewritePrefixAccumulator {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case", deny_unknown_fields)]
        struct Wire {
            occurrence_count: u64,
            digest: String,
        }
        let wire = Wire::deserialize(deserializer)?;
        let accumulator = Self {
            occurrence_count: wire.occurrence_count,
            digest: wire.digest,
        };
        if accumulator.raw_digest().is_none() {
            return Err(serde::de::Error::custom(
                "rewrite-prefix digest must be canonical sha256:<64 lowercase hex>",
            ));
        }
        Ok(accumulator)
    }
}

impl TranscriptRewritePrefixAccumulator {
    #[must_use]
    pub fn empty() -> Self {
        let mut hasher = Sha256::new();
        hasher.update(TRANSCRIPT_REWRITE_PREFIX_CHAIN_DOMAIN);
        Self {
            occurrence_count: 0,
            digest: format!("sha256:{:x}", hasher.finalize()),
        }
    }

    pub fn from_commits(commits: &[TranscriptRewriteCommit]) -> Result<Self, serde_json::Error> {
        let mut accumulator = Self::empty();
        for commit in commits {
            accumulator = accumulator.extend(commit)?;
        }
        Ok(accumulator)
    }

    pub fn extend(&self, commit: &TranscriptRewriteCommit) -> Result<Self, serde_json::Error> {
        let expected_generation = self.occurrence_count.checked_add(1).ok_or_else(|| {
            serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "rewrite-prefix occurrence count exceeds u64",
            ))
        })?;
        if commit.rewrite_generation != expected_generation {
            return Err(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "rewrite-prefix occurrence generation {} is not the expected {expected_generation}",
                    commit.rewrite_generation
                ),
            )));
        }
        let bytes = serde_json::to_vec(commit)?;
        TRANSCRIPT_REWRITE_PREFIX_COMMIT_SERIALIZATIONS
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let previous = self.raw_digest().ok_or_else(|| {
            serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "rewrite-prefix accumulator is not a canonical sha256 digest",
            ))
        })?;
        let mut hasher = Sha256::new();
        hasher.update(TRANSCRIPT_REWRITE_PREFIX_STEP_DOMAIN);
        hasher.update(previous);
        hasher.update((bytes.len() as u64).to_be_bytes());
        hasher.update(bytes);
        Ok(Self {
            occurrence_count: expected_generation,
            digest: format!("sha256:{:x}", hasher.finalize()),
        })
    }

    #[must_use]
    pub const fn occurrence_count(&self) -> u64 {
        self.occurrence_count
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    fn raw_digest(&self) -> Option<[u8; 32]> {
        let encoded = self.digest.strip_prefix("sha256:")?;
        if encoded.len() != 64 || !encoded.is_ascii() {
            return None;
        }
        let bytes = encoded.as_bytes();
        let mut decoded = [0u8; 32];
        for (index, output) in decoded.iter_mut().enumerate() {
            let high = hex_nibble(bytes[index * 2])?;
            let low = hex_nibble(bytes[index * 2 + 1])?;
            *output = (high << 4) | low;
        }
        Some(decoded)
    }
}

impl Default for TranscriptRewritePrefixAccumulator {
    fn default() -> Self {
        Self::empty()
    }
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

/// Canonical digest of an ordered, exact transcript-rewrite commit prefix.
///
/// Whole-blob validation and one-time 0.8.10 adoption use this rebuild seam.
/// Ordinary writers extend [`TranscriptRewritePrefixAccumulator`] directly.
pub fn transcript_rewrite_prefix_digest(
    commits: &[TranscriptRewriteCommit],
) -> Result<String, serde_json::Error> {
    Ok(TranscriptRewritePrefixAccumulator::from_commits(commits)?
        .digest()
        .to_string())
}

/// Extend a previously bound rewrite-prefix digest over an ordered delta.
pub fn extend_transcript_rewrite_prefix_accumulator(
    mut accumulator: TranscriptRewritePrefixAccumulator,
    commits: &[TranscriptRewriteCommit],
) -> Result<TranscriptRewritePrefixAccumulator, serde_json::Error> {
    for commit in commits {
        accumulator = accumulator.extend(commit)?;
    }
    Ok(accumulator)
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

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct Released0810RevisionEntry {
    revision: String,
    #[serde(default)]
    parent_revision: Option<String>,
    created_at: SystemTime,
    #[serde(default)]
    messages: Option<Vec<Message>>,
    #[serde(default)]
    rebase: Option<Released0810RevisionRebase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct Released0810RevisionRebase {
    base: String,
    at: usize,
    removed: usize,
    #[serde(default)]
    insert: Vec<Message>,
}

/// Exact released-0.8.10 commit shape.
///
/// The current commit carries a mandatory non-zero occurrence generation once
/// it enters compact authority. Keeping the released decoder separate prevents
/// a no-format candidate graph from laundering current-only fields through a
/// defaulted generation.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct Released0810Commit {
    parent_revision: String,
    revision: String,
    selection: TranscriptRewriteSelection,
    original_span_digest: String,
    replacement_digest: String,
    messages_before: usize,
    messages_after: usize,
    reason: TranscriptRewriteReason,
    #[serde(default)]
    actor: Option<String>,
    committed_at: SystemTime,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct Released0810HistoryWire {
    head: String,
    #[serde(default)]
    commits: Vec<Released0810Commit>,
    revisions: Vec<Released0810RevisionEntry>,
    #[serde(default, rename = "digest_format")]
    _digest_format: u32,
    #[serde(default, rename = "replay_cursor")]
    _replay_cursor: Option<serde::de::IgnoredAny>,
}

impl From<Released0810Commit> for TranscriptRewriteCommit {
    fn from(released: Released0810Commit) -> Self {
        Self {
            rewrite_generation: 0,
            parent_revision: released.parent_revision,
            revision: released.revision,
            selection: released.selection,
            original_span_digest: released.original_span_digest,
            replacement_digest: released.replacement_digest,
            messages_before: released.messages_before,
            messages_after: released.messages_after,
            reason: released.reason,
            actor: released.actor,
            committed_at: released.committed_at,
        }
    }
}

fn decode_released_0810_revision_chain<E>(
    entries: Vec<Released0810RevisionEntry>,
) -> Result<Vec<TranscriptRevisionBody>, E>
where
    E: serde::de::Error,
{
    let mut materialized: std::collections::HashMap<String, usize> =
        std::collections::HashMap::with_capacity(entries.len());
    let mut bodies: Vec<TranscriptRevisionBody> = Vec::with_capacity(entries.len());
    for entry in entries {
        let messages = match (entry.messages, entry.rebase) {
            (Some(messages), None) => messages,
            (None, Some(rebase)) => {
                let base = materialized
                    .get(&rebase.base)
                    .and_then(|index| bodies.get(*index))
                    .ok_or_else(|| {
                        E::custom(format!(
                            "released 0.8.10 revision {} rebases on missing {}",
                            entry.revision, rebase.base
                        ))
                    })?;
                let end = rebase
                    .at
                    .checked_add(rebase.removed)
                    .filter(|end| *end <= base.messages.len())
                    .ok_or_else(|| {
                        E::custom(format!(
                            "released 0.8.10 revision {} splice exceeds base {}",
                            entry.revision, rebase.base
                        ))
                    })?;
                let mut messages =
                    Vec::with_capacity(base.messages.len() - rebase.removed + rebase.insert.len());
                messages.extend_from_slice(&base.messages[..rebase.at]);
                messages.extend(rebase.insert);
                messages.extend_from_slice(&base.messages[end..]);
                messages
            }
            (Some(_), Some(_)) => {
                return Err(E::custom(format!(
                    "released 0.8.10 revision {} carries messages and rebase",
                    entry.revision
                )));
            }
            (None, None) => {
                return Err(E::custom(format!(
                    "released 0.8.10 revision {} carries neither messages nor rebase",
                    entry.revision
                )));
            }
        };
        let position = bodies.len();
        if materialized
            .insert(entry.revision.clone(), position)
            .is_some()
        {
            return Err(E::custom(format!(
                "released 0.8.10 revision vector repeats deduplicated id {}",
                entry.revision
            )));
        }
        bodies.push(TranscriptRevisionBody {
            revision: entry.revision,
            parent_revision: entry.parent_revision,
            messages,
            created_at: entry.created_at,
        });
    }
    Ok(bodies)
}

/// Frozen canonical witness input for the exact released-0.8.10 graph wire.
///
/// This intentionally never constructs [`TranscriptHistoryState`]. Formats 2
/// and 3 must prove the predecessor bytes before current occurrence edges,
/// row lineage, or generations are synthesized.
pub(crate) fn canonicalize_released_0810_checkpoint_history(
    value: &serde_json::Value,
) -> Result<serde_json::Value, serde_json::Error> {
    let wire: Released0810HistoryWire = serde_json::from_value(value.clone())?;
    let ValidatedReleased0810Wire {
        head,
        commits,
        revisions,
    } = validate_released_0810_wire(wire)?;
    let commits = released_0810_checkpoint_rewrite_commits_value(&commits)?;
    let mut revisions = revisions
        .into_iter()
        .map(|body| {
            serde_json::json!({
                "revision": body.revision,
                "messages": crate::session::canonicalize_messages_for_digest(&body.messages),
            })
        })
        .collect::<Vec<_>>();
    revisions.sort_by(|left, right| {
        left.get("revision")
            .and_then(serde_json::Value::as_str)
            .cmp(&right.get("revision").and_then(serde_json::Value::as_str))
    });
    Ok(serde_json::json!({
        "head": head,
        "commits": commits,
        "revisions": revisions,
    }))
}

/// Project current in-memory commit values back onto the exact released
/// 0.8.10 checkpoint witness shape.
///
/// Validation assigns occurrence generations before returning the commits,
/// but those generations did not exist in the released bytes and therefore
/// must not participate in their frozen checkpoint digest.
fn released_0810_checkpoint_rewrite_commits_value(
    commits: &[TranscriptRewriteCommit],
) -> Result<serde_json::Value, serde_json::Error> {
    let mut value = serde_json::to_value(commits)?;
    if let Some(commits) = value.as_array_mut() {
        for commit in commits {
            if let Some(fields) = commit.as_object_mut() {
                fields.remove("rewrite_generation");
            }
        }
    }
    Ok(value)
}

struct ValidatedReleased0810Wire {
    head: String,
    commits: Vec<TranscriptRewriteCommit>,
    revisions: Vec<TranscriptRevisionBody>,
}

fn validate_released_0810_wire(
    wire: Released0810HistoryWire,
) -> Result<ValidatedReleased0810Wire, serde_json::Error> {
    if wire.commits.is_empty() || wire.revisions.is_empty() {
        return Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "released 0.8.10 transcript graph must carry commits and bodies",
        )));
    }
    let mut commits = wire
        .commits
        .into_iter()
        .map(TranscriptRewriteCommit::from)
        .collect::<Vec<_>>();
    normalize_legacy_graph_rewrite_generations(&mut commits)
        .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))?;
    let revisions = decode_released_0810_revision_chain::<serde_json::Error>(wire.revisions)?;
    let bodies_by_revision = revisions
        .iter()
        .map(|body| (body.revision.as_str(), body))
        .collect::<std::collections::HashMap<_, _>>();
    if !bodies_by_revision.contains_key(wire.head.as_str()) {
        return Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "released 0.8.10 transcript graph omits advertised head {}",
                wire.head
            ),
        )));
    }
    for commit in &commits {
        let parent = bodies_by_revision
            .get(commit.parent_revision.as_str())
            .copied()
            .ok_or_else(|| {
                serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "released 0.8.10 graph omits parent body {}",
                        commit.parent_revision
                    ),
                ))
            })?;
        let revision = bodies_by_revision
            .get(commit.revision.as_str())
            .copied()
            .ok_or_else(|| {
                serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "released 0.8.10 graph omits revision body {}",
                        commit.revision
                    ),
                ))
            })?;
        validate_transcript_rewrite_record(commit, parent, revision)
            .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))?;
    }
    Ok(ValidatedReleased0810Wire {
        head: wire.head,
        commits,
        revisions,
    })
}

/// Bounded diagnostic validator for the compact current graph only.
///
/// A released full-body graph is refused before its representation-specific
/// decoder runs. Exact released semantic verification belongs to durable
/// ingress and is necessarily O(graph).
pub(crate) fn validate_current_transcript_history_slice(
    bytes: &[u8],
) -> Result<usize, serde_json::Error> {
    #[derive(Deserialize)]
    struct FormatProbe {
        #[serde(default)]
        format: Option<String>,
    }

    match serde_json::from_slice::<FormatProbe>(bytes)?.format {
        Some(format) if format == TRANSCRIPT_HISTORY_FORMAT_CURRENT => {
            let state: TranscriptHistoryState = serde_json::from_slice(bytes)?;
            Ok(state.commit_count())
        }
        Some(format) => Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unsupported transcript graph format {format}"),
        ))),
        None => Err(serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "bounded current-graph validation refuses released full-body history",
        ))),
    }
}

#[cfg(feature = "schema")]
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[schemars(rename = "SystemTime")]
struct SchemaSystemTime {
    secs_since_epoch: u64,
    nanos_since_epoch: u32,
}

/// Explicit durable format of the compact transcript-history authority.
///
/// Released 0.8.10 graphs have no discriminator and carry full revision
/// bodies. Current graphs must carry this exact value; a missing, empty, or
/// unknown discriminator never falls through to current interpretation.
pub const TRANSCRIPT_HISTORY_FORMAT_CURRENT: &str = "anchor_occurrence_edges_v1";

const TRANSCRIPT_GRAPH_PREFIX_ANCHOR_DOMAIN: &[u8] = b"meerkat.transcript-history.anchor.v1\0";
const TRANSCRIPT_GRAPH_PREFIX_EDGE_DOMAIN: &[u8] = b"meerkat.transcript-history.edge.v1\0";

static TRANSCRIPT_HISTORY_FULL_BODY_MATERIALIZATIONS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Number of explicit historical-body materializations.
///
/// Ordinary current-format decode, encode, rewrite construction, checkpoint
/// assembly, and WholeBlob preparation must leave this at zero. Restore and
/// 0.8.10 transcode are the only expected producers.
#[must_use]
pub fn transcript_history_full_body_materializations() -> u64 {
    TRANSCRIPT_HISTORY_FULL_BODY_MATERIALIZATIONS.load(std::sync::atomic::Ordering::Relaxed)
}

/// The one full transcript body retained by the compact graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TranscriptRevisionAnchor {
    revision: String,
    #[cfg_attr(feature = "schema", schemars(with = "Vec<serde_json::Value>"))]
    messages: Vec<Message>,
    row_prefix: SessionMessageRowPrefixAccumulator,
    #[cfg_attr(feature = "schema", schemars(with = "SchemaSystemTime"))]
    created_at: SystemTime,
}

impl TranscriptRevisionAnchor {
    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }

    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    #[must_use]
    pub fn row_prefix(&self) -> &SessionMessageRowPrefixAccumulator {
        &self.row_prefix
    }

    #[must_use]
    pub const fn created_at(&self) -> SystemTime {
        self.created_at
    }
}

/// Exact delta from the preceding audited child to the next rewrite parent.
///
/// Current writers construct only [`Self::ExactAppend`]. [`Self::ExactSplice`]
/// preserves an already-imported released 0.8.10 edge so historical audit
/// materialization remains exact without assigning semantic privilege to any
/// message role or transcript position.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TranscriptParentAdvance {
    ExactAppend {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[cfg_attr(feature = "schema", schemars(with = "Vec<serde_json::Value>"))]
        appended: Vec<Message>,
    },
    ExactSplice {
        at: usize,
        #[cfg_attr(feature = "schema", schemars(with = "Vec<serde_json::Value>"))]
        replacement: Vec<Message>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        #[cfg_attr(feature = "schema", schemars(with = "Vec<serde_json::Value>"))]
        appended: Vec<Message>,
    },
}

impl TranscriptParentAdvance {
    #[must_use]
    pub fn appended(&self) -> &[Message] {
        match self {
            Self::ExactAppend { appended } | Self::ExactSplice { appended, .. } => appended,
        }
    }

    #[must_use]
    pub fn exact_splice(&self) -> Option<(usize, &[Message])> {
        match self {
            Self::ExactAppend { .. } => None,
            Self::ExactSplice {
                at, replacement, ..
            } => Some((*at, replacement)),
        }
    }

    #[must_use]
    pub fn transition(&self) -> TranscriptRewriteParentTransition {
        match self {
            Self::ExactAppend { .. } => TranscriptRewriteParentTransition::ExactAppend,
            Self::ExactSplice { .. } => TranscriptRewriteParentTransition::ExactSplice,
        }
    }
}

/// Forward rewrite delta carried by one occurrence edge.
///
/// The removed span is committed by `original_span_digest` but is not retained
/// here, so this patch can materialize the child from its parent; it cannot
/// reconstruct the parent from the child.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TranscriptRewritePatch {
    at: usize,
    #[cfg_attr(feature = "schema", schemars(with = "Vec<serde_json::Value>"))]
    replacement: Vec<Message>,
}

impl TranscriptRewritePatch {
    #[must_use]
    pub const fn at(&self) -> usize {
        self.at
    }

    #[must_use]
    pub fn replacement(&self) -> &[Message] {
        &self.replacement
    }
}

/// Compact witness needed to relate an audited endpoint to a later live head.
///
/// The row lineage is mechanically derived from the preceding endpoint plus
/// typed append/splice operations; it is never a producer-attested flat content
/// root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TranscriptEndpointWitness {
    message_count: usize,
    row_prefix: SessionMessageRowPrefixAccumulator,
}

impl TranscriptEndpointWitness {
    pub(in crate::session) fn from_messages(
        messages: &[Message],
    ) -> Result<Self, serde_json::Error> {
        let row_prefix = SessionMessageRowPrefixAccumulator::from_messages(messages)
            .map_err(session_store_error_as_json)?;
        Self::from_messages_with_row_prefix(messages, row_prefix)
    }

    pub(in crate::session) fn from_messages_with_row_prefix(
        messages: &[Message],
        row_prefix: SessionMessageRowPrefixAccumulator,
    ) -> Result<Self, serde_json::Error> {
        if row_prefix.row_count() != messages.len() as u64 {
            return Err(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "endpoint row lineage count differs from its materialized messages",
            )));
        }
        Ok(Self {
            message_count: messages.len(),
            row_prefix,
        })
    }

    #[must_use]
    pub const fn message_count(&self) -> usize {
        self.message_count
    }

    #[must_use]
    pub fn row_prefix(&self) -> &SessionMessageRowPrefixAccumulator {
        &self.row_prefix
    }
}

/// One ordered rewrite occurrence in the compact graph.
///
/// Structural authority is occurrence-first: generation, rolling graph and
/// rewrite prefixes, exact parent/result row lineage, and the typed delta are
/// sufficient to persist/replay the occurrence without reconstructing every
/// preceding document. Commit revision/span digests remain checkpoint-bound
/// semantic audit labels and are re-proved lazily when a body is requested;
/// cold ingress performs one final-endpoint semantic replay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TranscriptRevisionEdge {
    commit: TranscriptRewriteCommit,
    rewrite_prefix: TranscriptRewritePrefixAccumulator,
    base_revision: String,
    messages_before_base: usize,
    parent_advance: TranscriptParentAdvance,
    parent_row_prefix: SessionMessageRowPrefixAccumulator,
    rewrite: TranscriptRewritePatch,
    result_witness: TranscriptEndpointWitness,
    #[cfg_attr(feature = "schema", schemars(with = "SchemaSystemTime"))]
    parent_created_at: SystemTime,
    #[cfg_attr(feature = "schema", schemars(with = "SchemaSystemTime"))]
    revision_created_at: SystemTime,
}

impl TranscriptRevisionEdge {
    /// Decode one exact current compact edge for cold replay.
    ///
    /// This is a strict wire decoder (`deny_unknown_fields`). The returned
    /// edge is not authority by itself: a caller must install it through a
    /// checkpoint/graph-prefix proved replay sequence.
    #[doc(hidden)]
    pub fn from_replay_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Stable exact bytes persisted beside the physical rewrite delta.
    #[doc(hidden)]
    pub fn to_replay_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    #[must_use]
    pub fn commit(&self) -> &TranscriptRewriteCommit {
        &self.commit
    }

    #[must_use]
    pub fn rewrite_prefix(&self) -> &TranscriptRewritePrefixAccumulator {
        &self.rewrite_prefix
    }

    #[must_use]
    pub const fn rewrite_generation(&self) -> u64 {
        self.commit.rewrite_generation
    }

    #[must_use]
    pub fn base_revision(&self) -> &str {
        &self.base_revision
    }

    #[must_use]
    pub fn parent_revision(&self) -> &str {
        &self.commit.parent_revision
    }

    #[must_use]
    pub fn revision(&self) -> &str {
        &self.commit.revision
    }

    #[must_use]
    pub const fn messages_before_base(&self) -> usize {
        self.messages_before_base
    }

    #[must_use]
    pub const fn messages_before(&self) -> usize {
        self.commit.messages_before
    }

    #[must_use]
    pub const fn messages_after(&self) -> usize {
        self.commit.messages_after
    }

    #[must_use]
    pub fn parent_advance(&self) -> &TranscriptParentAdvance {
        &self.parent_advance
    }

    #[must_use]
    pub fn parent_row_prefix(&self) -> &SessionMessageRowPrefixAccumulator {
        &self.parent_row_prefix
    }

    #[must_use]
    pub fn rewrite(&self) -> &TranscriptRewritePatch {
        &self.rewrite
    }

    #[must_use]
    pub fn result_witness(&self) -> &TranscriptEndpointWitness {
        &self.result_witness
    }

    #[must_use]
    pub const fn parent_created_at(&self) -> SystemTime {
        self.parent_created_at
    }

    #[must_use]
    pub const fn revision_created_at(&self) -> SystemTime {
        self.revision_created_at
    }
}

/// Rolling identity of the exact compact anchor and occurrence-edge sequence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TranscriptGraphPrefixAccumulator {
    occurrence_count: u64,
    digest: String,
}

impl TranscriptGraphPrefixAccumulator {
    fn from_anchor(anchor: &TranscriptRevisionAnchor) -> Result<Self, serde_json::Error> {
        let mut hasher = Sha256::new();
        hasher.update(TRANSCRIPT_GRAPH_PREFIX_ANCHOR_DOMAIN);
        update_graph_prefix_field(&mut hasher, b"revision", &anchor.revision)?;
        update_graph_prefix_field(&mut hasher, b"messages", &anchor.messages)?;
        update_graph_prefix_field(&mut hasher, b"row_prefix", &anchor.row_prefix)?;
        update_graph_prefix_field(&mut hasher, b"created_at", &anchor.created_at)?;
        Ok(Self {
            occurrence_count: 0,
            digest: format!("sha256:{:x}", hasher.finalize()),
        })
    }

    fn extend(&self, edge: &TranscriptRevisionEdge) -> Result<Self, serde_json::Error> {
        let expected = self.occurrence_count.checked_add(1).ok_or_else(|| {
            serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "transcript graph occurrence count exceeds u64",
            ))
        })?;
        if edge.commit.rewrite_generation != expected {
            return Err(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "transcript graph edge generation {} is not expected {expected}",
                    edge.commit.rewrite_generation
                ),
            )));
        }
        let previous = decode_canonical_sha256(&self.digest).ok_or_else(|| {
            serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "transcript graph prefix is not canonical sha256",
            ))
        })?;
        let mut hasher = Sha256::new();
        hasher.update(TRANSCRIPT_GRAPH_PREFIX_EDGE_DOMAIN);
        hasher.update(previous);
        update_graph_prefix_field(&mut hasher, b"commit", &edge.commit)?;
        update_graph_prefix_field(&mut hasher, b"rewrite_prefix", &edge.rewrite_prefix)?;
        update_graph_prefix_field(&mut hasher, b"base_revision", &edge.base_revision)?;
        update_graph_prefix_field(
            &mut hasher,
            b"messages_before_base",
            &edge.messages_before_base,
        )?;
        update_graph_prefix_field(&mut hasher, b"parent_advance", &edge.parent_advance)?;
        update_graph_prefix_field(&mut hasher, b"parent_row_prefix", &edge.parent_row_prefix)?;
        update_graph_prefix_field(&mut hasher, b"rewrite", &edge.rewrite)?;
        update_graph_prefix_field(&mut hasher, b"result_witness", &edge.result_witness)?;
        update_graph_prefix_field(&mut hasher, b"parent_created_at", &edge.parent_created_at)?;
        update_graph_prefix_field(
            &mut hasher,
            b"revision_created_at",
            &edge.revision_created_at,
        )?;
        Ok(Self {
            occurrence_count: expected,
            digest: format!("sha256:{:x}", hasher.finalize()),
        })
    }

    pub(super) fn from_graph<'a>(
        anchor: &TranscriptRevisionAnchor,
        edges: impl IntoIterator<Item = &'a TranscriptRevisionEdge>,
    ) -> Result<Self, serde_json::Error> {
        let mut prefix = Self::from_anchor(anchor)?;
        for edge in edges {
            prefix = prefix.extend(edge)?;
        }
        Ok(prefix)
    }

    #[must_use]
    pub const fn occurrence_count(&self) -> u64 {
        self.occurrence_count
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

fn update_graph_prefix_field<T: Serialize + ?Sized>(
    hasher: &mut Sha256,
    label: &[u8],
    value: &T,
) -> Result<(), serde_json::Error> {
    let bytes = serde_json::to_vec(value)?;
    hasher.update((label.len() as u64).to_be_bytes());
    hasher.update(label);
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    Ok(())
}

fn session_store_error_as_json(error: crate::SessionStoreError) -> serde_json::Error {
    serde_json::Error::io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        error.to_string(),
    ))
}

fn decode_canonical_sha256(value: &str) -> Option<[u8; 32]> {
    let encoded = value.strip_prefix("sha256:")?;
    if encoded.len() != 64 || !encoded.is_ascii() {
        return None;
    }
    let mut decoded = [0u8; 32];
    for (index, output) in decoded.iter_mut().enumerate() {
        let bytes = encoded.as_bytes();
        *output = (hex_nibble(bytes[index * 2])? << 4) | hex_nibble(bytes[index * 2 + 1])?;
    }
    Some(decoded)
}

/// Self-contained append-only transcript rewrite record.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptRewriteRecord {
    pub commit: TranscriptRewriteCommit,
    pub parent_body: TranscriptRevisionBody,
    pub revision_body: TranscriptRevisionBody,
    /// Digest-format generation of this record's revision strings. Records
    /// stamped `>= 2` were written by the content-addressed digest format, so
    /// decode skips the per-decode legacy-heal probe (a full-transcript hash
    /// of BOTH bodies); absent/0 means unknown provenance and the probe runs,
    /// exactly as it did before the marker existed. A compatibility
    /// convenience, not an integrity boundary: the record's own validation
    /// against its commit owns integrity, and a stamped record that does not
    /// validate is rejected exactly as an unstamped one is.
    ///
    /// Records are append-only and never restamped in place, so this skips
    /// the probe only for records minted from the version that added it.
    #[serde(default, skip_serializing_if = "digest_format_is_unknown")]
    pub digest_format: u32,
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
            #[serde(default)]
            digest_format: u32,
        }
        let wire = Wire::deserialize(deserializer)?;
        crate::digest_observability::record_rewrite_record_body_decode();
        let mut revisions = vec![wire.parent_body, wire.revision_body];
        let mut commits = vec![wire.commit];
        // Fast path: a record stamped with the current digest format skips the
        // heal outright — the heal hashes both full transcript bodies, and
        // every authoritative load decodes every record in the append-only
        // log. Unstamped records pay the probe exactly as before.
        if wire.digest_format < TRANSCRIPT_DIGEST_FORMAT_CURRENT {
            heal_legacy_revision_strings(&mut revisions, &mut commits, None)
                .map_err(serde::de::Error::custom)?;
            heal_legacy_compaction_rewrite_semantics(&mut commits, &revisions);
        }
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
            // The heal above leaves current-format strings behind, so the
            // decoded value is stamped whatever the wire carried.
            digest_format: TRANSCRIPT_DIGEST_FORMAT_CURRENT,
        })
    }
}

impl TranscriptRewriteRecord {
    /// Validate this already-owned record without cloning either transcript body.
    pub fn validate(&self) -> Result<(), TranscriptEditError> {
        if self.commit.rewrite_generation == 0 {
            return Err(TranscriptEditError::HistoryStateMalformed(
                "current transcript rewrite records require a non-zero occurrence generation"
                    .to_string(),
            ));
        }
        validate_transcript_rewrite_record(&self.commit, &self.parent_body, &self.revision_body)
    }

    pub fn new(
        commit: TranscriptRewriteCommit,
        parent_body: TranscriptRevisionBody,
        revision_body: TranscriptRevisionBody,
    ) -> Result<Self, TranscriptEditError> {
        let record = Self {
            commit,
            parent_body,
            revision_body,
            digest_format: TRANSCRIPT_DIGEST_FORMAT_CURRENT,
        };
        record.validate()?;
        Ok(record)
    }
}

#[derive(Debug)]
struct TranscriptEdgeNode {
    previous: Option<Arc<TranscriptEdgeNode>>,
    edge: Arc<TranscriptRevisionEdge>,
    graph_prefix: TranscriptGraphPrefixAccumulator,
}

#[derive(Debug)]
struct TranscriptEdgeChain {
    tail: Option<Arc<TranscriptEdgeNode>>,
    len: usize,
    ordered: std::sync::OnceLock<Vec<Arc<TranscriptRevisionEdge>>>,
}

/// Structurally shared append-only occurrence storage.
///
/// Session clones share one tail node. Appending a rewrite allocates exactly
/// one edge and one node; it never copies the accumulated edge vector. The
/// chronological `ordered` projection is populated only at explicit
/// validation/wire/restore boundaries.
#[derive(Debug, Clone)]
struct PersistentTranscriptEdges {
    chain: Arc<TranscriptEdgeChain>,
}

impl PersistentTranscriptEdges {
    fn empty() -> Self {
        Self {
            chain: Arc::new(TranscriptEdgeChain {
                tail: None,
                len: 0,
                ordered: std::sync::OnceLock::new(),
            }),
        }
    }

    fn from_vec(
        anchor: &TranscriptRevisionAnchor,
        edges: Vec<TranscriptRevisionEdge>,
    ) -> Result<Self, serde_json::Error> {
        let mut persistent = Self::empty();
        let mut graph_prefix = TranscriptGraphPrefixAccumulator::from_anchor(anchor)?;
        for edge in edges {
            graph_prefix = graph_prefix.extend(&edge)?;
            persistent.push(edge, graph_prefix.clone());
        }
        Ok(persistent)
    }

    fn push(
        &mut self,
        edge: TranscriptRevisionEdge,
        graph_prefix: TranscriptGraphPrefixAccumulator,
    ) {
        let node = Arc::new(TranscriptEdgeNode {
            previous: self.chain.tail.clone(),
            edge: Arc::new(edge),
            graph_prefix,
        });
        self.chain = Arc::new(TranscriptEdgeChain {
            tail: Some(node),
            len: self.chain.len + 1,
            ordered: std::sync::OnceLock::new(),
        });
    }

    fn len(&self) -> usize {
        self.chain.len
    }

    fn last(&self) -> Option<&TranscriptRevisionEdge> {
        self.chain.tail.as_ref().map(|node| node.edge.as_ref())
    }

    fn ordered(&self) -> &[Arc<TranscriptRevisionEdge>] {
        self.chain.ordered.get_or_init(|| {
            let mut ordered = Vec::with_capacity(self.chain.len);
            let mut cursor = self.chain.tail.clone();
            while let Some(node) = cursor {
                ordered.push(Arc::clone(&node.edge));
                cursor = node.previous.clone();
            }
            ordered.reverse();
            ordered
        })
    }

    fn get(&self, index: usize) -> Option<&TranscriptRevisionEdge> {
        if index >= self.chain.len {
            return None;
        }
        let mut cursor = self.chain.tail.as_deref();
        for _ in index + 1..self.chain.len {
            cursor = cursor?.previous.as_deref();
        }
        cursor.map(|node| node.edge.as_ref())
    }

    /// Rolling compact graph identity after exactly `edge_count` occurrences.
    ///
    /// The backward walk visits only the suffix after that occurrence. Current
    /// terminal lookup is O(1); proving an observed predecessor for `k` pending
    /// edges is O(k), never O(accumulated history).
    fn graph_prefix_at(&self, edge_count: usize) -> Option<&TranscriptGraphPrefixAccumulator> {
        if edge_count == 0 || edge_count > self.chain.len {
            return None;
        }
        let mut cursor = self.chain.tail.as_deref();
        for _ in edge_count..self.chain.len {
            cursor = cursor?.previous.as_deref();
        }
        cursor.map(|node| &node.graph_prefix)
    }

    /// Collect only the chronological suffix beginning at `start`.
    ///
    /// This is the ordinary hot-path projection. It walks and retains exactly
    /// `len - start` edge Arcs and never populates the full ordered cache.
    fn suffix(&self, start: usize) -> Option<Vec<Arc<TranscriptRevisionEdge>>> {
        if start > self.chain.len {
            return None;
        }
        let suffix_len = self.chain.len - start;
        let mut suffix = Vec::with_capacity(suffix_len);
        let mut cursor = self.chain.tail.clone();
        for _ in 0..suffix_len {
            let node = cursor?;
            suffix.push(Arc::clone(&node.edge));
            cursor = node.previous.clone();
        }
        suffix.reverse();
        Some(suffix)
    }

    fn prefix(&self, edge_count: usize) -> Option<Self> {
        if edge_count == 0 || edge_count > self.chain.len {
            return None;
        }
        let mut tail = self.chain.tail.clone();
        for _ in edge_count..self.chain.len {
            tail = tail?.previous.clone();
        }
        Some(Self {
            chain: Arc::new(TranscriptEdgeChain {
                tail,
                len: edge_count,
                ordered: std::sync::OnceLock::new(),
            }),
        })
    }
}

/// Typed session-local transcript revision authority.
///
/// The anchor and ordered occurrence edges are the ONE in-memory and durable
/// representation. Full historical bodies are not cached beside them.
/// Structural validation is edge-local and occurrence-ordered; it does not
/// claim that every intermediate semantic label was eagerly re-hashed.
#[derive(Debug, Clone)]
pub struct TranscriptHistoryState {
    format: &'static str,
    anchor: Arc<TranscriptRevisionAnchor>,
    edges: PersistentTranscriptEdges,
    rewrite_prefix: TranscriptRewritePrefixAccumulator,
    graph_prefix: TranscriptGraphPrefixAccumulator,
    /// Digest-format generation of the revision strings. Documents stamped
    /// `>= 2` were written by the content-addressed digest format, so decode
    /// skips the per-decode legacy-heal probe (a full-transcript hash);
    /// absent/0 means unknown provenance and the probe runs once — the next
    /// save persists the marker. A compatibility convenience, not an
    /// integrity boundary: validated graph ingress and store-owned physical
    /// authorities own integrity.
    digest_format: u32,
}

/// Published shape of the durable transcript graph.
///
/// [`TranscriptHistoryState`] serializes through a hand-written impl, so a
/// DERIVED schema over the typed struct would advertise `revisions` as an
/// array of full [`TranscriptRevisionBody`] objects — bytes this writer no
/// longer produces. Deriving a schema over a custom `Serialize` is exactly how
/// a published schema comes to describe behaviour the code does not have, so
/// the schema is taken from this mirror of the real wire form and
/// [`TranscriptHistoryState`]'s `JsonSchema` forwards to it.
#[cfg(feature = "schema")]
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[schemars(rename = "TranscriptHistoryState")]
struct SchemaTranscriptHistoryState {
    format: String,
    anchor: TranscriptRevisionAnchor,
    edges: Vec<TranscriptRevisionEdge>,
    rewrite_prefix: TranscriptRewritePrefixAccumulator,
    graph_prefix: TranscriptGraphPrefixAccumulator,
    digest_format: u32,
}

#[cfg(feature = "schema")]
impl schemars::JsonSchema for TranscriptHistoryState {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TranscriptHistoryState".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        <SchemaTranscriptHistoryState as schemars::JsonSchema>::json_schema(generator)
    }
}

fn digest_format_is_unknown(format: &u32) -> bool {
    *format == 0
}

/// The digest-format generation minted by [`transcript_messages_digest`].
pub(crate) const TRANSCRIPT_DIGEST_FORMAT_CURRENT: u32 = 2;

impl Serialize for TranscriptHistoryState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;

        let mut wire = serializer.serialize_struct("TranscriptHistoryState", 6)?;
        wire.serialize_field("format", self.format)?;
        wire.serialize_field("anchor", self.anchor.as_ref())?;
        wire.serialize_field("edges", self.edges.ordered())?;
        wire.serialize_field("rewrite_prefix", &self.rewrite_prefix)?;
        wire.serialize_field("graph_prefix", &self.graph_prefix)?;
        wire.serialize_field("digest_format", &self.digest_format)?;
        wire.end()
    }
}

impl<'de> Deserialize<'de> for TranscriptHistoryState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case", deny_unknown_fields)]
        struct CurrentWire {
            format: String,
            anchor: TranscriptRevisionAnchor,
            edges: Vec<TranscriptRevisionEdge>,
            rewrite_prefix: TranscriptRewritePrefixAccumulator,
            graph_prefix: TranscriptGraphPrefixAccumulator,
            digest_format: u32,
        }

        let wire = CurrentWire::deserialize(deserializer)?;
        if wire.format != TRANSCRIPT_HISTORY_FORMAT_CURRENT {
            return Err(serde::de::Error::custom(format!(
                "unsupported current transcript graph format {}",
                wire.format
            )));
        }
        if wire.digest_format != TRANSCRIPT_DIGEST_FORMAT_CURRENT {
            return Err(serde::de::Error::custom(format!(
                "current transcript graph digest format {} is not supported",
                wire.digest_format
            )));
        }
        let persistent_edges = PersistentTranscriptEdges::from_vec(&wire.anchor, wire.edges)
            .map_err(serde::de::Error::custom)?;
        let state = TranscriptHistoryState {
            format: TRANSCRIPT_HISTORY_FORMAT_CURRENT,
            anchor: Arc::new(wire.anchor),
            edges: persistent_edges,
            rewrite_prefix: wire.rewrite_prefix,
            graph_prefix: wire.graph_prefix,
            digest_format: wire.digest_format,
        };
        validate_transcript_history_state(&state).map_err(serde::de::Error::custom)?;
        Ok(state)
    }
}

/// One-time 0.8.10 importer for the frozen full-body graph wire.
///
/// Normal [`TranscriptHistoryState`] deserialization is deliberately
/// current-only. A released graph can cross this seam only after the enclosing
/// importer has verified its untouched checkpoint evidence.
pub(crate) fn import_released_0810_history(
    value: serde_json::Value,
) -> Result<TranscriptHistoryState, serde_json::Error> {
    let wire: Released0810HistoryWire = serde_json::from_value(value)?;
    let ValidatedReleased0810Wire {
        head,
        commits,
        revisions,
    } = validate_released_0810_wire(wire)?;
    TranscriptHistoryState::from_legacy_full_bodies(head, commits, revisions)
        .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))
}

impl TranscriptHistoryState {
    fn from_legacy_full_bodies(
        legacy_head: String,
        commits: Vec<TranscriptRewriteCommit>,
        revisions: Vec<TranscriptRevisionBody>,
    ) -> Result<Self, TranscriptEditError> {
        let first = commits.first().ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "released 0.8.10 transcript graph carries no rewrite commit".to_string(),
            )
        })?;
        for body in &revisions {
            let digest = transcript_messages_digest(&body.messages)
                .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
            if digest != body.revision {
                return Err(TranscriptEditError::HistoryStateMalformed(format!(
                    "released 0.8.10 transcript body {} has digest {digest}",
                    body.revision
                )));
            }
        }
        let bodies_by_revision = revisions
            .iter()
            .map(|body| (body.revision.as_str(), body))
            .collect::<std::collections::HashMap<_, _>>();
        if bodies_by_revision.len() != revisions.len() {
            return Err(TranscriptEditError::HistoryStateMalformed(
                "released 0.8.10 graph repeats a revision body id its writer deduplicated"
                    .to_string(),
            ));
        }
        let anchor_body = bodies_by_revision
            .get(first.parent_revision.as_str())
            .copied()
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(format!(
                    "released 0.8.10 graph omits first parent body {}",
                    first.parent_revision
                ))
            })?;
        let anchor = TranscriptRevisionAnchor {
            revision: anchor_body.revision.clone(),
            messages: anchor_body.messages.clone(),
            row_prefix: SessionMessageRowPrefixAccumulator::from_messages(&anchor_body.messages)
                .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?,
            created_at: anchor_body.created_at,
        };
        let mut edges = Vec::with_capacity(commits.len());
        let mut rewrite_prefix = TranscriptRewritePrefixAccumulator::empty();
        let mut previous = anchor_body;
        let mut previous_witness = TranscriptEndpointWitness::from_messages(&anchor_body.messages)
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        for commit in commits {
            let parent = bodies_by_revision
                .get(commit.parent_revision.as_str())
                .copied()
                .ok_or_else(|| {
                    TranscriptEditError::HistoryStateMalformed(format!(
                        "released 0.8.10 graph omits parent body {}",
                        commit.parent_revision
                    ))
                })?;
            let revision = bodies_by_revision
                .get(commit.revision.as_str())
                .copied()
                .ok_or_else(|| {
                    TranscriptEditError::HistoryStateMalformed(format!(
                        "released 0.8.10 graph omits revision body {}",
                        commit.revision
                    ))
                })?;
            validate_transcript_rewrite_record(&commit, parent, revision)?;
            rewrite_prefix = rewrite_prefix
                .extend(&commit)
                .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
            let edge = edge_from_materialized_bodies(
                previous,
                &previous_witness,
                parent,
                revision,
                commit,
                rewrite_prefix.clone(),
                MaterializedParentAdvanceSource::Released0810Import,
            )?;
            previous_witness = edge.result_witness().clone();
            edges.push(edge);
            previous = revision;
        }
        if legacy_head != previous.revision {
            let live_head = bodies_by_revision
                .get(legacy_head.as_str())
                .copied()
                .ok_or_else(|| {
                    TranscriptEditError::HistoryStateMalformed(format!(
                        "released 0.8.10 graph omits advertised head body {legacy_head}"
                    ))
                })?;
            parent_advance_from_materialized(
                previous,
                live_head,
                u64::MAX,
                MaterializedParentAdvanceSource::Released0810Import,
            )?;
        }
        let graph_prefix = TranscriptGraphPrefixAccumulator::from_graph(&anchor, &edges)
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        let persistent_edges = PersistentTranscriptEdges::from_vec(&anchor, edges)
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        let state = Self {
            format: TRANSCRIPT_HISTORY_FORMAT_CURRENT,
            anchor: Arc::new(anchor),
            edges: persistent_edges,
            rewrite_prefix,
            graph_prefix,
            digest_format: TRANSCRIPT_DIGEST_FORMAT_CURRENT,
        };
        validate_transcript_history_state(&state)?;
        Ok(state)
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::session) fn from_authorized_first_rewrite(
        parent: TranscriptRevisionBody,
        parent_row_prefix: SessionMessageRowPrefixAccumulator,
        revision: &str,
        revision_messages: &[Message],
        revision_created_at: SystemTime,
        result_row_prefix: SessionMessageRowPrefixAccumulator,
        replacement: Vec<Message>,
        commit: TranscriptRewriteCommit,
    ) -> Result<Self, TranscriptEditError> {
        if parent_row_prefix.row_count() != parent.messages.len() as u64
            || result_row_prefix.row_count() != revision_messages.len() as u64
            || commit.parent_revision != parent.revision
            || commit.revision != revision
            || commit.messages_before != parent.messages.len()
            || commit.messages_after != revision_messages.len()
        {
            return Err(TranscriptEditError::HistoryStateMalformed(
                "authorized first rewrite carries inconsistent endpoint authority".to_string(),
            ));
        }
        let anchor = TranscriptRevisionAnchor {
            revision: parent.revision.clone(),
            messages: parent.messages,
            row_prefix: parent_row_prefix.clone(),
            created_at: parent.created_at,
        };
        let rewrite_prefix = TranscriptRewritePrefixAccumulator::empty()
            .extend(&commit)
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        let base_witness = TranscriptEndpointWitness::from_messages_with_row_prefix(
            &anchor.messages,
            parent_row_prefix.clone(),
        )
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        let result_witness = TranscriptEndpointWitness::from_messages_with_row_prefix(
            revision_messages,
            result_row_prefix,
        )
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        let (at, _) = commit.selection.bounds();
        let edge = TranscriptRevisionEdge {
            commit,
            rewrite_prefix: rewrite_prefix.clone(),
            base_revision: anchor.revision.clone(),
            messages_before_base: anchor.messages.len(),
            parent_advance: TranscriptParentAdvance::ExactAppend {
                appended: Vec::new(),
            },
            parent_row_prefix,
            rewrite: TranscriptRewritePatch { at, replacement },
            result_witness,
            parent_created_at: anchor.created_at,
            revision_created_at,
        };
        super::validate::validate_transcript_revision_edge(
            anchor.revision(),
            &base_witness,
            &edge,
        )?;
        let graph_prefix = TranscriptGraphPrefixAccumulator::from_anchor(&anchor)
            .and_then(|prefix| prefix.extend(&edge))
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        let persistent_edges = PersistentTranscriptEdges::from_vec(&anchor, vec![edge])
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        Ok(Self {
            format: TRANSCRIPT_HISTORY_FORMAT_CURRENT,
            anchor: Arc::new(anchor),
            edges: persistent_edges,
            rewrite_prefix,
            graph_prefix,
            digest_format: TRANSCRIPT_DIGEST_FORMAT_CURRENT,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::session) fn append_authorized_rewrite(
        &mut self,
        commit: TranscriptRewriteCommit,
        messages_before_base: usize,
        parent_advance: TranscriptParentAdvance,
        parent_row_prefix: SessionMessageRowPrefixAccumulator,
        replacement: Vec<Message>,
        result_witness: TranscriptEndpointWitness,
        parent_created_at: SystemTime,
        revision_created_at: SystemTime,
    ) -> Result<(), TranscriptEditError> {
        let base_revision = self.head().to_string();
        let (at, _) = commit.selection.bounds();
        let rewrite_prefix = self
            .rewrite_prefix()
            .extend(&commit)
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        let edge = TranscriptRevisionEdge {
            commit,
            rewrite_prefix,
            base_revision,
            messages_before_base,
            parent_advance,
            parent_row_prefix,
            rewrite: TranscriptRewritePatch { at, replacement },
            result_witness,
            parent_created_at,
            revision_created_at,
        };
        let base_witness = self.final_endpoint_witness().ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "current transcript graph has no audited endpoint".to_string(),
            )
        })?;
        super::validate::validate_transcript_revision_edge(
            self.edges
                .last()
                .map_or(self.anchor.revision(), |edge| edge.revision()),
            base_witness,
            &edge,
        )?;
        self.graph_prefix = self
            .graph_prefix
            .extend(&edge)
            .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        self.rewrite_prefix = edge.rewrite_prefix.clone();
        self.edges.push(edge, self.graph_prefix.clone());
        Ok(())
    }

    /// Current compact wire format.
    #[must_use]
    pub fn format(&self) -> &str {
        self.format
    }

    #[must_use]
    pub const fn digest_format(&self) -> u32 {
        self.digest_format
    }

    #[must_use]
    pub fn head(&self) -> &str {
        self.edges
            .last()
            .map(TranscriptRevisionEdge::revision)
            .unwrap_or_else(|| self.anchor.revision())
    }

    pub fn commits(&self) -> impl ExactSizeIterator<Item = &TranscriptRewriteCommit> {
        self.edges.ordered().iter().map(|edge| edge.commit())
    }

    #[must_use]
    pub fn commit_count(&self) -> usize {
        self.edges.len()
    }

    #[must_use]
    pub fn commit(&self, index: usize) -> Option<&TranscriptRewriteCommit> {
        self.edges.get(index).map(TranscriptRevisionEdge::commit)
    }

    pub(crate) fn edge(&self, index: usize) -> Option<&TranscriptRevisionEdge> {
        self.edges.get(index)
    }

    #[must_use]
    pub fn last_commit(&self) -> Option<&TranscriptRewriteCommit> {
        self.edges.last().map(TranscriptRevisionEdge::commit)
    }

    #[must_use]
    pub fn rewrite_prefix(&self) -> &TranscriptRewritePrefixAccumulator {
        &self.rewrite_prefix
    }

    #[must_use]
    pub fn anchor(&self) -> &TranscriptRevisionAnchor {
        self.anchor.as_ref()
    }

    #[must_use]
    /// Explicit chronological projection for wire/cold validation/restore.
    ///
    /// The first call after an append is O(history). Ordinary persistence must
    /// use a sealed rewrite suffix instead.
    pub fn edges(&self) -> &[Arc<TranscriptRevisionEdge>] {
        self.edges.ordered()
    }

    pub(super) fn edge_suffix(&self, start: usize) -> Option<Vec<Arc<TranscriptRevisionEdge>>> {
        self.edges.suffix(start)
    }

    #[must_use]
    pub fn graph_prefix(&self) -> &TranscriptGraphPrefixAccumulator {
        &self.graph_prefix
    }

    /// Whether `prefix` is the exact structural prefix of this graph.
    ///
    /// Both values are construction- or decode-validated, so matching the
    /// rolling graph prefix at the predecessor's occurrence count binds the
    /// anchor and every exact ordered edge without materializing historical
    /// bodies or comparing a parallel commit vector.
    #[must_use]
    pub(crate) fn extends_exact_graph(&self, prefix: &Self) -> bool {
        let prefix_count = prefix.commit_count();
        if prefix_count > self.commit_count() {
            return false;
        }
        if prefix_count == 0 {
            let expected_graph_prefix =
                TranscriptGraphPrefixAccumulator::from_anchor(prefix.anchor()).ok();
            return self.anchor() == prefix.anchor()
                && prefix.rewrite_prefix() == &TranscriptRewritePrefixAccumulator::empty()
                && expected_graph_prefix.as_ref() == Some(prefix.graph_prefix());
        }
        self.graph_prefix_at(prefix_count) == Some(prefix.graph_prefix())
    }

    /// Unique logical occurrence position of one content revision.
    ///
    /// The child of every rewrite edge is a new logical occurrence even when
    /// its content digest equals its parent (`A -> A`). The parent of a later
    /// edge reuses the preceding child only when both labels are equal; a
    /// different parent label is an append-derived intermediate endpoint.
    ///
    /// Any digest that labels more than one of those logical occurrences is
    /// ambiguous by digest alone and returns `None`; callers holding a commit
    /// or graph prefix must use that exact occurrence authority instead.
    #[must_use]
    pub(crate) fn unique_revision_position(&self, revision: &str) -> Option<usize> {
        fn observe(
            candidate: &str,
            candidate_position: usize,
            revision: &str,
            found: &mut Option<usize>,
            ambiguous: &mut bool,
        ) {
            if candidate == revision && found.replace(candidate_position).is_some() {
                *ambiguous = true;
            }
        }

        let mut position = 0usize;
        let mut found = None;
        let mut ambiguous = false;
        let mut preceding_child = self.anchor.revision();

        observe(
            self.anchor.revision(),
            position,
            revision,
            &mut found,
            &mut ambiguous,
        );
        for edge in self.edges.ordered() {
            if edge.parent_revision() != preceding_child {
                position += 1;
                observe(
                    edge.parent_revision(),
                    position,
                    revision,
                    &mut found,
                    &mut ambiguous,
                );
            }

            // A rewrite always mints a new occurrence. Do not collapse an
            // exact-content no-op edge: its generation is still durable
            // authority and digest-only callers cannot choose either side.
            position += 1;
            observe(
                edge.revision(),
                position,
                revision,
                &mut found,
                &mut ambiguous,
            );
            preceding_child = edge.revision();
        }
        if ambiguous { None } else { found }
    }

    /// Digest-only ancestry, restricted to unique logical occurrences.
    ///
    /// Equality does not bypass occurrence resolution: in `A -> B -> A`, even
    /// `revision_extends(A, A)` is ambiguous. Exact commit/generation or graph
    /// prefix authority is required to distinguish those two `A` occurrences.
    #[must_use]
    pub(crate) fn revision_extends(&self, descendant: &str, ancestor: &str) -> bool {
        match (
            self.unique_revision_position(descendant),
            self.unique_revision_position(ancestor),
        ) {
            (Some(descendant), Some(ancestor)) => descendant >= ancestor,
            _ => false,
        }
    }

    /// Whether this graph contains this exact rewrite occurrence.
    #[must_use]
    pub(crate) fn contains_exact_commit(&self, commit: &TranscriptRewriteCommit) -> bool {
        commit
            .rewrite_generation
            .checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| self.commit(index))
            == Some(commit)
    }

    /// Compact graph identity after an exact positive occurrence count.
    ///
    /// `None` for zero means the physical head predates any retained graph.
    pub(crate) fn graph_prefix_at(
        &self,
        occurrence_count: usize,
    ) -> Option<&TranscriptGraphPrefixAccumulator> {
        self.edges.graph_prefix_at(occurrence_count)
    }

    pub fn final_endpoint_witness(&self) -> Option<&TranscriptEndpointWitness> {
        self.edges
            .last()
            .map(TranscriptRevisionEdge::result_witness)
    }

    /// Cold ingress: replay the final semantic endpoint exactly once, then
    /// derive the operation-lineage relation installed on the warm Session.
    pub(in crate::session) fn derive_live_row_lineage_after_final_semantic_replay(
        &self,
        live: &[Message],
    ) -> Result<Option<SessionMessageRowPrefixAccumulator>, TranscriptEditError> {
        let endpoint_witness = self.final_endpoint_witness().ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "compact transcript graph has no final endpoint witness".to_string(),
            )
        })?;
        let endpoint = self.materialize_revision(self.head())?;
        if live.len() < endpoint.messages.len() {
            return Ok(None);
        }
        let advance = if live[..endpoint.messages.len()] == endpoint.messages {
            TranscriptParentAdvance::ExactAppend {
                appended: live[endpoint.messages.len()..].to_vec(),
            }
        } else {
            return Ok(None);
        };
        row_prefix_after_parent_advance(endpoint_witness.row_prefix(), &advance).map(Some)
    }

    #[must_use]
    pub fn parent_transition(&self, index: usize) -> Option<TranscriptRewriteParentTransition> {
        self.edges
            .get(index)
            .map(|edge| edge.parent_advance.transition())
    }

    #[must_use]
    pub fn parent_transitions(&self) -> Vec<TranscriptRewriteParentTransition> {
        self.edges
            .ordered()
            .iter()
            .map(|edge| edge.parent_advance.transition())
            .collect()
    }

    #[must_use]
    pub fn contains_revision(&self, revision: &str) -> bool {
        self.anchor.revision == revision
            || self
                .edges
                .ordered()
                .iter()
                .any(|edge| edge.parent_revision() == revision || edge.revision() == revision)
    }

    #[must_use]
    pub fn retained_revision_count(&self) -> usize {
        1usize.saturating_add(self.edges.len().saturating_mul(2))
    }

    /// The current graph is already canonical. Kept as a narrow compatibility
    /// seam for callers that previously pruned full-body snapshots.
    pub(crate) fn compact_mechanical_revision_bodies(&mut self) -> Result<(), TranscriptEditError> {
        validate_transcript_history_state(self)?;
        Ok(())
    }

    /// Current compact construction never carries mechanical bodies.
    pub(crate) fn canonicalize_to_latest_audited_head(&mut self) {
        // `head()` is derived from the final edge.
    }

    pub(crate) fn prune_mechanical_revision_bodies(&mut self) {}

    pub(super) fn proved_prefix(&self, edge_count: usize) -> Result<Self, TranscriptEditError> {
        if edge_count == 0 || edge_count > self.edges.len() {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "compact graph prefix {edge_count} is outside 1..={}",
                self.edges.len()
            )));
        }
        let edges = self.edges.prefix(edge_count).ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "compact graph prefix could not address its persistent tail".to_string(),
            )
        })?;
        let rewrite_prefix = edges
            .last()
            .map(TranscriptRevisionEdge::rewrite_prefix)
            .cloned()
            .ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(
                    "compact graph prefix lost its final edge".to_string(),
                )
            })?;
        let graph_prefix = TranscriptGraphPrefixAccumulator::from_graph(
            self.anchor.as_ref(),
            edges.ordered().iter().map(AsRef::as_ref),
        )
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
        Ok(Self {
            format: TRANSCRIPT_HISTORY_FORMAT_CURRENT,
            anchor: Arc::clone(&self.anchor),
            edges,
            rewrite_prefix,
            graph_prefix,
            digest_format: TRANSCRIPT_DIGEST_FORMAT_CURRENT,
        })
    }

    /// Explicit exceptional materialization for restore/audit consumers.
    pub fn materialize_revision(
        &self,
        revision: &str,
    ) -> Result<TranscriptRevisionBody, TranscriptEditError> {
        if let Some(index) = self
            .edges
            .ordered()
            .iter()
            .rposition(|edge| edge.revision() == revision)
        {
            return self.materialize_occurrence(index, false);
        }
        if let Some(index) = self
            .edges
            .ordered()
            .iter()
            .rposition(|edge| edge.parent_revision() == revision)
        {
            return self.materialize_occurrence(index, true);
        }
        if self.anchor.revision == revision {
            TRANSCRIPT_HISTORY_FULL_BODY_MATERIALIZATIONS
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(TranscriptRevisionBody {
                revision: self.anchor.revision.clone(),
                parent_revision: None,
                messages: self.anchor.messages.clone(),
                created_at: self.anchor.created_at,
            });
        }
        Err(TranscriptEditError::HistoryStateMalformed(format!(
            "missing transcript revision {revision}"
        )))
    }

    /// Explicitly materialize the parent of one exact rewrite occurrence.
    pub(crate) fn materialize_occurrence_parent(
        &self,
        edge_index: usize,
    ) -> Result<TranscriptRevisionBody, TranscriptEditError> {
        self.materialize_occurrence(edge_index, true)
    }

    pub(crate) fn materialize_occurrence_child(
        &self,
        edge_index: usize,
    ) -> Result<TranscriptRevisionBody, TranscriptEditError> {
        self.materialize_occurrence(edge_index, false)
    }

    fn materialize_occurrence(
        &self,
        edge_index: usize,
        parent: bool,
    ) -> Result<TranscriptRevisionBody, TranscriptEditError> {
        if edge_index >= self.edges.len() {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "rewrite occurrence index {edge_index} is outside the compact graph"
            )));
        }
        TRANSCRIPT_HISTORY_FULL_BODY_MATERIALIZATIONS
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut messages = self.anchor.messages.clone();
        for (index, edge) in self.edges.ordered().iter().enumerate().take(edge_index + 1) {
            apply_parent_advance(&mut messages, &edge.parent_advance)?;
            if index == edge_index && parent {
                let digest = transcript_messages_digest(&messages).map_err(|error| {
                    TranscriptEditError::HistoryStateMalformed(error.to_string())
                })?;
                if digest != edge.parent_revision() {
                    return Err(TranscriptEditError::HistoryStateMalformed(format!(
                        "rewrite occurrence {} materialized parent has digest {digest}, expected {}",
                        edge.rewrite_generation(),
                        edge.parent_revision()
                    )));
                }
                return Ok(TranscriptRevisionBody {
                    revision: edge.parent_revision().to_string(),
                    parent_revision: Some(edge.base_revision.clone()),
                    messages,
                    created_at: edge.parent_created_at,
                });
            }
            let (_, end) = edge.commit.selection.bounds();
            if end > messages.len() {
                return Err(TranscriptEditError::HistoryStateMalformed(format!(
                    "rewrite occurrence {} patch exceeds materialized parent",
                    edge.rewrite_generation()
                )));
            }
            let removed_digest = transcript_messages_digest(&messages[edge.rewrite.at..end])
                .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
            if removed_digest != edge.commit.original_span_digest {
                return Err(TranscriptEditError::HistoryStateMalformed(format!(
                    "rewrite occurrence {} materialized removed span has wrong digest",
                    edge.rewrite_generation()
                )));
            }
            messages.splice(edge.rewrite.at..end, edge.rewrite.replacement.clone());
            if index == edge_index {
                let digest = transcript_messages_digest(&messages).map_err(|error| {
                    TranscriptEditError::HistoryStateMalformed(error.to_string())
                })?;
                if digest != edge.revision() {
                    return Err(TranscriptEditError::HistoryStateMalformed(format!(
                        "rewrite occurrence {} materialized child has digest {digest}, expected {}",
                        edge.rewrite_generation(),
                        edge.revision()
                    )));
                }
                return Ok(TranscriptRevisionBody {
                    revision: edge.revision().to_string(),
                    parent_revision: Some(edge.parent_revision().to_string()),
                    messages,
                    created_at: edge.revision_created_at,
                });
            }
        }
        Err(TranscriptEditError::HistoryStateMalformed(
            "rewrite occurrence materialization did not reach its target".to_string(),
        ))
    }

    /// Explicit compatibility view for diagnostics and 0.8.10 reconciliation.
    pub fn materialize_revision_bodies(
        &self,
    ) -> Result<Vec<TranscriptRevisionBody>, TranscriptEditError> {
        let mut seen = BTreeSet::new();
        let mut bodies = Vec::new();
        for revision in std::iter::once(self.anchor.revision.as_str()).chain(
            self.edges
                .ordered()
                .iter()
                .flat_map(|edge| [edge.parent_revision(), edge.revision()]),
        ) {
            if seen.insert(revision.to_string()) {
                bodies.push(self.materialize_revision(revision)?);
            }
        }
        Ok(bodies)
    }
}

/// Decode the relationship carried by already-materialized audit bodies.
/// Current audit reconstruction admits only
/// [`TranscriptParentAdvance::ExactAppend`]; the explicit released-0.8.10
/// importer alone may decode a frozen same-cardinality splice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaterializedParentAdvanceSource {
    CurrentAudit,
    Released0810Import,
}

fn parent_advance_from_materialized(
    base: &TranscriptRevisionBody,
    parent: &TranscriptRevisionBody,
    rewrite_generation: u64,
    source: MaterializedParentAdvanceSource,
) -> Result<TranscriptParentAdvance, TranscriptEditError> {
    if parent.messages.len() < base.messages.len() {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite occurrence {rewrite_generation} parent is shorter than its base"
        )));
    }
    if parent.messages[..base.messages.len()] == base.messages {
        return Ok(TranscriptParentAdvance::ExactAppend {
            appended: parent.messages[base.messages.len()..].to_vec(),
        });
    }
    if source == MaterializedParentAdvanceSource::CurrentAudit {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "current rewrite occurrence {rewrite_generation} parent is not an exact append"
        )));
    }
    let retained_parent = &parent.messages[..base.messages.len()];
    let at = base
        .messages
        .iter()
        .zip(retained_parent)
        .take_while(|(base, parent)| base == parent)
        .count();
    let common_suffix = base.messages[at..]
        .iter()
        .rev()
        .zip(retained_parent[at..].iter().rev())
        .take_while(|(base, parent)| base == parent)
        .count();
    let end = base.messages.len() - common_suffix;
    if at >= end {
        return Err(TranscriptEditError::HistoryStateMalformed(format!(
            "rewrite occurrence {rewrite_generation} importer could not derive an exact parent splice"
        )));
    }
    Ok(TranscriptParentAdvance::ExactSplice {
        at,
        replacement: retained_parent[at..end].to_vec(),
        appended: parent.messages[base.messages.len()..].to_vec(),
    })
}

fn row_prefix_after_parent_advance(
    base: &SessionMessageRowPrefixAccumulator,
    advance: &TranscriptParentAdvance,
) -> Result<SessionMessageRowPrefixAccumulator, TranscriptEditError> {
    let advanced = match advance {
        TranscriptParentAdvance::ExactAppend { .. } => base.clone(),
        TranscriptParentAdvance::ExactSplice {
            at, replacement, ..
        } => {
            let replacement_rows = replacement
                .iter()
                .map(serde_json::to_vec)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
            let end = at.checked_add(replacement.len()).ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(
                    "imported parent splice end overflowed".to_string(),
                )
            })?;
            let at = u64::try_from(*at).map_err(|_| {
                TranscriptEditError::HistoryStateMalformed(
                    "imported parent splice start exceeds durable row coordinates".to_string(),
                )
            })?;
            let end = u64::try_from(end).map_err(|_| {
                TranscriptEditError::HistoryStateMalformed(
                    "imported parent splice end exceeds durable row coordinates".to_string(),
                )
            })?;
            base.replace_serialized_range(at, end, &replacement_rows)
                .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?
        }
    };
    let appended = advance
        .appended()
        .iter()
        .map(serde_json::to_vec)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    advanced
        .extend_serialized_rows(&appended)
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))
}

fn edge_from_materialized_bodies(
    base: &TranscriptRevisionBody,
    base_witness: &TranscriptEndpointWitness,
    parent: &TranscriptRevisionBody,
    revision: &TranscriptRevisionBody,
    commit: TranscriptRewriteCommit,
    rewrite_prefix: TranscriptRewritePrefixAccumulator,
    source: MaterializedParentAdvanceSource,
) -> Result<TranscriptRevisionEdge, TranscriptEditError> {
    let parent_advance =
        parent_advance_from_materialized(base, parent, commit.rewrite_generation, source)?;
    let parent_row_prefix =
        row_prefix_after_parent_advance(base_witness.row_prefix(), &parent_advance)?;
    let (at, end) = commit.selection.bounds();
    let removed_len = end.checked_sub(at).ok_or_else(|| {
        TranscriptEditError::HistoryStateMalformed("rewrite selection is inverted".to_string())
    })?;
    let retained = commit
        .messages_before
        .checked_sub(removed_len)
        .ok_or_else(|| {
            TranscriptEditError::HistoryStateMalformed(
                "rewrite removes more messages than parent carries".to_string(),
            )
        })?;
    let replacement_len = commit.messages_after.checked_sub(retained).ok_or_else(|| {
        TranscriptEditError::HistoryStateMalformed(
            "rewrite message counts cannot describe replacement".to_string(),
        )
    })?;
    let replacement_end = at.checked_add(replacement_len).ok_or_else(|| {
        TranscriptEditError::HistoryStateMalformed("rewrite replacement end overflow".to_string())
    })?;
    let replacement = revision.messages[at..replacement_end].to_vec();
    let replacement_rows = replacement
        .iter()
        .map(serde_json::to_vec)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    let result_row_prefix = parent_row_prefix
        .replace_serialized_range(at as u64, end as u64, &replacement_rows)
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
    let edge = TranscriptRevisionEdge {
        rewrite_prefix,
        base_revision: base.revision.clone(),
        messages_before_base: base.messages.len(),
        parent_advance,
        parent_row_prefix,
        rewrite: TranscriptRewritePatch { at, replacement },
        result_witness: TranscriptEndpointWitness::from_messages_with_row_prefix(
            &revision.messages,
            result_row_prefix,
        )
        .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?,
        parent_created_at: parent.created_at,
        revision_created_at: revision.created_at,
        commit,
    };
    super::validate::validate_transcript_revision_edge(&base.revision, base_witness, &edge)?;
    Ok(edge)
}

fn apply_parent_advance(
    messages: &mut Vec<Message>,
    advance: &TranscriptParentAdvance,
) -> Result<(), TranscriptEditError> {
    match advance {
        TranscriptParentAdvance::ExactAppend { appended } => messages.extend_from_slice(appended),
        TranscriptParentAdvance::ExactSplice {
            at,
            replacement,
            appended,
        } => {
            let end = at.checked_add(replacement.len()).ok_or_else(|| {
                TranscriptEditError::HistoryStateMalformed(
                    "imported parent splice end overflowed".to_string(),
                )
            })?;
            if replacement.is_empty() || end > messages.len() {
                return Err(TranscriptEditError::HistoryStateMalformed(
                    "imported parent splice is empty or outside its base".to_string(),
                ));
            }
            messages.splice(*at..end, replacement.iter().cloned());
            messages.extend_from_slice(appended);
        }
    }
    Ok(())
}

/// Whether `proved` already carries every fact
/// [`validate_transcript_rewrite_record`] would derive for `record`.
///
/// That validator proves relations among exactly three values: the commit and
/// the two endpoint message vectors. This returns true only when the proved
/// graph holds all three — a byte-equal commit, and endpoint bodies whose
/// messages equal the record's — so the relations it would derive are the ones
/// [`validate_transcript_history_state`] already derived over those same three
/// values when `proved` was sealed.
///
/// The message equality is what stands in for the hash, and it is a proof
/// rather than a heuristic: [`transcript_messages_digest`] is a pure function
/// of the message vector, so a vector equal to one already verified against a
/// revision string digests to that same string. A body that no longer digests
/// to its commit, a body the proved graph does not retain, a commit it does not
/// carry, or a body whose own revision label disagrees with the commit all
/// return false and take the full validation, which rejects them exactly as
/// before.
pub(super) fn record_is_proved_by(
    _proved: Option<&ValidatedTranscriptHistory>,
    _record: &TranscriptRewriteRecord,
) -> bool {
    // A compact edge proves its delta and checkpoint-bound identities. It
    // cannot prove arbitrary bytes elsewhere in a legacy record's two full
    // bodies. Released 0.8.10 reconciliation therefore validates those bodies
    // once; current receipt-only replay never presents this full-body type.
    false
}

impl TranscriptHistoryState {
    /// Rebuild transcript revision graph state from append-only rewrite records.
    pub fn from_rewrite_records<I>(records: I) -> Result<Option<Self>, TranscriptEditError>
    where
        I: IntoIterator<Item = TranscriptRewriteRecord>,
    {
        Self::from_rewrite_records_with_proved(records, None)
    }

    /// [`Self::from_rewrite_records`] against a graph that already proves some
    /// of the log.
    ///
    /// Every authoritative load used to re-prove EVERY record in the log, and
    /// a rewrite record carries two FULL transcript bodies, so resume cost grew
    /// as retained-revisions x transcript — quadratic over a session's life.
    /// `proved` is the session's own validated graph: already in memory,
    /// already hashed. A record it covers needs no second hash pass; a record
    /// it does not cover is validated in full, unchanged. Integrity is not
    /// traded for the saving — `record_is_proved_by` documents exactly what
    /// "covers" has to mean before a proof may be skipped.
    pub fn from_rewrite_records_with_proved<I>(
        records: I,
        proved: Option<&ValidatedTranscriptHistory>,
    ) -> Result<Option<Self>, TranscriptEditError>
    where
        I: IntoIterator<Item = TranscriptRewriteRecord>,
    {
        let mut records = records.into_iter().collect::<Vec<_>>();
        if records.is_empty() {
            return Ok(None);
        }
        // A physical/audit vector is not occurrence authority. Exact-equal
        // 0.8.10 rows are information-theoretically ambiguous, so assigning
        // generation from caller order here could erase or invent a real
        // cycle occurrence. The one-time EventStore reconciliation must map
        // legacy rows against the checkpoint-bound graph Vec first and pass
        // only generation-bearing records to this generic rebuild seam.
        if records
            .iter()
            .any(|record| record.commit.rewrite_generation == 0)
        {
            return Err(TranscriptEditError::HistoryStateMalformed(
                "generation-less 0.8.10 rewrite records require checkpoint-bound EventStore reconciliation"
                    .to_string(),
            ));
        }
        records.sort_by_key(|record| record.commit.rewrite_generation);
        let proved_count = proved.map_or(0, |history| history.state().commit_count());
        let first_generation = records[0].commit.rewrite_generation;
        if first_generation > 1 && first_generation != proved_count as u64 + 1 {
            return Err(TranscriptEditError::HistoryStateMalformed(format!(
                "rewrite record set starts at occurrence {first_generation} without the exact proved prefix"
            )));
        }
        let mut new_records = Vec::new();
        for record in records {
            let generation = record.commit.rewrite_generation;
            if generation <= proved_count as u64 {
                validate_transcript_rewrite_record(
                    &record.commit,
                    &record.parent_body,
                    &record.revision_body,
                )?;
                let matches_prefix = proved.and_then(|history| {
                    usize::try_from(generation - 1)
                        .ok()
                        .and_then(|index| history.state().commit(index))
                }) == Some(&record.commit);
                if !matches_prefix {
                    return Err(TranscriptEditError::HistoryStateMalformed(format!(
                        "rewrite record occurrence {generation} conflicts with proved graph prefix"
                    )));
                }
                continue;
            }
            let expected = proved_count as u64 + new_records.len() as u64 + 1;
            if generation != expected {
                return Err(TranscriptEditError::HistoryStateMalformed(format!(
                    "rewrite record occurrence {generation} is not expected contiguous generation {expected}"
                )));
            }
            new_records.push(record);
        }
        if new_records.is_empty() {
            return Ok(proved.map(|history| history.state().clone()));
        }

        let mut state = proved.map(|history| history.state().clone());
        let mut previous = if let Some(history) = proved {
            Some(
                history
                    .state()
                    .materialize_revision(history.state().head())?,
            )
        } else {
            None
        };
        let mut previous_witness =
            proved.and_then(|history| history.state().final_endpoint_witness().cloned());
        for record in new_records {
            validate_transcript_rewrite_record(
                &record.commit,
                &record.parent_body,
                &record.revision_body,
            )?;
            if state.is_none() {
                let (start, end) = record.commit.selection.bounds();
                let removed = end.checked_sub(start).ok_or_else(|| {
                    TranscriptEditError::HistoryStateMalformed(
                        "rewrite record selection is inverted".to_string(),
                    )
                })?;
                let retained = record
                    .commit
                    .messages_before
                    .checked_sub(removed)
                    .ok_or_else(|| {
                        TranscriptEditError::HistoryStateMalformed(
                            "rewrite record selection exceeds its parent".to_string(),
                        )
                    })?;
                let replacement_len = record
                    .commit
                    .messages_after
                    .checked_sub(retained)
                    .ok_or_else(|| {
                        TranscriptEditError::HistoryStateMalformed(
                            "rewrite record successor is shorter than retained spans".to_string(),
                        )
                    })?;
                let replacement_end = start.checked_add(replacement_len).ok_or_else(|| {
                    TranscriptEditError::HistoryStateMalformed(
                        "rewrite record replacement range overflow".to_string(),
                    )
                })?;
                let replacement = record
                    .revision_body
                    .messages
                    .get(start..replacement_end)
                    .ok_or_else(|| {
                        TranscriptEditError::HistoryStateMalformed(
                            "rewrite record replacement range exceeds successor".to_string(),
                        )
                    })?
                    .to_vec();
                let parent_prefix =
                    SessionMessageRowPrefixAccumulator::from_messages(&record.parent_body.messages)
                        .map_err(|error| {
                            TranscriptEditError::HistoryStateMalformed(error.to_string())
                        })?;
                let result_prefix = SessionMessageRowPrefixAccumulator::from_messages(
                    &record.revision_body.messages,
                )
                .map_err(|error| TranscriptEditError::HistoryStateMalformed(error.to_string()))?;
                state = Some(Self::from_authorized_first_rewrite(
                    record.parent_body.clone(),
                    parent_prefix,
                    &record.revision_body.revision,
                    &record.revision_body.messages,
                    record.revision_body.created_at,
                    result_prefix,
                    replacement,
                    record.commit.clone(),
                )?);
            } else {
                let base = previous.as_ref().ok_or_else(|| {
                    TranscriptEditError::HistoryStateMalformed(
                        "rewrite tail lost its preceding endpoint".to_string(),
                    )
                })?;
                let next_rewrite_prefix = state
                    .as_ref()
                    .ok_or_else(|| {
                        TranscriptEditError::HistoryStateMalformed(
                            "rewrite state initialization failed".to_string(),
                        )
                    })?
                    .rewrite_prefix
                    .extend(&record.commit)
                    .map_err(|error| {
                        TranscriptEditError::HistoryStateMalformed(error.to_string())
                    })?;
                let edge = edge_from_materialized_bodies(
                    base,
                    previous_witness.as_ref().ok_or_else(|| {
                        TranscriptEditError::HistoryStateMalformed(
                            "rewrite tail lost its preceding row-lineage witness".to_string(),
                        )
                    })?,
                    &record.parent_body,
                    &record.revision_body,
                    record.commit.clone(),
                    next_rewrite_prefix.clone(),
                    MaterializedParentAdvanceSource::CurrentAudit,
                )?;
                let state = state.as_mut().ok_or_else(|| {
                    TranscriptEditError::HistoryStateMalformed(
                        "rewrite state initialization failed".to_string(),
                    )
                })?;
                state.rewrite_prefix = next_rewrite_prefix;
                state.graph_prefix = state.graph_prefix.extend(&edge).map_err(|error| {
                    TranscriptEditError::HistoryStateMalformed(error.to_string())
                })?;
                state.edges.push(edge, state.graph_prefix.clone());
            }
            previous_witness = state
                .as_ref()
                .and_then(TranscriptHistoryState::final_endpoint_witness)
                .cloned();
            previous = Some(record.revision_body);
        }
        Ok(state)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::redundant_clone)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_prefix_digest_binds_order_and_exact_commit_facts() {
        let records = rewrite_chain(2);
        let commits = records
            .iter()
            .map(|record| record.commit.clone())
            .collect::<Vec<_>>();
        let baseline =
            transcript_rewrite_prefix_digest(&commits).expect("canonical prefix serializes");
        let first = TranscriptRewritePrefixAccumulator::from_commits(&commits[..1])
            .expect("first prefix serializes");
        let extended = extend_transcript_rewrite_prefix_accumulator(first, &commits[1..])
            .expect("tail extension serializes");
        assert_eq!(extended.digest(), baseline);
        assert_eq!(
            extended.occurrence_count(),
            u64::try_from(commits.len()).expect("test length fits u64")
        );

        let mut actor_changed = commits.clone();
        actor_changed[0].actor = Some("different-actor".to_string());
        assert_ne!(
            transcript_rewrite_prefix_digest(&actor_changed).expect("changed prefix serializes"),
            baseline,
            "the receipt must bind full commit facts, not only revisions"
        );

        let mut reordered = commits.clone();
        reordered.reverse();
        for (index, commit) in reordered.iter_mut().enumerate() {
            commit.rewrite_generation = u64::try_from(index)
                .expect("test index fits u64")
                .saturating_add(1);
        }
        assert_ne!(
            transcript_rewrite_prefix_digest(&reordered).expect("reordered prefix serializes"),
            baseline,
            "the receipt must bind lineage order"
        );
    }

    #[test]
    fn rewrite_generation_distinguishes_byte_equal_occurrences_and_legacy_order_normalizes() {
        let mut commits = rewrite_chain(1)
            .into_iter()
            .map(|record| record.commit)
            .collect::<Vec<_>>();
        let mut recurrence = commits[0].clone();
        recurrence.rewrite_generation = 2;
        commits.push(recurrence);
        let one =
            transcript_rewrite_prefix_digest(&commits[..1]).expect("one occurrence serializes");
        let two = transcript_rewrite_prefix_digest(&commits).expect("two occurrences serialize");
        assert_ne!(
            one, two,
            "byte-equal rewrite facts at distinct generations are distinct occurrences"
        );

        for commit in &mut commits {
            commit.rewrite_generation = 0;
        }
        assert!(
            normalize_legacy_graph_rewrite_generations(&mut commits)
                .expect("proved 0.8.10 vector order normalizes")
        );
        assert_eq!(
            commits
                .iter()
                .map(|commit| commit.rewrite_generation)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn generic_record_rebuild_cannot_authorize_generation_zero_from_input_order() {
        let mut records = rewrite_chain(2);
        for record in &mut records {
            record.commit.rewrite_generation = 0;
        }
        let error = TranscriptHistoryState::from_rewrite_records(records)
            .expect_err("audit row order is not legacy occurrence authority");
        assert!(
            matches!(error, TranscriptEditError::HistoryStateMalformed(ref message)
                if message.contains("checkpoint-bound EventStore reconciliation")),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn rewrite_prefix_wire_and_count_mismatch_refuse_fail_closed() {
        let malformed = serde_json::json!({
            "occurrence_count": 1,
            "digest": "sha256:ABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCDEFABCD"
        });
        assert!(
            serde_json::from_value::<TranscriptRewritePrefixAccumulator>(malformed).is_err(),
            "uppercase or otherwise non-canonical digest text must not enter authority"
        );

        let records = rewrite_chain(2);
        let mut state = rebuild(&records);
        state.rewrite_prefix.occurrence_count = 1;
        assert!(
            validate_transcript_history_state(&state).is_err(),
            "the carried count/digest pair must match the exact graph occurrences"
        );
    }
    use crate::digest_observability::session_content_digest_bytes;
    use crate::session::{TranscriptRewriteReason, TranscriptRewriteSelection};
    use crate::types::UserMessage;

    fn message(text: &str) -> Message {
        Message::User(UserMessage::text(text.to_string()))
    }

    fn body(messages: Vec<Message>, parent: Option<&str>) -> TranscriptRevisionBody {
        let revision = transcript_messages_digest(&messages).expect("digest revision body");
        TranscriptRevisionBody {
            revision,
            parent_revision: parent.map(str::to_string),
            messages,
            created_at: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn released_importer_preserves_arbitrary_exact_parent_splice() {
        let base = body(
            vec![
                message("retained prefix"),
                message("released base"),
                message("retained turn"),
            ],
            None,
        );
        let parent = body(
            vec![
                message("retained prefix"),
                message("released replacement"),
                message("retained turn"),
                message("released appended turn"),
            ],
            Some(&base.revision),
        );

        let advance = parent_advance_from_materialized(
            &base,
            &parent,
            2,
            MaterializedParentAdvanceSource::Released0810Import,
        )
        .expect("released historical relationship remains materializable");
        assert!(matches!(
            &advance,
            TranscriptParentAdvance::ExactSplice {
                at: 1,
                replacement,
                appended,
            } if replacement == &[message("released replacement")]
                && appended == &[message("released appended turn")]
        ));
        let mut materialized = base.messages.clone();
        apply_parent_advance(&mut materialized, &advance)
            .expect("historical parent advance applies");
        assert_eq!(materialized, parent.messages);
    }

    #[test]
    fn current_record_rebuild_refuses_non_append_parent_divergence() {
        let first_parent = body(vec![message("retained prefix"), message("question")], None);
        let first_revision = body(
            vec![message("retained prefix"), message("first edit")],
            Some(&first_parent.revision),
        );
        let first_commit = TranscriptRewriteCommit {
            rewrite_generation: 1,
            parent_revision: first_parent.revision.clone(),
            revision: first_revision.revision.clone(),
            selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            original_span_digest: transcript_messages_digest(&first_parent.messages[1..2])
                .expect("digest first original span"),
            replacement_digest: transcript_messages_digest(&first_revision.messages[1..2])
                .expect("digest first replacement span"),
            messages_before: first_parent.messages.len(),
            messages_after: first_revision.messages.len(),
            reason: TranscriptRewriteReason::new("unit-test"),
            actor: None,
            committed_at: SystemTime::UNIX_EPOCH,
        };
        let first_record =
            TranscriptRewriteRecord::new(first_commit, first_parent, first_revision.clone())
                .expect("first rewrite record is valid");

        let second_parent = body(
            vec![message("divergent prefix"), message("first edit")],
            None,
        );
        let second_revision = body(
            vec![
                Message::System(crate::types::SystemMessage::new("replacement system")),
                message("second edit"),
            ],
            Some(&second_parent.revision),
        );
        let second_commit = TranscriptRewriteCommit {
            rewrite_generation: 2,
            parent_revision: second_parent.revision.clone(),
            revision: second_revision.revision.clone(),
            selection: TranscriptRewriteSelection::MessageRange { start: 1, end: 2 },
            original_span_digest: transcript_messages_digest(&second_parent.messages[1..2])
                .expect("digest second original span"),
            replacement_digest: transcript_messages_digest(&second_revision.messages[1..2])
                .expect("digest second replacement span"),
            messages_before: second_parent.messages.len(),
            messages_after: second_revision.messages.len(),
            reason: TranscriptRewriteReason::new("unit-test"),
            actor: None,
            committed_at: SystemTime::UNIX_EPOCH,
        };
        let second_record =
            TranscriptRewriteRecord::new(second_commit, second_parent, second_revision)
                .expect("second rewrite record is internally valid");

        let error = TranscriptHistoryState::from_rewrite_records([first_record, second_record])
            .expect_err("current audit reconstruction must reject a non-append parent bridge");
        assert!(
            matches!(error, TranscriptEditError::HistoryStateMalformed(ref message)
                if message.contains("current rewrite occurrence 2 parent is not an exact append")),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn current_record_cannot_request_legacy_compaction_semantic_healing() {
        let parent_body = body(vec![message("old turn"), message("newer turn")], None);
        let revision_body = body(
            vec![Message::User(UserMessage::compaction_summary("summary"))],
            Some(&parent_body.revision),
        );
        let commit = TranscriptRewriteCommit {
            rewrite_generation: 1,
            parent_revision: parent_body.revision.clone(),
            revision: revision_body.revision.clone(),
            selection: TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
            original_span_digest: transcript_messages_digest(&parent_body.messages)
                .expect("digest original transcript"),
            replacement_digest: transcript_messages_digest(&revision_body.messages)
                .expect("digest replacement transcript"),
            messages_before: parent_body.messages.len(),
            messages_after: revision_body.messages.len(),
            reason: TranscriptRewriteReason::new("current-untyped-selection"),
            actor: None,
            committed_at: SystemTime::UNIX_EPOCH,
        };
        let current_wire = serde_json::to_value(TranscriptRewriteRecord {
            commit,
            parent_body,
            revision_body,
            digest_format: TRANSCRIPT_DIGEST_FORMAT_CURRENT,
        })
        .expect("serialize current record");

        let current: TranscriptRewriteRecord =
            serde_json::from_value(current_wire.clone()).expect("decode current record");
        assert!(
            current.commit.selection.is_legacy_untyped(),
            "a current-format record must not enter importer-only semantic healing"
        );

        let mut released_wire = current_wire;
        released_wire
            .as_object_mut()
            .expect("record is an object")
            .remove("digest_format");
        let released: TranscriptRewriteRecord =
            serde_json::from_value(released_wire).expect("decode released record");
        assert!(
            !released.commit.selection.is_legacy_untyped(),
            "an unstamped released record retains its one-time typed compaction migration"
        );
    }

    /// A chain of `count` rewrites over a fixed-length transcript, each
    /// replacing message 0. This is retained as adversarial audit-record input;
    /// new System messages are ordered appends and never mint it.
    fn rewrite_chain(count: usize) -> Vec<TranscriptRewriteRecord> {
        let mut messages = (0..6)
            .map(|index| message(&format!("turn {index}")))
            .collect::<Vec<_>>();
        let mut records = Vec::with_capacity(count);
        for generation in 0..count {
            let parent_body = body(messages.clone(), None);
            messages[0] = message(&format!("system prompt generation {generation}"));
            let revision_body = body(messages.clone(), Some(&parent_body.revision));
            let commit = TranscriptRewriteCommit {
                rewrite_generation: u64::try_from(generation).expect("test generation fits u64")
                    + 1,
                parent_revision: parent_body.revision.clone(),
                revision: revision_body.revision.clone(),
                selection: TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                original_span_digest: transcript_messages_digest(&parent_body.messages[..1])
                    .expect("digest original span"),
                replacement_digest: transcript_messages_digest(&revision_body.messages[..1])
                    .expect("digest replacement span"),
                messages_before: parent_body.messages.len(),
                messages_after: revision_body.messages.len(),
                reason: TranscriptRewriteReason::new("adversarial-rewrite-chain"),
                actor: None,
                committed_at: SystemTime::UNIX_EPOCH,
            };
            records.push(
                TranscriptRewriteRecord::new(commit, parent_body, revision_body)
                    .expect("chain record is valid"),
            );
        }
        records
    }

    fn recurrence_chain() -> Vec<TranscriptRewriteRecord> {
        let a = body(vec![message("A")], None);
        let b = body(vec![message("B")], Some(&a.revision));
        let first = TranscriptRewriteRecord::new(
            TranscriptRewriteCommit {
                rewrite_generation: 1,
                parent_revision: a.revision.clone(),
                revision: b.revision.clone(),
                selection: TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                original_span_digest: transcript_messages_digest(&a.messages).expect("digest A"),
                replacement_digest: transcript_messages_digest(&b.messages).expect("digest B"),
                messages_before: 1,
                messages_after: 1,
                reason: TranscriptRewriteReason::new("recurrence-test"),
                actor: None,
                committed_at: SystemTime::UNIX_EPOCH,
            },
            a.clone(),
            b.clone(),
        )
        .expect("A to B record");
        let returned_a = body(a.messages.clone(), Some(&b.revision));
        let second = TranscriptRewriteRecord::new(
            TranscriptRewriteCommit {
                rewrite_generation: 2,
                parent_revision: b.revision.clone(),
                revision: returned_a.revision.clone(),
                selection: TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                original_span_digest: transcript_messages_digest(&b.messages).expect("digest B"),
                replacement_digest: transcript_messages_digest(&returned_a.messages)
                    .expect("digest returned A"),
                messages_before: 1,
                messages_after: 1,
                reason: TranscriptRewriteReason::new("recurrence-test"),
                actor: None,
                committed_at: SystemTime::UNIX_EPOCH,
            },
            b,
            returned_a,
        )
        .expect("B to A record");
        vec![first, second]
    }

    fn rebuild(records: &[TranscriptRewriteRecord]) -> TranscriptHistoryState {
        TranscriptHistoryState::from_rewrite_records(records.to_vec())
            .expect("rebuild from records")
            .expect("chain is non-empty")
    }

    fn sealed(records: &[TranscriptRewriteRecord]) -> ValidatedTranscriptHistory {
        ValidatedTranscriptHistory::seal_owned(rebuild(records)).expect("rebuilt chain seals")
    }

    fn hashed_bytes<T>(operation: impl FnOnce() -> T) -> (T, u64) {
        let before = session_content_digest_bytes();
        let value = operation();
        (value, session_content_digest_bytes() - before)
    }

    fn assert_same_graph(left: &TranscriptHistoryState, right: &TranscriptHistoryState) {
        assert_eq!(
            serde_json::to_value(left).expect("left graph serializes"),
            serde_json::to_value(right).expect("right graph serializes"),
            "compact graph wires differ"
        );
    }

    fn zero_edge_prefix(anchor_body: &TranscriptRevisionBody) -> TranscriptHistoryState {
        let anchor = TranscriptRevisionAnchor {
            revision: anchor_body.revision.clone(),
            messages: anchor_body.messages.clone(),
            row_prefix: SessionMessageRowPrefixAccumulator::from_messages(&anchor_body.messages)
                .expect("anchor row prefix"),
            created_at: anchor_body.created_at,
        };
        TranscriptHistoryState {
            format: TRANSCRIPT_HISTORY_FORMAT_CURRENT,
            graph_prefix: TranscriptGraphPrefixAccumulator::from_anchor(&anchor)
                .expect("anchor graph prefix"),
            anchor: Arc::new(anchor),
            edges: PersistentTranscriptEdges::empty(),
            rewrite_prefix: TranscriptRewritePrefixAccumulator::empty(),
            digest_format: TRANSCRIPT_DIGEST_FORMAT_CURRENT,
        }
    }

    #[test]
    fn exact_graph_extension_accepts_only_the_same_zero_edge_anchor_authority() {
        let records = rewrite_chain(2);
        let full = rebuild(&records);
        let prefix = zero_edge_prefix(&records[0].parent_body);
        assert!(
            full.extends_exact_graph(&prefix),
            "the exact anchor is the zero-occurrence prefix of its rewrite graph"
        );

        let foreign_anchor = body(vec![message("foreign anchor")], None);
        assert!(
            !full.extends_exact_graph(&zero_edge_prefix(&foreign_anchor)),
            "a different anchor cannot authorize a zero-occurrence prefix"
        );

        let mut malformed = prefix;
        malformed.rewrite_prefix =
            TranscriptRewritePrefixAccumulator::from_commits(&[records[0].commit.clone()])
                .expect("one-commit prefix");
        assert!(
            !full.extends_exact_graph(&malformed),
            "zero edges cannot carry a non-empty rewrite prefix"
        );
    }

    #[test]
    fn digest_only_ancestry_refuses_nonconsecutive_revision_recurrence() {
        let records = recurrence_chain();
        let state = rebuild(&records);
        let a = records[0].commit.parent_revision.as_str();
        let b = records[0].commit.revision.as_str();

        assert_eq!(state.unique_revision_position(a), None);
        assert!(state.unique_revision_position(b).is_some());
        assert!(
            !state.revision_extends(a, b),
            "the later A cannot make digest-only A authorize the exact B occurrence"
        );
        assert!(
            !state.revision_extends(b, a),
            "the earlier A cannot make digest-only A authorize B in the other direction"
        );
        assert!(
            !state.revision_extends(a, a),
            "digest equality cannot bypass ambiguous A occurrence identity"
        );
        assert!(state.revision_extends(b, b));
    }

    #[test]
    fn replay_of_a_fully_proved_log_hashes_nothing() {
        let records = rewrite_chain(6);
        let proved = sealed(&records);
        let (replayed, hashed) = hashed_bytes(|| {
            TranscriptHistoryState::from_rewrite_records_with_proved(records.clone(), Some(&proved))
        });
        let replayed = replayed.expect("replay succeeds").expect("non-empty");
        assert_eq!(
            hashed, 0,
            "every commit in the log is carried byte-equal by the proved graph, \
             so the replay must not hash a transcript a second time"
        );
        assert_same_graph(&replayed, &proved);
    }

    #[test]
    fn replay_cost_of_one_new_record_does_not_grow_with_the_proved_prefix() {
        let hash_one_new_record = |chain_len: usize| {
            let records = rewrite_chain(chain_len);
            let proved = sealed(&records[..chain_len - 1]);
            let (replayed, hashed) = hashed_bytes(|| {
                TranscriptHistoryState::from_rewrite_records_with_proved(
                    records.clone(),
                    Some(&proved),
                )
            });
            let replayed = replayed.expect("replay succeeds").expect("non-empty");
            assert_same_graph(&replayed, &rebuild(&records));
            assert!(
                hashed > 0,
                "the trailing record is not carried by the proved graph and must \
                 be proved in full"
            );
            hashed
        };
        assert_eq!(
            hash_one_new_record(2),
            hash_one_new_record(8),
            "resume must hash the records the session cannot already prove, and \
             only those: a longer proved prefix is not more work"
        );
    }

    /// The digest is unkeyed, so what a proved replay must still refuse is a
    /// body whose bytes no longer produce its revision string — accidental
    /// corruption, not a modification anyone able to write the log could not
    /// simply re-derive a matching digest for.
    #[test]
    fn a_corrupted_body_is_rejected_when_its_commit_is_proved() {
        let records = rewrite_chain(3);
        let proved = sealed(&records);
        let mut corrupted = records.clone();
        corrupted[1].revision_body.messages[3] = message("corrupted tail");
        let error =
            TranscriptHistoryState::from_rewrite_records_with_proved(corrupted, Some(&proved))
                .expect_err("a body that does not digest to its commit must be refused");
        assert!(
            matches!(error, TranscriptEditError::HistoryStateMalformed(_)),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_corrupted_new_record_is_rejected() {
        let records = rewrite_chain(3);
        let mut corrupted = records.clone();
        corrupted[2].parent_body.messages[3] = message("corrupted tail");
        let error = TranscriptHistoryState::from_rewrite_records(corrupted)
            .expect_err("a body that does not digest to its commit must be refused");
        assert!(
            matches!(error, TranscriptEditError::HistoryStateMalformed(_)),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_proved_prefix_cannot_launder_a_corrupted_tail_record() {
        let records = rewrite_chain(2);
        let proved = sealed(&records[..1]);
        let mut corrupted = records.clone();
        corrupted[1].revision_body.messages[3] = message("corrupted tail");
        let error =
            TranscriptHistoryState::from_rewrite_records_with_proved(corrupted, Some(&proved))
                .expect_err("a record whose endpoint the proved graph dropped is not proved");
        assert!(
            matches!(error, TranscriptEditError::HistoryStateMalformed(_)),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_body_mislabelled_against_its_commit_is_rejected_under_a_proved_graph() {
        let records = rewrite_chain(3);
        let proved = sealed(&records);
        let mut mislabelled = records.clone();
        mislabelled[1].parent_body.revision = "sha256:not-the-parent".to_string();
        let error =
            TranscriptHistoryState::from_rewrite_records_with_proved(mislabelled, Some(&proved))
                .expect_err("a body labelled with a revision it does not carry must be refused");
        assert!(
            matches!(error, TranscriptEditError::HistoryStateMalformed(_)),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_proved_replay_builds_the_same_graph_as_an_unproved_one() {
        let records = rewrite_chain(5);
        let proved = sealed(&records);
        let with_proof = TranscriptHistoryState::from_rewrite_records_with_proved(
            records.clone(),
            Some(&proved),
        )
        .expect("proved replay succeeds")
        .expect("non-empty");
        assert_same_graph(&with_proof, &rebuild(&records));
        validate_transcript_history_state(&with_proof)
            .expect("the proved replay's output is itself a valid graph");
    }

    /// Current graphs never serialize the retired store-position marker.
    #[test]
    fn a_graph_does_not_serialize_the_retired_replay_cursor() {
        let state = rebuild(&rewrite_chain(3));
        let wire = serde_json::to_value(&state).expect("graph serializes");
        assert!(
            wire.get("replay_cursor").is_none(),
            "physical store position is not session-document authority: {wire}"
        );
        let decoded: TranscriptHistoryState = serde_json::from_value(wire).expect("graph decodes");
        assert_same_graph(&decoded, &state);
    }

    /// A physical log cursor belongs only to the frozen 0.8.10 importer. The
    /// current compact graph decoder refuses it rather than silently accepting
    /// store position as session-document authority.
    #[test]
    fn a_replay_cursor_is_refused_by_the_current_graph_decoder() {
        let state = rebuild(&rewrite_chain(2));
        let mut wire = serde_json::to_value(&state).expect("graph serializes");
        wire["replay_cursor"] = serde_json::json!({
            "seq": 41,
            "commits": 2,
            "last_commit_revision": &state.commit(1).expect("second commit").revision,
        });
        assert!(
            serde_json::from_value::<TranscriptHistoryState>(wire).is_err(),
            "current graph ingress must reject the retired physical cursor"
        );
    }

    #[test]
    fn current_graph_refuses_missing_parent_advance_evidence() {
        let state = rebuild(&rewrite_chain(3));
        let mut wire = serde_json::to_value(&state).expect("graph serializes");
        wire["edges"][0]
            .as_object_mut()
            .expect("edge wire is an object")
            .remove("parent_advance");
        assert!(
            serde_json::from_value::<TranscriptHistoryState>(wire).is_err(),
            "current ingress cannot synthesize missing parent-advance evidence"
        );
    }

    #[test]
    fn null_parent_advance_evidence_is_malformed() {
        let state = rebuild(&rewrite_chain(2));
        let mut wire = serde_json::to_value(&state).expect("graph serializes");
        wire["edges"][0]["parent_advance"] = serde_json::Value::Null;
        assert!(
            serde_json::from_value::<TranscriptHistoryState>(wire).is_err(),
            "null parent-advance evidence is malformed current state"
        );
    }
}
