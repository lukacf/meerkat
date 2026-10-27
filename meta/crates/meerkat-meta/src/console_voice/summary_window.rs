//! Size-bounded input window and reuse cache for the console context summary.
//!
//! Both are mechanics beneath Meerkat's `LiveContextSummaryPolicy`: the policy
//! still owns snapshot capture, its own input ceiling, the output cap, timeout,
//! and staleness validation. The window only chooses how much of the exact
//! captured snapshot the summariser reads; the cache only spares the model
//! call when the same source snapshot is summarised again.

use std::collections::VecDeque;
use std::sync::Mutex;

use meerkat_core::SessionId;
use meerkat_core::types::Message;
use sha2::{Digest as _, Sha256};

/// Upper bound on retained summaries. One entry per source session is enough
/// for reopen; the bound only keeps a long-lived gateway from accumulating
/// text for members that no longer exist.
pub(crate) const MAX_CACHED_SUMMARIES: usize = 32;

/// The longest suffix of `messages` whose serialized JSON fits `max_bytes`,
/// and never less than the newest message.
///
/// This is a pure size bound measured on the same serialization the
/// summariser sends (`serde_json` of each `Message`, plus the array
/// separators): the newest messages are kept whole, oldest first to go, and
/// no message content is inspected or truncated. The bound degrades rather
/// than fails: when the newest message alone exceeds `max_bytes` (a large
/// tool result is common) the window is that one whole message, which stays
/// subject to Meerkat's capture ceiling for the truly oversized case.
pub(crate) fn recent_window(messages: &[Message], max_bytes: usize) -> &[Message] {
    // `[` + `]` of the JSON array.
    let mut used = 2usize;
    let mut start = messages.len();
    for (index, message) in messages.iter().enumerate().rev() {
        let Ok(encoded) = serde_json::to_vec(message) else {
            break;
        };
        // One `,` separator for every message after the first.
        let separator = usize::from(index + 1 < messages.len());
        let next = used.saturating_add(encoded.len()).saturating_add(separator);
        let newest = start == messages.len();
        if next > max_bytes && !newest {
            break;
        }
        used = next;
        start = index;
        if next > max_bytes {
            break;
        }
    }
    &messages[start..]
}

/// Exact source identity of one summarised window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SummaryKey {
    pub(crate) session: SessionId,
    pub(crate) canonical_message_cursor: u64,
    pub(crate) model: String,
    /// SHA-256 over the windowed messages as serialized for the summariser,
    /// so a rewrite that keeps the cursor cannot reuse a stale summary.
    pub(crate) input_digest: [u8; 32],
}

impl SummaryKey {
    pub(crate) fn new(
        session: &SessionId,
        canonical_message_cursor: u64,
        model: &str,
        window: &[Message],
    ) -> Result<Self, serde_json::Error> {
        let mut hasher = Sha256::new();
        hasher.update(serde_json::to_vec(window)?);
        Ok(Self {
            session: session.clone(),
            canonical_message_cursor,
            model: model.to_string(),
            input_digest: hasher.finalize().into(),
        })
    }
}

/// Bounded, most-recent-first store of produced summaries keyed by the exact
/// windowed source. A session holds at most one entry; a new key for the same
/// session replaces the old one, and the oldest session is evicted at the
/// bound.
#[derive(Default)]
pub(crate) struct SummaryCache {
    entries: Mutex<VecDeque<(SummaryKey, String)>>,
}

