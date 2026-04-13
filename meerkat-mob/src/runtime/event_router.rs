//! Mob-level event bus that merges per-member session streams into a
//! single `mpsc` channel of [`AttributedEvent`]s.
//!
//! The router runs as an independent tokio task:
//! 1. Bootstraps by subscribing to all current roster members.
//! 2. Polls the machine-routed mob event surface for
//!    `MemberSpawned`/`MemberRetired` to track roster changes and
//!    subscribe/unsubscribe streams.
//! 3. Tags events with [`AttributedEvent`] and forwards to the receiver.
//!
//! Streams for retired members end naturally when sessions are archived.

use crate::event::AttributedEvent;
use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, Generation, MeerkatId, ProfileName};
#[cfg(target_arch = "wasm32")]
use crate::tokio;

use super::MobHandle;
use futures::stream::{SelectAll, StreamExt};
use meerkat_core::comms::EventStream;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Configuration for the [`MobEventRouter`].
#[derive(Clone, Copy)]
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
    handle: MobHandle,
    config: MobEventRouterConfig,
) -> MobEventRouterHandle {
    let (event_tx, event_rx) = mpsc::channel(config.channel_capacity);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        run_event_router(handle, config, event_tx, cancel_clone).await;
    });

    MobEventRouterHandle { event_rx, cancel }
}

#[allow(clippy::ignored_unit_patterns)]
async fn run_event_router(
    handle: MobHandle,
    config: MobEventRouterConfig,
    event_tx: mpsc::Sender<AttributedEvent>,
    cancel: CancellationToken,
) {
    let mut merged: SelectAll<TaggedStream> = SelectAll::new();
    let mut tracked_ids: HashSet<MeerkatId> = HashSet::new();
    let mut mob_cursor: u64 = 0;

    // Bootstrap: subscribe to all current roster members.
    {
        for entry in handle.list_members().await {
            if tracked_ids.insert(entry.meerkat_id.clone())
                && let Some(stream) =
                    subscribe_member(&handle, entry.meerkat_id.clone(), entry.role.clone()).await
            {
                merged.push(stream);
            }
        }
    }

    // Seed cursor from current event store.
    if let Ok(all_events) = handle.poll_events(0, usize::MAX).await
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
                let identity = AgentIdentity::from(meerkat_id.as_str());
                let (runtime_id, fence) = {
                    let roster = handle.roster().await;
                    match roster.get_by_identity(&identity) {
                        Some(entry) => (entry.agent_runtime_id.clone(), entry.fence_token),
                        None => (
                            AgentRuntimeId::new(identity, Generation::INITIAL),
                            FenceToken::new(0),
                        ),
                    }
                };
                let attributed = AttributedEvent {
                    source: runtime_id,
                    source_fence_token: fence,
                    role: profile,
                    envelope,
                };
                if event_tx.send(attributed).await.is_err() {
                    // Receiver dropped — shut down.
                    break;
                }
            }

            // Poll mob events for roster changes.
            _ = poll_interval.tick() => {
                let new_events = match handle.poll_events(mob_cursor, 100).await {
                    Ok(evts) => evts,
                    Err(_) => continue,
                };
                for mob_event in new_events {
                    mob_cursor = mob_event.cursor;
                    match mob_event.kind {
                        crate::event::MobEventKind::MemberSpawned(ref event) => {
                            let meerkat_id =
                                crate::ids::MeerkatId::from(event.agent_identity.as_str());
                            if tracked_ids.insert(meerkat_id.clone())
                                && let Some(stream) =
                                    subscribe_member(&handle, meerkat_id, event.role.clone())
                                        .await
                            {
                                merged.push(stream);
                            }
                        }
                        crate::event::MobEventKind::MemberRetired {
                            ref agent_identity,
                            ..
                        } => {
                            let meerkat_id =
                                crate::ids::MeerkatId::from(agent_identity.as_str());
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
    handle: &MobHandle,
    meerkat_id: MeerkatId,
    profile: ProfileName,
) -> Option<TaggedStream> {
    let identity = AgentIdentity::from(meerkat_id.as_str());
    let stream = handle.subscribe_agent_events(&identity).await.ok()?;
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
