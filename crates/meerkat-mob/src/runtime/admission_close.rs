//! Public ownership of a running mob and the Closing admission state (D67).
//!
//! Every exported [`MobHandle`](super::MobHandle) clone carries one shared
//! public-owner token. The actor's own clones (flow executor, event pumps,
//! recovery drivers, member tools) and read-only views do not, so they never
//! keep a mob open for its owners. A foreground operation started on behalf
//! of an owner can take a lease of its own with `acquire_owner`, from any
//! handle, while admission is open.
//!
//! When the last owner (token or lease) drops, the actor observes the release
//! on its serialized loop and closes admission as soon as the mob is idle. An
//! explicit `close_if_idle` and an unconditional `close_admission` enter the
//! same Closing state. Owner presence and the close are decided together under
//! one lock ([`MobOwnership`]), so an owner acquired before an idle close
//! keeps that close from closing (an unconditional `close_admission` closes
//! with owners and admitted work present, by design) and an acquisition after
//! any close is refused. Closing never stops
//! admitted work: the ordinary Stop, Shutdown and Destroy remain the teardown
//! path. An admitted Shutdown closes admission itself, before its teardown;
//! so do an admitted Destroy, a durability fail-stop and a dispatch failure
//! that stops the actor, and a lost actor task closes it from its spawn
//! wrapper, so no actor exit leaves admission open. A Stop leaves it open.

use std::sync::{Arc, Mutex};

use crate::tokio::sync::Notify;

/// Why a mob's admission closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MobAdmissionCloseReason {
    /// An explicit `close_if_idle` found the mob idle.
    IdleClose,
    /// The last public owner handle dropped and the mob was then idle.
    LastOwnerReleased,
    /// The embedder closed admission unconditionally to stand the mob down.
    StandDown,
    /// The mob admitted a Shutdown: [`MobHandle::shutdown`](super::MobHandle::shutdown)
    /// closes admission before its teardown, and the actor accepts no more
    /// commands once the Shutdown completes.
    Shutdown,
    /// The mob's destroy was admitted (its durable destroy marker is
    /// written): the actor exits once the destroy completes.
    Destroyed,
    /// The actor fail-stopped for a cold recovery and takes no further
    /// command: a durable mutation's outcome stayed uncertain, or its durable
    /// recovery failed before it admitted a command.
    DurabilityFailStop,
    /// The actor task ended without its loop returning (a panic or an
    /// abort), so the mob takes no further command.
    ActorLost,
    /// A structural composition dispatch failure at a loop boundary stopped
    /// the actor, so the mob takes no further command.
    DispatchFailed,
}

impl std::fmt::Display for MobAdmissionCloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IdleClose => f.write_str("idle close"),
            Self::LastOwnerReleased => f.write_str("last public owner released"),
            Self::StandDown => f.write_str("stand down"),
            Self::Shutdown => f.write_str("shutdown"),
            Self::Destroyed => f.write_str("destroyed"),
            Self::DurabilityFailStop => f.write_str("durability fail-stop"),
            Self::ActorLost => f.write_str("actor lost"),
            Self::DispatchFailed => f.write_str("dispatch failure"),
        }
    }
}

/// What kept a mob open when it was asked to close. Counts are diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MobBusy {
    /// Members being spawned, retired, revived or respawned, unresolved
    /// kickoffs and identity convergence, and mob lifecycle operations (stop,
    /// resume, shutdown) still in progress.
    pub roster_transitions: usize,
    /// Flow runs, member sessions holding accepted work (queued, held or
    /// running) or an open live channel, peer deliveries and member effects
    /// in flight.
    pub runs: usize,
    /// Work parked or deferred ahead of its admission.
    pub parked_admission: usize,
    /// Foreground operations holding an owner lease from `acquire_owner`.
    pub foreground_owners: usize,
}

impl MobBusy {
    /// Whether nothing kept the mob open.
    pub fn is_idle(&self) -> bool {
        self.roster_transitions == 0
            && self.runs == 0
            && self.parked_admission == 0
            && self.foreground_owners == 0
    }
}

/// Answer of [`MobHandle::close_if_idle`](super::MobHandle::close_if_idle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CloseIfIdleOutcome {
    /// The mob was idle; admission is now closed.
    Closed,
    /// Admission was already closed, for this reason.
    AlreadyClosed(MobAdmissionCloseReason),
    /// Work, a roster transition or a foreground owner was in flight;
    /// admission stays open and nothing was touched.
    Busy(MobBusy),
}

