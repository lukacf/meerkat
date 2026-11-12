//! Wake forwarding between two mobs of one process.
//!
//! A member's peer sends are routed inside its own mob's namespace. A member
//! of another mob that is not built has no route there, and its own mob's
//! wake routes live in the other namespace, which a namespace-scoped send
//! never looks at. This module is the host composition between the two: for
//! every link a mob holds to a member of another mob of this process, the
//! mob's namespace gets a wake route under the remote member's key whose
//! owner is this module (a forwarder). Asked to wake, it forwards the
//! request to the remote member's own mob, and once that mob answered ready
//! it installs the route to the member's inbox of now.
//!
//! **The link table.** Process-wide, in memory, keyed by namespace: each mob
//! of this process has its own part. A link is one committed edge of a local
//! member's row to an in-process member of another mob. A part is a
//! projection of its mob's rows, and is written from them only:
//! - after a wire committed, and before an unwire commits
//!   ([`sync_member_links`], [`drop_member_link`]);
//! - once when the runtime starts ([`restore_from_roster`]): one member
//!   listing, then one row read for each member that names a member of
//!   another mob. Nobody is built and no store is read.
//! - a runtime's whole part goes when the runtime begins to shut down or
//!   stops its mob ([`close_part`]); the routes between its members and the
//!   other mobs' members go when its shutdown ran to its end
//!   ([`clear_part`]).
//!
//! A part belongs to the runtime whose start-time read created it. Nothing
//! else creates one: a wire on a runtime that never ran the read (a launch
//! that does not speak for the composition) holds no link. Only its own
//! runtime closes a part, so the late teardown of a runtime that was
//! dropped cannot clear the part of the runtime built after it.
//!
//! **What is forwarded.** Only a wake for a link BOTH mobs hold: the
//! sender's mob names the recipient, and the recipient's mob names the
//! sender, each by the other's key. The local member is resolved from the
//! sender's key, the remote member from the recipient's key, and the
//! remote namespace from the link, never from the request's text. Anything
//! else is answered as a key that is no member
//! ([`PeerWakeDenial::NotAMember`]): nothing is asked of the other mob and
//! nothing is installed. The forwarded request is the same request, under
//! the name the remote member's own mob published for the member, which
//! must be the linked member's; the remote mob's machine decides, and its
//! answer is passed on unchanged. The owner is read for each wake and never
//! kept. This module never reads or changes the other mob's state, and
//! never publishes or retracts a route in the other mob's namespace or
//! over a route another owner holds.
//!
//! **Pending wakes.** Content sends from senders that are not themselves
//! being built share one pending wake per remote member and observation
//! (at most two per link): every waiting send gets that wake's answer, one
//! waiter leaving does not end it, and it is forgotten when it is answered
//! or when its last waiter left. A silent lifecycle notice and a send from
//! a member that is being built are asked for themselves, of the remote
//! machine.
//!
//! **Locks.** `state` is taken by the methods the registry calls (`wake`,
//! `member_is_waking`), so no registry call is made while it is held and it
//! is never held across an await. `writing` orders the writers of one part
//! and is taken by nothing the registry calls.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError, Weak};

use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use meerkat_comms::{
    InprocPeerWaker, InprocRegistry, PeerWakeAnswer, PeerWakeDenial, PeerWakeFuture,
    PeerWakeInputClass, PeerWakeRequest, PubKey,
};
use meerkat_core::comms::{PeerWakeRefusal, TrustedPeerDescriptor};
use meerkat_mob::ids::AgentIdentity;
use meerkat_mob::{MobError, MobHandle};

use super::UnifiedRuntime;
use super::cross_mob::{
    CrossMobError, inproc_peer_namespace, install_route_alias, mob_inproc_namespace_for_id,
};

type WakeResult = Result<PeerWakeAnswer, PeerWakeDenial>;
type SharedWake = Shared<BoxFuture<'static, WakeResult>>;

