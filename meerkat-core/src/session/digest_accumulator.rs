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

use crate::session_store::SessionMessageRowPrefixAccumulator;
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
        crate::digest_observability::record_content_digest_bytes(bytes.len() as u64);
        self.hasher.update(bytes);
        self.covered += 1;
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
struct AccumulatorState {
    /// Midstate over the identity byte stream of the CURRENT message vector.
    /// `None` means "not seeded yet" (lazy) or "invalidated by a non-append
    /// mutation"; both degrade to a full recompute on the next query.
    stream_a: Option<Midstate>,
    /// Retained prefix midstates at previously witnessed boundaries.
    boundaries: VecDeque<Midstate>,
    /// Exact durable-row authority last installed by a verified
    /// head-canonical materialization or acknowledged commit.
    exact_row_anchor: Option<SessionMessageRowPrefixAccumulator>,
    /// The anchor extended through every append performed on this buffer.
    /// Non-append mutations invalidate both exact-row fields.
    exact_row_current: Option<SessionMessageRowPrefixAccumulator>,
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

pub(crate) fn take_verification_sample() -> bool {
    if cfg!(any(test, debug_assertions)) {
        CROSS_CHECK_ENABLED.with(std::cell::Cell::get)
    } else {
        // Production hot paths must remain O(delta) from the first turn after
        // every restart. Correctness comes from byte-bound structural
        // witnesses and mutation-exhaustive invalidation; full recomputation
        // belongs to explicit verification/migration phases, not a hidden
        // process-start sampling cliff.
        false
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
        state.exact_row_anchor = None;
        state.exact_row_current = None;
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
        if let Some(current) = state.exact_row_current.take() {
            let serialized = appended
                .iter()
                .map(serde_json::to_vec)
                .collect::<Result<Vec<_>, _>>();
            state.exact_row_current = serialized
                .ok()
                .and_then(|rows| current.extend_serialized_rows(&rows).ok());
        }
        if let Some(stream) = state.stream_a.as_mut() {
            for message in appended {
                if stream.absorb(message).is_err() {
                    // A message that cannot serialize also cannot be digested
                    // by the full path; drop every retained state and let the
                    // recompute surface the typed error at the call site.
                    state.stream_a = None;
                    state.boundaries.clear();
                    state.epoch = state.epoch.saturating_add(1);
                    return;
                }
            }
        }
    }

    /// Park the accumulator across an in-place scan of unknown outcome.
    fn begin_in_place_scan(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        if state.stream_a.is_none()
            && state.boundaries.is_empty()
            && state.exact_row_anchor.is_none()
            && state.exact_row_current.is_none()
        {
            return;
        }
        let parked = AccumulatorState {
            stream_a: state.stream_a.take(),
            boundaries: std::mem::take(&mut state.boundaries),
            exact_row_anchor: state.exact_row_anchor.take(),
            exact_row_current: state.exact_row_current.take(),
            epoch: state.epoch,
            parked: None,
        };
        state.parked = Some(Box::new(parked));
    }

    /// Resolve a parked in-place scan.
    ///
    /// `None` (the scan changed nothing) restores the parked state. A mutation
    /// invalidates every content/current-row witness and bumps the epoch, but
    /// may retain an exact durable-row ANCHOR when the lowest changed index is
    /// at or beyond the anchor's row count: by construction no row committed
    /// by that prefix changed. Dropping the scope without calling this — an
    /// early `?` return — leaves the accumulator invalidated, which is the
    /// fail-safe direction.
    fn finish_in_place_scan(&mut self, lowest_mutated_index: Option<usize>) {
        let state = self.state.get_mut().unwrap_or_else(PoisonError::into_inner);
        let Some(parked) = state.parked.take() else {
            if lowest_mutated_index.is_some() {
                state.stream_a = None;
                state.boundaries.clear();
                state.exact_row_anchor = None;
                state.exact_row_current = None;
                state.epoch = state.epoch.saturating_add(1);
            }
            return;
        };
        if let Some(lowest_mutated_index) = lowest_mutated_index {
            state.exact_row_anchor = parked.exact_row_anchor.filter(|anchor| {
                u64::try_from(lowest_mutated_index).is_ok_and(|index| index >= anchor.row_count())
            });
            state.epoch = state.epoch.saturating_add(1);
            return;
        }
        state.stream_a = parked.stream_a;
        state.boundaries = parked.boundaries;
        state.exact_row_anchor = parked.exact_row_anchor;
        state.exact_row_current = parked.exact_row_current;
    }

    fn install_exact_row_prefix(&self, prefix: SessionMessageRowPrefixAccumulator) {
        let mut state = self.locked();
        // An acknowledgement of the already-tracked current prefix does not
        // change its semantic ancestry. Preserve the audited graph-endpoint
        // anchor so the next coherence check stays O(1) after arbitrarily many
        // ordinary appends/saves. A new or changed authority starts a new
        // lineage at that exact prefix.
        if state.exact_row_current.as_ref() == Some(&prefix) && state.exact_row_anchor.is_some() {
            state.exact_row_current = Some(prefix);
            return;
        }
        state.exact_row_anchor = Some(prefix.clone());
        state.exact_row_current = Some(prefix);
    }

    fn install_exact_row_lineage(
        &self,
        anchor: SessionMessageRowPrefixAccumulator,
        current: SessionMessageRowPrefixAccumulator,
    ) {
        let mut state = self.locked();
        state.exact_row_anchor = Some(anchor);
        state.exact_row_current = Some(current);
    }

    fn exact_row_prefix_at(&self, row_count: u64) -> Option<SessionMessageRowPrefixAccumulator> {
        let state = self.locked();
        state
            .exact_row_current
            .as_ref()
            .filter(|prefix| prefix.row_count() == row_count)
            .or_else(|| {
                state
                    .exact_row_anchor
                    .as_ref()
                    .filter(|prefix| prefix.row_count() == row_count)
            })
            .cloned()
    }

    fn exact_row_lineage_extends(
        &self,
        anchor: &SessionMessageRowPrefixAccumulator,
        current_count: u64,
    ) -> bool {
        let state = self.locked();
        state.exact_row_anchor.as_ref() == Some(anchor)
            && state
                .exact_row_current
                .as_ref()
                .is_some_and(|current| current.row_count() == current_count)
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
        crate::digest_observability::record_content_digest_computation();
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
#[derive(Debug)]
pub(crate) struct TranscriptMessages {
    messages: Arc<Vec<Message>>,
    /// Boxed for the same reason the state inside it is: `Session` rides the
    /// agent's nested async futures against a 2 MB production stack budget.
    accumulator: Box<TranscriptDigestAccumulator>,
}

impl Default for TranscriptMessages {
    fn default() -> Self {
        let accumulator = TranscriptDigestAccumulator::default();
        let empty = SessionMessageRowPrefixAccumulator::empty();
        accumulator.install_exact_row_lineage(empty.clone(), empty);
        Self {
            messages: Arc::default(),
            accumulator: Box::new(accumulator),
        }
    }
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

    /// Install exact durable-row lineage proven by a head materialization or
    /// acknowledged prepared commit.
    pub(crate) fn install_exact_row_prefix(
        &self,
        prefix: SessionMessageRowPrefixAccumulator,
    ) -> bool {
        if prefix.row_count() != self.messages.len() as u64 {
            return false;
        }
        self.accumulator.install_exact_row_prefix(prefix);
        true
    }

    pub(crate) fn install_exact_row_lineage(
        &self,
        anchor: SessionMessageRowPrefixAccumulator,
        current: SessionMessageRowPrefixAccumulator,
    ) -> bool {
        if anchor.row_count() > current.row_count()
            || current.row_count() != self.messages.len() as u64
        {
            return false;
        }
        self.accumulator.install_exact_row_lineage(anchor, current);
        true
    }

    /// Exact durable-row prefix witnessed at `row_count`, if it is either the
    /// last installed authority or that authority extended only by appends.
    pub(crate) fn exact_row_prefix_at(
        &self,
        row_count: u64,
    ) -> Option<SessionMessageRowPrefixAccumulator> {
        self.accumulator.exact_row_prefix_at(row_count)
    }

    pub(crate) fn exact_row_lineage_extends(
        &self,
        anchor: &SessionMessageRowPrefixAccumulator,
        current_count: u64,
    ) -> bool {
        self.accumulator
            .exact_row_lineage_extends(anchor, current_count)
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
    #[cfg(test)]
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

    fn exact_row_prefix(
        messages: &[Message],
    ) -> crate::session_store::SessionMessageRowPrefixAccumulator {
        let rows = messages
            .iter()
            .map(serde_json::to_vec)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        crate::session_store::SessionMessageRowPrefixAccumulator::from_serialized_rows(&rows)
            .unwrap()
    }

    #[test]
    fn fresh_transcript_owns_exact_genesis_row_lineage() {
        let mut messages = TranscriptMessages::default();
        assert_eq!(
            messages.exact_row_prefix_at(0),
            Some(crate::session_store::SessionMessageRowPrefixAccumulator::empty())
        );

        messages.push(user("first"));
        assert_eq!(
            messages.exact_row_prefix_at(1),
            Some(exact_row_prefix(&messages)),
            "ordinary appends must extend fresh construction authority without a full rescan"
        );
    }

    #[test]
    fn suffix_only_in_place_scan_preserves_exact_durable_row_anchor() {
        let mut messages = TranscriptMessages::from_vec(transcript(4));
        let anchor = exact_row_prefix(&messages[..2]);
        let current = exact_row_prefix(&messages);
        assert!(messages.install_exact_row_lineage(anchor.clone(), current));

        {
            let buffer = messages.begin_in_place_scan();
            buffer[2] = user("externalized suffix");
        }
        messages.finish_in_place_scan(Some(2));

        assert_eq!(messages.exact_row_prefix_at(2), Some(anchor));
        assert!(
            messages.exact_row_prefix_at(4).is_none(),
            "a suffix rewrite must invalidate the exact current-row prefix"
        );
        assert!(
            messages.digest_witness().is_none(),
            "a suffix rewrite must invalidate the whole-transcript digest witness"
        );
    }

    #[test]
    fn in_place_scan_inside_exact_durable_prefix_drops_row_anchor() {
        let mut messages = TranscriptMessages::from_vec(transcript(4));
        let anchor = exact_row_prefix(&messages[..2]);
        let current = exact_row_prefix(&messages);
        assert!(messages.install_exact_row_lineage(anchor, current));

        {
            let buffer = messages.begin_in_place_scan();
            buffer[1] = user("rewritten durable prefix");
        }
        messages.finish_in_place_scan(Some(1));

        assert!(messages.exact_row_prefix_at(2).is_none());
        assert!(messages.exact_row_prefix_at(4).is_none());
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
