//! The hosted set: one `MobStoreRestoreLifecycleMachine` record per mob
//! store this state hosts.
//!
//! "Which mobs does this host have" is answered here and nowhere else. Each
//! record's phase says which set its store's mob is in: dormant (known, not
//! loaded), loading, managed, unloading (its idle unload is admitted and its
//! actor is being asked), held before activation, quarantined, retrying,
//! creating (a create of its mob is in flight and holds the id), or
//! gone. The managed, held, quarantined, dormant and retired views of
//! [`crate::MobMcpState`] are reads of these records; no map beside them
//! decides which set a mob is in.
//!
//! A mob is handed out exactly while its record says it is served, which the
//! machine keeps true in Managed and in no other phase. A record whose unload
//! was admitted is not served: a lookup of it waits for the unload's answer
//! and is then served by the mob loaded again, or by the same mob when its
//! actor answered that it is busy. It is not hidden: the readers that hand
//! out no handle (the list, the inventory, a read of a mob's own facts) see
//! the mob until its actor closed ([`HostedStores::loaded_stores`]), and its
//! store is dormant only once that actor has ended. Whether an unload is
//! admitted is the record's decision (only a store the set can load again);
//! no code here reads a record's origin. The answer to an admitted unload is
//! the actor's: nothing here resolves an unload on the actor's behalf.
//!
//! Whether a mob that is not loaded is stopped is a field of its record
//! too ([`HostedStore::stop`]): the mob's own answer when its actor closed
//! at an unload ([`HostedStores::resolve_unload`]), or the set's read of
//! the dormant store ([`HostedStores::note_stop_at`]), which the record
//! takes only while it does not know the answer. A load asked as a demand
//! ([`HostedStores::admit_load_as_a_demand`]) is refused by the record of
//! a stopped mob; no map beside the records says which mobs are stopped.
//!
//! The records live in one map under one lock. A move of a mob between sets
//! is one transition of one record under that lock's write side, so a reader
//! finds a mob in exactly one set and never in none. Every machine input is
//! applied in this module: the rest of the crate asks for a transition by
//! calling a method here and reads the verdict.
//!
//! What a record carries beside its machine state is mechanics for the phase
//! the machine names: the handle of a managed mob, the hold of a held one,
//! the failure a quarantined one met, the report of a destroyed one. None of
//! it is read to decide a phase.
//!
//! # Facts about the set as a whole
//!
//! Some facts are about the set and not about one store, so no store's
//! record can hold them. They are the state of one more machine, the set's
//! own (`MobHostedSetLifecycleMachine`, one state per set, [`SetFacts`]),
//! and none of them moves a mob between sets:
//!
//! - how far the pass over the stores this host has from before came: not
//!   begun, begun and not finished, or finished. The first pass is named by
//!   the machine ([`HostedSet::begin_pass`]); that one pass runs at a time
//!   is a lock of [`crate::MobMcpState`], which holds no fact.
//! - whether a census of such stores is owed at all: the host has a
//!   persistent root, or declared a default mob with a store file
//!   ([`HostedSet::expect_census`], said while the state is built).
//! - whether the census is complete: every store this host has from before
//!   is recorded (a pass finished, or the host declared its own restore
//!   done). Until then a mob id no record names may still turn up, so it is
//!   not answered as gone.
//! - whether a host registered a handle of its own, so that only the host
//!   can say when the set is complete. It is a latch and cannot be read
//!   from the records: the record of a destroyed host-inserted mob is
//!   dropped, after which "some record has the host-handle origin" would be
//!   false again while the host is still inserting its mobs. It is noted in
//!   the step that makes the handle visible
//!   ([`HostedStores::register_host_handle`])
//!   and never cleared, so it can only keep a member lookup incomplete for
//!   longer, never complete it early.
//! - whether every mob of the host is known ([`HostedSet::all_mobs_known`]):
//!   the one answer kept from the facts above, so that no code outside the
//!   machine combines them.
//!
//! Every input of that machine is applied in this module, through one
//! function. One more thing is about the set and is no lifecycle:
//!
//! - the host's forked-participant table ([`HostedSet::custody_table`]):
//!   the table of the declared store's file is the one such table of the
//!   host. It is configuration of the declaration ([`DeclaredDefault`]:
//!   the default mob's id, the file of its store, that table).
//!
//! # The host's default mob
//!
//! What makes the declared store its launch's is in that store's record,
//! which is filed at the declaration ([`HostedStores::declare`]) and lasts
//! as long as the process: the record's origin says the id is the
//! launch's, and its machine refuses every create and every handle but the
//! launch's own, a destroy through the set, an idle unload, and a pass's
//! load. Whether the mob still waits for its launch's first serve is a
//! field of the record too: a lookup that would load the mob waits for the
//! end of that wait ([`HostedSet::await_launch_wait_end`]), which the
//! launch's serve applies when it has ended, however it ended, and a
//! stand-down applies as well ([`HostedSet::end_launch_wait`]). Nothing
//! here keeps a second copy of any of it: each read below asks the record.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use meerkat_mob::machines::mob_hosted_set_lifecycle as set_dsl;
use meerkat_mob::machines::mob_store_restore_lifecycle as dsl;
use meerkat_mob::{MobError, MobHandle, MobId, MobState};

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use crate::{HeldChildMob, QuarantinedMob, RetiredMob, StoreQuarantine, StoreRestored};

type Record = dsl::MobStoreRestoreLifecycleMachineAuthority;
type Input = dsl::MobStoreRestoreLifecycleInput;
type Effect = dsl::MobStoreRestoreLifecycleEffect;
pub(crate) type Phase = dsl::MobStoreRestoreLifecyclePhase;
type Outcome = dsl::MobStoreRestoreOutcome;
pub(crate) type UnloadOutcome = dsl::MobStoreUnloadOutcome;
type CreateOutcome = dsl::MobStoreCreateOutcome;
type Stop = dsl::MobStoreStop;
type SetRecord = set_dsl::MobHostedSetLifecycleMachineAuthority;
type SetInput = set_dsl::MobHostedSetLifecycleInput;
type SetEffect = set_dsl::MobHostedSetLifecycleEffect;
type SetPhase = set_dsl::MobHostedSetLifecyclePhase;

/// The state of a hosted set as a whole: the one state of its
/// `MobHostedSetLifecycleMachine`. It holds what no store's record can: how
/// far the pass over the stores from before came, whether a census of them
/// is owed and complete, whether a host registered a handle of its own,
/// and whether every mob is known. In process only, like every record.
pub(crate) struct SetFacts {
    record: SetRecord,
}

impl SetFacts {
    fn new() -> Self {
        Self {
            record: SetRecord::new(),
        }
    }

    /// Apply one input and answer its effects: the one place an input of
    /// the set's machine is applied. The machine is total: an input it
    /// refuses comes back as a `RequestRefused` effect, and an error here
    /// means the generated kernel itself failed, which is logged and read
    /// as "nothing happened".
    fn apply(&mut self, input: SetInput) -> Vec<SetEffect> {
        match set_dsl::MobHostedSetLifecycleMachineMutator::apply(&mut self.record, input) {
            Ok(transition) => transition.effects().to_vec(),
            Err(error) => {
                tracing::error!(error = ?error, "the hosted set's machine refused an input");
                Vec::new()
            }
        }
    }
}

/// Apply `input` to the shared state of the set and answer its effects.
fn apply_to_set(set: &std::sync::Mutex<SetFacts>, input: SetInput) -> Vec<SetEffect> {
    set.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .apply(input)
}
type Refusal = dsl::MobStoreRestoreRefusal;

/// The token of one admitted load, re-attempt or unload: alive while that
/// attempt runs. A record left Loading, Retrying or Unloading whose token is
/// gone was left by an attempt that ended without resolving it (its future
/// was dropped, or it panicked). Dropping the token wakes everything that
/// waits on the set, so a waiter never sleeps past an abandoned attempt.
pub(crate) struct AttemptToken {
    changes: tokio::sync::watch::Sender<u64>,
    /// Set by a load that is left to run behind the pass that started it
    /// ([`Self::leave_to_run`]).
    left_to_run: tokio::sync::watch::Sender<bool>,
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl AttemptToken {
    fn new(changes: tokio::sync::watch::Sender<u64>) -> Arc<Self> {
        Arc::new(Self {
            changes,
            left_to_run: tokio::sync::watch::Sender::new(false),
        })
    }

    /// The attempt goes on without the pass that started it: the pass stops
    /// waiting for it, and whoever addresses the store's mob waits for the
    /// attempt as for any load in flight. A pass that already waits for the
    /// attempt is woken to read this.
    fn leave_to_run(&self) {
        self.left_to_run.send_replace(true);
        self.changes
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    /// Whether the attempt was left to run ([`Self::leave_to_run`]). The
    /// receiver closes when the attempt ends.
    pub(crate) fn left_to_run(&self) -> tokio::sync::watch::Receiver<bool> {
        self.left_to_run.subscribe()
    }

    /// Whether the attempt was left to run, now.
    fn is_left_to_run(&self) -> bool {
        *self.left_to_run.borrow()
    }
}

impl Drop for AttemptToken {
    fn drop(&mut self) {
        self.changes
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }
}

/// One mob store this state hosts: its record and the mechanics of the phase
/// the record names.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) struct HostedStore {
    mob_id: MobId,
    record: Record,
    /// The store's file, for a persisted store: a file under the persistent
    /// root, or the declared file of the host's default mob.
    path: Option<PathBuf>,
    /// The mob's handle, while the record is Managed or Unloading: the mob's
    /// actor exists in both. It is handed out only while the record says the
    /// mob is served ([`Self::handle`]).
    handle: Option<MobHandle>,
    /// The hold, while the record is Held.
    held: Option<HeldChildMob>,
    /// The failure the last load or re-attempt met, while the record is
    /// Quarantined or Retrying.
    cause: Option<Arc<MobError>>,
    /// The load, re-attempt or unload the record admitted last.
    attempt: Weak<AttemptToken>,
    /// The destroy this state ran, once the record is Gone.
    retired: Option<Arc<RetiredMob>>,
    /// Who created the mob, as its creation recorded it: read from the
    /// store in an open this state made anyway (the sweep over the
    /// dormant mobs, a load), or known from the create. It never changes
    /// for a mob and is no fact of the record's lifecycle: it is kept
    /// here, in process only, so that a listing names it for a mob that
    /// is not loaded without opening its store.
    creation: Option<meerkat_mob::MobProvenance>,
    /// The mob's own incarnation, as its creation recorded it: read from
    /// the store in an open this state made anyway (the sweep over the
    /// dormant mobs, a load), or known from the create. Like `creation`
    /// it is no fact of the record's lifecycle: it is kept here, in
    /// process only, so that a listing names it for a mob that is not
    /// loaded without opening its store.
    incarnation: Option<meerkat_core::service::MobIncarnationId>,
    /// When the mob became managed in this process: the idle unload leaves a
    /// mob alone until it has been loaded for its quiet period.
    #[cfg(not(target_arch = "wasm32"))]
    managed_since: Option<std::time::Instant>,
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl HostedStore {
    fn new(mob_id: MobId, path: Option<PathBuf>) -> Self {
        Self {
            mob_id,
            record: Record::new(),
            path,
            handle: None,
            held: None,
            cause: None,
            attempt: Weak::new(),
            retired: None,
            creation: None,
            incarnation: None,
            #[cfg(not(target_arch = "wasm32"))]
            managed_since: None,
        }
    }

    /// The mob became managed now.
    fn note_managed(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.managed_since = Some(std::time::Instant::now());
        }
    }

    /// Whether the mob has been managed for at least `loaded_for`.
    #[cfg(not(target_arch = "wasm32"))]
    fn managed_for(&self, loaded_for: std::time::Duration) -> bool {
        self.managed_since
            .is_some_and(|since| since.elapsed() >= loaded_for)
    }

    pub(crate) fn mob_id(&self) -> &MobId {
        &self.mob_id
    }

    /// Who created the store's mob, when this state has read it or knows it
    /// from the create.
    pub(crate) fn creation(&self) -> Option<&meerkat_mob::MobProvenance> {
        self.creation.as_ref()
    }

    pub(crate) fn phase(&self) -> Phase {
        self.record.state().lifecycle_phase
    }

    /// The store's file, for a persisted store: a file under the persistent
    /// root, or the declared file of the host's default mob.
    pub(crate) fn storage_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The handle to hand out: `None` unless the record says its mob is
    /// served. The machine keeps that true only in Managed, and false from
    /// the step that admits an unload.
    pub(crate) fn handle(&self) -> Option<&MobHandle> {
        if self.record.state().served {
            self.handle.as_ref()
        } else {
            None
        }
    }

    /// The handle of the mob whose actor this state runs for the store:
    /// the served one, or the one an admitted unload is asking. Never handed
    /// to a caller. The set reads it to end the unload, to close the actor's
    /// admission at a stand-down and to tell which mob a destroy removed;
    /// and a reader that hands out no handle reads the mob's facts from it
    /// (its lifecycle, its definition, its owner), so that a mob stays
    /// visible while its unload is not answered.
    pub(crate) fn loaded_handle(&self) -> Option<&MobHandle> {
        match self.phase() {
            Phase::Managed | Phase::Unloading => self.handle.as_ref(),
            _ => None,
        }
    }

    /// Whether an attempt the record admitted still runs.
    fn attempt_runs(&self) -> bool {
        self.attempt.upgrade().is_some()
    }

    /// The hold: `None` unless the record is Held.
    pub(crate) fn held(&self) -> Option<&HeldChildMob> {
        match self.phase() {
            Phase::Held => self.held.as_ref(),
            _ => None,
        }
    }

    /// The destroy this state ran: `None` unless the record is Gone.
    pub(crate) fn retired(&self) -> Option<&Arc<RetiredMob>> {
        match self.phase() {
            Phase::Gone => self.retired.as_ref(),
            _ => None,
        }
    }

    fn is_quarantined(&self) -> bool {
        matches!(self.phase(), Phase::Quarantined | Phase::Retrying)
    }

    /// Whether the record's mob still waits for its launch's first serve.
    /// Only a declared record ever does.
    pub(crate) fn awaits_launch(&self) -> bool {
        self.record.state().awaits_launch
    }

    /// Whether the store's mob, which is not loaded, is stopped: what the
    /// mob itself answered when its actor closed at an unload, or what the
    /// set read from the store. `None` while the record does not know it,
    /// and for every record whose mob is loaded, held, quarantined or
    /// gone: a loaded mob speaks for itself.
    pub(crate) fn stop(&self) -> Option<bool> {
        match self.record.state().stop {
            Stop::Unread => None,
            Stop::NotStopped => Some(false),
            Stop::Stopped => Some(true),
        }
    }

    /// Whether the declared store is known to hold no mob, and nothing
    /// waits for its launch any more: there is nothing under the record to
    /// serve or to load until the launch creates the mob.
    fn holds_nothing_to_serve(&self) -> bool {
        let state = self.record.state();
        state.vacant && !state.awaits_launch
    }

    /// Known and not loaded: dormant, or its one load in flight. A declared
    /// store that holds no mob counts only while its mob waits for its
    /// launch (a lookup of it then waits too).
    fn is_unloaded(&self) -> bool {
        matches!(self.phase(), Phase::Dormant | Phase::Loading) && !self.holds_nothing_to_serve()
    }

    /// Known and not served now, served again by a lookup: not loaded, or
    /// its admitted unload in flight.
    fn is_awaiting_service(&self) -> bool {
        self.is_unloaded() || self.phase() == Phase::Unloading
    }

    /// Whether `effects` carry the refusal `reason`.
    fn refused(effects: &[Effect], reason: Refusal) -> bool {
        effects.iter().any(
            |effect| matches!(effect, Effect::RequestRefused { reason: got } if *got == reason),
        )
    }

    fn quarantine_cause(&self) -> Arc<MobError> {
        self.cause.clone().unwrap_or_else(|| {
            Arc::new(MobError::Internal(
                "the quarantined store kept no cause".to_string(),
            ))
        })
    }

    /// The answer a lookup of the quarantined mob gets.
    pub(crate) fn quarantine_error(&self) -> MobError {
        MobError::MobQuarantined {
            mob_id: self.mob_id.clone(),
            class: self.record.state().class,
            cause: self.quarantine_cause(),
        }
    }

    /// The inventory row of the quarantined mob.
    pub(crate) fn quarantine_row(&self) -> QuarantinedMob {
        let state = self.record.state();
        QuarantinedMob {
            mob_id: self.mob_id.clone(),
            class: state.class,
            retrying: state.lifecycle_phase == Phase::Retrying,
            attempts: state.attempts,
            cause: self.quarantine_cause(),
        }
    }

    /// Apply one input and answer its effects. The machine is total: an
    /// input it refuses comes back as a `RequestRefused` effect, and an
    /// error here means the generated kernel itself failed, which is logged
    /// and read as "nothing happened".
    fn apply(&mut self, input: Input) -> Vec<Effect> {
        match dsl::MobStoreRestoreLifecycleMachineMutator::apply(&mut self.record, input) {
            Ok(transition) => transition.effects().to_vec(),
            Err(error) => {
                tracing::error!(
                    mob_id = %self.mob_id,
                    error = ?error,
                    "the mob store record refused an input"
                );
                Vec::new()
            }
        }
    }
}

/// What the record of a managed mob answered a request for its unload
/// ([`HostedStores::admit_unload`]).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) enum UnloadAdmission {
    /// The record admitted the unload: ask the mob's actor through `handle`
    /// and resolve the record with its answer, holding `attempt` until then.
    Admitted {
        handle: MobHandle,
        attempt: Arc<AttemptToken>,
    },
    /// The record refused: this state cannot load the mob's store again (a
    /// mob that lives in memory, a host's own handle, the launch's mob).
    NotReloadable,
    /// No unload was admitted for another reason: no record of the mob is
    /// managed (or managed for long enough), its record holds no handle to
    /// ask, or its unload is in flight already.
    NotAdmitted,
}

