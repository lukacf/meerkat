//! Incremental transcript digest accumulator.
//!
//! # Why this exists
//!
//! [`transcript_messages_digest`](super::transcript_messages_digest) is the
//! durable identity of a transcript: it appears in `session_heads.head_revision`,
//! in every transcript-history graph, in rewrite CAS tokens, and in every save
//! guard. Recomputing it is O(document), and a turn boundary recomputed it a
//! dozen or more times — so an ordinary one-word turn on a 90 MB session cost
//! minutes of canonical-JSON + SHA-256 work that had nothing to do with the
//! delta being saved.
//!
//! # What this does NOT change
//!
//! The digest VALUE. `digest_format` stays `2`; nothing new is persisted. The
//! byte stream that `serde_json::to_vec(&canonicalize_messages_for_digest(m))`
//! produces is
//!
//! ```text
//! "[" + json(c(m0)) + "," + json(c(m1)) + ... + "]"
//! ```
//!
//! which is append-extendable, and `sha2::Sha256` is `Clone`. So a retained
//! hasher midstate over the identity byte stream plus the appended suffix
//! yields the EXACT format-2 digest of the grown transcript. Every value this
//! module serves is byte-identical to a full recompute; only the cost differs.
//!
//! # Invalidation
//!
//! Staleness is prevented structurally, not by discipline: the accumulator is
//! a private field of [`TranscriptMessages`], which owns the message buffer and
//! exposes no `DerefMut`. Every write to the buffer therefore goes through one
//! of this type's typed mutators, and each mutator either extends the
//! accumulator (appends) or invalidates it (everything else). A new
//! message-mutation seam cannot be added without calling one of them, so the
//! invalidation set cannot silently drift.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use sha2::{Digest, Sha256};

use crate::types::Message;

/// Bounded ring of retained prefix midstates, keyed by covered message count.
///
/// Guards ask exactly one prefix question — "digest of the incoming transcript
/// truncated to the previously persisted length" — so a handful of recent save
/// boundaries answers every hit that matters; anything else falls back to a
/// full recompute.
const BOUNDARY_RING_CAPACITY: usize = 8;

#[derive(Debug, Clone)]
struct Midstate {
    hasher: Sha256,
    covered: usize,
}

impl Midstate {
    fn finalize(&self) -> String {
        let mut hasher = self.hasher.clone();
        hasher.update(b"]");
        let digest = hasher.finalize();
        let mut out = String::with_capacity(digest.len() * 2 + 7);
        out.push_str("sha256:");
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in digest {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }

    fn absorb(&mut self, message: &Message) -> Result<(), serde_json::Error> {
        if self.covered > 0 {
            self.hasher.update(b",");
        }
        let canonical = super::canonicalize_message_for_digest(message);
        let bytes = serde_json::to_vec(&canonical)?;
        crate::checkpoint::record_content_digest_bytes(bytes.len() as u64);
        self.hasher.update(bytes);
        self.covered += 1;
        Ok(())
    }
}

/// Retained canonical-SORTED bytes of the current message vector: the
/// `write_canonical_json` form (lexicographic object keys) of
/// `canonicalize_messages_for_digest(messages)`, interior only (no closing
/// `]`). This is the byte stream the transcript-history checkpoint WITNESS
/// hashes for the live head body — a different byte stream from the
/// `to_vec` identity stream the midstates cover, which is why it is retained
/// separately. Seeded lazily on the first witness assembly; extended per
/// append; invalidated with the midstates.
#[derive(Debug, Clone)]
struct SortedStream {
    bytes: Vec<u8>,
    covered: usize,
}

/// SHA-256 midstate over `prefix ++ "[" ++ sorted canonical elements` (no
/// closing `]`) — the canonical checkpoint DOCUMENT's transcript span. The
/// canonical document sorts only the immutable `created_at` and `id` before
/// `messages`, so the prefix is constant per session instance and everything
/// after the array is turn-sized; finalizing with `"]" ++ suffix` yields the
/// byte-identical whole-document digest. Keyed by the exact prefix bytes so
/// a document-framing change reseeds instead of serving a wrong digest.
#[derive(Debug, Clone)]
struct FramedMidstate {
    prefix: Vec<u8>,
    hasher: Sha256,
    covered: usize,
}

#[derive(Debug, Clone, Default)]
struct AccumulatorState {
    /// Midstate over the identity byte stream of the CURRENT message vector.
    /// `None` means "not seeded yet" (lazy) or "invalidated by a non-append
    /// mutation"; both degrade to a full recompute on the next query.
    stream_a: Option<Midstate>,
    /// Retained prefix midstates at previously witnessed boundaries.
    boundaries: VecDeque<Midstate>,
    /// Canonical-sorted byte stream for witness assembly, when enabled.
    sorted_stream: Option<SortedStream>,
    /// Canonical checkpoint-document midstate, when enabled.
    framed: Option<FramedMidstate>,
    /// Monotonic count of non-append transcript mutations. Observability for
    /// tests and diagnostics; correctness does not depend on it.
    epoch: u64,
    /// Accumulator parked across an in-place scan whose mutation verdict is
    /// not yet known. Never served as a witness while parked.
    parked: Option<Box<AccumulatorState>>,
}

/// Retained SHA-256 midstate over the transcript identity byte stream.
#[derive(Debug, Default)]
pub(crate) struct TranscriptDigestAccumulator {
    /// Boxed deliberately: `Session` is embedded in the agent's async state
    /// machine, whose futures compose sizes additively through nesting, so
    /// every inline byte here is paid again at each spawn depth. Holding the
    /// state behind a pointer keeps `Session` small enough for the production
    /// spawn stack budget (pinned by rkat's
    /// `tools_full_with_explicit_auth_binding_can_spawn_within_production_stack_budget`,
    /// which this struct overflowed at 208 inline bytes — `stream_a`'s
    /// `Option<Midstate>` is 128 of them). One small allocation per session,
    /// against a struct that already heap-allocates its transcript, metadata
    /// map and Arc.
    state: Mutex<Box<AccumulatorState>>,
}

impl Clone for TranscriptDigestAccumulator {
    fn clone(&self) -> Self {
        Self {
            state: Mutex::new(self.locked().clone()),
        }
    }
}

thread_local! {
    /// Debug/test builds cross-check every witness-served digest against a
    /// full recompute, so a missed invalidation seam fails loudly in CI
    /// instead of silently persisting a wrong `head_revision`.
    ///
    /// The cross-check deliberately uses the UNCOUNTED digest helper: it is
    /// verification scaffolding, so it must not appear in the
    /// `session_content_digest_computations` budget the regression tests
    /// measure.
    static CROSS_CHECK_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}

/// Release-mode sampled verification budget: the first N witness-served
/// digests per process are recomputed and compared even in release builds.
/// Debug/test builds verify every serve (the thread-local above); release
/// builds otherwise had ZERO runtime verification — the entire safety
/// argument rested on the type-level no-`DerefMut` enforcement, while a
/// wrong `head_revision` would be persisted into two durable stores and
/// make sessions unloadable. Sampling converts that argument into evidence
/// at bounded cost; a mismatch panics before anything wrong is persisted.
/// 64, not 32: three sampled cross-checks now share this budget (the
/// accumulator witness, the framed checkpoint digest, and the rewrite
/// fast-path validator), so the original 32 would halve-or-worse each
/// check's coverage. Raised deliberately when the framed and fast-path
/// samples were added (2026-07-27).
static RELEASE_VERIFICATION_BUDGET: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(64);

pub(crate) fn take_verification_sample() -> bool {
    if cfg!(any(test, debug_assertions)) {
        CROSS_CHECK_ENABLED.with(std::cell::Cell::get)
    } else {
        RELEASE_VERIFICATION_BUDGET
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |budget| budget.checked_sub(1),
            )
            .is_ok()
    }
}

