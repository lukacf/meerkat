//! Process-lifetime bounded decode memos for transcript graphs.
//!
//! Extracted verbatim from `session.rs`; the extraction commit changes
//! no behaviour, only where the code lives.

use super::graph::{TranscriptHistoryState, TranscriptRevisionBody, TranscriptRewriteCommit};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, OnceLock};

/// Decode-memo fact: the retained head body's content digest equals the
/// stored head revision string (the legacy heal probe found nothing to heal).
///
/// This entry is a verdict, but NOT an integrity boundary: a wrongly
/// memoized "current" only skips `heal_legacy_revision_strings`, after which
/// the decode path either substitutes a proven graph from the validated memo
/// or runs [`validate_transcript_history_state`](super::validate::validate_transcript_history_state) in full, which
/// independently re-proves every body digest.
pub(super) const TRANSCRIPT_GRAPH_FACT_HEAL_PROBE_CURRENT: u8 = 1;
/// Decode-memo fact tag for the validated-graph memo. Unlike the heal
/// probe, an entry under this tag is not a verdict about the incoming
/// bytes: a hit SUBSTITUTES the graph object that
/// [`validate_transcript_history_state`](super::validate::validate_transcript_history_state) proved, so a key collision serves
/// content that satisfies exactly the digests the key binds instead of
/// blessing content nobody checked.
pub(super) const TRANSCRIPT_GRAPH_FACT_VALIDATED: u8 = 2;

