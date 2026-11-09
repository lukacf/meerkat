//! Provisional assistant captions for console voice calls.
//!
//! Under `PublicGptLivePlaybackPolicy::ProviderManagedUnmeasured` Meerkat
//! commits one canonical assistant row per provider turn segment, so the row
//! lands when the segment seals, not while the agent speaks. Meerkat publishes
//! the in-progress text of each open segment to a host caption sink, keyed by
//! the segment's item id: the same id the committed row later records in
//! `realtime_origin.provider_item_ids`.
//!
//! [`VoiceCaptionHub`] is that sink for console voice. It is called
//! synchronously on the provider observation path, so it never waits on the
//! browser: each caption replaces the item's entry in a bounded per-channel
//! buffer and wakes readers. The browser reads the buffer through
//! `meta/console/voice/captions`, a request-scoped long poll with a
//! monotonic cursor. Entries are coalesced per item (the caption carries the
//! segment's whole text), so a slow or retried read still gets the newest
//! text, and a retraction replaces the item's entry with a typed tombstone.
//! Captions are display state only: dropping one never affects canonical
//! history, and the committed row, not the caption, is what the console keeps.
//!
//! The same stream carries Meerkat's barge-in playback hint
//! (`live/assistant_playback_hint`): `duck` when the user's speech overlaps
//! audible assistant audio, `restore` when the overlap ends. The browser has
//! no other way to stop audio the provider already queued, so it applies the
//! hint to its own playback gain. A hint is channel state, not a segment:
//! only the newest one is retained, so a late or retried read still gets the
//! current state.

use serde::{Deserialize, Serialize};

/// Longest wait one caption read may request.
pub(crate) const MAX_CAPTION_WAIT_MS: u64 = 15_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VoiceCaptionsRequest {
    pub identity: String,
    pub request_id: String,
    pub channel_id: String,
    /// Cursor of the last caption batch the caller applied; `0` reads
    /// everything retained for the channel.
    #[serde(default)]
    pub after: u64,
    /// How long to wait for a caption newer than `after` before answering
    /// with an empty batch. Capped at [`MAX_CAPTION_WAIT_MS`].
    #[serde(default)]
    pub wait_ms: u64,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct VoiceCaptions {
    pub identity: String,
    pub request_id: String,
    pub channel_id: String,
    #[serde(flatten)]
    pub batch: VoiceCaptionBatch,
}

/// Captions newer than the caller's cursor, oldest first.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct VoiceCaptionBatch {
    /// Pass this as `after` on the next read.
    pub cursor: u64,
    pub captions: Vec<VoiceCaption>,
}

/// The newest state of one provisional assistant segment, or the channel's
/// current playback hint.
#[cfg_attr(not(feature = "openai-live"), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum VoiceCaption {
    /// Everything the segment has said so far; replaces earlier text.
    Caption { item_id: String, text: String },
    /// No committed row will replace this segment's caption; drop it.
    Retracted { item_id: String },
    /// Duck or restore the assistant's playback (barge-in).
    PlaybackHint { hint: VoicePlaybackHint },
}

/// The playback a barge-in hint asks the browser for.
#[cfg_attr(not(feature = "openai-live"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VoicePlaybackHint {
    /// Silence assistant playback: the user is speaking over it.
    Duck,
    /// Play assistant audio normally again.
    Restore,
}

#[cfg(feature = "openai-live")]
pub(crate) use hub::{VoiceCaptionHub, VoiceCaptionRegistration};

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn shared_captions_fixture_matches_the_rust_request_and_response()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/console_voice_v1.json"))?;
        assert_eq!(
            fixture["captions_method"],
            super::super::VOICE_CAPTIONS_METHOD
        );
        let request: VoiceCaptionsRequest =
            serde_json::from_value(fixture["captions_request"].clone())?;
        assert_eq!(
            fixture["captions_request"],
            serde_json::json!({
                "identity": request.identity, "request_id": request.request_id,
                "channel_id": request.channel_id, "after": request.after, "wait_ms": request.wait_ms,
            })
        );
        let response = VoiceCaptions {
            identity: request.identity,
            request_id: request.request_id,
            channel_id: request.channel_id,
            batch: VoiceCaptionBatch {
                cursor: 7,
                captions: vec![
                    VoiceCaption::Caption {
                        item_id: "segment-a".to_string(),
                        text: "The vault phrase is amber.".to_string(),
                    },
                    VoiceCaption::Retracted {
                        item_id: "segment-b".to_string(),
                    },
                    VoiceCaption::PlaybackHint {
                        hint: VoicePlaybackHint::Duck,
                    },
                ],
            },
        };
        assert_eq!(
            serde_json::to_value(response)?,
            fixture["captions_response"]
        );
        let defaults: VoiceCaptionsRequest = serde_json::from_value(serde_json::json!({
            "identity": "agent-a", "request_id": "voice-request", "channel_id": "voice-channel-a",
        }))?;
        assert_eq!((defaults.after, defaults.wait_ms), (0, 0));
        assert!(
            serde_json::from_value::<VoiceCaptionsRequest>(serde_json::json!({
                "identity": "agent-a", "request_id": "voice-request", "channel_id": "voice-channel-a",
                "session_id": "not-a-caller-field",
            }))
            .is_err()
        );
        Ok(())
    }
}

