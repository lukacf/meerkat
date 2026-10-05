//! Typed settlement of a member retirement, and the per-member outcome of a
//! mob Shutdown.
//!
//! A retirement that has durably started (its `MemberRetirementStarted` fact
//! is journaled) always has an owner that drives it to a terminal state. The
//! caller of [`MobHandle::retire`](super::MobHandle::retire) may stop waiting;
//! the retirement does not stop. Its progress and terminal outcome are
//! published on a per-member watch that any handle can observe through
//! [`MobHandle::retirement_settlement`](super::MobHandle::retirement_settlement).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::error::MobError;
use crate::ids::{AgentIdentity, Generation};

/// The retirement stage a member is in, named by the stage's effect.
///
/// Stage names are stable diagnostics, not a protocol: compare a stage with
/// [`RetirementStage::as_str`] only for display or logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RetirementStage(&'static str);

impl RetirementStage {
    /// The stage before the actor has dispatched any effect.
    pub const ADMISSION: Self = Self("retire-admission");

    pub(crate) const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// The stage's stable name, for display and logging.
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for RetirementStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// Where a member's retirement stands.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum RetirementSettlement {
    /// The retirement is running and owned by the mob actor.
    InProgress { stage: RetirementStage },
    /// The member is retired: its terminal fact is journaled and its roster
    /// entry removed.
    Retired,
    /// The retirement durably started, then a stage failed (or never received
    /// the signal it waits on within the lifecycle hang guard). The member
    /// stays `Retiring`, owned by the mob actor's stuck-retirement registry;
    /// [`MobHandle::redrive_retirement`](super::MobHandle::redrive_retirement)
    /// or a mob resume drives it again.
    Stuck {
        stage: RetirementStage,
        cause: Arc<MobError>,
    },
    /// The retirement failed before it durably started; the member is
    /// unchanged.
    NotStarted { cause: Arc<MobError> },
}

impl RetirementSettlement {
    /// Whether this is a terminal outcome rather than progress.
    pub fn is_settled(&self) -> bool {
        !matches!(self, Self::InProgress { .. })
    }
}

/// Observer of one retirement of one member incarnation.
///
/// Each retirement of an incarnation publishes on its own channel. A terminal
/// value is final for that channel: a re-drive of a `Stuck` retirement is a
/// new retirement on a new channel, which
/// [`MobHandle::retirement_settlement`](super::MobHandle::retirement_settlement)
/// returns from then on. The terminal value of the latest retirement stays
/// readable (a tombstone) until the identity's next retirement, so "retired
/// and gone" is distinguishable from "never retired".
#[derive(Debug, Clone)]
pub struct RetirementSettlementWatch {
    generation: Generation,
    rx: crate::tokio::sync::watch::Receiver<RetirementSettlement>,
}

impl RetirementSettlementWatch {
    /// The member incarnation (roster generation) this retirement belongs to.
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// The settlement as currently published.
    pub fn current(&self) -> RetirementSettlement {
        self.rx.borrow().clone()
    }

    /// Wait until this retirement reaches a terminal outcome and return it:
    /// `Retired`, `Stuck` or `NotStarted`, never `InProgress`.
    ///
    /// Every terminal outcome is published before the actor releases the
    /// retirement, so this resolves on the actor's own transition. `None`
    /// means the actor exited with this retirement still in progress (a
    /// durably started one resumes on the next start).
    pub async fn settled(&mut self) -> Option<RetirementSettlement> {
        self.rx
            .wait_for(RetirementSettlement::is_settled)
            .await
            .ok()
            .map(|settlement| settlement.clone())
    }
}

/// Member lifecycle observations written only by the mob actor and shared
/// read-only with its handles: per-member retirement settlements and the
/// report of the most recent Shutdown.
#[derive(Debug, Default)]
pub(crate) struct MemberLifecycleObservations {
    members: std::sync::Mutex<BTreeMap<AgentIdentity, SettlementChannel>>,
    last_shutdown_report: std::sync::Mutex<Option<MobShutdownReport>>,
    /// Test probe: set once a Shutdown parks on its off-actor teardown.
    #[cfg(test)]
    pub(crate) shutdown_teardown_parked: crate::tokio::sync::watch::Sender<bool>,
    /// Test probe: set once a Shutdown passes its admission, before its
    /// lifecycle drains.
    #[cfg(test)]
    pub(crate) shutdown_admitted: crate::tokio::sync::watch::Sender<bool>,
}

#[derive(Debug)]
struct SettlementChannel {
    generation: Generation,
    sender: crate::tokio::sync::watch::Sender<RetirementSettlement>,
}

