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
/// The key pins everything a cached decode outcome may depend on EXCEPT
/// retained message content: the digest-format generation, the head revision
/// string, every retained body's content-addressed revision string, parent
/// pointer, `created_at`, and message count, and the full serialized commit
/// log (span digests, selection bounds, counts). Message content is
/// deliberately unpinned because the validated memo's VALUE is a graph whose
/// bodies were proved against those same revision strings — a hit
/// substitutes proven content rather than trusting incoming content, so the
/// key needs totality over the fields the digests do not pin, not
/// collision-resistance. The `fact` tag namespaces the two memos so one can
/// never satisfy a consult for the other. Hashing here is O(graph
/// structure), never O(message content), and deliberately does not count as
/// a content-digest computation.
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

/// Process-lifetime bounded FIFO memo keyed by transcript-graph shape.
///
/// One implementation serves both decode-path facts: the heal-probe memo
/// stores `()` (a pure verdict), the validated memo stores the proven
/// post-prune graph (substituted wholesale on a hit). Marker-less documents
/// (written by pre-marker code) and every decoded document's graph
/// validation otherwise pay a full canonical-JSON + SHA-256 pass over
/// retained transcript bodies on EVERY decode — O(document) work per repeat
/// load of unchanged bytes. Admission requires one complete verification,
/// so the first decode after boot always hashes, and changed structure
/// re-keys the memo (digest format, revision strings, counts, parents,
/// created_at, or commit bytes change) and re-verifies. Bounded FIFO
/// eviction only forces a redundant re-verification. Write and typed
/// mutation seams never consult these memos.
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

/// Deliberately small: entries retain whole transcript graphs by `Arc`, not
/// 66-byte key strings, and production documents on record reach 14-94 MB.
/// Eviction only costs a re-validation.
const TRANSCRIPT_GRAPH_VALIDATED_MEMO_CAPACITY: usize = 32;

static TRANSCRIPT_GRAPH_VALIDATED_MEMO: OnceLock<
    Mutex<BoundedTranscriptGraphMemo<Arc<TranscriptHistoryState>>>,
> = OnceLock::new();

fn transcript_graph_validated_memo()
-> &'static Mutex<BoundedTranscriptGraphMemo<Arc<TranscriptHistoryState>>> {
    TRANSCRIPT_GRAPH_VALIDATED_MEMO.get_or_init(|| {
        Mutex::new(BoundedTranscriptGraphMemo::new(
            TRANSCRIPT_GRAPH_VALIDATED_MEMO_CAPACITY,
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
        .and_then(|memo| memo.get(key).map(Arc::clone))
}

/// Record the post-prune graph one completed full validation proved for
/// this exact graph shape.
pub(super) fn record_validated_transcript_graph(key: String, state: Arc<TranscriptHistoryState>) {
    if let Ok(mut memo) = transcript_graph_validated_memo().lock() {
        memo.record(key, state);
    }
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
