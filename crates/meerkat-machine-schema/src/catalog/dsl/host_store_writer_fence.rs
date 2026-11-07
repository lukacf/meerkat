//! Canonical lifecycle authority for the writer fence of ONE host store realm.
//!
//! A process opens one realm over a host store: it acquires the realm's
//! fence, renews it once per renew interval, stops its writes when a renew
//! fails or its self-fence window closes, and goes on only through a fresh
//! acquire whose listing shows no batch it did not write. This machine owns
//! that protocol's facts: the writer's phase (live, stopped, re-acquiring,
//! proving a grant, resuming, waiting out a holder that may be its own late
//! grant, or lost), the token it commits with, the grant it got and has not
//! installed, the acquires it gave up on since its last grant, the holder it
//! waits out, and why it last stopped or why it lost the realm.
//!
//! # Scope discipline
//!
//! Realm-scoped: one machine state per open realm. Volatile: the realm holds
//! it in memory, and a reopen starts a fresh one at the open's grant.
//!
//! The machine deliberately does NOT own:
//!
//! - **Time.** It reads no clock. The realm keeps the self-fence window's
//!   anchor, its timers and the recorded end of a waited-out lease, and
//!   reports their facts as inputs: the window closed, a renew started
//!   inside or after the window, the waited-out lease ended, the lease of
//!   a holder met again moved later than the end the writer waited out.
//! - **Store I/O.** The realm sends the acquire, listing and renew the
//!   machine asks for, and reports each answer as an input. Every request is
//!   answered: an acquire or renew the store does not answer in time is
//!   reported timed out.
//! - **The listing comparison.** The versions the realm last saw or wrote are
//!   data; the realm compares a fresh listing with them and reports it clean,
//!   foreign or failed.
//! - **When to report, and the texts.** The realm decides when a scope is
//!   quarantined or poisoned and keeps the cause texts; the machine owns
//!   which scopes are quarantined (for a conflict the store reports, with
//!   each conflicted incarnation's version and commit ids) or poisoned
//!   (with the first cause), and the realm caches each scope's
//!   classification on its cell.
//!
//! # Totality discipline
//!
//! Every (phase, input) pair has an explicit arm. Refusal arms return to the
//! phase they matched and change nothing; arms that only observe (a renew
//! answer that arrives once the writer stopped) change nothing either. There
//! is no catch-all arm.

use meerkat_machine_dsl::machine;

use super::OptionValueExt;

/// The acquire a re-acquire attempt sends.
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
pub enum WriterFenceAcquireStep {
    /// An exclusive acquire: a live holder makes the store answer `Held`.
    #[default]
    Exclusive,
    /// A takeover of the writer's own recorded lease, after the exclusive
    /// acquire met it as the holder.
    TakeoverOwn,
}

/// How an acquire of a re-acquire answered.
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
pub enum WriterFenceAcquireOutcome {
    /// The store granted a token.
    #[default]
    Granted,
    /// The store answered that a live holder holds the fence.
    Held,
    /// The acquire failed with any other answer.
    Failed,
    /// The acquire did not answer within the realm's acquire timeout. The
    /// store may still grant it.
    TimedOut,
}

/// How a renew answered.
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
pub enum WriterFenceRenewOutcome {
    /// The store recorded a new lease for the renewed token.
    #[default]
    Renewed,
    /// The store answered that a higher token exists.
    FencedOut,
    /// The renew failed with any other answer.
    Failed,
    /// The renew did not answer within the realm's renew timeout.
    TimedOut,
}

/// How the listing of a re-acquire answered.
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
pub enum WriterFenceListingOutcome {
    /// Every scope is at a version the writer saw, wrote or may have
    /// written.
    #[default]
    Clean,
    /// A scope shows a batch the writer did not write.
    ForeignBatch,
    /// The listing failed.
    Failed,
}

/// Why the writer last stopped.
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
pub enum WriterFenceStopCause {
    /// A renew failed.
    #[default]
    RenewFailed,
    /// A renew did not answer in time.
    RenewTimedOut,
    /// The self-fence window was seen closed.
    WindowClosed,
    /// A renew answered after starting once the self-fence window had
    /// closed: an acquire made meanwhile may have missed the last renew.
    WindowClosedBeforeRenew,
    /// An acquire of a re-acquire failed.
    ReacquireFailed,
    /// An acquire of a re-acquire did not answer in time.
    ReacquireTimedOut,
    /// The listing of a re-acquire failed after its grant.
    ListingFailed,
    /// The renew right after a re-acquire failed.
    ResumeRenewFailed,
    /// The renew right after a re-acquire did not answer in time.
    ResumeRenewTimedOut,
}

/// Why the writer lost the realm.
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
pub enum WriterFenceLostCause {
    /// A renew answered that a higher token superseded the writer's.
    #[default]
    FencedOutRenew,
    /// A commit answered that a higher token superseded the batch's.
    FencedOutCommit,
    /// A stopped writer's exclusive acquire met a holder it does not know,
    /// with no acquire of its own timed out since its last grant.
    HeldByAnother,
    /// The holder the writer waited out still held the fence after its
    /// recorded lease ended: it renewed that lease, and only a live writer
    /// renews.
    HolderRenewed,
    /// The listing of a re-acquire showed a batch the writer did not write.
    ForeignBatch,
    /// The renew right after a re-acquire answered that a higher token
    /// exists.
    FencedOutAfterReacquire,
    /// The store granted a token at or below the installed one: it broke
    /// its obligation to answer every grant a token above every earlier
    /// grant.
    TokenOrder,
    /// An adapter broke a rule of the realm and could not single out the
    /// affected scope.
    AdapterRule,
}

