//! Mob-level event bus that merges per-member session streams into a
//! single `mpsc` channel of [`AttributedEvent`]s.
//!
//! The router runs as an independent tokio task:
//! 1. Bootstraps by subscribing to all current roster members.
//! 2. Polls the [`MobEventStore`] for `MeerkatSpawned`/`MeerkatRetired` to
//!    track roster changes and subscribe/unsubscribe streams.
//! 3. Tags events with [`AttributedEvent`] and forwards to the receiver.
//!
//! Streams for retired members end naturally when sessions are archived.

use crate::event::AttributedEvent;
use crate::ids::{MeerkatId, ProfileName};
use crate::roster::Roster;
use crate::store::MobEventStore;
#[cfg(target_arch = "wasm32")]
use crate::tokio;

use super::session_service::MobSessionService;
use futures::stream::{SelectAll, StreamExt};
use meerkat_core::comms::EventStream;
use meerkat_core::service::SessionService;
use meerkat_core::types::SessionId;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc};
use tokio_util::sync::CancellationToken;

/// Configuration for the [`MobEventRouter`].
pub struct MobEventRouterConfig {
    /// How often to poll the mob event store for roster changes.
    pub poll_interval: Duration,
    /// Capacity of the output `mpsc` channel.
    pub channel_capacity: usize,
}

impl Default for MobEventRouterConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_millis(500),
            channel_capacity: 256,
        }
    }
}

/// Handle returned by [`spawn_event_router`]. Drop to stop the router.
pub struct MobEventRouterHandle {
    /// Receive attributed events from all mob members.
    pub event_rx: mpsc::Receiver<AttributedEvent>,
    cancel: CancellationToken,
}

impl MobEventRouterHandle {
    /// Explicitly cancel the router task.
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

impl Drop for MobEventRouterHandle {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Spawn the event router task and return its handle.
pub(super) fn spawn_event_router(
    session_service: Arc<dyn MobSessionService>,
    events: Arc<dyn MobEventStore>,
    roster: Arc<RwLock<Roster>>,
    config: MobEventRouterConfig,
) -> MobEventRouterHandle {
    let (event_tx, event_rx) = mpsc::channel(config.channel_capacity);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        run_event_router(
            session_service,
            events,
            roster,
            config,
            event_tx,
            cancel_clone,
        )
        .await;
    });

    MobEventRouterHandle { event_rx, cancel }
}

async fn run_event_router(
    session_service: Arc<dyn MobSessionService>,
    events: Arc<dyn MobEventStore>,
    roster: Arc<RwLock<Roster>>,
    config: MobEventRouterConfig,
    event_tx: mpsc::Sender<AttributedEvent>,
    cancel: CancellationToken,
) {
    let mut merged: SelectAll<TaggedStream> = SelectAll::new();
    let mut tracked_ids: HashSet<MeerkatId> = HashSet::new();
    let mut mob_cursor: u64 = 0;

    // Bootstrap: subscribe to all current roster members.
    {
        let roster_snap = roster.read().await;
        for entry in roster_snap.list() {
            if let Some(session_id) = entry.member_ref.session_id()
                && tracked_ids.insert(entry.meerkat_id.clone())
                && let Some(stream) = subscribe_member(
                    &session_service,
                    session_id,
                    entry.meerkat_id.clone(),
                    entry.profile.clone(),
                )
                .await
            {
                merged.push(stream);
            }
        }
    }

    // Seed cursor from current event store.
    if let Ok(all_events) = events.poll(0, usize::MAX).await
        && let Some(last) = all_events.last()
    {
        mob_cursor = last.cursor;
    }

    let mut poll_interval = tokio::time::interval(config.poll_interval);
    #[cfg(not(target_arch = "wasm32"))]
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            () = cancel.cancelled() => break,

            // Forward attributed events from member streams.
            Some((meerkat_id, profile, envelope)) = merged.next() => {
                let attributed = AttributedEvent {
                    source: meerkat_id,
                    profile,
                    envelope,
                };
                if event_tx.send(attributed).await.is_err() {
                    // Receiver dropped — shut down.
                    break;
                }
            }

            // Poll mob events for roster changes.
            _ = poll_interval.tick() => {
                let new_events = match events.poll(mob_cursor, 100).await {
                    Ok(evts) => evts,
                    Err(_) => continue,
                };
                for mob_event in new_events {
                    mob_cursor = mob_event.cursor;
                    match mob_event.kind {
                        crate::event::MobEventKind::MeerkatSpawned {
                            meerkat_id,
                            role,
                            member_ref,
                            ..
                        } => {
                            if let Some(sid) = member_ref.session_id()
                                && tracked_ids.insert(meerkat_id.clone())
                                && let Some(stream) = subscribe_member(
                                    &session_service,
                                    sid,
                                    meerkat_id,
                                    role,
                                )
                                .await
                            {
                                merged.push(stream);
                            }
                        }
                        crate::event::MobEventKind::MeerkatRetired {
                            meerkat_id, ..
                        } => {
                            tracked_ids.remove(&meerkat_id);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

/// A tagged stream that yields (MeerkatId, ProfileName, EventEnvelope).
type TaggedItem = (
    MeerkatId,
    ProfileName,
    meerkat_core::event::EventEnvelope<meerkat_core::event::AgentEvent>,
);
type TaggedStream = futures::stream::Map<
    EventStream,
    Box<
        dyn FnMut(meerkat_core::event::EventEnvelope<meerkat_core::event::AgentEvent>) -> TaggedItem
            + Send,
    >,
>;

async fn subscribe_member(
    session_service: &Arc<dyn MobSessionService>,
    session_id: &SessionId,
    meerkat_id: MeerkatId,
    profile: ProfileName,
) -> Option<TaggedStream> {
    let stream = SessionService::subscribe_session_events(session_service.as_ref(), session_id)
        .await
        .ok()?;
    let mid = meerkat_id;
    let prof = profile;
    Some(stream.map(
        Box::new(move |envelope| (mid.clone(), prof.clone(), envelope))
            as Box<
                dyn FnMut(
                        meerkat_core::event::EventEnvelope<meerkat_core::event::AgentEvent>,
                    ) -> TaggedItem
                    + Send,
            >,
    ))
}