/// How a load request for a store ended at its admission.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum LoadAdmission {
    /// The record admitted the one load: run it, holding `attempt` until the
    /// record is resolved.
    Admitted {
        path: PathBuf,
        attempt: Arc<AttemptToken>,
    },
    /// A load of the store is in flight: wait for the set to change and read
    /// again.
    InFlight,
    /// The store is its launch's and the launch has not ended its first
    /// serve: wait for the end of that wait and read again.
    AwaitsLaunch,
    /// Nothing to load under the id now: read the set again.
    NotDormant,
    /// A demand met a mob that is stopped and not loaded: nothing is
    /// loaded, and the caller answers what a loaded stopped mob answers
    /// that call.
    Stopped,
    /// A demand met a store whose record does not know yet whether its mob
    /// is stopped: read the store, tell the record
    /// ([`HostedStores::note_stop_at`]) and ask again.
    StopUnread,
}

/// What this state knows of who created a mob that is not served now: it
/// has no actor, or its idle unload is admitted and not answered yet.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum UnservedCreation {
    /// The creation record was read: at the mob's create, by a load, by the
    /// sweep over the dormant stores, or by a read of this one store.
    Read(meerkat_mob::MobProvenance),
    /// The store is dormant and was not read: its file.
    Unread(PathBuf),
    /// Nothing was read and nothing may be opened for it now: no such mob
    /// that is not served, a store whose load is in flight, whose unload is
    /// admitted or that is quarantined, or the declared store.
    NotRead,
}

/// How registering a mob built outside a load ended.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Registration {
    /// The mob is managed.
    Registered,
    /// The record under the id refused: its unload is admitted. Wait for the
    /// unload to end and register again.
    UnloadInFlight,
    /// The record under the id refused: the id is its launch's, and only
    /// the launch's own create joins under it.
    LaunchOwned,
    /// A record under the id refused: the set hosts a store under the id
    /// whose mob it runs itself (it loaded the store, or created the mob),
    /// is loading, holds before activation, or whose store is quarantined,
    /// or a create under the id is in flight, and the id is that store's
    /// or that create's. Nothing is registered:
    /// the caller answers what a create under the id is answered.
    AlreadyHosted,
}

/// How a create's request for its mob's id ended.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum CreateReservation {
    /// The id is the create's: build the mob, holding the token until the
    /// create is resolved ([`HostedStores::resolve_created`],
    /// [`HostedStores::resolve_create_failed`]).
    Reserved(Arc<AttemptToken>),
    /// A create under the id is in flight: the id is that create's until it
    /// is resolved.
    InFlight,
    /// A record names the id: a mob the set knows, loaded or not, a held
    /// child, a quarantined store, or the declared store when it holds a
    /// mob.
    Taken,
    /// The id is its launch's, and this create is not the launch's own.
    LaunchOwned,
    /// A destroyed mob's storage cleanup is still owed under the id.
    CleanupOwed,
}

/// How registering a resumed mob as managed was refused.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum ManagedRefused {
    /// A destroyed mob's storage cleanup is still owed under the id.
    CleanupOwed,
    /// Another store's mob is managed under the id.
    Duplicate,
    /// The store's record admitted no load, re-attempt or release that this
    /// mob could resolve.
    NotAdmitted,
}