impl TranscriptDigestAccumulator {
    fn locked(&self) -> std::sync::MutexGuard<'_, Box<AccumulatorState>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Non-append mutation count. Diagnostics/tests only.
    pub(crate) fn epoch(&self) -> u64 {
        self.locked().epoch
    }

    /// Drop every retained midstate and advance the mutation epoch.
    fn invalidate(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        state.stream_a = None;
        state.boundaries.clear();
        state.sorted_stream = None;
        state.framed = None;
        state.parked = None;
        state.epoch = state.epoch.saturating_add(1);
    }

    /// Fold appended messages into the retained midstate.
    ///
    /// A not-yet-seeded accumulator stays unseeded (seeding needs the whole
    /// vector and is deferred to the first digest query). Retained prefix
    /// boundaries survive an append by construction.
    fn extend(&mut self, appended: &[Message]) {
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        if let Some(stream) = state.stream_a.as_mut() {
            for message in appended {
                if stream.absorb(message).is_err() {
                    // A message that cannot serialize also cannot be digested
                    // by the full path; drop every retained state and let the
                    // recompute surface the typed error at the call site.
                    state.stream_a = None;
                    state.boundaries.clear();
                    state.sorted_stream = None;
                    state.framed = None;
                    state.epoch = state.epoch.saturating_add(1);
                    return;
                }
            }
        }
        if state.sorted_stream.is_some() || state.framed.is_some() {
            for message in appended {
                match sorted_canonical_message_bytes(message) {
                    Ok(bytes) => {
                        if let Some(sorted) = state.sorted_stream.as_mut() {
                            if sorted.covered > 0 {
                                sorted.bytes.push(b',');
                            }
                            sorted.bytes.extend_from_slice(&bytes);
                            sorted.covered += 1;
                        }
                        if let Some(framed) = state.framed.as_mut() {
                            if framed.covered > 0 {
                                framed.hasher.update(b",");
                            }
                            framed.hasher.update(&bytes);
                            framed.covered += 1;
                        }
                    }
                    Err(_) => {
                        state.sorted_stream = None;
                        state.framed = None;
                        break;
                    }
                }
            }
        }
    }