#[cfg(feature = "openai-live")]
mod hub {
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use meerkat_core::SessionId;
    use tokio::sync::Notify;

    use super::{VoiceCaption, VoiceCaptionBatch, VoicePlaybackHint};
    use crate::console_voice::VoiceError;

    /// Channels retained per registered session: the current one and the
    /// ones it replaced during recovery.
    pub(crate) const MAX_CHANNELS_PER_SESSION: usize = 4;
    /// Segments retained per channel. One provider turn is one segment unless
    /// a typed boundary split it, so this spans many turns; the oldest entry
    /// is evicted first. Evicting a caption loses display state only.
    pub(crate) const MAX_ITEMS_PER_CHANNEL: usize = 64;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum ItemState {
        Caption(String),
        Retracted,
    }

    struct Item {
        item_id: String,
        sequence: u64,
        state: ItemState,
    }

    struct Channel {
        channel_id: String,
        /// Ordered by `sequence`, oldest first.
        items: VecDeque<Item>,
        /// The newest playback hint and its sequence. Channel state, never
        /// evicted by segment captions.
        playback_hint: Option<(u64, VoicePlaybackHint)>,
    }

    struct Registration {
        token: u64,
        closed: bool,
        changed: Arc<Notify>,
        /// Oldest first.
        channels: VecDeque<Channel>,
    }

    /// Bounded caption buffer shared by every console voice session of one
    /// live host. Captions for sessions without a registered console call
    /// are dropped.
    #[derive(Default)]
    pub(crate) struct VoiceCaptionHub {
        sessions: Mutex<HashMap<SessionId, Registration>>,
        /// Process-wide, so a cursor never repeats across registrations.
        sequence: AtomicU64,
        tokens: AtomicU64,
    }

    impl VoiceCaptionHub {
        /// Start retaining captions for one console voice call on `session`.
        /// A newer registration for the same session replaces an older one.
        pub(crate) fn register(self: &Arc<Self>, session: &SessionId) -> VoiceCaptionRegistration {
            let token = self.tokens.fetch_add(1, Ordering::Relaxed) + 1;
            let changed = Arc::new(Notify::new());
            let previous = self.lock().insert(
                session.clone(),
                Registration {
                    token,
                    closed: false,
                    changed: Arc::clone(&changed),
                    channels: VecDeque::new(),
                },
            );
            if let Some(previous) = previous {
                previous.changed.notify_waiters();
            }
            VoiceCaptionRegistration {
                hub: Arc::clone(self),
                session: session.clone(),
                token,
                changed,
            }
        }

        /// Replace the caption of `item_id` with the segment's whole text.
        /// Never waits: one short critical section, then a wake-up.
        pub(crate) fn publish_text(
            &self,
            session: &SessionId,
            channel: &str,
            item_id: &str,
            text: &str,
        ) {
            self.record(
                session,
                channel,
                item_id,
                ItemState::Caption(text.to_string()),
            );
        }

        /// Drop the caption of `item_id`: no committed row will replace it.
        pub(crate) fn retract_item(&self, session: &SessionId, channel: &str, item_id: &str) {
            self.record(session, channel, item_id, ItemState::Retracted);
        }

        /// Replace the channel's playback hint. Never waits, like captions.
        pub(crate) fn publish_playback_hint(
            &self,
            session: &SessionId,
            channel: &str,
            hint: VoicePlaybackHint,
        ) {
            let mut sessions = self.lock();
            let Some(registration) = sessions.get_mut(session).filter(|entry| !entry.closed) else {
                return;
            };
            let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
            Self::channel_entry(registration, channel).playback_hint = Some((sequence, hint));
            let changed = Arc::clone(&registration.changed);
            drop(sessions);
            changed.notify_waiters();
        }

