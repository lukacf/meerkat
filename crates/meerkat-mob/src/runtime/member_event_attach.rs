//! Member agent event subscriptions that attach at the member's wake.
//!
//! A member MobMachine holds seated and not materialized in this process
//! (dormant, or waking while its wake runs) has no live session actor, so a
//! subscription to its agent events has nothing to attach to yet. Lazy
//! materialization is the default, so a `MobHandle` member subscription
//! (`subscribe_agent_events`, `subscribe_agent_events_from`,
//! `subscribe_agent_events_observation`, `subscribe_all_agent_events`) and
//! the mob event router open for such a member and attach at its wake, from
//! the woken actor's first retained event: a host that subscribes and then
//! sends receives the run its send starts from `RunStarted`. A subscription
//! never wakes its member, and sends the mob's actor nothing while it waits.
//!
//! The waits keep no lifecycle state of their own. They read MobMachine's
//! published facts: each member's seat (`identity_to_runtime`), its
//! materialization phase (`member_materialization`) and the epoch of its
//! runtime registration (`member_materialization_epoch`). The actor diffs
//! those maps between the state it published last and the state it
//! publishes, once per published change and only while some subscription
//! waits, and broadcasts the members whose entries changed once the state
//! is published ([`MemberMaterializationFeed`]). A waiter wakes only for a published change
//! that moved some member, and reads only the members that change names, so
//! the cost a waiter pays per machine change is bounded by the members that
//! changed, not by the roster.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, OnceLock};

use futures::StreamExt as _;
use meerkat_core::comms::{EventStream, SessionEventCursor};

use super::MobHandle;
use super::session_service::AgentEventSubscription;
use crate::error::MobError;
use crate::ids::AgentIdentity;
use crate::machines::mob_machine as mob_dsl;
#[cfg(target_arch = "wasm32")]
use crate::tokio;

/// The members whose seat, materialization phase or registration epoch
/// changed in one published MobMachine state.
pub(crate) type MaterializationChange = Arc<[AgentIdentity]>;

/// How many published changes the feed holds for a waiter that has not read
/// them yet. A waiter further behind rereads the published state of every
/// member it waits on instead ([`MaterializationTick::Resync`]).
const MATERIALIZATION_FEED_CAPACITY: usize = 1024;

/// The mob actor's feed of member materialization changes, shared with its
/// handles through `MemberLifecycleObservations`. The actor is its only
/// sender.
#[derive(Debug)]
pub(crate) struct MemberMaterializationFeed {
    changes: tokio::sync::broadcast::Sender<MaterializationChange>,
}

impl Default for MemberMaterializationFeed {
    fn default() -> Self {
        Self {
            changes: tokio::sync::broadcast::channel(MATERIALIZATION_FEED_CAPACITY).0,
        }
    }
}

impl MemberMaterializationFeed {
    /// The members whose seat, materialization phase or registration epoch
    /// differs between `published`, the state the actor published last, and
    /// `next`, the state it publishes now. `None` when none differs, and
    /// while no subscription waits (nothing is diffed then).
    pub(crate) fn changes_between(
        &self,
        published: &mob_dsl::MobMachineState,
        next: &mob_dsl::MobMachineState,
    ) -> Option<MaterializationChange> {
        if self.changes.receiver_count() == 0 {
            return None;
        }
        let mut changed = BTreeSet::new();
        changed_keys(
            &published.identity_to_runtime,
            &next.identity_to_runtime,
            &mut changed,
        );
        changed_keys(
            &published.member_materialization,
            &next.member_materialization,
            &mut changed,
        );
        changed_keys(
            &published.member_materialization_epoch,
            &next.member_materialization_epoch,
            &mut changed,
        );
        if changed.is_empty() {
            return None;
        }
        Some(
            changed
                .into_iter()
                .map(|identity| AgentIdentity::from(identity.0.as_str()))
                .collect::<Vec<_>>()
                .into(),
        )
    }