    /// Park the accumulator across an in-place scan of unknown outcome.
    fn begin_in_place_scan(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        if state.stream_a.is_none()
            && state.boundaries.is_empty()
            && state.sorted_stream.is_none()
            && state.framed.is_none()
        {
            return;
        }
        let parked = AccumulatorState {
            stream_a: state.stream_a.take(),
            boundaries: std::mem::take(&mut state.boundaries),
            sorted_stream: state.sorted_stream.take(),
            framed: state.framed.take(),
            epoch: state.epoch,
            parked: None,
        };
        state.parked = Some(Box::new(parked));
    }

    /// Resolve a parked in-place scan.
    ///
    /// `None` (the scan changed nothing) restores the parked midstate;
    /// anything else discards it and bumps the epoch. Dropping the scope
    /// without calling this — an early `?` return — leaves the accumulator
    /// invalidated, which is the fail-safe direction.
    fn finish_in_place_scan(&mut self, lowest_mutated_index: Option<usize>) {
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        let Some(parked) = state.parked.take() else {
            if lowest_mutated_index.is_some() {
                state.stream_a = None;
                state.boundaries.clear();
                state.epoch = state.epoch.saturating_add(1);
            }
            return;
        };
        if lowest_mutated_index.is_some() {
            state.epoch = state.epoch.saturating_add(1);
            return;
        }
        state.stream_a = parked.stream_a;
        state.boundaries = parked.boundaries;
        state.sorted_stream = parked.sorted_stream;
        state.framed = parked.framed;
    }

    /// Digest of `messages` — witness-served when the retained midstate covers
    /// exactly this vector, a full seeding pass otherwise.
    ///
    /// Either way the current count is recorded as a boundary: taking the full
    /// digest of a transcript is exactly the act that makes that length a save
    /// boundary, and the next turn's guard asks for the digest of that prefix.
    fn digest(&self, messages: &[Message]) -> Result<String, serde_json::Error> {
        if let Some(witness) = self.witness(messages) {
            let mut state = self.locked();
            if let Some(stream) = state.stream_a.clone() {
                record_boundary(&mut state, &stream);
            }
            return Ok(witness);
        }
        let mut state = self.locked();
        let mut stream = Midstate {
            hasher: Sha256::new(),
            covered: 0,
        };
        stream.hasher.update(b"[");
        crate::checkpoint::record_content_digest_computation();
        for message in messages {
            stream.absorb(message)?;
        }
        let digest = stream.finalize();
        record_boundary(&mut state, &stream);
        state.stream_a = Some(stream);
        Ok(digest)
    }

    /// Digest of `messages` when a retained midstate already covers it.
    fn witness(&self, messages: &[Message]) -> Option<String> {
        let state = self.locked();
        let stream = state.stream_a.as_ref()?;
        if stream.covered != messages.len() {
            return None;
        }
        let digest = stream.finalize();
        drop(state);
        if take_verification_sample()
            && let Ok(recomputed) = super::transcript_messages_digest_uncounted(messages)
        {
            assert_eq!(
                digest, recomputed,
                "transcript digest accumulator served a stale witness: a message-mutation \
                 seam extended or replaced the transcript without invalidating the midstate"
            );
        }
        Some(digest)
    }

    /// Digest of the first `count` messages when a retained boundary covers
    /// exactly that prefix.
    fn prefix_witness(&self, messages: &[Message], count: usize) -> Option<String> {
        if count > messages.len() {
            return None;
        }
        let state = self.locked();
        let boundary = state
            .boundaries
            .iter()
            .find(|midstate| midstate.covered == count)
            .or_else(|| {
                state
                    .stream_a
                    .as_ref()
                    .filter(|stream| stream.covered == count)
            })?;
        let digest = boundary.finalize();
        drop(state);
        if take_verification_sample()
            && let Ok(recomputed) = super::transcript_messages_digest_uncounted(&messages[..count])
        {
            assert_eq!(
                digest, recomputed,
                "transcript digest accumulator served a stale prefix witness: a \
                 message-mutation seam rewrote a retained prefix without invalidating the ring"
            );
        }
        Some(digest)
    }

    /// Feed the canonical-sorted byte form of `messages` (a complete
    /// canonical JSON array) into `hasher`.
    ///
    /// Seeds the retained sorted stream on first use (one O(transcript)
    /// canonicalization), then serves and extends in O(delta) per append.
    /// Returns `false` when the stream cannot prove it covers exactly this
    /// vector (parked scan, count mismatch it cannot reseed, serialization
    /// failure); the caller falls back to the full canonicalization path.
    fn hash_sorted_canonical_into(&self, messages: &[Message], hasher: &mut Sha256) -> bool {
        let mut state = self.locked();
        if state.parked.is_some() {
            return false;
        }
        let serve = match state.sorted_stream.as_ref() {
            Some(sorted) if sorted.covered == messages.len() => true,
            _ => {
                let mut sorted = SortedStream {
                    bytes: Vec::new(),
                    covered: 0,
                };
                for message in messages {
                    let Ok(bytes) = sorted_canonical_message_bytes(message) else {
                        return false;
                    };
                    if sorted.covered > 0 {
                        sorted.bytes.push(b',');
                    }
                    sorted.bytes.extend_from_slice(&bytes);
                    sorted.covered += 1;
                }
                state.sorted_stream = Some(sorted);
                true
            }
        };
        if !serve {
            return false;
        }
        let Some(sorted) = state.sorted_stream.as_ref() else {
            return false;
        };
        hasher.update(b"[");
        hasher.update(&sorted.bytes);
        hasher.update(b"]");
        crate::checkpoint::record_content_digest_bytes(sorted.bytes.len() as u64 + 2);
        true
    }