/// The pending wake a send joins: the remote member's key, and whether the
/// send observed the member missing.
type WakeSlot = (PubKey, bool);

/// One committed edge of a local member's row to an in-process member of
/// another mob.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Link {
    /// The local member, as its mob's roster names it.
    member: String,
    /// The local member's comms name.
    local: String,
    /// The remote member's comms name.
    remote: String,
    /// The remote member's key, as the local member's row holds it.
    remote_key: PubKey,
}

/// One pending forwarded wake and the sends waiting on it.
struct PendingWake {
    wake: SharedWake,
    waiters: usize,
}

#[derive(Default)]
struct RoutesState {
    links: HashSet<Link>,
    pending: HashMap<WakeSlot, PendingWake>,
}

/// One mob's part of the link table, and the owner of the wake routes its
/// namespace holds for members of other mobs.
pub(crate) struct CrossMobWakeRoutes {
    namespace: String,
    state: Mutex<RoutesState>,
    writing: Mutex<()>,
    this: Weak<CrossMobWakeRoutes>,
}

/// The link table: every mob of this process that holds a part.
static NAMESPACES: LazyLock<Mutex<HashMap<String, Arc<CrossMobWakeRoutes>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn routes_of(namespace: &str) -> Option<Arc<CrossMobWakeRoutes>> {
    locked(&NAMESPACES).get(namespace).cloned()
}

/// A new, empty part for `namespace`, in place of whatever part an earlier
/// runtime of the mob left there (that part's links and wake routes go, and
/// the routes between the mob's members and other mobs' members with them).
fn start_part(namespace: &str) -> Arc<CrossMobWakeRoutes> {
    let part = Arc::new_cyclic(|this| CrossMobWakeRoutes {
        namespace: namespace.to_string(),
        state: Mutex::new(RoutesState::default()),
        writing: Mutex::new(()),
        this: Weak::clone(this),
    });
    let earlier = locked(&NAMESPACES).insert(namespace.to_string(), Arc::clone(&part));
    if let Some(earlier) = earlier {
        earlier.clear();
    }
    sweep_inbox_routes(namespace);
    part
}

/// Whether `owner` is a part of the link table: a wake is forwarded to a
/// mob's own owner only, never to this or another forwarder.
fn is_forwarder(owner: &Weak<dyn InprocPeerWaker>) -> bool {
    locked(&NAMESPACES)
        .values()
        .any(|routes| Weak::ptr_eq(owner, &routes.waker()))
}

/// The mob a member comms name belongs to: its namespace, and the member's
/// name in that mob.
fn member_of(comms_name: &str) -> Option<(String, String)> {
    let name = comms_name.parse::<meerkat_core::MemberCommsName>().ok()?;
    let namespace = mob_inproc_namespace_for_id(name.mob_id()).ok()?;
    Some((namespace, name.member().to_string()))
}

fn member_namespace(comms_name: &str) -> Option<String> {
    member_of(comms_name).map(|(namespace, _)| namespace)
}

fn denied(denial: PeerWakeDenial) -> PeerWakeFuture {
    Box::pin(std::future::ready(Err(denial)))
}

/// A send waiting on a pending wake. When it leaves (answered, timed out or
/// dropped) the pending wake loses a waiter; the last one takes the entry
/// with it, so a wake the remote mob never answers is not kept.
struct Waiter {
    routes: Weak<CrossMobWakeRoutes>,
    slot: WakeSlot,
    wake: SharedWake,
}

impl Drop for Waiter {
    fn drop(&mut self) {
        let Some(routes) = self.routes.upgrade() else {
            return;
        };
        let mut state = locked(&routes.state);
        let Some(pending) = state.pending.get_mut(&self.slot) else {
            return;
        };
        if !pending.wake.ptr_eq(&self.wake) {
            return;
        }
        pending.waiters = pending.waiters.saturating_sub(1);
        if pending.waiters == 0 {
            state.pending.remove(&self.slot);
        }
    }
}