/// Cheap structural identity of a transcript revision graph for the
/// process-lifetime decode memos.
///
/// The key pins everything a substituted graph value may DIFFER in even when
/// every content digest matches: the digest-format generation, the head
/// revision string, every retained body's content-addressed revision string,
/// parent pointer, `created_at`, and message count, the full serialized
/// commit log (span digests, selection bounds, counts), and — critically —
/// the per-message fields transcript-digest canonicalization deliberately
/// ERASES before hashing (`erase_message_construction_bookkeeping` /
/// `canonicalize_digest_image_blocks` in `session.rs`): each message's
/// [`TranscriptMessageIdentity`] (run/interaction/objective ids), its
/// `created_at`, and each image block's inline-vs-blob representation form.
/// Without those, two documents proving the same revision strings could
/// still differ in run provenance or storage form, and a memo hit would
/// substitute one document's provenance onto another invisibly to every
/// later digest/witness check (they canonicalize the same way).
///
/// Canonicalized message CONTENT stays unpinned: the revision strings are
/// content addresses over it, and inline image bytes are bound transitively
/// through their content-addressed blob ids. Hashing here is O(graph
/// structure + erased bookkeeping), never O(message content), and
/// deliberately does not count as a content-digest computation. The `fact`
/// tag namespaces the two memos so one can never satisfy a consult for the
/// other.
pub(super) fn transcript_graph_shape_key(
    fact: u8,
    digest_format: u32,
    head: &str,
    commits: &[TranscriptRewriteCommit],
    revisions: &[TranscriptRevisionBody],
) -> Option<String> {
    let mut hasher = Sha256::new();
    hasher.update([fact]);
    hasher.update((digest_format as u64).to_le_bytes());
    hasher.update((head.len() as u64).to_le_bytes());
    hasher.update(head.as_bytes());
    hasher.update((revisions.len() as u64).to_le_bytes());
    for body in revisions {
        hasher.update((body.revision.len() as u64).to_le_bytes());
        hasher.update(body.revision.as_bytes());
        match body.parent_revision.as_deref() {
            Some(parent) => {
                hasher.update([1]);
                hasher.update((parent.len() as u64).to_le_bytes());
                hasher.update(parent.as_bytes());
            }
            None => hasher.update([0]),
        }
        let created_at = serde_json::to_vec(&body.created_at).ok()?;
        hasher.update((created_at.len() as u64).to_le_bytes());
        hasher.update(&created_at);
        hasher.update((body.messages.len() as u64).to_le_bytes());
        for message in &body.messages {
            hash_digest_erased_message_fields(&mut hasher, message)?;
        }
    }
    hasher.update((commits.len() as u64).to_le_bytes());
    for commit in commits {
        let bytes = serde_json::to_vec(commit).ok()?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(2 + digest.len() * 2);
    out.push(char::from(b'0' + fact));
    out.push(':');
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Some(out)
}

/// Pin the per-message fields the transcript digest deliberately erases, so
/// a memo key can never collide across documents that differ ONLY in those
/// fields. Must stay the exact complement of
/// `erase_message_construction_bookkeeping` +
/// `canonicalize_digest_image_blocks` in `session.rs`; the coupling is
/// pinned by `shape_key_pins_digest_erased_fields` in the decode-memo tests.
fn hash_digest_erased_message_fields(hasher: &mut Sha256, message: &crate::Message) -> Option<()> {
    use crate::Message;
    fn hash_time(hasher: &mut Sha256, at: &crate::types::MessageTimestamp) -> Option<()> {
        let bytes = serde_json::to_vec(at).ok()?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
        Some(())
    }
    fn hash_identity(
        hasher: &mut Sha256,
        identity: &crate::types::TranscriptMessageIdentity,
    ) -> Option<()> {
        let bytes = serde_json::to_vec(identity).ok()?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
        Some(())
    }
    fn hash_image_forms(hasher: &mut Sha256, blocks: &[crate::types::ContentBlock]) {
        for block in blocks {
            if let crate::types::ContentBlock::Image { data, .. } = block {
                match data {
                    crate::types::ImageData::Inline { .. } => hasher.update([b'i']),
                    crate::types::ImageData::Blob { .. } => hasher.update([b'b']),
                }
            }
        }
    }
    match message {
        Message::System(system) => {
            hasher.update([1]);
            hash_time(hasher, &system.created_at)?;
        }
        Message::SystemNotice(notice) => {
            hasher.update([2]);
            hash_time(hasher, &notice.created_at)?;
            for block in &notice.blocks {
                match block {
                    crate::types::SystemNoticeBlock::Comms { content, .. }
                    | crate::types::SystemNoticeBlock::ExternalEvent { content, .. } => {
                        hash_image_forms(hasher, content);
                    }
                    _ => {}
                }
            }
        }
        Message::User(user) => {
            hasher.update([3]);
            hash_identity(hasher, &user.identity)?;
            hash_time(hasher, &user.created_at)?;
            hash_image_forms(hasher, &user.content);
        }
        Message::BlockAssistant(assistant) => {
            hasher.update([4]);
            hash_identity(hasher, &assistant.identity)?;
            hash_time(hasher, &assistant.created_at)?;
        }
        Message::ToolResults {
            created_at,
            results,
            ..
        } => {
            hasher.update([5]);
            hash_time(hasher, created_at)?;
            for result in results {
                hash_image_forms(hasher, &result.content);
            }
        }
    }
    Some(())
}

/// Process-lifetime bounded FIFO memo keyed by transcript-graph shape.
///
/// One implementation serves both decode-path facts: the heal-probe memo
/// stores `()` (a pure verdict), the validated memo stores the proven
/// post-prune graph (substituted wholesale on a hit). Marker-less documents
/// (written by pre-marker code) and every decoded document's graph
/// validation otherwise pay a full canonical-JSON + SHA-256 pass over
/// retained transcript bodies on EVERY decode — O(document) work per repeat
/// load of unchanged bytes. Admission requires full validity: either one
/// complete verification (the decode path, the FullVerify producer seams)
/// or a typed construction-plus-content proof whose conjunction IS full
/// validity (the append and rewrite fast paths, which fall through to
/// FullVerify for any shape their O(1) proofs cannot cover). A cold reader
/// of bytes nobody admitted still hashes in full, and changed structure
/// re-keys the memo (digest format, revision strings, counts, parents,
/// created_at, or commit bytes change) and re-verifies. Bounded FIFO
/// eviction only forces a redundant re-verification. Write and typed
/// mutation seams never CONSULT these memos.
struct BoundedTranscriptGraphMemo<V> {
    capacity: usize,
    entries: HashMap<String, V>,
    order: VecDeque<String>,
}

impl<V> BoundedTranscriptGraphMemo<V> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: &str) -> Option<&V> {
        self.entries.get(key)
    }

    fn record(&mut self, key: String, value: V) {
        if self.entries.contains_key(&key) {
            return;
        }
        while self.entries.len() >= self.capacity {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&evicted);
        }
        self.order.push_back(key.clone());
        self.entries.insert(key, value);
    }
}

const TRANSCRIPT_GRAPH_HEAL_PROBE_MEMO_CAPACITY: usize = 4096;

static TRANSCRIPT_GRAPH_HEAL_PROBE_MEMO: OnceLock<Mutex<BoundedTranscriptGraphMemo<()>>> =
    OnceLock::new();

fn transcript_graph_heal_probe_memo() -> &'static Mutex<BoundedTranscriptGraphMemo<()>> {
    TRANSCRIPT_GRAPH_HEAL_PROBE_MEMO.get_or_init(|| {
        Mutex::new(BoundedTranscriptGraphMemo::new(
            TRANSCRIPT_GRAPH_HEAL_PROBE_MEMO_CAPACITY,
        ))
    })
}