    /// SHA-256 midstate over `prefix ++ "[" ++ canonical elements of exactly
    /// this vector` (no closing `]`) — a clone the caller finalizes with
    /// `"]" ++ suffix` to obtain the byte-identical canonical checkpoint
    /// document digest.
    ///
    /// Serves the retained midstate when it covers exactly this vector under
    /// exactly this prefix; otherwise reseeds with one full canonicalization
    /// pass (counted — that is the pass the digest budget observes) and
    /// retains the result. `None` when the accumulator is parked or a
    /// message fails to serialize; the caller falls back to the full
    /// document path — the fail-safe direction.
    fn framed_document_hasher(&self, messages: &[Message], prefix: &[u8]) -> Option<Sha256> {
        let mut state = self.locked();
        if state.parked.is_some() {
            return None;
        }
        if let Some(framed) = state.framed.as_ref()
            && framed.covered == messages.len()
            && framed.prefix == prefix
        {
            return Some(framed.hasher.clone());
        }
        let mut hasher = Sha256::new();
        hasher.update(prefix);
        hasher.update(b"[");
        let mut element_bytes = 0u64;
        let mut covered = 0usize;
        for message in messages {
            let Ok(bytes) = sorted_canonical_message_bytes(message) else {
                return None;
            };
            if covered > 0 {
                hasher.update(b",");
                element_bytes = element_bytes.saturating_add(1);
            }
            hasher.update(&bytes);
            element_bytes = element_bytes.saturating_add(bytes.len() as u64);
            covered += 1;
        }
        crate::checkpoint::record_content_digest_bytes(element_bytes);
        state.framed = Some(FramedMidstate {
            prefix: prefix.to_vec(),
            hasher: hasher.clone(),
            covered,
        });
        Some(hasher)
    }
}

/// Canonical-sorted bytes of one message: `write_canonical_json` over the
/// digest-canonicalized message value. This is the per-element form of the
/// history-witness byte stream (canonical array writing is element-wise).
fn sorted_canonical_message_bytes(message: &Message) -> Result<Vec<u8>, serde_json::Error> {
    let canonical = serde_json::to_value(super::canonicalize_message_for_digest(message))?;
    let mut bytes = Vec::new();
    crate::checkpoint::write_canonical_json(&canonical, &mut bytes)?;
    Ok(bytes)
}

/// Process-global bounded memo carrying accumulator midstates across the
/// serialize → deserialize boundary, keyed by the SHA-256 of the EXACT
/// serialized session bytes.
///
/// Every fact in an [`AccumulatorState`] is a pure function of the message
/// vector, and the message vector is a pure function of the serialized
/// bytes, so a key that is a collision-resistant hash of those exact bytes
/// binds precisely what a fresh recompute would bind — this is the
/// byte-binding discipline the P0 memo review required, not a shape key.
/// Producers record the WARM state at the moment they serialize a session
/// (or fully verify a decoded one); a decode of the same bytes adopts the
/// snapshot instead of reseeding every midstate with an O(document)
/// canonicalize-and-hash pass per guard, head build, and checkpoint
/// verification. The debug/release sampled witness cross-checks
/// (`take_verification_sample`) keep verifying adopted midstates against
/// full recomputes, so a keying defect fails loudly instead of persisting a
/// wrong digest. Entries are byte-budgeted (the sorted stream retains the
/// canonical transcript bytes); eviction only costs a reseed. Honors
/// `MEERKAT_DISABLE_GRAPH_DECODE_MEMO` so the established kill-switch
/// restores the full pre-memo decode cost end to end.
struct ByteBoundAccumulatorMemo {
    budget_bytes: usize,
    entry_cap_bytes: usize,
    retained_bytes: usize,
    entries: std::collections::HashMap<[u8; 32], (Arc<AccumulatorState>, usize)>,
    order: VecDeque<[u8; 32]>,
}

const BYTE_BOUND_ACCUMULATOR_MEMO_BUDGET: usize = 128 * 1024 * 1024;
const BYTE_BOUND_ACCUMULATOR_MEMO_ENTRY_CAP: usize = BYTE_BOUND_ACCUMULATOR_MEMO_BUDGET / 2;

impl ByteBoundAccumulatorMemo {
    fn get(&mut self, key: &[u8; 32]) -> Option<Arc<AccumulatorState>> {
        let (state, _) = self.entries.get(key)?;
        let state = Arc::clone(state);
        if let Some(position) = self.order.iter().position(|entry| entry == key)
            && let Some(entry) = self.order.remove(position)
        {
            self.order.push_back(entry);
        }
        Some(state)
    }

