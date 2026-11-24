//! Canonical lifecycle authority for a host's mob SET as a whole.
//!
//! A host's mob set hosts any number of mobs, one store each, and each
//! store's stay in the set is one `MobStoreRestoreLifecycleMachine` record.
//! Some facts are about the set and not about one store, so no store's
//! record can hold them. This machine owns them, one state per mob set:
//!
//! - how far the pass over the stores the host has from before came: not
//!   begun, begun and not finished (it runs, or it failed or was dropped and
//!   runs again), or finished;
//! - whether a census of such stores is owed at all (the host has a
//!   persistent mob root, or declared a default mob with a store file);
//! - whether the census is complete: every store the host has from before
//!   is recorded, because the pass finished or because the host declared its
//!   own restore done;
//! - whether a host registered a handle of its own: such a host restores
//!   its mobs itself, so only it can say when the set is complete;
//! - whether every mob of the host is known: no mob can still turn up that
//!   no record names. The machine keeps this answer itself, so that nothing
//!   outside it combines the facts above.
//!
//! "A mob no record names is gone for good" and "this lookup read every
//! mob that could name the session" are read from this state and from the
//! records, and from nothing else.
//!
//! # Scope discipline
//!
//! Set-scoped: one machine state per mob set, no index inside the machine.
//! Volatile: the mob set holds the state in memory and never persists it. A
//! new process begins again, as it derives every store's record again.
//!
//! The machine deliberately does NOT own:
//!
//! - **Anything about one store.** Whether a store's mob is dormant,
//!   loading, managed, held, quarantined or gone is its record's.
//! - **The pass itself.** The shell lists the stores and loads what the
//!   records admit; the machine owns that a pass was begun and whether one
//!   finished. That one pass runs at a time is the shell's lock: it holds
//!   no fact.
//! - **Time.** It reads no clock and begins nothing on its own.
//!
//! # Latches
//!
//! No input clears `census_complete` or `host_inserted`, and `Restored` is
//! never left. A host handle that was registered and destroyed again leaves
//! `host_inserted` set: the host is still one that restores its mobs itself.
//!
//! # Totality discipline
//!
//! Every (phase, input) pair has an explicit transition naming its own `to`
//! phase. Refusal arms return to the exact phase they matched, so no typed
//! refusal changes the state. There is no catch-all arm.

use meerkat_machine_dsl::machine;

/// Why an input was refused. No refusal changes the state.
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
pub enum MobHostedSetRefusal {
    /// Whether a census is owed is said before the first pass begins: a
    /// pass was begun already.
    #[default]
    PassAlreadyBegun,
    /// The host has no stores from before: there is nothing to pass over.
    NoCensusOwed,
    /// A pass already ran to its end: the set runs no second one.
    AlreadyRestored,
    /// A pass was reported finished while none was begun.
    NoPassBegun,
    /// The census is complete already.
    AlreadyComplete,
}