/// The set a mob id is in, by the first of managed, unloading, held,
/// quarantined and unloaded that one of its stores' records names.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum Hosted<'a> {
    Managed(&'a HostedStore),
    /// Its idle unload is admitted and not answered yet: a lookup waits for
    /// the answer and reads again.
    Unloading,
    Held(&'a HostedStore),
    Quarantined(&'a HostedStore),
    /// Known and dormant.
    Dormant,
    /// Known, its one load in flight.
    Loading,
    /// No live record names the id.
    Absent,
}

/// Where the journal of a hosted mob is read without loading the mob
/// ([`HostedStores::journal_source`]).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum JournalSource<'a> {
    /// The mob is managed: its own handle answers.
    Loaded(&'a MobHandle),
    /// The mob is known and no loaded handle serves it (it is not loaded,
    /// its one load or its idle unload is in flight, or it is held before
    /// activation): the file of its store.
    Stored(&'a Path),
    /// The store is quarantined: the answer every lookup of it gets.
    Quarantined(MobError),
    /// No set holds the mob, or its record names no store file.
    Absent,
}

/// Where the creation record of a hosted mob is read without loading the mob
/// ([`HostedStores::creation_source`]).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) enum CreationSource<'a> {
    /// The mob is managed: its own handle reads the record.
    Served(&'a MobHandle),
    /// The mob is known and not served (it is not loaded, or its one load
    /// or its idle unload is in flight): the file of its store.
    Stored(&'a Path),
    /// Nothing here proves a creator: the mob is held before activation or
    /// quarantined, a create of the id is in flight, no set holds the id,
    /// or its record names no store file.
    Closed,
}

/// The records, behind the hosted set's lock.
pub(crate) struct HostedStores {
    by_id: BTreeMap<MobId, Vec<HostedStore>>,
    /// The state of the set as a whole, shared with [`HostedSet`]: the step
    /// that registers a host's own handle tells it so.
    set: Arc<std::sync::Mutex<SetFacts>>,
    /// Where the declared record was filed ([`Self::declare`]): the id the
    /// host declared and the place of its store (`None`: a mob in memory).
    /// It tells which record is the declared one without reading what a
    /// record holds; what that record admits and refuses is the record's.
    declared: Option<(MobId, Option<PathBuf>)>,
    /// Bumped whenever the managed handle set gains or loses an entry.
    managed_epoch: tokio::sync::watch::Sender<u64>,
    /// Bumped on every record transition and when an attempt ends.
    changes: tokio::sync::watch::Sender<u64>,
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl HostedStores {
    fn note_changed(&self) {
        self.changes
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    fn note_managed_changed(&self) {
        self.managed_epoch
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        self.note_changed();
    }

    fn stores(&self) -> impl Iterator<Item = &HostedStore> {
        self.by_id.values().flatten()
    }

    fn stores_of(&self, mob_id: &MobId) -> impl Iterator<Item = &HostedStore> {
        self.by_id.get(mob_id).into_iter().flatten()
    }

    fn stores_of_mut(&mut self, mob_id: &MobId) -> impl Iterator<Item = &mut HostedStore> {
        self.by_id.get_mut(mob_id).into_iter().flatten()
    }

    /// The live (not Gone) record of the store at `path`.
    fn live_at_mut(&mut self, path: &Path) -> Option<&mut HostedStore> {
        self.by_id
            .values_mut()
            .flatten()
            .find(|store| store.phase() != Phase::Gone && store.path.as_deref() == Some(path))
    }

    /// File `store` last under its mob id.
    fn push(&mut self, store: HostedStore) {
        self.by_id
            .entry(store.mob_id.clone())
            .or_default()
            .push(store);
    }

    /// Drop every record `keep` refuses, and every id left without one.
    fn retain(&mut self, mut keep: impl FnMut(&HostedStore) -> bool) {
        self.by_id.retain(|_, stores| {
            stores.retain(|store| keep(store));
            !stores.is_empty()
        });
    }

    /// File the live record of the store at `path` under `mob_id`. A store
    /// names its mob itself once it is read, and a record listed under its
    /// file's stem (a name that encodes no id) takes that id here. A record
    /// listed under the id its file name encodes is never filed under
    /// another: the restore refuses a store whose mob is not the one its
    /// file is named for before anything of it is resumed, held or torn
    /// down, and so does it refuse a stem-listed store whose mob another
    /// store file is named for. For those records `mob_id` is the id they
    /// already have, or (a name an earlier release gave the store) the id
    /// the whole stem encodes.
    fn rename_at(&mut self, path: &Path, mob_id: &MobId) {
        let Some(from) = self
            .by_id
            .iter()
            .find(|(_, stores)| {
                stores.iter().any(|store| {
                    store.phase() != Phase::Gone && store.path.as_deref() == Some(path)
                })
            })
            .map(|(id, _)| id.clone())
        else {
            return;
        };
        if &from == mob_id {
            return;
        }
        let Some(stores) = self.by_id.get_mut(&from) else {
            return;
        };
        let Some(index) = stores
            .iter()
            .position(|store| store.phase() != Phase::Gone && store.path.as_deref() == Some(path))
        else {
            return;
        };
        let mut store = stores.remove(index);
        if stores.is_empty() {
            self.by_id.remove(&from);
        }
        store.mob_id = mob_id.clone();
        self.push(store);
    }

    // ------------------------------------------------------------------
    // Views
    // ------------------------------------------------------------------

    /// Whether the mob `mob_id` serves here now or will at its next lookup:
    /// it is managed, known and not loaded, or its admitted unload is in
    /// flight.
    pub(crate) fn managed_or_unloaded(&self, mob_id: &MobId) -> bool {
        self.stores_of(mob_id)
            .any(|store| store.phase() == Phase::Managed || store.is_awaiting_service())
    }

    /// Whether the mob `mob_id` is known and not served now, and a lookup of
    /// it serves it again: it is not loaded, or its admitted unload is in
    /// flight.
    pub(crate) fn awaits_service(&self, mob_id: &MobId) -> bool {
        self.stores_of(mob_id).any(HostedStore::is_awaiting_service)
    }

    /// The set `mob_id` is in.
    pub(crate) fn hosted(&self, mob_id: &MobId) -> Hosted<'_> {
        let mut answer = Hosted::Absent;
        let mut rank = 0;
        for store in self.stores_of(mob_id) {
            let (candidate, candidate_rank) = match store.phase() {
                Phase::Managed => (Hosted::Managed(store), 6),
                Phase::Unloading => (Hosted::Unloading, 5),
                Phase::Held => (Hosted::Held(store), 4),
                Phase::Quarantined | Phase::Retrying => (Hosted::Quarantined(store), 3),
                Phase::Loading => (Hosted::Loading, 2),
                // A declared store that holds no mob, once nothing waits
                // for its launch, has nothing to serve or load: its id
                // reads as no mob's until the launch creates one.
                Phase::Dormant if store.holds_nothing_to_serve() => continue,
                Phase::Dormant => (Hosted::Dormant, 1),
                // A create in flight serves nothing yet: its mob is
                // not there to be found.
                Phase::Gone | Phase::Creating => continue,
            };
            if candidate_rank > rank {
                answer = candidate;
                rank = candidate_rank;
            }
        }
        answer
    }

    /// Where the journal of `mob_id` is read without loading the mob: the
    /// set the id is in ([`Self::hosted`]) decides, so a mob id two stores
    /// name is answered from the same record a lookup is. Only a record's
    /// phase, handle and path are read; no input is applied.
    pub(crate) fn journal_source(&self, mob_id: &MobId) -> JournalSource<'_> {
        fn stored(store: Option<&HostedStore>) -> JournalSource<'_> {
            store
                .and_then(HostedStore::storage_path)
                .map_or(JournalSource::Absent, JournalSource::Stored)
        }
        let in_phase = |phase: Phase| {
            self.stores_of(mob_id).find(move |store| {
                store.phase() == phase
                    && !(phase == Phase::Dormant && store.holds_nothing_to_serve())
            })
        };
        match self.hosted(mob_id) {
            Hosted::Managed(store) => store
                .handle()
                .map_or(JournalSource::Absent, JournalSource::Loaded),
            Hosted::Held(store) => stored(Some(store)),
            Hosted::Quarantined(store) => JournalSource::Quarantined(store.quarantine_error()),
            Hosted::Unloading => stored(in_phase(Phase::Unloading)),
            Hosted::Loading => stored(in_phase(Phase::Loading)),
            Hosted::Dormant => stored(in_phase(Phase::Dormant)),
            Hosted::Absent => JournalSource::Absent,
        }
    }

    /// Where the creation record of `mob_id` is read without loading the
    /// mob: the set the id is in ([`Self::hosted`]) decides, so a mob id two
    /// stores name is answered from the same record a lookup is. Only a
    /// record's phase, handle and path are read; no input is applied.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn creation_source(&self, mob_id: &MobId) -> CreationSource<'_> {
        let stored_in = |phase: Phase| {
            self.stores_of(mob_id)
                .find(move |store| {
                    store.phase() == phase
                        && !(phase == Phase::Dormant && store.holds_nothing_to_serve())
                })
                .and_then(HostedStore::storage_path)
                .map_or(CreationSource::Closed, CreationSource::Stored)
        };
        match self.hosted(mob_id) {
            Hosted::Managed(store) => store
                .handle()
                .map_or(CreationSource::Closed, CreationSource::Served),
            Hosted::Unloading => stored_in(Phase::Unloading),
            Hosted::Loading => stored_in(Phase::Loading),
            Hosted::Dormant => stored_in(Phase::Dormant),
            Hosted::Held(_) | Hosted::Quarantined(_) | Hosted::Absent => CreationSource::Closed,
        }
    }

    /// The record of the managed mob `mob_id`.
    pub(crate) fn managed(&self, mob_id: &MobId) -> Option<&HostedStore> {
        self.stores_of(mob_id)
            .find(|store| store.phase() == Phase::Managed)
    }

    /// The declared record: the store of the mob the host's launch names.
    /// It lasts from the declaration to the end of the process.
    fn declared_record(&self) -> Option<&HostedStore> {
        self.stores().find(|store| self.is_the_declared(store))
    }

    /// Whether `store` is the record filed at the declaration: the live
    /// record under the declared id at the declared place. Told by where
    /// it is filed, not by anything the record holds.
    fn is_the_declared(&self, store: &HostedStore) -> bool {
        self.declared.as_ref().is_some_and(|(mob_id, place)| {
            &store.mob_id == mob_id && &store.path == place && store.phase() != Phase::Gone
        })
    }

    /// Whether `mob_id` is the id the declared record was filed under.
    /// Only the launch's serve creates under it, no handle a host inserts
    /// takes it, and its mob is not destroyed through the set: the
    /// declared record refuses each of them when it is asked. This is the
    /// read that lets a door refuse before it does anything else.
    pub(crate) fn launch_owns(&self, mob_id: &MobId) -> bool {
        self.declared
            .as_ref()
            .is_some_and(|(declared, _)| declared == mob_id)
    }

    /// The mob that still waits for its launch's first serve, if any.
    pub(crate) fn awaiting_launch(&self) -> Option<&MobId> {
        self.declared_record()
            .filter(|store| store.awaits_launch())
            .map(HostedStore::mob_id)
    }

    /// Whether the mob `mob_id` still waits for its launch's first serve.
    pub(crate) fn awaits_launch(&self, mob_id: &MobId) -> bool {
        self.awaiting_launch() == Some(mob_id)
    }

    /// Whether `mob_id` is its launch's and its store is known to hold no
    /// mob: the launch's serve creates the mob, and loads nothing.
    pub(crate) fn declared_vacant(&self, mob_id: &MobId) -> bool {
        self.declared_record().is_some_and(|store| {
            &store.mob_id == mob_id
                && store.phase() == Phase::Dormant
                && store.record.state().vacant
        })
    }

    /// Whether the mob `mob_id` is managed: for the rows of the hosted set.
    #[cfg(test)]
    pub(crate) fn is_managed(&self, mob_id: &MobId) -> bool {
        self.managed(mob_id).is_some()
    }

    /// Whether an actor of a mob runs under `mob_id`: the mob is managed, or
    /// its admitted unload is in flight. The id and the store are in use
    /// either way.
    pub(crate) fn is_loaded(&self, mob_id: &MobId) -> bool {
        self.stores_of(mob_id)
            .any(|store| matches!(store.phase(), Phase::Managed | Phase::Unloading))
    }

    /// The handle of every mob whose actor this state runs, served or being
    /// unloaded, for the stand-down that closes each actor's admission. Not
    /// a view to hand a mob out by.
    pub(crate) fn loaded_handles(&self) -> impl Iterator<Item = (&MobId, &MobHandle)> {
        self.stores()
            .filter_map(|store| store.loaded_handle().map(|handle| (&store.mob_id, handle)))
    }

    /// Every mob whose actor this state runs, with its record, one per id,
    /// in id order: the managed mobs, and the mobs whose admitted unload is
    /// not answered yet. A mob is in this view until its unload closed it.
    /// It is the view of the readers that hand out no handle (the list, the
    /// inventory, a read of a mob's own facts); a handle is handed out only
    /// from [`Self::managed_stores`].
    pub(crate) fn loaded_stores(&self) -> impl Iterator<Item = (&MobId, &HostedStore)> {
        self.by_id.iter().filter_map(|(mob_id, stores)| {
            stores
                .iter()
                .find(|store| store.phase() == Phase::Managed)
                .or_else(|| {
                    stores
                        .iter()
                        .find(|store| store.phase() == Phase::Unloading)
                })
                .map(|store| (mob_id, store))
        })
    }

    /// Every managed mob with its record, one per id, in id order.
    pub(crate) fn managed_stores(&self) -> impl Iterator<Item = (&MobId, &HostedStore)> {
        self.by_id.iter().filter_map(|(mob_id, stores)| {
            stores
                .iter()
                .find(|store| store.phase() == Phase::Managed)
                .map(|store| (mob_id, store))
        })
    }

    /// Every managed mob with its handle, one per id, in id order.
    pub(crate) fn managed_handles(&self) -> impl Iterator<Item = (&MobId, &MobHandle)> {
        self.managed_stores()
            .filter_map(|(mob_id, store)| store.handle().map(|handle| (mob_id, handle)))
    }

    /// The hold of the held child mob `mob_id`.
    pub(crate) fn held(&self, mob_id: &MobId) -> Option<&HeldChildMob> {
        self.stores_of(mob_id).find_map(HostedStore::held)
    }

    /// Every held child mob's hold, in id order.
    pub(crate) fn held_mobs(&self) -> impl Iterator<Item = &HeldChildMob> {
        self.stores().filter_map(HostedStore::held)
    }

    /// The record `mob_id` is quarantined under.
    pub(crate) fn quarantined(&self, mob_id: &MobId) -> Option<&HostedStore> {
        self.stores_of(mob_id).find(|store| store.is_quarantined())
    }

    /// Every quarantined store's record, in id order.
    pub(crate) fn quarantined_stores(&self) -> impl Iterator<Item = &HostedStore> {
        self.stores().filter(|store| store.is_quarantined())
    }

    /// The ids of the mobs that are known and not served now (not loaded,
    /// or their admitted unload in flight), in id order. A lookup of each
    /// serves it again. An id that is served is not among them, whatever
    /// else is recorded under it: a lookup of that id is answered by the
    /// mob that serves it and loads nothing. (A census files no record for
    /// a store whose id a running mob holds, so a host's handle inserted
    /// before the census has no record of the root's store beside it.)
    pub(crate) fn unloaded_ids(&self) -> Vec<MobId> {
        self.by_id
            .iter()
            .filter(|(_, stores)| {
                stores.iter().any(HostedStore::is_awaiting_service)
                    && !stores.iter().any(|store| store.phase() == Phase::Managed)
            })
            .map(|(mob_id, _)| mob_id.clone())
            .collect()
    }

    /// The ids of the mobs that are known and not loaded (dormant, or their
    /// one load in flight), in id order: no actor runs for them. An id an
    /// actor runs under (it is managed, or its admitted unload is in
    /// flight) is not among them, whatever else is recorded under it.
    pub(crate) fn not_loaded_ids(&self) -> Vec<MobId> {
        self.by_id
            .iter()
            .filter(|(_, stores)| {
                stores.iter().any(HostedStore::is_unloaded)
                    && !stores
                        .iter()
                        .any(|store| matches!(store.phase(), Phase::Managed | Phase::Unloading))
            })
            .map(|(mob_id, _)| mob_id.clone())
            .collect()
    }

    /// Keep whether the mob of the dormant store at `path` is stopped, as
    /// the set read it from the store. The record takes the read only
    /// while it is dormant and does not know the answer: what the mob
    /// itself answered at an unload is newer than any read of its store,
    /// and is never overwritten. `true` when the record took it; `false`
    /// tells the caller to read the set again.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn note_stop_at(&mut self, path: &Path, stopped: bool) -> bool {
        let Some(store) = self.live_at_mut(path) else {
            return false;
        };
        let taken = store
            .apply(Input::ReadStop { stopped })
            .iter()
            .any(|effect| matches!(effect, Effect::StopRead { .. }));
        if taken {
            self.note_changed();
        }
        taken
    }

    /// The ids of the mobs that are not loaded and stopped, in id order:
    /// each lists as stopped, and no demand loads it.
    pub(crate) fn stopped_unloaded_ids(&self) -> Vec<MobId> {
        self.by_id
            .iter()
            .filter(|(_, stores)| stores.iter().any(|store| store.stop() == Some(true)))
            .map(|(mob_id, _)| mob_id.clone())
            .collect()
    }

    /// Whether the mob under `mob_id`, which is not loaded, is stopped by
    /// its record: [`Self::stopped_unloaded_ids`] for one id. A record that
    /// has not read the store's stop says nothing, and nothing is read or
    /// loaded here.
    pub(crate) fn stopped_unloaded(&self, mob_id: &MobId) -> bool {
        self.stores_of(mob_id)
            .any(|store| store.stop() == Some(true))
    }

    /// Keep what the creation record of the mob of the live record at
    /// `path` says: who created the mob and the mob's own incarnation
    /// ([`HostedStore`]'s `creation` and `incarnation`). Both are given in
    /// the one call, so that a place that reads one cannot forget the
    /// other; what was not read (`None`) is left as it was. Nothing an
    /// observer of the set waits on changes by it.
    pub(crate) fn note_creation_at(
        &mut self,
        path: &Path,
        creation: Option<meerkat_mob::MobProvenance>,
        incarnation: Option<meerkat_core::service::MobIncarnationId>,
    ) {
        if let Some(store) = self
            .by_id
            .values_mut()
            .flatten()
            .find(|store| store.phase() != Phase::Gone && store.path.as_deref() == Some(path))
        {
            if creation.is_some() {
                store.creation = creation;
            }
            if incarnation.is_some() {
                store.incarnation = incarnation;
            }
        }
    }

    /// Keep who created the mob managed under `mob_id`, for a mob with no
    /// store file to find its record by ([`Self::note_creation_at`]): a mob
    /// this state created in memory. Nothing an observer of the set waits
    /// on changes by it.
    pub(crate) fn note_creation_of_managed(
        &mut self,
        mob_id: &MobId,
        creation: meerkat_mob::MobProvenance,
    ) {
        if let Some(store) = self
            .stores_of_mut(mob_id)
            .find(|store| store.phase() == Phase::Managed)
        {
            store.creation = Some(creation);
        }
    }

    /// The own incarnation of the mob that is known and not loaded under
    /// `mob_id`, when this state has read it.
    pub(crate) fn incarnation_of_unloaded(
        &self,
        mob_id: &MobId,
    ) -> Option<meerkat_core::service::MobIncarnationId> {
        self.stores_of(mob_id)
            .find(|store| store.is_unloaded())
            .and_then(|store| store.incarnation)
    }

    /// Who created the mob that is known and not loaded under `mob_id`,
    /// when this state has read it.
    pub(crate) fn creation_of_unloaded(
        &self,
        mob_id: &MobId,
    ) -> Option<meerkat_mob::MobProvenance> {
        self.stores_of(mob_id)
            .find(|store| store.is_unloaded())
            .and_then(|store| store.creation.clone())
    }

    /// Who created the mob that is known under `mob_id` and is not served
    /// now (not loaded, its admitted unload not answered yet, or its store
    /// quarantined), as far as this state has read it; and for a dormant
    /// store it has not read, the file a read of its creation record opens.
    /// A record whose unload is admitted answers only from what it keeps:
    /// its store is not opened while the mob's last actor may still hold
    /// it.
    pub(crate) fn creation_of_unserved(&self, mob_id: &MobId) -> UnservedCreation {
        let unserved = self.stores_of(mob_id).find(|store| {
            store.is_awaiting_service()
                || matches!(store.phase(), Phase::Quarantined | Phase::Retrying)
        });
        let Some(store) = unserved else {
            return UnservedCreation::NotRead;
        };
        match (&store.creation, store.phase(), &store.path) {
            (Some(creation), _, _) => UnservedCreation::Read(creation.clone()),
            (None, Phase::Dormant, Some(path)) if !self.is_the_declared(store) => {
                UnservedCreation::Unread(path.clone())
            }
            _ => UnservedCreation::NotRead,
        }
    }

    /// The stores a pass of the state may load on its own: the dormant ones,
    /// with the id each is filed under. The declared store is not among
    /// them: its record refuses a pass's load (it is its launch's), and a
    /// pass is not offered a store whose load it would be refused, so it
    /// neither reads nor loads it.
    pub(crate) fn dormant_stores(&self) -> Vec<(MobId, PathBuf)> {
        self.stores()
            .filter(|store| store.phase() == Phase::Dormant && !self.is_the_declared(store))
            .filter_map(|store| store.path.clone().map(|path| (store.mob_id.clone(), path)))
            .collect()
    }

    /// The file of the store that is known and dormant under `mob_id`, for
    /// a read of that one store without a load. `None` for the declared
    /// store (its launch serves it) and for a record in any other phase: a
    /// store that is loaded, loading, unloading, quarantined, held or gone
    /// is not read so.
    pub(crate) fn dormant_store_of(&self, mob_id: &MobId) -> Option<PathBuf> {
        self.stores_of(mob_id)
            .find(|store| store.phase() == Phase::Dormant && !self.is_the_declared(store))
            .and_then(|store| store.path.clone())
    }

    /// Whether the store at `path` is dormant now.
    pub(crate) fn is_dormant_at(&self, path: &Path) -> bool {
        self.stores()
            .any(|store| store.phase() == Phase::Dormant && store.path.as_deref() == Some(path))
    }

    /// The mobs an idle unload pass turns to: each has been managed for at
    /// least `loaded_for`, or was left unloading by an unload that ended
    /// without an answer. Whether a mob may be unloaded at all is not read
    /// here: its record decides when it is asked
    /// ([`Self::request_unload`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn unload_candidates(&self, loaded_for: std::time::Duration) -> Vec<MobId> {
        self.stores()
            .filter(|store| match store.phase() {
                Phase::Managed => store.managed_for(loaded_for),
                Phase::Unloading => !store.attempt_runs(),
                _ => false,
            })
            .map(|store| store.mob_id.clone())
            .collect()
    }

    /// Whether a load of a store of `mob_id` is in flight: its record is
    /// Loading and the attempt it admitted still runs.
    pub(crate) fn load_in_flight(&self, mob_id: &MobId) -> bool {
        self.stores_of(mob_id)
            .any(|store| store.phase() == Phase::Loading && store.attempt_runs())
    }

    /// Whether a restore pass waits for the load or re-attempt of the store
    /// at `path`: one is in flight (its record is Loading or Retrying and
    /// the attempt it admitted still runs) and was not left to run. An
    /// attempt left to run is the one pending task of its store: a pass
    /// that meets it starts no second one and does not wait for it.
    pub(crate) fn pass_awaits_attempt_at(&self, path: &Path) -> bool {
        self.stores().any(|store| {
            store.path.as_deref() == Some(path)
                && matches!(store.phase(), Phase::Loading | Phase::Retrying)
                && store
                    .attempt
                    .upgrade()
                    .is_some_and(|attempt| !attempt.is_left_to_run())
        })
    }

    /// Whether an unload of the mob `mob_id` is in flight: its record is
    /// Unloading and the attempt it admitted still runs.
    pub(crate) fn unload_in_flight(&self, mob_id: &MobId) -> bool {
        self.stores_of(mob_id)
            .any(|store| store.phase() == Phase::Unloading && store.attempt_runs())
    }

    /// Whether an unload of the mob `mob_id` was left without an answer: its
    /// record is Unloading and nothing runs for it.
    pub(crate) fn unload_left_unanswered(&self, mob_id: &MobId) -> bool {
        self.stores_of(mob_id)
            .any(|store| store.phase() == Phase::Unloading && !store.attempt_runs())
    }

    /// Whether a live record names `mob_id`: the id is taken, whether or not
    /// its mob is loaded.
    pub(crate) fn knows(&self, mob_id: &MobId) -> bool {
        self.stores_of(mob_id)
            .any(|store| store.phase() != Phase::Gone)
    }

    /// The destroy this state ran under `mob_id`.
    pub(crate) fn retired(&self, mob_id: &MobId) -> Option<&Arc<RetiredMob>> {
        self.stores_of(mob_id).find_map(HostedStore::retired)
    }

    /// Whether this state destroyed a mob under `mob_id`.
    pub(crate) fn destroyed_here(&self, mob_id: &MobId) -> bool {
        self.retired(mob_id).is_some()
    }

    /// Whether this state still owes the storage cleanup of a mob it
    /// destroyed under `mob_id`.
    pub(crate) fn destroy_cleanup_owed(&self, mob_id: &MobId) -> bool {
        self.retired(mob_id)
            .is_some_and(|retired| retired.cleanup_owed())
    }

    /// Whether this state still owes the storage cleanup of a mob it
    /// destroyed whose database is at `path`.
    pub(crate) fn destroy_cleanup_owed_at(&self, path: &Path) -> bool {
        self.stores()
            .filter_map(HostedStore::retired)
            .any(|retired| {
                retired
                    .storage
                    .as_ref()
                    .is_some_and(|storage| storage.path == path)
                    && retired.cleanup_owed()
            })
    }

    // ------------------------------------------------------------------
    // Transitions
    // ------------------------------------------------------------------

    /// The host declares the store of the mob its launch names: its record
    /// is filed now, whether or not a file is at `path`, and says that the
    /// mob waits for its launch's first serve. `path` is `None` for a mob
    /// that lives in memory: its store is known to hold no mob until the
    /// launch creates it. In process only, like every record: a new process
    /// declares the store again.
    pub(crate) fn declare(&mut self, mob_id: &MobId, path: Option<PathBuf>) {
        let vacant = path.is_none();
        self.declared = Some((mob_id.clone(), path.clone()));
        let mut store = HostedStore::new(mob_id.clone(), path);
        store.apply(Input::Declare { vacant });
        self.push(store);
        self.note_changed();
    }

    /// The wait for the launch ended: its first serve ended, however it
    /// ended, or the state stands down. The record decides whether there
    /// was a wait to end; when there was, everything that waits on the set
    /// is woken, the waiters on the managed mobs included (the serve may
    /// have ended without a mob, and that is news for them as well).
    pub(crate) fn end_launch_wait(&mut self) {
        let Some((mob_id, place)) = self.declared.clone() else {
            return;
        };
        let Some(store) = self
            .stores_of_mut(&mob_id)
            .find(|store| store.path == place && store.phase() != Phase::Gone)
        else {
            return;
        };
        let ended = store
            .apply(Input::EndLaunchWait)
            .iter()
            .any(|effect| matches!(effect, Effect::LaunchWaitEnded));
        if ended {
            self.note_managed_changed();
        }
    }

    /// A census pass listed the store file at `path`, whose name encodes
    /// `mob_id`. `true` when the store is dormant after it: a store the set
    /// did not know, a dormant one, or a quarantined one a new pass finds
    /// afresh. `false` when the set already loads, manages, holds or
    /// re-attempts it, owes the cleanup of the destroyed mob it held, or
    /// when a mob that runs here holds its id already: that mob keeps the
    /// id for this process, and no record is filed for the store.
    pub(crate) fn list(&mut self, path: &Path, mob_id: &MobId) -> bool {
        if let Some(store) = self.live_at_mut(path) {
            // A load or re-attempt that ended without an outcome (its future
            // was dropped, or it panicked) is resolved as failed first: the
            // store is then quarantined, and this pass finds it afresh.
            if store.attempt.upgrade().is_none() {
                let class = meerkat_mob::MobQuarantineClass::StoreUnavailable;
                match store.phase() {
                    Phase::Loading => {
                        store.apply(Input::ResolveLoad {
                            outcome: Outcome::Failed,
                            class,
                        });
                    }
                    Phase::Retrying => {
                        store.apply(Input::ResolveRetry {
                            outcome: Outcome::Failed,
                            class,
                        });
                    }
                    _ => {}
                }
            }
            let listed = store
                .apply(Input::Census)
                .iter()
                .any(|effect| matches!(effect, Effect::StoreListed));
            if listed {
                store.cause = None;
                store.attempt = Weak::new();
                self.note_changed();
            }
            return listed;
        }
        if self.destroy_cleanup_owed_at(path) {
            return false;
        }
        // The store of a create that is in flight: the create wrote its
        // file and has not been resolved yet. Its record is asked, and it
        // answers that the set hosts it: the store is the create's, and no
        // second record is filed for it.
        self.release_abandoned_creates(mob_id);
        if let Some(creating) = self
            .stores_of_mut(mob_id)
            .find(|store| store.phase() == Phase::Creating)
            && HostedStore::refused(&creating.apply(Input::Census), Refusal::AlreadyHosted)
        {
            return false;
        }
        // The id may be held by a mob that runs here already: a handle a
        // host registered before this census, or the mob of another store.
        // The first holder keeps the id. Its record is asked, and it
        // answers that the set hosts it: no second record is filed under
        // the id, so the store at `path` is never loaded beside that mob.
        // The store is left exactly as it was found, and the census of the
        // next process lists it again.
        if let Some(holder) = self
            .stores_of_mut(mob_id)
            .find(|store| store.loaded_handle().is_some())
            && HostedStore::refused(&holder.apply(Input::Census), Refusal::AlreadyHosted)
        {
            tracing::warn!(
                mob_id = %mob_id,
                store = %path.display(),
                "a mob that runs here holds the id of a store under the persistent root: \
                 the store is left as found and is not loaded in this process"
            );
            return false;
        }
        let mut store = HostedStore::new(mob_id.clone(), Some(path.to_path_buf()));
        let listed = store
            .apply(Input::Census)
            .iter()
            .any(|effect| matches!(effect, Effect::StoreListed));
        self.push(store);
        self.note_changed();
        listed
    }

    /// Ask for the one load of the dormant store at `path`, for a pass of
    /// the state that loads stores on its own.
    pub(crate) fn admit_load_at(&mut self, path: &Path) -> LoadAdmission {
        let changes = self.changes.clone();
        let Some(store) = self.live_at_mut(path) else {
            return LoadAdmission::NotDormant;
        };
        let admission = Self::admit_load_of(store, changes, dsl::MobStoreLoadTrigger::Pass);
        self.note_changed();
        admission
    }

    /// Ask for the one load of a dormant store of `mob_id`, naming what
    /// asks: something that addresses the mob, or the launch's own serve.
    pub(crate) fn admit_load(
        &mut self,
        mob_id: &MobId,
        trigger: dsl::MobStoreLoadTrigger,
    ) -> LoadAdmission {
        let changes = self.changes.clone();
        let Some(store) = self.stores_of_mut(mob_id).find(|store| store.is_unloaded()) else {
            return LoadAdmission::NotDormant;
        };
        let admission = Self::admit_load_of(store, changes, trigger);
        self.note_changed();
        admission
    }

    /// [`Self::admit_load`] for something that addresses the mob: for the
    /// rows of the hosted set.
    #[cfg(test)]
    pub(crate) fn admit_load_as_a_lookup(&mut self, mob_id: &MobId) -> LoadAdmission {
        self.admit_load(mob_id, dsl::MobStoreLoadTrigger::Lookup)
    }

    /// [`Self::admit_load`] for something that originates work for the mob
    /// (a send, a schedule fire, a delivery, an announcement): the record
    /// admits the load of a mob it knows not to be stopped, answers
    /// [`LoadAdmission::Stopped`] for a stopped one and
    /// [`LoadAdmission::StopUnread`] while it does not know. A read of the
    /// mob and an operator's verb ask as a lookup and load a stopped mob
    /// too.
    // No caller yet outside the rows: the demand form of the lookup lands
    // on this name. Remove the attribute with the first caller.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn admit_load_as_a_demand(&mut self, mob_id: &MobId) -> LoadAdmission {
        self.admit_load(mob_id, dsl::MobStoreLoadTrigger::Demand)
    }

    fn admit_load_of(
        store: &mut HostedStore,
        changes: tokio::sync::watch::Sender<u64>,
        trigger: dsl::MobStoreLoadTrigger,
    ) -> LoadAdmission {
        // A load that ended without an outcome (its future was dropped, or
        // it panicked) is resolved as failed: the store could not be served
        // then, and an on-demand re-attempt may pass. Whether a load was in
        // flight is the record's fact: a record with no live attempt is
        // given the resolution, and refuses it in every phase but Loading.
        if store.attempt.upgrade().is_none() {
            let abandoned = store
                .apply(Input::ResolveLoad {
                    outcome: Outcome::Failed,
                    class: meerkat_mob::MobQuarantineClass::StoreUnavailable,
                })
                .iter()
                .any(|effect| matches!(effect, Effect::StoreQuarantined { .. }));
            if abandoned {
                store.cause = Some(Arc::new(MobError::Internal(
                    "the load of this mob store ended without an outcome".to_string(),
                )));
                return LoadAdmission::NotDormant;
            }
        }
        let Some(path) = store.path.clone() else {
            // No file to load from: a declared mob that lives in memory.
            // The launch has nothing to load (it creates the mob), so its
            // record is not asked for a load that could not run. Every
            // other request is the record's to answer, as for a store
            // with a file: it refuses a lookup while the launch is
            // awaited, and a pass always.
            if trigger == dsl::MobStoreLoadTrigger::Launch {
                return LoadAdmission::NotDormant;
            }
            let effects = store.apply(Input::RequestLoad { trigger });
            if effects
                .iter()
                .any(|effect| matches!(effect, Effect::LoadAdmitted))
            {
                // The record admitted a load of a store with no file:
                // there is nothing to read, so the load ends at once as
                // one that found no mob.
                store.apply(Input::ResolveLoad {
                    outcome: Outcome::Settled,
                    class: meerkat_mob::MobQuarantineClass::StoreUnavailable,
                });
                return LoadAdmission::NotDormant;
            }
            return if HostedStore::refused(&effects, Refusal::AwaitsLaunch) {
                LoadAdmission::AwaitsLaunch
            } else {
                LoadAdmission::NotDormant
            };
        };
        let effects = store.apply(Input::RequestLoad { trigger });
        if effects
            .iter()
            .any(|effect| matches!(effect, Effect::LoadAdmitted))
        {
            let attempt = AttemptToken::new(changes);
            store.attempt = Arc::downgrade(&attempt);
            return LoadAdmission::Admitted { path, attempt };
        }
        if HostedStore::refused(&effects, Refusal::LoadInFlight) {
            return LoadAdmission::InFlight;
        }
        if HostedStore::refused(&effects, Refusal::AwaitsLaunch) {
            return LoadAdmission::AwaitsLaunch;
        }
        if HostedStore::refused(&effects, Refusal::Stopped) {
            return LoadAdmission::Stopped;
        }
        if HostedStore::refused(&effects, Refusal::StopUnread) {
            return LoadAdmission::StopUnread;
        }
        LoadAdmission::NotDormant
    }

    /// Admit one re-attempt of the restore of the store `mob_id` is
    /// quarantined under: its record moves to Retrying. `Ok((path, attempt))`
    /// when the record admitted it; hold `attempt` until the record is
    /// resolved. Otherwise the answer the caller gets (the quarantine, typed,
    /// while another re-attempt is in flight or for an on-demand re-attempt
    /// of a store that needs a repair), or `None` when no store of `mob_id`
    /// is quarantined. A Retrying record whose attempt's token is gone was
    /// left by an attempt that ended without resolving it: that attempt is
    /// resolved as failed, under the class the record holds, and a new one
    /// may be admitted.
    pub(crate) fn admit_retry(
        &mut self,
        mob_id: &MobId,
        trigger: dsl::MobStoreRetryTrigger,
    ) -> Option<Result<(PathBuf, Arc<AttemptToken>), MobError>> {
        let changes = self.changes.clone();
        let store = self
            .stores_of_mut(mob_id)
            .find(|store| store.is_quarantined())?;
        // A re-attempt that ended without resolving its record is resolved
        // as failed first. Whether one was in flight is the record's fact:
        // a record with no live attempt is given the resolution, and refuses
        // it in every phase but Retrying.
        if store.attempt.upgrade().is_none() {
            let class = store.record.state().class;
            store.apply(Input::ResolveRetry {
                outcome: Outcome::Failed,
                class,
            });
        }
        let admitted = store
            .apply(Input::RequestRetry { trigger })
            .iter()
            .any(|effect| matches!(effect, Effect::RetryAdmitted { .. }));
        let answer = match (admitted, store.path.clone()) {
            (true, Some(path)) => {
                let attempt = AttemptToken::new(changes);
                store.attempt = Arc::downgrade(&attempt);
                Ok((path, attempt))
            }
            // Refused: a re-attempt is in flight (answered at once, never
            // waited for), or an on-demand re-attempt of a store that needs
            // a repair. The answer is the quarantine.
            _ => Err(store.quarantine_error()),
        };
        self.note_changed();
        Some(answer)
    }

    /// The mob resumed from the store at `path` is managed: its record
    /// leaves Loading, Retrying or Held in this step, so no reader finds the
    /// mob in two sets or in none. Refused, with the handle back, when a
    /// destroyed mob's storage cleanup is owed under the id, when another
    /// store's mob is managed under it, or when the store's record admitted
    /// nothing this mob could resolve.
    pub(crate) fn resolve_managed(
        &mut self,
        path: &Path,
        mob_id: &MobId,
        handle: MobHandle,
    ) -> Result<(), Box<(MobHandle, ManagedRefused)>> {
        if self.is_loaded(mob_id) {
            return Err(Box::new((handle, ManagedRefused::Duplicate)));
        }
        if self.destroy_cleanup_owed(mob_id) {
            return Err(Box::new((handle, ManagedRefused::CleanupOwed)));
        }
        let Some(store) = self.live_at_mut(path) else {
            return Err(Box::new((handle, ManagedRefused::NotAdmitted)));
        };
        let class = store.record.state().class;
        let input = match store.phase() {
            Phase::Loading => Input::ResolveLoad {
                outcome: Outcome::Managed,
                class,
            },
            Phase::Retrying => Input::ResolveRetry {
                outcome: Outcome::Managed,
                class,
            },
            Phase::Held => Input::ReleaseHeld {
                outcome: Outcome::Managed,
            },
            Phase::Dormant
            | Phase::Managed
            | Phase::Unloading
            | Phase::Quarantined
            | Phase::Gone
            | Phase::Creating => {
                return Err(Box::new((handle, ManagedRefused::NotAdmitted)));
            }
        };
        store.apply(input);
        if store.phase() != Phase::Managed {
            return Err(Box::new((handle, ManagedRefused::NotAdmitted)));
        }
        store.handle = Some(handle);
        store.held = None;
        store.cause = None;
        store.note_managed();
        // A stem-listed store takes its mob's id; any other is already
        // filed under it ([`Self::rename_at`]).
        self.rename_at(path, mob_id);
        self.note_managed_changed();
        Ok(())
    }

    /// The child mob in the store at `held.storage_path` is held before
    /// activation: its record leaves Loading or Retrying in this step. A
    /// record that is already Held keeps its phase and takes the new reason.
    pub(crate) fn resolve_held(&mut self, held: HeldChildMob) {
        let path = held.storage_path.clone();
        let mob_id = held.mob_id.clone();
        if self.live_at_mut(&path).is_none() {
            // A store first seen held (no census listed it): list it and
            // admit the load its hold resolves.
            let mut store = HostedStore::new(mob_id.clone(), Some(path.clone()));
            store.apply(Input::Census);
            store.apply(Input::RequestLoad {
                trigger: dsl::MobStoreLoadTrigger::Lookup,
            });
            self.push(store);
        }
        let Some(store) = self.live_at_mut(&path) else {
            return;
        };
        let class = store.record.state().class;
        match store.phase() {
            Phase::Loading => {
                store.apply(Input::ResolveLoad {
                    outcome: Outcome::Held,
                    class,
                });
            }
            Phase::Retrying => {
                store.apply(Input::ResolveRetry {
                    outcome: Outcome::Held,
                    class,
                });
            }
            Phase::Held => {}
            Phase::Dormant
            | Phase::Managed
            | Phase::Unloading
            | Phase::Quarantined
            | Phase::Gone
            | Phase::Creating => {
                tracing::error!(
                    mob_id = %mob_id,
                    phase = ?store.phase(),
                    "a hold met a mob store record that admitted no load"
                );
                return;
            }
        }
        store.held = Some(held);
        store.cause = None;
        // As for a managed mob: only a stem-listed store changes its id.
        self.rename_at(&path, &mob_id);
        self.note_changed();
    }

    /// The load or re-attempt admitted for the store at `path` ended as
    /// `restored`. A mob it made managed or held resolved the record in the
    /// step that moved it ([`Self::resolve_managed`], [`Self::resolve_held`]);
    /// here a store that was swept or settled is gone and its record is
    /// dropped, and a failed one is quarantined under the class it met, with
    /// its failure. The answer is the caller's: how the store restored, or
    /// the typed quarantine.
    pub(crate) fn resolve_attempt(
        &mut self,
        path: &Path,
        restored: Result<StoreRestored, StoreQuarantine>,
    ) -> Result<StoreRestored, MobError> {
        let Some(store) = self.live_at_mut(path) else {
            return restored.map_err(|quarantine| quarantine.cause);
        };
        let retrying = match store.phase() {
            Phase::Loading => false,
            Phase::Retrying => true,
            // The move into the managed or held set resolved the record.
            _ => return restored.map_err(|quarantine| quarantine.cause),
        };
        let held_class = store.record.state().class;
        let (outcome, class) = match &restored {
            Ok(StoreRestored::Managed) => (Outcome::Managed, held_class),
            Ok(StoreRestored::Held) => (Outcome::Held, held_class),
            Ok(StoreRestored::Settled) => (Outcome::Settled, held_class),
            Err(quarantine) => (Outcome::Failed, quarantine.class),
        };
        // An attempt that says the mob is managed or held without having
        // moved it there has nothing to show for it: the store is kept
        // quarantined as one that could not be served.
        let (outcome, class) = match outcome {
            Outcome::Managed | Outcome::Held => (
                Outcome::Failed,
                meerkat_mob::MobQuarantineClass::StoreUnavailable,
            ),
            other => (other, class),
        };
        if retrying {
            store.apply(Input::ResolveRetry { outcome, class });
        } else {
            store.apply(Input::ResolveLoad { outcome, class });
        }
        let answer = match restored {
            Ok(StoreRestored::Settled) => Ok(StoreRestored::Settled),
            Ok(_) => {
                store.cause = Some(Arc::new(MobError::Internal(
                    "the restore of this mob store ended without registering its mob".to_string(),
                )));
                Err(store.quarantine_error())
            }
            Err(quarantine) => {
                tracing::warn!(
                    mob_id = %store.mob_id,
                    class = %quarantine.class,
                    error = %quarantine.cause,
                    "a persisted mob store is quarantined: every other mob serves"
                );
                store.cause = Some(Arc::new(quarantine.cause));
                let named = quarantine.mob_id;
                let error = store.quarantine_error();
                if let Some(mob_id) = named {
                    // The id the restore names for the fault: the store's
                    // own mob for a stem-listed store, and for a store
                    // refused for its name the id it is already filed
                    // under, so that it is never filed under the mob it
                    // holds.
                    self.rename_at(path, &mob_id);
                    Err(match self.quarantined(&mob_id) {
                        Some(store) => store.quarantine_error(),
                        None => error,
                    })
                } else {
                    Err(error)
                }
            }
        };
        // A store that is gone with no destroy of this state to remember
        // leaves nothing to own.
        self.retain(|store| store.phase() != Phase::Gone || store.retired.is_some());
        // A load or a re-attempt that settles drops a record with no mob
        // managed for it (an empty store that is swept): what waits on the
        // managed mobs for that mob must hear that it is gone.
        if matches!(answer, Ok(StoreRestored::Settled)) {
            self.note_managed_changed();
        } else {
            self.note_changed();
        }
        answer
    }

    /// A handle a host inserted joins the set as managed (a create joins
    /// through its own record, [`Self::reserve_create`] and
    /// [`Self::resolve_created`]). A host's
    /// handle takes the record the id already
    /// has: under the id of a mob a host inserted before it replaces that
    /// mob's handle, and
    /// under the id of a dormant mob it makes that record managed, so no
    /// dormant record is left beside it (the set would know a mob it never
    /// serves). Under the id of a mob the set itself runs (a store it
    /// loaded, a mob it created) it is refused by that mob's record while
    /// the mob's actor runs: the mob keeps its id, and its actor is not
    /// left with nobody holding it. Once that mob has ended without a
    /// destroy (it was shut down and its actor is over) the record admits
    /// the handle, and the set serves the host's mob under the id. A mob
    /// that is being destroyed keeps its id until its destroy has recorded
    /// itself ([`Self::destroyed`]): its actor is over before that step,
    /// and a handle admitted in between would be the mob the destroy then
    /// finds under the id.
    /// Under the id of a mob whose unload is admitted it is refused
    /// by that mob's record, and the caller waits for the unload to end.
    /// Under the id of a store that is being loaded, is held before
    /// activation or is quarantined it is refused by that store's record
    /// too: the store was there first and keeps the id, and the caller
    /// answers what a create under the id is answered. The
    /// caller checked, under this same guard, that the id is free for a
    /// create.
    pub(crate) fn register_host_handle(
        &mut self,
        mob_id: &MobId,
        handle: MobHandle,
        path: Option<PathBuf>,
    ) -> Registration {
        // The record of an admitted unload is asked first: it refuses, so
        // what the unload was admitted for does not change under it.
        if let Some(store) = self
            .stores_of_mut(mob_id)
            .find(|store| store.phase() == Phase::Unloading)
            && HostedStore::refused(
                &store.apply(Input::Register {
                    holder_ended: false,
                }),
                Refusal::UnloadInFlight,
            )
        {
            return Registration::UnloadInFlight;
        }
        // The declared id is its launch's: no host's handle joins under it
        // or replaces its mob (the launch's own mob joins through its
        // create). Every record filed under the id at the declared place is
        // asked, and each refuses by itself: the declared record has no arm
        // that registers a handle, and a record that is gone refuses as
        // gone. No record's phase is read here, and nothing of a record
        // changes; the answer is the launch's whatever they said.
        if let Some((_, place)) = self
            .declared
            .clone()
            .filter(|(declared, _)| declared == mob_id)
        {
            for store in self
                .stores_of_mut(mob_id)
                .filter(|store| store.path == place)
            {
                store.apply(Input::Register {
                    holder_ended: false,
                });
            }
            return Registration::LaunchOwned;
        }
        // A host's handle under the id of a store the set is loading, holds
        // before activation, or keeps quarantined, or under an id a create
        // in flight took: the first holder keeps the id. That record is
        // asked, and it refuses: no second record is filed beside it, so
        // the store's mob is never registered as a duplicate of the handle,
        // and the create is not undone by it.
        self.release_abandoned_creates(mob_id);
        if let Some(store) = self.stores_of_mut(mob_id).find(|store| {
            matches!(
                store.phase(),
                Phase::Loading
                    | Phase::Held
                    | Phase::Quarantined
                    | Phase::Retrying
                    | Phase::Creating
            )
        }) && HostedStore::refused(
            &store.apply(Input::Register {
                holder_ended: false,
            }),
            Refusal::AlreadyHosted,
        ) {
            return Registration::AlreadyHosted;
        }
        // The phase of the record the handle takes, if the id has one.
        let taken = [Phase::Managed, Phase::Dormant]
            .into_iter()
            .find(|phase| self.stores_of(mob_id).any(|store| store.phase() == *phase));
        if taken.is_none() {
            self.push(HostedStore::new(mob_id.clone(), None));
        }
        // The record to register: the one the handle takes, otherwise the
        // one just filed (the last under the id).
        let Some(store) = self.by_id.get_mut(mob_id).and_then(|stores| match taken {
            Some(phase) => stores.iter_mut().find(|store| store.phase() == phase),
            None => stores.last_mut(),
        }) else {
            return Registration::Registered;
        };
        // The record decides whether the mob is registered. It refuses a
        // host's handle under the id of a mob the set itself runs (a store
        // it loaded, a mob it created) while that mob's actor runs: the
        // actor runs for the set, and a handle put in its place would
        // leave it with nobody holding it. Nothing of the record changes
        // then. Once that mob has ended (it was shut down and its actor
        // is over) nothing is left to hold, and the record admits the
        // handle: whether its mob has ended is read from the record's own
        // handle and told to it with the registration.
        //
        // A destroyed mob's actor is over too, and a destroy records
        // itself here only after the native destroy replied
        // ([`Self::destroyed`]). Until then the record is still that
        // mob's, and the id stays with it: the destroy must find its own
        // mob under the id when it records itself. So a mob whose last
        // phase is `Destroyed` is not told as ended. The end of the task
        // is read before the phase: once the task has ended, the phase it
        // published last is final.
        let holder_ended = store.loaded_handle().is_some_and(|held| {
            held.actor_task_watch().has_ended()
                && held.status_observation_snapshot() != MobState::Destroyed
        });
        let registered = store
            .apply(Input::Register { holder_ended })
            .iter()
            .any(|effect| matches!(effect, Effect::StoreRegistered { .. }));
        if !registered {
            // A record filed for this registration alone is not kept.
            if taken.is_none()
                && let Some(stores) = self.by_id.get_mut(mob_id)
            {
                stores.pop();
                if stores.is_empty() {
                    self.by_id.remove(mob_id);
                }
            }
            return Registration::AlreadyHosted;
        }
        store.handle = Some(handle);
        store.path = path;
        store.note_managed();
        // A host that inserts its own handles restores its mobs itself:
        // the set's machine notes it in the step that makes the handle
        // visible, and never forgets it.
        apply_to_set(&self.set, SetInput::RegisterHostHandle);
        self.note_managed_changed();
        Registration::Registered
    }

    /// A create asks for its mob's id, before anything of the mob is
    /// built. Under the declared id the declared record decides: it admits
    /// the launch's own create while its store holds no mob. Under any
    /// other id the create gets the id only when no live record names it
    /// and no destroyed mob's storage cleanup is owed under it: a record is
    /// filed for the create in this same step and admits it. From here
    /// until the create is resolved the id is the create's: a second
    /// create meets [`CreateReservation::InFlight`], and a host's handle is
    /// refused by the record ([`Self::register_host_handle`]). A create that ended
    /// without resolving its record (its future was dropped, or it
    /// panicked) no longer holds the id: its record is released first.
    pub(crate) fn reserve_create(&mut self, mob_id: &MobId, launch: bool) -> CreateReservation {
        self.release_abandoned_creates(mob_id);
        let changes = self.changes.clone();
        if let Some((_, place)) = self
            .declared
            .clone()
            .filter(|(declared, _)| declared == mob_id)
        {
            let Some(store) = self
                .stores_of_mut(mob_id)
                .find(|store| store.path == place && store.phase() != Phase::Gone)
            else {
                return CreateReservation::Taken;
            };
            let effects = store.apply(Input::RequestCreate { launch });
            if effects
                .iter()
                .any(|effect| matches!(effect, Effect::CreateAdmitted))
            {
                let attempt = AttemptToken::new(changes);
                store.attempt = Arc::downgrade(&attempt);
                self.note_changed();
                return CreateReservation::Reserved(attempt);
            }
            return if HostedStore::refused(&effects, Refusal::CreateInFlight) {
                CreateReservation::InFlight
            } else if HostedStore::refused(&effects, Refusal::LaunchOwned) {
                CreateReservation::LaunchOwned
            } else {
                CreateReservation::Taken
            };
        }
        // Which record the id has is a fact of the set's map, read under
        // the lock that files the create's record. Whether the id is free
        // for a create is the record's answer: a live record under the id
        // is asked, and it refuses (it stands for a store or a mob the
        // set knows) or says a create is in flight. Without a live record
        // one is filed for this create and asked in the same step: a
        // record that stands for nothing yet admits it.
        let filed = !self
            .stores_of(mob_id)
            .any(|store| store.phase() != Phase::Gone);
        if filed {
            if self.destroy_cleanup_owed(mob_id) {
                return CreateReservation::CleanupOwed;
            }
            self.push(HostedStore::new(mob_id.clone(), None));
        }
        let asked = self.by_id.get_mut(mob_id).and_then(|stores| {
            if filed {
                stores.last_mut()
            } else {
                stores.iter_mut().find(|store| store.phase() != Phase::Gone)
            }
        });
        let Some(store) = asked else {
            return CreateReservation::Taken;
        };
        let effects = store.apply(Input::RequestCreate { launch });
        if effects
            .iter()
            .any(|effect| matches!(effect, Effect::CreateAdmitted))
        {
            let attempt = AttemptToken::new(changes);
            store.attempt = Arc::downgrade(&attempt);
            self.note_changed();
            return CreateReservation::Reserved(attempt);
        }
        let in_flight = HostedStore::refused(&effects, Refusal::CreateInFlight);
        // A record filed for this create alone is not kept when it refused
        // (a create that names the launch under an id nothing declared).
        if filed && let Some(stores) = self.by_id.get_mut(mob_id) {
            stores.pop();
            if stores.is_empty() {
                self.by_id.remove(mob_id);
            }
        }
        if in_flight {
            CreateReservation::InFlight
        } else {
            CreateReservation::Taken
        }
    }

    /// The record of `mob_id` whose create holds `attempt`.
    fn creating_mut(
        &mut self,
        mob_id: &MobId,
        attempt: &Arc<AttemptToken>,
    ) -> Option<&mut HostedStore> {
        self.stores_of_mut(mob_id).find(|store| {
            store.phase() == Phase::Creating
                && store
                    .attempt
                    .upgrade()
                    .is_some_and(|held| Arc::ptr_eq(&held, attempt))
        })
    }

    /// The create that holds `attempt` built its mob: the record is managed
    /// and serves `handle` from this step, with the mob's store file at
    /// `path` (`None`: a mob in memory). `false` when no record holds the
    /// create any more; nothing is registered then.
    pub(crate) fn resolve_created(
        &mut self,
        mob_id: &MobId,
        attempt: &Arc<AttemptToken>,
        handle: MobHandle,
        path: Option<PathBuf>,
    ) -> bool {
        let Some(store) = self.creating_mut(mob_id, attempt) else {
            return false;
        };
        let registered = store
            .apply(Input::ResolveCreate {
                outcome: CreateOutcome::Created,
                in_memory: path.is_none(),
            })
            .iter()
            .any(|effect| matches!(effect, Effect::StoreRegistered { .. }));
        if !registered {
            return false;
        }
        store.attempt = Weak::new();
        store.handle = Some(handle);
        // The declared record keeps its place when its mob lives in memory.
        if path.is_some() {
            store.path = path;
        }
        store.note_managed();
        self.note_managed_changed();
        true
    }

    /// The create that holds `attempt` ended without a mob (it was refused
    /// or it failed): the id is free again in this step. The record filed
    /// for the create is dropped; the declared record stays, known to hold
    /// no mob.
    pub(crate) fn resolve_create_failed(&mut self, mob_id: &MobId, attempt: &Arc<AttemptToken>) {
        let Some(store) = self.creating_mut(mob_id, attempt) else {
            return;
        };
        store.apply(Input::ResolveCreate {
            outcome: CreateOutcome::Failed,
            in_memory: true,
        });
        store.attempt = Weak::new();
        self.retain(|store| store.phase() != Phase::Gone || store.retired.is_some());
        self.note_changed();
    }

    /// Release every record of `mob_id` whose create ended without
    /// resolving it (its future was dropped, or it panicked): the token of
    /// the create is gone, so nothing builds a mob for the record any more.
    fn release_abandoned_creates(&mut self, mob_id: &MobId) {
        let mut released = false;
        for store in self
            .stores_of_mut(mob_id)
            .filter(|store| store.phase() == Phase::Creating && !store.attempt_runs())
        {
            store.apply(Input::ResolveCreate {
                outcome: CreateOutcome::Failed,
                in_memory: true,
            });
            store.attempt = Weak::new();
            released = true;
        }
        if released {
            self.retain(|store| store.phase() != Phase::Gone || store.retired.is_some());
            self.note_changed();
        }
    }

    /// Test seam: take the handle out of the managed record of `mob_id`.
    #[cfg(test)]
    pub(crate) fn take_handle_for_test(&mut self, mob_id: &MobId) -> Option<MobHandle> {
        self.stores_of_mut(mob_id)
            .find(|store| store.phase() == Phase::Managed)
            .and_then(|store| store.handle.take())
    }

    /// Ask the record of the managed mob `mob_id` for its idle unload, once
    /// the mob has been managed for `loaded_for`. The record decides: it
    /// admits the unload only for a store the set can load again, and from
    /// that step the mob is not handed out. `Some` when it admitted: ask the
    /// mob's actor through the handle and resolve the record with its
    /// answer, holding `attempt` until then.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn request_unload(
        &mut self,
        mob_id: &MobId,
        loaded_for: std::time::Duration,
    ) -> Option<(MobHandle, Arc<AttemptToken>)> {
        match self.admit_unload(mob_id, loaded_for) {
            UnloadAdmission::Admitted { handle, attempt } => Some((handle, attempt)),
            UnloadAdmission::NotReloadable | UnloadAdmission::NotAdmitted => None,
        }
    }

    /// [`Self::request_unload`] with the record's answer told apart: a
    /// caller that asks for the unload of one named mob says why it was not
    /// admitted.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn admit_unload(
        &mut self,
        mob_id: &MobId,
        loaded_for: std::time::Duration,
    ) -> UnloadAdmission {
        let changes = self.changes.clone();
        let Some(store) = self
            .stores_of_mut(mob_id)
            .find(|store| store.phase() == Phase::Managed && store.managed_for(loaded_for))
        else {
            return UnloadAdmission::NotAdmitted;
        };
        // The handle is read before the record is asked: a record with
        // nothing to ask admits no unload, so it is never left Unloading
        // with no actor to answer.
        let Some(handle) = store.handle.clone() else {
            return UnloadAdmission::NotAdmitted;
        };
        let effects = store.apply(Input::RequestUnload);
        if HostedStore::refused(&effects, Refusal::NotReloadable) {
            return UnloadAdmission::NotReloadable;
        }
        let admitted = effects
            .iter()
            .any(|effect| matches!(effect, Effect::UnloadAdmitted));
        if !admitted {
            return UnloadAdmission::NotAdmitted;
        }
        let attempt = AttemptToken::new(changes);
        store.attempt = Arc::downgrade(&attempt);
        // The question changes nothing a reader of the managed set sees:
        // the mob is listed until its actor closed, and only its handle is
        // withheld. The observers of the managed set are not woken.
        self.note_changed();
        UnloadAdmission::Admitted { handle, attempt }
    }

    /// Take over the unload of `mob_id` that an earlier attempt left
    /// without an answer (its future was dropped, or the mob's actor had not
    /// ended when it gave up): the record is still Unloading and nothing
    /// runs for it. The caller asks the actor again and resolves the record.
    /// `None` when no unload of the mob was left so.
    pub(crate) fn resume_unload(
        &mut self,
        mob_id: &MobId,
    ) -> Option<(MobHandle, Arc<AttemptToken>)> {
        let changes = self.changes.clone();
        let store = self
            .stores_of_mut(mob_id)
            .find(|store| store.phase() == Phase::Unloading && !store.attempt_runs())?;
        let handle = store.handle.clone()?;
        let attempt = AttemptToken::new(changes);
        store.attempt = Arc::downgrade(&attempt);
        Some((handle, attempt))
    }

    /// The actor `actor` observes answered the unload its mob's record
    /// admitted. `Closed`: the actor closed idle and has ended, so the record
    /// returns to Dormant and the next lookup loads the mob again. `Busy`:
    /// the actor declined, so the record returns to Managed and the same mob
    /// is served again. `true` when the mob was unloaded.
    ///
    /// `stopped` is what the mob itself answered about its stop when its
    /// actor closed: the record of a mob that was unloaded keeps it, so the
    /// mob lists as stopped and no demand loads it. `None` says it was not
    /// read (a resumed unload has no moment to read it): the record stays
    /// without the answer and the set's next read of the store gives it
    /// ([`Self::note_stop_at`]). It is ignored for a mob that stays
    /// managed.
    pub(crate) fn resolve_unload(
        &mut self,
        mob_id: &MobId,
        actor: &meerkat_mob::MobActorTaskWatch,
        outcome: UnloadOutcome,
        stopped: Option<bool>,
    ) -> bool {
        let Some(store) = self.stores_of_mut(mob_id).find(|store| {
            store.phase() == Phase::Unloading
                && store
                    .loaded_handle()
                    .is_some_and(|handle| actor.observes(handle))
        }) else {
            return false;
        };
        let stop = match stopped {
            None => Stop::Unread,
            Some(false) => Stop::NotStopped,
            Some(true) => Stop::Stopped,
        };
        let effects = store.apply(Input::ResolveUnload { outcome, stop });
        store.attempt = Weak::new();
        let unloaded = effects
            .iter()
            .any(|effect| matches!(effect, Effect::StoreUnloaded));
        if unloaded {
            // The mob left the managed set in this step, not when its
            // unload was admitted.
            store.handle = None;
            self.note_managed_changed();
        } else {
            self.note_changed();
        }
        unloaded
    }

    /// The held child mob `mob_id`'s store is gone (its destroy finished
    /// elsewhere and the store was swept), or another store's mob owns the
    /// id: the hold is settled and its record dropped.
    pub(crate) fn settle_held(&mut self, mob_id: &MobId) {
        for store in self.stores_of_mut(mob_id) {
            // Every record under the id is asked. Only a held one releases
            // its hold: the record refuses the input in every other phase,
            // and a record that refused is left as it is. A record that
            // released is Gone and is dropped below.
            store.apply(Input::ReleaseHeld {
                outcome: Outcome::Settled,
            });
        }
        let recorded = self.stores().count();
        self.retain(|store| store.phase() != Phase::Gone || store.retired.is_some());
        if self.stores().count() == recorded {
            // Nothing was held under the id: every record refused, and no
            // record left the set.
            self.note_changed();
            return;
        }
        // A hold that is settled drops its record with no mob ever managed
        // for it. What waits on the managed mobs for that mob (an outcome
        // owed to one of its members) must hear that it is gone.
        self.note_managed_changed();
    }

    /// Replace the reason the held child mob `mob_id` is held for.
    pub(crate) fn set_hold_reason(&mut self, mob_id: &MobId, reason: crate::ChildMobHoldReason) {
        for store in self.stores_of_mut(mob_id) {
            if store.phase() == Phase::Held
                && let Some(held) = store.held.as_mut()
            {
                held.reason = reason;
                return;
            }
        }
    }

    /// Whether the unload of the mob whose actor task `actor` observes is
    /// admitted and not answered yet: its record is Unloading. A destroy of
    /// that mob is refused by the record until the unload is answered.
    pub(crate) fn unload_admitted_for(
        &self,
        mob_id: &MobId,
        actor: &meerkat_mob::MobActorTaskWatch,
    ) -> bool {
        self.stores_of(mob_id).any(|store| {
            store.phase() == Phase::Unloading
                && store
                    .loaded_handle()
                    .is_some_and(|handle| actor.observes(handle))
        })
    }

    /// Whether the mob whose actor task `actor` observes is the one served
    /// under `mob_id` now: its record is Managed.
    pub(crate) fn serves_actor(
        &self,
        mob_id: &MobId,
        actor: &meerkat_mob::MobActorTaskWatch,
    ) -> bool {
        self.stores_of(mob_id).any(|store| {
            store.phase() == Phase::Managed
                && store
                    .loaded_handle()
                    .is_some_and(|handle| actor.observes(handle))
        })
    }

    /// The mob whose actor task `actor` observes was destroyed natively, and
    /// `retired` is the record of that destroy. Its store's record is Gone
    /// from this step on and keeps `retired`, unless a destroy recorded
    /// earlier under the id still owes its storage cleanup: replacing that
    /// one would lose the cleanup, so this destroy is not kept. `true` when
    /// the destroyed mob was the one managed under the id (the managed set
    /// lost it); a handle registered under the id while the destroy ran is
    /// another mob and stays managed.
    pub(crate) fn destroyed(
        &mut self,
        mob_id: &MobId,
        actor: &meerkat_mob::MobActorTaskWatch,
        retired: &Arc<RetiredMob>,
    ) -> bool {
        let keep = !self.destroy_cleanup_owed(mob_id);
        if keep {
            // The record of an earlier destroy under the id that owes
            // nothing is replaced.
            self.retain(|store| !(store.phase() == Phase::Gone && &store.mob_id == mob_id));
        }
        let observed = |store: &HostedStore| {
            store
                .loaded_handle()
                .is_some_and(|handle| actor.observes(handle))
        };
        let removed = self.stores_of(mob_id).any(observed);
        if !removed {
            // The destroyed mob left the managed set while its destroy ran
            // (a host inserted another handle under the id): it is entered
            // again only to be recorded gone, so the cleanup it owes is not
            // lost. It was a mob of the set: its record is filed as a
            // create's is and resolved as created, in memory (the path of
            // its storage is taken from the destroy's report below).
            self.push(HostedStore::new(mob_id.clone(), None));
            if let Some(store) = self
                .by_id
                .get_mut(mob_id)
                .and_then(|stores| stores.last_mut())
            {
                store.apply(Input::RequestCreate { launch: false });
                store.apply(Input::ResolveCreate {
                    outcome: CreateOutcome::Created,
                    in_memory: true,
                });
            }
        }
        let Some(store) = self.by_id.get_mut(mob_id).and_then(|stores| {
            if removed {
                stores.iter_mut().find(|store| observed(store))
            } else {
                stores.last_mut()
            }
        }) else {
            return removed;
        };
        // The record is managed here: a record whose unload is admitted
        // refuses the destroy, and the answer to that unload is its actor's.
        // The caller waits for it before it records the destroy
        // ([`Self::unload_admitted_for`]), under the same lock as this step.
        // What follows is the record's answer: a record that refuses the
        // destroy (a declared store's does) keeps its mob served, with
        // its handle, and nothing left the managed set.
        let gone = store
            .apply(Input::Destroy)
            .iter()
            .any(|effect| matches!(effect, Effect::StoreGone));
        if !gone {
            return false;
        }
        store.handle = None;
        store.retired = keep.then(|| Arc::clone(retired));
        if let Some(storage) = retired.storage.as_ref() {
            store.path = Some(storage.path.clone());
        }
        self.retain(|store| store.phase() != Phase::Gone || store.retired.is_some());
        if removed {
            self.note_managed_changed();
        } else {
            self.note_changed();
        }
        removed
    }

    /// The load or re-attempt in flight for the store at `path` is left to
    /// run behind the pass that started it ([`AttemptToken::leave_to_run`]).
    /// Nothing changes in the record: its attempt is in flight as before,
    /// and a lookup of its mob waits for it.
    pub(crate) fn leave_attempt_to_run_at(&self, path: &Path) {
        if let Some(attempt) = self
            .stores()
            .find(|store| store.phase() != Phase::Gone && store.path.as_deref() == Some(path))
            .and_then(|store| store.attempt.upgrade())
        {
            attempt.leave_to_run();
        }
    }
}

