//! Mob-level event bus that merges per-member session streams into a
//! single `mpsc` channel of [`AttributedEvent`]s.
//!
//! The router runs as an independent tokio task:
//! 1. Bootstraps by subscribing to all current roster members.
//! 2. Follows the structural event-store subscription for causal
//!    `MemberSpawned`/`MemberRetired` wakeups, with a slow durable-cursor
//!    safety sweep for missed/cross-process signals.
//! 3. Tags events with [`AttributedEvent`] and forwards to the receiver.
//!
//! Streams for retired members end naturally when sessions are archived.

use crate::event::AttributedEvent;
use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, MobId, ProfileName};
#[cfg(target_arch = "wasm32")]
use crate::tokio;

use super::MobHandle;
use futures::stream::{SelectAll, StreamExt};
use meerkat_core::time_compat::Instant;
use std::collections::{BTreeSet, HashMap};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const EVENT_ROUTER_POLL_BATCH_LIMIT: usize = 100;
const EVENT_ROUTER_SAFETY_SWEEP_INTERVAL: Duration = Duration::from_secs(30);
const EVENT_ROUTER_MAX_RECOVERY_BACKOFF: Duration = Duration::from_secs(30);

/// Configuration for the [`MobEventRouter`].
#[derive(Clone, Copy)]
pub struct MobEventRouterConfig {
    /// Initial retry delay after a structural event-store read failure.
    ///
    /// Healthy routing is driven by the store's causal subscription, not by
    /// this interval. Repeated failures back off exponentially.
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

/// Durable-poll cadence for the structural event cursor.
///
/// The absolute deadline is kept outside `select!`: rebuilding a sleep future
/// after every broadcast must not postpone a due safety sweep or allow a
/// notification storm to bypass recovery backoff.
struct EventRouterPollCadence {
    deadline: super::ReconcileScanDeadline,
    base_recovery_backoff: Duration,
    next_recovery_backoff: Duration,
    recovering: bool,
}

impl EventRouterPollCadence {
    fn new(now: Instant, configured_base: Duration) -> Self {
        let base_recovery_backoff = configured_base
            .max(Duration::from_millis(1))
            .min(EVENT_ROUTER_MAX_RECOVERY_BACKOFF);
        Self {
            deadline: super::ReconcileScanDeadline::first_scan(now, Duration::ZERO),
            base_recovery_backoff,
            next_recovery_backoff: base_recovery_backoff,
            recovering: false,
        }
    }

    fn on_causal_wake(&mut self, now: Instant) {
        if !self.recovering {
            self.deadline.pull_earlier(now, Duration::ZERO);
        }
    }

    fn on_poll_succeeded(&mut self, now: Instant, more_may_remain: bool) {
        self.recovering = false;
        self.next_recovery_backoff = self.base_recovery_backoff;
        self.deadline.rearm(
            now,
            if more_may_remain {
                Duration::ZERO
            } else {
                EVENT_ROUTER_SAFETY_SWEEP_INTERVAL
            },
        );
    }

    fn on_poll_failed(&mut self, now: Instant) -> Duration {
        let retry_after = self.next_recovery_backoff;
        self.recovering = true;
        self.deadline.rearm(now, retry_after);
        self.next_recovery_backoff = (retry_after * 2).min(EVENT_ROUTER_MAX_RECOVERY_BACKOFF);
        retry_after
    }