impl CrossMobWakeRoutes {
    fn waker(&self) -> Weak<dyn InprocPeerWaker> {
        let waker: Weak<dyn InprocPeerWaker> = self.this.clone();
        waker
    }

    /// The link this mob holds from its member `local` to a member of
    /// another mob under `remote_key`.
    fn link(&self, local: &str, remote_key: &PubKey) -> Option<Link> {
        locked(&self.state)
            .links
            .iter()
            .find(|link| link.local == local && link.remote_key == *remote_key)
            .cloned()
    }

    /// Whether this mob holds the link from its member `local` to the member
    /// `remote` of another mob under `remote_key`.
    fn holds(&self, local: &str, remote: &str, remote_key: &PubKey) -> bool {
        self.link(local, remote_key)
            .is_some_and(|link| link.remote == remote)
    }

    /// Whether this namespace's wake route under `key` is free for this
    /// part: absent, or this part's own. A key a member of this mob is
    /// seated under keeps its own mob's route.
    fn may_route(&self, key: &PubKey) -> bool {
        match InprocRegistry::global().wake_route_owner_in_namespace(&self.namespace, key) {
            None => true,
            Some((_, owner)) => Weak::ptr_eq(&owner, &self.waker()),
        }
    }

    /// Replace the links of `member` with `links` (what its row names now):
    /// a key that gained its first link gets this part's wake route, and a
    /// key that lost its last link loses it, with its pending wakes.
    fn set_member_links(&self, member: &str, links: Vec<Link>) {
        let registry = InprocRegistry::global();
        let _writing = locked(&self.writing);
        // Read the registry before `state` is taken.
        let routable: Vec<Link> = links
            .into_iter()
            .filter(|link| self.may_route(&link.remote_key))
            .collect();
        let (retract, publish) = {
            let mut state = locked(&self.state);
            let before: HashSet<PubKey> = state.links.iter().map(|link| link.remote_key).collect();
            state.links.retain(|link| link.member != member);
            state.links.extend(routable);
            let after: HashMap<PubKey, String> = state
                .links
                .iter()
                .map(|link| (link.remote_key, link.remote.clone()))
                .collect();
            let retract: Vec<PubKey> = before
                .iter()
                .filter(|key| !after.contains_key(*key))
                .copied()
                .collect();
            for key in &retract {
                state
                    .pending
                    .retain(|(pending_key, _), _| pending_key != key);
            }
            let publish: Vec<(PubKey, String)> = after
                .into_iter()
                .filter(|(key, _)| !before.contains(key))
                .collect();
            (retract, publish)
        };
        for key in retract {
            if !registry.retract_wake_route(&self.namespace, &key, &self.waker()) {
                tracing::debug!(
                    namespace = %self.namespace,
                    "cross-mob wake routes: the route to retract was no longer this part's"
                );
            }
        }
        for (key, remote) in publish {
            // Never over another owner's route: it was free when this
            // write began, and is looked at again here.
            let published = self.may_route(&key)
                && registry.publish_wake_route(&self.namespace, key, remote, self.waker());
            if !published {
                locked(&self.state)
                    .links
                    .retain(|link| link.remote_key != key);
                tracing::warn!(
                    namespace = %self.namespace,
                    "cross-mob wake routes: a wake route was not published; its links are not held"
                );
            }
        }
    }

    /// Remove the link of `member` to `remote`, whatever key it names.
    fn drop_link(&self, member: &str, remote: &str) {
        let kept: Vec<Link> = locked(&self.state)
            .links
            .iter()
            .filter(|link| link.member == member && link.remote != remote)
            .cloned()
            .collect();
        self.set_member_links(member, kept);
    }

