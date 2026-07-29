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
use super::sealed::ValidatedTranscriptHistory;
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
use std::collections::{BTreeSet, HashMap};
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

/// Durable form of one retained revision inside a graph's revision chain.
///
/// The first entry of the chain is the ANCHOR and carries `messages` in full.
/// Every later entry carries `rebase` instead: the inverse splice that
/// reconstructs it from an entry that appears earlier in the same array. A
/// retained revision therefore costs the bytes of the edit that distinguishes
/// it from its neighbour, not a second copy of the whole transcript — the
/// difference between a 371-message transcript retaining 98 revisions as 98
/// full documents and retaining them as one document plus 98 splices.
///
/// Both spellings are accepted on decode. `messages` is not reserved to the
/// anchor: a producer that hands the array a self-contained body (the store's
/// evidence-rebuild paths do) stays readable, and a chain whose bases are all
/// resolvable decodes identically either way.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct RevisionEntryWire {
    revision: String,
    #[serde(default)]
    parent_revision: Option<String>,
    created_at: SystemTime,
    #[serde(default)]
    messages: Option<Vec<Message>>,
    #[serde(default)]
    rebase: Option<RevisionRebaseWire>,
}

/// The inverse splice that reconstructs one retained revision from another.
///
/// `insert` replaces the `removed` messages of `base` starting at index `at`.
/// The shared prefix (`base[..at]`) and shared suffix (`base[at + removed..]`)
/// are addressed by position and never re-serialized.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct RevisionRebaseWire {
    base: String,
    at: usize,
    removed: usize,
    #[serde(default)]
    insert: Vec<Message>,
}

/// Borrowed serialization mirror of [`RevisionEntryWire`].
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct RevisionEntryRef<'a> {
    revision: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_revision: Option<&'a str>,
    created_at: &'a SystemTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    messages: Option<&'a [Message]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rebase: Option<RevisionRebaseRef<'a>>,
}

/// Borrowed serialization mirror of [`RevisionRebaseWire`].
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct RevisionRebaseRef<'a> {
    base: &'a str,
    at: usize,
    removed: usize,
    insert: &'a [Message],
}

/// Published shape of one entry of the durable revision chain.
///
/// EXACTLY ONE of `messages` and `rebase` is present on every entry. The first
/// entry of the array is the chain anchor and carries `messages`; every later
/// entry carries `rebase` and is reconstructed by splicing it onto an entry
/// that appeared earlier in the same array. A reader that materializes the
/// array front to back always has the base in hand before it is referenced.
#[cfg(feature = "schema")]
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[schemars(rename = "TranscriptRevisionEntry")]
struct SchemaRevisionEntry {
    revision: String,
    parent_revision: Option<String>,
    created_at: SchemaSystemTime,
    /// The revision's full ordered messages. Present on the chain anchor.
    messages: Option<Vec<serde_json::Value>>,
    /// The inverse splice reconstructing this revision. Present on every
    /// entry after the anchor.
    rebase: Option<SchemaRevisionRebase>,
}

/// Published shape of one inverse splice.
///
/// `insert` replaces the `removed` messages of the entry named by `base`,
/// starting at index `at`. The shared prefix and suffix are addressed by
/// position and never carried.
#[cfg(feature = "schema")]
#[allow(dead_code)]
#[derive(schemars::JsonSchema)]
#[schemars(rename = "TranscriptRevisionRebase")]
struct SchemaRevisionRebase {
    base: String,
    at: usize,
    removed: usize,
    insert: Vec<serde_json::Value>,
}

