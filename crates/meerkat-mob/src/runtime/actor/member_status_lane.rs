//! Actor-owned member-status observation lanes.
//!
//! A member status read observes one member's session off the actor loop and
//! brings the observation back to the actor, which alone applies it to
//! MobMachine and builds the snapshot. This module owns how those reads are
//! admitted:
//!
//! - **One in-flight observation per member.** The actor keeps a map from
//!   member identity to the observation in flight for it. A second request for
//!   the same member joins that observation as another waiter and receives the
//!   same result, so two observations of one member never overlap and a stale
//!   absence can never land after a later success for the same member.
//!   Observations of different members run concurrently; MobMachine already
//!   orders observations across members through `member_last_observed_at_ms`.
//! - **A mob-wide capacity with a bounded wait.** At most
//!   [`MAX_CONCURRENT_MEMBER_STATUS_OBSERVATIONS`] observations read session
//!   state at once. The capacity is waited for inside the spawned observation
//!   task for at most [`MEMBER_STATUS_OBSERVATION_ADMISSION_TIMEOUT`], raced
//!   against every waiter going away. Only that elapsed wait refuses a read, as
//!   `LifecycleOperationAdmissionPending`, so its `deadline_reached` is true.
//! - **Waiters outlive target drift, not the actor.** When the member's target
//!   changed while its observation was in flight the actor re-drives a fresh
//!   observation for the same waiters. When the actor exits, the in-flight map
//!   and every observation task are dropped, so each waiter's reply channel
//!   closes.
//!
//! This is actor-owned shell state for read admission. It holds no MobMachine
//! fact; the machine stays the only authority over what an observation means.

use super::*;
use std::future::Future;
use std::task::Poll;

/// How many member-status observations may read session state at once across
/// one mob. Sized for the fan-out of polling surfaces (console progress
/// sweeps, operator inspection, fork re-link watchers) over a mob of a few
/// dozen members.
#[cfg(not(test))]
pub(in crate::runtime) const MAX_CONCURRENT_MEMBER_STATUS_OBSERVATIONS: usize = 16;
#[cfg(test)]
pub(in crate::runtime) const MAX_CONCURRENT_MEMBER_STATUS_OBSERVATIONS: usize = 2;

/// How long an observation waits for the mob-wide capacity before its waiters
/// are told to retry.
pub(in crate::runtime) const MEMBER_STATUS_OBSERVATION_ADMISSION_TIMEOUT: Duration =
    Duration::from_secs(2);

/// Upper bound on one observation's session reads. A read still running at
/// this deadline yields a degraded observation carrying a typed
/// preview-unavailable marker instead of holding the lane.
pub(in crate::runtime) const MEMBER_STATUS_OBSERVATION_DEADLINE: Duration = Duration::from_secs(1);

/// Reply channel of one `member_status` caller.
pub(in crate::runtime) type MemberStatusReply =
    oneshot::Sender<Result<super::super::MobMemberSnapshot, MobError>>;

enum WaiterSet {
    Open(Vec<MemberStatusReply>),
    /// Settled, or abandoned because every caller went away. A closed set
    /// admits no joiner: the actor starts a fresh observation instead.
    Closed,
}

/// The callers waiting on one member's in-flight observation.
///
/// Shared between the actor (which joins callers and settles the set) and the
/// observation task (which gives up when every caller has gone). The mutex is
/// never held across an await.
pub(in crate::runtime) struct MemberStatusObservationWaiters(std::sync::Mutex<WaiterSet>);