machine! {
    machine MobHostedSetLifecycleMachine {
        version: 1,
        rust: "self" / "catalog::dsl::mob_hosted_set_lifecycle",

        state {
            lifecycle_phase: MobHostedSetLifecyclePhase,
            // Whether the host has stores from before whose census is the
            // only way to know them: a persistent mob root, or a declared
            // default mob with a store file. Said while the set is built,
            // before a pass begins.
            census_owed: bool,
            // Whether every store the host has from before is recorded: a
            // pass finished, or the host declared its own restore done.
            // Never cleared.
            census_complete: bool,
            // Whether a host registered a handle of its own. Never cleared.
            host_inserted: bool,
            // Whether no mob can still turn up that no record names: the
            // census is complete, or none is owed and no host inserts its
            // own handles.
            all_mobs_known: bool,
        }

        init(Unrestored) {
            census_owed = false,
            census_complete = false,
            host_inserted = false,
            all_mobs_known = true,
        }

        terminal []

        phase MobHostedSetLifecyclePhase {
            // No pass over the stores from before was begun.
            Unrestored,
            // A pass was begun and none has finished: it runs, or it failed
            // or was dropped and the next one runs again.
            Attempted,
            // A pass ran to its end.
            Restored,
        }

        input MobHostedSetLifecycleInput {
            // The set's builder says whether the host has stores from
            // before. It may say so again while no pass was begun.
            ExpectCensus { owed: bool },
            // Ask to begin a pass over the stores from before.
            BeginPass,
            // The pass that was begun ran to its end.
            FinishPass,
            // The host declares that it has inserted every mob it restores.
            DeclareRestored,
            // A host's own handle joined the set.
            RegisterHostHandle,
        }

        effect MobHostedSetLifecycleEffect {
            CensusExpected { owed: bool },
            // `first` is true for the first pass this set begins.
            PassBegun { first: bool },
            PassFinished,
            // The census became complete in this step.
            CensusCompleted,
            HostHandleNoted,
            RequestRefused { reason: Enum<MobHostedSetRefusal> },
        }

        // A pass that ran to its end recorded every store from before.
        invariant a_restored_set_has_its_census {
            self.lifecycle_phase != Phase::Restored || self.census_complete
        }

        // A pass is begun only over stores the host has from before.
        invariant a_pass_is_begun_only_where_a_census_is_owed {
            self.lifecycle_phase == Phase::Unrestored || self.census_owed
        }

        // Every mob is known exactly when the census is complete, or none
        // is owed and no host inserts its own handles. Both halves are one
        // fact: a set read as knowing every mob while one can still turn up
        // answers "gone" about a mob that comes back; a set read as not
        // knowing them all when nothing can turn up refuses forever.
        invariant all_mobs_are_known_exactly_when_none_can_still_turn_up {
            (self.all_mobs_known
                && (self.census_complete || (!self.census_owed && !self.host_inserted)))
                || (!self.all_mobs_known
                    && !self.census_complete
                    && (self.census_owed || self.host_inserted))
        }

        // Verdict effects: the mob-set state mirrors them as its typed result.
        disposition CensusExpected => local seam SurfaceResultAlignment,
        disposition PassBegun => local seam SurfaceResultAlignment,
        disposition PassFinished => local seam SurfaceResultAlignment,
        disposition CensusCompleted => local seam SurfaceResultAlignment,
        disposition HostHandleNoted => local seam SurfaceResultAlignment,
        disposition RequestRefused => local seam SurfaceResultAlignment,

        // ------------------------------------------------------------------
        // ExpectCensus
        // Whether a census is owed is configuration of the set, said by its
        // builder. It can be said again (a builder sets and unsets a root)
        // until the first pass begins.
        // ------------------------------------------------------------------

        transition ExpectCensusOwed {
            on input ExpectCensus { owed }
            guard "unrestored" { self.lifecycle_phase == Phase::Unrestored }
            guard "owed" { owed == true }
            guard "census_not_complete" { self.census_complete == false }
            update {
                self.census_owed = true;
                self.all_mobs_known = false;
            }
            to Unrestored
            emit CensusExpected { owed: true }
        }

        transition ExpectCensusOwedComplete {
            on input ExpectCensus { owed }
            guard "unrestored" { self.lifecycle_phase == Phase::Unrestored }
            guard "owed" { owed == true }
            guard "census_complete" { self.census_complete == true }
            update {
                self.census_owed = true;
            }
            to Unrestored
            emit CensusExpected { owed: true }
        }

        transition ExpectCensusNotOwed {
            on input ExpectCensus { owed }
            guard "unrestored" { self.lifecycle_phase == Phase::Unrestored }
            guard "not_owed" { owed == false }
            guard "nothing_can_turn_up" { self.census_complete == true || self.host_inserted == false }
            update {
                self.census_owed = false;
                self.all_mobs_known = true;
            }
            to Unrestored
            emit CensusExpected { owed: false }
        }

        transition ExpectCensusNotOwedHostInserts {
            on input ExpectCensus { owed }
            guard "unrestored" { self.lifecycle_phase == Phase::Unrestored }
            guard "not_owed" { owed == false }
            guard "a_host_inserts" { self.census_complete == false && self.host_inserted == true }
            update {
                self.census_owed = false;
                self.all_mobs_known = false;
            }
            to Unrestored
            emit CensusExpected { owed: false }
        }

        transition ExpectCensusAttempted {
            on input ExpectCensus { owed }
            guard "attempted" { self.lifecycle_phase == Phase::Attempted }
            update {}
            to Attempted
            emit RequestRefused { reason: MobHostedSetRefusal::PassAlreadyBegun }
        }

        transition ExpectCensusRestored {
            on input ExpectCensus { owed }
            guard "restored" { self.lifecycle_phase == Phase::Restored }
            update {}
            to Restored
            emit RequestRefused { reason: MobHostedSetRefusal::PassAlreadyBegun }
        }

        // ------------------------------------------------------------------
        // BeginPass
        // The first pass is named: what must happen before any pass (the
        // host's one forked-participant table is settled) happens on it and
        // never on a pass that follows one that did not finish.
        // ------------------------------------------------------------------

        transition BeginPassFirst {
            on input BeginPass {}
            guard "unrestored" { self.lifecycle_phase == Phase::Unrestored }
            guard "census_owed" { self.census_owed == true }
            update {}
            to Attempted
            emit PassBegun { first: true }
        }

        transition BeginPassNoCensusOwed {
            on input BeginPass {}
            guard "unrestored" { self.lifecycle_phase == Phase::Unrestored }
            guard "no_census_owed" { self.census_owed == false }
            update {}
            to Unrestored
            emit RequestRefused { reason: MobHostedSetRefusal::NoCensusOwed }
        }

        transition BeginPassAgain {
            on input BeginPass {}
            guard "attempted" { self.lifecycle_phase == Phase::Attempted }
            update {}
            to Attempted
            emit PassBegun { first: false }
        }

        transition BeginPassRestored {
            on input BeginPass {}
            guard "restored" { self.lifecycle_phase == Phase::Restored }
            update {}
            to Restored
            emit RequestRefused { reason: MobHostedSetRefusal::AlreadyRestored }
        }

        // ------------------------------------------------------------------
        // FinishPass
        // ------------------------------------------------------------------

        transition FinishPassCompletesCensus {
            on input FinishPass {}
            guard "attempted" { self.lifecycle_phase == Phase::Attempted }
            guard "census_not_complete" { self.census_complete == false }
            update {
                self.census_complete = true;
                self.all_mobs_known = true;
            }
            to Restored
            emit PassFinished
            emit CensusCompleted
        }

        transition FinishPassCensusComplete {
            on input FinishPass {}
            guard "attempted" { self.lifecycle_phase == Phase::Attempted }
            guard "census_complete" { self.census_complete == true }
            update {}
            to Restored
            emit PassFinished
        }

        transition FinishPassUnrestored {
            on input FinishPass {}
            guard "unrestored" { self.lifecycle_phase == Phase::Unrestored }
            update {}
            to Unrestored
            emit RequestRefused { reason: MobHostedSetRefusal::NoPassBegun }
        }

        transition FinishPassRestored {
            on input FinishPass {}
            guard "restored" { self.lifecycle_phase == Phase::Restored }
            update {}
            to Restored
            emit RequestRefused { reason: MobHostedSetRefusal::AlreadyRestored }
        }

        // ------------------------------------------------------------------
        // DeclareRestored
        // The host's own word completes the census in every phase.
        // ------------------------------------------------------------------

        transition DeclareRestoredUnrestored {
            on input DeclareRestored {}
            guard "unrestored" { self.lifecycle_phase == Phase::Unrestored }
            guard "census_not_complete" { self.census_complete == false }
            update {
                self.census_complete = true;
                self.all_mobs_known = true;
            }
            to Unrestored
            emit CensusCompleted
        }

        transition DeclareRestoredUnrestoredComplete {
            on input DeclareRestored {}
            guard "unrestored" { self.lifecycle_phase == Phase::Unrestored }
            guard "census_complete" { self.census_complete == true }
            update {}
            to Unrestored
            emit RequestRefused { reason: MobHostedSetRefusal::AlreadyComplete }
        }

        transition DeclareRestoredAttempted {
            on input DeclareRestored {}
            guard "attempted" { self.lifecycle_phase == Phase::Attempted }
            guard "census_not_complete" { self.census_complete == false }
            update {
                self.census_complete = true;
                self.all_mobs_known = true;
            }
            to Attempted
            emit CensusCompleted
        }

        transition DeclareRestoredAttemptedComplete {
            on input DeclareRestored {}
            guard "attempted" { self.lifecycle_phase == Phase::Attempted }
            guard "census_complete" { self.census_complete == true }
            update {}
            to Attempted
            emit RequestRefused { reason: MobHostedSetRefusal::AlreadyComplete }
        }

        transition DeclareRestoredRestored {
            on input DeclareRestored {}
            guard "restored" { self.lifecycle_phase == Phase::Restored }
            update {}
            to Restored
            emit RequestRefused { reason: MobHostedSetRefusal::AlreadyComplete }
        }

        // ------------------------------------------------------------------
        // RegisterHostHandle
        // Admitted in every phase. While the census is not complete, a host
        // that inserts its own handles may still insert a mob no record
        // names.
        // ------------------------------------------------------------------

        transition RegisterHostHandleUnrestored {
            on input RegisterHostHandle {}
            guard "unrestored" { self.lifecycle_phase == Phase::Unrestored }
            guard "census_not_complete" { self.census_complete == false }
            update {
                self.host_inserted = true;
                self.all_mobs_known = false;
            }
            to Unrestored
            emit HostHandleNoted
        }

        transition RegisterHostHandleUnrestoredComplete {
            on input RegisterHostHandle {}
            guard "unrestored" { self.lifecycle_phase == Phase::Unrestored }
            guard "census_complete" { self.census_complete == true }
            update {
                self.host_inserted = true;
            }
            to Unrestored
            emit HostHandleNoted
        }

        transition RegisterHostHandleAttempted {
            on input RegisterHostHandle {}
            guard "attempted" { self.lifecycle_phase == Phase::Attempted }
            guard "census_not_complete" { self.census_complete == false }
            update {
                self.host_inserted = true;
                self.all_mobs_known = false;
            }
            to Attempted
            emit HostHandleNoted
        }

        transition RegisterHostHandleAttemptedComplete {
            on input RegisterHostHandle {}
            guard "attempted" { self.lifecycle_phase == Phase::Attempted }
            guard "census_complete" { self.census_complete == true }
            update {
                self.host_inserted = true;
            }
            to Attempted
            emit HostHandleNoted
        }

        transition RegisterHostHandleRestored {
            on input RegisterHostHandle {}
            guard "restored" { self.lifecycle_phase == Phase::Restored }
            update {
                self.host_inserted = true;
            }
            to Restored
            emit HostHandleNoted
        }
    }
}
