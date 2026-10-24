//! Typed per-member result of a mob Stop (#1500).
//!
//! Stop pauses a mob. Each member's runtime is held, so input admitted before
//! the stop stays queued until Resume instead of starting a run while the mob
//! is Stopped. The report says, for every member, what happened to the run it
//! had and whether its run starts are actually held. A member whose runtime
//! cannot be held (a remote peer without the capability) is reported as such
//! rather than hidden behind a bare `Ok`.

use std::collections::BTreeMap;

use meerkat_core::lifecycle::RunId;

use crate::AgentIdentity;

/// What a mob Stop did to each member.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MobStopReport {
    /// One outcome per member the stop reached, by identity.
    pub members: BTreeMap<AgentIdentity, MemberStopOutcome>,
}

impl MobStopReport {
    /// Members whose run starts are not held: they may still start a turn
    /// while the mob is Stopped. A member reported
    /// [`MemberRunStarts::ReloadRequired`] is not one: its degraded
    /// registration starts no run, and the `MobStop` the Stop staged for it
    /// first holds the registration a reload replaces it with until Resume.
    pub fn not_holdable(&self) -> impl Iterator<Item = (&AgentIdentity, &NotHoldableReason)> {
        self.members
            .iter()
            .filter_map(|(identity, outcome)| match &outcome.starts {
                MemberRunStarts::NotHoldable { reason } => Some((identity, reason)),
                MemberRunStarts::Held
                | MemberRunStarts::NotBound
                | MemberRunStarts::DelegatedToHost
                | MemberRunStarts::ReloadRequired => None,
            })
    }
}

/// Caller options for
/// [`MobHandle::stop_with_options`](super::MobHandle::stop_with_options).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct StopOptions {
    /// Leave every member's current run running. See
    /// [`Self::leave_runs_running`].
    pub leave_runs_running: bool,
}

impl StopOptions {
    /// Leave every member's current run running: an autonomous member's run
    /// is not cancelled at its next boundary, as a plain Stop does. Its run
    /// starts are held, and the Stop reports the run `LeftRunning` and
    /// returns without waiting for any turn. Its kickoff stop intent is
    /// recorded as with a plain Stop, except for a local autonomous member
    /// whose session commits each round: that member's kickoff stays open
    /// for the Shutdown that follows to decide. A Stop that arrives while a
    /// plain Stop still waits for the turns it cancelled joins that Stop and
    /// returns its report. A Stop with this option does not recall cancels
    /// that an earlier, failed plain Stop already dispatched: those runs
    /// still end at their next boundary.
    ///
    /// Meant for Stop, then
    /// [`MobHandle::drain_stopped_runs`](super::MobHandle::drain_stopped_runs),
    /// then Shutdown. The default Shutdown keeps a run that commits each
    /// round, and a durable member's kickoff with it, for the mob's next
    /// start, and ends every other run still running; a Shutdown with an
    /// explicit [`QueuedInputsOnShutdown::Stop`](super::QueuedInputsOnShutdown::Stop)
    /// ends them all. A host that stops with this option and never shuts
    /// down leaves autonomous turns running under a Stopped mob, and a
    /// durable member's kickoff open until its run ends.
    #[must_use]
    pub fn leave_runs_running(mut self) -> Self {
        self.leave_runs_running = true;
        self
    }
}

/// One member's stop outcome.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemberStopOutcome {
    /// The member's run when the stop reached it.
    pub run: MemberStopRun,
    /// Whether the member can start a new run before Resume.
    pub starts: MemberRunStarts,
}

/// The member's run when the stop reached it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "run", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemberStopRun {
    /// The member had no run, so there was nothing to cancel.
    NoRun,
    /// The run was cancelled at its next boundary; the stop waits for it.
    CancelledAtBoundary { run_id: RunId },
    /// A mob Shutdown cancelled the run immediately rather than at its next
    /// boundary: the run's recorded terminal is the cancel.
    CancelledByShutdown { run_id: RunId },
    /// A mob Shutdown dispatched its immediate cancel and the runtime owns
    /// its outcome. Shutdown resolves it from the run's recorded terminal once
    /// the run has settled (`CancelledByShutdown`, or `RunEndedBeforeCancel`
    /// when the run ended on its own first); it is reported as is only when
    /// the run recorded no terminal before the Shutdown's deadline.
    CancelDispatched { run_id: RunId },
    /// The run ended on its own between the hold and the cancel.
    RunEndedBeforeCancel { run_id: RunId },
    /// A run the Stop left running: a turn-driven member's, or any member's
    /// under [`StopOptions::leave_runs_running`]. It finishes normally, and
    /// no new run starts after it until Resume.
    LeftRunning { run_id: RunId },
    /// A remote member that cannot report its run (it lacks the run-start
    /// hold capability); the stop sent it the ambient interrupt.
    Interrupted,
    /// The member's runtime registration is durability-degraded: it refused
    /// the hold, or it degraded after the hold was placed and refused the
    /// cancel of the run the hold found current (the run starts then report
    /// `Held`, and nothing was dispatched). No outcome for its run is recorded
    /// on that registration: durable truth decides it, at the reload that
    /// replaces the registration or at the discard a mob Shutdown performs.
    /// This does not say the run was cancelled.
    ReloadRequired,
}