impl MemberStatusObservationWaiters {
    pub(in crate::runtime) fn new(first: MemberStatusReply) -> Arc<Self> {
        Arc::new(Self(std::sync::Mutex::new(WaiterSet::Open(vec![first]))))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WaiterSet> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Join the in-flight observation. A closed set hands the reply back so
    /// the caller can start a fresh observation.
    pub(in crate::runtime) fn join(
        &self,
        reply_tx: MemberStatusReply,
    ) -> Result<(), MemberStatusReply> {
        match &mut *self.lock() {
            WaiterSet::Open(waiters) => {
                waiters.push(reply_tx);
                Ok(())
            }
            WaiterSet::Closed => Err(reply_tx),
        }
    }

    /// Resolves once no caller is waiting any more, closing the set so no
    /// later caller can join an observation that is being abandoned.
    pub(in crate::runtime) fn abandoned(&self) -> impl Future<Output = ()> + '_ {
        std::future::poll_fn(move |cx| {
            let mut set = self.lock();
            match &mut *set {
                WaiterSet::Closed => Poll::Ready(()),
                WaiterSet::Open(waiters) => {
                    waiters.retain_mut(|reply_tx| reply_tx.poll_closed(cx).is_pending());
                    if waiters.is_empty() {
                        *set = WaiterSet::Closed;
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                }
            }
        })
    }

    /// Close the set when every caller has already gone.
    pub(in crate::runtime) fn close_if_abandoned(&self) -> bool {
        let mut set = self.lock();
        match &mut *set {
            WaiterSet::Closed => true,
            WaiterSet::Open(waiters) => {
                waiters.retain(|reply_tx| !reply_tx.is_closed());
                if waiters.is_empty() {
                    *set = WaiterSet::Closed;
                    true
                } else {
                    false
                }
            }
        }
    }

    fn take_open(&self) -> Vec<MemberStatusReply> {
        match std::mem::replace(&mut *self.lock(), WaiterSet::Closed) {
            WaiterSet::Open(waiters) => waiters
                .into_iter()
                .filter(|reply_tx| !reply_tx.is_closed())
                .collect(),
            WaiterSet::Closed => Vec::new(),
        }
    }

    /// Deliver one result to every waiting caller and close the set.
    ///
    /// Snapshots are cloned per caller. An error is owned by a single caller;
    /// joined callers share it through the transparent
    /// [`MobError::SharedLifecycleFailure`] carrier, which forwards the typed
    /// cause, wire code, and structured data.
    pub(in crate::runtime) fn settle(
        &self,
        result: Result<super::super::MobMemberSnapshot, MobError>,
    ) {
        let waiters = self.take_open();
        match result {
            Ok(snapshot) => {
                for reply_tx in waiters {
                    let _ = reply_tx.send(Ok(snapshot.clone()));
                }
            }
            Err(error) if waiters.len() <= 1 => {
                if let Some(reply_tx) = waiters.into_iter().next() {
                    let _ = reply_tx.send(Err(error));
                }
            }
            Err(error) => {
                let shared = Arc::new(error);
                for reply_tx in waiters {
                    let _ =
                        reply_tx.send(Err(MobError::SharedLifecycleFailure(Arc::clone(&shared))));
                }
            }
        }
    }

    /// Deliver a freshly constructed error to every waiting caller and close
    /// the set.
    pub(in crate::runtime) fn settle_each_with(&self, error: impl Fn() -> MobError) {
        for reply_tx in self.take_open() {
            let _ = reply_tx.send(Err(error()));
        }
    }
}

/// What an observation task brings back to the actor.
pub(in crate::runtime) enum MemberStatusObservationOutcome {
    /// The member's session was observed (possibly degraded at the
    /// observation deadline, with a typed marker).
    Observed(Box<super::super::state::MemberStatusSessionObservation>),
    /// The mob-wide capacity stayed full for the whole admission wait.
    AdmissionDeadline,
    /// The capacity refused admission for a reason other than time.
    AdmissionRefused,
}

/// Result of waiting for the mob-wide observation capacity.
pub(in crate::runtime) enum MemberStatusObservationAdmission {
    Admitted(tokio::sync::OwnedSemaphorePermit),
    /// The wait elapsed with the capacity still full.
    Deadline,
    /// The capacity semaphore was closed. The actor never closes it.
    Refused,
    /// Every waiter went away while the observation waited.
    Abandoned,
}

/// Wait at most `timeout` for one unit of the mob-wide observation capacity,
/// giving up as soon as every waiter has gone.
pub(in crate::runtime) async fn admit_member_status_observation(
    capacity: Arc<tokio::sync::Semaphore>,
    waiters: &MemberStatusObservationWaiters,
    timeout: Duration,
) -> MemberStatusObservationAdmission {
    tokio::select! {
        biased;
        () = waiters.abandoned() => MemberStatusObservationAdmission::Abandoned,
        admitted = tokio::time::timeout(timeout, capacity.acquire_owned()) => match admitted {
            Ok(Ok(permit)) => MemberStatusObservationAdmission::Admitted(permit),
            Ok(Err(_closed)) => MemberStatusObservationAdmission::Refused,
            Err(_elapsed) => MemberStatusObservationAdmission::Deadline,
        },
    }
}

/// The refusal a caller receives when the mob-wide capacity stayed full for
/// the whole admission wait. Retryable; `deadline_reached` is true because a
/// bounded wait elapsed.
pub(in crate::runtime) fn member_status_admission_deadline_error() -> MobError {
    MobError::LifecycleOperationAdmissionPending {
        intent: "member_status_observation".to_string(),
        stage: "observation_lane_saturated",
    }
}

impl MobActor {
    /// Admit one `member_status` caller: join the member's in-flight
    /// observation, or start one.
    pub(super) fn project_member_status(
        &mut self,
        agent_identity: AgentIdentity,
        reply_tx: MemberStatusReply,
    ) {
        if reply_tx.is_closed() {
            return;
        }
        let reply_tx = match self.member_status_observations.get(&agent_identity) {
            Some(in_flight) => match in_flight.join(reply_tx) {
                Ok(()) => return,
                Err(reply_tx) => reply_tx,
            },
            None => reply_tx,
        };
        let waiters = MemberStatusObservationWaiters::new(reply_tx);
        self.member_status_observations
            .insert(agent_identity.clone(), Arc::clone(&waiters));
        self.drive_member_status_observation(agent_identity, waiters);
    }

