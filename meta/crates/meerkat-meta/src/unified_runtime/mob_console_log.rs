//! The console log of the mobs a host serves beside its default mob.
//!
//! A mob's console events are a store of its own
//! ([`super::ConsoleEventStore::of_mob`]), kept in memory. What makes them
//! frames of the host's one console log, under that mob, is a registration
//! with the runtime's console projection
//! ([`MetaConsoleAggregator::register_mob_console_events`]). This module is
//! where the table of the other mobs' entries reaches that projection: an
//! entry's console events are registered when the entry is made and when a
//! projection is built over entries that exist, and the registration ends
//! with the entry, after what the mob's store still holds was projected.
//!
//! A runtime builds its console projection with its first console router
//! and may replace it (a host that sets another console log store): the
//! slot holds the one that is current. A runtime with no projection stores
//! no frame of any mob, the default mob's included.
//!
//! The console log itself is reached here too ([`HostConsoleLog`]): what it
//! keeps under a mob's id is removed before a mob is created under that id,
//! whether the runtime built a projection yet or not.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use meerkat_mob::MobId;

use super::ConsoleEventStore;
use super::mob_entry::MobEntry;
use crate::console_aggregator::mob_writers::{MobWriters, runtime_key_of};
use crate::console_aggregator::{
    ConsoleLogResult, ConsoleLogStore, ConsoleVisibilityPolicy, MetaConsoleAggregator,
};

/// The host's console log store, as the table of the other mobs' entries
/// reaches it: the store the runtime holds now. A host may set another
/// before it serves, so the table asks for it at each use.
#[derive(Clone)]
pub(super) struct HostConsoleLog(Arc<Mutex<Arc<dyn ConsoleLogStore>>>);

impl HostConsoleLog {
    pub(super) fn new(store: Arc<dyn ConsoleLogStore>) -> Self {
        Self(Arc::new(Mutex::new(store)))
    }

    /// The runtime holds `store` from here on.
    pub(super) fn set(&self, store: Arc<dyn ConsoleLogStore>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = store;
    }

    /// Remove every frame the log keeps under the mob `mob_id` and answer
    /// how many went ([`ConsoleLogStore::remove_frames_of_mob`]).
    pub(super) async fn remove_frames_of_mob(
        &self,
        mob_id: &meerkat_mob::MobId,
    ) -> ConsoleLogResult<u64> {
        let store: Arc<dyn ConsoleLogStore> = {
            let held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(&held)
        };
        store.remove_frames_of_mob(mob_id).await
    }
}

/// What a mob's entry holds of its mob's console events with the host: the
/// mob's own store as it stood when the entry was made, and the
/// registration of that store with the runtime's console projection.
///
/// Both are the ENTRY's, never asked again by the mob's id: a mob created
/// under a destroyed mob's id has an entry of its own, a store of its own
/// and a registration of its own, and what ends, reads or writes for one
/// entry reaches the other's by neither.
#[derive(Default)]
pub(super) struct EntryConsoleEvents {
    store: OnceLock<ConsoleEventStore>,
    registered: Mutex<Option<Registered>>,
}

/// One registration of an entry's console events: the projection that
/// answered it and the token it answered
/// ([`MetaConsoleAggregator::register_mob_console_events`]).
struct Registered {
    projection: MetaConsoleAggregator,
    token: uuid::Uuid,
}

impl EntryConsoleEvents {
    /// What the entry of a mob the host serves beside its default mob holds
    /// from its construction: `store`, the mob's own console events as the
    /// host's stores name them when the entry is built. The entry is built
    /// under the lock of the table's passes, so no other entry of the id
    /// stands then.
    pub(super) fn of(store: ConsoleEventStore) -> Self {
        let named = OnceLock::new();
        // A new cell takes its first value.
        let _ = named.set(store);
        Self {
            store: named,
            registered: Mutex::new(None),
        }
    }

    /// The console events of the entry's mob: the store the entry was
    /// built with ([`Self::of`]), whatever store the id names later. An
    /// entry built with none (the default mob's, which never asks) names
    /// the store the host's (`console_events`) answers for `mob_id` at its
    /// first ask.
    pub(super) fn store(
        &self,
        console_events: &ConsoleEventStore,
        mob_id: &MobId,
    ) -> ConsoleEventStore {
        self.store
            .get_or_init(|| console_events.of_mob(Some(mob_id)))
            .clone()
    }

    /// The store the entry holds: a table entry is built with its store
    /// ([`Self::of`]), the default mob's entry holds none (its console
    /// events are the host's own store).
    pub(super) fn kept(&self) -> Option<ConsoleEventStore> {
        self.store.get().cloned()
    }