/// Whether a member's run starts are held until Resume.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "starts", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemberRunStarts {
    /// No new run starts until Resume.
    Held,
    /// The member's runtime could not be held: it may start a turn while the
    /// mob is Stopped.
    NotHoldable { reason: NotHoldableReason },
    /// The member is not bound to this mob right now, so the hold did not
    /// reach it, and nothing from the mob reaches it either: a placed member
    /// whose host carrier is dormant (MobMachine re-activates it only while
    /// Running), or a remote peer that is unbound (a bind while the mob is
    /// Stopped delivers the hold first).
    NotBound,
    /// Shutdown only, never in a Stop report (a Stop always contacts remote
    /// members): a remote member a mob Shutdown did not contact. Its
    /// host owns its run starts and teardown; Shutdown never probes a remote
    /// host it is not otherwise stopping a member on.
    DelegatedToHost,
    /// The member's runtime registration needs a durability reload: no run
    /// starts on it until it is reloaded, and a mob Shutdown discards it
    /// instead. No hold was placed on it. A Stop staged `MobStop` for the
    /// member before the hold, so the registration a reload replaces it with
    /// is held until Resume.
    ReloadRequired,
}

/// Why a member's run starts could not be held.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum NotHoldableReason {
    /// A remote member whose host does not support the run-start hold.
    PeerLacksCapability,
    /// The provisioner serving the member does not implement the hold.
    ProvisionerLacksCapability,
}

/// A run-start hold a mob's host places on restored members and releases
/// itself (#1500). A mob Stop's hold is not one of these: only Resume
/// releases it.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum HostRunStartHoldReason {
    /// The member's tools are not published yet: it starts no run until
    /// its host releases this hold.
    ToolsNotPublished,
}

impl HostRunStartHoldReason {
    pub(crate) fn runtime(self) -> meerkat_runtime::RunStartHoldReason {
        match self {
            Self::ToolsNotPublished => meerkat_runtime::RunStartHoldReason::ToolsNotPublished,
        }
    }
}

/// What [`MobHandle::drain_stopped_runs`](super::MobHandle::drain_stopped_runs)
/// found for each member of a Stopped mob: whether the run each member had
/// when the drain started ended within its bound.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct MobRunDrainReport {
    /// One outcome per roster member, by identity.
    pub members: BTreeMap<AgentIdentity, MemberRunDrainOutcome>,
}

/// One member's drain outcome: what happened to its run, and whether its run
/// starts are held, as the Stop the drain follows reported them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct MemberRunDrainOutcome {
    /// The member's run, as the drain saw it.
    pub run: MemberRunDrain,
    /// The member's run starts as this process's last Stop recorded them;
    /// `None` when no Stop outcome is recorded in this process (the mob was
    /// stopped before a restart, or by another process). A member whose
    /// starts are not `Held` may have started runs the drain neither waited
    /// for nor reports.
    pub starts: Option<MemberRunStarts>,
}

/// A member's run as a drain saw it. Only the run current when the drain
/// started is drained.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "run", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MemberRunDrain {
    /// The member had no run when the drain started.
    NoRun,
    /// The run ended within the bound. `terminal` is the run's recorded
    /// terminal, or `None` when its runtime registration ended with it.
    Ended {
        run_id: RunId,
        terminal: Option<meerkat_core::turn_execution_authority::TurnTerminalOutcome>,
    },
    /// The run was still current at the bound. The drain does not cancel it:
    /// a mob Shutdown that follows does, as without a drain.
    StillRunning { run_id: RunId },
    /// The member's session had an open live channel when the drain started.
    /// Its run, if any, was not waited for.
    LiveChannelOpen { run_id: Option<RunId> },
    /// A placed or remote member: its host owns its run.
    NotLocal,
    /// The member's runtime registration needs a durability reload: no run
    /// outcome is recorded on it.
    ReloadRequired,
}
