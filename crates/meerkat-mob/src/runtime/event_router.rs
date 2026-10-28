//! Mob-level event bus that merges per-member session streams into a
//! single `mpsc` channel of [`AttributedEvent`]s.
//!
//! The router runs as an independent tokio task:
//! 1. Bootstraps by subscribing to all current roster members.
//! 2. Follows the structural event-store subscription for causal
//!    `MemberSpawned`/`MemberRetired` wakeups, with a slow durable-cursor
//!    safety sweep for missed/cross-process signals.
//! 3. Follows its local members across their materializations: a member that
//!    is not materialized when the router subscribes it (dormant, or waking),
//!    and one whose stream ended with its actor (a release ends it), is
//!    attached at its next wake, from the woken actor's first retained event.
//!    One wait on the mob's materialization feed serves every member of the
//!    router, and a published machine change makes it read only the members
//!    the change names, so the router's cost per machine change is bounded by
//!    the members that changed, not by the roster.
//! 4. Tags events with [`AttributedEvent`] and forwards to the receiver.
//!
//! Streams for retired members end naturally when sessions are archived.

use crate::event::AttributedEvent;
use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, MobId, ProfileName};
#[cfg(target_arch = "wasm32")]
use crate::tokio;

use super::MobHandle;
use super::member_event_attach::{
    AttachPath, MaterializationTick, MaterializationWatch, MemberResidency, member_residency,
};
use crate::error::MobError;
use futures::stream::{SelectAll, StreamExt};
use meerkat_core::time_compat::Instant;
use std::collections::{BTreeSet, HashMap};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const EVENT_ROUTER_POLL_BATCH_LIMIT: usize = 100;
const EVENT_ROUTER_SAFETY_SWEEP_INTERVAL: Duration = Duration::from_secs(30);
const EVENT_ROUTER_MAX_RECOVERY_BACKOFF: Duration = Duration::from_secs(30);

/// Configuration for the mob event router.
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

/// Handle returned by `spawn_event_router`. Drop to stop the router.
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

/// Subscribe the router to every member it starts with, then spawn its task
/// and return its handle.
///
/// The initial member subscriptions (local session streams and placed
/// members' pump taps) are established before this returns, never inside the
/// spawned task: a caller that subscribes and then drives a member turn must
/// see that turn's events. Established in the task, they raced the caller's
/// first turn, and a fast turn finished before its member's stream was
/// subscribed, so the merged stream never carried it. Members that join
/// later are covered by the durable cursor from `initial_cursor` on.
pub(super) async fn spawn_event_router(
    handle: MobHandle,
    authority: AuthorizedMobEventRouter,
) -> MobEventRouterHandle {
    let (event_tx, event_rx) = mpsc::channel(authority.config.channel_capacity);
    let cancel = CancellationToken::new();
    // Opened before the members are subscribed: a member that wakes between
    // its refused subscription and the router's first wait is still seen.
    let materialization = MaterializationWatch::open(&handle);
    let (ended_tx, ended_rx) = mpsc::unbounded_channel();
    let initial =
        subscribe_initial_members(&handle, &authority, MemberStreamEnds::new(ended_tx)).await;
    tokio::spawn(run_event_router(
        handle,
        authority,
        initial,
        RouterMaterialization {
            watch: materialization,
            ended: ended_rx,
        },
        event_tx,
        cancel.clone(),
    ));
    MobEventRouterHandle { event_rx, cancel }
}

/// The subscriptions a router starts with: the structural event receiver
/// and every authorized member's stream.
struct InitialRouterSubscriptions {
    structural_events: Option<crate::store::MobEventReceiver>,
    merged: SelectAll<TaggedStream>,
    tracked_ids: HashMap<AgentIdentity, AgentRuntimeId>,
    followed: HashMap<AgentIdentity, FollowedMember>,
    ends: MemberStreamEnds,
}

/// What the router follows its local members' materializations by: the
/// mob's materialization feed, and the ends its members' streams report.
struct RouterMaterialization {
    watch: MaterializationWatch,
    ended: mpsc::UnboundedReceiver<(AgentIdentity, u64)>,
}

/// A local member the router carries across its materializations.
struct FollowedMember {
    member: AuthorizedMobEventRouterMember,
    /// The token of the member's stream the router carries, from the attach
    /// until that stream reports its end. `None` while it carries none: the
    /// member is not materialized, or its stream ended with its actor.
    carried: Option<u64>,
}

/// Where the router's local member streams report their end: a stream ends
/// when its actor does, and the router then attaches the member again at
/// its next wake.
struct MemberStreamEnds {
    next_token: u64,
    ended: mpsc::UnboundedSender<(AgentIdentity, u64)>,
}

