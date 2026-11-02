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
use std::sync::{Arc, Weak};

use meerkat_core::time_compat::{Duration, Instant};

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

/// How the mob's actor task ended, published once by the task's spawn
/// wrapper after the actor future returned or was dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ActorTaskEnd {
    #[default]
    Running,
    /// The actor future returned: the actor, and every store handle it
    /// owned, has dropped.
    Returned,
    /// The task ended without its future returning (a panic or an abort).
    Lost,
}

/// Member lifecycle observations written only by the mob actor (and its
/// task's spawn wrapper) and shared read-only with its handles: per-member
/// retirement settlements, the report of the most recent Shutdown, why the
/// actor fail-stopped, and how the actor task ended.
#[derive(Debug, Default)]
pub(crate) struct MemberLifecycleObservations {
    members: std::sync::Mutex<BTreeMap<AgentIdentity, SettlementChannel>>,
    last_shutdown_report: std::sync::Mutex<Option<MobShutdownReport>>,
    fail_stop_cause: std::sync::Mutex<Option<Arc<MobError>>>,
    actor_task: crate::tokio::sync::watch::Sender<ActorTaskEnd>,
    /// The members whose materialization changed in each published
    /// MobMachine state, for the subscriptions that attach at a member's
    /// wake.
    materialization: super::member_event_attach::MemberMaterializationFeed,
    /// Test probe: set once a Shutdown parks on its off-actor teardown.
    #[cfg(test)]
    pub(crate) shutdown_teardown_parked: crate::tokio::sync::watch::Sender<bool>,
    /// Test probe: set once a Shutdown passes its admission, before its
    /// lifecycle drains.
    #[cfg(test)]
    pub(crate) shutdown_admitted: crate::tokio::sync::watch::Sender<bool>,
    /// Test probe: how many Retire/Respawn/Reload controls the actor has
    /// parked in its deferred-command queue, per target member, counted when
    /// it parks them.
    #[cfg(test)]
    pub(crate) parked_member_controls:
        crate::tokio::sync::watch::Sender<BTreeMap<AgentIdentity, u64>>,
    /// Test probe: how many member-work starts a parked Shutdown refused.
    #[cfg(test)]
    pub(crate) parked_shutdown_member_work_refusals: crate::tokio::sync::watch::Sender<u64>,
    /// Test probe: how many SubmitWork commands the submit-work pump handed
    /// back to the actor loop for dispatch.
    #[cfg(test)]
    pub(crate) submit_work_pump_dispatches: crate::tokio::sync::watch::Sender<u64>,
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

    /// Record why the actor fail-stopped (its startup recovery failed, or a
    /// durable mutation's outcome stayed uncertain), before the actor closes
    /// its admission and drops the commands it has not answered. The first
    /// record stands.
    pub(crate) fn record_fail_stop_cause(&self, cause: Arc<MobError>) {
        self.fail_stop_cause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert(cause);
    }

