//! Wake forwarding between two mobs of one process.
//!
//! A member's peer sends are routed inside its own mob's namespace. A member
//! of another mob that is not built has no route there, and its own mob's
//! wake routes live in the other namespace, which a namespace-scoped send
//! never looks at. This module is the host composition between the two: for
//! every link a mob holds to a member of another mob of this process, the
//! mob's namespace gets a forwarder's wake route under the remote member's
//! key whose owner is this module. Asked to wake, it forwards the request to
//! the remote member's own mob, and once that mob answered ready it installs
//! the route to the member's inbox of now.
//!
//! **The link table.** Process-wide, in memory, keyed by namespace: each mob
//! of this process has its own part. A link is one committed edge of a local
//! member's row to an in-process member of another mob. A part is a
//! projection of its mob's rows, and is written from them only:
//! - by every wire and unwire of this runtime that changes such an edge:
//!   after a wire committed, and before an unwire commits, with the row read
//!   again when the unwire fails ([`sync_member_links`],
//!   [`drop_member_link`]; called by `wire_local_edge` / `unwire_local_edge`
//!   and by `mutate_member_unchecked`, the one body of the bilateral wires);
//! - once when the runtime starts ([`restore_from_roster`]): one member
//!   listing, then one row read for each member that names a member of
//!   another mob. Nobody is built and no store is read.
//! - a runtime's whole part goes when the runtime begins to shut down or
//!   stops its mob ([`close_part`]); when its shutdown ran to its end the
//!   inbox routes of its links go too ([`clear_part`]). A part that was
//!   closed, or that a later runtime of the mob replaced, takes no link
//!   again.
//!
//! A part belongs to the runtime whose start-time read created it. Nothing
//! else creates one: a wire on a runtime that never ran the read (a launch
//! that does not speak for the composition) holds no link. Only its own
//! runtime closes a part, so the late teardown of a runtime that was
//! dropped cannot clear the part of the runtime built after it.
//!
//! **What is forwarded.** Only a content send, and only for a link BOTH mobs
//! hold: the sender's mob names the recipient, and the recipient's mob names
//! the sender, each by the other's key. The local member is resolved from
//! the sender's key, the remote member from the recipient's key, and the
//! remote namespace from the link, never from the request's text. Anything
//! else is answered as a key that is no member
//! ([`PeerWakeDenial::NotAMember`]): nothing is asked of the other mob and
//! nothing is installed. A silent lifecycle notice is never forwarded: it is
//! answered as a key that is no member, as before there was a forwarder (the
//! other mob's machine could not carry another mob's notice to the member's
//! next wake). The forwarded request is the same request, under the name
//! the remote member's own mob published for the member, which must be the
//! linked member's; the remote mob's machine decides, and its answer is
//! passed on unchanged. The owner is read for each wake and never kept.
//!
//! **What this module writes.** In its own namespace: its forwarder's wake
//! routes (never over a route another owner holds, and it retracts only its
//! own) and the inbox routes of its links. A key that lost its last link
//! loses both: its wake route, and the inbox route the namespace holds
//! under that key while it is under the link's name, whichever wire or
//! wake installed it (an unwire that fails reads the link back, and the
//! next forwarded wake installs the route again). In another mob's
//! namespace: nothing. When a runtime's shutdown ran to its end, each OTHER
//! part drops
//! the inbox routes of its own links to that runtime's members, in its own
//! namespace (they lead to inboxes that are gone). A route no link names is
//! never touched. This module never reads or changes another mob's state.
//!
//! **Pending wakes.** Content sends from senders that are not themselves
//! being built share one pending wake per remote member and observation
//! (at most two per linked key): every waiting send gets that wake's
//! answer for its own link (a link that left while the wake was pending is
//! answered as no member), one waiter leaving does not end it, and it is
//! forgotten when it is answered or when its last waiter left. A send from
//! a member that is being built is asked for itself, of the remote machine.
//!
//! **Locks.** `state` is taken by the methods the registry calls (`wake`,
//! `member_is_waking`). Nothing is called while it is held: no registry
//! call, no owner, and it is never held across an await. `writing` orders
//! the writers of one part, and the install of an inbox route after a wake
//! with them; it is taken by nothing the registry calls, never while `state`
//! is held, and never held across an await.

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