    /// Drop every link, wake route and pending wake of this part.
    fn clear(&self) {
        let registry = InprocRegistry::global();
        let _writing = locked(&self.writing);
        let keys: HashSet<PubKey> = {
            let mut state = locked(&self.state);
            state.pending.clear();
            state.links.drain().map(|link| link.remote_key).collect()
        };
        for key in keys {
            registry.retract_wake_route(&self.namespace, &key, &self.waker());
        }
    }

    /// The members of `gone` are gone with their runtime: the routes THIS
    /// namespace holds to their inboxes go. This part's links stay (its
    /// mob's rows still name them).
    fn forget_inboxes_of(&self, gone: &str) {
        let registry = InprocRegistry::global();
        for peer in registry.peers_in_namespace(&self.namespace) {
            if member_namespace(&peer.name).as_deref() == Some(gone) {
                registry.unregister_in_namespace(&self.namespace, &peer.pubkey);
            }
        }
    }

    /// Forward a wake of the member under `recipient`, published by this
    /// part as `member` (its comms name), for a send from `sender`.
    ///
    /// `ask` hands the request to the remote member's own owner, under that
    /// owner's name for the member. `slot` is the pending wake the send may
    /// share; `None` asks for this send alone.
    fn forward(
        &self,
        sender: PubKey,
        recipient: PubKey,
        member: &str,
        slot: Option<WakeSlot>,
        ask: impl FnOnce(&dyn InprocPeerWaker, String) -> PeerWakeFuture + Send + 'static,
    ) -> PeerWakeFuture {
        let registry = InprocRegistry::global();
        // The link, as BOTH mobs hold it. The local member is the one this
        // namespace routes under the sender's key; the remote member is the
        // one its row names under the recipient's key.
        let Some(sender_name) = registry.get_name_by_pubkey_in_namespace(&self.namespace, &sender)
        else {
            return denied(PeerWakeDenial::NotAMember);
        };
        let Some(link) = self.link(&sender_name, &recipient) else {
            return denied(PeerWakeDenial::NotAMember);
        };
        if link.remote != member {
            return denied(PeerWakeDenial::NotAMember);
        }
        let Some((remote_namespace, remote_member)) = member_of(&link.remote) else {
            return denied(PeerWakeDenial::NotAMember);
        };
        if remote_namespace == self.namespace {
            return denied(PeerWakeDenial::NotAMember);
        }
        let remote_holds = routes_of(&remote_namespace)
            .is_some_and(|remote| remote.holds(&link.remote, &sender_name, &sender));
        if !remote_holds {
            return denied(PeerWakeDenial::NotAMember);
        }
        // The member's own owner, as its mob published it: read for this
        // wake, under the linked member's own name, and no forwarder.
        let Some((owner_member, owner)) =
            registry.wake_route_owner_in_namespace(&remote_namespace, &recipient)
        else {
            return denied(PeerWakeDenial::NotAMember);
        };
        if owner_member != remote_member || is_forwarder(&owner) {
            return denied(PeerWakeDenial::NotAMember);
        }
        let Some(owner) = owner.upgrade() else {
            return denied(PeerWakeDenial::Refused(PeerWakeRefusal::MobUnavailable));
        };

        let this = Weak::clone(&self.this);
        let namespace = self.namespace.clone();
        let asking = move || -> SharedWake {
            // The owner is asked and let go: only its answer is kept.
            let asked = ask(owner.as_ref(), owner_member);
            drop(owner);
            async move {
                let answer = asked.await;
                if matches!(answer, Ok(PeerWakeAnswer::Ready)) {
                    // The member is ready in its own namespace: this
                    // namespace's route is pointed at its inbox of now
                    // before the send hears "ready", unless the link left
                    // while the wake was pending.
                    let still_linked = this
                        .upgrade()
                        .is_some_and(|this| this.holds(&sender_name, &link.remote, &recipient));
                    if !still_linked {
                        return Err(PeerWakeDenial::NotAMember);
                    }
                    if let Err(error) = install_route_alias(
                        &namespace,
                        &remote_namespace,
                        &link.remote,
                        recipient,
                        "remote",
                    ) {
                        // The send resolves the route again and answers a
                        // miss itself.
                        tracing::debug!(
                            member = %link.remote,
                            %error,
                            "cross-mob wake: the member answered ready and its route was not installed"
                        );
                    }
                }
                answer
            }
            .boxed()
            .shared()
        };
        let Some(slot) = slot else {
            return Box::pin(asking());
        };
        let waiter = {
            let mut state = locked(&self.state);
            let pending = state.pending.entry(slot).or_insert_with(|| PendingWake {
                wake: asking(),
                waiters: 0,
            });
            pending.waiters += 1;
            Waiter {
                routes: Weak::clone(&self.this),
                slot,
                wake: pending.wake.clone(),
            }
        };
        Box::pin(async move {
            let answer = waiter.wake.clone().await;
            // An answered wake is not joined by a later send.
            if let Some(routes) = waiter.routes.upgrade() {
                let mut state = locked(&routes.state);
                if state
                    .pending
                    .get(&waiter.slot)
                    .is_some_and(|pending| pending.wake.ptr_eq(&waiter.wake))
                {
                    state.pending.remove(&waiter.slot);
                }
            }
            answer
        })
    }
}

