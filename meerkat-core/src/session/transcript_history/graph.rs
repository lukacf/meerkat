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

fn digest_format_is_unknown(format: u32) -> bool {
    format == 0
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
        let emit_digest_format = !digest_format_is_unknown(self.digest_format);
        let fields = 1
            + usize::from(emit_commits)
            + usize::from(emit_revisions)
            + usize::from(emit_digest_format);
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
        }
        let wire = Wire::deserialize(deserializer)?;
        let (revisions, spliced) = decode_revision_chain::<D::Error>(wire.revisions)?;
        let mut state = TranscriptHistoryState {
            head: wire.head,
            commits: wire.commits,
            revisions,
            digest_format: wire.digest_format,
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