/// The host's default mob, as its host declared it: the mob's id and the
/// file of its store (`None`: the mob lives in memory). Set once, when the
/// state is built. The facts the set holds about it are in the module doc.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct DeclaredDefault {
    mob_id: MobId,
    store: Option<PathBuf>,
    /// The declared file's forked-participant table
    /// ([`HostedSet::custody_table`]): opened at the declaration, or, when
    /// that failed, before the set's first pass
    /// ([`HostedSet::adopt_custody_table`]). Never set for a mob in memory.
    table: std::sync::OnceLock<Arc<dyn meerkat_mob::store::ForkedParticipantStore>>,
    /// The declared file's canonical path, read once
    /// ([`HostedSet::is_default_store_file`]).
    canonical_store: std::sync::OnceLock<PathBuf>,
}

/// The hosted set of one [`crate::MobMcpState`]: the records behind one
/// lock, with the two change signals over them.
pub(crate) struct HostedSet {
    stores: tokio::sync::RwLock<HostedStores>,
    /// The state of the set as a whole ([`SetFacts`]; see the module doc
    /// for the facts it holds). Everything about one store is in its
    /// record. Shared with the records' side, whose registration of a
    /// host's own handle writes it; read without the records' lock.
    set: Arc<std::sync::Mutex<SetFacts>>,
    /// The host's default mob, when it declared one.
    declared: Option<DeclaredDefault>,
    managed_epoch: tokio::sync::watch::Sender<u64>,
    changes: tokio::sync::watch::Sender<u64>,
}