    /// Why the actor fail-stopped, if it did.
    pub(crate) fn fail_stop_cause(&self) -> Option<Arc<MobError>> {
        self.fail_stop_cause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Record how the actor task ended. Written once, by its spawn wrapper.
    pub(crate) fn record_actor_task_end(&self, end: ActorTaskEnd) {
        self.actor_task.send_replace(end);
    }

    /// Observe how the actor task ends.
    pub(crate) fn subscribe_actor_task(&self) -> crate::tokio::sync::watch::Receiver<ActorTaskEnd> {
        self.actor_task.subscribe()
    }

    /// How the actor task has ended so far: `Running` until its spawn
    /// wrapper records the end.
    pub(crate) fn actor_task_end(&self) -> ActorTaskEnd {
        *self.actor_task.borrow()
    }

    /// The actor's feed of member materialization changes.
    pub(crate) fn materialization_feed(
        &self,
    ) -> &super::member_event_attach::MemberMaterializationFeed {
        &self.materialization
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
    /// names the unregister step. `settlement` follows the member's exact
    /// registration after Shutdown returned, also after the mob's actor
    /// exited, until a later owner takes the registration over.
    #[non_exhaustive]
    UnregisterPending {
        stage: String,
        settlement: MemberUnregisterSettlement,
    },
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

/// How often a [`MemberUnregisterSettlement`] whose mob runtime handle is
/// gone re-reads whether its registration has ended: nothing announces it.
const ENDED_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Where a member's runtime unregister that a Shutdown reported pending
/// stands ([`MemberUnregisterSettlement`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberUnregisterState {
    /// The member's registration is gone: its exact runtime unregister
    /// completed and no registration of the session is current, or the
    /// registration has ended for good.
    Settled,
    /// The unregister has not completed yet.
    Pending,
    /// The unregister failed. Nothing settles it later.
    Failed(String),
    /// A later owner took the registration over after Shutdown returned and
    /// before this settlement started its unregister: a resumed mob, or any
    /// caller that reused the registration or attached to it in place.
    /// Nothing was unregistered or discarded; the registration is that
    /// owner's to settle. Terminal.
    HandedOver,
}

/// The settlement of one member's runtime unregister that a mob Shutdown
/// reported as [`MemberShutdownOutcome::UnregisterPending`]. It follows the
/// member's exact registration after Shutdown returned, also after the mob's
/// actor exited.
///
/// It owns no teardown. It joins the runtime's unregister coordinator for
/// that exact registration. When Shutdown's deadline passed before that
/// unregister was admitted, it admits it through the same exact-registration
/// path Shutdown uses, which never forces and never reaches a registration
/// the session gains later. A durability-degraded registration first goes
/// through the runtime's reload-required discard, as in Shutdown's own
/// teardown, and the cold successor that discard returns as published is
/// the registration then unregistered and followed, never one read after
/// the discard returned.
///
/// The registration's witness alone cannot tell the member's registration
/// from a later owner's: a resumed mob, or any caller, can take a
/// registration over in place (reuse it, attach an executor to it, or
/// materialize an actor on it), keeping its witness. Every such adoption
/// gives the registration a new use, so after Shutdown returned this
/// settlement starts the discard or the unregister only while the
/// registration's use is still the one Shutdown handed back, and reports
/// [`MemberUnregisterState::HandedOver`] otherwise. When the discard it
/// joins was started by a later owner, it follows that owner's successor
/// too: it then either sees the owner's use and reports `HandedOver`, or
/// unregisters the successor before the owner adopts it, and the owner joins
/// that unregister and registers anew, as after a Shutdown that settled
/// before the resume. It holds the mob's runtime handle only weakly, so it
/// keeps nothing alive.
#[derive(Clone)]
pub struct MemberUnregisterSettlement {
    /// The exact registration followed: the member's own, or the cold
    /// successor a reload-required discard replaced it with.
    registration: meerkat_runtime::RuntimeSessionRegistrationWitness,
    /// The use `registration` had when Shutdown handed it back, or the use
    /// the successor a discard published started at. `None`: the
    /// registration was not current when Shutdown handed it back, so none of
    /// it can be current later.
    registration_use: Option<meerkat_runtime::RuntimeSessionRegistrationUse>,
    /// The coordinator's observer, once the unregister was admitted.
    observer: Option<meerkat_runtime::RuntimeSessionUnregisterObserver>,
    runtime: Weak<meerkat_runtime::MeerkatMachine>,
    /// The mob's session-operation bindings, so a reload-required discard
    /// retains the member's bound operation as Shutdown's does.
    #[cfg(feature = "runtime-adapter")]
    ops_adapter: Weak<super::ops_adapter::MobOpsAdapter>,
    /// What an unregister this settlement begins does with the session's
    /// queued inputs: the choice of the Shutdown that handed it back.
    queued_inputs: meerkat_runtime::UnregisterQueuedInputs,
    /// `Settled`, `Failed` or `HandedOver`, once reached.
    terminal: Option<MemberUnregisterState>,
}

impl MemberUnregisterSettlement {
    #[cfg(feature = "runtime-adapter")]
    pub(crate) fn new(
        registration: meerkat_runtime::RuntimeSessionRegistrationWitness,
        registration_use: Option<meerkat_runtime::RuntimeSessionRegistrationUse>,
        observer: Option<meerkat_runtime::RuntimeSessionUnregisterObserver>,
        runtime: &Arc<meerkat_runtime::MeerkatMachine>,
        ops_adapter: Option<&Arc<super::ops_adapter::MobOpsAdapter>>,
    ) -> Self {
        Self {
            registration,
            registration_use,
            observer,
            runtime: Arc::downgrade(runtime),
            ops_adapter: ops_adapter.map_or_else(Weak::new, Arc::downgrade),
            queued_inputs: meerkat_runtime::UnregisterQueuedInputs::Stop,
            terminal: None,
        }
    }

    /// Begin a later unregister with `queued_inputs`, the choice of the
    /// Shutdown that handed this settlement back. Without it the settlement
    /// stops the session's queued inputs.
    #[cfg(feature = "runtime-adapter")]
    #[must_use]
    pub(crate) fn with_queued_inputs(
        mut self,
        queued_inputs: meerkat_runtime::UnregisterQueuedInputs,
    ) -> Self {
        self.queued_inputs = queued_inputs;
        self
    }

    /// The state as known now, without waiting: the terminal state
    /// [`Self::settle_within`] reached, `Failed` once the coordinator
    /// published a failure, `Settled` once the registration has provably
    /// ended, `Pending` otherwise. A completed unregister reads `Settled`
    /// here only when the registration's own end proves it;
    /// `settle_within` verifies it through the runtime.
    pub fn try_state(&self) -> MemberUnregisterState {
        if let Some(terminal) = &self.terminal {
            return terminal.clone();
        }
        if let Some(observer) = &self.observer {
            match observer.try_result() {
                // Refused until a durability repair: `settle_within` runs the
                // discard and admits the unregister again.
                Ok(Some(Err(error))) if unregister_refused_until_repair(&error) => {}
                Ok(Some(Err(error))) | Err(error) => return self.failed(&error),
                Ok(Some(Ok(())) | None) => {}
            }
        }
        self.ended_or_pending()
    }

    /// Wait until `deadline` for the unregister to settle: join the exact
    /// registration's coordinator (admitting its unregister first when none
    /// was admitted yet), then prove the registration gone the way Shutdown's
    /// own teardown does. Dropping the future cancels only this wait, never
    /// the unregister. `Pending` at the deadline; a later call continues. A
    /// terminal state is kept.
    pub async fn settle_within(&mut self, deadline: Instant) -> MemberUnregisterState {
        if let Some(terminal) = &self.terminal {
            return terminal.clone();
        }
        let state = self.settle(deadline).await;
        if state != MemberUnregisterState::Pending {
            self.terminal = Some(state.clone());
        }
        state
    }

    async fn settle(&mut self, deadline: Instant) -> MemberUnregisterState {
        let remaining = || deadline.saturating_duration_since(Instant::now());
        // A refusal until a durability repair, from the admission or from the
        // coordinator, is followed by the discard and a fresh admission once
        // per call; a second one waits for a later call.
        let mut repair_retried = false;
        loop {
            if let Some(observer) = self.observer.as_mut() {
                match crate::tokio::time::timeout(remaining(), observer.wait_for_result()).await {
                    Err(_) => return MemberUnregisterState::Pending,
                    Ok(Ok(())) => return self.verify_ended(deadline).await,
                    // The coordinator ended without unregistering and released
                    // the registration for the discard.
                    Ok(Err(error)) if unregister_refused_until_repair(&error) => {
                        self.observer = None;
                    }
                    Ok(Err(error)) => return self.failed(&error),
                }
            } else {
                // Shutdown's deadline passed before the unregister was
                // admitted. Admit it now, or join the coordinator already
                // running for exactly this registration, while the
                // registration's use is still the one Shutdown handed back.
                let Some(runtime) = self.runtime.upgrade() else {
                    return self.ended_within(deadline).await;
                };
                let Some(expected_use) = self.registration_use else {
                    // Not current when Shutdown handed it back: nothing of
                    // this registration can be current now.
                    return self.verify_ended(deadline).await;
                };
                #[cfg(feature = "runtime-adapter")]
                let expected_use = match self
                    .discard_degraded_first(&runtime, expected_use, deadline)
                    .await
                {
                    std::ops::ControlFlow::Continue(expected_use) => expected_use,
                    std::ops::ControlFlow::Break(state) => return state,
                };
                let admitted = crate::tokio::time::timeout(
                    remaining(),
                    runtime
                        .observe_unregister_session_registration_with_queued_inputs_if_use_unchanged(
                            &self.registration,
                            expected_use,
                            self.queued_inputs,
                        ),
                )
                .await;
                match admitted {
                    Err(_) => return MemberUnregisterState::Pending,
                    Ok(Ok(meerkat_runtime::RuntimeSessionUnregisterUseAdmission::Changed)) => {
                        return MemberUnregisterState::HandedOver;
                    }
                    Ok(Ok(meerkat_runtime::RuntimeSessionUnregisterUseAdmission::Admitted(
                        meerkat_runtime::RuntimeSessionUnregisterAdmission::Completed
                        | meerkat_runtime::RuntimeSessionUnregisterAdmission::NotCurrent,
                    ))) => return self.verify_ended(deadline).await,
                    Ok(Ok(meerkat_runtime::RuntimeSessionUnregisterUseAdmission::Admitted(
                        meerkat_runtime::RuntimeSessionUnregisterAdmission::Pending(observer),
                    ))) => {
                        self.observer = Some(observer);
                        continue;
                    }
                    Ok(Ok(admission)) => {
                        return MemberUnregisterState::Failed(format!(
                            "runtime unregister admission of session {} answered {admission:?}",
                            self.registration.session_id()
                        ));
                    }
                    Ok(Err(error)) if unregister_refused_until_repair(&error) => {}
                    Ok(Err(error)) => return self.failed(&error),
                }
            }
            if repair_retried {
                return MemberUnregisterState::Pending;
            }
            repair_retried = true;
        }
    }

    /// Before its exact unregister is admitted, a durability-degraded
    /// registration goes through the runtime's reload-required discard: its
    /// exited runtime loop can hold the session's mutation fence, which only
    /// that discard releases. The discard runs only while the registration's
    /// use is still `expected_use`. After a discard, the cold successor it
    /// returns as published is the registration followed, from the use it
    /// started at. `Continue` carries the use the unregister is then
    /// admitted with; `Break` ends this call with that state.
    #[cfg(feature = "runtime-adapter")]
    async fn discard_degraded_first(
        &mut self,
        runtime: &Arc<meerkat_runtime::MeerkatMachine>,
        expected_use: meerkat_runtime::RuntimeSessionRegistrationUse,
        deadline: Instant,
    ) -> std::ops::ControlFlow<MemberUnregisterState, meerkat_runtime::RuntimeSessionRegistrationUse>
    {
        use super::provisioner::{DegradedUnregisterStep, MemberSessionDisposalArc};
        use std::ops::ControlFlow;

        let ops_adapter = self.ops_adapter.upgrade();
        match MemberSessionDisposalArc::discard_degraded_registration_first(
            runtime,
            ops_adapter.as_ref(),
            &self.registration,
            Some(expected_use),
            deadline,
        )
        .await
        {
            DegradedUnregisterStep::Unchanged => ControlFlow::Continue(expected_use),
            DegradedUnregisterStep::Replaced {
                successor,
                successor_use,
            } => {
                self.registration = successor;
                self.registration_use = Some(successor_use);
                ControlFlow::Continue(successor_use)
            }
            DegradedUnregisterStep::HandedOver => {
                ControlFlow::Break(MemberUnregisterState::HandedOver)
            }
            DegradedUnregisterStep::TimedOut => ControlFlow::Break(MemberUnregisterState::Pending),
            DegradedUnregisterStep::Failed(failure) => {
                ControlFlow::Break(MemberUnregisterState::Failed(failure))
            }
        }
    }

    /// The exact unregister completed, or its registration is no longer
    /// current: prove the registration gone, as Shutdown's own teardown
    /// does. The registration still current after its coordinator completed
    /// is a failure. A later registration of the session is not this
    /// settlement's: it settles once the exact one has provably ended.
    async fn verify_ended(&self, deadline: Instant) -> MemberUnregisterState {
        let Some(runtime) = self.runtime.upgrade() else {
            return self.ended_within(deadline).await;
        };
        let session_id = self.registration.session_id();
        match runtime
            .current_session_registration_witness(session_id)
            .await
        {
            None => MemberUnregisterState::Settled,
            Some(current) if current == self.registration => {
                MemberUnregisterState::Failed(format!(
                    "runtime unregister coordinator for session {session_id} completed while its exact registration remained current"
                ))
            }
            Some(_) => self.ended_within(deadline).await,
        }
    }

    /// Without the mob's runtime handle, only the registration's own end
    /// proves the unregister settled. Dropping that handle ends nothing: the
    /// runtime lives on in every other handle, its unregister coordinator
    /// among them (the coordinator runs on a clone, so dropping a caller's
    /// handle never cancels a teardown), and the registration can still be
    /// current there. So a dropped handle settles only once the registration
    /// has ended for good, which no current entry anywhere allows; until
    /// then the unregister stays pending.
    fn ended_or_pending(&self) -> MemberUnregisterState {
        if self.registration.has_ended() {
            MemberUnregisterState::Settled
        } else {
            MemberUnregisterState::Pending
        }
    }

    /// [`Self::ended_or_pending`], watched until `deadline`: nothing
    /// announces the end, so it is polled.
    async fn ended_within(&self, deadline: Instant) -> MemberUnregisterState {
        loop {
            let state = self.ended_or_pending();
            let remaining = deadline.saturating_duration_since(Instant::now());
            if state != MemberUnregisterState::Pending || remaining.is_zero() {
                return state;
            }
            crate::tokio::time::sleep(remaining.min(ENDED_POLL_INTERVAL)).await;
        }
    }

    fn failed(&self, error: &impl fmt::Display) -> MemberUnregisterState {
        MemberUnregisterState::Failed(format!(
            "runtime unregister of session {} failed: {error}",
            self.registration.session_id()
        ))
    }
}

/// Whether the runtime refused a member's unregister only until its
/// registration's durability is repaired: the reload-required discard
/// releases it, so the unregister is pending, not failed.
pub(crate) fn unregister_refused_until_repair(error: &meerkat_runtime::RuntimeDriverError) -> bool {
    matches!(
        error,
        meerkat_runtime::RuntimeDriverError::RecoveryRepairBlocked { .. }
    )
}

impl fmt::Debug for MemberUnregisterSettlement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemberUnregisterSettlement")
            .field("session_id", self.registration.session_id())
            .field("coordinator_admitted", &self.observer.is_some())
            .field("terminal", &self.terminal)
            .finish_non_exhaustive()
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
    /// What the Shutdown does with a member's running turn and the inputs
    /// still queued behind it. The default is
    /// [`QueuedInputsOnShutdown::RequeueDurable`].
    pub queued_inputs: QueuedInputsOnShutdown,
}

impl ShutdownOptions {
    /// Bound the Shutdown's waits by `deadline`.
    #[must_use]
    pub fn with_deadline(mut self, deadline: meerkat_core::time_compat::Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Choose what the Shutdown does with a member's running turn and the
    /// inputs still queued behind it, for every member alike, instead of the
    /// per-member default ([`QueuedInputsOnShutdown::RequeueDurable`]).
    /// [`QueuedInputsOnShutdown::Requeue`] keeps only the eligible queued
    /// inputs: an input bound to an objective, such as a member's kickoff,
    /// still ends as stopped. [`QueuedInputsOnShutdown::Stop`] is the
    /// behavior before 0.9.5. [`QueuedInputsOnShutdown::Keep`] keeps the
    /// inputs it finds as they are, but those of a run whose stop was asked,
    /// for a host that served its members nothing.
    #[must_use]
    pub fn with_queued_inputs(mut self, queued_inputs: QueuedInputsOnShutdown) -> Self {
        self.queued_inputs = queued_inputs;
        self
    }
}

/// What a graceful mob Shutdown does with a member's running turn and the
/// inputs still queued behind it.
///
/// Only a whole-mob Shutdown takes this choice: a member's retirement,
/// archive or destroy always ends its queued inputs as stopped, since that
/// session is not expected to resume. Shutdown applies it to the members it
/// unregisters and keeps; a member already retiring when Shutdown reaches it
/// is stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum QueuedInputsOnShutdown {
    /// The default since 0.9.5: [`Self::Requeue`] with
    /// [`RequeueMaxAge::DEFAULT`] for a member whose session commits each
    /// round (the mob's session service keeps persistent sessions, whose runs
    /// commit each round, and its runtime has a persistence store), and
    /// [`Self::Stop`] for any other member. A deploy that shuts the mob down
    /// gracefully therefore keeps durable members' work for the next start;
    /// pass [`Self::Stop`] to end it as before 0.9.5.
    #[default]
    RequeueDurable,
    /// End them as stopped: the runtime stop cancels the running turn, and
    /// every queued input ends as stopped. The default before 0.9.5.
    Stop,
    /// Keep a durable member's work for the mob's next start: its running
    /// turn and its eligible queued inputs, unless they are older than the
    /// bound by then.
    ///
    /// The Shutdown holds the member's run starts and does not cancel its
    /// running turn itself: it unregisters the member's runtime with this
    /// choice, and the runtime's stop suspends a run that commits each round
    /// at its last committed round, as a crash would leave it. The next
    /// start resumes that run, redoing the work since that commit. The
    /// waiters of the run's inputs are answered
    /// [`RequeuedAcrossStop`](super::BoundedTurnFailure::RequeuedAcrossStop).
    /// An autonomous member whose session commits each round keeps its
    /// kickoff open too: the next start resumes its run under the same
    /// objective, and the kickoff resolves when that run ends. Any other
    /// autonomous member's kickoff is closed and its run cancelled at once,
    /// as under [`Self::Stop`]. The runtime stop cancels a run that commits
    /// no rounds (a live bridge run, or any run of a session without a
    /// checkpointer).
    ///
    /// Eligible queued inputs: a durable prompt in the queue lane that is
    /// still queued when the member's runtime stops, is not a directed
    /// interaction and is not bound to an objective. A member's kickoff is
    /// bound to its objective, so it is never kept: it ends as stopped and
    /// its kickoff is recorded failed, as under [`Self::Stop`]. Every other
    /// input still ends as stopped. The waiter of a kept input
    /// is answered [`RequeuedAcrossStop`](super::BoundedTurnFailure::RequeuedAcrossStop):
    /// do not resubmit it; a same-key replay after the restart re-attaches
    /// to it, and one older than the bound by then ends
    /// [`RequeueExpired`](super::BoundedTurnFailure::RequeueExpired).
    ///
    /// This is a guarantee only for members whose run starts the Shutdown
    /// holds ([`MemberRunStarts::Held`](super::stop_report::MemberRunStarts::Held)).
    /// A member that cannot hold them may start its queued input once its
    /// running turn is cancelled, before its runtime stops; that input then
    /// belongs to the cancelled run and is not kept.
    Requeue(RequeueMaxAge),
    /// Keep the members' work the Shutdown finds as it is, for the next
    /// start: the runtime unregister's
    /// [`UnregisterQueuedInputs::Keep`](meerkat_runtime::UnregisterQueuedInputs::Keep).
    /// For a host that boots over a store another host serves and must hand
    /// it back unchanged, such as a candidate launch that holds every
    /// member's run starts and admits nothing to them.
    ///
    /// For a member whose runtime registration served no live work (no input
    /// was admitted to it, no resubmission was deduplicated and its
    /// registration was not recycled), every input the registration
    /// recovered stays as the Shutdown finds it, with no requeue mark and no
    /// expiry, and a run its session left open stays open: the next start
    /// recovers them again and resumes that run. (The boot's ordinary
    /// recovery, which every start runs, may already have normalized a
    /// staged input whose run cannot continue to queued; the Keep leaves
    /// that as it is.) An
    /// autonomous member's kickoff stays open too, as under [`Self::Requeue`].
    /// The exception is a run the registration marked resumable whose stop
    /// was asked before the Shutdown (a stop of the member's run needs no
    /// executor): the inputs bound to it end as stopped, and the next start
    /// does not resume it. A member whose registration served live work is
    /// stopped instead, as under [`Self::Stop`]: the keep is exact only for
    /// work this process never touched.
    Keep,
}

impl QueuedInputsOnShutdown {
    /// The runtime unregister's choice for one member session.
    /// `commits_per_round` says whether that session commits each round
    /// durably, which decides [`Self::RequeueDurable`]; an explicit choice
    /// applies as given.
    #[cfg(feature = "runtime-adapter")]
    pub(crate) fn unregister_queued_inputs(
        self,
        commits_per_round: bool,
    ) -> meerkat_runtime::UnregisterQueuedInputs {
        match self {
            Self::RequeueDurable if commits_per_round => {
                meerkat_runtime::UnregisterQueuedInputs::Requeue {
                    max_age: RequeueMaxAge::DEFAULT,
                }
            }
            Self::RequeueDurable | Self::Stop => meerkat_runtime::UnregisterQueuedInputs::Stop,
            Self::Requeue(max_age) => meerkat_runtime::UnregisterQueuedInputs::Requeue {
                max_age: max_age.get(),
            },
            Self::Keep => meerkat_runtime::UnregisterQueuedInputs::Keep,
        }
    }
}

/// The age bound of a requeued input, measured from its admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequeueMaxAge(Duration);

impl RequeueMaxAge {
    /// The longest accepted bound, the runtime's
    /// [`UnregisterQueuedInputs::MAX_AGE_CAP`](meerkat_runtime::UnregisterQueuedInputs::MAX_AGE_CAP).
    pub const CAP: Duration = meerkat_runtime::UnregisterQueuedInputs::MAX_AGE_CAP;
    /// The bound a requeue uses when the caller names none, the runtime's
    /// [`UnregisterQueuedInputs::DEFAULT_MAX_AGE`](meerkat_runtime::UnregisterQueuedInputs::DEFAULT_MAX_AGE).
    pub const DEFAULT: Duration = meerkat_runtime::UnregisterQueuedInputs::DEFAULT_MAX_AGE;