/// The narrowest splice that turns `base` into `target`.
///
/// Returns `(at, removed, insert)` such that
/// `base[..at] ++ insert ++ base[at + removed..] == target`, with `insert`
/// borrowed straight out of `target`. Shared leading and trailing messages are
/// elided, so an edit that touches one message carries one message however
/// long the transcript is — including the pathological index-0 rewrite, which
/// shares no prefix at all but shares the entire tail.
fn minimal_splice<'a>(base: &[Message], target: &'a [Message]) -> (usize, usize, &'a [Message]) {
    let at = base
        .iter()
        .zip(target.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = base[at..]
        .iter()
        .rev()
        .zip(target[at..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    (
        at,
        base.len() - at - suffix,
        &target[at..target.len() - suffix],
    )
}

/// Encode a retained-body list as an anchor plus a chain of inverse splices.
///
/// Every entry after the first rebases onto an entry that already appeared, so
/// a decoder resolves the chain in one forward pass. The base preference —
/// an identical revision already emitted, else this body's lineage parent,
/// else the immediately preceding entry — only decides how SMALL the splice
/// is. Correctness does not depend on it: [`minimal_splice`] is computed from
/// the two message vectors themselves, so any base whatsoever yields a splice
/// that reconstructs the body exactly.
fn encode_revision_chain(revisions: &[TranscriptRevisionBody]) -> Vec<RevisionEntryRef<'_>> {
    let mut emitted: HashMap<&str, usize> = HashMap::with_capacity(revisions.len());
    let mut entries = Vec::with_capacity(revisions.len());
    for (index, body) in revisions.iter().enumerate() {
        let base_index = if index == 0 {
            None
        } else if let Some(&same) = emitted.get(body.revision.as_str()) {
            Some(same)
        } else if let Some(&parent) = body
            .parent_revision
            .as_deref()
            .and_then(|parent| emitted.get(parent))
        {
            Some(parent)
        } else {
            Some(index - 1)
        };
        let (messages, rebase) = match base_index.map(|base_index| &revisions[base_index]) {
            Some(base) => {
                let (at, removed, insert) = minimal_splice(&base.messages, &body.messages);
                (
                    None,
                    Some(RevisionRebaseRef {
                        base: &base.revision,
                        at,
                        removed,
                        insert,
                    }),
                )
            }
            None => (Some(body.messages.as_slice()), None),
        };
        emitted.entry(body.revision.as_str()).or_insert(index);
        entries.push(RevisionEntryRef {
            revision: &body.revision,
            parent_revision: body.parent_revision.as_deref(),
            created_at: &body.created_at,
            messages,
            rebase,
        });
    }
    entries
}

/// Materialize a decoded revision chain back into full retained bodies.
///
/// Returns the bodies together with whether any entry arrived as a splice —
/// the pre-parent-pointer lineage inference below is a property of the
/// all-full legacy spelling only, and must never fire on a chain whose
/// parent pointers were written explicitly.
fn decode_revision_chain<E>(
    entries: Vec<RevisionEntryWire>,
) -> Result<(Vec<TranscriptRevisionBody>, bool), E>
where
    E: serde::de::Error,
{
    let mut materialized: HashMap<String, usize> = HashMap::with_capacity(entries.len());
    let mut bodies: Vec<TranscriptRevisionBody> = Vec::with_capacity(entries.len());
    let mut spliced = false;
    for entry in entries {
        let messages = match (entry.messages, entry.rebase) {
            (Some(messages), None) => messages,
            (None, Some(rebase)) => {
                spliced = true;
                let base = materialized
                    .get(&rebase.base)
                    .and_then(|index| bodies.get(*index))
                    .ok_or_else(|| {
                        E::custom(format!(
                            "transcript revision body {} rebases on {}, which no \
                             earlier retained body materializes",
                            entry.revision, rebase.base
                        ))
                    })?
                    .messages
                    .as_slice();
                let end = rebase
                    .at
                    .checked_add(rebase.removed)
                    .filter(|end| *end <= base.len())
                    .ok_or_else(|| {
                        E::custom(format!(
                            "transcript revision body {} splices {} messages at \
                             index {} of its {}-message base {}",
                            entry.revision,
                            rebase.removed,
                            rebase.at,
                            base.len(),
                            rebase.base
                        ))
                    })?;
                let mut messages =
                    Vec::with_capacity(base.len() - rebase.removed + rebase.insert.len());
                messages.extend_from_slice(&base[..rebase.at]);
                messages.extend(rebase.insert);
                messages.extend_from_slice(&base[end..]);
                messages
            }
            (Some(_), Some(_)) => {
                return Err(E::custom(format!(
                    "transcript revision body {} carries both a full message \
                     vector and a rebase splice",
                    entry.revision
                )));
            }
            (None, None) => {
                return Err(E::custom(format!(
                    "transcript revision body {} carries neither a full message \
                     vector nor a rebase splice",
                    entry.revision
                )));
            }
        };
        let position = bodies.len();
        materialized
            .entry(entry.revision.clone())
            .or_insert(position);
        bodies.push(TranscriptRevisionBody {
            revision: entry.revision,
            parent_revision: entry.parent_revision,
            messages,
            created_at: entry.created_at,
        });
    }
    Ok((bodies, spliced))
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
        crate::checkpoint::record_rewrite_record_body_decode();
        let mut revisions = vec![wire.parent_body, wire.revision_body];
        let mut commits = vec![wire.commit];
        // Fast path: a record stamped with the current digest format skips the
        // heal outright — the heal hashes both full transcript bodies, and
        // every authoritative load decodes every record in the append-only
        // log. Unstamped records pay the probe exactly as before.
        if wire.digest_format < TRANSCRIPT_DIGEST_FORMAT_CURRENT {
            heal_legacy_revision_strings(&mut revisions, &mut commits, None)
                .map_err(serde::de::Error::custom)?;
        }
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
            // The heal above leaves current-format strings behind, so the
            // decoded value is stamped whatever the wire carried.
            digest_format: TRANSCRIPT_DIGEST_FORMAT_CURRENT,
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
            digest_format: TRANSCRIPT_DIGEST_FORMAT_CURRENT,
        })
    }
}