    /// Start one observation of `agent_identity`'s current target for
    /// `waiters`.
    fn drive_member_status_observation(
        &mut self,
        agent_identity: AgentIdentity,
        waiters: Arc<MemberStatusObservationWaiters>,
    ) {
        let expected_target = match self.member_status_projection_target(&agent_identity) {
            Ok(target) => target,
            Err(error) => {
                self.settle_member_status_observation(&agent_identity, &waiters, Err(error));
                return;
            }
        };
        let observed_at_ms = match self.issue_member_status_observed_at_ms() {
            Ok(observed_at_ms) => observed_at_ms,
            Err(error) => {
                self.settle_member_status_observation(&agent_identity, &waiters, Err(error));
                return;
            }
        };
        let session_service = Arc::clone(&self.session_service);
        #[cfg(feature = "runtime-adapter")]
        let runtime_adapter = self.runtime_adapter.clone();
        #[cfg(not(feature = "runtime-adapter"))]
        let runtime_adapter = None;
        let capacity = Arc::clone(&self.member_status_observation_capacity);
        let command_tx = self.command_tx.clone();
        self.actor_io_tasks.spawn(async move {
            let outcome = match admit_member_status_observation(
                capacity,
                &waiters,
                MEMBER_STATUS_OBSERVATION_ADMISSION_TIMEOUT,
            )
            .await
            {
                MemberStatusObservationAdmission::Abandoned => return,
                MemberStatusObservationAdmission::Deadline => {
                    MemberStatusObservationOutcome::AdmissionDeadline
                }
                MemberStatusObservationAdmission::Refused => {
                    MemberStatusObservationOutcome::AdmissionRefused
                }
                MemberStatusObservationAdmission::Admitted(permit) => {
                    let observation = tokio::select! {
                        biased;
                        () = waiters.abandoned() => return,
                        observation = Self::observe_member_status_session(
                            session_service,
                            runtime_adapter,
                            agent_identity.clone(),
                            expected_target.bridge_session_id.clone(),
                            expected_target.include_local_session_details,
                            observed_at_ms,
                        ) => observation,
                    };
                    // The capacity bounds concurrent session reads only; the
                    // actor-side completion does not hold it.
                    drop(permit);
                    MemberStatusObservationOutcome::Observed(Box::new(observation))
                }
            };
            enqueue_member_status_observation(
                command_tx,
                agent_identity,
                expected_target,
                outcome,
                waiters,
            )
            .await;
        });
    }