/// Why a scope of the realm was poisoned: the decision site. Every one
/// leaves the scope's memory possibly ahead of the store, so the scope
/// refuses every call until the realm is reopened.
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
pub enum ScopePoisonCause {
    /// A commit's plan failed after memory changed.
    #[default]
    PlanFailed,
    /// A batch's outcome stayed unknown.
    Unsettled,
    /// The store refused a batch after memory changed.
    Refused,
    /// A call was dropped while its change was armed.
    Dropped,
    /// A commit could not be prepared after memory changed.
    PrepareFailed,
    /// A scope's body did not import after its load.
    ImportFailed,
    /// A call changed memory outside the scopes it locked.
    StrayTouch,
    /// An adapter reported a broken rule for one scope.
    Adapter,
}

/// Why an input was refused.
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
pub enum WriterFenceRefusal {
    /// The realm has not opened: no grant installed a token.
    #[default]
    NotOpened,
    /// The open already installed a token.
    AlreadyOpened,
    /// The writer is live: there is nothing to re-acquire.
    AlreadyLive,
    /// A re-acquire attempt is in flight.
    AttemptInFlight,
    /// The writer waits out a holder's recorded lease and makes no acquire
    /// before it ends.
    WaitingOutHolder,
    /// No holder is waited out.
    NotWaitingOut,
    /// No acquire is in flight.
    NotAcquiring,
    /// No grant is being proved, or the listing named another grant.
    NotProving,
    /// No renew of the writer can be in flight.
    NoRenewInFlight,
    /// The writer lost the realm: every input refuses until a reopen.
    Lost,
    /// The open's listing failed: the realm is never served, and no
    /// re-acquire lists against a baseline the open never installed.
    OpenListingFailed,
    /// The scope is quarantined: a poison report changes nothing.
    Quarantined,
    /// The scope is poisoned: a quarantine changes nothing.
    Poisoned,
}