/// Work the mob machine records as held or owed, which the idle close's
/// counts ([`MobBusy`]) do not read. Each field is one fact of the machine's
/// own state, read by the actor in the step of the observation; none is
/// worked out from another. The idle close is not decided on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct MobMachineHeld {
    /// Member incarnations the machine marks as retiring.
    pub members_retiring: usize,
    /// Flow runs the machine holds pending or running.
    pub flow_runs_open: usize,
    /// Members that are not materialized and are owed a wake.
    pub boot_work_owed: usize,
    /// Completions and turn outcomes still owed for members placed on a
    /// host.
    pub placed_obligations: usize,
    /// Route installs, recipient trust and edge repairs still owed.
    pub wiring_obligations: usize,
    /// A declared apply is between its first and its last step.
    pub declared_apply_open: bool,
    /// Constructions of identity members that are held or activating.
    pub constructions_held: usize,
    /// An adaptive run is open.
    pub adaptive_run_open: bool,
    /// Host binds requested and not yet bound.
    pub host_binds_requested: usize,
}

impl MobMachineHeld {
    /// Whether the machine records none of these.
    pub fn is_none(&self) -> bool {
        self.members_retiring == 0
            && self.flow_runs_open == 0
            && self.boot_work_owed == 0
            && self.placed_obligations == 0
            && self.wiring_obligations == 0
            && !self.declared_apply_open
            && self.constructions_held == 0
            && !self.adaptive_run_open
            && self.host_binds_requested == 0
    }
}

/// One reason an observation that counted nothing in flight still cannot
/// say that nothing is: a piece of evidence it did not read. Each cause is a
/// fact of its own, never worked out from another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MobUnknownCause {
    /// The mob has no runtime adapter (or the build has none), so no
    /// member's accepted work or live channel was read.
    NoRuntimeAdapter,
    /// The mob machine records a placement for a member: it runs on a host,
    /// and its own work is not read from here.
    MembersPlaced,
    /// A member has no session the runtime of this process holds, so its
    /// accepted work was not read.
    SessionsNotReadable,
    /// The step between the actor's delivery of a peer message and the
    /// member's accepted input is not traced.
    PeerDeliveryNotTraced,
    /// Work the actor handed to a task of its own, and has not yet taken
    /// back, is not tracked.
    HandoffsNotTracked,
}

impl MobUnknownCause {
    /// The cause's closed word, as surfaces render it.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoRuntimeAdapter => "no_runtime_adapter",
            Self::MembersPlaced => "members_placed",
            Self::SessionsNotReadable => "sessions_not_readable",
            Self::PeerDeliveryNotTraced => "peer_delivery_not_traced",
            Self::HandoffsNotTracked => "handoffs_not_tracked",
        }
    }
}

/// What one observation did not read. Each field is recorded by the actor
/// as the fact it is; [`Self::causes`] names the ones that stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct MobUnread {
    /// [`MobUnknownCause::NoRuntimeAdapter`].
    pub no_runtime_adapter: bool,
    /// Members the machine records a placement for
    /// ([`MobUnknownCause::MembersPlaced`]).
    pub members_placed: usize,
    /// Members with no session the runtime of this process holds
    /// ([`MobUnknownCause::SessionsNotReadable`]).
    pub sessions_not_readable: usize,
    /// [`MobUnknownCause::PeerDeliveryNotTraced`]. True of every
    /// observation in this release.
    pub peer_delivery_not_traced: bool,
    /// [`MobUnknownCause::HandoffsNotTracked`]. True of every observation
    /// in this release.
    pub handoffs_not_tracked: bool,
}

impl MobUnread {
    /// The causes that stand, in a fixed order.
    pub fn causes(&self) -> impl Iterator<Item = MobUnknownCause> + '_ {
        [
            (self.no_runtime_adapter, MobUnknownCause::NoRuntimeAdapter),
            (self.members_placed > 0, MobUnknownCause::MembersPlaced),
            (
                self.sessions_not_readable > 0,
                MobUnknownCause::SessionsNotReadable,
            ),
            (
                self.peer_delivery_not_traced,
                MobUnknownCause::PeerDeliveryNotTraced,
            ),
            (
                self.handoffs_not_tracked,
                MobUnknownCause::HandoffsNotTracked,
            ),
        ]
        .into_iter()
        .filter_map(|(stands, cause)| stands.then_some(cause))
    }
}

/// How a mob that takes no more work ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MobFinished {
    /// The mob's lifecycle is Completed.
    Completed,
    /// The mob's lifecycle is Destroyed (a destroy is admitted).
    Destroyed,
}