/// How much of the append-only rewrite log a graph has already folded and
/// reconciled with.
///
/// A rewrite record carries TWO full transcript bodies, and every authoritative
/// load read the log from sequence 1 before it could conclude the log had
/// nothing new to say. Even a reader that never materializes a body still
/// reads, lexes and copies every byte of every record to get there, so the cost
/// stayed `Theta(records x transcript)` on a session that mints one record per
/// resume. An append-only log does not need re-reading: this marks how far a
/// load already got, so the next one starts after it.
///
/// The marked position is MUTUAL, and both halves are load-bearing because the
/// log's two consumers read it in OPPOSITE directions. At stamp time every log
/// rewrite record at or below [`Self::seq`] was represented in this graph's
/// commits (what the replay needs), AND this graph's leading [`Self::commits`]
/// commits were all present in the log at or below that sequence (what the
/// audit verifier needs, since it hunts for graph commits the log never
/// recorded). A sequence alone would answer only the first, and the second
/// consumer would re-read the whole log anyway.
///
/// A compatibility convenience, not an integrity boundary. Nothing here is
/// covered by the checkpoint stamp — the whole graph value is stripped from the
/// checkpoint preimage and stands in as its witness, and neither witness format
/// reads a top-level field it does not name. So a wrong cursor cannot be caught
/// by a digest, and is instead caught by its reader: `meerkat-session` re-reads
/// from sequence 1 on any inconsistency. Wrong means SLOW, never a missed
/// record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TranscriptReplayCursor {
    /// Highest event-log sequence whose rewrite records are all represented in
    /// this graph's commits. Every one of them was fully validated when it was
    /// folded; none is re-proved.
    pub seq: u64,
    /// How many of this graph's LEADING commits were also present in the log at
    /// or below [`Self::seq`]. Only the commits after that prefix can still be
    /// missing an audit event.
    pub commits: usize,
    /// The revision of the last commit in that prefix; `None` exactly when the
    /// prefix is empty.
    ///
    /// The prefix is trusted only while the graph still ends it here, which is
    /// what keeps "commits are append-only" a CHECKED precondition rather than
    /// a silent assumption. It catches truncation, a front prune, a lineage
    /// swap and wholesale replacement.
    ///
    /// What it does NOT catch, stated so no reader infers a guarantee: a
    /// reorder strictly INTERIOR to the prefix that preserves both its length
    /// and its last element. The cost of that miss is a skipped audit-event
    /// repair — never a missed rewrite record, because the replay side is
    /// guarded separately by proving that every record read attaches to the
    /// graph. Verifying the interior would mean hashing the prefix on every
    /// load, i.e. paying for the evidence with the very quantity this cursor
    /// exists to reduce.
    pub last_commit_revision: Option<String>,
    /// Log sequence of the rewrite record that established
    /// [`Self::last_commit_revision`] — the cursor's positively-verifiable
    /// anchor in the log itself.
    ///
    /// [`Self::seq`] and the count/boundary answer questions about the GRAPH
    /// and about how far the log was observed; neither can be checked against
    /// the log without re-reading it, so an inflated `seq` whose tail happens
    /// to hold no rewrite records used to be accepted vacuously (the empty
    /// tail attaches trivially and the audit verifier's unreconciled slice is
    /// empty). This field is checkable in O(rows since the last rewrite): the
    /// reader starts its tail read AT this sequence and requires the row here
    /// to be a rewrite record whose commit revision equals
    /// [`Self::last_commit_revision`] before trusting anything else the
    /// cursor claims.
    ///
    /// `None` on documents written before this field existed. Such a cursor
    /// carries no verifiable anchor and is not trusted: the reader re-reads
    /// the whole log once and the restamp carries the anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_commit_seq: Option<u64>,
}