    /// Apply one observation brought back by its task: materialize and settle
    /// it, or re-drive it for the same waiters when the member's target moved
    /// while it was in flight.
    pub(super) async fn complete_member_status_observation(
        &mut self,
        agent_identity: AgentIdentity,
        expected_target: super::super::state::MemberStatusProjectionTarget,
        outcome: MemberStatusObservationOutcome,
        waiters: Arc<MemberStatusObservationWaiters>,
    ) {
        if waiters.close_if_abandoned() {
            self.forget_member_status_observation(&agent_identity, &waiters);
            return;
        }
        let observation = match outcome {
            MemberStatusObservationOutcome::Observed(observation) => *observation,
            MemberStatusObservationOutcome::AdmissionDeadline => {
                self.forget_member_status_observation(&agent_identity, &waiters);
                waiters.settle_each_with(member_status_admission_deadline_error);
                return;
            }
            MemberStatusObservationOutcome::AdmissionRefused => {
                self.settle_member_status_observation(
                    &agent_identity,
                    &waiters,
                    Err(MobError::Internal(
                        "member-status observation capacity was closed".to_string(),
                    )),
                );
                return;
            }
        };
        match self.member_status_projection_target(&agent_identity) {
            Ok(current_target) if current_target == expected_target => {
                let preview_unavailable = observation.preview_unavailable;
                let result = self
                    .machine_member_material_from_observation(
                        &agent_identity,
                        current_target.bridge_session_id,
                        current_target.include_local_session_details,
                        observation,
                    )
                    .await
                    .map(|material| {
                        material
                            .to_snapshot()
                            .with_preview_unavailable(preview_unavailable)
                    });
                self.settle_member_status_observation(&agent_identity, &waiters, result);
            }
            Ok(_) => self.drive_member_status_observation(agent_identity, waiters),
            Err(error) => {
                self.settle_member_status_observation(&agent_identity, &waiters, Err(error));
            }
        }
    }

    fn settle_member_status_observation(
        &mut self,
        agent_identity: &AgentIdentity,
        waiters: &Arc<MemberStatusObservationWaiters>,
        result: Result<super::super::MobMemberSnapshot, MobError>,
    ) {
        self.forget_member_status_observation(agent_identity, waiters);
        waiters.settle(result);
    }