/// What one observation says about the mob as a whole
/// ([`MobIdleObservation::standing`]). A match on it keeps a wildcard arm:
/// a later release adds the answer for a mob that is observed idle.
///
/// No value of this release says that the mob is idle. An observation that
/// counted nothing in flight, on a running mob with admission open, answers
/// [`Self::Unknown`] with the evidence it did not read: the released
/// [`MobIdleObservation::is_idle`] is true of the same observation and keeps
/// its meaning (the mob an idle close would have closed at that instant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MobStanding {
    /// Work is in flight: a count of [`MobBusy`] is not zero, the mob
    /// machine records work it holds or owes ([`MobMachineHeld`]), or the
    /// mob is still being created.
    Busy,
    /// Nothing was counted in flight and the mob's lifecycle has ended: it
    /// takes no more work.
    Finished(MobFinished),
    /// Nothing was counted in flight and admission is closed, for this
    /// reason, on a mob whose lifecycle has not ended: this actor takes no
    /// more work. A mob closed because it was idle is closed, not finished.
    Closed(MobAdmissionCloseReason),
    /// Nothing was counted in flight and the mob is Stopped: paused, with
    /// admission open, until it is resumed.
    Stopped,
    /// Nothing was counted in flight on a running mob with admission open,
    /// and the observation did not read all the evidence that nothing is.
    /// [`MobUnread::causes`] names what it did not read.
    Unknown(MobUnread),
}

impl MobStanding {
    /// The standing's closed word, as surfaces render it.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Busy => "busy",
            Self::Finished(_) => "finished",
            Self::Closed(_) => "closed",
            Self::Stopped => "stopped",
            Self::Unknown(_) => "unknown",
        }
    }
}

/// Answer of [`MobHandle::observe_idle`](super::MobHandle::observe_idle):
/// what one observation on the actor's serialized admission path found, the
/// same facts an idle close is decided on. Nothing is decided by it and
/// nothing is closed, held or reserved: the mob may be busy again, or
/// closed, when the caller next acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct MobIdleObservation {
    /// What was in flight. The counts are diagnostic: their categories
    /// overlap, and the native work of the members' sessions is read only
    /// when the actor's own counts are idle.
    pub busy: MobBusy,
    /// Why the mob's admission is closed, when it is. A mob whose admission
    /// is closed takes no new work, whatever `busy` says.
    pub admission_closed: Option<MobAdmissionCloseReason>,
    /// The mob's lifecycle phase, read by the actor in the same step.
    pub phase: crate::MobState,
    /// Work the mob machine records as held or owed, which `busy` does not
    /// read.
    pub machine_held: MobMachineHeld,
    /// What the observation did not read.
    pub unread: MobUnread,
}

impl MobIdleObservation {
    /// Whether the observation found nothing in flight and admission open:
    /// the mob an idle close would have closed at that instant. It reads
    /// `busy` and `admission_closed` only, as it always has: it is true of
    /// a Stopped mob and of a Completed one whose admission is open, and it
    /// does not know what the observation did not read. Ask
    /// [`Self::standing`] to tell those apart.
    pub fn is_idle(&self) -> bool {
        self.busy.is_idle() && self.admission_closed.is_none()
    }

    /// What the observation says about the mob as a whole, as one typed
    /// answer: busy, finished, closed, stopped, or unknown with the
    /// evidence it did not read.
    ///
    /// It covers the mob's own admission and nothing outside it: a schedule
    /// that is due, a delivery another owner holds for one of its members,
    /// and the mobs its members created are outside the answer's scope, not
    /// unread evidence of it.
    ///
    /// The answer is never "idle" in this release. Two pieces of evidence
    /// are not read by any observation yet (work the actor handed to a task
    /// of its own and has not taken back, and a peer message between its
    /// delivery and the member's accepted input), so a quiet running mob
    /// answers [`MobStanding::Unknown`] with those causes, beside the ones
    /// that hold for it alone (no runtime adapter, a placed member, a
    /// session the runtime here does not hold).
    pub fn standing(&self) -> MobStanding {
        use crate::MobState;
        if !self.busy.is_idle() || !self.machine_held.is_none() || self.phase == MobState::Creating
        {
            return MobStanding::Busy;
        }
        match (self.phase, self.admission_closed) {
            (MobState::Completed, _) => MobStanding::Finished(MobFinished::Completed),
            (MobState::Destroyed, _) => MobStanding::Finished(MobFinished::Destroyed),
            (_, Some(reason)) => MobStanding::Closed(reason),
            (MobState::Stopped, None) => MobStanding::Stopped,
            (MobState::Running | MobState::Creating, None) => MobStanding::Unknown(self.unread),
        }
    }
}

/// Owner presence and the Closing state of one mob, shared by its handles and
/// its actor. The actor writes `closed`, and so does the actor task's end
/// guard, only once the actor future has stopped (a lost task); every write
/// decides under the same lock that owner acquisition takes.
pub(super) struct MobOwnership {
    state: Mutex<OwnershipState>,
    /// Signalled when the last owner drops. A stored permit survives until
    /// the actor waits, so the release is never lost.
    released: Notify,
}

#[derive(Default)]
struct OwnershipState {
    root_owner: bool,
    leases: usize,
    closed: Option<MobAdmissionCloseReason>,
}