    fn record(&mut self, key: [u8; 32], state: Arc<AccumulatorState>, bytes: usize) {
        if bytes > self.entry_cap_bytes {
            return;
        }
        if let Some((_, existing)) = self.entries.remove(&key) {
            self.retained_bytes = self.retained_bytes.saturating_sub(existing);
            if let Some(position) = self.order.iter().position(|entry| entry == &key) {
                self.order.remove(position);
            }
        }
        while self.retained_bytes + bytes > self.budget_bytes {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            if let Some((_, evicted_bytes)) = self.entries.remove(&evicted) {
                self.retained_bytes = self.retained_bytes.saturating_sub(evicted_bytes);
            }
        }
        self.order.push_back(key);
        self.retained_bytes += bytes;
        self.entries.insert(key, (state, bytes));
    }
}

static BYTE_BOUND_ACCUMULATOR_MEMO: std::sync::OnceLock<Mutex<ByteBoundAccumulatorMemo>> =
    std::sync::OnceLock::new();

fn byte_bound_accumulator_memo() -> &'static Mutex<ByteBoundAccumulatorMemo> {
    BYTE_BOUND_ACCUMULATOR_MEMO.get_or_init(|| {
        Mutex::new(ByteBoundAccumulatorMemo {
            budget_bytes: BYTE_BOUND_ACCUMULATOR_MEMO_BUDGET,
            entry_cap_bytes: BYTE_BOUND_ACCUMULATOR_MEMO_ENTRY_CAP,
            retained_bytes: 0,
            entries: std::collections::HashMap::new(),
            order: VecDeque::new(),
        })
    })
}

fn session_bytes_key(serialized: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(serialized);
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    key
}

fn accumulator_state_retained_bytes(state: &AccumulatorState) -> usize {
    let sorted = state
        .sorted_stream
        .as_ref()
        .map_or(0, |sorted| sorted.bytes.len());
    let framed = state
        .framed
        .as_ref()
        .map_or(0, |framed| framed.prefix.len() + 128);
    // Midstates and the boundary ring are ~constant-size hasher states.
    sorted + framed + 4096
}

/// Shared handle to a message vector plus the accumulator midstates proven
/// for it — the value type of the slim-materialization substitution memo.
/// The vector rides the buffer's own copy-on-write `Arc`, so recording is
/// O(1) and retention is shared with the live session while it is alive.
#[derive(Debug, Clone)]
pub(crate) struct SharedTranscriptSnapshot {
    messages: Arc<Vec<Message>>,
    state: Arc<AccumulatorState>,
}

impl SharedTranscriptSnapshot {
    pub(crate) fn message_count(&self) -> usize {
        self.messages.len()
    }

    pub(crate) fn messages(&self) -> &Arc<Vec<Message>> {
        &self.messages
    }
}

impl TranscriptMessages {
    /// Snapshot the current buffer and its midstates for the substitution
    /// memo. `None` when the accumulator is parked or a retained midstate
    /// does not cover exactly this vector.
    pub(crate) fn shared_snapshot(&self) -> Option<SharedTranscriptSnapshot> {
        let state = self.accumulator.locked();
        if state.parked.is_some()
            || state
                .stream_a
                .as_ref()
                .is_some_and(|stream| stream.covered != self.messages.len())
            || state
                .sorted_stream
                .as_ref()
                .is_some_and(|sorted| sorted.covered != self.messages.len())
            || state
                .framed
                .as_ref()
                .is_some_and(|framed| framed.covered != self.messages.len())
        {
            return None;
        }
        let snapshot = (**state).clone();
        drop(state);
        Some(SharedTranscriptSnapshot {
            messages: Arc::clone(&self.messages),
            state: Arc::new(snapshot),
        })
    }

    /// Rebuild a buffer from a snapshot: the shared vector plus the exact
    /// midstates proven for it.
    pub(crate) fn from_shared_snapshot(snapshot: &SharedTranscriptSnapshot) -> Self {
        Self {
            messages: Arc::clone(&snapshot.messages),
            accumulator: Box::new(TranscriptDigestAccumulator {
                state: Mutex::new(Box::new((*snapshot.state).clone())),
            }),
        }
    }

    /// Record this buffer's retained midstates under the SHA-256 of the
    /// exact bytes the enclosing session just serialized to (or was fully
    /// verified against). No-op when nothing is retained, when the
    /// accumulator is parked mid-scan, or when the kill-switch is set.
    pub(crate) fn record_state_for_serialized_bytes(&self, serialized: &[u8]) {
        if std::env::var_os("MEERKAT_DISABLE_GRAPH_DECODE_MEMO").is_some() {
            return;
        }
        let state = self.accumulator.locked();
        if state.parked.is_some()
            || (state.stream_a.is_none() && state.sorted_stream.is_none() && state.framed.is_none())
        {
            return;
        }
        // Only snapshot states that cover exactly the current buffer: a
        // stale-but-uninvalidated midstate cannot exist by construction, but
        // count agreement is cheap and keeps the recorded fact self-evident.
        if state
            .stream_a
            .as_ref()
            .is_some_and(|stream| stream.covered != self.messages.len())
        {
            return;
        }
        let snapshot: AccumulatorState = (**state).clone();
        drop(state);
        let bytes = accumulator_state_retained_bytes(&snapshot);
        if let Ok(mut memo) = byte_bound_accumulator_memo().lock() {
            memo.record(session_bytes_key(serialized), Arc::new(snapshot), bytes);
        }
    }

