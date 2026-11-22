//! Canonical lifecycle authority for ONE mob store on a host.
//!
//! A host's mob set hosts any number of mobs, one store each. This machine
//! owns one store's stay in that set, from the moment the set knows it to
//! the moment it is gone: whether the store's mob is known and dormant, being
//! loaded, managed, being unloaded, held before activation, quarantined, being
//! re-attempted, or gone for good. "Which mobs does this host have, and which are loaded"
//! is the set of these records and nothing else: knowing a mob never
//! requires loading it, and a mob that is not loaded is never read as gone.
//!
//! A quarantined store is the case the machine began with and still owns: a
//! store whose load failed is kept as found, no actor runs for it, a lookup
//! of its mob answers the typed quarantine, and every other mob of the host
//! serves. The record keeps why it is quarantined (its class), whether a
//! re-attempt of its restore is in flight, and how each re-attempt ended.
//!
//! One store of a host can be the store its launch DECLARES (the host's
//! default mob). Its record exists from the declaration on, whether or not a
//! file is at its place, and the record owns what makes that store its
//! launch's: its mob waits for the launch's first serve (a lookup's load and
//! an on-demand re-attempt are refused until that serve has ended, however it
//! ended, or the host stood down); no pass of the set loads it; no create
//! but the launch's, and no host's handle, joins under it; it is not
//! destroyed through the set and never unloaded for idleness; and a load
//! that finds no mob in it keeps the record, known to hold no mob, for the
//! launch's create. The wait is a fact of one process: like every record it
//! is never persisted, and a new process declares the store again.
//!
//! A CREATE takes its mob's id before anything of the mob is built. The set
//! files a record for the create in the step that finds no record under the
//! id (the launch's create asks the declared record instead), and from the
//! record's admission until the create is resolved the id is the create's:
//! a second create under it is refused or waits, and no host's handle
//! registers over it. A created mob is managed from the step that resolves
//! the create; a create that ended without a mob frees the id in that step.
//! Only a record that stands for nothing yet admits a create (`known`): the
//! one filed for it. A record a census listed, and the record of a mob that
//! was loaded, registered or created and is not loaded now, stands for that
//! store and refuses.
//!
//! Whether a mob that is NOT LOADED is stopped is a fact of its record
//! (`stop`). A loaded mob speaks for itself, so the record holds the answer
//! only while it is dormant or its load is in flight, and two steps write
//! it: the mob's own answer when its actor closed at an unload, and the
//! set's read of the store of a mob that is not loaded, which the record
//! takes only while it does not know the answer. What originates work for
//! the mob asks for its load as a demand, and the record refuses the load
//! of a stopped mob; a read of the mob and an operator's verb ask as a
//! lookup and load it stopped or not.
//!
//! # Scope discipline
//!
//! Record-scoped, not registry-scoped: one machine state per store, so there
//! is no index keyed by store or mob inside the machine. The mob set holds
//! the records in one map and reads its sets (managed, held, quarantined,
//! dormant, gone) from their phases. Volatile: the mob set holds each record
//! in memory and never persists it. The durable fact is the store itself; a
//! new process takes its census again and derives every record from what it
//! finds.
//!
//! The machine deliberately does NOT own:
//!
//! - **The mob's own lifecycle.** `MobMachine` is canonical there while the
//!   store is managed; a store in any other phase has no `MobMachine` state
//!   in this process at all.
//! - **Whether a mob is idle.** The mob's actor decides that in the step
//!   that closes its admission. The machine decides, before the actor is
//!   asked, whether the question may be put at all (only a managed mob
//!   whose store can be loaded again), and from that step until the
//!   actor's answer is resolved the mob's handle is not handed out.
//! - **Time.** It reads no clock and never loads, re-attempts or unloads on
//!   its own. The shell asks, naming what asks where it matters (an explicit
//!   retry, or an id-addressed verb that meets a quarantine on demand), and
//!   the machine decides: one load or re-attempt at a time, an explicit
//!   re-attempt of any class, an on-demand one only for a store that could
//!   not serve now.
//! - **The load itself.** The shell runs the load or re-attempt the machine
//!   admitted and reports how it ended; the machine owns what the outcome
//!   means for the record.
//!
//! # Totality discipline
//!
//! Every (phase, input) pair has an explicit transition naming its own `to`
//! phase. Refusal arms return to the exact phase they matched, so no typed
//! refusal changes the record. There is no catch-all arm.

use meerkat_machine_dsl::machine;

/// Why a persisted mob store is quarantined.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MobQuarantineClass {
    /// A store could not serve a read the restore needed now: nothing is
    /// known to be wrong with the stored state, and a re-attempt may pass.
    #[default]
    StoreUnavailable,
    /// The mob's store, or stored state its restore needs, cannot be opened,
    /// read or decoded.
    StoreUnreadable,
    /// The restore refused the store for another typed reason (for example
    /// a store whose records disagree among themselves).
    ResumeRefused,
}

impl MobQuarantineClass {
    /// Stable token in structured data and on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StoreUnavailable => "store_unavailable",
            Self::StoreUnreadable => "store_unreadable",
            Self::ResumeRefused => "resume_refused",
        }
    }

    /// Whether the store may restore on a later attempt without a repair: a
    /// store that could not serve now. The other classes need the store, or
    /// the state it holds, repaired first.
    #[must_use]
    pub const fn is_retryable(self) -> bool {
        matches!(self, Self::StoreUnavailable)
    }
}

impl std::fmt::Display for MobQuarantineClass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What asks for a re-attempt of a quarantined store's restore.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MobStoreRetryTrigger {
    /// An explicit retry, for example after the store was repaired.
    #[default]
    Explicit,
    /// An id-addressed verb that met the quarantine.
    OnDemand,
}

/// How a load of a store, a re-attempt of a quarantined store's restore, or
/// the release of a held mob ended.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MobStoreRestoreOutcome {
    /// The mob resumed and is managed.
    #[default]
    Managed,
    /// The mob is a child held before activation until its owner resolves.
    Held,
    /// The store held no mob and was swept, or the child was torn down with
    /// its archived owner: the store is gone.
    Settled,
    /// The load or re-attempt failed: the store is quarantined under the
    /// class the attempt met.
    Failed,
}

/// How a mob store came to its host's set.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MobStoreOrigin {
    /// A store file under the host's mob root: the census listed it, or a
    /// create of this host wrote it. The set can load it again.
    #[default]
    Persisted,
    /// A store that lives only in this process (in-memory storage): once its
    /// mob is not managed there is nothing to load.
    Volatile,
    /// A mob a host built itself and inserted by handle: the set never
    /// builds or loads it.
    HostHandle,
    /// The store of the mob the host's launch declares, a file or none (a
    /// mob that lives in memory). It is its launch's: only the launch's
    /// serve creates its mob or loads it first, no pass of the set loads
    /// it, no other create or handle takes its id, it is not destroyed
    /// through the set, and it is never unloaded for idleness.
    Declared,
}

/// What asks for the one load of a dormant store.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MobStoreLoadTrigger {
    /// Something that addresses the store's mob.
    #[default]
    Lookup,
    /// A pass of the set that loads stores on its own: the restore of an
    /// eager set, or the start scan of one that restores on demand.
    Pass,
    /// The launch's serve of the mob it declared.
    Launch,
    /// Something that originates work for the store's mob (a send, a
    /// schedule fire, a delivery, an announcement): it loads a dormant mob
    /// and never a stopped one. A read of the mob and an operator's verb
    /// (get, resume, redefine, destroy) are lookups and load a stopped mob
    /// too.
    Demand,
}