impl InprocPeerWaker for CrossMobWakeRoutes {
    fn wake(&self, request: PeerWakeRequest) -> PeerWakeFuture {
        let shared = request.input_class == PeerWakeInputClass::Content && !request.sender_waking;
        let slot = shared.then_some((request.recipient, request.observed_missing));
        let (sender, recipient, member) =
            (request.sender, request.recipient, request.member.clone());
        self.forward(
            sender,
            recipient,
            &member,
            slot,
            move |owner, owner_member| {
                // The same request, under the owner's own name for the member.
                let mut forwarded = request;
                forwarded.member = owner_member;
                owner.wake(forwarded)
            },
        )
    }

    /// The remote member's residency as its own mob publishes it, for a
    /// link both mobs hold; "not waking" otherwise.
    fn member_is_waking(&self, member: &str) -> bool {
        let linked: Vec<Link> = locked(&self.state)
            .links
            .iter()
            .filter(|link| link.remote == member)
            .cloned()
            .collect();
        let Some((remote_namespace, remote_member)) = member_of(member) else {
            return false;
        };
        let Some(remote) = routes_of(&remote_namespace) else {
            return false;
        };
        let Some(link) = linked.into_iter().find(|link| {
            locked(&remote.state)
                .links
                .iter()
                .any(|theirs| theirs.local == link.remote && theirs.remote == link.local)
        }) else {
            return false;
        };
        InprocRegistry::global()
            .wake_route_owner_in_namespace(&remote_namespace, &link.remote_key)
            .filter(|(owner_member, owner)| *owner_member == remote_member && !is_forwarder(owner))
            .and_then(|(owner_member, owner)| {
                owner
                    .upgrade()
                    .map(|owner| owner.member_is_waking(&owner_member))
            })
            .unwrap_or(false)
    }
}

/// The links `member`'s row names now: its committed edges to in-process
/// members of other mobs, each with the key the row holds. One row read.
async fn member_links(
    handle: &MobHandle,
    mob_id: &str,
    local_namespace: &str,
    member: &AgentIdentity,
) -> Result<Vec<Link>, MobError> {
    let Some(entry) = handle.get_member(member).await? else {
        return Ok(Vec::new());
    };
    let Ok(local) = meerkat_core::MemberCommsName::new(
        mob_id,
        entry.role.as_str(),
        entry.agent_identity.as_str(),
    ) else {
        return Ok(Vec::new());
    };
    let local = local.to_string();
    Ok(entry
        .wired_to
        .iter()
        .filter_map(|peer| {
            let spec = entry.external_peer_descriptor(peer.as_str())?;
            let remote_namespace = inproc_peer_namespace(spec)?;
            (remote_namespace != local_namespace && spec.pubkey != [0u8; 32]).then(|| Link {
                member: entry.agent_identity.as_str().to_string(),
                local: local.clone(),
                remote: spec.name.as_str().to_string(),
                remote_key: PubKey::new(spec.pubkey),
            })
        })
        .collect())
}