impl MemberStreamEnds {
    fn new(ended: mpsc::UnboundedSender<(AgentIdentity, u64)>) -> Self {
        Self {
            next_token: 0,
            ended,
        }
    }

    /// `stream`, each event tagged with `member`'s attribution, reporting
    /// its end under a fresh token, which is returned with it. A token tells
    /// the end of the stream the router carries from that of a stream it
    /// replaced since.
    fn carry(
        &mut self,
        stream: meerkat_core::comms::EventStream,
        member: &AuthorizedMobEventRouterMember,
    ) -> (TaggedStream, u64) {
        let token = self.next_token;
        self.next_token = self.next_token.wrapping_add(1);
        let ended = self.ended.clone();
        let identity = member.agent_identity.clone();
        let end = futures::stream::once(async move {
            // The router holds the receiver for as long as it runs.
            let _ = ended.send((identity, token));
            None::<TaggedItem>
        })
        .filter_map(futures::future::ready);
        (
            Box::pin(tag_member_stream(stream, member).chain(end)),
            token,
        )
    }
}

async fn subscribe_initial_members(
    handle: &MobHandle,
    authority: &AuthorizedMobEventRouter,
    mut ends: MemberStreamEnds,
) -> InitialRouterSubscriptions {
    let mut merged: SelectAll<TaggedStream> = SelectAll::new();
    // Track the SUBSCRIBED incarnation per identity: a respawn (ADJ-24)
    // replaces the member's stream, so re-subscription keys on the runtime
    // id, never on bare identity presence.
    let mut tracked_ids: HashMap<AgentIdentity, AgentRuntimeId> = HashMap::new();
    let mut followed = HashMap::new();
    let structural_events = match handle.events.subscribe() {
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

    for member in handle
        .authorized_mob_event_router_members(&authority.session_bound_runtimes)
        .await
    {
        if tracked_ids.contains_key(&member.agent_identity) {
            continue;
        }
        if let Some(member) = subscribe_member(handle, member, &mut merged, &mut ends).await {
            tracked_ids.insert(
                member.member.agent_identity.clone(),
                member.member.runtime_id.clone(),
            );
            followed.insert(member.member.agent_identity.clone(), member);
        }
    }
    // Placed members fan in through pump taps - shape-identical items in
    // the SAME merge (phase 6).
    for dsl_identity in &authority.external_members {
        let member_identity = AgentIdentity::from(dsl_identity.0.as_str());
        if tracked_ids.contains_key(&member_identity) {
            continue;
        }
        let Some(runtime_id) = handle.member_runtime_id_observation(&member_identity) else {
            continue;
        };
        if let Some(stream) = subscribe_external_member(handle, &member_identity).await {
            tracked_ids.insert(member_identity, runtime_id);
            merged.push(stream);
        }
    }
    InitialRouterSubscriptions {
        structural_events,
        merged,
        tracked_ids,
        followed,
        ends,
    }
}

#[allow(clippy::ignored_unit_patterns)]
async fn run_event_router(
    handle: MobHandle,
    authority: AuthorizedMobEventRouter,
    initial: InitialRouterSubscriptions,
    materialization: RouterMaterialization,
    event_tx: mpsc::Sender<AttributedEvent>,
    cancel: CancellationToken,
) {
    let InitialRouterSubscriptions {
        mut structural_events,
        mut merged,
        mut tracked_ids,
        mut followed,
        mut ends,
    } = initial;
    let RouterMaterialization {
        watch,
        ended: mut stream_ends,
    } = materialization;
    let mut materialization = Some(watch);
    let mut mob_cursor: u64 = authority.initial_cursor;

    // A member that woke before the router's first wait is attached now.
    for member in followed.values_mut() {
        follow_materialization(&handle, member, &mut merged, &mut ends).await;
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

            // A member the router carries no stream of is attached at the
            // change that makes it Live.
            tick = wait_for_materialization(&mut materialization) => {
                let named = match tick {
                    Some(MaterializationTick::Changed(changed)) => changed
                        .iter()
                        .filter(|identity| {
                            followed
                                .get(*identity)
                                .is_some_and(|member| member.carried.is_none())
                        })
                        .cloned()
                        .collect::<Vec<_>>(),
                    Some(MaterializationTick::Resync) => followed
                        .iter()
                        .filter(|(_, member)| member.carried.is_none())
                        .map(|(identity, _)| identity.clone())
                        .collect(),
                    None => Vec::new(),
                };
                for identity in named {
                    if let Some(member) = followed.get_mut(&identity) {
                        follow_materialization(&handle, member, &mut merged, &mut ends).await;
                    }
                }
            }

            // A carried stream ended with its actor: the member is attached
            // again now if it is Live again, else at its next wake.
            Some((identity, token)) = stream_ends.recv() => {
                if let Some(member) = followed.get_mut(&identity)
                    && member.carried == Some(token)
                {
                    member.carried = None;
                    follow_materialization(&handle, member, &mut merged, &mut ends).await;
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
                                    // A placed member rides its pump tap,
                                    // not a followed local stream.
                                    followed.remove(&member_identity);
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
                                        if let Some(member) = subscribe_member(
                                            &handle,
                                            member,
                                            &mut merged,
                                            &mut ends,
                                        )
                                        .await
                                        {
                                            tracked_ids.insert(
                                                member.member.agent_identity.clone(),
                                                member.member.runtime_id.clone(),
                                            );
                                            followed.insert(
                                                member.member.agent_identity.clone(),
                                                member,
                                            );
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
                                    followed.remove(&member_identity);
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

/// Wait for the next change of the mob's member materialization. `None` once
/// the mob's actor has ended: the watch is removed before returning, so it
/// cannot become an always-ready select arm and hot-loop the router.
async fn wait_for_materialization(
    watch: &mut Option<MaterializationWatch>,
) -> Option<MaterializationTick> {
    let Some(active) = watch.as_mut() else {
        return std::future::pending::<Option<MaterializationTick>>().await;
    };
    let tick = active.next().await;
    if tick.is_none() {
        *watch = None;
    }
    tick
}

/// Attach `followed`, whose stream the router does not carry, if MobMachine
/// holds it Live now: from the woken actor's first retained event, so the run
/// its wake started is carried from `RunStarted`. A member that is not Live,
/// or whose actor is not up (or is gone again), waits for its next change.
async fn follow_materialization(
    handle: &MobHandle,
    followed: &mut FollowedMember,
    merged: &mut SelectAll<TaggedStream>,
    ends: &mut MemberStreamEnds,
) {
    if followed.carried.is_some()
        || !matches!(
            member_residency(handle, &followed.member.agent_identity),
            MemberResidency::Live
        )
    {
        return;
    }
    match AttachPath::Authorized
        .attach_from_the_wake(handle, &followed.member.agent_identity)
        .await
    {
        Ok(subscription) => {
            let (stream, token) = ends.carry(subscription.stream, &followed.member);
            merged.push(stream);
            followed.carried = Some(token);
        }
        Err(MobError::MemberSessionNotLive { .. }) => {}
        Err(error) => {
            tracing::warn!(
                agent_identity = %followed.member.agent_identity,
                error = %error,
                "mob event router: failed to attach a woken member's agent events",
            );
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
    let Some(active_receiver) = receiver.as_mut() else {
        return std::future::pending::<Option<(MobId, u64)>>().await;
    };
    let received = active_receiver.recv().await;
    match received {
        Ok(event) => Some((event.mob_id, event.cursor)),
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => None,
        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
            *receiver = None;
            None
        }
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

/// Subscribe the router to `member` and follow it: a member served now is
/// carried at once; one refused not live (MobMachine holds it unmaterialized,
/// dormant or waking, or its actor is gone) is attached at its next wake.
/// `None` for a member refused otherwise, which the router does not follow.
async fn subscribe_member(
    handle: &MobHandle,
    member: AuthorizedMobEventRouterMember,
    merged: &mut SelectAll<TaggedStream>,
    ends: &mut MemberStreamEnds,
) -> Option<FollowedMember> {
    let carried = match handle
        .subscribe_authorized_agent_session_events(&member.agent_identity, &member.session_id)
        .await
    {
        Ok(stream) => {
            let (stream, token) = ends.carry(stream, &member);
            merged.push(stream);
            Some(token)
        }
        Err(MobError::MemberSessionNotLive { .. }) => None,
        Err(error) => {
            tracing::warn!(
                agent_identity = %member.agent_identity,
                error = %error,
                "mob event router: failed to subscribe to member agent events",
            );
            return None;
        }
    };
    Some(FollowedMember { member, carried })
}

/// `stream`, each event tagged with `member`'s attribution.
fn tag_member_stream(
    stream: meerkat_core::comms::EventStream,
    member: &AuthorizedMobEventRouterMember,
) -> TaggedStream {
    let prof = member.role.clone();
    let source_runtime_id = member.runtime_id.clone();
    let source_fence_token = member.fence_token;
    Box::pin(stream.map(move |envelope| {
        (
            source_runtime_id.clone(),
            source_fence_token,
            prof.clone(),
            envelope,
        )
    }))
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
                let attributed = std::sync::Arc::unwrap_or_clone(attributed);
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