    fn sleep_duration(&self, now: Instant) -> Duration {
        self.deadline.sleep_duration(now)
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
    let mut structural_events = match handle.events.subscribe() {
        Ok(receiver) => Some(receiver),
        Err(error) => {
            tracing::warn!(
                error = %error,
                safety_sweep_secs = EVENT_ROUTER_SAFETY_SWEEP_INTERVAL.as_secs(),
                "mob event router: structural event subscription unavailable; using slow durable safety sweep",
            );
            None
        }
    };

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

    // The unconditional first read closes the gap between the actor's
    // `initial_cursor` snapshot and this task installing its subscription.
    let mut poll_cadence =
        EventRouterPollCadence::new(Instant::now(), authority.config.poll_interval);

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

            structural_event = wait_for_structural_event(&mut structural_events) => {
                if structural_event_requires_poll(
                    structural_event.as_ref(),
                    handle.mob_id(),
                    mob_cursor,
                ) {
                    poll_cadence.on_causal_wake(Instant::now());
                }
            }

            // The durable cursor remains the authority. Broadcasts only pull
            // this absolute deadline earlier; a slow safety sweep covers
            // missed/cross-process signals.
            () = tokio::time::sleep(poll_cadence.sleep_duration(Instant::now())) => {
                let cursor_before_poll = mob_cursor;
                let new_events = match handle
                    .poll_events(mob_cursor, EVENT_ROUTER_POLL_BATCH_LIMIT)
                    .await
                {
                    Ok(evts) => evts,
                    Err(error) => {
                        let retry_after = poll_cadence.on_poll_failed(Instant::now());
                        tracing::warn!(
                            error = %error,
                            retry_after_ms = retry_after.as_millis(),
                            "mob event router: structural event cursor read failed; backing off",
                        );
                        continue;
                    }
                };
                let batch_was_full = new_events.len() == EVENT_ROUTER_POLL_BATCH_LIMIT;
                for mob_event in new_events {
                    if mob_event.cursor <= mob_cursor {
                        continue;
                    }
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
                poll_cadence.on_poll_succeeded(
                    Instant::now(),
                    batch_was_full && mob_cursor > cursor_before_poll,
                );
            }
        }
    }
}

/// Wait for one event-store notification.
///
/// `None` is a conservative rescan request caused by lag or closure. A closed
/// receiver is removed before returning so it cannot become an always-ready
/// select arm and hot-loop the router.
async fn wait_for_structural_event(
    receiver: &mut Option<crate::store::MobEventReceiver>,
) -> Option<(MobId, u64)> {
    match receiver {
        Some(receiver) => match receiver.recv().await {
            Ok(event) => Some((event.mob_id, event.cursor)),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                *receiver = None;
                None
            }
        },
        None => std::future::pending::<Option<(MobId, u64)>>().await,
    }
}

fn structural_event_requires_poll(
    event: Option<&(MobId, u64)>,
    routed_mob_id: &MobId,
    cursor: u64,
) -> bool {
    event.is_none_or(|(event_mob_id, event_cursor)| {
        event_mob_id == routed_mob_id && *event_cursor > cursor
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_causal_wake_pulls_safety_sweep_to_now() {
        let start = Instant::now();
        let mut cadence = EventRouterPollCadence::new(start, Duration::from_millis(100));

        cadence.on_poll_succeeded(start, false);
        assert_eq!(
            cadence.sleep_duration(start),
            EVENT_ROUTER_SAFETY_SWEEP_INTERVAL
        );

        let wake = start + Duration::from_secs(1);
        cadence.on_causal_wake(wake);
        assert_eq!(cadence.sleep_duration(wake), Duration::ZERO);
    }

    #[test]
    fn recovery_backoff_is_absolute_under_notification_storms() {
        let start = Instant::now();
        let base = Duration::from_millis(100);
        let mut cadence = EventRouterPollCadence::new(start, base);

        assert_eq!(cadence.on_poll_failed(start), base);
        for wake_ms in [1_u64, 10, 25, 50, 99] {
            cadence.on_causal_wake(start + Duration::from_millis(wake_ms));
            assert_eq!(
                cadence.sleep_duration(start),
                base,
                "notifications must not bypass the recovery deadline"
            );
        }

        let retry_at = start + base;
        assert_eq!(cadence.sleep_duration(retry_at), Duration::ZERO);
        assert_eq!(cadence.on_poll_failed(retry_at), base * 2);

        cadence.on_poll_succeeded(retry_at + base * 2, false);
        assert_eq!(
            cadence.on_poll_failed(retry_at + base * 2),
            base,
            "a successful read resets the recovery ladder"
        );
    }

    #[test]
    fn unrelated_or_already_consumed_notifications_do_not_poll() {
        let routed = MobId::from("routed");
        let unrelated = (MobId::from("other"), 12);
        let stale = (routed.clone(), 10);
        let fresh = (routed.clone(), 11);

        assert!(!structural_event_requires_poll(
            Some(&unrelated),
            &routed,
            10
        ));
        assert!(!structural_event_requires_poll(Some(&stale), &routed, 10));
        assert!(structural_event_requires_poll(Some(&fresh), &routed, 10));
        assert!(
            structural_event_requires_poll(None, &routed, 10),
            "lag or closure requires a conservative durable rescan"
        );
    }
}