/// What a managed mob's actor answered when it was asked to close if idle.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MobStoreUnloadOutcome {
    /// The mob was idle: its actor closed its admission and was shut down.
    /// The store is dormant again.
    #[default]
    Closed,
    /// The mob was busy, or its actor did not close for the unload: the
    /// mob stays managed.
    Busy,
}

/// How a create of a store's mob ended.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MobStoreCreateOutcome {
    /// The mob was built and is managed.
    #[default]
    Created,
    /// The create ended without a mob: it was refused, it failed, or its
    /// caller went away. The id is free again.
    Failed,
}

/// Whether the mob of a store that is not loaded is stopped.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MobStoreStop {
    /// Not known: nothing wrote it since the record was filed or since its
    /// mob was last loaded.
    #[default]
    Unread,
    /// The mob is not stopped: a demand loads it.
    NotStopped,
    /// The mob is stopped: only a lookup loads it.
    Stopped,
}

/// Why an input was refused. No refusal changes the record.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MobStoreRestoreRefusal {
    /// A re-attempt of this store's restore is already in flight.
    #[default]
    RetryInFlight,
    /// The record holds no quarantine to re-attempt.
    NotQuarantined,
    /// A re-attempt outcome arrived while no re-attempt was in flight.
    NotRetrying,
    /// An on-demand re-attempt of a store that needs a repair first: only
    /// an explicit retry re-attempts it.
    NotRetryable,
    /// A load of this store is already in flight: the caller waits for it.
    LoadInFlight,
    /// The store is not dormant, so there is nothing to load.
    NotDormant,
    /// A load outcome arrived while no load was in flight.
    NotLoading,
    /// The store's mob is not managed.
    NotManaged,
    /// The store's mob is not held.
    NotHeld,
    /// The release did not resolve the hold: the mob stays held.
    HoldKept,
    /// The store cannot be loaded again (it is volatile, or a host's own
    /// handle), so its mob is never unloaded.
    NotReloadable,
    /// An unload of this store's mob is admitted and not resolved yet: the
    /// caller waits for it.
    UnloadInFlight,
    /// An unload outcome arrived while no unload was admitted.
    NotUnloading,
    /// The set already hosts this store.
    AlreadyHosted,
    /// The store is gone for good: the record admits nothing more.
    Gone,
    /// The store is its launch's and the launch has not made its first
    /// serve: what addresses the mob waits for that serve to end.
    AwaitsLaunch,
    /// The store is its launch's: no pass of the set loads it, no other
    /// create or handle takes its id, and it is not destroyed through the
    /// set.
    LaunchOwned,
    /// The end of a wait for the launch arrived for a record that awaits
    /// no launch.
    NotAwaitingLaunch,
    /// A create named the launch for a store that was never declared: a
    /// store becomes the declared one by `Declare` alone.
    NotDeclared,
    /// A create of this store's mob is in flight: the id is taken by it.
    CreateInFlight,
    /// A create outcome arrived while no create was in flight.
    NotCreating,
    /// The store's mob is stopped and not loaded: nothing that originates
    /// work for it loads it.
    Stopped,
    /// Whether the store's mob is stopped is not known yet: the set reads
    /// the store before a demand loads it.
    StopUnread,
    /// A read of the stored stop arrived for a record that knows it.
    StopKnown,
}