    /// Take the entry's registration, when it has one: whoever takes it
    /// ends it.
    fn take_registered(&self) -> Option<Registered> {
        self.registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    fn set_registered(&self, registered: Registered) {
        *self
            .registered
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(registered);
    }
}

/// The runtime's current console projection and the host's payload policy
/// its registrations carry, as the table of the other mobs' entries reaches
/// them, and the host's registry of the writers under its mobs' ids
/// ([`MobWriters`]).
///
/// The registry is the HOST's: it is made with the slot, handed to every
/// projection the runtime builds, and outlives each of them. So a write of
/// a projection the runtime replaced is still counted when the host asks
/// whether the writers under an id are at rest ([`Self::rest_of`]).
#[derive(Clone, Default)]
pub(super) struct ConsoleProjectionSlot {
    built: Arc<Mutex<Option<BuiltProjection>>>,
    writers: MobWriters,
}

#[derive(Clone)]
struct BuiltProjection {
    projection: MetaConsoleAggregator,
    payload_policy: Arc<dyn ConsoleVisibilityPolicy>,
}

impl ConsoleProjectionSlot {
    fn built(&self) -> Option<BuiltProjection> {
        self.built
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The runtime built its console projection: the entries made from
    /// here on register their console events with it, and it opens the
    /// gates of those registrations in the host's registry.
    pub(super) fn set(
        &self,
        projection: MetaConsoleAggregator,
        payload_policy: Arc<dyn ConsoleVisibilityPolicy>,
    ) {
        projection.share_mob_writers(self.writers.clone());
        *self.built.lock().unwrap_or_else(PoisonError::into_inner) = Some(BuiltProjection {
            projection,
            payload_policy,
        });
    }

    /// The runtime's console projection is replaced or gone: nothing
    /// registers with it any more.
    pub(super) fn clear(&self) {
        *self.built.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// Whether nothing writes into the host's console log for a mob under
    /// the id `mob_id` any more: every registration made under the id
    /// ended, and none of their writes is in flight. Waits up to `bound`
    /// for the writes in flight ([`MobWriters::rest`]); a registration that
    /// has not ended is not at rest, and no wait makes it so. The host asks
    /// before it removes the frames under the id.
    pub(super) async fn rest_of(&self, mob_id: &MobId, bound: Duration) -> bool {
        self.writers.rest(&runtime_key_of(mob_id), bound).await
    }

    /// The frames under the id `mob_id` were removed: the registry forgets
    /// the registrations that wrote them, so a mob of another incarnation
    /// may be registered under the id ([`MobWriters::released`]).
    pub(super) fn released(&self, mob_id: &MobId) {
        self.writers.released(&runtime_key_of(mob_id));
    }

    /// Register the console events of `entry`'s mob with the runtime's
    /// console projection, when it has one: each event of that mob's store
    /// (`console_events` is the host's; the mob's own is asked of it) is
    /// from here on a frame of the host's console log under that mob.
    ///
    /// The mob's own store is named here whether the runtime has a
    /// projection or not: a mob that has console events of its own with the
    /// host is a mob the host serves beside its default mob, which is how
    /// the default mob's console lists tell it from a mob a member made.
    ///
    /// The store is the ENTRY's from here on ([`EntryConsoleEvents`]), and
    /// so is the registration: the entry keeps the token the projection
    /// answers, and its end presents it. A registration the entry still
    /// holds (with a projection the runtime replaced) ends first.
    ///
    /// REFUSED, and logged, where the host's registry still holds under the
    /// mob's id a registration of ANOTHER incarnation: the frames under the
    /// id are that mob's and were not removed, and no mob is registered
    /// over them. The mob then has no frames in the host's console log.
    /// Every create removes the frames under its id first, so this is not
    /// reached by a mob the host created in this process.
    pub(super) fn register(&self, console_events: &ConsoleEventStore, entry: &MobEntry) {
        let mob_id = entry.mob_id();
        let of_the_mob = entry.console_log.store(console_events, mob_id);
        let Some(built) = self.built() else {
            return;
        };
        if let Some(earlier) = entry.console_log.take_registered() {
            earlier
                .projection
                .end_mob_console_events(mob_id, earlier.token);
        }
        match built.projection.register_mob_console_events(
            mob_id,
            entry.mob_runtime.clone(),
            of_the_mob,
            built.payload_policy,
        ) {
            Ok(token) => entry.console_log.set_registered(Registered {
                projection: built.projection,
                token,
            }),
            Err(error) => {
                tracing::warn!(
                    mob_id = %mob_id,
                    %error,
                    "the console events of a mob of the host are not stored in its console log"
                );
            }
        }
    }

    /// End the registration of the console events of `entry`'s mob: what
    /// the mob's store still holds is projected first, since the live
    /// projection may stop before it received it, and then the
    /// registration goes. The frames stay in the host's console log.
    ///
    /// The store read is the ENTRY's own and the last projection presents
    /// the ENTRY's token, as every writer of a registration does: where the
    /// id names another mob's store or registration by now, neither is
    /// read, written for or ended. The end does not wait for a write of the
    /// registration that is in flight elsewhere: it marks the registration
    /// ended (no write of it starts from here on) and returns; the host
    /// waits for the writes in flight where it removes the frames
    /// ([`Self::rest_of`]), outside the lock this end runs under.
    ///
    /// Both reads are awaited from the heap. The end of an entry is awaited
    /// by the resolution of a named mob, and that by every arm of the
    /// unified dispatch that names one: a future awaited in place is part
    /// of each caller's frame, and these two would be in every such arm's.
    pub(super) async fn end(&self, console_events: &ConsoleEventStore, entry: &MobEntry) {
        let Some(registered) = entry.console_log.take_registered() else {
            return;
        };
        let mob_id = entry.mob_id();
        let of_the_mob = entry.console_log.store(console_events, mob_id);
        let events = Box::pin(of_the_mob.replay_all(None))
            .await
            .unwrap_or_default();
        if let Err(error) = Box::pin(registered.projection.project_mob_console_events(
            mob_id,
            registered.token,
            events,
        ))
        .await
        {
            tracing::warn!(
                mob_id = %mob_id,
                %error,
                "the last console events of a mob of the host were not all stored"
            );
        }
        registered
            .projection
            .end_mob_console_events(mob_id, registered.token);
    }

    /// End the registration of `entry`'s mob with no last projection: the
    /// runtime's console projection is being replaced, and the entry
    /// registers with the next one.
    pub(super) fn unregister(&self, entry: &MobEntry) {
        if let Some(registered) = entry.console_log.take_registered() {
            registered
                .projection
                .end_mob_console_events(entry.mob_id(), registered.token);
        }
    }
}