    /// Adopt the midstates previously recorded for these EXACT serialized
    /// bytes. Sound because the key is a collision-resistant hash of the
    /// bytes this buffer was just decoded from: the recorded state was
    /// computed over the identical message vector. Count mismatches (or a
    /// recorded parked state) are discarded, and every witness serve stays
    /// covered by the sampled full-recompute cross-check.
    pub(crate) fn adopt_state_for_serialized_bytes(&mut self, serialized: &[u8]) {
        if std::env::var_os("MEERKAT_DISABLE_GRAPH_DECODE_MEMO").is_some() {
            return;
        }
        let Some(snapshot) = byte_bound_accumulator_memo()
            .lock()
            .ok()
            .and_then(|mut memo| memo.get(&session_bytes_key(serialized)))
        else {
            return;
        };
        if snapshot.parked.is_some()
            || snapshot
                .stream_a
                .as_ref()
                .is_some_and(|stream| stream.covered != self.messages.len())
            || snapshot
                .sorted_stream
                .as_ref()
                .is_some_and(|sorted| sorted.covered != self.messages.len())
            || snapshot
                .framed
                .as_ref()
                .is_some_and(|framed| framed.covered != self.messages.len())
        {
            return;
        }
        **self.accumulator.locked() = (*snapshot).clone();
    }
}

fn record_boundary(state: &mut AccumulatorState, stream: &Midstate) {
    if state
        .boundaries
        .iter()
        .any(|midstate| midstate.covered == stream.covered)
    {
        return;
    }
    if state.boundaries.len() >= BOUNDARY_RING_CAPACITY {
        state.boundaries.pop_front();
    }
    state.boundaries.push_back(stream.clone());
}

/// The live transcript buffer plus its incremental digest accumulator.
///
/// Reads deref to the message vector. Writes have no `DerefMut`: they must go
/// through the typed mutators below, each of which states its effect on the
/// accumulator. That is the whole invalidation-exhaustiveness argument.
#[derive(Debug, Default)]
pub(crate) struct TranscriptMessages {
    messages: Arc<Vec<Message>>,
    /// Boxed for the same reason the state inside it is: `Session` rides the
    /// agent's nested async futures against a 2 MB production stack budget.
    accumulator: Box<TranscriptDigestAccumulator>,
}

impl Clone for TranscriptMessages {
    fn clone(&self) -> Self {
        Self {
            messages: Arc::clone(&self.messages),
            accumulator: Box::new((*self.accumulator).clone()),
        }
    }
}

impl std::ops::Deref for TranscriptMessages {
    type Target = Vec<Message>;

    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}