/// A wire to an in-process member of another mob is refused when a member of
/// the local mob is seated under that key: the key's wake route in the local
/// namespace is that member's, and stays its.
pub(super) fn refuse_locally_seated_key(
    local_mob_id: &str,
    spec: &TrustedPeerDescriptor,
) -> Result<(), CrossMobError> {
    let Some(remote_namespace) = inproc_peer_namespace(spec) else {
        return Ok(());
    };
    let local_namespace = mob_inproc_namespace_for_id(local_mob_id)?;
    if remote_namespace == local_namespace || spec.pubkey == [0u8; 32] {
        return Ok(());
    }
    let key = PubKey::new(spec.pubkey);
    let free =
        match InprocRegistry::global().wake_route_owner_in_namespace(&local_namespace, &key) {
            None => true,
            Some((_, owner)) => routes_of(&local_namespace)
                .is_some_and(|routes| Weak::ptr_eq(&owner, &routes.waker())),
        };
    if free {
        return Ok(());
    }
    Err(CrossMobError::InprocAlias(format!(
        "this mob already holds that key under another name, so {:?} cannot be wired in-process",
        spec.name.as_str()
    )))
}

/// After a wire of `member` committed, or an unwire of it failed: the
/// member's links are what its row names now.
pub(super) async fn sync_member_links(handle: &MobHandle, member: &AgentIdentity) {
    let mob_id = handle.mob_id().to_string();
    let Ok(namespace) = mob_inproc_namespace_for_id(&mob_id) else {
        return;
    };
    // A part is created by its runtime's start-time read only.
    let Some(routes) = routes_of(&namespace) else {
        // A launch that holds no part (it does not speak for the
        // composition) records the edge and holds no link for it.
        if member_links(handle, &mob_id, &namespace, member)
            .await
            .is_ok_and(|links| !links.is_empty())
        {
            tracing::warn!(
                mob_id = %mob_id,
                member = %member,
                "cross-mob wire on a launch that holds no link table: the edge is recorded, and \
                 no peer message of the other mob is forwarded for it"
            );
        }
        return;
    };
    let links = match member_links(handle, &mob_id, &namespace, member).await {
        Ok(links) => links,
        Err(error) => {
            // Fail closed: a member whose row cannot be read holds no link.
            tracing::warn!(
                mob_id = %mob_id,
                member = %member,
                %error,
                "cross-mob wake routes: the member's row could not be read; it holds no link"
            );
            Vec::new()
        }
    };
    routes.set_member_links(member.as_str(), links);
}

/// Before an unwire of `member` from `remote` commits: the link goes first.
pub(super) fn drop_member_link(mob_id: &str, member: &AgentIdentity, remote: &str) {
    let Ok(namespace) = mob_inproc_namespace_for_id(mob_id) else {
        return;
    };
    if let Some(routes) = routes_of(&namespace) {
        routes.drop_link(member.as_str(), remote);
    }
}