/// What the registry's request says about the send, as far as forwarding
/// depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WakeShape {
    input_class: PeerWakeInputClass,
    /// The sender is itself being built.
    sender_waking: bool,
    /// The send observed the member missing.
    observed_missing: bool,
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
    /// The part was closed by its runtime, or replaced by the part of a
    /// later runtime of the mob: it takes no link again. Set under the
    /// writers' lock, in the write that drops its links.
    closed: bool,
    /// The links the part held when its runtime closed it, kept for the
    /// sweep of their inbox routes at the end of the shutdown. `None` while
    /// the part is open, and for a part that was no longer its namespace's
    /// when it was closed.
    closed_links: Option<Vec<Link>>,
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
/// runtime of the mob left there (that part's links and wake routes go).
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

/// Whether the link from `local` (under `local_key`) to `remote` (under
/// `remote_key`) is held by BOTH mobs now: by `part`, and by the part of the
/// remote member's mob, each naming the other by its key.
fn whole_link(
    part: &CrossMobWakeRoutes,
    local: &str,
    local_key: &PubKey,
    remote: &str,
    remote_key: &PubKey,
) -> bool {
    if !part.holds(local, remote, remote_key) {
        return false;
    }
    let Some(remote_namespace) = member_namespace(remote) else {
        return false;
    };
    remote_namespace != part.namespace
        && routes_of(&remote_namespace).is_some_and(|theirs| theirs.holds(remote, local, local_key))
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
    /// key that lost its last link loses it, with its pending wakes and with
    /// the inbox route this namespace held for it.
    fn set_member_links(&self, member: &str, links: Vec<Link>) {
        let registry = InprocRegistry::global();
        let _writing = locked(&self.writing);
        // Read the registry before `state` is taken.
        let routable: Vec<Link> = links
            .into_iter()
            .filter(|link| self.may_route(&link.remote_key))
            .collect();
        let (retract, forget, publish) = {
            let mut state = locked(&self.state);
            if state.closed {
                // The runtime closed this part, or a later runtime of the
                // mob replaced it: it holds no link any more.
                return;
            }
            let held: Vec<Link> = state.links.iter().cloned().collect();
            let before: HashSet<PubKey> = held.iter().map(|link| link.remote_key).collect();
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
            let forget: Vec<Link> = held
                .into_iter()
                .filter(|link| retract.contains(&link.remote_key))
                .collect();
            let publish: Vec<(PubKey, String)> = after
                .into_iter()
                .filter(|(key, _)| !before.contains(key))
                .collect();
            (retract, forget, publish)
        };
        for key in retract {
            if !registry.retract_wake_route(&self.namespace, &key, &self.waker()) {
                tracing::debug!(
                    namespace = %self.namespace,
                    "cross-mob wake routes: the route to retract was no longer this part's"
                );
            }
        }
        // The inbox route of a key that lost its last link goes with the
        // link: a route no link names would never be taken again.
        self.forget_inbox_routes(&forget);
        for (key, remote) in publish {
            // Never over another owner's route: it was free when this
            // write began, and is looked at again here.
            let published = self.may_route(&key)
                && registry.publish_forwarder_wake_route(
                    &self.namespace,
                    key,
                    remote,
                    self.waker(),
                );
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

    /// Drop every link, wake route and pending wake of this part, and mark
    /// it closed before the writers' lock is released: a write that already
    /// held the part adds no link after this. Returns the links it held.
    fn clear(&self) -> Vec<Link> {
        let registry = InprocRegistry::global();
        let _writing = locked(&self.writing);
        let links: Vec<Link> = {
            let mut state = locked(&self.state);
            state.closed = true;
            state.pending.clear();
            state.links.drain().collect()
        };
        let keys: HashSet<PubKey> = links.iter().map(|link| link.remote_key).collect();
        for key in keys {
            registry.retract_wake_route(&self.namespace, &key, &self.waker());
        }
        links
    }

    /// Drop the inbox route this namespace holds for each of `links`: the
    /// route under the link's key, while it is still under the link's name.
    fn forget_inbox_routes(&self, links: &[Link]) {
        let registry = InprocRegistry::global();
        for link in links {
            let routed =
                registry.get_name_by_pubkey_in_namespace(&self.namespace, &link.remote_key);
            if routed.as_deref() == Some(link.remote.as_str()) {
                registry.unregister_in_namespace(&self.namespace, &link.remote_key);
            }
        }
    }

    /// The members of `gone` are gone with their runtime: the inbox routes
    /// of THIS part's links to them go, in this part's own namespace. The
    /// links stay (this mob's rows still name them).
    fn forget_inboxes_of(&self, gone: &str) {
        let links: Vec<Link> = locked(&self.state)
            .links
            .iter()
            .filter(|link| member_namespace(&link.remote).as_deref() == Some(gone))
            .cloned()
            .collect();
        self.forget_inbox_routes(&links);
    }

    /// Point this namespace's route under `remote_key` at the inbox the
    /// member `remote` has now, while some link of this part to it is whole.
    /// In the writers' order: a key that loses its last link takes the route
    /// with it ([`Self::set_member_links`]), so the route is installed
    /// before that write, which then drops it, or not at all.
    fn install_inbox_route(&self, remote_namespace: &str, remote: &str, remote_key: PubKey) {
        let _writing = locked(&self.writing);
        if !self.some_whole_link_to(remote, &remote_key) {
            return;
        }
        if let Err(error) = install_route_alias(
            &self.namespace,
            remote_namespace,
            remote,
            remote_key,
            "remote",
        ) {
            // The send resolves the route again and answers a miss itself.
            tracing::debug!(
                member = %remote,
                %error,
                "cross-mob wake: the member answered ready and its route was not installed"
            );
        }
    }

    /// Forward a wake of the member under `recipient`, published by this
    /// part as `member` (its comms name), for a send from `sender`.
    ///
    /// `ask` hands the request to the remote member's own owner, under that
    /// owner's name for the member.
    fn forward(
        &self,
        sender: PubKey,
        recipient: PubKey,
        member: &str,
        shape: WakeShape,
        ask: impl FnOnce(&dyn InprocPeerWaker, String) -> PeerWakeFuture + Send + 'static,
    ) -> PeerWakeFuture {
        let registry = InprocRegistry::global();
        // A silent lifecycle notice is never forwarded: the other mob's
        // machine could not carry another mob's notice to the member's next
        // wake, so the answer is the one such a notice had without a
        // forwarder.
        if shape.input_class != PeerWakeInputClass::Content {
            return denied(PeerWakeDenial::NotAMember);
        }
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
        if !whole_link(self, &sender_name, &sender, &link.remote, &recipient) {
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

        // One wake of the remote member: ask its owner, and once it answered
        // ready point this namespace's route at the member's inbox of now,
        // before any waiting send hears "ready". The route is installed for
        // the key while some link of this part to it is still whole.
        let this = Weak::clone(&self.this);
        let remote_name = link.remote.clone();
        let asking = move || -> SharedWake {
            // The owner is asked and let go: only its answer is kept.
            let asked = ask(owner.as_ref(), owner_member);
            drop(owner);
            async move {
                let answer = asked.await;
                if matches!(answer, Ok(PeerWakeAnswer::Ready))
                    && let Some(this) = this.upgrade()
                {
                    this.install_inbox_route(&remote_namespace, &remote_name, recipient);
                }
                answer
            }
            .boxed()
            .shared()
        };
        // A send from a member that is itself being built shares no wake:
        // the remote machine answers it for itself.
        let slot = (!shape.sender_waking).then_some((recipient, shape.observed_missing));
        let (wake, waiter) = match slot {
            None => (asking(), None),
            Some(slot) => {
                let joined = {
                    let mut state = locked(&self.state);
                    state.pending.get_mut(&slot).map(|pending| {
                        pending.waiters += 1;
                        pending.wake.clone()
                    })
                };
                let wake = match joined {
                    Some(wake) => wake,
                    None => {
                        // The owner is asked with no lock of this part held.
                        let mine = asking();
                        let mut state = locked(&self.state);
                        let pending = state.pending.entry(slot).or_insert_with(|| PendingWake {
                            wake: mine,
                            waiters: 0,
                        });
                        pending.waiters += 1;
                        pending.wake.clone()
                    }
                };
                let waiter = Waiter {
                    routes: Weak::clone(&self.this),
                    slot,
                    wake: wake.clone(),
                };
                (wake, Some(waiter))
            }
        };
        let this = Weak::clone(&self.this);
        Box::pin(async move {
            let answer = wake.await;
            // An answered wake is not joined by a later send.
            if let Some(waiter) = &waiter
                && let Some(routes) = waiter.routes.upgrade()
            {
                let mut state = locked(&routes.state);
                if state
                    .pending
                    .get(&waiter.slot)
                    .is_some_and(|pending| pending.wake.ptr_eq(&waiter.wake))
                {
                    state.pending.remove(&waiter.slot);
                }
            }
            // "Ready" is this send's answer only while ITS link is still
            // whole: a link that left on either side while the wake was
            // pending is no member.
            if matches!(answer, Ok(PeerWakeAnswer::Ready)) {
                let still_whole = this.upgrade().is_some_and(|this| {
                    whole_link(&this, &sender_name, &sender, &link.remote, &recipient)
                });
                if !still_whole {
                    return Err(PeerWakeDenial::NotAMember);
                }
            }
            answer
        })
    }

    /// Whether some link of this part to the member `remote` under
    /// `remote_key` is held by both mobs now.
    fn some_whole_link_to(&self, remote: &str, remote_key: &PubKey) -> bool {
        let registry = InprocRegistry::global();
        let locals: Vec<String> = locked(&self.state)
            .links
            .iter()
            .filter(|link| link.remote == remote && link.remote_key == *remote_key)
            .map(|link| link.local.clone())
            .collect();
        locals.into_iter().any(|local| {
            // The local member's key is the one this namespace routes its
            // name under, when it is built.
            registry
                .peers_in_namespace(&self.namespace)
                .into_iter()
                .find(|peer| peer.name == local)
                .is_some_and(|peer| whole_link(self, &local, &peer.pubkey, remote, remote_key))
        })
    }
}

impl InprocPeerWaker for CrossMobWakeRoutes {
    fn wake(&self, request: PeerWakeRequest) -> PeerWakeFuture {
        let shape = WakeShape {
            input_class: request.input_class,
            sender_waking: request.sender_waking,
            observed_missing: request.observed_missing,
        };
        let (sender, recipient, member) =
            (request.sender, request.recipient, request.member.clone());
        self.forward(
            sender,
            recipient,
            &member,
            shape,
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
/// direction, and no later write adds a link to this part. The namespace's
/// entry is taken only while it is still this part (a later runtime of the
/// mob has its own); the links such a part held are kept for
/// [`clear_part`].
pub(super) fn close_part(part: &Arc<CrossMobWakeRoutes>) {
    let was_current = {
        let mut namespaces = locked(&NAMESPACES);
        let current = namespaces
            .get(&part.namespace)
            .is_some_and(|current| Arc::ptr_eq(current, part));
        if current {
            namespaces.remove(&part.namespace);
        }
        current
    };
    let links = part.clear();
    let mut state = locked(&part.state);
    if was_current && state.closed_links.is_none() {
        state.closed_links = Some(links);
    }
}

/// The shutdown of the runtime of `part` ran to its end, so its members'
/// inboxes are gone: the inbox routes of the links it held go, in its own
/// namespace, and every other part drops the inbox routes of ITS links to
/// this mob's members, in its own namespace. A part that was no longer its
/// namespace's when it was closed sweeps nothing (a later runtime of the
/// mob runs there), and a route no link names is never touched.
pub(super) fn clear_part(part: &Arc<CrossMobWakeRoutes>) {
    close_part(part);
    let Some(links) = locked(&part.state).closed_links.replace(Vec::new()) else {
        return;
    };
    if routes_of(&part.namespace).is_some() {
        // A later runtime of the mob already started in this namespace.
        return;
    }
    part.forget_inbox_routes(&links);
    let others: Vec<Arc<CrossMobWakeRoutes>> = locked(&NAMESPACES).values().cloned().collect();
    for other in others {
        other.forget_inboxes_of(&part.namespace);
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