machine! {
    machine MobStoreRestoreLifecycleMachine {
        version: 7,
        rust: "self" / "catalog::dsl::mob_store_restore_lifecycle",

        state {
            lifecycle_phase: MobStoreRestoreLifecyclePhase,
            // Why the store is quarantined. Meaningful while the record is
            // Quarantined or Retrying (the class the last failed attempt
            // met); the default in every other phase.
            class: Enum<MobQuarantineClass>,
            // Re-attempts of a quarantined store's restore that ended, in
            // either outcome, since the store was last listed.
            attempts: u64,
            // How the store came to the set. Only a persisted or a declared
            // store is ever dormant, loading, held or quarantined, and only
            // a persisted one is ever unloading.
            origin: Enum<MobStoreOrigin>,
            // Whether the mob's handle may be handed out. True only
            // while the record is Managed: never once an unload is admitted.
            served: bool,
            // Whether the store's mob still waits for its launch's first
            // serve. True only for a declared store, from its declaration
            // until that serve has ended (however it ended) or the host
            // stood down; never true again in the same process.
            awaits_launch: bool,
            // Whether the declared store is known to hold no mob: it was
            // declared as holding none (a mob that lives in memory, before
            // its create), or a load of it found none. Nothing is loaded
            // from it then, and a lookup of its mob finds none; the launch
            // creates the mob in it. Never true for any other store, and
            // only while the record is Dormant or its load is in flight.
            vacant: bool,
            // Whether the store's mob, which is not loaded, is stopped: as
            // the mob itself answered when its actor closed at an unload,
            // or as the set read it from the store. `Unread` until one of
            // the two wrote it, and again from every step that leaves
            // Dormant and Loading: a loaded mob speaks for itself.
            stop: Enum<MobStoreStop>,
            // Whether the record stands for a store or a mob the set
            // knows. False only for a record that was filed and has
            // admitted nothing yet: the one the set files for a create.
            // A census, a registration, a declaration, a load, a read of
            // the stop and the create itself make it true, and nothing
            // makes it false again.
            known: bool,
        }

        init(Dormant) {
            class = MobQuarantineClass::StoreUnavailable,
            attempts = 0,
            origin = MobStoreOrigin::Persisted,
            served = false,
            awaits_launch = false,
            vacant = false,
            stop = MobStoreStop::Unread,
            known = false,
        }

        terminal [Gone]

        phase MobStoreRestoreLifecyclePhase {
            // The set knows the store and nothing of it is loaded: no actor
            // runs for its mob and its content was not read.
            Dormant,
            // One load of the store is in flight.
            Loading,
            // The store's mob runs and is managed.
            Managed,
            // An idle unload of the managed mob is admitted: its actor is
            // being asked whether it is idle, or is being shut down. The mob
            // still runs and is not handed out.
            Unloading,
            // The store's mob is a child held before activation until its
            // owner resolves: no actor runs for it.
            Held,
            // The store is quarantined under its class.
            Quarantined,
            // One re-attempt of the quarantined store's restore is in flight.
            Retrying,
            // The store is gone for good: its mob was destroyed, or the
            // store held no mob and was swept.
            Gone,
            // A create of the store's mob is in flight: the id is taken by
            // it. Nothing of the mob is served, and no store was loaded.
            Creating,
        }

        input MobStoreRestoreLifecycleInput {
            // A census pass listed the store's file.
            Census,
            // A handle a host inserted joins the set: a mob the host built
            // itself. A create joins through its own record
            // (`RequestCreate`, `ResolveCreate`). `holder_ended` says the
            // mob the record manages has ended without a destroy: its actor
            // is over and it was not destroyed (a mob that is being
            // destroyed keeps its record until its destroy has recorded
            // itself). It is read only over a mob the set itself runs.
            Register { holder_ended: bool },
            // The host declares the store of the mob its launch names: the
            // record is that store's from here on, and its mob waits for the
            // launch's first serve. `vacant` says the store is known to hold
            // no mob (a mob that lives in memory has no store to load).
            Declare { vacant: bool },
            // The wait for the launch ended: its first serve ended, however
            // it ended, or the host stood down.
            EndLaunchWait,
            // Ask for the one load of a dormant store, naming what asks.
            RequestLoad { trigger: Enum<MobStoreLoadTrigger> },
            // The load the machine admitted ended. `class` is the class a
            // failed load met; it is ignored for any other outcome.
            ResolveLoad { outcome: Enum<MobStoreRestoreOutcome>, class: Enum<MobQuarantineClass> },
            // Ask for one re-attempt of a quarantined store's restore,
            // naming what asks.
            RequestRetry { trigger: Enum<MobStoreRetryTrigger> },
            // The re-attempt the machine admitted ended. `class` is the class
            // a failed attempt met; it is ignored for any other outcome.
            ResolveRetry { outcome: Enum<MobStoreRestoreOutcome>, class: Enum<MobQuarantineClass> },
            // Ask to put the idle question to the managed mob's actor. From
            // its admission on the mob's handle is not handed out.
            RequestUnload,
            // The actor answered the admitted unload: it closed idle and was
            // shut down, or it stays managed. `stop` is what the mob
            // itself answered about its stop when it closed (`Unread`
            // when that was not read); it is ignored for a mob that stays
            // managed.
            ResolveUnload { outcome: Enum<MobStoreUnloadOutcome>, stop: Enum<MobStoreStop> },
            // A held mob left its hold: it resumed and is managed, or its
            // store is gone.
            ReleaseHeld { outcome: Enum<MobStoreRestoreOutcome> },
            // The managed mob was destroyed.
            Destroy,
            // Ask to create the store's mob: the create takes the id. For a
            // record the set files for this create alone, and for the
            // declared record when the launch creates its mob (`launch`).
            RequestCreate { launch: bool },
            // The create the machine admitted ended. `in_memory` says that
            // a created mob has no store file; it is ignored for a failed
            // create and for the declared record.
            ResolveCreate { outcome: Enum<MobStoreCreateOutcome>, in_memory: bool },
            // The set read, from the store of a mob that is not loaded,
            // whether the mob is stopped.
            ReadStop { stopped: bool },
        }

        effect MobStoreRestoreLifecycleEffect {
            StoreListed,
            StoreRegistered { origin: Enum<MobStoreOrigin> },
            LaunchDeclared,
            LaunchWaitEnded,
            LoadAdmitted,
            StoreLoaded { outcome: Enum<MobStoreRestoreOutcome> },
            StoreQuarantined { class: Enum<MobQuarantineClass> },
            RetryAdmitted { attempts: u64 },
            StoreReleased { outcome: Enum<MobStoreRestoreOutcome>, attempts: u64 },
            StoreStillQuarantined { class: Enum<MobQuarantineClass>, attempts: u64 },
            UnloadAdmitted,
            StoreUnloaded,
            UnloadDeclined,
            HoldReleased { outcome: Enum<MobStoreRestoreOutcome> },
            StoreGone,
            CreateAdmitted,
            CreateReleased,
            StopRead { stopped: bool },
            RequestRefused { reason: Enum<MobStoreRestoreRefusal> },
        }

        // The class is a fact of a quarantine only.
        invariant only_a_quarantine_holds_a_class {
            self.lifecycle_phase == Phase::Quarantined
                || self.lifecycle_phase == Phase::Retrying
                || self.class == MobQuarantineClass::StoreUnavailable
        }

        // A store the set cannot load again is managed or gone, never
        // waiting to be loaded. A declared store waits like a persisted
        // one: for its launch first, and after that for whatever addresses
        // its mob.
        invariant only_a_persisted_store_waits_unloaded {
            self.origin == MobStoreOrigin::Persisted
                || self.origin == MobStoreOrigin::Declared
                || self.lifecycle_phase == Phase::Managed
                || self.lifecycle_phase == Phase::Gone
        }

        // The wait for a launch is a fact of a declared store only.
        invariant only_a_declared_store_awaits_its_launch {
            !self.awaits_launch || self.origin == MobStoreOrigin::Declared
        }

        // A store that is gone waits for nothing: nobody is left waiting on
        // a record that admits nothing more.
        invariant a_store_that_is_gone_awaits_no_launch {
            !self.awaits_launch || self.lifecycle_phase != Phase::Gone
        }

        // An unload is admitted for a persisted store only, so it is never
        // admitted for a store that awaits its launch.
        invariant a_store_whose_unload_is_admitted_awaits_no_launch {
            !self.awaits_launch || self.lifecycle_phase != Phase::Unloading
        }

        // A declared store's record lasts as long as the declaration: the
        // id stays its launch's whatever its store holds.
        invariant a_declared_store_is_never_gone {
            self.origin != MobStoreOrigin::Declared || self.lifecycle_phase != Phase::Gone
        }

        // "Holds no mob" is a fact of a declared store that is not loaded.
        invariant only_a_declared_store_that_is_not_loaded_is_vacant {
            !self.vacant
                || (self.origin == MobStoreOrigin::Declared
                    && (self.lifecycle_phase == Phase::Dormant
                        || self.lifecycle_phase == Phase::Loading
                        || self.lifecycle_phase == Phase::Creating))
        }

        // What the record says of its mob's stop is a fact of a mob that is
        // not loaded: a loaded mob speaks for itself, and a held, quarantined
        // or destroyed one, or one that is being created, has no such answer.
        invariant only_a_store_that_is_not_loaded_knows_its_mobs_stop {
            self.stop == MobStoreStop::Unread
                || self.lifecycle_phase == Phase::Dormant
                || self.lifecycle_phase == Phase::Loading
        }

        // A mob's handle is handed out exactly while its record is Managed:
        // `served` is true in Managed and in no other phase. Both halves are
        // one fact. A handle handed out in another phase would reach a mob
        // that is being unloaded or is not there; a managed record that is
        // not served would answer a lookup as if the mob did not exist.
        invariant a_mob_is_handed_out_exactly_while_managed {
            (self.served && self.lifecycle_phase == Phase::Managed)
                || (!self.served && self.lifecycle_phase != Phase::Managed)
        }

        // A record that stands for nothing yet is exactly as it was filed:
        // every step that makes it stand for a store or a mob says so.
        invariant a_record_that_stands_for_nothing_is_as_it_was_filed {
            self.known
                || (self.lifecycle_phase == Phase::Dormant
                    && self.origin == MobStoreOrigin::Persisted
                    && !self.served
                    && !self.awaits_launch
                    && !self.vacant
                    && self.stop == MobStoreStop::Unread
                    && self.attempts == 0
                    && self.class == MobQuarantineClass::StoreUnavailable)
        }

        // Verdict effects: the mob-set state mirrors them as its typed result.
        disposition StoreListed => local seam SurfaceResultAlignment,
        disposition StoreRegistered => local seam SurfaceResultAlignment,
        disposition LaunchDeclared => local seam SurfaceResultAlignment,
        disposition LaunchWaitEnded => local seam SurfaceResultAlignment,
        disposition LoadAdmitted => local seam SurfaceResultAlignment,
        disposition StoreLoaded => local seam SurfaceResultAlignment,
        disposition StoreQuarantined => local seam SurfaceResultAlignment,
        disposition RetryAdmitted => local seam SurfaceResultAlignment,
        disposition StoreReleased => local seam SurfaceResultAlignment,
        disposition StoreStillQuarantined => local seam SurfaceResultAlignment,
        disposition UnloadAdmitted => local seam SurfaceResultAlignment,
        disposition StoreUnloaded => local seam SurfaceResultAlignment,
        disposition UnloadDeclined => local seam SurfaceResultAlignment,
        disposition HoldReleased => local seam SurfaceResultAlignment,
        disposition StoreGone => local seam SurfaceResultAlignment,
        disposition CreateAdmitted => local seam SurfaceResultAlignment,
        disposition CreateReleased => local seam SurfaceResultAlignment,
        disposition StopRead => local seam SurfaceResultAlignment,
        disposition RequestRefused => local seam SurfaceResultAlignment,

        // ------------------------------------------------------------------
        // Census
        // A census pass lists a store file. A dormant store stays dormant; a
        // quarantined store is found afresh (a later pass judges it again at
        // its next load). A store the set already loads, manages, holds or
        // re-attempts is not listed again.
        // ------------------------------------------------------------------

        transition CensusDormant {
            on input Census {}
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            update {
                self.known = true;
            }
            to Dormant
            emit StoreListed
        }

        transition CensusQuarantined {
            on input Census {}
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {
                self.class = MobQuarantineClass::StoreUnavailable;
                self.attempts = 0;
            }
            to Dormant
            emit StoreListed
        }

        transition CensusLoading {
            on input Census {}
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition CensusManaged {
            on input Census {}
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition CensusHeld {
            on input Census {}
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition CensusRetrying {
            on input Census {}
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition CensusUnloading {
            on input Census {}
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition CensusGone {
            on input Census {}
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        transition CensusCreating {
            on input Census {}
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        // ------------------------------------------------------------------
        // Register
        // A handle a host inserted joins the set as managed: a mob the host
        // built itself. (A create joins through its own record:
        // `RequestCreate`, `ResolveCreate`.) Under the id of a mob that is
        // not loaded the handle takes the record. It also replaces the
        // handle of a mob a host inserted before: the host inserts its own
        // mob again. A mob the set itself runs (a store it loaded, a mob it
        // created) keeps its id against a host's handle: its actor runs
        // for the set, and a handle registered over it would leave that
        // actor with nobody holding it. Once that mob has ended without a
        // destroy (its actor is over and it was not destroyed, which the
        // registration says: `holder_ended`) nothing is left to hold, and
        // the host's handle takes the id. A mob that is being destroyed is
        // not told as ended: the record stays its own until its destroy
        // has recorded itself. A declared
        // store is its launch's: no host's handle joins under it or
        // replaces its mob.
        // ------------------------------------------------------------------

        transition RegisterDormant {
            on input Register { holder_ended }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "not_declared" { self.origin != MobStoreOrigin::Declared }
            update {
                self.known = true;
                self.origin = MobStoreOrigin::HostHandle;
                self.served = true;
                self.vacant = false;
                self.stop = MobStoreStop::Unread;
            }
            to Managed
            emit StoreRegistered { origin: self.origin }
        }

        transition RegisterDormantLaunchOwned {
            on input Register { holder_ended }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::LaunchOwned }
        }

        transition RegisterManagedHostHandle {
            on input Register { holder_ended }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            guard "a_hosts_own_mob" { self.origin == MobStoreOrigin::HostHandle }
            update {}
            to Managed
            emit StoreRegistered { origin: self.origin }
        }

        // The mob is run by the set: the first holder keeps the id.
        transition RegisterManagedHostHandleSetRun {
            on input Register { holder_ended }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            guard "run_by_the_set" { self.origin == MobStoreOrigin::Persisted || self.origin == MobStoreOrigin::Volatile }
            guard "its_mob_runs" { holder_ended == false }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        // The mob the set ran has ended without a destroy: its actor is
        // over, so nothing is left running with nobody holding it, and no
        // destroy is still to record itself on this record. The host's
        // handle takes the id, as it does under the id of a mob that is
        // not loaded.
        transition RegisterManagedHostHandleOverEnded {
            on input Register { holder_ended }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            guard "run_by_the_set" { self.origin == MobStoreOrigin::Persisted || self.origin == MobStoreOrigin::Volatile }
            guard "its_mob_has_ended" { holder_ended == true }
            update {
                self.origin = MobStoreOrigin::HostHandle;
            }
            to Managed
            emit StoreRegistered { origin: self.origin }
        }

        transition RegisterManagedLaunchOwned {
            on input Register { holder_ended }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::LaunchOwned }
        }

        transition RegisterLoading {
            on input Register { holder_ended }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition RegisterHeld {
            on input Register { holder_ended }
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition RegisterQuarantined {
            on input Register { holder_ended }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition RegisterRetrying {
            on input Register { holder_ended }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition RegisterUnloading {
            on input Register { holder_ended }
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::UnloadInFlight }
        }

        transition RegisterGone {
            on input Register { holder_ended }
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        // A create took the id first: nothing registers over it, a host's
        // handle included.
        transition RegisterCreating {
            on input Register { holder_ended }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        // ------------------------------------------------------------------
        // Declare
        // The host declares the store of the mob its launch names. The
        // record of a store the set does not load, manage, hold or
        // quarantine becomes the declared one, and its mob waits for the
        // launch's first serve. A store is declared once.
        // ------------------------------------------------------------------

        transition DeclareDormant {
            on input Declare { vacant }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "not_declared" { self.origin != MobStoreOrigin::Declared }
            update {
                self.known = true;
                self.origin = MobStoreOrigin::Declared;
                self.awaits_launch = true;
                self.vacant = vacant;
            }
            to Dormant
            emit LaunchDeclared
        }

        transition DeclareDormantDeclared {
            on input Declare { vacant }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition DeclareLoading {
            on input Declare { vacant }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition DeclareManaged {
            on input Declare { vacant }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition DeclareUnloading {
            on input Declare { vacant }
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition DeclareHeld {
            on input Declare { vacant }
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition DeclareQuarantined {
            on input Declare { vacant }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition DeclareRetrying {
            on input Declare { vacant }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition DeclareGone {
            on input Declare { vacant }
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        transition DeclareCreating {
            on input Declare { vacant }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        // ------------------------------------------------------------------
        // EndLaunchWait
        // The launch's first serve ended, however it ended, or the host
        // stood down: nothing waits for the launch any more, in whatever
        // phase the serve left the record. The wait ends once.
        // ------------------------------------------------------------------

        transition EndLaunchWaitDormant {
            on input EndLaunchWait {}
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "awaits_launch" { self.awaits_launch }
            update {
                self.awaits_launch = false;
            }
            to Dormant
            emit LaunchWaitEnded
        }

        transition EndLaunchWaitDormantNotAwaited {
            on input EndLaunchWait {}
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "not_awaiting_launch" { !self.awaits_launch }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotAwaitingLaunch }
        }

        transition EndLaunchWaitLoading {
            on input EndLaunchWait {}
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            guard "awaits_launch" { self.awaits_launch }
            update {
                self.awaits_launch = false;
            }
            to Loading
            emit LaunchWaitEnded
        }

        transition EndLaunchWaitLoadingNotAwaited {
            on input EndLaunchWait {}
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            guard "not_awaiting_launch" { !self.awaits_launch }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotAwaitingLaunch }
        }

        transition EndLaunchWaitManaged {
            on input EndLaunchWait {}
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            guard "awaits_launch" { self.awaits_launch }
            update {
                self.awaits_launch = false;
            }
            to Managed
            emit LaunchWaitEnded
        }

        transition EndLaunchWaitManagedNotAwaited {
            on input EndLaunchWait {}
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            guard "not_awaiting_launch" { !self.awaits_launch }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotAwaitingLaunch }
        }

        // Only a persisted store has an unload admitted, and only a
        // declared one awaits a launch: a store whose unload is admitted
        // has no wait to end
        // (`a_store_whose_unload_is_admitted_awaits_no_launch`).
        transition EndLaunchWaitUnloading {
            on input EndLaunchWait {}
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotAwaitingLaunch }
        }

        transition EndLaunchWaitHeld {
            on input EndLaunchWait {}
            guard "held" { self.lifecycle_phase == Phase::Held }
            guard "awaits_launch" { self.awaits_launch }
            update {
                self.awaits_launch = false;
            }
            to Held
            emit LaunchWaitEnded
        }

        transition EndLaunchWaitHeldNotAwaited {
            on input EndLaunchWait {}
            guard "held" { self.lifecycle_phase == Phase::Held }
            guard "not_awaiting_launch" { !self.awaits_launch }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotAwaitingLaunch }
        }

        transition EndLaunchWaitQuarantined {
            on input EndLaunchWait {}
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            guard "awaits_launch" { self.awaits_launch }
            update {
                self.awaits_launch = false;
            }
            to Quarantined
            emit LaunchWaitEnded
        }

        transition EndLaunchWaitQuarantinedNotAwaited {
            on input EndLaunchWait {}
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            guard "not_awaiting_launch" { !self.awaits_launch }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotAwaitingLaunch }
        }

        transition EndLaunchWaitRetrying {
            on input EndLaunchWait {}
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            guard "awaits_launch" { self.awaits_launch }
            update {
                self.awaits_launch = false;
            }
            to Retrying
            emit LaunchWaitEnded
        }

        transition EndLaunchWaitRetryingNotAwaited {
            on input EndLaunchWait {}
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            guard "not_awaiting_launch" { !self.awaits_launch }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotAwaitingLaunch }
        }

        transition EndLaunchWaitGone {
            on input EndLaunchWait {}
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        // The launch creates its mob while the record still waits for the
        // launch's serve: the wait may end while the create is in flight.
        transition EndLaunchWaitCreating {
            on input EndLaunchWait {}
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            guard "awaited" { self.awaits_launch == true }
            update {
                self.awaits_launch = false;
            }
            to Creating
            emit LaunchWaitEnded
        }

        transition EndLaunchWaitCreatingNotAwaited {
            on input EndLaunchWait {}
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            guard "not_awaited" { self.awaits_launch == false }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotAwaitingLaunch }
        }

        // ------------------------------------------------------------------
        // RequestLoad
        // At most one load of a dormant store is in flight: a second request
        // is refused at once, and its caller waits for the first. A declared
        // store is loaded first by its launch's serve: until that serve has
        // ended a lookup's request is refused and its caller waits for the
        // end of the wait, and a pass of the set never loads it. The trigger
        // decides nothing for any other store.
        // ------------------------------------------------------------------

        transition RequestLoadDormant {
            on input RequestLoad { trigger }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "admitted" {
                self.origin != MobStoreOrigin::Declared
                    || trigger == MobStoreLoadTrigger::Launch
                    || (trigger == MobStoreLoadTrigger::Lookup && !self.awaits_launch && !self.vacant)
            }
            guard "not_a_demand" { trigger != MobStoreLoadTrigger::Demand }
            update {
                self.known = true;
            }
            to Loading
            emit LoadAdmitted
        }

        transition RequestLoadDormantAwaitsLaunch {
            on input RequestLoad { trigger }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            guard "lookup" { trigger == MobStoreLoadTrigger::Lookup || trigger == MobStoreLoadTrigger::Demand }
            guard "awaits_launch" { self.awaits_launch }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::AwaitsLaunch }
        }

        transition RequestLoadDormantVacant {
            on input RequestLoad { trigger }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            guard "lookup" { trigger == MobStoreLoadTrigger::Lookup || trigger == MobStoreLoadTrigger::Demand }
            guard "not_awaiting_launch" { !self.awaits_launch }
            guard "vacant" { self.vacant }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }

        transition RequestLoadDormantLaunchOwned {
            on input RequestLoad { trigger }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            guard "pass" { trigger == MobStoreLoadTrigger::Pass }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::LaunchOwned }
        }

        // A demand loads a dormant mob that is known not to be stopped. For
        // the declared store it asks as a lookup does: not while the mob
        // waits for its launch, and not when the store holds no mob.
        transition RequestLoadDormantDemand {
            on input RequestLoad { trigger }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "demand" { trigger == MobStoreLoadTrigger::Demand }
            guard "serves_a_lookup" { self.origin != MobStoreOrigin::Declared || (!self.awaits_launch && !self.vacant) }
            guard "not_stopped" { self.stop == MobStoreStop::NotStopped }
            update {}
            to Loading
            emit LoadAdmitted
        }

        // A stopped mob is not loaded by what originates work for it.
        transition RequestLoadDormantStopped {
            on input RequestLoad { trigger }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "demand" { trigger == MobStoreLoadTrigger::Demand }
            guard "serves_a_lookup" { self.origin != MobStoreOrigin::Declared || (!self.awaits_launch && !self.vacant) }
            guard "stopped" { self.stop == MobStoreStop::Stopped }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::Stopped }
        }

        // Whether the mob is stopped is not known: the set reads its store
        // before a demand loads it.
        transition RequestLoadDormantStopUnread {
            on input RequestLoad { trigger }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "demand" { trigger == MobStoreLoadTrigger::Demand }
            guard "serves_a_lookup" { self.origin != MobStoreOrigin::Declared || (!self.awaits_launch && !self.vacant) }
            guard "unread" { self.stop == MobStoreStop::Unread }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::StopUnread }
        }

        transition RequestLoadLoading {
            on input RequestLoad { trigger }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::LoadInFlight }
        }

        transition RequestLoadManaged {
            on input RequestLoad { trigger }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }

        transition RequestLoadHeld {
            on input RequestLoad { trigger }
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }

        transition RequestLoadQuarantined {
            on input RequestLoad { trigger }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }

        transition RequestLoadRetrying {
            on input RequestLoad { trigger }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::RetryInFlight }
        }

        transition RequestLoadUnloading {
            on input RequestLoad { trigger }
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::UnloadInFlight }
        }

        transition RequestLoadGone {
            on input RequestLoad { trigger }
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        transition RequestLoadCreating {
            on input RequestLoad { trigger }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }

        // ------------------------------------------------------------------
        // ResolveLoad
        // The admitted load ended: the mob is managed or held, the store is
        // gone, or the store is quarantined under the class the load met.
        // ------------------------------------------------------------------

        transition ResolveLoadManaged {
            on input ResolveLoad { outcome, class }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            guard "managed" { outcome == MobStoreRestoreOutcome::Managed }
            update {
                self.served = true;
                self.vacant = false;
                self.stop = MobStoreStop::Unread;
            }
            to Managed
            emit StoreLoaded { outcome: outcome }
        }

        transition ResolveLoadHeld {
            on input ResolveLoad { outcome, class }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            guard "held" { outcome == MobStoreRestoreOutcome::Held }
            update {
                self.vacant = false;
                self.stop = MobStoreStop::Unread;
            }
            to Held
            emit StoreLoaded { outcome: outcome }
        }

        transition ResolveLoadSettled {
            on input ResolveLoad { outcome, class }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            guard "settled" { outcome == MobStoreRestoreOutcome::Settled }
            guard "not_declared" { self.origin != MobStoreOrigin::Declared }
            update {
                self.stop = MobStoreStop::Unread;
            }
            to Gone
            emit StoreLoaded { outcome: outcome }
        }

        transition ResolveLoadSettledDeclared {
            on input ResolveLoad { outcome, class }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            guard "settled" { outcome == MobStoreRestoreOutcome::Settled }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            update {
                self.vacant = true;
                self.stop = MobStoreStop::Unread;
            }
            to Dormant
            emit StoreLoaded { outcome: outcome }
        }

        transition ResolveLoadFailed {
            on input ResolveLoad { outcome, class }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            guard "failed" { outcome == MobStoreRestoreOutcome::Failed }
            update {
                self.class = class;
                self.vacant = false;
                self.stop = MobStoreStop::Unread;
            }
            to Quarantined
            emit StoreQuarantined { class: self.class }
        }

        transition ResolveLoadDormant {
            on input ResolveLoad { outcome, class }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotLoading }
        }

        transition ResolveLoadWhileManaged {
            on input ResolveLoad { outcome, class }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotLoading }
        }

        transition ResolveLoadWhileHeld {
            on input ResolveLoad { outcome, class }
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotLoading }
        }

        transition ResolveLoadQuarantined {
            on input ResolveLoad { outcome, class }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotLoading }
        }

        transition ResolveLoadRetrying {
            on input ResolveLoad { outcome, class }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotLoading }
        }

        transition ResolveLoadUnloading {
            on input ResolveLoad { outcome, class }
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotLoading }
        }

        transition ResolveLoadGone {
            on input ResolveLoad { outcome, class }
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        transition ResolveLoadCreating {
            on input ResolveLoad { outcome, class }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotLoading }
        }

        // ------------------------------------------------------------------
        // RequestRetry
        // An explicit request re-attempts a store of any class; an on-demand
        // request only a store that could not serve now. At most one
        // re-attempt is in flight; a second request is refused at once and
        // never waits for the first. A declared store whose launch has not
        // ended its first serve is not re-attempted on demand: the launch
        // decides what that serve does with a store it found quarantined.
        // ------------------------------------------------------------------

        transition RequestRetryQuarantined {
            on input RequestRetry { trigger }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            guard "retryable" { trigger == MobStoreRetryTrigger::Explicit || self.class == MobQuarantineClass::StoreUnavailable }
            guard "not_awaiting_launch" { trigger == MobStoreRetryTrigger::Explicit || !self.awaits_launch }
            update {}
            to Retrying
            emit RetryAdmitted { attempts: self.attempts }
        }

        transition RequestRetryQuarantinedNotRetryable {
            on input RequestRetry { trigger }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            guard "on_demand" { trigger == MobStoreRetryTrigger::OnDemand }
            guard "needs_repair" { self.class != MobQuarantineClass::StoreUnavailable }
            guard "not_awaiting_launch" { !self.awaits_launch }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotRetryable }
        }

        transition RequestRetryQuarantinedAwaitsLaunch {
            on input RequestRetry { trigger }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            guard "on_demand" { trigger == MobStoreRetryTrigger::OnDemand }
            guard "awaits_launch" { self.awaits_launch }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::AwaitsLaunch }
        }

        transition RequestRetryRetrying {
            on input RequestRetry { trigger }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::RetryInFlight }
        }

        transition RequestRetryDormant {
            on input RequestRetry { trigger }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotQuarantined }
        }

        transition RequestRetryLoading {
            on input RequestRetry { trigger }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotQuarantined }
        }

        transition RequestRetryManaged {
            on input RequestRetry { trigger }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotQuarantined }
        }

        transition RequestRetryHeld {
            on input RequestRetry { trigger }
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotQuarantined }
        }

        transition RequestRetryUnloading {
            on input RequestRetry { trigger }
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotQuarantined }
        }

        transition RequestRetryGone {
            on input RequestRetry { trigger }
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        transition RequestRetryCreating {
            on input RequestRetry { trigger }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotQuarantined }
        }

        // ------------------------------------------------------------------
        // ResolveRetry
        // The admitted re-attempt ended: a failure keeps the store
        // quarantined under the class it met; any other outcome releases it
        // into the phase the outcome names.
        // ------------------------------------------------------------------

        transition ResolveRetryFailed {
            on input ResolveRetry { outcome, class }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            guard "failed" { outcome == MobStoreRestoreOutcome::Failed }
            update {
                self.class = class;
                self.attempts += 1;
            }
            to Quarantined
            emit StoreStillQuarantined { class: self.class, attempts: self.attempts }
        }

        transition ResolveRetryManaged {
            on input ResolveRetry { outcome, class }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            guard "managed" { outcome == MobStoreRestoreOutcome::Managed }
            update {
                self.class = MobQuarantineClass::StoreUnavailable;
                self.attempts += 1;
                self.served = true;
            }
            to Managed
            emit StoreReleased { outcome: outcome, attempts: self.attempts }
        }

        transition ResolveRetryHeld {
            on input ResolveRetry { outcome, class }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            guard "held" { outcome == MobStoreRestoreOutcome::Held }
            update {
                self.class = MobQuarantineClass::StoreUnavailable;
                self.attempts += 1;
            }
            to Held
            emit StoreReleased { outcome: outcome, attempts: self.attempts }
        }

        transition ResolveRetrySettled {
            on input ResolveRetry { outcome, class }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            guard "settled" { outcome == MobStoreRestoreOutcome::Settled }
            guard "not_declared" { self.origin != MobStoreOrigin::Declared }
            update {
                self.class = MobQuarantineClass::StoreUnavailable;
                self.attempts += 1;
            }
            to Gone
            emit StoreReleased { outcome: outcome, attempts: self.attempts }
        }

        transition ResolveRetrySettledDeclared {
            on input ResolveRetry { outcome, class }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            guard "settled" { outcome == MobStoreRestoreOutcome::Settled }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            update {
                self.class = MobQuarantineClass::StoreUnavailable;
                self.attempts += 1;
                self.vacant = true;
            }
            to Dormant
            emit StoreReleased { outcome: outcome, attempts: self.attempts }
        }

        transition ResolveRetryDormant {
            on input ResolveRetry { outcome, class }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotRetrying }
        }

        transition ResolveRetryLoading {
            on input ResolveRetry { outcome, class }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotRetrying }
        }

        transition ResolveRetryWhileManaged {
            on input ResolveRetry { outcome, class }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotRetrying }
        }

        transition ResolveRetryWhileHeld {
            on input ResolveRetry { outcome, class }
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotRetrying }
        }

        transition ResolveRetryQuarantined {
            on input ResolveRetry { outcome, class }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotRetrying }
        }

        transition ResolveRetryUnloading {
            on input ResolveRetry { outcome, class }
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotRetrying }
        }

        transition ResolveRetryGone {
            on input ResolveRetry { outcome, class }
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        transition ResolveRetryCreating {
            on input ResolveRetry { outcome, class }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotRetrying }
        }

        // ------------------------------------------------------------------
        // RequestUnload
        // The idle question is put to a managed mob's actor only once the
        // record admitted it: a mob whose store can be loaded again. From
        // that step the mob's handle is not handed out, and a register, a
        // load request or a destroy that arrives waits for the answer.
        // ------------------------------------------------------------------

        transition RequestUnloadManaged {
            on input RequestUnload {}
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            guard "reloadable" { self.origin == MobStoreOrigin::Persisted }
            update {
                self.served = false;
            }
            to Unloading
            emit UnloadAdmitted
        }

        transition RequestUnloadManagedNotReloadable {
            on input RequestUnload {}
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            guard "not_reloadable" { self.origin != MobStoreOrigin::Persisted }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotReloadable }
        }

        transition RequestUnloadUnloading {
            on input RequestUnload {}
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::UnloadInFlight }
        }

        transition RequestUnloadDormant {
            on input RequestUnload {}
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotManaged }
        }

        transition RequestUnloadLoading {
            on input RequestUnload {}
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotManaged }
        }

        transition RequestUnloadHeld {
            on input RequestUnload {}
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotManaged }
        }

        transition RequestUnloadQuarantined {
            on input RequestUnload {}
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotManaged }
        }

        transition RequestUnloadRetrying {
            on input RequestUnload {}
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotManaged }
        }

        transition RequestUnloadGone {
            on input RequestUnload {}
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        transition RequestUnloadCreating {
            on input RequestUnload {}
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotManaged }
        }

        // ------------------------------------------------------------------
        // ResolveUnload
        // The actor answered the admitted unload: it closed idle and was
        // shut down, so the store is dormant again; or the mob stays
        // managed and is handed out again.
        // ------------------------------------------------------------------

        transition ResolveUnloadClosed {
            on input ResolveUnload { outcome, stop }
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            guard "closed" { outcome == MobStoreUnloadOutcome::Closed }
            update {
                self.stop = stop;
            }
            to Dormant
            emit StoreUnloaded
        }

        transition ResolveUnloadBusy {
            on input ResolveUnload { outcome, stop }
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            guard "busy" { outcome == MobStoreUnloadOutcome::Busy }
            update {
                self.served = true;
            }
            to Managed
            emit UnloadDeclined
        }

        transition ResolveUnloadDormant {
            on input ResolveUnload { outcome, stop }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotUnloading }
        }

        transition ResolveUnloadLoading {
            on input ResolveUnload { outcome, stop }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotUnloading }
        }

        transition ResolveUnloadWhileManaged {
            on input ResolveUnload { outcome, stop }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotUnloading }
        }

        transition ResolveUnloadHeld {
            on input ResolveUnload { outcome, stop }
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotUnloading }
        }

        transition ResolveUnloadQuarantined {
            on input ResolveUnload { outcome, stop }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotUnloading }
        }

        transition ResolveUnloadRetrying {
            on input ResolveUnload { outcome, stop }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotUnloading }
        }

        transition ResolveUnloadGone {
            on input ResolveUnload { outcome, stop }
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        transition ResolveUnloadCreating {
            on input ResolveUnload { outcome, stop }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotUnloading }
        }

        // ------------------------------------------------------------------
        // ReleaseHeld
        // A held mob leaves its hold in one step: it resumed and is managed,
        // or its store is gone. Until then the record stays Held, so a
        // release that is cancelled or fails leaves the mob held.
        // ------------------------------------------------------------------

        transition ReleaseHeldManaged {
            on input ReleaseHeld { outcome }
            guard "held" { self.lifecycle_phase == Phase::Held }
            guard "managed" { outcome == MobStoreRestoreOutcome::Managed }
            update {
                self.served = true;
            }
            to Managed
            emit HoldReleased { outcome: outcome }
        }

        transition ReleaseHeldSettled {
            on input ReleaseHeld { outcome }
            guard "held" { self.lifecycle_phase == Phase::Held }
            guard "settled" { outcome == MobStoreRestoreOutcome::Settled }
            guard "not_declared" { self.origin != MobStoreOrigin::Declared }
            update {}
            to Gone
            emit HoldReleased { outcome: outcome }
        }

        transition ReleaseHeldSettledDeclared {
            on input ReleaseHeld { outcome }
            guard "held" { self.lifecycle_phase == Phase::Held }
            guard "settled" { outcome == MobStoreRestoreOutcome::Settled }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            update {
                self.vacant = true;
            }
            to Dormant
            emit HoldReleased { outcome: outcome }
        }

        transition ReleaseHeldKept {
            on input ReleaseHeld { outcome }
            guard "held" { self.lifecycle_phase == Phase::Held }
            guard "unresolved" { outcome == MobStoreRestoreOutcome::Held || outcome == MobStoreRestoreOutcome::Failed }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::HoldKept }
        }

        transition ReleaseHeldDormant {
            on input ReleaseHeld { outcome }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotHeld }
        }

        transition ReleaseHeldLoading {
            on input ReleaseHeld { outcome }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotHeld }
        }

        transition ReleaseHeldWhileManaged {
            on input ReleaseHeld { outcome }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotHeld }
        }

        transition ReleaseHeldQuarantined {
            on input ReleaseHeld { outcome }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotHeld }
        }

        transition ReleaseHeldRetrying {
            on input ReleaseHeld { outcome }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotHeld }
        }

        transition ReleaseHeldUnloading {
            on input ReleaseHeld { outcome }
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotHeld }
        }

        transition ReleaseHeldGone {
            on input ReleaseHeld { outcome }
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        transition ReleaseHeldCreating {
            on input ReleaseHeld { outcome }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotHeld }
        }

        // ------------------------------------------------------------------
        // Destroy
        // A managed mob that was destroyed is gone for good. A declared
        // store's mob is its launch's for as long as the host runs it: it is
        // not destroyed through the set.
        // ------------------------------------------------------------------

        transition DestroyManaged {
            on input Destroy {}
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            guard "not_declared" { self.origin != MobStoreOrigin::Declared }
            update {
                self.served = false;
            }
            to Gone
            emit StoreGone
        }

        transition DestroyManagedLaunchOwned {
            on input Destroy {}
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::LaunchOwned }
        }

        transition DestroyDormant {
            on input Destroy {}
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotManaged }
        }

        transition DestroyLoading {
            on input Destroy {}
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotManaged }
        }

        transition DestroyHeld {
            on input Destroy {}
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotManaged }
        }

        transition DestroyQuarantined {
            on input Destroy {}
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotManaged }
        }

        transition DestroyRetrying {
            on input Destroy {}
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotManaged }
        }

        transition DestroyUnloading {
            on input Destroy {}
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::UnloadInFlight }
        }

        transition DestroyGone {
            on input Destroy {}
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        transition DestroyCreating {
            on input Destroy {}
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotManaged }
        }

        // ------------------------------------------------------------------
        // RequestCreate
        // A create takes its mob's id before anything of the mob is built:
        // the admission of a host, the storage and the build all come after
        // this step. The set files a record for the create in the step that
        // finds no record under the id, and asks that record here; the
        // launch's create asks the declared record, which holds no mob. A
        // record that stands for a store or a mob the set already knows
        // refuses, and so does one whose create is in flight.
        // ------------------------------------------------------------------

        transition RequestCreateDormant {
            on input RequestCreate { launch }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "not_declared" { self.origin != MobStoreOrigin::Declared }
            guard "not_the_launch" { launch == false }
            guard "stands_for_nothing_yet" { self.known == false }
            update {
                self.known = true;
                self.stop = MobStoreStop::Unread;
            }
            to Creating
            emit CreateAdmitted
        }

        // The record stands for a store or a mob the set knows: a census
        // listed it, or its mob was loaded, registered or created and is
        // not loaded now. The id is that store's.
        transition RequestCreateDormantKnown {
            on input RequestCreate { launch }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "not_declared" { self.origin != MobStoreOrigin::Declared }
            guard "not_the_launch" { launch == false }
            guard "stands_for_a_store" { self.known == true }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        // A create that names the launch joins under the declared record
        // only: a store becomes the declared one by `Declare` alone.
        transition RequestCreateDormantNotDeclared {
            on input RequestCreate { launch }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "not_declared" { self.origin != MobStoreOrigin::Declared }
            guard "the_launch" { launch == true }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDeclared }
        }

        transition RequestCreateDormantLaunch {
            on input RequestCreate { launch }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            guard "the_launch" { launch == true }
            guard "holds_no_mob" { self.vacant == true }
            update {
                self.stop = MobStoreStop::Unread;
            }
            to Creating
            emit CreateAdmitted
        }

        // The declared store holds a mob: the launch loads it, it creates
        // nothing.
        transition RequestCreateDormantLaunchHoldsAMob {
            on input RequestCreate { launch }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            guard "the_launch" { launch == true }
            guard "holds_a_mob" { self.vacant == false }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition RequestCreateDormantLaunchOwned {
            on input RequestCreate { launch }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            guard "not_the_launch" { launch == false }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::LaunchOwned }
        }

        transition RequestCreateLoading {
            on input RequestCreate { launch }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition RequestCreateManaged {
            on input RequestCreate { launch }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition RequestCreateUnloading {
            on input RequestCreate { launch }
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::UnloadInFlight }
        }

        transition RequestCreateHeld {
            on input RequestCreate { launch }
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition RequestCreateQuarantined {
            on input RequestCreate { launch }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition RequestCreateRetrying {
            on input RequestCreate { launch }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::AlreadyHosted }
        }

        transition RequestCreateGone {
            on input RequestCreate { launch }
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::Gone }
        }

        transition RequestCreateCreating {
            on input RequestCreate { launch }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::CreateInFlight }
        }

        // ------------------------------------------------------------------
        // ResolveCreate
        // The create the machine admitted ended. A created mob is managed
        // and served from this step, under the origin its create gives it:
        // a store file under the root, a mob in memory, or the declaration.
        // A create that ended without a mob frees the id: its record is
        // gone, and the declared record is known to hold no mob again.
        // ------------------------------------------------------------------

        transition ResolveCreateCreated {
            on input ResolveCreate { outcome, in_memory }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            guard "created" { outcome == MobStoreCreateOutcome::Created }
            guard "not_declared" { self.origin != MobStoreOrigin::Declared }
            guard "with_a_store_file" { in_memory == false }
            update {
                self.origin = MobStoreOrigin::Persisted;
                self.served = true;
            }
            to Managed
            emit StoreRegistered { origin: self.origin }
        }

        transition ResolveCreateCreatedInMemory {
            on input ResolveCreate { outcome, in_memory }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            guard "created" { outcome == MobStoreCreateOutcome::Created }
            guard "not_declared" { self.origin != MobStoreOrigin::Declared }
            guard "in_memory" { in_memory == true }
            update {
                self.origin = MobStoreOrigin::Volatile;
                self.served = true;
            }
            to Managed
            emit StoreRegistered { origin: self.origin }
        }

        transition ResolveCreateCreatedDeclared {
            on input ResolveCreate { outcome, in_memory }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            guard "created" { outcome == MobStoreCreateOutcome::Created }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            update {
                self.served = true;
                self.vacant = false;
            }
            to Managed
            emit StoreRegistered { origin: self.origin }
        }

        transition ResolveCreateFailed {
            on input ResolveCreate { outcome, in_memory }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            guard "failed" { outcome == MobStoreCreateOutcome::Failed }
            guard "not_declared" { self.origin != MobStoreOrigin::Declared }
            update {}
            to Gone
            emit CreateReleased
        }

        // The declared record lasts as long as the declaration: the store
        // holds no mob again, and the next serve creates it.
        transition ResolveCreateFailedDeclared {
            on input ResolveCreate { outcome, in_memory }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            guard "failed" { outcome == MobStoreCreateOutcome::Failed }
            guard "declared" { self.origin == MobStoreOrigin::Declared }
            update {}
            to Dormant
            emit CreateReleased
        }

        transition ResolveCreateDormant {
            on input ResolveCreate { outcome, in_memory }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotCreating }
        }

        transition ResolveCreateLoading {
            on input ResolveCreate { outcome, in_memory }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotCreating }
        }

        transition ResolveCreateManaged {
            on input ResolveCreate { outcome, in_memory }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotCreating }
        }

        transition ResolveCreateUnloading {
            on input ResolveCreate { outcome, in_memory }
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotCreating }
        }

        transition ResolveCreateHeld {
            on input ResolveCreate { outcome, in_memory }
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotCreating }
        }

        transition ResolveCreateQuarantined {
            on input ResolveCreate { outcome, in_memory }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotCreating }
        }

        transition ResolveCreateRetrying {
            on input ResolveCreate { outcome, in_memory }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotCreating }
        }

        transition ResolveCreateGone {
            on input ResolveCreate { outcome, in_memory }
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotCreating }
        }

        // ------------------------------------------------------------------
        // ReadStop
        // The set read, from the store of a mob that is not loaded, whether
        // the mob is stopped. Only a dormant record that does not know it
        // takes the read: the mob's own answer at an unload is newer than
        // any read of its store, and a read that began before the mob was
        // loaded never overwrites it.
        // ------------------------------------------------------------------

        transition ReadStopDormantStopped {
            on input ReadStop { stopped }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "unread" { self.stop == MobStoreStop::Unread }
            guard "stopped" { stopped == true }
            update {
                self.known = true;
                self.stop = MobStoreStop::Stopped;
            }
            to Dormant
            emit StopRead { stopped: stopped }
        }

        transition ReadStopDormantNotStopped {
            on input ReadStop { stopped }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "unread" { self.stop == MobStoreStop::Unread }
            guard "not_stopped" { stopped == false }
            update {
                self.known = true;
                self.stop = MobStoreStop::NotStopped;
            }
            to Dormant
            emit StopRead { stopped: stopped }
        }

        transition ReadStopDormantKnown {
            on input ReadStop { stopped }
            guard "dormant" { self.lifecycle_phase == Phase::Dormant }
            guard "known" { self.stop != MobStoreStop::Unread }
            update {}
            to Dormant
            emit RequestRefused { reason: MobStoreRestoreRefusal::StopKnown }
        }

        transition ReadStopLoading {
            on input ReadStop { stopped }
            guard "loading" { self.lifecycle_phase == Phase::Loading }
            update {}
            to Loading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }

        transition ReadStopManaged {
            on input ReadStop { stopped }
            guard "managed" { self.lifecycle_phase == Phase::Managed }
            update {}
            to Managed
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }

        transition ReadStopUnloading {
            on input ReadStop { stopped }
            guard "unloading" { self.lifecycle_phase == Phase::Unloading }
            update {}
            to Unloading
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }

        transition ReadStopHeld {
            on input ReadStop { stopped }
            guard "held" { self.lifecycle_phase == Phase::Held }
            update {}
            to Held
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }

        transition ReadStopQuarantined {
            on input ReadStop { stopped }
            guard "quarantined" { self.lifecycle_phase == Phase::Quarantined }
            update {}
            to Quarantined
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }

        transition ReadStopRetrying {
            on input ReadStop { stopped }
            guard "retrying" { self.lifecycle_phase == Phase::Retrying }
            update {}
            to Retrying
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }

        transition ReadStopGone {
            on input ReadStop { stopped }
            guard "gone" { self.lifecycle_phase == Phase::Gone }
            update {}
            to Gone
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }

        transition ReadStopCreating {
            on input ReadStop { stopped }
            guard "creating" { self.lifecycle_phase == Phase::Creating }
            update {}
            to Creating
            emit RequestRefused { reason: MobStoreRestoreRefusal::NotDormant }
        }
    }
}