/// Typed session-local transcript revision graph state.
///
/// The typed value carries every retained body in full; the DURABLE form does
/// not. Serialization emits `revisions` as an anchor plus a chain of inverse
/// splices ([`encode_revision_chain`]) and deserialization materializes the
/// full bodies back ([`decode_revision_chain`]), so retaining a revision costs
/// the bytes of the edit that distinguishes it rather than a second copy of
/// the transcript. Content addressing is unaffected: a materialized body is
/// still verified against its revision string by
/// [`validate_transcript_history_state`], and a splice that reconstructs the
/// wrong messages fails exactly that check.
#[derive(Debug, Clone)]
pub struct TranscriptHistoryState {
    pub head: String,
    pub commits: Vec<TranscriptRewriteCommit>,
    pub revisions: Vec<TranscriptRevisionBody>,
    /// Digest-format generation of the revision strings. Documents stamped
    /// `>= 2` were written by the content-addressed digest format, so decode
    /// skips the per-decode legacy-heal probe (a full-transcript hash);
    /// absent/0 means unknown provenance and the probe runs once — the next
    /// save persists the marker. A compatibility convenience, not an
    /// integrity boundary (checkpoint stamps own integrity).
    pub digest_format: u32,
    /// How far the append-only rewrite log has already been folded into this
    /// graph; see [`TranscriptReplayCursor`].
    ///
    /// `None` claims nothing, and is byte-for-byte the behaviour of every
    /// document written before this field existed: read the whole log from the
    /// first sequence. Every ambiguous case resolves to `None` for that reason
    /// — a cursor that is too low costs a slow load, one that is too high costs
    /// a missed record, and those are not comparable.
    pub replay_cursor: Option<TranscriptReplayCursor>,
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
    head: String,
    #[serde(default)]
    commits: Vec<TranscriptRewriteCommit>,
    /// Chain anchor followed by inverse splices; see [`SchemaRevisionEntry`].
    /// Omitted entirely when the graph retains nothing.
    #[serde(default)]
    revisions: Vec<SchemaRevisionEntry>,
    /// Omitted when unknown (0).
    #[serde(default)]
    digest_format: u32,
    /// Omitted when the graph has not been reconciled against the append-only
    /// rewrite log; see [`TranscriptReplayCursor`].
    #[serde(default)]
    replay_cursor: Option<TranscriptReplayCursor>,
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

        let emit_commits = !self.commits.is_empty();
        let emit_revisions = !self.revisions.is_empty();
        let emit_digest_format = !digest_format_is_unknown(&self.digest_format);
        let emit_replay_cursor = self.replay_cursor.is_some();
        let fields = 1
            + usize::from(emit_commits)
            + usize::from(emit_revisions)
            + usize::from(emit_digest_format)
            + usize::from(emit_replay_cursor);
        let mut wire = serializer.serialize_struct("TranscriptHistoryState", fields)?;
        wire.serialize_field("head", &self.head)?;
        if emit_commits {
            wire.serialize_field("commits", &self.commits)?;
        }
        if emit_revisions {
            wire.serialize_field("revisions", &encode_revision_chain(&self.revisions))?;
        }
        if emit_digest_format {
            wire.serialize_field("digest_format", &self.digest_format)?;
        }
        // Absent when unclaimed, so a graph that has never been reconciled
        // against the log produces exactly the bytes it produced before this
        // field existed.
        if let Some(cursor) = &self.replay_cursor {
            wire.serialize_field("replay_cursor", cursor)?;
        }
        wire.end()
    }
}

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
            revisions: Vec<RevisionEntryWire>,
            #[serde(default)]
            digest_format: u32,
            #[serde(default)]
            replay_cursor: Option<TranscriptReplayCursor>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let (revisions, spliced) = decode_revision_chain::<D::Error>(wire.revisions)?;
        let mut state = TranscriptHistoryState {
            head: wire.head,
            commits: wire.commits,
            revisions,
            digest_format: wire.digest_format,
            replay_cursor: wire.replay_cursor,
        };
        // Pre-parent-pointer v1 snapshots serialized each body as
        // {created_at,messages,revision}. When every non-root body lacks a
        // parent, the append order is the only lineage the old format
        // carried; reconstruct that exact linear order before digest healing
        // and full validation. A spliced chain writes its parent pointers
        // explicitly, so their absence there is a fact about lineage, not a
        // gap the append order may fill.
        if !spliced
            && state.revisions.len() > 1
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
                replay_cursor,
            } = &mut state;
            // The heal rewrites revision strings, so a prefix boundary recorded
            // under the old spelling no longer names anything in this graph.
            // Drop the claim rather than carry a stale one: the reader falls
            // back to a full read and re-establishes it.
            *replay_cursor = None;
            heal_legacy_revision_strings(revisions, commits, Some(head))
                .map_err(serde::de::Error::custom)?;
        }
        heal_legacy_compaction_rewrite_semantics(&mut state.commits, &state.revisions);
        // `heal_legacy_compaction_rewrite_semantics` can rewrite a commit's
        // selection semantics in place. It preserves order and length, so the
        // prefix boundary still names the same commit and the cursor survives;
        // the audit verifier compares whole commits, so a healed commit that no
        // longer equals its logged form simply reads as unreconciled and takes
        // the repair path.
        Ok(state)
    }
}