    /// Broadcast `change` to the waiting subscriptions, once the state it
    /// was diffed from is published.
    pub(crate) fn publish(&self, change: MaterializationChange) {
        // A waiter that left since the diff is not a fault.
        let _ = self.changes.send(change);
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<MaterializationChange> {
        self.changes.subscribe()
    }
}

/// Add to `changed` every key whose entry differs between `old` and `new`
/// (present in one only, or with another value), in one ordered walk of both.
fn changed_keys<K: Ord + Clone, V: PartialEq>(
    old: &BTreeMap<K, V>,
    new: &BTreeMap<K, V>,
    changed: &mut BTreeSet<K>,
) {
    let mut old = old.iter().peekable();
    let mut new = new.iter().peekable();
    loop {
        match (old.peek(), new.peek()) {
            (None, None) => return,
            (Some((key, _)), None) => {
                changed.insert((*key).clone());
                old.next();
            }
            (None, Some((key, _))) => {
                changed.insert((*key).clone());
                new.next();
            }
            (Some((old_key, old_value)), Some((new_key, new_value))) => {
                match old_key.cmp(new_key) {
                    std::cmp::Ordering::Less => {
                        changed.insert((*old_key).clone());
                        old.next();
                    }
                    std::cmp::Ordering::Greater => {
                        changed.insert((*new_key).clone());
                        new.next();
                    }
                    std::cmp::Ordering::Equal => {
                        if old_value != new_value {
                            changed.insert((*old_key).clone());
                        }
                        old.next();
                        new.next();
                    }
                }
            }
        }
    }
}

/// A waiter's view of its mob's [`MemberMaterializationFeed`], ended by the
/// end of the mob's actor.
pub(super) struct MaterializationWatch {
    changes: tokio::sync::broadcast::Receiver<MaterializationChange>,
    actor_task: tokio::sync::watch::Receiver<super::ActorTaskEnd>,
}

/// What a waiter learns from its [`MaterializationWatch`].
pub(super) enum MaterializationTick {
    /// These members' seat, materialization phase or registration epoch
    /// changed.
    Changed(MaterializationChange),
    /// The waiter fell behind the feed: any member may have changed, so it
    /// rereads every member it waits on.
    Resync,
}

impl MaterializationWatch {
    /// Watch `handle`'s mob. A waiter opens its watch first and reads the
    /// published state of the members it waits on after: a change published
    /// before the watch is in that read, and every later one is on the feed.
    pub(super) fn open(handle: &MobHandle) -> Self {
        Self {
            changes: handle
                .lifecycle_observations
                .materialization_feed()
                .subscribe(),
            actor_task: handle.lifecycle_observations.subscribe_actor_task(),
        }
    }

    /// The next change, or `None` once the mob's actor has ended: nothing
    /// materializes after that.
    pub(super) async fn next(&mut self) -> Option<MaterializationTick> {
        loop {
            if *self.actor_task.borrow() != super::ActorTaskEnd::Running {
                return None;
            }
            tokio::select! {
                received = self.changes.recv() => {
                    return match received {
                        Ok(changed) => Some(MaterializationTick::Changed(changed)),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            Some(MaterializationTick::Resync)
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
                    };
                }
                ended = self.actor_task.changed() => {
                    if ended.is_err() {
                        return None;
                    }
                }
            }
        }
    }
}

/// Where a member stands for a subscription that waits on it, as MobMachine
/// last published it.
pub(super) enum MemberResidency {
    /// No longer seated (retired, or never seated): nothing will attach.
    NotSeated,
    /// Seated and not materialized in this process: dormant, or waking.
    Unmaterialized,
    /// Live: materialized in this process.
    Live,
    /// Seated in no residency phase (broken, retiring, or a member that is
    /// not session-bound): an attempt to subscribe decides.
    Unserved,
}

pub(super) fn member_residency(handle: &MobHandle, identity: &AgentIdentity) -> MemberResidency {
    let state = handle.machine_state_watch_rx.borrow();
    let key = mob_dsl::AgentIdentity::from_domain(identity);
    if !state.identity_to_runtime.contains_key(&key) {
        return MemberResidency::NotSeated;
    }
    match state.member_materialization.get(&key) {
        Some(
            mob_dsl::MemberMaterializationPhase::Dormant
            | mob_dsl::MemberMaterializationPhase::Waking,
        ) => MemberResidency::Unmaterialized,
        Some(mob_dsl::MemberMaterializationPhase::Live) => MemberResidency::Live,
        None => MemberResidency::Unserved,
    }
}

/// Which subscription a member subscription makes once its member is
/// materialized.
#[derive(Clone, Copy)]
pub(super) enum AttachPath {
    /// Through MobMachine's subscription authority, as
    /// `MobHandle::subscribe_agent_events_from` subscribes.
    Authorized,
    /// At the session MobMachine binds the member to, through the session
    /// service and with no command to the mob's actor, as
    /// `MobHandle::subscribe_agent_events_observation` subscribes.
    Observation,
}

impl AttachPath {
    async fn attempt(
        self,
        handle: &MobHandle,
        identity: &AgentIdentity,
        cursor: SessionEventCursor,
    ) -> Result<AgentEventSubscription, MobError> {
        match self {
            Self::Authorized => handle.subscribe_agent_events_once(identity, cursor).await,
            Self::Observation => {
                handle
                    .subscribe_agent_events_observation_once(identity, cursor)
                    .await
            }
        }
    }