#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
impl HostedSet {
    pub(crate) fn new() -> Self {
        let managed_epoch = tokio::sync::watch::Sender::new(0);
        let changes = tokio::sync::watch::Sender::new(0);
        let set = Arc::new(std::sync::Mutex::new(SetFacts::new()));
        Self {
            stores: tokio::sync::RwLock::new(HostedStores {
                by_id: BTreeMap::new(),
                set: Arc::clone(&set),
                declared: None,
                managed_epoch: managed_epoch.clone(),
                changes: changes.clone(),
            }),
            set,
            declared: None,
            managed_epoch,
            changes,
        }
    }

    // ------------------------------------------------------------------
    // The host's default mob: facts of the set (see the module doc). One
    // function per fact; nothing else reads the declaration.
    // ------------------------------------------------------------------

    /// The host declares its default mob: its id, the file of its store,
    /// and that file's forked-participant table, which the caller opened
    /// ([`Self::custody_table`]). Called while the state is built, before
    /// the set is shared. The store's record is filed in the same step
    /// ([`HostedStores::declare`]): from here on the record says that the
    /// id is its launch's and that its mob waits for the launch's serve.
    pub(crate) fn declare_default(
        &mut self,
        mob_id: MobId,
        store: Option<PathBuf>,
        table: Option<Arc<dyn meerkat_mob::store::ForkedParticipantStore>>,
    ) {
        self.stores.get_mut().declare(&mob_id, store.clone());
        self.declared = Some(DeclaredDefault {
            mob_id,
            store,
            table: table.map_or_else(std::sync::OnceLock::new, std::sync::OnceLock::from),
            canonical_store: std::sync::OnceLock::new(),
        });
    }