    /// Remove `waiters` from the in-flight map when it is still the entry for
    /// `agent_identity`; a replacement observation is left in place.
    fn forget_member_status_observation(
        &mut self,
        agent_identity: &AgentIdentity,
        waiters: &Arc<MemberStatusObservationWaiters>,
    ) {
        if self
            .member_status_observations
            .get(agent_identity)
            .is_some_and(|in_flight| Arc::ptr_eq(in_flight, waiters))
        {
            self.member_status_observations.remove(agent_identity);
        }
    }
}

/// Hand an observation back to the actor, giving up when every waiter has
/// gone while the mailbox was full.
pub(in crate::runtime) async fn enqueue_member_status_observation(
    command_tx: mpsc::Sender<RoutedMobCommand>,
    agent_identity: AgentIdentity,
    expected_target: super::super::state::MemberStatusProjectionTarget,
    outcome: MemberStatusObservationOutcome,
    waiters: Arc<MemberStatusObservationWaiters>,
) -> bool {
    let permit = tokio::select! {
        biased;
        () = waiters.abandoned() => return false,
        permit = command_tx.reserve() => permit,
    };
    let Ok(permit) = permit else {
        return false;
    };
    permit.send(RoutedMobCommand::internal(
        MobCommand::ProjectMemberStatusObserved {
            agent_identity,
            expected_target,
            outcome: Box::new(outcome),
            waiters,
        },
    ));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply_channel() -> (
        MemberStatusReply,
        oneshot::Receiver<Result<super::super::super::MobMemberSnapshot, MobError>>,
    ) {
        oneshot::channel()
    }

    /// A caller over the mob-wide capacity waits for it and is refused only
    /// once the bounded wait has elapsed, so the refusal's
    /// `deadline_reached` is true by construction.
    #[tokio::test(start_paused = true)]
    async fn admission_waits_for_capacity_and_refuses_only_after_the_deadline() {
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let held = Arc::clone(&capacity)
            .try_acquire_owned()
            .expect("hold the only permit");
        let (reply_tx, _reply_rx) = reply_channel();
        let waiters = MemberStatusObservationWaiters::new(reply_tx);
        let admission = admit_member_status_observation(
            Arc::clone(&capacity),
            &waiters,
            MEMBER_STATUS_OBSERVATION_ADMISSION_TIMEOUT,
        );
        tokio::pin!(admission);
        assert!(
            tokio::time::timeout(
                MEMBER_STATUS_OBSERVATION_ADMISSION_TIMEOUT
                    .saturating_sub(Duration::from_millis(1)),
                &mut admission,
            )
            .await
            .is_err(),
            "a full capacity is waited for, never refused before the deadline"
        );
        assert!(matches!(
            admission.await,
            MemberStatusObservationAdmission::Deadline
        ));
        let error = member_status_admission_deadline_error();
        let data = error.structured_data().expect("typed refusal data");
        assert_eq!(data["deadline_reached"], true);
        assert_eq!(data["retryable"], true);

        // A released permit admits a waiting observation.
        let (reply_tx, _reply_rx) = reply_channel();
        let waiters = MemberStatusObservationWaiters::new(reply_tx);
        let admission = admit_member_status_observation(
            Arc::clone(&capacity),
            &waiters,
            MEMBER_STATUS_OBSERVATION_ADMISSION_TIMEOUT,
        );
        tokio::pin!(admission);
        assert!(
            tokio::time::timeout(Duration::from_millis(500), &mut admission)
                .await
                .is_err()
        );
        drop(held);
        assert!(matches!(
            admission.await,
            MemberStatusObservationAdmission::Admitted(_)
        ));
    }

    /// An observation whose callers all went away stops waiting at once and
    /// admits no later joiner.
    #[tokio::test(start_paused = true)]
    async fn abandoned_waiters_stop_admission_and_refuse_joiners() {
        let capacity = Arc::new(tokio::sync::Semaphore::new(0));
        let (first_tx, first_rx) = reply_channel();
        let waiters = MemberStatusObservationWaiters::new(first_tx);
        let (second_tx, second_rx) = reply_channel();
        assert!(waiters.join(second_tx).is_ok());
        let admission = admit_member_status_observation(
            capacity,
            &waiters,
            MEMBER_STATUS_OBSERVATION_ADMISSION_TIMEOUT,
        );
        tokio::pin!(admission);
        drop(first_rx);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), &mut admission)
                .await
                .is_err(),
            "one remaining caller keeps the observation"
        );
        drop(second_rx);
        assert!(matches!(
            tokio::time::timeout(Duration::from_millis(10), &mut admission)
                .await
                .expect("the last caller leaving abandons the wait"),
            MemberStatusObservationAdmission::Abandoned
        ));
        let (late_tx, _late_rx) = reply_channel();
        assert!(
            waiters.join(late_tx).is_err(),
            "an abandoned observation admits no joiner"
        );
    }

    /// Every joined caller receives the one result; an error is shared by
    /// its typed cause.
    #[tokio::test]
    async fn settle_delivers_one_result_to_every_joined_caller() {
        let (first_tx, first_rx) = reply_channel();
        let waiters = MemberStatusObservationWaiters::new(first_tx);
        let (second_tx, second_rx) = reply_channel();
        assert!(waiters.join(second_tx).is_ok());
        waiters.settle(Err(member_status_admission_deadline_error()));
        for rx in [first_rx, second_rx] {
            let error = rx
                .await
                .expect("settled caller receives a reply")
                .expect_err("the shared error");
            assert_eq!(
                error.structured_data().expect("typed data")["kind"],
                "mob_lifecycle_operation_admission_pending"
            );
        }
        let (late_tx, _late_rx) = reply_channel();
        assert!(waiters.join(late_tx).is_err(), "a settled set is closed");
    }
}