machine! {
    machine HostStoreWriterFenceMachine {
        version: 1,
        rust: "self" / "catalog::dsl::host_store_writer_fence",

        state {
            lifecycle_phase: HostStoreWriterFencePhase,
            // The token the realm commits with: the open's grant, then each
            // proved re-acquire's. Zero only while Unopened.
            token: u64,
            // The grant of the attempt in Proving and Resuming.
            granted: Option<u64>,
            // The last grant while its token is not installed: the store
            // records its lease as the writer's own.
            own_grant: Option<u64>,
            // The acquire in flight while Acquiring.
            step: Enum<WriterFenceAcquireStep>,
            // Acquires that timed out since the last grant.
            timed_out: u64,
            // The holder met while acquires had timed out: waited out, and
            // judged when met again.
            waited_holder: Option<u64>,
            // Why the writer last stopped. Meaningful while it is not live.
            stop_cause: Enum<WriterFenceStopCause>,
            // Why the writer lost the realm. Meaningful while Lost.
            lost_cause: Enum<WriterFenceLostCause>,
            // The writer stopped and no re-acquire resumed it since: set by
            // its first stop, cleared only when a re-acquire resumes it, and
            // kept through a loss (a writer lost while live reads
            // unstopped). The realm's self-fence window reads it as its
            // failed renew.
            writer_stopped: bool,
            // The open's listing failed: no re-acquire is made.
            open_listing_failed: bool,
            // Scopes quarantined for a conflict the store reports, by the
            // store name without its incarnation (a chunk by its full
            // name). Only grows until a reopen.
            quarantined_keys: Set<String>,
            // Each conflicted incarnation of a quarantined key, by its full
            // store name: the version that holds two batches. Only grows,
            // and a record is never overwritten.
            quarantine_versions: Map<String, u64>,
            // The same incarnations' conflicting commit ids (none when the
            // store did not name them).
            quarantine_commit_ids: Map<String, Set<String>>,
            // Scopes poisoned, keyed as the quarantined keys, each with
            // the first cause. Only grows until a reopen.
            poisoned_scopes: Map<String, Enum<ScopePoisonCause>>,
        }

        init(Unopened) {
            token = 0,
            granted = None,
            own_grant = None,
            step = WriterFenceAcquireStep::Exclusive,
            timed_out = 0,
            waited_holder = None,
            stop_cause = WriterFenceStopCause::RenewFailed,
            lost_cause = WriterFenceLostCause::FencedOutRenew,
            writer_stopped = false,
            open_listing_failed = false,
            quarantined_keys = EmptySet,
            quarantine_versions = EmptyMap,
            quarantine_commit_ids = EmptyMap,
            poisoned_scopes = EmptyMap,
        }

        terminal [Lost]

        phase HostStoreWriterFencePhase {
            // The realm has not installed the open's grant.
            Unopened,
            // The writer holds `token` and renews it. Whether its self-fence
            // window is open is a clock fact the realm reads.
            Live,
            // A renew failed or timed out, or the window was seen closed, or
            // a re-acquire attempt ended without resuming. No write starts.
            Stopped,
            // One acquire of a re-acquire is in flight.
            Acquiring,
            // A grant is being proved by a fresh listing.
            Proving,
            // The grant's token is installed; its first renew is in flight.
            Resuming,
            // A holder that may be the writer's own late grant is waited
            // out: no acquire before its recorded lease ends.
            WaitingOut,
            // Another writer holds the realm, or a rule of the protocol was
            // broken. Terminal: every input refuses until a reopen.
            Lost,
        }

        input HostStoreWriterFenceInput {
            // The open installed its grant's token.
            Open { token: u64 },
            // The realm saw the self-fence window closed.
            WindowClosed,
            // A renew of `renewed` answered. `current` is the higher token a
            // fenced-out answer names; `in_window` says whether the renew
            // started before the self-fence window closed.
            RenewAnswered { outcome: Enum<WriterFenceRenewOutcome>, renewed: u64, current: u64, in_window: bool },
            // A caller of the stopped writer asks for a re-acquire.
            BeginReacquire,
            // The acquire in flight answered. `token` is the granted token,
            // or the holder of a `Held` answer. `lease_moved` is a clock fact
            // the realm reads for a `Held` answer naming the holder it waited
            // out: whether that holder's lease now ends more than one renew
            // interval later than the end the writer waited out (the holder
            // renewed it). False for every other answer.
            AcquireAnswered { outcome: Enum<WriterFenceAcquireOutcome>, token: u64, lease_moved: bool },
            // The recorded lease of the waited-out holder ended.
            LeaseEnded,
            // The listing that proves the grant `token` answered.
            ListingChecked { outcome: Enum<WriterFenceListingOutcome>, token: u64 },
            // A commit answered fenced out, or an adapter broke a rule.
            Poison { cause: Enum<WriterFenceLostCause> },
            // The open's listing failed.
            OpenListingFailed,
            // The conflicted incarnation `name` of the scope `key`, listed
            // or met at a load, holds two batches at `version`, with their
            // commit ids (possibly none).
            ScopeQuarantined { key: String, name: String, version: u64, commit_ids: Set<String> },
            // A decision site poisoned `scope`.
            ScopePoisoned { scope: String, cause: Enum<ScopePoisonCause> },
        }

        effect HostStoreWriterFenceEffect {
            // The writer is live under the open's token: renewals start.
            WriterOpened { token: u64 },
            // A renew succeeded inside the window: the window counts from
            // its start.
            WindowAnchored { token: u64 },
            // The writer stopped: no write starts until it resumes.
            WriterStopped { cause: Enum<WriterFenceStopCause> },
            // Send this acquire.
            AcquireRequested { step: Enum<WriterFenceAcquireStep> },
            // The store granted `token`: list every scope to prove it.
            GrantReceived { token: u64 },
            // The listing proved the grant: commit with `token` and renew it.
            TokenInstalled { token: u64 },
            // The renew after a re-acquire succeeded: the writer is live.
            WriterResumed { token: u64 },
            // Wait out `holder`'s recorded lease before acquiring again.
            HolderWaitedOut { holder: u64 },
            // The writer lost the realm.
            RealmLost { cause: Enum<WriterFenceLostCause> },
            // A renew answer that changes nothing: the writer stopped while
            // it was in flight, or it renewed a token this writer superseded
            // itself.
            RenewIgnored,
            RequestRefused { reason: Enum<WriterFenceRefusal> },
            // A conflicted incarnation was recorded: the realm caches the
            // key's quarantine on its cell.
            ScopeQuarantineRecorded { key: String, name: String },
            // A scope's poison was recorded: the realm caches it, with its
            // cause text, on the scope's cell.
            ScopePoisonRecorded { scope: String, cause: Enum<ScopePoisonCause> },
        }

        // An unopened writer holds no token and no attempt.
        invariant unopened_holds_nothing {
            self.lifecycle_phase != Phase::Unopened
                || (
                    self.token == 0
                    && self.granted == None
                    && self.own_grant == None
                    && self.timed_out == 0
                    && self.waited_holder == None
                    && !self.writer_stopped
                    && !self.open_listing_failed
                    && self.quarantined_keys.len() == 0
                    && self.quarantine_versions.keys().len() == 0
                    && self.poisoned_scopes.keys().len() == 0
                )
        }

        // One classification per scope: never quarantined and poisoned.
        invariant no_scope_is_quarantined_and_poisoned {
            for_all(key in self.quarantined_keys, !self.poisoned_scopes.contains_key(key))
        }

        // A conflicted incarnation's record has both parts.
        invariant quarantine_records_are_whole {
            self.quarantine_versions.keys() == self.quarantine_commit_ids.keys()
        }

        // A live writer has not stopped since it last resumed.
        invariant live_writer_is_not_stopped {
            self.lifecycle_phase != Phase::Live || !self.writer_stopped
        }

        // A writer that stopped, re-acquires or waits out a holder writes
        // nothing until it resumes.
        invariant stopped_and_attempting_writer_is_stopped {
            (
                self.lifecycle_phase != Phase::Stopped
                && self.lifecycle_phase != Phase::Acquiring
                && self.lifecycle_phase != Phase::Proving
                && self.lifecycle_phase != Phase::Resuming
                && self.lifecycle_phase != Phase::WaitingOut
            )
            || self.writer_stopped
        }

        // A grant is carried while it is proved and resumed on.
        invariant proving_and_resuming_carry_their_grant {
            (
                self.lifecycle_phase != Phase::Proving
                && self.lifecycle_phase != Phase::Resuming
            )
            || self.granted != None
        }

        // And only then.
        invariant grant_carried_only_while_proving_or_resuming {
            self.granted == None
                || self.lifecycle_phase == Phase::Proving
                || self.lifecycle_phase == Phase::Resuming
        }

        // A grant whose token is not installed is above the installed token.
        invariant own_grant_is_above_the_token {
            self.own_grant == None || self.own_grant.get("value") > self.token
        }

        // A writer waits out a holder only after an acquire of its own timed
        // out since its last grant.
        invariant waiting_out_follows_a_timed_out_acquire {
            self.lifecycle_phase != Phase::WaitingOut
                || (self.timed_out > 0 && self.waited_holder != None)
        }

        // A waited-out holder is recorded only while acquires had timed out.
        invariant waited_holder_needs_a_timed_out_acquire {
            self.waited_holder == None || self.timed_out > 0
        }

        // Requests: the realm sends the acquire, listing or renew and
        // reports its answer, or installs the token, or moves the window's
        // anchor.
        disposition WriterOpened => local seam OwnerRealizationOnly,
        disposition WindowAnchored => local seam OwnerRealizationOnly,
        disposition AcquireRequested => local seam OwnerRealizationOnly,
        disposition GrantReceived => local seam OwnerRealizationOnly,
        disposition TokenInstalled => local seam OwnerRealizationOnly,
        // Verdicts: the realm mirrors them in its answers.
        disposition WriterStopped => local seam SurfaceResultAlignment,
        disposition WriterResumed => local seam SurfaceResultAlignment,
        disposition HolderWaitedOut => local seam SurfaceResultAlignment,
        disposition RealmLost => local seam SurfaceResultAlignment,
        disposition RenewIgnored => local seam SurfaceResultAlignment,
        disposition RequestRefused => local seam SurfaceResultAlignment,
        // Classifications: the realm caches them on the scope's cell.
        disposition ScopeQuarantineRecorded => local seam SurfaceResultAlignment,
        disposition ScopePoisonRecorded => local seam SurfaceResultAlignment,

        // ------------------------------------------------------------------
        // Open
        // ------------------------------------------------------------------

        // The open's grant is the first: any token the store answers.
        transition Open {
            on input Open { token }
            guard "unopened" { self.lifecycle_phase == Phase::Unopened }
            update {
                self.token = token;
            }
            to Live
            emit WriterOpened { token: self.token }
        }

        transition OpenAgain {
            per_phase [Live, Stopped, Acquiring, Proving, Resuming, WaitingOut]
            on input Open { token }
            update {}
            to Live
            emit RequestRefused { reason: WriterFenceRefusal::AlreadyOpened }
        }

        transition OpenLost {
            on input Open { token }
            guard "lost" { self.lifecycle_phase == Phase::Lost }
            update {}
            to Lost
            emit RequestRefused { reason: WriterFenceRefusal::Lost }
        }

        // ------------------------------------------------------------------
        // WindowClosed
        // A live writer whose window closed stops; a writer that is not live
        // already writes nothing.
        // ------------------------------------------------------------------

        transition WindowClosedLive {
            on input WindowClosed
            guard "live" { self.lifecycle_phase == Phase::Live }
            update {
                self.stop_cause = WriterFenceStopCause::WindowClosed;
                self.writer_stopped = true;
            }
            to Stopped
            emit WriterStopped { cause: self.stop_cause }
        }

        transition WindowClosedNotLive {
            per_phase [Stopped, Acquiring, Proving, Resuming, WaitingOut]
            on input WindowClosed
            update {}
            to Stopped
        }

        transition WindowClosedUnopened {
            on input WindowClosed
            guard "unopened" { self.lifecycle_phase == Phase::Unopened }
            update {}
            to Unopened
            emit RequestRefused { reason: WriterFenceRefusal::NotOpened }
        }

        transition WindowClosedLost {
            on input WindowClosed
            guard "lost" { self.lifecycle_phase == Phase::Lost }
            update {}
            to Lost
            emit RequestRefused { reason: WriterFenceRefusal::Lost }
        }

        // ------------------------------------------------------------------
        // RenewAnswered
        // A renew runs while the writer is live, and its answer can meet the
        // writer stopped (the window was seen closed meanwhile). The renew
        // right after a re-acquire answers in Resuming.
        // ------------------------------------------------------------------

        transition RenewedInWindow {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "live" { self.lifecycle_phase == Phase::Live }
            guard "renewed" { outcome == WriterFenceRenewOutcome::Renewed }
            guard "started_inside_the_window" { in_window }
            update {}
            to Live
            emit WindowAnchored { token: self.token }
        }

        transition RenewedAfterTheWindow {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "live" { self.lifecycle_phase == Phase::Live }
            guard "renewed" { outcome == WriterFenceRenewOutcome::Renewed }
            guard "started_after_the_window_closed" { !in_window }
            update {
                self.stop_cause = WriterFenceStopCause::WindowClosedBeforeRenew;
                self.writer_stopped = true;
            }
            to Stopped
            emit WriterStopped { cause: self.stop_cause }
        }

        transition RenewStaleLive {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "live" { self.lifecycle_phase == Phase::Live }
            guard "fenced_out" { outcome == WriterFenceRenewOutcome::FencedOut }
            guard "superseded_by_this_writer" { current != renewed && current == self.token }
            update {}
            to Live
            emit RenewIgnored
        }

        transition RenewFencedOutLive {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "live" { self.lifecycle_phase == Phase::Live }
            guard "fenced_out" { outcome == WriterFenceRenewOutcome::FencedOut }
            guard "superseded_by_another" { current == renewed || current != self.token }
            update {
                self.lost_cause = WriterFenceLostCause::FencedOutRenew;
            }
            to Lost
            emit RealmLost { cause: self.lost_cause }
        }

        transition RenewFailedLive {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "live" { self.lifecycle_phase == Phase::Live }
            guard "failed" { outcome == WriterFenceRenewOutcome::Failed }
            update {
                self.stop_cause = WriterFenceStopCause::RenewFailed;
                self.writer_stopped = true;
            }
            to Stopped
            emit WriterStopped { cause: self.stop_cause }
        }

        transition RenewTimedOutLive {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "live" { self.lifecycle_phase == Phase::Live }
            guard "timed_out" { outcome == WriterFenceRenewOutcome::TimedOut }
            update {
                self.stop_cause = WriterFenceStopCause::RenewTimedOut;
                self.writer_stopped = true;
            }
            to Stopped
            emit WriterStopped { cause: self.stop_cause }
        }

        // A stopped writer resumes on a fresh acquire alone: a renew that
        // answers once it stopped moves nothing.
        transition RenewAnsweredStopped {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "stopped" { self.lifecycle_phase == Phase::Stopped }
            guard "not_fenced_out" { outcome != WriterFenceRenewOutcome::FencedOut }
            update {}
            to Stopped
            emit RenewIgnored
        }

        transition RenewStaleStopped {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "stopped" { self.lifecycle_phase == Phase::Stopped }
            guard "fenced_out" { outcome == WriterFenceRenewOutcome::FencedOut }
            guard "superseded_by_this_writer" { current != renewed && current == self.token }
            update {}
            to Stopped
            emit RenewIgnored
        }

        transition RenewFencedOutStopped {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "stopped" { self.lifecycle_phase == Phase::Stopped }
            guard "fenced_out" { outcome == WriterFenceRenewOutcome::FencedOut }
            guard "superseded_by_another" { current == renewed || current != self.token }
            update {
                self.lost_cause = WriterFenceLostCause::FencedOutRenew;
            }
            to Lost
            emit RealmLost { cause: self.lost_cause }
        }

        transition ResumeRenewed {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "resuming" { self.lifecycle_phase == Phase::Resuming }
            guard "renewed" { outcome == WriterFenceRenewOutcome::Renewed }
            update {
                self.granted = None;
                self.writer_stopped = false;
            }
            to Live
            emit WriterResumed { token: self.token }
        }

        transition ResumeFencedOut {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "resuming" { self.lifecycle_phase == Phase::Resuming }
            guard "fenced_out" { outcome == WriterFenceRenewOutcome::FencedOut }
            update {
                self.granted = None;
                self.lost_cause = WriterFenceLostCause::FencedOutAfterReacquire;
            }
            to Lost
            emit RealmLost { cause: self.lost_cause }
        }

        transition ResumeRenewFailed {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "resuming" { self.lifecycle_phase == Phase::Resuming }
            guard "failed" { outcome == WriterFenceRenewOutcome::Failed }
            update {
                self.granted = None;
                self.stop_cause = WriterFenceStopCause::ResumeRenewFailed;
            }
            to Stopped
            emit WriterStopped { cause: self.stop_cause }
        }

        transition ResumeRenewTimedOut {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "resuming" { self.lifecycle_phase == Phase::Resuming }
            guard "timed_out" { outcome == WriterFenceRenewOutcome::TimedOut }
            update {
                self.granted = None;
                self.stop_cause = WriterFenceStopCause::ResumeRenewTimedOut;
            }
            to Stopped
            emit WriterStopped { cause: self.stop_cause }
        }

        // Renews and re-acquires never overlap, and no renew runs before the
        // open or while a holder is waited out.
        transition RenewAnsweredNoRenew {
            per_phase [Unopened, Acquiring, Proving, WaitingOut]
            on input RenewAnswered { outcome, renewed, current, in_window }
            update {}
            to Unopened
            emit RequestRefused { reason: WriterFenceRefusal::NoRenewInFlight }
        }

        transition RenewAnsweredLost {
            on input RenewAnswered { outcome, renewed, current, in_window }
            guard "lost" { self.lifecycle_phase == Phase::Lost }
            update {}
            to Lost
            emit RequestRefused { reason: WriterFenceRefusal::Lost }
        }

        // ------------------------------------------------------------------
        // BeginReacquire
        // One attempt at a time, from Stopped only. A waited-out holder's
        // lease ends first (LeaseEnded).
        // ------------------------------------------------------------------

        transition BeginReacquire {
            on input BeginReacquire
            guard "stopped" { self.lifecycle_phase == Phase::Stopped }
            guard "the_open_listed" { !self.open_listing_failed }
            update {
                self.step = WriterFenceAcquireStep::Exclusive;
            }
            to Acquiring
            emit AcquireRequested { step: self.step }
        }

        // A realm whose open failed its listing has no baseline to compare
        // a fresh listing with, and is never served: no re-acquire.
        transition BeginReacquireAfterFailedOpen {
            on input BeginReacquire
            guard "stopped" { self.lifecycle_phase == Phase::Stopped }
            guard "the_open_listing_failed" { self.open_listing_failed }
            update {}
            to Stopped
            emit RequestRefused { reason: WriterFenceRefusal::OpenListingFailed }
        }

        transition BeginReacquireLive {
            on input BeginReacquire
            guard "live" { self.lifecycle_phase == Phase::Live }
            update {}
            to Live
            emit RequestRefused { reason: WriterFenceRefusal::AlreadyLive }
        }

        transition BeginReacquireInFlight {
            per_phase [Acquiring, Proving, Resuming]
            on input BeginReacquire
            update {}
            to Acquiring
            emit RequestRefused { reason: WriterFenceRefusal::AttemptInFlight }
        }

        transition BeginReacquireWaitingOut {
            on input BeginReacquire
            guard "waiting_out" { self.lifecycle_phase == Phase::WaitingOut }
            update {}
            to WaitingOut
            emit RequestRefused { reason: WriterFenceRefusal::WaitingOutHolder }
        }

        transition BeginReacquireUnopened {
            on input BeginReacquire
            guard "unopened" { self.lifecycle_phase == Phase::Unopened }
            update {}
            to Unopened
            emit RequestRefused { reason: WriterFenceRefusal::NotOpened }
        }

        transition BeginReacquireLost {
            on input BeginReacquire
            guard "lost" { self.lifecycle_phase == Phase::Lost }
            update {}
            to Lost
            emit RequestRefused { reason: WriterFenceRefusal::Lost }
        }

        // ------------------------------------------------------------------
        // AcquireAnswered
        // ------------------------------------------------------------------

        // A grant above the installed token is this writer's own until a
        // listing proves it: no other writer committed since the stop.
        transition AcquireGranted {
            on input AcquireAnswered { outcome, token, lease_moved }
            guard "acquiring" { self.lifecycle_phase == Phase::Acquiring }
            guard "granted" { outcome == WriterFenceAcquireOutcome::Granted }
            guard "above_the_installed_token" { token > self.token }
            update {
                self.granted = Some(token);
                self.own_grant = Some(token);
                self.timed_out = 0;
                self.waited_holder = None;
            }
            to Proving
            emit GrantReceived { token: token }
        }

        // A grant at or below the installed token breaks the store's token
        // order: fail closed.
        transition AcquireGrantedOutOfOrder {
            on input AcquireAnswered { outcome, token, lease_moved }
            guard "acquiring" { self.lifecycle_phase == Phase::Acquiring }
            guard "granted" { outcome == WriterFenceAcquireOutcome::Granted }
            guard "at_or_below_the_installed_token" { token <= self.token }
            update {
                self.lost_cause = WriterFenceLostCause::TokenOrder;
            }
            to Lost
            emit RealmLost { cause: self.lost_cause }
        }

        // The exclusive acquire met the writer's own recorded lease (its
        // installed token, or a grant whose listing failed): take it over.
        transition AcquireHeldByOwnLease {
            on input AcquireAnswered { outcome, token, lease_moved }
            guard "acquiring" { self.lifecycle_phase == Phase::Acquiring }
            guard "held" { outcome == WriterFenceAcquireOutcome::Held }
            guard "exclusive" { self.step == WriterFenceAcquireStep::Exclusive }
            guard "own_lease" { token == self.token || self.own_grant == Some(token) }
            update {
                self.step = WriterFenceAcquireStep::TakeoverOwn;
            }
            to Acquiring
            emit AcquireRequested { step: self.step }
        }

        // With no acquire of its own timed out since its last grant, a holder
        // the writer does not know is another writer.
        transition AcquireHeldByAnother {
            on input AcquireAnswered { outcome, token, lease_moved }
            guard "acquiring" { self.lifecycle_phase == Phase::Acquiring }
            guard "held" { outcome == WriterFenceAcquireOutcome::Held }
            guard "exclusive" { self.step == WriterFenceAcquireStep::Exclusive }
            guard "unknown_holder" { token != self.token && self.own_grant != Some(token) }
            guard "no_acquire_timed_out" { self.timed_out == 0 }
            update {
                self.lost_cause = WriterFenceLostCause::HeldByAnother;
            }
            to Lost
            emit RealmLost { cause: self.lost_cause }
        }

        // The holder met again after the writer waited out its recorded lease
        // renewed it (its lease moved later): another live writer.
        transition AcquireHeldByRenewedHolder {
            on input AcquireAnswered { outcome, token, lease_moved }
            guard "acquiring" { self.lifecycle_phase == Phase::Acquiring }
            guard "held" { outcome == WriterFenceAcquireOutcome::Held }
            guard "exclusive" { self.step == WriterFenceAcquireStep::Exclusive }
            guard "unknown_holder" { token != self.token && self.own_grant != Some(token) }
            guard "an_acquire_timed_out" { self.timed_out > 0 }
            guard "the_waited_holder" { self.waited_holder == Some(token) }
            guard "its_lease_moved" { lease_moved }
            update {
                self.lost_cause = WriterFenceLostCause::HolderRenewed;
            }
            to Lost
            emit RealmLost { cause: self.lost_cause }
        }

        // The holder met again after the writer waited out the end it
        // recorded, its lease not moved: nobody renewed it, and the writer's
        // recorded end came early (a store that answers the remaining lease
        // rounded down, or on a slower clock). Wait out its new end.
        transition AcquireHeldByUnrenewedHolder {
            on input AcquireAnswered { outcome, token, lease_moved }
            guard "acquiring" { self.lifecycle_phase == Phase::Acquiring }
            guard "held" { outcome == WriterFenceAcquireOutcome::Held }
            guard "exclusive" { self.step == WriterFenceAcquireStep::Exclusive }
            guard "unknown_holder" { token != self.token && self.own_grant != Some(token) }
            guard "an_acquire_timed_out" { self.timed_out > 0 }
            guard "the_waited_holder" { self.waited_holder == Some(token) }
            guard "its_lease_did_not_move" { !lease_moved }
            update {}
            to WaitingOut
            emit HolderWaitedOut { holder: token }
        }

        // While an acquire of its own timed out, an unknown holder may be its
        // own late grant: wait out its recorded lease.
        transition AcquireHeldByPossibleLateGrant {
            on input AcquireAnswered { outcome, token, lease_moved }
            guard "acquiring" { self.lifecycle_phase == Phase::Acquiring }
            guard "held" { outcome == WriterFenceAcquireOutcome::Held }
            guard "exclusive" { self.step == WriterFenceAcquireStep::Exclusive }
            guard "unknown_holder" { token != self.token && self.own_grant != Some(token) }
            guard "an_acquire_timed_out" { self.timed_out > 0 }
            guard "not_the_waited_holder" { self.waited_holder != Some(token) }
            update {
                self.waited_holder = Some(token);
            }
            to WaitingOut
            emit HolderWaitedOut { holder: token }
        }

        // A takeover of the writer's own lease supersedes any holder; one
        // that answers held failed.
        transition AcquireHeldAtTakeover {
            on input AcquireAnswered { outcome, token, lease_moved }
            guard "acquiring" { self.lifecycle_phase == Phase::Acquiring }
            guard "held" { outcome == WriterFenceAcquireOutcome::Held }
            guard "takeover" { self.step == WriterFenceAcquireStep::TakeoverOwn }
            update {
                self.stop_cause = WriterFenceStopCause::ReacquireFailed;
            }
            to Stopped
            emit WriterStopped { cause: self.stop_cause }
        }

        transition AcquireFailed {
            on input AcquireAnswered { outcome, token, lease_moved }
            guard "acquiring" { self.lifecycle_phase == Phase::Acquiring }
            guard "failed" { outcome == WriterFenceAcquireOutcome::Failed }
            update {
                self.stop_cause = WriterFenceStopCause::ReacquireFailed;
            }
            to Stopped
            emit WriterStopped { cause: self.stop_cause }
        }

        // The store may still grant an acquire the realm gave up on.
        transition AcquireTimedOut {
            on input AcquireAnswered { outcome, token, lease_moved }
            guard "acquiring" { self.lifecycle_phase == Phase::Acquiring }
            guard "timed_out" { outcome == WriterFenceAcquireOutcome::TimedOut }
            update {
                self.timed_out += 1;
                self.stop_cause = WriterFenceStopCause::ReacquireTimedOut;
            }
            to Stopped
            emit WriterStopped { cause: self.stop_cause }
        }

        transition AcquireAnsweredNotAcquiring {
            per_phase [Unopened, Live, Stopped, Proving, Resuming, WaitingOut]
            on input AcquireAnswered { outcome, token, lease_moved }
            update {}
            to Unopened
            emit RequestRefused { reason: WriterFenceRefusal::NotAcquiring }
        }

        transition AcquireAnsweredLost {
            on input AcquireAnswered { outcome, token, lease_moved }
            guard "lost" { self.lifecycle_phase == Phase::Lost }
            update {}
            to Lost
            emit RequestRefused { reason: WriterFenceRefusal::Lost }
        }

        // ------------------------------------------------------------------
        // LeaseEnded
        // ------------------------------------------------------------------

        transition LeaseEnded {
            on input LeaseEnded
            guard "waiting_out" { self.lifecycle_phase == Phase::WaitingOut }
            update {}
            to Stopped
        }

        transition LeaseEndedNotWaitingOut {
            per_phase [Unopened, Live, Stopped, Acquiring, Proving, Resuming]
            on input LeaseEnded
            update {}
            to Unopened
            emit RequestRefused { reason: WriterFenceRefusal::NotWaitingOut }
        }

        transition LeaseEndedLost {
            on input LeaseEnded
            guard "lost" { self.lifecycle_phase == Phase::Lost }
            update {}
            to Lost
            emit RequestRefused { reason: WriterFenceRefusal::Lost }
        }

        // ------------------------------------------------------------------
        // ListingChecked
        // ------------------------------------------------------------------

        // The listing proved the grant: install its token and renew it once.
        transition ListingClean {
            on input ListingChecked { outcome, token }
            guard "proving" { self.lifecycle_phase == Phase::Proving }
            guard "the_attempts_grant" { self.granted == Some(token) }
            guard "clean" { outcome == WriterFenceListingOutcome::Clean }
            update {
                self.token = token;
                self.own_grant = None;
            }
            to Resuming
            emit TokenInstalled { token: self.token }
        }

        transition ListingForeign {
            on input ListingChecked { outcome, token }
            guard "proving" { self.lifecycle_phase == Phase::Proving }
            guard "the_attempts_grant" { self.granted == Some(token) }
            guard "foreign_batch" { outcome == WriterFenceListingOutcome::ForeignBatch }
            update {
                self.granted = None;
                self.lost_cause = WriterFenceLostCause::ForeignBatch;
            }
            to Lost
            emit RealmLost { cause: self.lost_cause }
        }

        // The grant stays the writer's own (`own_grant`): the next re-acquire
        // takes its lease over.
        transition ListingFailed {
            on input ListingChecked { outcome, token }
            guard "proving" { self.lifecycle_phase == Phase::Proving }
            guard "the_attempts_grant" { self.granted == Some(token) }
            guard "failed" { outcome == WriterFenceListingOutcome::Failed }
            update {
                self.granted = None;
                self.stop_cause = WriterFenceStopCause::ListingFailed;
            }
            to Stopped
            emit WriterStopped { cause: self.stop_cause }
        }

        transition ListingOtherGrant {
            on input ListingChecked { outcome, token }
            guard "proving" { self.lifecycle_phase == Phase::Proving }
            guard "another_grant" { self.granted != Some(token) }
            update {}
            to Proving
            emit RequestRefused { reason: WriterFenceRefusal::NotProving }
        }

        transition ListingCheckedNotProving {
            per_phase [Unopened, Live, Stopped, Acquiring, Resuming, WaitingOut]
            on input ListingChecked { outcome, token }
            update {}
            to Unopened
            emit RequestRefused { reason: WriterFenceRefusal::NotProving }
        }

        transition ListingCheckedLost {
            on input ListingChecked { outcome, token }
            guard "lost" { self.lifecycle_phase == Phase::Lost }
            update {}
            to Lost
            emit RequestRefused { reason: WriterFenceRefusal::Lost }
        }

        // ------------------------------------------------------------------
        // Poison
        // ------------------------------------------------------------------

        transition Poison {
            on input Poison { cause }
            guard "opened_and_holding" {
                self.lifecycle_phase != Phase::Unopened && self.lifecycle_phase != Phase::Lost
            }
            update {
                self.granted = None;
                self.lost_cause = cause;
            }
            to Lost
            emit RealmLost { cause: self.lost_cause }
        }

        transition PoisonUnopened {
            on input Poison { cause }
            guard "unopened" { self.lifecycle_phase == Phase::Unopened }
            update {}
            to Unopened
            emit RequestRefused { reason: WriterFenceRefusal::NotOpened }
        }

        // The first cause stands.
        transition PoisonLost {
            on input Poison { cause }
            guard "lost" { self.lifecycle_phase == Phase::Lost }
            update {}
            to Lost
            emit RequestRefused { reason: WriterFenceRefusal::Lost }
        }

        // ------------------------------------------------------------------
        // OpenListingFailed
        // The open's listing failed: its realm is never served, and no
        // re-acquire may list against a baseline the open never installed.
        // ------------------------------------------------------------------

        transition OpenListingFailed {
            per_phase [Live, Stopped, Acquiring, Proving, Resuming, WaitingOut]
            on input OpenListingFailed
            update {
                self.open_listing_failed = true;
            }
            to Live
        }

        transition OpenListingFailedUnopened {
            on input OpenListingFailed
            guard "unopened" { self.lifecycle_phase == Phase::Unopened }
            update {}
            to Unopened
            emit RequestRefused { reason: WriterFenceRefusal::NotOpened }
        }

        transition OpenListingFailedLost {
            on input OpenListingFailed
            guard "lost" { self.lifecycle_phase == Phase::Lost }
            update {}
            to Lost
            emit RequestRefused { reason: WriterFenceRefusal::Lost }
        }

        // ------------------------------------------------------------------
        // ScopeQuarantined
        // In every opened phase: the open's listing, a load meeting a
        // conflict, and the runtime store's open may each record one. A
        // re-acquire records nothing (the realm compares its listing with
        // the records). Nothing removes a key or a record.
        // ------------------------------------------------------------------

        // A conflicted incarnation not recorded yet, on a scope not
        // poisoned: the key joins the set (if absent) and the record is
        // added.
        transition ScopeQuarantined {
            per_phase [Live, Stopped, Acquiring, Proving, Resuming, WaitingOut]
            on input ScopeQuarantined { key, name, version, commit_ids }
            guard "a_new_incarnation" { !self.quarantine_versions.contains_key(name) }
            guard "not_poisoned" { !self.poisoned_scopes.contains_key(key) }
            update {
                self.quarantined_keys.insert(key);
                self.quarantine_versions.insert(name, version);
                self.quarantine_commit_ids.insert(name, commit_ids);
            }
            to Live
            emit ScopeQuarantineRecorded { key: key, name: name }
        }

        // A recorded incarnation met for a key not quarantined yet, on a
        // scope not poisoned: a chunk's conflict, recorded under the chunk's
        // own name, that its owner's load meets. The key joins the set; the
        // record stands as recorded.
        transition ScopeQuarantinedNewKey {
            per_phase [Live, Stopped, Acquiring, Proving, Resuming, WaitingOut]
            on input ScopeQuarantined { key, name, version, commit_ids }
            guard "a_recorded_incarnation" { self.quarantine_versions.contains_key(name) }
            guard "a_new_key" { !self.quarantined_keys.contains(key) }
            guard "not_poisoned" { !self.poisoned_scopes.contains_key(key) }
            update {
                self.quarantined_keys.insert(key);
            }
            to Live
            emit ScopeQuarantineRecorded { key: key, name: name }
        }

        // A recorded incarnation again, for a key already quarantined: the
        // key and the record stand.
        transition ScopeQuarantinedAgain {
            per_phase [Live, Stopped, Acquiring, Proving, Resuming, WaitingOut]
            on input ScopeQuarantined { key, name, version, commit_ids }
            guard "a_recorded_incarnation" { self.quarantine_versions.contains_key(name) }
            guard "a_quarantined_key" { self.quarantined_keys.contains(key) }
            update {}
            to Live
        }

        // A poisoned scope stays poisoned: one classification per scope.
        transition ScopeQuarantinedPoisoned {
            per_phase [Live, Stopped, Acquiring, Proving, Resuming, WaitingOut]
            on input ScopeQuarantined { key, name, version, commit_ids }
            guard "poisoned" { self.poisoned_scopes.contains_key(key) }
            guard "a_new_incarnation_or_key" {
                !self.quarantine_versions.contains_key(name) || !self.quarantined_keys.contains(key)
            }
            update {}
            to Live
            emit RequestRefused { reason: WriterFenceRefusal::Poisoned }
        }

        transition ScopeQuarantinedUnopened {
            on input ScopeQuarantined { key, name, version, commit_ids }
            guard "unopened" { self.lifecycle_phase == Phase::Unopened }
            update {}
            to Unopened
            emit RequestRefused { reason: WriterFenceRefusal::NotOpened }
        }

        transition ScopeQuarantinedLost {
            on input ScopeQuarantined { key, name, version, commit_ids }
            guard "lost" { self.lifecycle_phase == Phase::Lost }
            update {}
            to Lost
            emit RequestRefused { reason: WriterFenceRefusal::Lost }
        }

        // ------------------------------------------------------------------
        // ScopePoisoned
        // The first cause stands, and nothing removes an entry before a
        // reopen.
        // ------------------------------------------------------------------

        transition ScopePoisoned {
            per_phase [Live, Stopped, Acquiring, Proving, Resuming, WaitingOut]
            on input ScopePoisoned { scope, cause }
            guard "not_poisoned" { !self.poisoned_scopes.contains_key(scope) }
            guard "not_quarantined" { !self.quarantined_keys.contains(scope) }
            update {
                self.poisoned_scopes.insert(scope, cause);
            }
            to Live
            emit ScopePoisonRecorded { scope: scope, cause: cause }
        }

        transition ScopePoisonedAgain {
            per_phase [Live, Stopped, Acquiring, Proving, Resuming, WaitingOut]
            on input ScopePoisoned { scope, cause }
            guard "poisoned" { self.poisoned_scopes.contains_key(scope) }
            update {}
            to Live
        }

        // A quarantined scope keeps its quarantine, which already refuses
        // everything a poison refuses.
        transition ScopePoisonedQuarantined {
            per_phase [Live, Stopped, Acquiring, Proving, Resuming, WaitingOut]
            on input ScopePoisoned { scope, cause }
            guard "not_poisoned" { !self.poisoned_scopes.contains_key(scope) }
            guard "quarantined" { self.quarantined_keys.contains(scope) }
            update {}
            to Live
            emit RequestRefused { reason: WriterFenceRefusal::Quarantined }
        }

        transition ScopePoisonedUnopened {
            on input ScopePoisoned { scope, cause }
            guard "unopened" { self.lifecycle_phase == Phase::Unopened }
            update {}
            to Unopened
            emit RequestRefused { reason: WriterFenceRefusal::NotOpened }
        }

        transition ScopePoisonedLost {
            on input ScopePoisoned { scope, cause }
            guard "lost" { self.lifecycle_phase == Phase::Lost }
            update {}
            to Lost
            emit RequestRefused { reason: WriterFenceRefusal::Lost }
        }
    }
}