        fn channel_entry<'a>(registration: &'a mut Registration, channel: &str) -> &'a mut Channel {
            let position = registration
                .channels
                .iter()
                .position(|entry| entry.channel_id == channel);
            if let Some(position) = position {
                &mut registration.channels[position]
            } else {
                if registration.channels.len() >= MAX_CHANNELS_PER_SESSION {
                    registration.channels.pop_front();
                }
                registration.channels.push_back(Channel {
                    channel_id: channel.to_string(),
                    items: VecDeque::new(),
                    playback_hint: None,
                });
                let last = registration.channels.len() - 1;
                &mut registration.channels[last]
            }
        }

        fn record(&self, session: &SessionId, channel: &str, item_id: &str, state: ItemState) {
            let mut sessions = self.lock();
            let Some(registration) = sessions.get_mut(session).filter(|entry| !entry.closed) else {
                return;
            };
            let sequence = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
            let entry = Self::channel_entry(registration, channel);
            entry.items.retain(|item| item.item_id != item_id);
            if entry.items.len() >= MAX_ITEMS_PER_CHANNEL {
                entry.items.pop_front();
            }
            entry.items.push_back(Item {
                item_id: item_id.to_string(),
                sequence,
                state,
            });
            let changed = Arc::clone(&registration.changed);
            drop(sessions);
            changed.notify_waiters();
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<SessionId, Registration>> {
            self.sessions.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }

    /// One console voice call's view of the hub. Dropping it stops retention.
    pub(crate) struct VoiceCaptionRegistration {
        hub: Arc<VoiceCaptionHub>,
        session: SessionId,
        token: u64,
        changed: Arc<Notify>,
    }

    impl VoiceCaptionRegistration {
        /// Captions of `channel` newer than `after`, without waiting.
        pub(crate) fn read(
            &self,
            channel: &str,
            after: u64,
        ) -> Result<VoiceCaptionBatch, VoiceError> {
            let sessions = self.hub.lock();
            let registration = sessions
                .get(&self.session)
                .filter(|entry| entry.token == self.token && !entry.closed)
                .ok_or(VoiceError::Closed)?;
            let mut batch = VoiceCaptionBatch {
                cursor: after,
                captions: Vec::new(),
            };
            let Some(entry) = registration
                .channels
                .iter()
                .find(|entry| entry.channel_id == channel)
            else {
                return Ok(batch);
            };
            let mut newer: Vec<(u64, VoiceCaption)> = entry
                .items
                .iter()
                .filter(|item| item.sequence > after)
                .map(|item| {
                    (
                        item.sequence,
                        match &item.state {
                            ItemState::Caption(text) => VoiceCaption::Caption {
                                item_id: item.item_id.clone(),
                                text: text.clone(),
                            },
                            ItemState::Retracted => VoiceCaption::Retracted {
                                item_id: item.item_id.clone(),
                            },
                        },
                    )
                })
                .collect();
            if let Some((sequence, hint)) = entry.playback_hint
                && sequence > after
            {
                newer.push((sequence, VoiceCaption::PlaybackHint { hint }));
            }
            newer.sort_by_key(|(sequence, _)| *sequence);
            for (sequence, caption) in newer {
                batch.cursor = batch.cursor.max(sequence);
                batch.captions.push(caption);
            }
            Ok(batch)
        }

        /// Wait up to `wait` for captions of `channel` newer than `after`.
        /// Answers early when one arrives or the registration closes.
        pub(crate) async fn wait_read(
            &self,
            channel: &str,
            after: u64,
            wait: Duration,
        ) -> Result<VoiceCaptionBatch, VoiceError> {
            let deadline = tokio::time::Instant::now() + wait;
            loop {
                let notified = self.changed.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let batch = self.read(channel, after)?;
                if !batch.captions.is_empty() || tokio::time::Instant::now() >= deadline {
                    return Ok(batch);
                }
                tokio::select! {
                    () = &mut notified => {}
                    () = tokio::time::sleep_until(deadline) => {}
                }
            }
        }

        /// Stop retaining captions and wake every waiting read.
        pub(crate) fn close(&self) {
            let mut sessions = self.hub.lock();
            if let Some(registration) = sessions
                .get_mut(&self.session)
                .filter(|entry| entry.token == self.token)
            {
                registration.closed = true;
                registration.channels.clear();
            }
            drop(sessions);
            self.changed.notify_waiters();
        }
    }

    impl Drop for VoiceCaptionRegistration {
        fn drop(&mut self) {
            let mut sessions = self.hub.lock();
            if sessions
                .get(&self.session)
                .is_some_and(|entry| entry.token == self.token)
            {
                sessions.remove(&self.session);
            }
            drop(sessions);
            self.changed.notify_waiters();
        }
    }

    #[cfg(test)]
    #[allow(clippy::expect_used)]
    mod tests {
        use super::*;

        fn session() -> SessionId {
            SessionId::new()
        }

        fn caption(item_id: &str, text: &str) -> VoiceCaption {
            VoiceCaption::Caption {
                item_id: item_id.to_string(),
                text: text.to_string(),
            }
        }

        /// The barge-in playback hint rides the caption stream in sequence
        /// order. Only the newest hint is retained (a late read gets the
        /// current state), segment captions never evict it, and a hint for a
        /// session without a console call is dropped.
        #[test]
        fn playback_hints_ride_the_stream_newest_only_and_survive_eviction() {
            let hub = Arc::new(VoiceCaptionHub::default());
            let session = session();
            let registration = hub.register(&session);
            hub.publish_text(&session, "channel-a", "segment-a", "Here is the long");
            hub.publish_playback_hint(&session, "channel-a", VoicePlaybackHint::Duck);
            hub.publish_text(
                &session,
                "channel-a",
                "segment-a",
                "Here is the long readout",
            );
            let batch = registration.read("channel-a", 0).expect("read");
            assert_eq!(
                batch.captions,
                vec![
                    VoiceCaption::PlaybackHint {
                        hint: VoicePlaybackHint::Duck
                    },
                    caption("segment-a", "Here is the long readout"),
                ],
                "in sequence order; the segment's caption is coalesced"
            );
            let cursor = batch.cursor;

            hub.publish_playback_hint(&session, "channel-a", VoicePlaybackHint::Restore);
            assert_eq!(
                registration
                    .read("channel-a", cursor)
                    .expect("read")
                    .captions,
                vec![VoiceCaption::PlaybackHint {
                    hint: VoicePlaybackHint::Restore
                }]
            );
            assert_eq!(
                registration
                    .read("channel-a", 0)
                    .expect("read")
                    .captions
                    .iter()
                    .filter(|caption| matches!(caption, VoiceCaption::PlaybackHint { .. }))
                    .collect::<Vec<_>>(),
                vec![&VoiceCaption::PlaybackHint {
                    hint: VoicePlaybackHint::Restore
                }],
                "only the newest hint is retained"
            );

            for index in 0..(MAX_ITEMS_PER_CHANNEL + 4) {
                hub.publish_text(&session, "channel-a", &format!("segment-{index}"), "text");
            }
            assert!(
                registration
                    .read("channel-a", 0)
                    .expect("read")
                    .captions
                    .contains(&VoiceCaption::PlaybackHint {
                        hint: VoicePlaybackHint::Restore
                    }),
                "segment captions never evict the channel's hint"
            );

            let other = SessionId::new();
            hub.publish_playback_hint(&other, "channel-a", VoicePlaybackHint::Duck);
            assert!(hub.lock().get(&other).is_none(), "no console call, no hint");
        }

        #[test]
        fn playback_hints_serialize_as_typed_caption_entries() {
            assert_eq!(
                serde_json::to_value(VoiceCaption::PlaybackHint {
                    hint: VoicePlaybackHint::Duck
                })
                .expect("serialize"),
                serde_json::json!({"kind": "playback_hint", "hint": "duck"})
            );
            assert_eq!(
                serde_json::to_value(VoiceCaption::PlaybackHint {
                    hint: VoicePlaybackHint::Restore
                })
                .expect("serialize"),
                serde_json::json!({"kind": "playback_hint", "hint": "restore"})
            );
        }

        #[test]
        fn captions_forward_in_order_and_replace_per_item() {
            let hub = Arc::new(VoiceCaptionHub::default());
            let session = session();
            let registration = hub.register(&session);
            hub.publish_text(&session, "channel-a", "item-1", "The vault");
            hub.publish_text(&session, "channel-a", "item-1", "The vault phrase");
            hub.publish_text(&session, "channel-a", "item-2", "Next");
            let first = registration.read("channel-a", 0).expect("read");
            assert_eq!(
                first.captions,
                vec![
                    caption("item-1", "The vault phrase"),
                    caption("item-2", "Next")
                ],
                "each item carries its newest whole text, oldest item first"
            );
            assert_eq!(
                registration.read("channel-a", first.cursor).expect("read"),
                VoiceCaptionBatch {
                    cursor: first.cursor,
                    captions: Vec::new()
                },
                "the cursor fences what the caller already applied"
            );
            hub.publish_text(&session, "channel-a", "item-1", "The vault phrase is amber");
            let second = registration.read("channel-a", first.cursor).expect("read");
            assert_eq!(
                second.captions,
                vec![caption("item-1", "The vault phrase is amber")]
            );
            assert!(second.cursor > first.cursor);
            assert!(
                registration
                    .read("channel-b", 0)
                    .expect("read")
                    .captions
                    .is_empty(),
                "captions are scoped to their channel"
            );
        }

        #[test]
        fn retract_drops_the_caption_of_its_item_only() {
            let hub = Arc::new(VoiceCaptionHub::default());
            let session = session();
            let registration = hub.register(&session);
            hub.publish_text(&session, "channel-a", "item-1", "Kept");
            hub.publish_text(&session, "channel-a", "item-2", "Never committed");
            let seen = registration.read("channel-a", 0).expect("read").cursor;
            hub.retract_item(&session, "channel-a", "item-2");
            let batch = registration.read("channel-a", seen).expect("read");
            assert_eq!(
                batch.captions,
                vec![VoiceCaption::Retracted {
                    item_id: "item-2".to_string()
                }],
                "a caller that showed the caption learns to drop it"
            );
            assert_eq!(
                registration.read("channel-a", 0).expect("read").captions,
                vec![
                    caption("item-1", "Kept"),
                    VoiceCaption::Retracted {
                        item_id: "item-2".to_string()
                    }
                ],
                "a fresh reader never sees the retracted text"
            );
        }

        #[test]
        fn captions_of_unregistered_or_closed_calls_are_dropped_and_bounded() {
            let hub = Arc::new(VoiceCaptionHub::default());
            let session = session();
            hub.publish_text(&session, "channel-a", "early", "No console call yet");
            let registration = hub.register(&session);
            assert!(
                registration
                    .read("channel-a", 0)
                    .expect("read")
                    .captions
                    .is_empty()
            );
            for index in 0..(MAX_ITEMS_PER_CHANNEL + 5) {
                hub.publish_text(&session, "channel-a", &format!("item-{index}"), "text");
            }
            let batch = registration.read("channel-a", 0).expect("read");
            assert_eq!(batch.captions.len(), MAX_ITEMS_PER_CHANNEL);
            assert_eq!(
                batch.captions.first(),
                Some(&caption("item-5", "text")),
                "the oldest items are evicted first"
            );
            for index in 0..=MAX_CHANNELS_PER_SESSION {
                hub.publish_text(&session, &format!("replacement-{index}"), "item", "text");
            }
            assert!(
                registration
                    .read("channel-a", 0)
                    .expect("read")
                    .captions
                    .is_empty(),
                "the oldest channel is evicted first"
            );
            registration.close();
            assert_eq!(
                registration.read("replacement-4", 0),
                Err(VoiceError::Closed)
            );
            hub.publish_text(&session, "replacement-4", "late", "after close");
            drop(registration);
            assert!(
                hub.lock().is_empty(),
                "dropping the registration releases it"
            );
        }

        #[test]
        fn a_newer_registration_is_not_released_by_the_older_one() {
            let hub = Arc::new(VoiceCaptionHub::default());
            let session = session();
            let older = hub.register(&session);
            let newer = hub.register(&session);
            assert_eq!(older.read("channel-a", 0), Err(VoiceError::Closed));
            drop(older);
            hub.publish_text(&session, "channel-a", "item", "text");
            assert_eq!(
                newer.read("channel-a", 0).expect("read").captions,
                vec![caption("item", "text")]
            );
        }

        #[tokio::test]
        async fn a_waiting_read_wakes_on_a_caption_and_on_close() {
            let hub = Arc::new(VoiceCaptionHub::default());
            let session = session();
            let registration = Arc::new(hub.register(&session));
            let reader = Arc::clone(&registration);
            let waiting = tokio::spawn(async move {
                reader
                    .wait_read("channel-a", 0, Duration::from_secs(30))
                    .await
            });
            tokio::task::yield_now().await;
            hub.publish_text(&session, "channel-a", "item", "Spoken");
            let batch = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .expect("the caption wakes the read")
                .expect("join")
                .expect("batch");
            assert_eq!(batch.captions, vec![caption("item", "Spoken")]);

            let empty = registration
                .wait_read("channel-a", batch.cursor, Duration::from_millis(20))
                .await
                .expect("an expired wait answers");
            assert!(empty.captions.is_empty());
            assert_eq!(empty.cursor, batch.cursor);

            let reader = Arc::clone(&registration);
            let waiting = tokio::spawn(async move {
                reader
                    .wait_read("channel-a", batch.cursor, Duration::from_secs(30))
                    .await
            });
            tokio::task::yield_now().await;
            registration.close();
            let closed = tokio::time::timeout(Duration::from_secs(5), waiting)
                .await
                .expect("close wakes the read")
                .expect("join");
            assert_eq!(closed, Err(VoiceError::Closed));
        }
    }
}