    /// A bound of `max_age`, checked by the runtime's own rule
    /// ([`UnregisterQueuedInputs::check`](meerkat_runtime::UnregisterQueuedInputs::check)).
    ///
    /// # Errors
    /// Refuses a bound under one millisecond (zero included), the resolution
    /// the stop mark records, and a bound over [`Self::CAP`]. A bound is
    /// never clamped.
    pub fn new(max_age: Duration) -> Result<Self, ShutdownOptionsError> {
        match (meerkat_runtime::UnregisterQueuedInputs::Requeue { max_age }).check() {
            Ok(()) => Ok(Self(max_age)),
            Err(meerkat_runtime::UnregisterQueuedInputsError::MaxAgeOverCap { max_age, cap }) => {
                Err(ShutdownOptionsError::RequeueMaxAgeOverCap { max_age, cap })
            }
            Err(_) => Err(ShutdownOptionsError::RequeueMaxAgeUnderOneMillisecond { max_age }),
        }
    }

    /// The bound.
    #[must_use]
    pub fn get(self) -> Duration {
        self.0
    }
}

impl Default for RequeueMaxAge {
    fn default() -> Self {
        Self(Self::DEFAULT)
    }
}

/// A refused [`ShutdownOptions`] value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ShutdownOptionsError {
    /// A requeue bound under one millisecond, zero included.
    #[error("the requeue max age {max_age:?} is under one millisecond")]
    RequeueMaxAgeUnderOneMillisecond { max_age: Duration },
    /// A requeue bound over [`RequeueMaxAge::CAP`].
    #[error("the requeue max age {max_age:?} is over the cap of {cap:?}")]
    RequeueMaxAgeOverCap { max_age: Duration, cap: Duration },
}

/// Per-member account of a completed mob Shutdown.
#[derive(Debug, Clone, Default)]
pub struct MobShutdownReport {
    pub members: BTreeMap<AgentIdentity, MemberShutdownOutcome>,
    /// What the Shutdown did to each autonomous member's run: a run current
    /// when it reached the member is cancelled immediately
    /// ([`MemberStopRun::CancelledByShutdown`](super::stop_report::MemberStopRun::CancelledByShutdown)),
    /// and a member whose registration is durability-degraded is reported
    /// [`MemberStopRun::ReloadRequired`](super::stop_report::MemberStopRun::ReloadRequired).
    pub runs: BTreeMap<AgentIdentity, super::stop_report::MemberStopRun>,
    /// Whether each roster member's run starts were held by the Shutdown:
    /// `Held` (locally, or through a remote member's host when the Shutdown
    /// stopped that member), `NotHoldable`, `DelegatedToHost` for a remote
    /// member the Shutdown did not contact, or `ReloadRequired` for a local
    /// member whose durability-degraded registration refused the hold (its
    /// teardown discards it).
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