    /// The id of the host's declared default mob.
    pub(crate) fn default_mob_id(&self) -> Option<&MobId> {
        self.declared.as_ref().map(|declared| &declared.mob_id)
    }

    /// The file of the default mob's store: `None` when the host declared
    /// no default mob, or one that lives in memory.
    pub(crate) fn default_store(&self) -> Option<&Path> {
        self.declared
            .as_ref()
            .and_then(|declared| declared.store.as_deref())
    }

    /// Whether `path` is the declared file of the default mob's store.
    pub(crate) fn is_default_store(&self, path: &Path) -> bool {
        self.default_store() == Some(path)
    }

    /// Whether the file at `path` is the default mob's store: the declared
    /// path itself, or another spelling of the same file. The census of a
    /// persistent root asks this of every store file it meets, so the
    /// declared file's canonical path is read once.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn is_default_store_file(&self, path: &Path) -> bool {
        let Some(declared) = self.declared.as_ref() else {
            return false;
        };
        let Some(store) = declared.store.as_deref() else {
            return false;
        };
        if store == path {
            return true;
        }
        let canonical = match declared.canonical_store.get() {
            Some(canonical) => canonical.clone(),
            None => match tokio::fs::canonicalize(store).await {
                Ok(canonical) => declared.canonical_store.get_or_init(|| canonical).clone(),
                // No file at the declared place: no file is it.
                Err(_) => return false,
            },
        };
        matches!(tokio::fs::canonicalize(path).await, Ok(path) if path == canonical)
    }

    /// The host's one forked-participant table, when its default mob has a
    /// store of its own: the table of the declared file. The host's
    /// councils, every mob the set builds, the set's own reads and the
    /// default mob then use this one table, whatever table the set would
    /// use otherwise. `None`: no default mob with a store file is declared,
    /// or its table is not open ([`Self::custody_table_missing`]), and the
    /// set uses its own.
    pub(crate) fn custody_table(
        &self,
    ) -> Option<&Arc<dyn meerkat_mob::store::ForkedParticipantStore>> {
        self.declared
            .as_ref()
            .and_then(|declared| declared.table.get())
    }

    /// Whether the host declared a default mob with a store file whose
    /// forked-participant table is not open. The default mob is not built
    /// in that state: it would use its file's table beside the one the set
    /// has used since its first pass.
    pub(crate) fn custody_table_missing(&self) -> bool {
        self.declared
            .as_ref()
            .is_some_and(|declared| declared.store.is_some() && declared.table.get().is_none())
    }

    /// The declared file's table was opened after the declaration, before
    /// the set's first pass used any table: it is the host's one table
    /// from here on. Without effect once a table is set.
    pub(crate) fn adopt_custody_table(
        &self,
        table: Arc<dyn meerkat_mob::store::ForkedParticipantStore>,
    ) {
        if let Some(declared) = self.declared.as_ref() {
            let _ = declared.table.set(table);
        }
    }

    /// The default mob, while it awaits its launch's first serve: `None`
    /// once the launch has served (however that ended), after a
    /// stand-down, and for a host that declared no default mob. The answer
    /// is the declared record's ([`HostedStores::awaiting_launch`]). Read by
    /// what must not commit to a guess while the default mob is not served
    /// and must not wait for it either.
    pub(crate) async fn launch_serve_awaited(&self) -> Option<MobId> {
        self.stores.read().await.awaiting_launch().cloned()
    }

    /// Whether `mob_id` is the id the host declared as its default mob's:
    /// which store a build under the id uses, and under which id a fault of
    /// that store is filed. That the id is its launch's (no other create,
    /// no host's handle, no destroy through the set) is not decided here:
    /// the declared record refuses each of them
    /// ([`HostedStores::launch_owns`]).
    pub(crate) fn is_default_mob(&self, mob_id: &MobId) -> bool {
        self.default_mob_id() == Some(mob_id)
    }

    /// Whether `mob_id` is the id the declared record was filed under
    /// ([`HostedStores::launch_owns`]): the same id as
    /// [`Self::is_default_mob`], read under the set's lock.
    pub(crate) async fn launch_owns(&self, mob_id: &MobId) -> bool {
        self.stores.read().await.launch_owns(mob_id)
    }

    /// The mobs an idle unload pass turns to
    /// ([`HostedStores::unload_candidates`]). The default mob may be among
    /// them: its record refuses the unload when it is asked (a declared
    /// store is never unloaded for idleness), so its actor is never asked.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn unload_candidates(&self, loaded_for: std::time::Duration) -> Vec<MobId> {
        self.stores.read().await.unload_candidates(loaded_for)
    }

    /// The dormant stores a pass of the state may load on its own
    /// ([`HostedStores::dormant_stores`]): never the declared store, whose
    /// record refuses a pass's load.
    pub(crate) async fn stores_a_pass_may_load(&self) -> Vec<(MobId, PathBuf)> {
        self.stores.read().await.dormant_stores()
    }

    /// Wait for the end of the wait for the launch, for a lookup of the mob
    /// `mob_id` that would load it. Answers at once for every mob whose
    /// record awaits no launch. Otherwise the caller waits here, on the
    /// set's change signal, and reads the record again at each change; it
    /// holds no lock of the set while it waits, and it is no task of the
    /// state: any number of lookups wait on the one record. The wait ends
    /// when the launch's first serve has ended, however it ended, and at a
    /// stand-down; the caller then meets the set, or the state's closed
    /// admission, as it is.
    pub(crate) async fn await_launch_wait_end(&self, mob_id: &MobId) {
        let mut changes = self.changes.subscribe();
        loop {
            changes.borrow_and_update();
            if !self.stores.read().await.awaits_launch(mob_id) {
                return;
            }
            if changes.changed().await.is_err() {
                return;
            }
        }
    }

    /// The wait for the launch ended ([`HostedStores::end_launch_wait`]):
    /// the launch's first serve ended, however it ended, or the state
    /// stands down.
    pub(crate) async fn end_launch_wait(&self) {
        self.stores.write().await.end_launch_wait();
    }

    // ------------------------------------------------------------------
    // The set as a whole: each function applies one input of the set's
    // machine or reads one of its facts.
    // ------------------------------------------------------------------

    fn set_state<T>(&self, read: impl FnOnce(&SetRecord) -> T) -> T {
        read(
            &self
                .set
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .record,
        )
    }

    /// The builder says whether this host has stores from before whose
    /// census is the only way to know them: a persistent root, or a
    /// declared default mob with a store file. Said while the state is
    /// built; the machine refuses it once a pass was begun.
    pub(crate) fn expect_census(&self, owed: bool) {
        apply_to_set(&self.set, SetInput::ExpectCensus { owed });
    }

    /// Whether a census of the stores from before is owed.
    pub(crate) fn census_owed(&self) -> bool {
        self.set_state(|record| record.state().census_owed)
    }

    /// Ask to begin a pass over the stores from before. `Some(first)` when
    /// the machine admitted it, `first` for the first pass this set
    /// begins: what must be settled before any pass is settled on it and
    /// never on a pass that follows one that did not finish. `None` when
    /// there is nothing to pass over or a pass already ran to its end.
    pub(crate) fn begin_pass(&self) -> Option<bool> {
        apply_to_set(&self.set, SetInput::BeginPass)
            .iter()
            .find_map(|effect| match effect {
                SetEffect::PassBegun { first } => Some(*first),
                _ => None,
            })
    }

    /// The pass that was begun ran to its end: every store file of the
    /// root has its record now, loaded or not, so the census is complete.
    pub(crate) fn finish_pass(&self) {
        let effects = apply_to_set(&self.set, SetInput::FinishPass);
        self.note_census(&effects);
    }

    /// Whether a pass over the stores from before ran to its end.
    pub(crate) fn pass_finished(&self) -> bool {
        self.set_state(|record| record.state().lifecycle_phase == SetPhase::Restored)
    }

    /// The host declared its own restore done: the census is complete.
    pub(crate) fn declare_restored(&self) {
        let effects = apply_to_set(&self.set, SetInput::DeclareRestored);
        self.note_census(&effects);
    }

    /// Everything that waits on the set reads it again when the census
    /// became complete.
    fn note_census(&self, effects: &[SetEffect]) {
        if effects
            .iter()
            .any(|effect| matches!(effect, SetEffect::CensusCompleted))
        {
            self.changes
                .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        }
    }

    /// Whether every store this host has from before is recorded.
    pub(crate) fn census_complete(&self) -> bool {
        self.set_state(|record| record.state().census_complete)
    }

    /// Whether no mob can still turn up that no record names: the census
    /// is complete, or none is owed and no host inserts its own handles.
    /// The machine keeps this answer; nothing outside it combines the
    /// facts.
    pub(crate) fn all_mobs_known(&self) -> bool {
        self.set_state(|record| record.state().all_mobs_known)
    }

    pub(crate) async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, HostedStores> {
        self.stores.read().await
    }

    pub(crate) async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, HostedStores> {
        self.stores.write().await
    }

    /// The lock itself, for a test that must not wait for it.
    #[cfg(test)]
    pub(crate) fn lock(&self) -> &tokio::sync::RwLock<HostedStores> {
        &self.stores
    }

    /// Change signal over the managed handle set.
    pub(crate) fn managed_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.managed_epoch.subscribe()
    }

    /// Change signal over every record: a caller that found a load in flight
    /// subscribes before it reads and waits here to read again.
    pub(crate) fn changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }
}
