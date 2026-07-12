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
use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, ProfileName};
#[cfg(target_arch = "wasm32")]
use crate::tokio;

use super::MobHandle;
use futures::stream::{SelectAll, StreamExt};
use std::collections::{BTreeSet, HashMap};
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

#[derive(Clone)]
pub(super) struct AuthorizedMobEventRouter {
    pub initial_cursor: u64,
    pub config: MobEventRouterConfig,
    pub session_bound_runtimes: BTreeSet<crate::machines::mob_machine::AgentRuntimeId>,
    /// Placed members whose events fan in through pump taps (phase 6,
    /// DEC-P6E-12 kill site 4 — the mob-wide stream INCLUDES remote
    /// members). Built from the machine's placement facts at the
    /// subscribe call site.
    pub external_members: BTreeSet<crate::machines::mob_machine::AgentIdentity>,
}

#[derive(Clone)]
pub(super) struct AuthorizedMobEventRouterMember {
    pub agent_identity: AgentIdentity,
    pub runtime_id: AgentRuntimeId,
    pub fence_token: FenceToken,
    pub session_id: meerkat_core::types::SessionId,
    pub role: ProfileName,
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
    authority: AuthorizedMobEventRouter,
) -> MobEventRouterHandle {
    let (event_tx, event_rx) = mpsc::channel(authority.config.channel_capacity);
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    tokio::spawn(async move {
        run_event_router(handle, authority, event_tx, cancel_clone).await;
    });

    MobEventRouterHandle { event_rx, cancel }
}