    /// Attach to `identity`'s woken actor from its first retained event:
    /// everything the stream then delivers happened after the wake, so the
    /// run the wake started is read from `RunStarted`. A session service
    /// that cannot replay a cursor attaches live instead.
    pub(super) async fn attach_from_the_wake(
        self,
        handle: &MobHandle,
        identity: &AgentIdentity,
    ) -> Result<AgentEventSubscription, MobError> {
        match self
            .attempt(handle, identity, SessionEventCursor::Earliest)
            .await
        {
            Err(MobError::AgentEventCursorRejected { .. }) => {
                self.attempt(handle, identity, SessionEventCursor::Live)
                    .await
            }
            result => result,
        }
    }
}

/// Subscribe to `identity`'s agent events through `path` from `cursor`. The
/// subscription is asked for once, so MobMachine's subscription authority
/// decides it. A member it refuses not live while it holds the member
/// unmaterialized gets a subscription that attaches at the member's wake
/// ([`spawn_wake_attach`]), and nothing more is asked for it meanwhile; a
/// member it holds Live whose session actor is gone keeps the typed
/// `MobError::MemberSessionNotLive`.
pub(super) async fn subscribe_or_attach_at_wake(
    handle: &MobHandle,
    identity: &AgentIdentity,
    cursor: SessionEventCursor,
    path: AttachPath,
) -> Result<AgentEventSubscription, MobError> {
    let unmaterialized_before = handle.member_unmaterialized(identity);
    match path.attempt(handle, identity, cursor).await {
        Err(MobError::MemberSessionNotLive { .. })
            if unmaterialized_before || handle.member_unmaterialized(identity) =>
        {
            spawn_wake_attach(handle, vec![identity.clone()], path)
                .pop()
                .map(|(_, subscription)| subscription)
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "the wake attach of '{identity}' answered no subscription"
                    ))
                })
        }
        result => result,
    }
}

/// One member a wake attach waits on.
struct WakeAttach {
    attached: tokio::sync::oneshot::Sender<AgentEventSubscription>,
}

/// Subscriptions for `members`, which MobMachine holds unmaterialized, that
/// attach at each member's wake: one waiting task serves them all. Each
/// subscription's stream yields nothing until its member's woken actor is
/// attached ([`AttachPath::attach_from_the_wake`]); its `attached_epoch` is
/// set at the attach, to the sequence space that actor's stream names. A
/// stream ends without an event when its member leaves the mob or is
/// refused other than not live, and when the mob's actor ends first. A
/// not-live refusal of a member that reads materialized waits for the
/// member's next change. The task ends once every member attached or left,
/// or every stream was dropped.
pub(super) fn spawn_wake_attach(
    handle: &MobHandle,
    members: Vec<AgentIdentity>,
    path: AttachPath,
) -> Vec<(AgentIdentity, AgentEventSubscription)> {
    // The watch opens before the task reads any member: see
    // `MaterializationWatch::open`.
    let watch = MaterializationWatch::open(handle);
    let (keepalive, dropped) = tokio::sync::mpsc::channel::<()>(1);
    let mut waiting = HashMap::with_capacity(members.len());
    let mut subscriptions = Vec::with_capacity(members.len());
    for identity in members {
        let (attached, attach) = tokio::sync::oneshot::channel();
        waiting.insert(identity.clone(), WakeAttach { attached });
        let attached_epoch = Arc::new(OnceLock::new());
        let named = Arc::clone(&attached_epoch);
        let keepalive = keepalive.clone();
        let stream = futures::stream::once(async move {
            // Held until the member attaches: once every waiting stream is
            // dropped, the task stops waiting.
            let _keepalive = keepalive;
            match attach.await {
                Ok(subscription) => {
                    let _ = named.set(subscription.epoch);
                    subscription.stream
                }
                Err(_) => Box::pin(futures::stream::empty::<
                    meerkat_core::event::EventEnvelope<meerkat_core::event::AgentEvent>,
                >()) as EventStream,
            }
        })
        .flatten();
        subscriptions.push((
            identity,
            AgentEventSubscription::attaching_at_wake(Box::pin(stream), attached_epoch),
        ));
    }
    drop(keepalive);
    tokio::spawn(attach_at_wake(
        handle.observer(),
        watch,
        path,
        waiting,
        dropped,
    ));
    subscriptions
}