impl MemberLifecycleObservations {
    fn members(&self) -> std::sync::MutexGuard<'_, BTreeMap<AgentIdentity, SettlementChannel>> {
        self.members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Publish `settlement` for the retirement of `identity`'s `generation`.
    /// Progress after a terminal value, or any value for another incarnation,
    /// starts a new channel, so observers of the previous retirement keep its
    /// terminal value. The latest channel is kept (a tombstone) until the
    /// identity's next retirement.
    pub(crate) fn publish(
        &self,
        identity: &AgentIdentity,
        generation: Generation,
        settlement: RetirementSettlement,
    ) {
        let mut members = self.members();
        match members.get(identity) {
            Some(channel)
                if channel.generation == generation
                    && (!channel.sender.borrow().is_settled() || settlement.is_settled()) =>
            {
                channel.sender.send_replace(settlement);
            }
            _ => {
                let (sender, _) = crate::tokio::sync::watch::channel(settlement);
                members.insert(identity.clone(), SettlementChannel { generation, sender });
            }
        }
    }

    /// Current settlement of `identity`'s latest retirement, if any.
    pub(crate) fn current(&self, identity: &AgentIdentity) -> Option<RetirementSettlement> {
        self.members()
            .get(identity)
            .map(|channel| channel.sender.borrow().clone())
    }

    /// Record the report of a completed Shutdown, before its reply is sent.
    pub(crate) fn store_shutdown_report(&self, report: MobShutdownReport) {
        *self
            .last_shutdown_report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(report);
    }

    /// The report of the most recent completed Shutdown.
    pub(crate) fn last_shutdown_report(&self) -> Option<MobShutdownReport> {
        self.last_shutdown_report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn subscribe(&self, identity: &AgentIdentity) -> Option<RetirementSettlementWatch> {
        self.members()
            .get(identity)
            .map(|channel| RetirementSettlementWatch {
                generation: channel.generation,
                rx: channel.sender.subscribe(),
            })
    }
}

/// What a mob Shutdown did for one member.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum MemberShutdownOutcome {
    /// The member's runtime binding was unregistered.
    Unregistered,
    /// The member's in-flight retirement was interrupted at `stage`; it stays
    /// `Retiring` and the next resume drives it again.
    RetirementInterrupted { stage: RetirementStage },
    /// The member's retirement was already stuck at `stage`.
    RetirementStuck {
        stage: RetirementStage,
        cause: Arc<MobError>,
    },
    /// The runtime refused or has not yet completed the unregister; `stage`
    /// names the unregister step.
    UnregisterPending { stage: String },
    /// An effect owned by the member did not settle before teardown and its
    /// custody is retained.
    EffectCustodyRetained { context: &'static str },
}

impl MemberShutdownOutcome {
    /// Whether Shutdown left nothing outstanding for the member.
    pub fn is_clean(&self) -> bool {
        matches!(self, Self::Unregistered)
    }
}

/// Caller options for [`MobHandle::shutdown_with_report`](super::MobHandle::shutdown_with_report).
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct ShutdownOptions {
    /// The caller's own bound for the Shutdown's waits. When it elapses,
    /// every member still being waited on is reported and Shutdown returns.
    /// `None` uses the member lifecycle hang guard as the failure bound.
    pub deadline: Option<meerkat_core::time_compat::Instant>,
}

impl ShutdownOptions {
    /// Bound the Shutdown's waits by `deadline`.
    #[must_use]
    pub fn with_deadline(mut self, deadline: meerkat_core::time_compat::Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }
}

/// Per-member account of a completed mob Shutdown.
#[derive(Debug, Clone, Default)]
pub struct MobShutdownReport {
    pub members: BTreeMap<AgentIdentity, MemberShutdownOutcome>,
    /// What the Shutdown did to each autonomous member's run: a run current
    /// when it reached the member is cancelled immediately
    /// ([`MemberStopRun::CancelledByShutdown`](super::stop_report::MemberStopRun::CancelledByShutdown)).
    pub runs: BTreeMap<AgentIdentity, super::stop_report::MemberStopRun>,
    /// Whether each roster member's run starts were held by the Shutdown:
    /// `Held` (locally, or through a remote member's host when the Shutdown
    /// stopped that member), `NotHoldable`, or `DelegatedToHost` for a remote
    /// member the Shutdown did not contact.
    pub run_starts: BTreeMap<AgentIdentity, super::stop_report::MemberRunStarts>,
}

impl MobShutdownReport {
    /// Whether every member's outcome is clean.
    pub fn is_clean(&self) -> bool {
        self.members.values().all(MemberShutdownOutcome::is_clean)
    }

    pub(crate) fn record(&mut self, identity: AgentIdentity, outcome: MemberShutdownOutcome) {
        // An outstanding outcome is never overwritten by a later clean one.
        match self.members.get(&identity) {
            Some(existing) if !existing.is_clean() && outcome.is_clean() => {}
            _ => {
                self.members.insert(identity, outcome);
            }
        }
    }
}