#[allow(clippy::ignored_unit_patterns)]
async fn run_event_router(
    handle: MobHandle,
    authority: AuthorizedMobEventRouter,
    event_tx: mpsc::Sender<AttributedEvent>,
    cancel: CancellationToken,
) {
    let mut merged: SelectAll<TaggedStream> = SelectAll::new();
    // Track the SUBSCRIBED incarnation per identity: a respawn (ADJ-24)
    // replaces the member's stream, so re-subscription keys on the runtime
    // id, never on bare identity presence.
    let mut tracked_ids: HashMap<AgentIdentity, AgentRuntimeId> = HashMap::new();
    let mut mob_cursor: u64 = authority.initial_cursor;

    {
        for member in handle
            .authorized_mob_event_router_members(&authority.session_bound_runtimes)
            .await
        {
            if tracked_ids.contains_key(&member.agent_identity) {
                continue;
            }
            if let Some(stream) = subscribe_member(&handle, member.clone()).await {
                tracked_ids.insert(member.agent_identity, member.runtime_id);
                merged.push(stream);
            }
        }
        // Placed members fan in through pump taps — shape-identical items
        // in the SAME merge (phase 6).
        for dsl_identity in &authority.external_members {
            let member_identity = AgentIdentity::from(dsl_identity.0.as_str());
            if tracked_ids.contains_key(&member_identity) {
                continue;
            }
            let Some(runtime_id) = handle.member_runtime_id_observation(&member_identity) else {
                continue;
            };
            if let Some(stream) = subscribe_external_member(&handle, &member_identity).await {
                tracked_ids.insert(member_identity, runtime_id);
                merged.push(stream);
            }
        }
    }

    let mut poll_interval = tokio::time::interval(authority.config.poll_interval);
    #[cfg(not(target_arch = "wasm32"))]
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            () = cancel.cancelled() => break,

            // Forward attributed events from member streams.
            Some((runtime_id, fence, profile, envelope)) = merged.next() => {
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
                            let member_identity =
                                crate::ids::AgentIdentity::from(event.agent_identity.as_str());
                            // A respawned identity (ADJ-24) arrives with a
                            // NEW runtime id: replace the subscription; the
                            // old stream ends with its torn-down source.
                            let already_current = tracked_ids
                                .get(&member_identity)
                                .is_some_and(|tracked| tracked == &event.agent_runtime_id);
                            if !already_current {
                                // Phase 6 roster-delta placement switch: a
                                // placed spawn fans in through its pump tap.
                                if handle.member_placement_present(&member_identity) {
                                    if let Some(stream) =
                                        subscribe_external_member(&handle, &member_identity).await
                                    {
                                        tracked_ids.insert(
                                            member_identity,
                                            event.agent_runtime_id.clone(),
                                        );
                                        merged.push(stream);
                                    }
                                    continue;
                                }
                                match handle
                                    .authorize_mob_event_router_member_subscription(
                                        &member_identity,
                                        &event.agent_runtime_id,
                                        event.fence_token,
                                        event.role.clone(),
                                    )
                                    .await
                                {
                                    Ok(member) => {
                                        if let Some(stream) = subscribe_member(&handle, member.clone()).await {
                                            tracked_ids
                                                .insert(member.agent_identity, member.runtime_id);
                                            merged.push(stream);
                                        }
                                    }
                                    Err(error) => {
                                        tracing::warn!(
                                            agent_identity = %member_identity,
                                            error = %error,
                                            "mob event router: MobMachine rejected spawned member event subscription",
                                        );
                                    }
                                }
                            }
                        }
                        crate::event::MobEventKind::MemberRetired {
                            ref agent_identity,
                            ..
                        } => {
                            let member_identity =
                                crate::ids::AgentIdentity::from(agent_identity.as_str());
                            match handle
                                .authorize_mob_event_router_member_removal(&member_identity)
                                .await
                            {
                                Ok(true) => {
                                    tracked_ids.remove(&member_identity);
                                }
                                Ok(false) => {}
                                Err(error) => {
                                    tracing::warn!(
                                        agent_identity = %member_identity,
                                        error = %error,
                                        "mob event router: MobMachine rejected retired member removal",
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

/// A tagged stream that yields (AgentRuntimeId, FenceToken, ProfileName, EventEnvelope).
type TaggedItem = (
    AgentRuntimeId,
    FenceToken,
    ProfileName,
    meerkat_core::event::EventEnvelope<meerkat_core::event::AgentEvent>,
);
/// Boxed so local session streams and remote pump-tap streams merge in the
/// SAME `SelectAll` (phase 6 — DEC-P6E-12).
type TaggedStream = std::pin::Pin<Box<dyn futures::Stream<Item = TaggedItem> + Send>>;

async fn subscribe_member(
    handle: &MobHandle,
    member: AuthorizedMobEventRouterMember,
) -> Option<TaggedStream> {
    let stream = match handle
        .subscribe_authorized_agent_session_events(&member.agent_identity, &member.session_id)
        .await
    {
        Ok(stream) => stream,
        Err(error) => {
            tracing::warn!(
                agent_identity = %member.agent_identity,
                error = %error,
                "mob event router: failed to subscribe to member agent events",
            );
            return None;
        }
    };
    let prof = member.role;
    let source_runtime_id = member.runtime_id;
    let source_fence_token = member.fence_token;
    Some(Box::pin(stream.map(move |envelope| {
        (
            source_runtime_id.clone(),
            source_fence_token,
            prof.clone(),
            envelope,
        )
    })))
}

/// Pump-tap stream for one placed member: the tap already carries full
/// attribution, so the mapping is a field re-shape.
async fn subscribe_external_member(
    handle: &MobHandle,
    member_identity: &AgentIdentity,
) -> Option<TaggedStream> {
    let tap = match handle.external_member_event_tap(member_identity).await {
        Ok(tap) => tap,
        Err(error) => {
            tracing::warn!(
                agent_identity = %member_identity,
                error = %error,
                "mob event router: failed to open external member event tap",
            );
            return None;
        }
    };
    Some(Box::pin(futures::stream::unfold(
        tap,
        |mut tap| async move {
            tap.recv().await.map(|attributed| {
                (
                    (
                        attributed.source,
                        attributed.source_fence_token,
                        attributed.role,
                        attributed.envelope,
                    ),
                    tap,
                )
            })
        },
    )))
}