impl TranscriptMessages {
    /// Shared handle for the copy-on-write buffer. Reads only: mutating
    /// through a cloned `Arc` would bypass the accumulator.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn arc(&self) -> &Arc<Vec<Message>> {
        &self.messages
    }

    /// Adopt an owned message vector. The accumulator starts unseeded; the
    /// first digest query pays one full pass.
    pub(crate) fn from_vec(messages: Vec<Message>) -> Self {
        Self {
            messages: Arc::new(messages),
            accumulator: Box::default(),
        }
    }

    /// SEAM (append): fold one appended message into the accumulator.
    pub(crate) fn push(&mut self, message: Message) {
        let Self {
            messages,
            accumulator,
        } = self;
        let inner = Arc::make_mut(messages);
        inner.push(message);
        let appended = &inner[inner.len() - 1..];
        accumulator.extend(appended);
    }

    /// SEAM (append): fold an appended batch into the accumulator.
    pub(crate) fn extend_batch(&mut self, appended: Vec<Message>) {
        if appended.is_empty() {
            return;
        }
        let Self {
            messages,
            accumulator,
        } = self;
        let inner = Arc::make_mut(messages);
        let start = inner.len();
        inner.extend(appended);
        accumulator.extend(&inner[start..]);
    }

    /// SEAM (replacement): install a new transcript; invalidates.
    pub(crate) fn replace(&mut self, messages: Vec<Message>) {
        self.messages = Arc::new(messages);
        self.accumulator.invalidate();
    }

    /// SEAM (in-place rewrite of unknown shape): invalidates unconditionally.
    pub(crate) fn mutate_in_place(&mut self) -> &mut Vec<Message> {
        self.accumulator.invalidate();
        Arc::make_mut(&mut self.messages)
    }

    /// SEAM (in-place media scan): parks the accumulator until the scan
    /// reports whether it mutated anything. Must be paired with
    /// [`Self::finish_in_place_scan`]; an unpaired call (early error return)
    /// leaves the accumulator invalidated.
    pub(crate) fn begin_in_place_scan(&mut self) -> &mut Vec<Message> {
        self.accumulator.begin_in_place_scan();
        Arc::make_mut(&mut self.messages)
    }

    /// Resolve a parked in-place scan with the lowest mutated message index
    /// (`None` = the scan changed nothing, so the witness stays valid).
    pub(crate) fn finish_in_place_scan(&mut self, lowest_mutated_index: Option<usize>) {
        self.accumulator.finish_in_place_scan(lowest_mutated_index);
    }

    /// Format-2 transcript digest of the current buffer.
    pub(crate) fn digest(&self) -> Result<String, serde_json::Error> {
        self.accumulator.digest(&self.messages)
    }

    /// Format-2 transcript digest, only when a retained midstate already
    /// proves it (never seeds, never recomputes).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn digest_witness(&self) -> Option<String> {
        self.accumulator.witness(&self.messages)
    }

    /// Format-2 digest of the first `count` messages, only when a retained
    /// boundary already proves it.
    pub(crate) fn prefix_digest_witness(&self, count: usize) -> Option<String> {
        self.accumulator.prefix_witness(&self.messages, count)
    }

    /// Non-append mutation count of this buffer.
    pub(crate) fn mutation_epoch(&self) -> u64 {
        self.accumulator.epoch()
    }

    /// Feed the canonical-sorted form of the current buffer into `hasher`;
    /// see [`TranscriptDigestAccumulator::hash_sorted_canonical_into`].
    pub(crate) fn hash_sorted_canonical_into(&self, hasher: &mut Sha256) -> bool {
        self.accumulator
            .hash_sorted_canonical_into(&self.messages, hasher)
    }

    /// Canonical checkpoint-document midstate for the current buffer under
    /// `prefix`; see [`TranscriptDigestAccumulator::framed_document_hasher`].
    pub(crate) fn framed_document_hasher(&self, prefix: &[u8]) -> Option<Sha256> {
        self.accumulator
            .framed_document_hasher(&self.messages, prefix)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::{Message, UserMessage};

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    fn transcript(count: usize) -> Vec<Message> {
        (0..count).map(|i| user(&format!("m{i}"))).collect()
    }

    /// The accumulator folds per-message canonical bytes; that is only the
    /// same byte stream the whole-vector digest hashes because transcript
    /// canonicalization is element-wise. This pin fails the moment someone
    /// adds a cross-message normalization to
    /// `canonicalize_messages_for_digest`.
    #[test]
    fn canonicalize_messages_for_digest_is_element_wise() {
        let messages = transcript(6);
        let whole = super::super::canonicalize_messages_for_digest(&messages);
        let per_message = messages
            .iter()
            .map(super::super::canonicalize_message_for_digest)
            .collect::<Vec<_>>();
        assert_eq!(whole, per_message);

        // ... and the array's serialized bytes really are the delimiter-joined
        // element bytes, which is the accumulator's stream construction.
        let array_bytes = serde_json::to_vec(&whole).unwrap();
        let mut streamed = Vec::from(b"[".as_slice());
        for (index, message) in per_message.iter().enumerate() {
            if index > 0 {
                streamed.extend_from_slice(b",");
            }
            streamed.extend_from_slice(&serde_json::to_vec(message).unwrap());
        }
        streamed.extend_from_slice(b"]");
        assert_eq!(array_bytes, streamed);
    }

    #[test]
    fn seeded_digest_matches_full_recompute() {
        for count in [0usize, 1, 2, 7, 40] {
            let messages = TranscriptMessages::from_vec(transcript(count));
            assert_eq!(
                messages.digest().unwrap(),
                super::super::transcript_messages_digest(&messages).unwrap(),
                "count {count}"
            );
        }
    }

    #[test]
    fn appended_digest_matches_full_recompute() {
        let mut messages = TranscriptMessages::from_vec(transcript(3));
        // Seed.
        let _ = messages.digest().unwrap();
        messages.push(user("appended"));
        assert!(messages.digest_witness().is_some());
        assert_eq!(
            messages.digest().unwrap(),
            super::super::transcript_messages_digest(&messages).unwrap()
        );
        messages.extend_batch(vec![user("a"), user("b")]);
        assert!(messages.digest_witness().is_some());
        assert_eq!(
            messages.digest().unwrap(),
            super::super::transcript_messages_digest(&messages).unwrap()
        );
    }

    #[test]
    fn unseeded_accumulator_has_no_witness() {
        let messages = TranscriptMessages::from_vec(transcript(3));
        assert!(messages.digest_witness().is_none());
        assert!(messages.prefix_digest_witness(2).is_none());
    }

    #[test]
    fn replacement_invalidates_the_witness() {
        let mut messages = TranscriptMessages::from_vec(transcript(3));
        let _ = messages.digest().unwrap();
        let epoch = messages.mutation_epoch();
        messages.replace(transcript(2));
        assert!(messages.digest_witness().is_none());
        assert!(messages.mutation_epoch() > epoch);
        assert_eq!(
            messages.digest().unwrap(),
            super::super::transcript_messages_digest(&messages).unwrap()
        );
    }

    #[test]
    fn in_place_mutation_invalidates_the_witness() {
        let mut messages = TranscriptMessages::from_vec(transcript(3));
        let _ = messages.digest().unwrap();
        messages.mutate_in_place()[0] = user("rewritten");
        assert!(messages.digest_witness().is_none());
        assert_eq!(
            messages.digest().unwrap(),
            super::super::transcript_messages_digest(&messages).unwrap()
        );
    }

    #[test]
    fn unmutated_in_place_scan_keeps_the_witness() {
        let mut messages = TranscriptMessages::from_vec(transcript(3));
        let seeded = messages.digest().unwrap();
        let _buffer = messages.begin_in_place_scan();
        assert!(
            messages.digest_witness().is_none(),
            "a parked accumulator must not serve a witness"
        );
        messages.finish_in_place_scan(None);
        assert_eq!(messages.digest_witness(), Some(seeded));
    }

    #[test]
    fn mutated_in_place_scan_drops_the_witness() {
        let mut messages = TranscriptMessages::from_vec(transcript(3));
        let _ = messages.digest().unwrap();
        {
            let buffer = messages.begin_in_place_scan();
            buffer[1] = user("changed");
        }
        messages.finish_in_place_scan(Some(1));
        assert!(messages.digest_witness().is_none());
        assert_eq!(
            messages.digest().unwrap(),
            super::super::transcript_messages_digest(&messages).unwrap()
        );
    }

    #[test]
    fn abandoned_in_place_scan_fails_safe() {
        let mut messages = TranscriptMessages::from_vec(transcript(3));
        let _ = messages.digest().unwrap();
        let buffer = messages.begin_in_place_scan();
        buffer[2] = user("changed");
        // No finish call: the parked midstate must never come back.
        assert!(messages.digest_witness().is_none());
        assert_eq!(
            messages.digest().unwrap(),
            super::super::transcript_messages_digest(&messages).unwrap()
        );
    }

    #[test]
    fn boundary_ring_answers_prefix_queries_after_appends() {
        let mut messages = TranscriptMessages::from_vec(transcript(5));
        let boundary = messages.digest().unwrap();
        messages.extend_batch(vec![user("x"), user("y")]);
        assert_eq!(messages.prefix_digest_witness(5), Some(boundary));
        assert_eq!(
            messages.prefix_digest_witness(5).unwrap(),
            super::super::transcript_messages_digest(&messages[..5]).unwrap()
        );
        assert!(messages.prefix_digest_witness(4).is_none());
    }

    #[test]
    fn boundary_ring_is_bounded_and_dropped_on_invalidation() {
        let mut messages = TranscriptMessages::from_vec(Vec::new());
        for _ in 0..(BOUNDARY_RING_CAPACITY + 4) {
            messages.push(user("m"));
            let _ = messages.digest().unwrap();
        }
        let retained = (0..=(BOUNDARY_RING_CAPACITY + 4))
            .filter(|count| messages.prefix_digest_witness(*count).is_some())
            .count();
        assert!(
            retained <= BOUNDARY_RING_CAPACITY,
            "boundary ring grew past its bound: {retained}"
        );
        messages.mutate_in_place();
        assert_eq!(
            (0..=(BOUNDARY_RING_CAPACITY + 4))
                .filter(|count| messages.prefix_digest_witness(*count).is_some())
                .count(),
            0
        );
    }

    #[test]
    fn randomized_mutation_sequences_match_full_recompute() {
        // Deterministic pseudo-random walk over every typed mutator.
        let mut seed = 0x5eed_1234_u64;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        let mut messages = TranscriptMessages::from_vec(transcript(4));
        for step in 0..200 {
            match next() % 6 {
                0 => messages.push(user(&format!("p{step}"))),
                1 => messages.extend_batch(vec![user(&format!("b{step}")), user("b2")]),
                2 => messages.replace(transcript(next() % 9)),
                3 => {
                    let buffer = messages.mutate_in_place();
                    if !buffer.is_empty() {
                        let index = next() % buffer.len();
                        buffer[index] = user(&format!("r{step}"));
                    }
                }
                4 => {
                    messages.begin_in_place_scan();
                    messages.finish_in_place_scan(None);
                }
                _ => {
                    let buffer = messages.begin_in_place_scan();
                    let mutated = if buffer.is_empty() {
                        None
                    } else {
                        let index = next() % buffer.len();
                        buffer[index] = user(&format!("s{step}"));
                        Some(index)
                    };
                    messages.finish_in_place_scan(mutated);
                }
            }
            assert_eq!(
                messages.digest().unwrap(),
                super::super::transcript_messages_digest(&messages).unwrap(),
                "step {step}"
            );
            let count = messages.len();
            if count > 1 {
                messages.push(user("tail"));
                assert_eq!(
                    messages.prefix_digest_witness(count),
                    Some(super::super::transcript_messages_digest(&messages[..count]).unwrap()),
                    "prefix witness at step {step}"
                );
            }
        }
    }
}