/// The start-time roster read: the mob's part of the link table is built
/// from its rows, once, on a part that starts empty. Returns the part, which
/// the runtime keeps to close it.
///
/// Cost: one member listing (the machine's current state, no store read),
/// and one row read for each member that is wired to a member of another
/// mob. Nobody is built.
pub(super) async fn restore_from_roster(handle: &MobHandle) -> Option<Arc<CrossMobWakeRoutes>> {
    let mob_id = handle.mob_id().to_string();
    let namespace = mob_inproc_namespace_for_id(&mob_id).ok()?;
    let routes = start_part(&namespace);
    for listed in handle.list_members().await {
        let names_another_mob = listed
            .wired_to
            .iter()
            .any(|peer| member_namespace(peer.as_str()).is_some_and(|remote| remote != namespace));
        if !names_another_mob {
            continue;
        }
        match member_links(handle, &mob_id, &namespace, &listed.agent_identity).await {
            Ok(links) => routes.set_member_links(listed.agent_identity.as_str(), links),
            Err(error) => tracing::warn!(
                mob_id = %mob_id,
                member = %listed.agent_identity,
                %error,
                "cross-mob wake routes: the member's row could not be read; it holds no link"
            ),
        }
    }
    Some(routes)
}

/// The runtime of `part` begins to shut down, or stops its mob: its whole
/// part of the link table goes, with its wake routes and pending wakes.
/// From here on no wake is forwarded for a link of this mob, in either
/// direction. The namespace's entry is taken only while it is still this
/// part (a later runtime of the mob has its own).
pub(super) fn close_part(part: &Arc<CrossMobWakeRoutes>) {
    {
        let mut namespaces = locked(&NAMESPACES);
        if namespaces
            .get(&part.namespace)
            .is_some_and(|current| Arc::ptr_eq(current, part))
        {
            namespaces.remove(&part.namespace);
        }
    }
    part.clear();
}

/// The shutdown of the runtime of `part` ran to its end: besides the part,
/// the routes its own namespace holds to other mobs' members go, and every
/// other part drops the routes ITS namespace holds to this mob's members
/// (they lead to inboxes that are gone). Nothing is swept when a later
/// runtime of the mob already holds the namespace: its start swept.
pub(super) fn clear_part(part: &Arc<CrossMobWakeRoutes>) {
    close_part(part);
    if routes_of(&part.namespace).is_none() {
        sweep_inbox_routes(&part.namespace);
    }
}

/// Drop the routes `namespace` holds to other mobs' members, and have every
/// other part drop the routes its namespace holds to `namespace`'s members.
fn sweep_inbox_routes(namespace: &str) {
    let registry = InprocRegistry::global();
    for peer in registry.peers_in_namespace(namespace) {
        if member_namespace(&peer.name).is_some_and(|theirs| theirs != namespace) {
            registry.unregister_in_namespace(namespace, &peer.pubkey);
        }
    }
    let others: Vec<Arc<CrossMobWakeRoutes>> = locked(&NAMESPACES)
        .values()
        .filter(|other| other.namespace != namespace)
        .cloned()
        .collect();
    for other in others {
        other.forget_inboxes_of(namespace);
    }
}

impl UnifiedRuntime {
    /// The start-time read of this mob's links to members of other mobs of
    /// this process ([`restore_from_roster`]), once per runtime, when its
    /// mob runs. A launch that does not speak for the composition holds no
    /// link: it wakes nobody.
    pub(super) async fn restore_cross_mob_wake_routes(&self) {
        if !self.speaks_for_composition() || self.cross_mob_routes.get().is_some() {
            return;
        }
        let handle = self.mob_runtime.handle();
        if let Some(part) = restore_from_roster(&handle).await
            && let Err(second) = self.cross_mob_routes.set(part)
        {
            // Another start-time read of this runtime won: this one's part
            // is not kept.
            close_part(&second);
        }
    }

    /// See [`close_part`].
    pub(super) fn close_cross_mob_links(&self) {
        if let Some(part) = self.cross_mob_routes.get() {
            close_part(part);
        }
    }

    /// See [`clear_part`].
    pub(super) fn clear_cross_mob_wake_routes(&self) {
        if let Some(part) = self.cross_mob_routes.get() {
            clear_part(part);
        }
    }
}

#[cfg(test)]
#[path = "cross_mob_routes_tests.rs"]
mod tests;