/// Byte budget for the validated-graph memo. Entries retain whole transcript
/// graphs by `Arc`, and production documents on record reach 14-94 MB — an
/// entry-COUNT bound of 32 would nominally retain 448 MB-3 GB and convert a
/// CPU optimization into deterministic OOM pressure. Bounding by retained
/// bytes keeps ~18 typical (14 MB) graphs or ~2 pathological (94 MB) ones;
/// eviction only costs a re-validation.
const TRANSCRIPT_GRAPH_VALIDATED_MEMO_BYTE_BUDGET: usize = 256 * 1024 * 1024;
/// An entry larger than half the budget would monopolize the memo for
/// marginal hit value; skip it (the graph simply re-validates each decode).
const TRANSCRIPT_GRAPH_VALIDATED_MEMO_ENTRY_CAP: usize =
    TRANSCRIPT_GRAPH_VALIDATED_MEMO_BYTE_BUDGET / 2;

/// Byte-budgeted LRU over `Arc`-retained validated graphs.
///
/// `get` promotes; `record` measures the entry by its serialized size (paid
/// once, on the same decode that just paid a full validation), evicts
/// least-recently-used entries until the budget holds, and skips oversized
/// graphs entirely.
struct ByteBudgetedTranscriptGraphMemo {
    budget_bytes: usize,
    entry_cap_bytes: usize,
    retained_bytes: usize,
    entries: HashMap<String, (Arc<TranscriptHistoryState>, usize)>,
    order: VecDeque<String>,
}

impl ByteBudgetedTranscriptGraphMemo {
    fn new(budget_bytes: usize, entry_cap_bytes: usize) -> Self {
        Self {
            budget_bytes,
            entry_cap_bytes,
            retained_bytes: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&mut self, key: &str) -> Option<Arc<TranscriptHistoryState>> {
        let (state, _) = self.entries.get(key)?;
        let state = Arc::clone(state);
        if let Some(position) = self.order.iter().position(|entry| entry == key) {
            let entry = self.order.remove(position);
            if let Some(entry) = entry {
                self.order.push_back(entry);
            }
        }
        Some(state)
    }

    fn record(&mut self, key: String, value: Arc<TranscriptHistoryState>, bytes: usize) {
        if bytes > self.entry_cap_bytes || self.entries.contains_key(&key) {
            return;
        }
        while self.retained_bytes + bytes > self.budget_bytes {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            if let Some((_, evicted_bytes)) = self.entries.remove(&evicted) {
                self.retained_bytes = self.retained_bytes.saturating_sub(evicted_bytes);
            }
        }
        self.order.push_back(key.clone());
        self.retained_bytes += bytes;
        self.entries.insert(key, (value, bytes));
    }
}

static TRANSCRIPT_GRAPH_VALIDATED_MEMO: OnceLock<Mutex<ByteBudgetedTranscriptGraphMemo>> =
    OnceLock::new();

fn transcript_graph_validated_memo() -> &'static Mutex<ByteBudgetedTranscriptGraphMemo> {
    TRANSCRIPT_GRAPH_VALIDATED_MEMO.get_or_init(|| {
        Mutex::new(ByteBudgetedTranscriptGraphMemo::new(
            TRANSCRIPT_GRAPH_VALIDATED_MEMO_BYTE_BUDGET,
            TRANSCRIPT_GRAPH_VALIDATED_MEMO_ENTRY_CAP,
        ))
    })
}

/// Whether the heal probe already found this exact graph shape current on
/// this process's decode path. A poisoned lock degrades to "not cached":
/// the caller re-probes.
///
/// Setting `MEERKAT_DISABLE_GRAPH_DECODE_MEMO` (any value) forces every
/// lookup on BOTH decode memos to miss, reproducing the pre-memo decode
/// cost. It is a diagnostic kill-switch with exactly two uses: red-first
/// verification of the e2e gates that assert these memos absorb repeat
/// decodes (see the marker-less resume-cost assertion in
/// `meerkat-mob/tests/smoke_mob_idle_burn.rs`), and ruling the memos in or
/// out when stale memoized trust is suspected. It must never be set in
/// production — it restores the O(document)-per-decode verification cost
/// these memos exist to remove.
pub(super) fn transcript_graph_heal_probe_is_memoized(key: &str) -> bool {
    if std::env::var_os("MEERKAT_DISABLE_GRAPH_DECODE_MEMO").is_some() {
        return false;
    }
    transcript_graph_heal_probe_memo()
        .lock()
        .map(|memo| memo.get(key).is_some())
        .unwrap_or(false)
}

/// Record one completed heal-probe proof of this exact graph shape.
pub(super) fn record_transcript_graph_heal_probe(key: String) {
    if let Ok(mut memo) = transcript_graph_heal_probe_memo().lock() {
        memo.record(key, ());
    }
}

/// The post-prune graph [`validate_transcript_history_state`](super::validate::validate_transcript_history_state) proved for
/// this exact graph shape, if one was recorded on this process's decode
/// path. Honors the same `MEERKAT_DISABLE_GRAPH_DECODE_MEMO` kill-switch as
/// the heal probe; a poisoned lock degrades to a miss.
pub(super) fn memoized_validated_transcript_graph(
    key: &str,
) -> Option<Arc<TranscriptHistoryState>> {
    if std::env::var_os("MEERKAT_DISABLE_GRAPH_DECODE_MEMO").is_some() {
        return None;
    }
    transcript_graph_validated_memo()
        .lock()
        .ok()
        .and_then(|mut memo| memo.get(key))
}