impl SummaryCache {
    pub(crate) fn get(&self, key: &SummaryKey) -> Option<String> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries
            .iter()
            .find(|(cached, _)| cached == key)
            .map(|(_, text)| text.clone())
    }

    pub(crate) fn insert(&self, key: SummaryKey, text: String) {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|(cached, _)| cached.session != key.session);
        while entries.len() >= MAX_CACHED_SUMMARIES {
            entries.pop_front();
        }
        entries.push_back((key, text));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use meerkat_core::types::UserMessage;

    /// Role is irrelevant to a size bound; two user rows per turn keep the
    /// fixture independent of assistant block construction.
    fn transcript(turns: usize, words: usize) -> Vec<Message> {
        (0..turns)
            .flat_map(|turn| {
                let body = format!("turn {turn} ").repeat(words);
                [
                    Message::User(UserMessage::text(format!("question {turn}: {body}"))),
                    Message::User(UserMessage::text(format!("answer {turn}: {body}"))),
                ]
            })
            .collect()
    }

    fn serialized_len(messages: &[Message]) -> usize {
        serde_json::to_vec(messages).expect("serialize").len()
    }

    #[test]
    fn recent_window_keeps_the_newest_messages_within_the_exact_serialized_bound() {
        let messages = transcript(40, 12);
        let full = serialized_len(&messages);
        let window = recent_window(&messages, full / 3);
        assert!(!window.is_empty());
        assert!(window.len() < messages.len());
        assert!(serialized_len(window) <= full / 3, "window fits the bound");
        // Adding the next-older message would exceed the bound.
        let start = messages.len() - window.len();
        assert!(serialized_len(&messages[start - 1..]) > full / 3);
        // The suffix is the newest part of the transcript, whole messages only.
        assert_eq!(window.last(), messages.last());
        assert_eq!(window, &messages[start..]);
    }

    #[test]
    fn recent_window_returns_everything_when_it_fits_and_degrades_to_the_newest_message() {
        let messages = transcript(3, 4);
        assert_eq!(
            recent_window(&messages, serialized_len(&messages)),
            &messages[..]
        );
        // The newest message alone exceeds the bound: the window is exactly
        // that one whole message, not an empty slice and not a cut message.
        let oversized = recent_window(&messages, 8);
        assert_eq!(oversized.len(), 1);
        assert_eq!(oversized.last(), messages.last());
        assert!(serialized_len(oversized) > 8);
        // A bound that fits the newest message but not the next-older one
        // yields exactly the newest message as well.
        let newest_only = serialized_len(&messages[messages.len() - 1..]);
        assert_eq!(recent_window(&messages, newest_only).len(), 1);
        assert!(recent_window(&[], 1024).is_empty());
    }

    #[test]
    fn recent_window_keeps_a_large_newest_tool_result_whole() {
        let mut messages = transcript(4, 4);
        let large = "x".repeat(96 * 1024);
        messages.push(Message::User(UserMessage::text(format!(
            "tool result: {large}"
        ))));
        let window = recent_window(&messages, 64 * 1024);
        assert_eq!(window.len(), 1);
        assert_eq!(window[0], messages[messages.len() - 1]);
        assert!(serialized_len(window) > 64 * 1024, "no content is cut");
    }

    #[test]
    fn summary_key_changes_with_content_cursor_and_model() {
        let session = SessionId::new();
        let messages = transcript(2, 3);
        let base = SummaryKey::new(&session, 4, "gpt-5.5", &messages).expect("key");
        assert_eq!(
            base,
            SummaryKey::new(&session, 4, "gpt-5.5", &messages).expect("key")
        );
        assert_ne!(
            base,
            SummaryKey::new(&session, 5, "gpt-5.5", &messages).expect("key")
        );
        assert_ne!(
            base,
            SummaryKey::new(&session, 4, "gpt-5.4-mini", &messages).expect("key")
        );
        let mut rewritten = messages.clone();
        rewritten[0] = Message::User(UserMessage::text("rewritten"));
        assert_ne!(
            base,
            SummaryKey::new(&session, 4, "gpt-5.5", &rewritten).expect("key")
        );
        assert_ne!(
            base,
            SummaryKey::new(&SessionId::new(), 4, "gpt-5.5", &messages).expect("key")
        );
    }

    #[test]
    fn summary_cache_holds_one_entry_per_session_and_stays_bounded() {
        let cache = SummaryCache::default();
        let session = SessionId::new();
        let messages = transcript(2, 3);
        let first = SummaryKey::new(&session, 4, "gpt-5.5", &messages).expect("key");
        assert!(cache.get(&first).is_none());
        cache.insert(first.clone(), "first".to_string());
        assert_eq!(cache.get(&first).as_deref(), Some("first"));
        let advanced = SummaryKey::new(&session, 6, "gpt-5.5", &messages).expect("key");
        cache.insert(advanced.clone(), "second".to_string());
        assert_eq!(cache.len(), 1, "a session keeps only its newest summary");
        assert!(cache.get(&first).is_none());
        assert_eq!(cache.get(&advanced).as_deref(), Some("second"));
        for _ in 0..MAX_CACHED_SUMMARIES {
            let key = SummaryKey::new(&SessionId::new(), 1, "gpt-5.5", &messages).expect("key");
            cache.insert(key, "other".to_string());
        }
        assert_eq!(cache.len(), MAX_CACHED_SUMMARIES);
        assert!(
            cache.get(&advanced).is_none(),
            "the oldest session is evicted first"
        );
    }
}