impl TranscriptHistoryState {
    /// The cursor to stamp on this graph once a load has reconciled it with the
    /// append-only log through `seq`.
    ///
    /// Callers must have established BOTH directions in that same load before
    /// using this: every log record at or below `seq` folded into this graph,
    /// and every one of this graph's commits present in the log. Stamping on
    /// anything weaker is the one way this mechanism can lose a record.
    ///
    /// `last_commit_seq` is the log sequence of the rewrite record whose
    /// commit is this graph's LAST commit, positively identified in that same
    /// load — the anchor the next load verifies against the log before
    /// trusting the cursor. Stamping a sequence whose row is not that record
    /// only costs the next load a full read (the anchor check refuses).
    #[must_use]
    pub fn replay_cursor_at(&self, seq: u64, last_commit_seq: u64) -> TranscriptReplayCursor {
        TranscriptReplayCursor {
            seq,
            commits: self.commits.len(),
            last_commit_revision: self.commits.last().map(|commit| commit.revision.clone()),
            last_commit_seq: Some(last_commit_seq),
        }
    }

    /// The commits `cursor` does NOT already prove are recorded in the log.
    ///
    /// `None` means the cursor does not describe this graph and nothing may be
    /// skipped on its word — the caller must read the whole log, exactly as it
    /// did before cursors existed. That is the self-healing path, and it fires
    /// on a graph that lost commits, a prefix that no longer ends where the
    /// cursor says, and a cursor minted against a different lineage.
    #[must_use]
    pub fn commits_beyond_replay_cursor(
        &self,
        cursor: &TranscriptReplayCursor,
    ) -> Option<&[TranscriptRewriteCommit]> {
        let boundary_holds = match cursor.commits.checked_sub(1) {
            None => cursor.last_commit_revision.is_none(),
            Some(last) => {
                cursor.last_commit_revision.as_deref()
                    == self
                        .commits
                        .get(last)
                        .map(|commit| commit.revision.as_str())
            }
        };
        boundary_holds
            .then(|| self.commits.get(cursor.commits..))
            .flatten()
    }

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
            //
            // The replay cursor is deliberately NOT substituted: it is
            // reader-side log state that `validate_transcript_history_state`
            // never proves, and the shape key deliberately does not bind it
            // (two documents differing ONLY in cursor must share an entry).
            // Substituting the memo entry's cursor would resurrect another
            // document's log position onto this one — a stale or foreign
            // claim that could skip unseen rewrite records indefinitely — so
            // the incoming document's own claim is preserved and left to the
            // reader-side cursor verification, exactly as on a memo miss.
            let replay_cursor = self.replay_cursor.take();
            *self = TranscriptHistoryState::clone(&proved);
            self.replay_cursor = replay_cursor;
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
    proved: Option<&ValidatedTranscriptHistory>,
    record: &TranscriptRewriteRecord,
) -> bool {
    let Some(proved) = proved else {
        return false;
    };
    if record.parent_body.revision != record.commit.parent_revision
        || record.revision_body.revision != record.commit.revision
    {
        return false;
    }
    if !proved.commits.contains(&record.commit) {
        return false;
    }
    let retained = |revision: &str| {
        proved
            .revisions
            .iter()
            .find(|body| body.revision == revision)
    };
    let (Some(parent), Some(revision)) = (
        retained(&record.commit.parent_revision),
        retained(&record.commit.revision),
    ) else {
        return false;
    };
    parent.messages == record.parent_body.messages
        && revision.messages == record.revision_body.messages
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
        let mut state: Option<Self> = None;
        for record in records {
            if !record_is_proved_by(proved, &record) {
                validate_transcript_rewrite_record(
                    &record.commit,
                    &record.parent_body,
                    &record.revision_body,
                )?;
            }
            let state = state.get_or_insert_with(|| Self {
                head: record.commit.parent_revision.clone(),
                commits: Vec::new(),
                revisions: Vec::new(),
                digest_format: TRANSCRIPT_DIGEST_FORMAT_CURRENT,
                replay_cursor: None,
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::redundant_clone)]
mod tests {
    use super::*;
    use crate::checkpoint::session_content_digest_bytes;
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

    /// A chain of `count` rewrites over a fixed-length transcript, each
    /// replacing message 0 — the shape a resume-time system-prompt refresh
    /// mints, and the shape whose per-record proof hashes the whole transcript
    /// on both sides of a one-message edit.
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
                parent_revision: parent_body.revision.clone(),
                revision: revision_body.revision.clone(),
                selection: TranscriptRewriteSelection::MessageRange { start: 0, end: 1 },
                original_span_digest: transcript_messages_digest(&parent_body.messages[..1])
                    .expect("digest original span"),
                replacement_digest: transcript_messages_digest(&revision_body.messages[..1])
                    .expect("digest replacement span"),
                messages_before: parent_body.messages.len(),
                messages_after: revision_body.messages.len(),
                reason: TranscriptRewriteReason::new("resume-system-prompt-refresh"),
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
        assert_eq!(left.head, right.head);
        assert_eq!(left.commits, right.commits);
        assert_eq!(
            left.revisions
                .iter()
                .map(|body| (&body.revision, &body.messages))
                .collect::<Vec<_>>(),
            right
                .revisions
                .iter()
                .map(|body| (&body.revision, &body.messages))
                .collect::<Vec<_>>()
        );
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
    fn a_proved_graph_missing_an_endpoint_body_cannot_launder_a_corrupted_record() {
        let records = rewrite_chain(2);
        let mut state = rebuild(&records);
        state
            .revisions
            .retain(|body| body.revision != records[1].commit.revision);
        // `seal` refuses a graph missing a commit endpoint, so a marker adopted
        // over a graph that lost one is the only way this branch is reachable.
        let proved = ValidatedTranscriptHistory::adopt_session_validated(Arc::new(state));
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

    /// The wire form a pre-marker writer produced: the same record, with
    /// `digest_format` absent.
    fn unstamped_wire(record: &TranscriptRewriteRecord) -> serde_json::Value {
        let mut wire = serde_json::to_value(record).expect("record serializes");
        wire.as_object_mut()
            .expect("record wire form is an object")
            .remove("digest_format");
        wire
    }

    #[test]
    fn a_stamped_record_serializes_its_marker_and_an_unknown_one_omits_it() {
        let record = rewrite_chain(1).remove(0);
        let wire = serde_json::to_value(&record).expect("record serializes");
        assert_eq!(
            wire.get("digest_format")
                .and_then(serde_json::Value::as_u64),
            Some(u64::from(TRANSCRIPT_DIGEST_FORMAT_CURRENT)),
            "a minted record carries the digest-format marker"
        );
        let unknown = TranscriptRewriteRecord {
            digest_format: 0,
            ..record
        };
        assert!(
            serde_json::to_value(&unknown)
                .expect("record serializes")
                .get("digest_format")
                .is_none(),
            "an unknown-provenance record must keep producing the pre-marker bytes"
        );
    }

    #[test]
    fn a_record_without_a_marker_decodes_to_the_same_value_as_a_stamped_one() {
        let record = rewrite_chain(1).remove(0);
        let unstamped: TranscriptRewriteRecord =
            serde_json::from_value(unstamped_wire(&record)).expect("pre-marker record decodes");
        assert_eq!(unstamped.commit, record.commit);
        assert_eq!(unstamped.parent_body.messages, record.parent_body.messages);
        assert_eq!(unstamped.parent_body.revision, record.parent_body.revision);
        assert_eq!(
            unstamped.revision_body.messages,
            record.revision_body.messages
        );
        assert_eq!(
            unstamped.revision_body.revision,
            record.revision_body.revision
        );
    }

    #[test]
    fn only_an_unmarked_record_pays_the_legacy_heal_probe() {
        let record = rewrite_chain(1).remove(0);
        let stamped = serde_json::to_value(&record).expect("record serializes");
        let unstamped = unstamped_wire(&record);

        let (_, unstamped_hashed) = hashed_bytes(|| {
            serde_json::from_value::<TranscriptRewriteRecord>(unstamped)
                .expect("pre-marker record decodes")
        });
        let (_, stamped_hashed) = hashed_bytes(|| {
            serde_json::from_value::<TranscriptRewriteRecord>(stamped)
                .expect("stamped record decodes")
        });
        assert!(
            unstamped_hashed > 0,
            "an unmarked record's provenance is unknown, so decode must still \
             probe both bodies exactly as it did before the marker existed"
        );
        assert_eq!(
            stamped_hashed, 0,
            "a record stamped with the current digest format must not hash its \
             two transcript bodies on every decode: unmarked hashed \
             {unstamped_hashed} bytes, stamped hashed {stamped_hashed}"
        );
    }

    #[test]
    fn an_unmarked_record_still_heals_legacy_revision_strings() {
        use super::super::heal::legacy_transcript_messages_digest;

        let record = rewrite_chain(1).remove(0);
        let legacy_parent = legacy_transcript_messages_digest(&record.parent_body.messages)
            .expect("legacy parent digest");
        let legacy_revision = legacy_transcript_messages_digest(&record.revision_body.messages)
            .expect("legacy revision digest");
        // A pre-0.7.14 writer's bytes: bookkeeping-inclusive revision strings,
        // no marker to say which format minted them.
        let mut wire = unstamped_wire(&record);
        wire["commit"]["parent_revision"] = legacy_parent.clone().into();
        wire["commit"]["revision"] = legacy_revision.clone().into();
        wire["commit"]["original_span_digest"] =
            legacy_transcript_messages_digest(&record.parent_body.messages[..1])
                .expect("legacy original span digest")
                .into();
        wire["commit"]["replacement_digest"] =
            legacy_transcript_messages_digest(&record.revision_body.messages[..1])
                .expect("legacy replacement span digest")
                .into();
        wire["parent_body"]["revision"] = legacy_parent.clone().into();
        wire["revision_body"]["revision"] = legacy_revision.into();
        wire["revision_body"]["parent_revision"] = legacy_parent.into();

        let healed: TranscriptRewriteRecord =
            serde_json::from_value(wire).expect("legacy record decodes");
        assert_eq!(healed.commit.parent_revision, record.commit.parent_revision);
        assert_eq!(healed.commit.revision, record.commit.revision);
        assert_eq!(
            healed.commit.original_span_digest,
            record.commit.original_span_digest
        );
        assert_eq!(
            healed.commit.replacement_digest,
            record.commit.replacement_digest
        );
        validate_transcript_rewrite_record(
            &healed.commit,
            &healed.parent_body,
            &healed.revision_body,
        )
        .expect("the healed record validates against the current digest format");
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

    // -----------------------------------------------------------------
    // Replay cursor
    // -----------------------------------------------------------------

    #[test]
    fn a_cursor_survives_the_hand_written_round_trip() {
        let mut state = rebuild(&rewrite_chain(3));
        state.replay_cursor = Some(state.replay_cursor_at(41, 40));
        let decoded: TranscriptHistoryState =
            serde_json::from_value(serde_json::to_value(&state).expect("graph serializes"))
                .expect("graph decodes");
        assert_eq!(
            decoded.replay_cursor, state.replay_cursor,
            "the cursor is threaded through FOUR hand-written impls; missing one \
             loses it silently on the next save"
        );
    }

    /// The compatibility contract: a graph that claims nothing must produce the
    /// exact bytes it produced before the field existed, and decode unchanged.
    #[test]
    fn a_graph_without_a_cursor_is_byte_identical_to_the_pre_cursor_form() {
        let state = rebuild(&rewrite_chain(3));
        assert!(state.replay_cursor.is_none());
        let wire = serde_json::to_value(&state).expect("graph serializes");
        assert!(
            wire.get("replay_cursor").is_none(),
            "an unclaimed cursor must not appear on the wire at all: {wire}"
        );
        let decoded: TranscriptHistoryState = serde_json::from_value(wire).expect("graph decodes");
        assert_same_graph(&decoded, &state);
        assert_eq!(decoded.replay_cursor, None);
    }

    /// A document written before either marker existed.
    #[test]
    fn a_pre_marker_document_decodes_with_neither_marker_claimed() {
        let state = rebuild(&rewrite_chain(2));
        let mut wire = serde_json::to_value(&state).expect("graph serializes");
        let object = wire.as_object_mut().expect("graph wire form is an object");
        object.remove("digest_format");
        object.remove("replay_cursor");
        let decoded: TranscriptHistoryState =
            serde_json::from_value(wire).expect("pre-marker graph decodes");
        assert_same_graph(&decoded, &state);
        assert_eq!(
            decoded.replay_cursor, None,
            "an absent cursor claims nothing, so the next load reads the whole log"
        );
    }

    #[test]
    fn a_cursor_admits_exactly_the_commits_after_its_prefix() {
        let state = rebuild(&rewrite_chain(4));
        let cursor = state.replay_cursor_at(9, 8);
        assert_eq!(cursor.commits, 4);
        assert_eq!(
            state
                .commits_beyond_replay_cursor(&cursor)
                .expect("a cursor minted from this graph describes it")
                .len(),
            0,
            "a cursor stamped over the whole commit list leaves nothing unreconciled"
        );

        let earlier = TranscriptReplayCursor {
            seq: 4,
            commits: 2,
            last_commit_revision: Some(state.commits[1].revision.clone()),
            last_commit_seq: Some(3),
        };
        let beyond = state
            .commits_beyond_replay_cursor(&earlier)
            .expect("the prefix still ends where the cursor says");
        assert_eq!(
            beyond,
            &state.commits[2..],
            "only the commits the cursor never reconciled may need an audit event"
        );
    }

    /// The self-healing precondition. Each of these must report "cannot be
    /// trusted" rather than silently admitting a prefix that is not the one the
    /// cursor reconciled — the caller then re-reads the whole log.
    #[test]
    fn a_cursor_that_does_not_describe_the_graph_is_refused() {
        let state = rebuild(&rewrite_chain(3));
        let valid = state.replay_cursor_at(7, 6);

        let too_high = TranscriptReplayCursor {
            commits: valid.commits + 1,
            ..valid.clone()
        };
        assert!(
            state.commits_beyond_replay_cursor(&too_high).is_none(),
            "a prefix longer than the graph's commit list describes some other graph"
        );

        let wrong_boundary = TranscriptReplayCursor {
            last_commit_revision: Some("sha256:not-this-commit".to_string()),
            ..valid.clone()
        };
        assert!(
            state
                .commits_beyond_replay_cursor(&wrong_boundary)
                .is_none(),
            "the prefix no longer ends at the commit the cursor reconciled"
        );

        let claims_empty_prefix = TranscriptReplayCursor {
            commits: 0,
            last_commit_revision: Some(state.commits[0].revision.clone()),
            ..valid.clone()
        };
        assert!(
            state
                .commits_beyond_replay_cursor(&claims_empty_prefix)
                .is_none(),
            "an empty prefix cannot also name a boundary commit"
        );

        let empty_prefix = TranscriptReplayCursor {
            seq: 1,
            commits: 0,
            last_commit_revision: None,
            last_commit_seq: None,
        };
        assert_eq!(
            state
                .commits_beyond_replay_cursor(&empty_prefix)
                .expect("an honestly empty prefix is describable"),
            state.commits.as_slice(),
            "a cursor that reconciled nothing leaves every commit to check"
        );
    }

    /// A validated-memo hit substitutes the PROVEN post-prune graph — but the
    /// replay cursor is reader-side log state the proof never covered, and
    /// the shape key deliberately does not bind it (two documents differing
    /// only in cursor share an entry). The substitution must keep the
    /// INCOMING document's cursor: resurrecting the memoized document's
    /// cursor would graft a stale or foreign log position onto a different
    /// document, which the reader-side cursor checks cannot always
    /// distinguish from an honest one.
    ///
    /// (Under `MEERKAT_DISABLE_GRAPH_DECODE_MEMO` the second decode misses
    /// and re-validates, which also preserves the incoming cursor — the
    /// assertion holds on both paths; the memo path is the one that used to
    /// fail it.)
    #[test]
    fn a_memo_hit_keeps_the_incoming_documents_cursor() {
        let records = rewrite_chain(2);
        let mut first = rebuild(&records);
        first.replay_cursor = Some(first.replay_cursor_at(40, 39));

        let mut second = first.clone();
        let incoming = second.replay_cursor_at(7, 6);
        second.replay_cursor = Some(incoming.clone());

        first
            .compact_mechanical_revision_bodies_for(TranscriptGraphValidationMode::DecodeMemoized)
            .expect("first decode validates in full and records the proven graph");
        second
            .compact_mechanical_revision_bodies_for(TranscriptGraphValidationMode::DecodeMemoized)
            .expect("second decode substitutes the memoized graph");
        assert_eq!(
            second.replay_cursor,
            Some(incoming),
            "a memo hit must serve the proven CONTENT but keep the incoming \
             document's own replay cursor, never the recorded document's"
        );

        // The unclaiming direction too: a document with NO cursor must not
        // gain one from the memo — `None` means "read the whole log", and
        // granting it a foreign position would skip records outright.
        let mut third = first.clone();
        third.replay_cursor = None;
        third
            .compact_mechanical_revision_bodies_for(TranscriptGraphValidationMode::DecodeMemoized)
            .expect("third decode substitutes the memoized graph");
        assert_eq!(
            third.replay_cursor, None,
            "a cursorless document must stay cursorless across a memo hit"
        );
    }

    /// The heal rewrites revision strings, so a prefix boundary minted under
    /// the old spelling names nothing in the healed graph. Carrying it would be
    /// a claim about commits that no longer exist under those names.
    #[test]
    fn the_legacy_heal_drops_a_cursor_it_would_invalidate() {
        use super::super::heal::legacy_transcript_messages_digest;

        let records = rewrite_chain(1);
        let state = rebuild(&records);
        let head_index = state
            .revisions
            .iter()
            .position(|body| body.revision == state.head)
            .expect("head body retained");
        let legacy_head = legacy_transcript_messages_digest(&state.revisions[head_index].messages)
            .expect("legacy head digest");
        let mut wire = serde_json::to_value(&state).expect("graph serializes");
        // A pre-0.7.14 writer's spelling: the head body carries a
        // bookkeeping-inclusive revision string, and the graph's head names it.
        // The decode probe re-digests that body under the current algorithm,
        // sees the mismatch, and heals.
        wire["head"] = legacy_head.clone().into();
        wire["revisions"][head_index]["revision"] = legacy_head.into();
        wire["digest_format"] = serde_json::Value::from(0);
        wire["replay_cursor"] = serde_json::json!({
            "seq": 12,
            "commits": 1,
            "last_commit_revision": state.commits[0].revision,
        });

        let decoded: TranscriptHistoryState =
            serde_json::from_value(wire).expect("legacy graph decodes");
        assert_eq!(
            decoded.replay_cursor, None,
            "a healed graph must not carry a cursor minted against the pre-heal \
             revision strings"
        );
    }
}