/// Record the post-prune graph one completed full validation proved for
/// this exact graph shape. The entry is measured by its serialized size —
/// paid once, on the decode that just paid a full validation — so the memo's
/// byte budget bounds real retention, not entry counts. Sizing streams
/// through a counting writer instead of materializing the document bytes:
/// the size feeds a memory BUDGET only, never an integrity boundary, so an
/// allocation-free count is sufficient.
pub(super) fn record_validated_transcript_graph(key: String, state: Arc<TranscriptHistoryState>) {
    let Some(bytes) = approximate_serialized_bytes(state.as_ref()) else {
        return;
    };
    if let Ok(mut memo) = transcript_graph_validated_memo().lock() {
        memo.record(key, state, bytes);
    }
}

/// Admit a graph a PRODUCER just proved (or extended under a typed inductive
/// proof) and is about to persist, so the next decode of those exact bytes
/// substitutes the proven value instead of paying a full re-validation.
///
/// Semantic rule: memoizing a graph is strictly less consequential than
/// persisting it — if the producer's proof were unsound, the unsound graph
/// reaches disk regardless, so the memo introduces no failure mode that
/// persistence does not already accept. A cold reader (restart, other
/// process, other host) still validates fully.
///
/// The key MUST come from [`transcript_graph_shape_key`] under
/// [`TRANSCRIPT_GRAPH_FACT_VALIDATED`] — same fact tag, same key function as
/// the decode-side recorder — so producer and consumer can never drift.
/// `approx_bytes` is a memory-budget estimate (callers size it from the
/// serialized `Value` they already hold), never an integrity input. Honors
/// the `MEERKAT_DISABLE_GRAPH_DECODE_MEMO` kill-switch so disabling the memo
/// reproduces the pre-memo decode cost end to end.
pub(in crate::session) fn record_producer_validated_transcript_graph(
    state: Arc<TranscriptHistoryState>,
    approx_bytes: usize,
) {
    if std::env::var_os("MEERKAT_DISABLE_GRAPH_DECODE_MEMO").is_some() {
        return;
    }
    let Some(key) = transcript_graph_shape_key(
        TRANSCRIPT_GRAPH_FACT_VALIDATED,
        state.digest_format,
        &state.head,
        &state.commits,
        &state.revisions,
    ) else {
        return;
    };
    if let Ok(mut memo) = transcript_graph_validated_memo().lock() {
        memo.record(key, state, approx_bytes);
    }
}

/// Approximate serialized size of a JSON value without serializing it.
///
/// Feeds the validated-memo byte budget only — an estimate is sufficient for
/// a memory bound, and the exact serialize this replaces was an O(document)
/// pass whose entire output was one `usize`.
pub(in crate::session) fn approximate_json_bytes(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Null => 4,
        serde_json::Value::Bool(_) => 5,
        serde_json::Value::Number(_) => 12,
        serde_json::Value::String(text) => text.len().saturating_add(8),
        serde_json::Value::Array(items) => items
            .iter()
            .map(approximate_json_bytes)
            .fold(2usize.saturating_add(items.len()), usize::saturating_add),
        serde_json::Value::Object(entries) => entries
            .iter()
            .map(|(key, value)| {
                key.len()
                    .saturating_add(4)
                    .saturating_add(approximate_json_bytes(value))
            })
            .fold(2usize, usize::saturating_add),
    }
}

/// Exact serialized byte count via a counting writer: the format pass runs,
/// but no document-sized buffer is allocated. Budget sizing only.
fn approximate_serialized_bytes<T: serde::Serialize>(value: &T) -> Option<usize> {
    struct CountingWriter(usize);
    impl std::io::Write for CountingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(buf.len());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = CountingWriter(0);
    serde_json::to_writer(&mut writer, value).ok()?;
    Some(writer.0)
}

/// Validation trust mode for
/// [`TranscriptHistoryState::compact_mechanical_revision_bodies_for`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TranscriptGraphValidationMode {
    /// Always run the full per-body digest validation. Every write, typed
    /// mutation, and serialization seam uses this mode: a cached hit is
    /// memoized trust, not a fresh proof of current bytes.
    FullVerify,
    /// Decode path for durable documents: a graph shape whose full
    /// validation already succeeded on this process's decode path is
    /// SUBSTITUTED with the proven post-prune graph the memo retains — the
    /// hit returns verified content instead of trusting incoming content.
    /// First sight still verifies fully and admits the proven graph into
    /// the bounded memo.
    DecodeMemoized,
}