impl OwnershipState {
    fn owned(&self) -> bool {
        self.root_owner || self.leases > 0
    }
}

/// The close the actor is deciding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CloseRequest {
    /// `close_if_idle`: closes only an idle mob with no foreground owner.
    IfIdle,
    /// The latched last-owner release: closes only when no owner is present
    /// any more and the mob is idle.
    LastOwnerReleased,
    /// `close_admission`: closes unconditionally, keeping the first reason.
    Unconditional(MobAdmissionCloseReason),
}

/// The actor's decision for one [`CloseRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CloseDecision {
    /// Admission closed now, for this reason; `busy` is what was in flight.
    Closed {
        reason: MobAdmissionCloseReason,
        busy: MobBusy,
    },
    /// Admission was already closed.
    AlreadyClosed {
        reason: MobAdmissionCloseReason,
        busy: MobBusy,
    },
    /// Admission stays open.
    Busy(MobBusy),
    /// A last-owner release found an owner present again: nothing to close.
    OwnerPresent,
}

impl MobOwnership {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(OwnershipState::default()),
            released: Notify::new(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, OwnershipState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Mint the root token the launched handle carries. Called once.
    pub(super) fn mint_root(self: &Arc<Self>) -> Arc<MobPublicOwner> {
        self.lock().root_owner = true;
        Arc::new(MobPublicOwner {
            ownership: Arc::clone(self),
            kind: OwnerKind::Root,
        })
    }

    /// Take an owner lease while admission is open.
    pub(super) fn acquire_lease(
        self: &Arc<Self>,
    ) -> Result<Arc<MobPublicOwner>, MobAdmissionCloseReason> {
        let mut state = self.lock();
        if let Some(reason) = state.closed {
            return Err(reason);
        }
        state.leases = state.leases.saturating_add(1);
        drop(state);
        Ok(Arc::new(MobPublicOwner {
            ownership: Arc::clone(self),
            kind: OwnerKind::Lease,
        }))
    }

    /// Resolves after the last owner dropped (or immediately, for a release
    /// signalled before the wait).
    pub(super) fn released(&self) -> &Notify {
        &self.released
    }

    /// Decide one close. `busy` is evaluated under the ownership lock, on the
    /// actor's serialized path, so no owner can be acquired between the
    /// decision and the close.
    pub(super) fn decide(
        &self,
        request: CloseRequest,
        busy: impl FnOnce() -> MobBusy,
    ) -> CloseDecision {
        let mut state = self.lock();
        if request == CloseRequest::LastOwnerReleased && state.owned() {
            return CloseDecision::OwnerPresent;
        }
        let mut busy = busy();
        busy.foreground_owners = state.leases;
        if let Some(reason) = state.closed {
            return CloseDecision::AlreadyClosed { reason, busy };
        }
        let reason = match request {
            CloseRequest::IfIdle => MobAdmissionCloseReason::IdleClose,
            CloseRequest::LastOwnerReleased => MobAdmissionCloseReason::LastOwnerReleased,
            CloseRequest::Unconditional(reason) => reason,
        };
        if !matches!(request, CloseRequest::Unconditional(_)) && !busy.is_idle() {
            return CloseDecision::Busy(busy);
        }
        state.closed = Some(reason);
        CloseDecision::Closed { reason, busy }
    }

    /// Read what a close is decided on, and decide nothing: `busy` with the
    /// foreground leases, and why admission is closed when it is, both under
    /// the ownership lock. The root owner is not a busy term, as it is none
    /// for an idle close. Nothing is written. The phase, the machine's held
    /// work and what was not read are the actor's own reads of the same
    /// step, carried as given.
    pub(super) fn observe(
        &self,
        mut busy: MobBusy,
        phase: crate::MobState,
        machine_held: MobMachineHeld,
        unread: MobUnread,
    ) -> MobIdleObservation {
        let state = self.lock();
        busy.foreground_owners = state.leases;
        MobIdleObservation {
            busy,
            admission_closed: state.closed,
            phase,
            machine_held,
            unread,
        }
    }

    fn release(&self, kind: OwnerKind) {
        let mut state = self.lock();
        match kind {
            OwnerKind::Root => state.root_owner = false,
            OwnerKind::Lease => state.leases = state.leases.saturating_sub(1),
        }
        let last = !state.owned() && state.closed.is_none();
        drop(state);
        if last {
            self.released.notify_one();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnerKind {
    Root,
    Lease,
}

/// One owner of a mob: the root token shared by the launched handle's clones,
/// or a lease taken by `acquire_owner` and shared by that handle's clones.
pub(super) struct MobPublicOwner {
    ownership: Arc<MobOwnership>,
    kind: OwnerKind,
}

impl Drop for MobPublicOwner {
    fn drop(&mut self) {
        self.ownership.release(self.kind);
    }
}