async fn attach_at_wake(
    handle: MobHandle,
    mut watch: MaterializationWatch,
    path: AttachPath,
    mut waiting: HashMap<AgentIdentity, WakeAttach>,
    mut dropped: tokio::sync::mpsc::Receiver<()>,
) {
    // A member whose wake ended before the watch opened is attached now.
    let every = waiting.keys().cloned().collect::<Vec<_>>();
    attach_members(&handle, path, &mut waiting, every).await;
    while !waiting.is_empty() {
        tokio::select! {
            tick = watch.next() => {
                let named = match tick {
                    // The mob's actor ended: every waiting stream ends.
                    None => return,
                    Some(MaterializationTick::Changed(changed)) => changed
                        .iter()
                        .filter(|identity| waiting.contains_key(*identity))
                        .cloned()
                        .collect::<Vec<_>>(),
                    Some(MaterializationTick::Resync) => waiting.keys().cloned().collect(),
                };
                attach_members(&handle, path, &mut waiting, named).await;
            }
            // Every waiting stream was dropped.
            None = dropped.recv() => return,
        }
    }
}

/// Attach each of `members` that MobMachine holds materialized now, and
/// drop the waits that can no longer attach.
async fn attach_members(
    handle: &MobHandle,
    path: AttachPath,
    waiting: &mut HashMap<AgentIdentity, WakeAttach>,
    members: Vec<AgentIdentity>,
) {
    for identity in members {
        if waiting
            .get(&identity)
            .is_none_or(|wait| wait.attached.is_closed())
        {
            waiting.remove(&identity);
            continue;
        }
        match member_residency(handle, &identity) {
            MemberResidency::NotSeated => {
                waiting.remove(&identity);
            }
            MemberResidency::Unmaterialized => {}
            MemberResidency::Live | MemberResidency::Unserved => {
                match path.attach_from_the_wake(handle, &identity).await {
                    Ok(subscription) => {
                        if let Some(wait) = waiting.remove(&identity) {
                            let _ = wait.attached.send(subscription);
                        }
                    }
                    // Not served yet (its actor is not up, or it was
                    // released again): its next change is waited for.
                    Err(MobError::MemberSessionNotLive { .. }) => {}
                    Err(error) => {
                        tracing::debug!(
                            agent_identity = %identity,
                            %error,
                            "a member event subscription waiting for its wake ends refused"
                        );
                        waiting.remove(&identity);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_keys_names_every_added_removed_and_changed_entry_once() {
        let old = BTreeMap::from([(1, 'a'), (2, 'b'), (4, 'd'), (5, 'e')]);
        let new = BTreeMap::from([(2, 'b'), (3, 'c'), (4, 'x'), (5, 'e'), (6, 'f')]);
        let mut changed = BTreeSet::new();
        changed_keys(&old, &new, &mut changed);
        assert_eq!(changed, BTreeSet::from([1, 3, 4, 6]));
        let mut unchanged = BTreeSet::new();
        changed_keys(&old, &old, &mut unchanged);
        assert!(unchanged.is_empty());
    }
}
