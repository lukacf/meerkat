//! The runtime wake turn of one attempt.
//!
//! A boot restores its members in roster order; a wake restores one member at
//! runtime, possibly while other members wake. The turn is the actor's commit
//! step for that one attempt, on the member effect lane:
//!
//! - Step A (on the actor, [`MobActor::start_member_materialization`]): the
//!   attempt MobMachine authorized is still open; take the member's own fork
//!   of the authority, its overlay rule, the overlay this process last built
//!   it with (kept, as a warm revival keeps it), where a fork that follows
//!   its source takes that source's overlay from (see `fork_build`), its
//!   continuity intent, and dispatch the restore.
//! - Step B (off the actor, [`plan_member_wake`]): the cold restore recipe
//!   (`restore_resumed_member`) plans the member's restore on its own fork,
//!   writing nothing shared. A bound session the member's seat never created
//!   is created there, through the spawn provision path, under the seat's
//!   id; any other missing bound session searches its successor here and
//!   provisions it on a fork repointed to it. The woken comms runtime's
//!   endpoint is checked against the durable one.
//! - Step C (on the actor, [`MobActor::commit_member_wake_plan`]): the
//!   attempt is still this wake's (attempt and incarnation fences); a
//!   successor is repointed (journal first); `apply_member_restore_plan`
//!   applies the plan to the actor's authority, rosters and overlays in the
//!   member's turn, ending in `ResolveMemberRevivalBound` (the runtime
//!   binding is requested, the member is still Waking), or, for a session
//!   the member's seat never created, `ResolveMemberRevivalSessionCreated`
//!   after its born row is journaled.
//! - Readiness (the member's comms drain) and topology (its wiring restored
//!   over the machine's edges, dormant peers included), then
//!   `ResolveMemberRevivalSucceeded` (Waking -> Live) and every caller of the
//!   attempt is answered.
//!
//! Every wake feedback is attempt-fenced and answered by a machine verdict:
//! a stale attempt is a no-op, never a fail-stop. A provision a stale or
//! failed wake leaves behind is rolled back through its `PendingProvision`.

use super::member_effect_lane::{
    MemberEffectAck, MemberEffectCommit, MemberEffectCommitFuture, MemberEffectRequest,
    MemberEffectRetention, MemberEffectSend, MemberEffectSettlement, MemberFence,
};
use super::member_wake::{
    MemberWakeAttempt, MemberWakeFailure, MemberWakeOutcome, MemberWiringEdge,
    PendingMemberMaterialization, RunningMemberWake,
};
use super::*;
use crate::runtime::builder::{
    MemberRestoreCommit, MemberRestorePlan, MemberRestoreProvisionOutcome, MemberSuccessorSearch,
    PersistedMemberSessionIndex, RecordedContinuity, ResumeMemberRestoreContext,
    ResumeRestoreCandidate, RetryableRestoreCause, SeatedMemberRestoreOutcome,
    append_recovered_session_binding, apply_member_restore_plan, member_endpoint_broken_reason,
    member_endpoint_mismatch_detail, member_restore_failure, provision_durable_member_restore,
    restore_error_is_retryable, restore_resumed_member,
};

/// The wake one turn runs.
#[derive(Debug, Clone)]
struct WakeRun {
    identity: AgentIdentity,
    attempt: MemberWakeAttempt,
    cause: mob_dsl::MemberWakeCause,
}

/// What rolls back the provision a wake made, captured from its receipt
/// before the plan is applied: a wake that fails or goes stale after it
/// provisioned leaves no materialization the machine does not hold.
#[derive(Clone)]
struct WakeRollback {
    member_ref: MemberRef,
    operation_id: meerkat_core::ops::OperationId,
    session_origin: super::super::provisioner::ProvisionSessionOrigin,
    rollback_authority: Option<super::super::provisioner::ResumedMemberRollbackAuthority>,
    /// The provision created the session the member's seat named (its first
    /// materialization): a wake that ends retryable keeps that session
    /// ([`MobActor::wake_keeps_created_seat_session`]).
    seat_session: bool,
}

impl WakeRollback {
    fn from_plan(plan: &MemberRestorePlan) -> Option<Self> {
        let MemberRestorePlan::Seated {
            outcome: SeatedMemberRestoreOutcome::Provision(provision),
            ..
        } = plan
        else {
            return None;
        };
        let (receipt, seat_session) = match &provision.outcome {
            MemberRestoreProvisionOutcome::Durable { receipt, .. } => (receipt, false),
            MemberRestoreProvisionOutcome::Fresh {
                receipt,
                seat_session,
                ..
            } => (receipt, *seat_session),
            MemberRestoreProvisionOutcome::Failed(_)
            | MemberRestoreProvisionOutcome::Retryable { .. }
            | MemberRestoreProvisionOutcome::AlreadyLive { .. } => return None,
        };
        Some(Self {
            member_ref: receipt.member_ref.clone(),
            operation_id: receipt.operation_id.clone(),
            session_origin: receipt.session_origin,
            rollback_authority: receipt.rollback_authority.clone(),
            seat_session,
        })
    }

    fn pending(
        self,
        identity: &AgentIdentity,
        provisioner: Arc<dyn MobProvisioner>,
    ) -> PendingProvision {
        PendingProvision::new(
            self.member_ref,
            identity.clone(),
            provisioner,
            self.operation_id,
            self.session_origin,
            self.rollback_authority,
            // A failed wake keeps the member seated.
            super::super::provisioner::RollbackOrigin::RevivalOrRebuild,
        )
    }
}

/// The woken member's endpoint as its new comms runtime presents it.
enum WakeEndpoint {
    /// It matches the durable endpoint (or nothing was materialized).
    Matches,
    /// No durable endpoint is recorded yet (a legacy member, or a successor):
    /// record this one once the plan applied.
    Record(TrustedPeerDescriptor),
}

/// What Step B hands to the member's turn.
enum WakePlan {
    /// The bound session's restore plan.
    Restore {
        plan: MemberRestorePlan,
        endpoint: WakeEndpoint,
    },
    /// The bound session is absent: its persisted successor, provisioned on
    /// a fork repointed to it. The turn repoints the binding (journal first)
    /// and applies the plan.
    Successor {
        successor_session_id: SessionId,
        plan: MemberRestorePlan,
        endpoint: WakeEndpoint,
    },
    /// The bound session is absent and has no successor.
    NoSuccessor { bridge_session_id: SessionId },
    /// The woken comms runtime presents an endpoint other than the durable
    /// one; its provision was already rolled back.
    EndpointMismatch {
        bridge_session_id: SessionId,
        reason: String,
    },
}

/// The topology step's edges, taken a chunk at a time
/// ([`super::wake_topology`]).
struct WakeTopology {
    /// The edges no chunk has taken yet, in order.
    remaining: VecDeque<super::wake_topology::WakeTopologyEdge>,
    /// The current chunk's edges that take the single-edge wire path.
    single: VecDeque<super::wake_topology::WakeTopologyEdge>,
    /// The edges each chunk takes, fixed when the step starts.
    chunk: usize,
    /// A chunk ran since the turn last returned to the actor: the turn
    /// yields before it takes the next one.
    yield_due: bool,
    failed: Vec<RespawnTopologyPeerId>,
}

/// One step of a wake turn, carried back to the actor.
enum WakeStep {
    Planned(Box<Result<WakePlan, MobError>>),
    Ready(Result<(), MobError>),
    Topology {
        topology: WakeTopology,
        peer: RespawnTopologyPeerId,
        edge: Box<MemberWiringEdge>,
        result: Result<(), MobError>,
    },
    /// The topology step yielded to the actor between two chunks.
    TopologyYield {
        topology: WakeTopology,
    },
    /// A rollback of the wake's provision finished (`false`: it could not be
    /// settled and its custody was abandoned).
    RolledBack {
        settled: bool,
    },
    /// The effects never returned their own step.
    Unsettled,
}

struct WakeCommit {
    run: WakeRun,
    rollback: Option<WakeRollback>,
    step: WakeStep,
}

impl MemberEffectCommit for WakeCommit {
    fn commit(
        self: Box<Self>,
        actor: &mut MobActor,
        settlement: MemberEffectSettlement,
    ) -> MemberEffectCommitFuture<'_> {
        Box::pin(async move {
            if actor.durable_uncertainty_fail_stop.is_set() {
                return MemberEffectAck::Retained(MemberEffectRetention::unresumable(
                    MobError::Internal("member wake commit is held by fail-stop".to_string()),
                ));
            }
            boxed_arm_future(|| actor.commit_member_wake_step(*self, settlement)).await;
            MemberEffectAck::Settled
        })
    }
}

/// Everything Step B reads, owned: the effect future outlives the actor's
/// borrow (never hand a borrowed item into a `Send` future).
struct WakeRestoreInput {
    definition: Arc<MobDefinition>,
    session_service: Arc<dyn MobSessionService>,
    provisioner: Arc<super::super::provisioner::MultiBackendProvisioner>,
    provisioner_dyn: Arc<dyn MobProvisioner>,
    continuity: RecordedContinuity,
    spawn_member_customizer: Option<Arc<dyn super::super::SpawnMemberCustomizer>>,
    tool_bundles: BTreeMap<String, Arc<dyn AgentToolDispatcher>>,
    tool_handle: MobHandle,
    default_external_tools_provider: Option<crate::ExternalToolsProvider>,
    realm_profile_store: Option<Arc<dyn crate::store::RealmProfileStore>>,
    default_llm_client: Option<Arc<dyn LlmClient>>,
    tool_consequence_policy_registry: Option<Arc<meerkat_core::ToolConsequencePolicyRegistry>>,
    member_build_preparer: Option<Arc<dyn super::super::MemberBuildPreparer>>,
    /// The read-only view the host's build preparer receives: the launch's
    /// own (the owner's authority), never `tool_handle`'s agent lane.
    host_read_handle: super::super::MobReadHandle,
    candidate: ResumeRestoreCandidate,
    base: mob_dsl::MobMachineAuthority,
    fork_rule: super::super::fork_build::ForkOverlayRule,
    /// The overlay this process last built the member with, if any.
    retained: Option<super::super::fork_build::RetainedMemberOverlay>,
    /// For a fork that follows its source and that this process did not
    /// build: the source, and where its overlay comes from.
    followed: Option<(
        AgentIdentity,
        super::super::fork_build::FollowedSourceOverlay,
    )>,
    durable_endpoint: Option<mob_dsl::MemberPeerEndpoint>,
    member_bound_sessions: HashSet<SessionId>,
}

/// Test probe of one mob's member wake restores (step B): the members whose
/// restore started, in order, the most restores that ran at once, and holds
/// on chosen restores (a member's first, second, ... restore since the probe
/// was installed) that wait at their start until the row releases them, or
/// end them there, before they read anything.
#[cfg(test)]
pub(in crate::runtime) struct MemberWakeRestoreProbe {
    /// The members whose restore started, in order.
    started: tokio::sync::watch::Sender<Vec<AgentIdentity>>,
    /// Restores started and not ended.
    running: std::sync::atomic::AtomicUsize,
    /// The most restores that ran at once.
    max_running: std::sync::atomic::AtomicUsize,
    /// The holds armed, by member and the restore of it they hold (1 for the
    /// member's first restore).
    held: std::sync::Mutex<BTreeMap<(AgentIdentity, usize), Arc<MemberWakeRestoreHold>>>,
}

/// One held restore: it waits at its start until the gate closes, and ends
/// there when the row aborted it.
#[cfg(test)]
struct MemberWakeRestoreHold {
    /// Never given a permit: releasing or aborting the hold closes it.
    gate: tokio::sync::Semaphore,
    aborted: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
fn armed_member_wake_restore_hold() -> Arc<MemberWakeRestoreHold> {
    Arc::new(MemberWakeRestoreHold {
        gate: tokio::sync::Semaphore::new(0),
        aborted: std::sync::atomic::AtomicBool::new(false),
    })
}

#[cfg(test)]
static MEMBER_WAKE_RESTORE_PROBES: std::sync::Mutex<BTreeMap<MobId, Arc<MemberWakeRestoreProbe>>> =
    std::sync::Mutex::new(BTreeMap::new());

/// Test seam: watch the member wake restores of `mob_id` from now on, and
/// hold the first restore of each member in `held` at its start until the
/// row releases it ([`MemberWakeRestoreProbe::release`]).
#[cfg(test)]
pub(in crate::runtime) fn probe_member_wake_restores_for_test(
    mob_id: &MobId,
    held: impl IntoIterator<Item = AgentIdentity>,
) -> Arc<MemberWakeRestoreProbe> {
    let probe = Arc::new(MemberWakeRestoreProbe {
        started: tokio::sync::watch::Sender::new(Vec::new()),
        running: std::sync::atomic::AtomicUsize::new(0),
        max_running: std::sync::atomic::AtomicUsize::new(0),
        held: std::sync::Mutex::new(
            held.into_iter()
                .map(|identity| ((identity, 1), armed_member_wake_restore_hold()))
                .collect(),
        ),
    });
    MEMBER_WAKE_RESTORE_PROBES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(mob_id.clone(), Arc::clone(&probe));
    probe
}

/// A restore of `identity` in `mob_id` starts: recorded by the mob's probe,
/// if one is installed, and held while its hold is armed. Counted running
/// until the answer is dropped. The lift's rebuild preparation of a member it
/// realizes through the explicit-resume work path enters it too.
#[cfg(test)]
pub(in crate::runtime) async fn enter_member_wake_restore_probe(
    mob_id: &MobId,
    identity: AgentIdentity,
) -> Option<ProbedMemberWakeRestore> {
    let probe = MEMBER_WAKE_RESTORE_PROBES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(mob_id)
        .cloned()?;
    Some(probe.enter(identity).await)
}

/// One restore the probe counts as running until it is dropped.
#[cfg(test)]
pub(in crate::runtime) struct ProbedMemberWakeRestore {
    probe: Arc<MemberWakeRestoreProbe>,
    /// The row ended this restore at its start: it reads nothing.
    pub(in crate::runtime) aborted: bool,
}

#[cfg(test)]
impl Drop for ProbedMemberWakeRestore {
    fn drop(&mut self) {
        self.probe
            .running
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
impl MemberWakeRestoreProbe {
    /// A restore of `identity` starts: recorded and counted running, then
    /// held while its hold is armed.
    async fn enter(self: Arc<Self>, identity: AgentIdentity) -> ProbedMemberWakeRestore {
        let running = self
            .running
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        self.max_running
            .fetch_max(running, std::sync::atomic::Ordering::SeqCst);
        let mut restore = 0;
        self.started.send_modify(|started| {
            restore = started.iter().filter(|member| **member == identity).count() + 1;
            started.push(identity.clone());
        });
        let hold = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(identity, restore))
            .cloned();
        let aborted = match hold {
            Some(hold) => {
                let _closed = hold.gate.acquire().await;
                hold.aborted.load(std::sync::atomic::Ordering::SeqCst)
            }
            None => false,
        };
        ProbedMemberWakeRestore {
            probe: self,
            aborted,
        }
    }

    /// Hold `identity`'s restore number `restore` (1 for its first) at its
    /// start until the row releases or aborts it.
    pub(in crate::runtime) fn hold(&self, identity: &AgentIdentity, restore: usize) {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry((identity.clone(), restore))
            .or_insert_with(armed_member_wake_restore_hold);
    }

    /// Let every held restore of `identity` go on; no later restore of it is
    /// held.
    pub(in crate::runtime) fn release(&self, identity: &AgentIdentity) {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(member, _), hold| {
                if member == identity {
                    hold.gate.close();
                    false
                } else {
                    true
                }
            });
    }

    /// End `identity`'s held restore number `restore` at its start, before it
    /// reads anything: its wake fails as a restore that read nothing.
    pub(in crate::runtime) fn abort(&self, identity: &AgentIdentity, restore: usize) {
        let hold = self
            .held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(identity.clone(), restore));
        if let Some(hold) = hold {
            hold.aborted
                .store(true, std::sync::atomic::Ordering::SeqCst);
            hold.gate.close();
        }
    }

    /// The members whose restore started so far, in order.
    pub(in crate::runtime) fn started(&self) -> Vec<AgentIdentity> {
        self.started.borrow().clone()
    }

    /// Wait until `count` restores started.
    pub(in crate::runtime) async fn wait_for_started(&self, count: usize) {
        let mut started = self.started.subscribe();
        let _ = started.wait_for(|started| started.len() >= count).await;
    }

    /// The most restores that ran at once.
    pub(in crate::runtime) fn max_running(&self) -> usize {
        self.max_running.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl MobActor {
    /// Start every materialization MobMachine handed off since the last loop
    /// boundary.
    ///
    /// One effect of a member at a time: an open attempt of a member that
    /// still has an earlier wake or warm-reload step in flight (a Stop or a
    /// Complete left it running; its late commit fails its attempt fence and
    /// rolls its provision back) does not start its restore beside that
    /// step. It stays handed off, with its wake bound armed, and starts at
    /// the first loop boundary after the member's last such step settled.
    pub(in crate::runtime) async fn start_pending_member_materializations(&mut self) {
        let mut waiting = Vec::new();
        while let Some(pending) = self.member_wakes.pending.pop_front() {
            if self.member_wake_attempt_is_open(&pending.identity, pending.attempt)
                && self.member_wake_effects_in_flight(&pending.identity)
            {
                self.begin_running_member_wake(&pending.identity, pending.cause, pending.attempt);
                waiting.push(pending);
                continue;
            }
            self.start_member_materialization(pending).await;
        }
        self.member_wakes.pending.extend(waiting);
    }

    /// Take `attempt` as a wake this actor runs and arm the mob's wake bound
    /// for it, once per attempt.
    fn begin_running_member_wake(
        &mut self,
        identity: &AgentIdentity,
        cause: mob_dsl::MemberWakeCause,
        attempt: MemberWakeAttempt,
    ) {
        if self.member_wakes.running.contains_key(&attempt) {
            return;
        }
        self.member_wakes.running.insert(
            attempt,
            RunningMemberWake {
                identity: identity.clone(),
                cause,
            },
        );
        self.arm_member_wake_bound(identity, attempt);
    }

    /// Step A: dispatch the cold restore of one authorized attempt.
    async fn start_member_materialization(&mut self, pending: PendingMemberMaterialization) {
        let PendingMemberMaterialization {
            identity,
            cause,
            attempt,
        } = pending;
        if !self.member_wake_attempt_is_open(&identity, attempt) {
            // The machine closed it before it started (a stop, a retire):
            // its callers are answered from the state the closure left.
            self.finish_closed_member_wake(&identity, attempt).await;
            return;
        }
        let run = WakeRun {
            identity: identity.clone(),
            attempt,
            cause,
        };
        self.begin_running_member_wake(&identity, cause, attempt);

        // The seat's build inputs ride the member's first build, as a
        // spawn's do; a member whose session a build created has none.
        let (entry, seat_build) = {
            let roster = self.roster.read().await;
            (roster.get(&identity).cloned(), roster.seat_build(&identity))
        };
        let Some(entry) = entry else {
            self.abandon_member_wake(&run, mob_dsl::MemberWakeAbandonKind::NotCurrent, None)
                .await;
            return;
        };
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&identity);
        let (machine_session_id, durable_endpoint, member_bound_sessions) = {
            let state = self.dsl_authority.state();
            (
                state.member_session_bindings.get(&dsl_identity).cloned(),
                state.member_peer_endpoints.get(&dsl_identity).cloned(),
                state
                    .member_session_bindings
                    .values()
                    .filter_map(|session| SessionId::parse(&session.0).ok())
                    .collect::<HashSet<_>>(),
            )
        };
        let Some(machine_session_id) = machine_session_id else {
            self.abandon_member_wake(&run, mob_dsl::MemberWakeAbandonKind::NotCurrent, None)
                .await;
            return;
        };
        if let Err(error) = self
            .member_wakes
            .continuity
            .refresh(&self.events, &self.definition.id)
            .await
        {
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %identity,
                %error,
                "the member's continuity intent could not be read; its wake is retried later"
            );
            self.abandon_member_wake(&run, mob_dsl::MemberWakeAbandonKind::Unsettled, None)
                .await;
            return;
        }
        let continuity = self.member_wakes.continuity.slice_for(&identity);
        let fork_rule = self.fork_overlay_rule(&entry).await;
        // The overlay this process last built the member with, which the
        // member keeps (a released member, or a revival through its wake), as
        // its warm revival does. A fork that follows its source and that this
        // process did not build takes the overlay its source holds here, or
        // the one its source would be rebuilt with, computed in the restore
        // without waking the source (see `fork_build`).
        let retained = self
            .per_spawn_external_tools
            .read()
            .await
            .retained(&identity);
        let followed = match &fork_rule {
            super::super::fork_build::ForkOverlayRule::FollowSource(source)
                if retained.is_none() =>
            {
                self.followed_source_overlay(source)
                    .await
                    .map(|followed| (source.clone(), followed))
            }
            _ => None,
        };
        let input = WakeRestoreInput {
            definition: Arc::clone(&self.definition),
            session_service: Arc::clone(&self.session_service),
            provisioner: Arc::clone(&self.member_wakes.provisioner),
            provisioner_dyn: Arc::clone(&self.provisioner),
            continuity,
            spawn_member_customizer: self.spawn_member_customizer.clone(),
            tool_bundles: self.tool_bundles.clone(),
            tool_handle: self.mob_handle_for_tools(),
            default_external_tools_provider: self.default_external_tools_provider.clone(),
            realm_profile_store: self.realm_profile_store.clone(),
            default_llm_client: self.default_llm_client.clone(),
            tool_consequence_policy_registry: self.tool_consequence_policy_registry.clone(),
            member_build_preparer: self.member_residency.build_preparer.clone(),
            host_read_handle: self.member_wakes.host_read_handle.clone(),
            candidate: ResumeRestoreCandidate {
                entry: entry.clone(),
                machine_session_id,
                in_turn: false,
                seat_build,
            },
            base: self.dsl_authority.fork(),
            fork_rule,
            retained,
            followed,
            durable_endpoint,
            member_bound_sessions,
        };
        #[cfg(test)]
        let (probe_mob, probe_member) = (self.definition.id.clone(), identity.clone());
        self.dispatch_member_wake_step(
            run,
            MemberFence::exact(&entry),
            None,
            "member_wake_restore",
            async move {
                #[cfg(test)]
                let probed = enter_member_wake_restore_probe(&probe_mob, probe_member).await;
                #[cfg(test)]
                if probed.as_ref().is_some_and(|restore| restore.aborted) {
                    return WakeStep::Planned(Box::new(Err(MobError::Internal(
                        "the test probe ended this member wake restore at its start".to_string(),
                    ))));
                }
                WakeStep::Planned(Box::new(plan_member_wake(input).await))
            },
        );
    }

    fn dispatch_member_wake_step<F>(
        &mut self,
        run: WakeRun,
        fence: MemberFence,
        rollback: Option<WakeRollback>,
        context: &'static str,
        effects: F,
    ) where
        F: std::future::Future<Output = WakeStep> + MemberEffectSend + 'static,
    {
        let worker_run = run.clone();
        let worker_rollback = rollback.clone();
        self.dispatch_member_wake_effect(MemberEffectRequest {
            context,
            members: vec![fence],
            effects: Box::pin(async move {
                Box::new(WakeCommit {
                    run: worker_run,
                    rollback: worker_rollback,
                    step: effects.await,
                }) as Box<dyn MemberEffectCommit>
            }),
            unsettled_commit: Box::new(WakeCommit {
                run,
                rollback,
                step: WakeStep::Unsettled,
            }),
        });
    }

    async fn commit_member_wake_step(
        &mut self,
        commit: WakeCommit,
        settlement: MemberEffectSettlement,
    ) {
        let WakeCommit {
            run,
            rollback,
            step,
        } = commit;
        if settlement.unsettled.is_some() || matches!(step, WakeStep::Unsettled) {
            // The effects never published their own result: whether the
            // provision happened is unknown, so the mob fail-stops (its
            // state is discarded), after it abandoned the attempt
            // explicitly so no caller waits on it.
            self.abandon_member_wake(&run, mob_dsl::MemberWakeAbandonKind::Unsettled, None)
                .await;
            let _ = self
                .durable_uncertainty_fail_stop
                .record(MobError::Internal(
                    "member wake effect ended without owned settlement evidence".to_string(),
                ));
            return;
        }
        match step {
            WakeStep::Planned(planned) => match *planned {
                Ok(plan) => {
                    self.commit_member_wake_plan(run, plan, settlement.fence.is_current())
                        .await;
                }
                // Work still in flight on the member's session (its
                // attachment's takeover) or a store that cannot serve it now
                // never breaks the member: the wake ends retryable.
                Err(error) if restore_error_is_retryable(&error) => {
                    self.end_member_wake_retryable(&run, error).await;
                }
                Err(error) => self.fail_member_wake(&run, None, &error).await,
            },
            WakeStep::Ready(result) => {
                self.member_wake_ready(run, rollback, settlement.fence.is_current(), result)
                    .await;
            }
            WakeStep::Topology {
                mut topology,
                peer,
                edge,
                result,
            } => {
                if !settlement.fence.is_current()
                    || !self.member_wake_attempt_is_open(&run.identity, run.attempt)
                {
                    self.roll_back_member_wake(run, rollback);
                    return;
                }
                if let Err(error) = &result {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        agent_identity = %run.identity,
                        peer = %peer,
                        %error,
                        "a woken member's wiring edge could not be realized; its repair is owed"
                    );
                    topology.failed.push(peer);
                }
                self.resolve_member_edge_realization(&edge, result.is_ok());
                self.advance_member_wake_topology(run, rollback, topology)
                    .await;
            }
            WakeStep::TopologyYield { topology } => {
                if !settlement.fence.is_current()
                    || !self.member_wake_attempt_is_open(&run.identity, run.attempt)
                {
                    self.roll_back_member_wake(run, rollback);
                    return;
                }
                self.advance_member_wake_topology(run, rollback, topology)
                    .await;
            }
            WakeStep::RolledBack { settled } => {
                if !settled {
                    tracing::error!(
                        mob_id = %self.definition.id,
                        agent_identity = %run.identity,
                        attempt = run.attempt,
                        "a failed member wake could not roll back its provision"
                    );
                }
                if self.member_wake_attempt_is_open(&run.identity, run.attempt) {
                    // Rolled back while the attempt stayed open (a fenced
                    // incarnation change): close it, nothing is materialized.
                    self.abandon_member_wake(
                        &run,
                        mob_dsl::MemberWakeAbandonKind::NotCurrent,
                        None,
                    )
                    .await;
                } else {
                    self.finish_closed_member_wake(&run.identity, run.attempt)
                        .await;
                }
            }
            WakeStep::Unsettled => {}
        }
    }

    /// Step C: apply the member's plan in its turn.
    async fn commit_member_wake_plan(&mut self, run: WakeRun, plan: WakePlan, fence_current: bool) {
        let plan_rollback = match &plan {
            WakePlan::Restore { plan, .. } | WakePlan::Successor { plan, .. } => {
                WakeRollback::from_plan(plan)
            }
            WakePlan::NoSuccessor { .. } | WakePlan::EndpointMismatch { .. } => None,
        };
        if !fence_current
            || !self.member_wake_attempt_is_open(&run.identity, run.attempt)
            || self.retirements.contains_key(&run.identity)
        {
            // Stale: the machine closed the attempt, or the member's
            // incarnation changed, while it planned, or a retirement of the
            // member is in progress (its start closes this attempt, and its
            // steps are fenced to the incarnation it started from). Nothing
            // of the plan is applied; its provision is rolled back.
            self.roll_back_member_wake(run, plan_rollback);
            return;
        }
        let (plan, endpoint, successor) = match plan {
            WakePlan::NoSuccessor { bridge_session_id } => {
                let diagnostic = member_restore_failure(
                    &bridge_session_id,
                    format!("missing durable session snapshot for '{bridge_session_id}'"),
                );
                self.fail_member_wake_with(&run, diagnostic).await;
                return;
            }
            WakePlan::EndpointMismatch {
                bridge_session_id,
                reason,
            } => {
                let diagnostic = member_restore_failure(&bridge_session_id, reason);
                self.fail_member_wake_with(&run, diagnostic).await;
                return;
            }
            WakePlan::Restore { plan, endpoint } => (plan, endpoint, None),
            WakePlan::Successor {
                successor_session_id,
                plan,
                endpoint,
            } => (plan, endpoint, Some(successor_session_id)),
        };
        // Outcomes the cold recipe already decided are resolved here,
        // through the machine, instead of being written as a boot restore
        // writes them (a boot records a diagnostic and seeds Broken later).
        match &plan {
            MemberRestorePlan::Unseated(diagnostic) => {
                self.fail_member_wake_with(&run, diagnostic.clone()).await;
                return;
            }
            MemberRestorePlan::Seated {
                outcome: SeatedMemberRestoreOutcome::Refused(diagnostic),
                ..
            } => {
                self.fail_member_wake_with(&run, diagnostic.clone()).await;
                return;
            }
            MemberRestorePlan::Seated {
                outcome: SeatedMemberRestoreOutcome::Provision(provision),
                ..
            } => match &provision.outcome {
                MemberRestoreProvisionOutcome::Failed(diagnostic) => {
                    self.fail_member_wake_with(&run, diagnostic.clone()).await;
                    return;
                }
                MemberRestoreProvisionOutcome::Retryable { reason, cause } => {
                    tracing::info!(
                        mob_id = %self.definition.id,
                        agent_identity = %run.identity,
                        ?cause,
                        %reason,
                        "the member's wake is to be retried"
                    );
                    self.member_wakes.running.remove(&run.attempt);
                    self.resolve_member_wake_retryable(
                        &run.identity,
                        run.attempt,
                        cause.wake_kind(),
                        cause.wake_refusal(),
                        "member_wake_restore_retryable",
                    )
                    .await;
                    return;
                }
                // A session another materialization made live is kept: the
                // plan's application binds to it, as a warm revival does.
                MemberRestoreProvisionOutcome::Durable { .. }
                | MemberRestoreProvisionOutcome::Fresh { .. }
                | MemberRestoreProvisionOutcome::AlreadyLive { .. } => {}
            },
            MemberRestorePlan::Seated {
                outcome: SeatedMemberRestoreOutcome::SuccessorNeeded { .. },
                ..
            } => {
                // Step B runs the successor search; reaching here is a
                // planning fault, never a delivery answer.
                self.fail_member_wake(
                    &run,
                    plan_rollback,
                    &MobError::Internal(
                        "a member wake reached its turn without its successor search".to_string(),
                    ),
                )
                .await;
                return;
            }
            MemberRestorePlan::Kept => {}
        }
        let Some(entry) = self.roster.read().await.get(&run.identity).cloned() else {
            self.roll_back_member_wake(run, plan_rollback);
            return;
        };
        if let Some(successor) = successor.as_ref() {
            // The successor repoint is durable history before it is
            // authority (journal first), exactly as a boot repoints.
            match append_recovered_session_binding(
                &mut self.dsl_authority,
                &self.events,
                &self.definition.id,
                &entry,
                successor,
                None,
                "member_wake_repoint_missing_member_session_binding",
            )
            .await
            {
                Ok(event) => self.roster.write().await.apply_event(&event),
                Err(error) => {
                    self.fail_member_wake(&run, plan_rollback, &error).await;
                    return;
                }
            }
        }
        let applied = self.apply_member_wake_plan(&entry, plan).await;
        self.publish_machine_state_projection();
        // The plan's seeded inputs ran on the authority directly; realize
        // the member's wake route from the state they left (a successor
        // repoint moved its session).
        self.publish_member_wake_route(&run.identity);
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&run.identity);
        let bound = self
            .dsl_authority
            .state()
            .member_wake_bound
            .contains(&dsl_identity);
        match applied {
            Ok(()) if bound => {}
            Ok(()) => {
                // The turn recorded a per-member failure (its binding or its
                // runtime binding was refused): Broken, through the machine.
                match self.restore_failure_for(&run.identity).await {
                    Some(diagnostic) => {
                        self.roll_back_member_wake_provision(&run, plan_rollback.clone());
                        self.fail_member_wake_with(&run, diagnostic).await;
                    }
                    None => {
                        self.fail_member_wake(
                            &run,
                            plan_rollback,
                            &MobError::Internal(
                                "a member wake's plan applied without binding the member"
                                    .to_string(),
                            ),
                        )
                        .await;
                    }
                }
                return;
            }
            Err(error) => {
                // A restore input MobMachine admitted on the member's fork was
                // refused on the mob's authority (the lifecycle moved): never
                // a failed mob, only this wake's typed answer and rollback.
                tracing::warn!(
                    mob_id = %self.definition.id,
                    agent_identity = %run.identity,
                    %error,
                    "a member wake's plan was refused in its turn"
                );
                // Rolled back before the attempt ends (NotCurrent while it
                // is open): a seat session the plan created and did not
                // journal born is kept for the next wake.
                self.roll_back_member_wake(run, plan_rollback);
                return;
            }
        }
        // Register the woken member's endpoint where none was durable yet,
        // before its Ready verdict: a member owed EndpointPending is never
        // Live without one. A registration MobMachine refuses fails the wake.
        if let WakeEndpoint::Record(spec) = endpoint
            && let Err(error) = self.record_woken_member_endpoint(&run.identity, spec).await
        {
            self.fail_member_wake(&run, plan_rollback, &error).await;
            return;
        }
        self.dispatch_member_wake_readiness(run, plan_rollback)
            .await;
    }

    /// Apply a plan through `apply_member_restore_plan` over the
    /// actor's own state: its authority, its roster, its retained overlays.
    async fn apply_member_wake_plan(
        &mut self,
        entry: &RosterEntry,
        plan: MemberRestorePlan,
    ) -> Result<(), MobError> {
        let tool_handle = self.mob_handle_for_tools();
        let no_active_ids = HashSet::new();
        let continuity = self
            .member_wakes
            .continuity
            .slice_for(&entry.agent_identity);
        let mut roster = self.roster.write().await;
        let mut retained_overlays = self.per_spawn_external_tools.write().await;
        let mut restored_overlays = BTreeMap::new();
        let mut successor_index = None;
        let mut recovered_without_endpoint = HashSet::new();
        let no_listed_sessions = Vec::new();
        let no_bound_sessions = HashSet::new();
        let ctx = ResumeMemberRestoreContext {
            definition: &self.definition,
            session_service: &self.session_service,
            provisioner: &self.member_wakes.provisioner,
            active_ids: &no_active_ids,
            recorded_continuity: &continuity,
            spawn_member_customizer: &self.spawn_member_customizer,
            tool_bundles: &self.tool_bundles,
            tool_handle: &tool_handle,
            default_external_tools_provider: &self.default_external_tools_provider,
            realm_profile_store: self.realm_profile_store.as_ref(),
            default_llm_client: self.default_llm_client.as_ref(),
            tool_consequence_policy_registry: self.tool_consequence_policy_registry.as_ref(),
            member_build_preparer: None,
            host_read_handle: &self.member_wakes.host_read_handle,
            unreadable_view_is_retryable: false,
        };
        let mut commit = MemberRestoreCommit {
            authority: &mut self.dsl_authority,
            roster: roster.restore_projection_mut(),
            per_spawn_external_tools_seed: &mut retained_overlays,
            restored_overlays: &mut restored_overlays,
            tool_handle: &tool_handle,
            composition_binding: &self.composition_binding,
        };
        let mut successors = MemberSuccessorSearch {
            listed_sessions: &no_listed_sessions,
            member_bound_sessions: &no_bound_sessions,
            index: &mut successor_index,
            recovered_without_endpoint: &mut recovered_without_endpoint,
        };
        apply_member_restore_plan(&ctx, &mut commit, &mut successors, entry, plan).await
    }

    /// Register the woken member's endpoint where none was durable: journal
    /// first, then authority and rosters, exactly as a boot upgrades a
    /// recovered binding without one. A wired member's recovered endpoint is
    /// refused there (its wiring predates any durable endpoint), so it is
    /// registered on its spawned incarnation instead
    /// (`RecoverSpawnedMemberPeerEndpoint`, which ends its EndpointPending
    /// exception); that registration lives in this process, and a later
    /// start wakes the member again to register it. Both refused is an
    /// error: the member cannot be Live without an endpoint.
    async fn record_woken_member_endpoint(
        &mut self,
        identity: &AgentIdentity,
        spec: TrustedPeerDescriptor,
    ) -> Result<(), MobError> {
        let Some(entry) = self.roster.read().await.get(identity).cloned() else {
            return Ok(());
        };
        let Some(bridge_session_id) = entry.member_ref.bridge_session_id().cloned() else {
            return Ok(());
        };
        match append_recovered_session_binding(
            &mut self.dsl_authority,
            &self.events,
            &self.definition.id,
            &entry,
            &bridge_session_id,
            Some(spec.clone()),
            "member_wake_record_member_peer_endpoint",
        )
        .await
        {
            Ok(event) => self.roster.write().await.apply_event(&event),
            Err(recovered_error) => {
                self.apply_dsl_signal(
                    mob_dsl::MobMachineSignal::RecoverSpawnedMemberPeerEndpoint {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(identity),
                        agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(
                            &entry.agent_runtime_id,
                        ),
                        fence_token: mob_dsl::FenceToken::from_domain(entry.fence_token),
                        generation: mob_dsl::Generation::from_domain(entry.generation),
                        peer_endpoint: mob_dsl::MemberPeerEndpoint::from(&spec),
                    },
                    "member_wake_register_spawned_member_peer_endpoint",
                )
                .map_err(|spawned_error| {
                    MobError::Internal(format!(
                        "the woken member '{identity}' could not register its endpoint: \
                         {recovered_error}; on its spawned incarnation: {spawned_error}"
                    ))
                })?;
            }
        }
        self.publish_machine_state_projection();
        self.publish_member_wake_route(identity);
        Ok(())
    }

    /// Readiness: the woken member's comms drain, as every readiness path
    /// requires it ([`super::member_requires_comms_drain`]).
    async fn dispatch_member_wake_readiness(
        &mut self,
        run: WakeRun,
        rollback: Option<WakeRollback>,
    ) {
        let Some(entry) = self.roster.read().await.get(&run.identity).cloned() else {
            self.roll_back_member_wake(run, rollback);
            return;
        };
        let drain_required = super::member_requires_comms_drain(self.dsl_authority.state(), &entry);
        let readiness = self.detached_member_readiness_context();
        let identity = run.identity.clone();
        let member_ref = entry.member_ref.clone();
        self.dispatch_member_wake_step(
            run,
            MemberFence::exact(&entry),
            rollback,
            "member_wake_readiness",
            async move {
                #[cfg(test)]
                wait_for_wake_readiness_release_for_test(&identity).await;
                WakeStep::Ready(
                    readiness
                        .ensure_member_comms_drain(&identity, &member_ref, drain_required)
                        .await,
                )
            },
        );
    }

    async fn member_wake_ready(
        &mut self,
        run: WakeRun,
        rollback: Option<WakeRollback>,
        fence_current: bool,
        result: Result<(), MobError>,
    ) {
        if !fence_current || !self.member_wake_attempt_is_open(&run.identity, run.attempt) {
            self.roll_back_member_wake(run, rollback);
            return;
        }
        if let Err(error) = result {
            self.roll_back_member_wake_provision(&run, rollback);
            self.abandon_member_wake(
                &run,
                mob_dsl::MemberWakeAbandonKind::ReadinessFailed,
                Some(format!(
                    "the woken member's comms readiness failed: {error}"
                )),
            )
            .await;
            return;
        }
        let plan = match self.machine_restore_wiring_plan(&run.identity) {
            Ok(plan) => plan,
            Err(error) => {
                self.roll_back_member_wake_provision(&run, rollback);
                self.abandon_member_wake(
                    &run,
                    mob_dsl::MemberWakeAbandonKind::TopologyFailed,
                    Some(format!("the woken member's wiring plan failed: {error}")),
                )
                .await;
                return;
            }
        };
        let local = mob_dsl::AgentIdentity::from_domain(&run.identity);
        let mut remaining: VecDeque<_> = plan
            .local_peers
            .into_iter()
            .filter(|peer| peer != &run.identity)
            .map(|peer| {
                let key = RespawnTopologyPeerId::from(peer.as_str());
                let edge = MemberWiringEdge::Member(mob_dsl::WiringEdge::new(
                    local.clone(),
                    mob_dsl::AgentIdentity::from_domain(&peer),
                ));
                (super::super::handle::PeerTarget::Local(peer), key, edge)
            })
            .collect();
        // The member's external edges, keyed as MobMachine keys them.
        let external_edges: Vec<mob_dsl::ExternalPeerEdge> = self
            .dsl_authority
            .state()
            .external_peer_edges_by_key
            .values()
            .filter(|edge| edge.local == local)
            .cloned()
            .collect();
        for edge in external_edges {
            match Self::trusted_peer_descriptor_from_machine_endpoint(&edge.endpoint) {
                Ok(descriptor) => {
                    let key = RespawnTopologyPeerId::from(descriptor.peer_id.as_str());
                    remaining.push_back((
                        super::super::handle::PeerTarget::External(descriptor),
                        key,
                        MemberWiringEdge::External(edge),
                    ));
                }
                Err(error) => {
                    self.roll_back_member_wake_provision(&run, rollback);
                    self.abandon_member_wake(
                        &run,
                        mob_dsl::MemberWakeAbandonKind::TopologyFailed,
                        Some(format!("the woken member's wiring plan failed: {error}")),
                    )
                    .await;
                    return;
                }
            }
        }
        let chunk = self.wake_topology_chunk_size(remaining.len());
        self.advance_member_wake_topology(
            run,
            rollback,
            WakeTopology {
                remaining,
                single: VecDeque::new(),
                chunk,
                yield_due: false,
                failed: Vec::new(),
            },
        )
        .await;
    }

    /// Topology: restore the woken member's wiring over the machine's edges,
    /// a chunk at a time ([`super::wake_topology`]): a chunk's edges to
    /// members that are not materialized as one batch, its other edges one
    /// at a time on the wiring lane. The turn yields to the actor between
    /// chunks. Then it resolves the member Ready.
    async fn advance_member_wake_topology(
        &mut self,
        run: WakeRun,
        rollback: Option<WakeRollback>,
        mut topology: WakeTopology,
    ) {
        loop {
            while let Some((target, peer, edge)) = topology.single.pop_front() {
                if self.wake_edge_unwired(&edge) {
                    tracing::debug!(
                        mob_id = %self.definition.id,
                        agent_identity = %run.identity,
                        peer = %peer,
                        "a woken member's edge MobMachine no longer holds is not restored"
                    );
                    continue;
                }
                match self.prepare_member_wire(run.identity.clone(), target).await {
                    Ok(prepared) => {
                        if let Some(prepared) = prepared.into_prepared() {
                            let continuation = self.realize_wiring_detached(*prepared);
                            let Some(entry) = self.roster.read().await.get(&run.identity).cloned()
                            else {
                                self.roll_back_member_wake(run, rollback);
                                return;
                            };
                            self.dispatch_member_wake_step(
                                run,
                                MemberFence::exact(&entry),
                                rollback,
                                "member_wake_topology",
                                async move {
                                    let result = continuation.settled().await;
                                    WakeStep::Topology {
                                        topology,
                                        peer,
                                        edge: Box::new(edge),
                                        result,
                                    }
                                },
                            );
                            return;
                        }
                        // Settled inside its preparation: the edge is realized.
                        self.resolve_member_edge_realization(&edge, true);
                    }
                    Err(error) => {
                        tracing::warn!(
                            mob_id = %self.definition.id,
                            agent_identity = %run.identity,
                            peer = %peer,
                            %error,
                            "a woken member's wiring edge could not be prepared; its repair is owed"
                        );
                        topology.failed.push(peer);
                        self.resolve_member_edge_realization(&edge, false);
                    }
                }
            }
            if topology.remaining.is_empty() {
                break;
            }
            if topology.yield_due {
                // Between two chunks the turn returns to the actor, which
                // serves what waits for it before the next chunk.
                topology.yield_due = false;
                let Some(entry) = self.roster.read().await.get(&run.identity).cloned() else {
                    self.roll_back_member_wake(run, rollback);
                    return;
                };
                #[cfg(test)]
                let hold = take_wake_topology_yield_hold_for_test(&run.identity);
                self.dispatch_member_wake_step(
                    run,
                    MemberFence::exact(&entry),
                    rollback,
                    "member_wake_topology",
                    async move {
                        #[cfg(test)]
                        if let Some((entered, release)) = hold {
                            let _ = entered.send(());
                            let _ = release.await;
                        }
                        WakeStep::TopologyYield { topology }
                    },
                );
                return;
            }
            let take = topology.chunk.min(topology.remaining.len());
            let chunk = topology.remaining.drain(..take).collect::<Vec<_>>();
            topology.single = Box::pin(self.realize_wake_topology_chunk(
                &run.identity,
                chunk,
                &mut topology.failed,
            ))
            .await;
            topology.yield_due = true;
        }
        if let Err(error) =
            self.resolve_respawn_topology_restore_result(&run.identity, topology.failed)
        {
            self.roll_back_member_wake_provision(&run, rollback);
            self.abandon_member_wake(
                &run,
                mob_dsl::MemberWakeAbandonKind::TopologyFailed,
                Some(format!("the woken member's topology failed: {error}")),
            )
            .await;
            return;
        }
        self.resolve_member_wake_ready(run, rollback).await;
    }

    /// Waking -> Live, after readiness and topology settled (Live means
    /// ready).
    async fn resolve_member_wake_ready(&mut self, run: WakeRun, rollback: Option<WakeRollback>) {
        let verdict = self.apply_member_wake_feedback(
            &run.identity,
            run.attempt,
            mob_dsl::MobMachineSignal::ResolveMemberRevivalSucceeded {
                agent_identity: mob_dsl::AgentIdentity::from_domain(&run.identity),
                attempt: run.attempt,
            },
            "member_wake_ready",
        );
        match verdict {
            mob_dsl::MemberWakeFeedbackVerdict::Applied => {
                self.member_wakes.running.remove(&run.attempt);
                self.clear_member_restore_failure(&run.identity).await;
                self.member_wakes
                    .answer(run.attempt, Ok(MemberWakeOutcome::Ready));
                self.report_member_built_after_wake(&run.identity).await;
                self.schedule_owed_member_wake_retry(&run.identity);
                tracing::debug!(
                    mob_id = %self.definition.id,
                    agent_identity = %run.identity,
                    attempt = run.attempt,
                    cause = ?run.cause,
                    "member woken"
                );
            }
            mob_dsl::MemberWakeFeedbackVerdict::StaleAttempt => {
                // The attempt ended while it settled (a stop): the machine
                // does not hold this materialization, so it is rolled back.
                self.roll_back_member_wake(run, rollback);
            }
        }
    }

    /// Report a woken member's build to the host's preparer (off the actor).
    async fn report_member_built_after_wake(&mut self, identity: &AgentIdentity) {
        let Some(session_id) = self
            .roster
            .read()
            .await
            .get(identity)
            .and_then(|entry| entry.member_ref.bridge_session_id().cloned())
        else {
            return;
        };
        self.report_member_built_to_host(identity, session_id);
    }

    /// Report a member Core built live on `session_id` (a wake's, or a
    /// spawn's once its activation committed) to the host's preparer, off
    /// the actor. Nothing happens without a preparer.
    pub(in crate::runtime) fn report_member_built_to_host(
        &mut self,
        identity: &AgentIdentity,
        session_id: SessionId,
    ) {
        let Some(preparer) = self.member_residency.build_preparer.clone() else {
            return;
        };
        let session_service = Arc::clone(&self.session_service);
        self.actor_io_tasks
            .spawn(super::super::member_build_preparer::report_member_built(
                Some(preparer),
                session_service,
                identity.clone(),
                session_id,
            ));
    }

    /// Resolve `run`'s attempt Broken for `error` (the recorded reason keeps a
    /// host refusal typed), roll back its provision, answer its callers.
    async fn fail_member_wake(
        &mut self,
        run: &WakeRun,
        rollback: Option<WakeRollback>,
        error: &MobError,
    ) {
        self.roll_back_member_wake_provision(run, rollback);
        let session_id = self
            .roster
            .read()
            .await
            .get(&run.identity)
            .and_then(|entry| entry.member_ref.bridge_session_id().cloned());
        let diagnostic = super::super::handle::RestoreFailureDiagnostic {
            bridge_session_id: session_id,
            reason: error.to_string(),
            hold: error.durable_resume_hold(),
            registration_refusal: match error {
                MobError::MemberRegistrationRefused { call, .. } => Some(*call),
                _ => None,
            },
        };
        self.fail_member_wake_with(run, diagnostic).await;
    }

    /// End `run`'s attempt retryable for a restore error that clears on its
    /// own ([`restore_error_is_retryable`]): nothing was provisioned, the
    /// member is Dormant again with its boot obligation owed (its retry joins
    /// the work still in flight), and its callers get the typed retryable
    /// refusal. An explicit resume that waits on the member takes that as
    /// the member's own outcome.
    async fn end_member_wake_retryable(&mut self, run: &WakeRun, error: MobError) {
        tracing::info!(
            mob_id = %self.definition.id,
            agent_identity = %run.identity,
            attempt = run.attempt,
            %error,
            "the member's restore waits on work in flight or an unavailable store; its wake is retried"
        );
        let cause = RetryableRestoreCause::RestoreUnavailable;
        self.member_wakes.running.remove(&run.attempt);
        if self
            .resolve_member_wake_retryable(
                &run.identity,
                run.attempt,
                cause.wake_kind(),
                cause.wake_refusal(),
                "member_wake_restore_unavailable",
            )
            .await
            == mob_dsl::MemberWakeFeedbackVerdict::StaleAttempt
        {
            self.finish_closed_member_wake(&run.identity, run.attempt)
                .await;
        }
    }

    /// Resolve `run`'s attempt Broken with `diagnostic` and answer its
    /// callers with the Broken answer.
    async fn fail_member_wake_with(
        &mut self,
        run: &WakeRun,
        diagnostic: super::super::handle::RestoreFailureDiagnostic,
    ) {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&run.identity);
        let signal = match diagnostic.registration_refusal {
            Some(call) => mob_dsl::MobMachineSignal::ResolveMemberRevivalHostRefused {
                agent_identity: dsl_identity,
                refusal_code: call.as_code().to_string(),
                reason: diagnostic.reason.clone(),
                attempt: run.attempt,
            },
            None => mob_dsl::MobMachineSignal::ResolveMemberRevivalFailed {
                agent_identity: dsl_identity,
                reason: diagnostic.reason.clone(),
                attempt: run.attempt,
            },
        };
        let verdict = self.apply_member_wake_feedback(
            &run.identity,
            run.attempt,
            signal,
            "member_wake_failed",
        );
        self.member_wakes.running.remove(&run.attempt);
        match verdict {
            mob_dsl::MemberWakeFeedbackVerdict::Applied => {
                self.record_member_restore_failure(&run.identity, diagnostic)
                    .await;
                let failure = self.member_wake_broken(&run.identity).await;
                self.member_wakes.answer(run.attempt, Err(failure));
            }
            mob_dsl::MemberWakeFeedbackVerdict::StaleAttempt => {
                self.finish_closed_member_wake(&run.identity, run.attempt)
                    .await;
            }
        }
    }

    /// End `run`'s attempt without a Broken verdict of its own
    /// (`AbandonMemberWake`): NotCurrent, ReloadRefused and Unsettled leave
    /// the member Dormant; ReadinessFailed and TopologyFailed leave it Broken
    /// with `reason`.
    async fn abandon_member_wake(
        &mut self,
        run: &WakeRun,
        kind: mob_dsl::MemberWakeAbandonKind,
        reason: Option<String>,
    ) {
        let reason_text = reason.clone().unwrap_or_default();
        let verdict = self.apply_member_wake_feedback(
            &run.identity,
            run.attempt,
            mob_dsl::MobMachineSignal::AbandonMemberWake {
                agent_identity: mob_dsl::AgentIdentity::from_domain(&run.identity),
                attempt: run.attempt,
                kind,
                reason: reason_text.clone(),
            },
            "member_wake_abandoned",
        );
        self.member_wakes.running.remove(&run.attempt);
        if verdict == mob_dsl::MemberWakeFeedbackVerdict::StaleAttempt {
            self.finish_closed_member_wake(&run.identity, run.attempt)
                .await;
            return;
        }
        let broken = matches!(
            kind,
            mob_dsl::MemberWakeAbandonKind::ReadinessFailed
                | mob_dsl::MemberWakeAbandonKind::TopologyFailed
        );
        if broken {
            let session_id = self
                .roster
                .read()
                .await
                .get(&run.identity)
                .and_then(|entry| entry.member_ref.bridge_session_id().cloned());
            self.record_member_restore_failure(
                &run.identity,
                super::super::handle::RestoreFailureDiagnostic {
                    bridge_session_id: session_id,
                    reason: reason_text,
                    hold: None,
                    registration_refusal: None,
                },
            )
            .await;
            let failure = self.member_wake_broken(&run.identity).await;
            self.member_wakes.answer(run.attempt, Err(failure));
        } else {
            let refusal = match kind {
                mob_dsl::MemberWakeAbandonKind::ReloadRefused => {
                    crate::MemberWakeRefusal::ResumeGateUnavailable
                }
                mob_dsl::MemberWakeAbandonKind::Unsettled => {
                    crate::MemberWakeRefusal::MobUnavailable
                }
                _ => crate::MemberWakeRefusal::WakeTimedOut,
            };
            self.member_wakes
                .answer(run.attempt, Err(MemberWakeFailure::Refused(refusal)));
            self.schedule_owed_member_wake_retry(&run.identity);
        }
    }

    /// Roll back the provision a wake made (off the actor), then finish the
    /// attempt by the state the rollback leaves: NotCurrent (Dormant) while
    /// it is still open. A seat session the wake created is kept rather than
    /// archived ([`Self::wake_keeps_created_seat_session`]).
    fn roll_back_member_wake(&mut self, run: WakeRun, rollback: Option<WakeRollback>) {
        let Some(rollback) = rollback else {
            let identity = run.identity.clone();
            let attempt = run.attempt;
            // Nothing provisioned: finish at the next commit point.
            self.member_wakes.running.remove(&attempt);
            self.member_wakes.closed_answer_later(identity, attempt);
            return;
        };
        let fence = match self.member_wake_fence_or_absent(&run.identity) {
            Some(fence) => fence,
            None => MemberFence::ExpectedAbsent(run.identity.clone()),
        };
        if self.wake_keeps_created_seat_session(&run.identity, &rollback) {
            let provisioner = Arc::clone(&self.member_wakes.provisioner);
            let mob_id = self.definition.id.clone();
            let identity = run.identity.clone();
            self.dispatch_member_wake_step(
                run,
                fence,
                None,
                "member_wake_keep_seat_session",
                async move {
                    match release_kept_seat_session(&provisioner, &rollback.member_ref).await {
                        Ok(()) => WakeStep::RolledBack { settled: true },
                        Err(error) => {
                            tracing::error!(
                                mob_id = %mob_id,
                                agent_identity = %identity,
                                %error,
                                "a kept seat session's live materialization could not be released"
                            );
                            WakeStep::RolledBack { settled: false }
                        }
                    }
                },
            );
            return;
        }
        // The guard is built in the step that rolls it back (as a warm
        // revival's committed rollback builds it): a teardown that aborts the
        // step before it ran drops no guard, and one that aborts it during
        // the rollback drops a guard whose rollback is in flight.
        let identity = run.identity.clone();
        let provisioner = Arc::clone(&self.provisioner);
        self.dispatch_member_wake_step(run, fence, None, "member_wake_rollback", async move {
            let provision = rollback.pending(&identity, provisioner);
            match provision.rollback_retaining_custody().await {
                Ok(()) => WakeStep::RolledBack { settled: true },
                Err(retained) => {
                    let _ = retained.abandon("member wake rollback could not settle");
                    WakeStep::RolledBack { settled: false }
                }
            }
        });
    }

    /// Roll back a wake's provision without waiting for it (the caller
    /// resolves the attempt itself).
    fn roll_back_member_wake_provision(&mut self, run: &WakeRun, rollback: Option<WakeRollback>) {
        let Some(rollback) = rollback else {
            return;
        };
        let provisioner = Arc::clone(&self.provisioner);
        let identity = run.identity.clone();
        let mob_id = self.definition.id.clone();
        // Built in the task, for the reason `roll_back_member_wake` gives.
        self.actor_io_tasks.spawn(async move {
            let provision = rollback.pending(&identity, provisioner);
            if let Err(retained) = provision.rollback_retaining_custody().await {
                let _ = retained.abandon("member wake rollback could not settle");
                tracing::error!(
                    mob_id = %mob_id,
                    agent_identity = %identity,
                    "a failed member wake could not roll back its provision"
                );
            }
        });
    }

    /// Whether a wake that ends retryable keeps the session it created
    /// instead of archiving it: its provision created the session the
    /// member's seat named (before or after the born row was journaled), the
    /// member is still seated and not retiring, and the service keeps durable
    /// sessions. Only the session's live materialization is released then.
    /// The next wake adopts the session while it is unborn (as one a crash
    /// left created), or revives it as any born member's once its born row
    /// stands; it is never created again, and never read as lost. A wake
    /// that ends Broken rolls its provision back as before, and so does one
    /// of a member that is leaving.
    fn wake_keeps_created_seat_session(
        &self,
        identity: &AgentIdentity,
        rollback: &WakeRollback,
    ) -> bool {
        let state = self.dsl_authority.state();
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        rollback.seat_session
            && self.session_service.supports_persistent_sessions()
            && state
                .identity_to_runtime
                .get(&dsl_identity)
                .is_some_and(|runtime_id| {
                    state.member_state_markers.get(runtime_id)
                        != Some(&mob_dsl::MobMemberState::Retiring)
                })
    }

    /// The exact fence of `identity`'s current incarnation, if it is seated.
    fn member_wake_fence_or_absent(&self, identity: &AgentIdentity) -> Option<MemberFence> {
        self.roster
            .try_read()
            .ok()
            .and_then(|roster| roster.get(identity).cloned())
            .map(|entry| MemberFence::exact(&entry))
    }
}

/// Release the live materialization of a seat session a retryable wake
/// keeps: this provisioner's own live attachment of it is retired (its
/// actor discarded, its registration closed), as the explicit resume retires
/// a live actor it does not reuse, and the run-start hold its build
/// preparation staged is removed (the next wake prepares the build again).
/// No rebuild follows, so the inputs queued on that registration end as
/// stopped rather than requeued: the member is Dormant until its next wake,
/// and nothing would run them before it.
/// The session itself stays, for the next wake. Nothing retired is not a
/// release: the wake's own provision left that attachment live.
async fn release_kept_seat_session(
    provisioner: &super::super::provisioner::MultiBackendProvisioner,
    member_ref: &MemberRef,
) -> Result<(), MobError> {
    let session_id = member_ref.bridge_session_id().ok_or_else(|| {
        MobError::Internal("a kept seat session's provision names no session".to_string())
    })?;
    let retired = provisioner
        .release_live_member_session_without_rebuild(
            session_id,
            Instant::now() + super::super::provisioner::EXPLICIT_RESUME_RETIRE_TOTAL_TIMEOUT,
        )
        .await?;
    if !retired {
        return Err(MobError::Internal(format!(
            "the live attachment of kept seat session '{session_id}' was not retired"
        )));
    }
    if let Err(error) = provisioner
        .unstage_member_registration_run_start_hold(
            member_ref,
            meerkat_runtime::RunStartHoldReason::ToolsNotPublished,
        )
        .await
    {
        tracing::warn!(
            session_id = %session_id,
            %error,
            "a kept seat session's staged run-start hold could not be removed"
        );
    }
    Ok(())
}

/// Step B: plan the member's restore off the actor.
async fn plan_member_wake(input: WakeRestoreInput) -> Result<WakePlan, MobError> {
    let WakeRestoreInput {
        definition,
        session_service,
        provisioner,
        provisioner_dyn,
        continuity,
        spawn_member_customizer,
        tool_bundles,
        tool_handle,
        default_external_tools_provider,
        realm_profile_store,
        default_llm_client,
        tool_consequence_policy_registry,
        member_build_preparer,
        host_read_handle,
        candidate,
        base,
        fork_rule,
        retained,
        followed,
        durable_endpoint,
        member_bound_sessions,
    } = input;
    let bridge_session_id = SessionId::parse(&candidate.machine_session_id.0).map_err(|error| {
        MobError::Internal(format!(
            "member wake found invalid session binding '{}' for '{}': {error}",
            candidate.machine_session_id.0, candidate.entry.agent_identity
        ))
    })?;
    // This member's own liveness observation (the wake path's drift
    // observation): a session actor still live in this process is kept.
    let mut active_ids = HashSet::new();
    if session_service
        .live_session_actor_registered(&bridge_session_id)
        .await
        .unwrap_or(false)
    {
        active_ids.insert(bridge_session_id.clone());
    }
    let ctx = ResumeMemberRestoreContext {
        definition: &definition,
        session_service: &session_service,
        provisioner: &provisioner,
        active_ids: &active_ids,
        recorded_continuity: &continuity,
        spawn_member_customizer: &spawn_member_customizer,
        tool_bundles: &tool_bundles,
        tool_handle: &tool_handle,
        default_external_tools_provider: &default_external_tools_provider,
        realm_profile_store: realm_profile_store.as_ref(),
        default_llm_client: default_llm_client.as_ref(),
        tool_consequence_policy_registry: tool_consequence_policy_registry.as_ref(),
        member_build_preparer: member_build_preparer.as_ref(),
        host_read_handle: &host_read_handle,
        // A view the store cannot serve now ends this wake retryable.
        unreadable_view_is_retryable: true,
    };
    // The overlay the followed source holds in this process, or the one its
    // rebuild would compose, computed here, off the actor, without waking it.
    // A source whose rebuild would fail leaves the fork to its own rule,
    // warned, as when its source is not rebuilt beside it.
    let mut overlays = BTreeMap::new();
    if let Some((source, followed)) = followed {
        match followed {
            super::super::fork_build::FollowedSourceOverlay::Retained(overlay) => {
                overlays.insert(source, overlay);
            }
            super::super::fork_build::FollowedSourceOverlay::Rebuild {
                entry: source_entry,
                session_id,
                seat_build,
            } => match super::super::fork_build::rebuilt_own_overlay(
                &definition.id,
                session_service.as_ref(),
                spawn_member_customizer.as_ref(),
                &source_entry,
                &session_id,
                seat_build.as_deref(),
            )
            .await
            {
                Ok(overlay) => {
                    overlays.insert(source, overlay);
                }
                Err(error) => tracing::warn!(
                    mob_id = %definition.id,
                    agent_identity = %candidate.entry.agent_identity,
                    source = %source,
                    overlay_member = %source_entry.agent_identity,
                    %error,
                    "the overlay a woken fork follows could not be computed from its source's rebuild"
                ),
            },
        }
    }
    let plan = restore_resumed_member(
        &ctx,
        &candidate,
        &base,
        fork_rule,
        &overlays,
        retained.as_ref(),
    )
    .await?;
    let entry = &candidate.entry;
    let (seating, missing, mut seated) = match plan {
        MemberRestorePlan::Seated {
            seating,
            outcome:
                SeatedMemberRestoreOutcome::SuccessorNeeded {
                    bridge_session_id,
                    seated,
                },
        } => (seating, bridge_session_id, *seated),
        plan => {
            return match check_woken_member_endpoint(
                &provisioner,
                &definition,
                entry,
                &plan,
                durable_endpoint.as_ref(),
            )
            .await
            {
                Ok(endpoint) => Ok(WakePlan::Restore { plan, endpoint }),
                Err(reason) => {
                    if let Some(rollback) = WakeRollback::from_plan(&plan) {
                        let provision = rollback.pending(&entry.agent_identity, provisioner_dyn);
                        if let Err(retained) = provision.rollback_retaining_custody().await {
                            let _ = retained.abandon("member wake endpoint mismatch rollback");
                        }
                    }
                    Ok(WakePlan::EndpointMismatch {
                        bridge_session_id,
                        reason,
                    })
                }
            };
        }
    };
    // The bound session is absent: its latest persisted successor, found
    // the way a boot finds it, provisioned on a fork repointed to it.
    let listed = session_service
        .list(meerkat_core::service::SessionQuery::default())
        .await?;
    let index = PersistedMemberSessionIndex::scan(
        session_service.as_ref(),
        &listed,
        &member_bound_sessions,
        || {},
    )
    .await?;
    let Some((successor, authorized)) = index
        .latest_for_member(
            session_service.as_ref(),
            &missing,
            &definition.id,
            &entry.role,
            &entry.agent_identity,
            seated.restore_resume_from_role.as_ref(),
        )
        .await?
    else {
        return Ok(WakePlan::NoSuccessor {
            bridge_session_id: missing,
        });
    };
    let rebuilt = provisioner
        .retire_exact_attachment_for_explicit_resume(&successor)
        .await?;
    let successor_live = !rebuilt
        && session_service
            .live_session_actor_registered(&successor)
            .await
            .unwrap_or(false);
    if successor_live {
        // A successor live in this process keeps serving when it runs the
        // binding the member's spec names (otherwise the wake fails, typed,
        // and the live actor is retired, not reused): repoint and bind it,
        // nothing is provisioned.
        if let Some(requested) = seated.restore_spec.auth_binding.as_ref()
            && let Err(conflict) = super::super::live_actor_binding::verify_live_actor_auth_binding(
                session_service.as_ref(),
                &entry.agent_identity,
                &successor,
                requested,
            )
            .await
        {
            let reason = super::super::live_actor_binding::refuse_conflicting_live_actor(
                session_service.as_ref(),
                &successor,
                &conflict,
            )
            .await;
            return Ok(WakePlan::Restore {
                plan: MemberRestorePlan::Unseated(member_restore_failure(&successor, reason)),
                endpoint: WakeEndpoint::Matches,
            });
        }
        let endpoint = live_member_endpoint(&provisioner, &definition, entry, &successor).await;
        return Ok(WakePlan::Successor {
            successor_session_id: successor,
            plan: MemberRestorePlan::Kept,
            endpoint,
        });
    }
    if entry.fork_source.is_none() && spawn_member_customizer.is_some() {
        let view = match super::super::fork_build::load_resumed_member_view(
            session_service.as_ref(),
            &successor,
        )
        .await
        {
            Ok(view) => view,
            // A read the store cannot serve now ends the wake retryable, as
            // the bound session's view read does.
            Err(error) if restore_error_is_retryable(&error) => return Err(error),
            Err(error) => {
                return Ok(WakePlan::Restore {
                    plan: MemberRestorePlan::Unseated(member_restore_failure(
                        &successor,
                        error.to_string(),
                    )),
                    endpoint: WakeEndpoint::Matches,
                });
            }
        };
        seated.restore_spec.resume_provider_params =
            match super::super::fork_build::successor_resume_provider_params(
                &definition.id,
                spawn_member_customizer.as_ref(),
                entry,
                &view,
            ) {
                Ok(params) => params,
                Err(error) => {
                    return Ok(WakePlan::Restore {
                        plan: MemberRestorePlan::Unseated(member_restore_failure(
                            &successor,
                            error.to_string(),
                        )),
                        endpoint: WakeEndpoint::Matches,
                    });
                }
            };
    }
    let mut repointed = base.fork();
    let dsl_identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
    let replacing = repointed
        .state()
        .member_session_bindings
        .get(&dsl_identity)
        .cloned();
    repointed
        .apply_signal(mob_dsl::MobMachineSignal::RecoverMemberSessionBinding {
            agent_identity: dsl_identity,
            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id),
            bridge_session_id: mob_dsl::SessionId::from_domain(&successor),
            replacing,
        })
        .map_err(|error| {
            MobError::Internal(format!(
                "member wake could not repoint '{}' to its successor on its own fork: {error}",
                entry.agent_identity
            ))
        })?;
    let provision =
        provision_durable_member_restore(&ctx, entry, &seated, &successor, authorized, &repointed)
            .await?;
    let plan = MemberRestorePlan::Seated {
        seating,
        outcome: SeatedMemberRestoreOutcome::Provision(Box::new(provision)),
    };
    // The successor's endpoint is recorded after the repoint, as a boot
    // records a repointed member's endpoint.
    let endpoint = match plan_member_ref(&plan) {
        Some(member_ref) => {
            live_member_endpoint_for_ref(&provisioner, &definition, entry, &member_ref).await
        }
        None => WakeEndpoint::Matches,
    };
    Ok(WakePlan::Successor {
        successor_session_id: successor,
        plan,
        endpoint,
    })
}

/// The member ref a plan provisioned.
fn plan_member_ref(plan: &MemberRestorePlan) -> Option<MemberRef> {
    WakeRollback::from_plan(plan).map(|rollback| rollback.member_ref)
}

/// Check the woken comms runtime's endpoint against the durable one: a
/// changed endpoint is refused (the member's peers trust its durable key); a
/// missing durable one is recorded after the plan applied.
async fn check_woken_member_endpoint(
    provisioner: &super::super::provisioner::MultiBackendProvisioner,
    definition: &MobDefinition,
    entry: &RosterEntry,
    plan: &MemberRestorePlan,
    durable: Option<&mob_dsl::MemberPeerEndpoint>,
) -> Result<WakeEndpoint, String> {
    let Some(member_ref) = plan_member_ref(plan) else {
        return Ok(WakeEndpoint::Matches);
    };
    let WakeEndpoint::Record(spec) =
        live_member_endpoint_for_ref(provisioner, definition, entry, &member_ref).await
    else {
        return Ok(WakeEndpoint::Matches);
    };
    let candidate = mob_dsl::MemberPeerEndpoint::from(&spec);
    match durable {
        // The same typed reason a boot or a resume records for a changed
        // endpoint: the mismatch and its repair.
        Some(durable) if durable != &candidate => Err(member_endpoint_broken_reason(
            &entry.agent_identity,
            &member_endpoint_mismatch_detail(&candidate, durable),
        )),
        Some(_) => Ok(WakeEndpoint::Matches),
        None => Ok(WakeEndpoint::Record(spec)),
    }
}

async fn live_member_endpoint(
    provisioner: &super::super::provisioner::MultiBackendProvisioner,
    definition: &MobDefinition,
    entry: &RosterEntry,
    session_id: &SessionId,
) -> WakeEndpoint {
    let member_ref = MemberRef::from_bridge_session_id(session_id.clone());
    live_member_endpoint_for_ref(provisioner, definition, entry, &member_ref).await
}

/// The endpoint the member's live comms runtime presents, as a boot reads
/// it; `Matches` when there is none to read.
async fn live_member_endpoint_for_ref(
    provisioner: &super::super::provisioner::MultiBackendProvisioner,
    definition: &MobDefinition,
    entry: &RosterEntry,
    member_ref: &MemberRef,
) -> WakeEndpoint {
    let Some(comms) = provisioner.comms_runtime(member_ref).await else {
        return WakeEndpoint::Matches;
    };
    let Some(key) = comms.public_key() else {
        return WakeEndpoint::Matches;
    };
    let Ok(name) = render_member_comms_name(
        definition.id.as_str(),
        entry.role.as_str(),
        entry.agent_identity.as_str(),
    ) else {
        return WakeEndpoint::Matches;
    };
    match provisioner.trusted_peer_spec(member_ref, &name, &key).await {
        Ok(spec) => WakeEndpoint::Record(spec),
        Err(_) => WakeEndpoint::Matches,
    }
}

/// Test seam: members whose wake readiness waits until the test releases
/// it, so a row can act between a wake's commit and its readiness.
#[cfg(test)]
static WAKE_READINESS_HOLDS_FOR_TEST: std::sync::Mutex<
    Vec<(AgentIdentity, crate::tokio::sync::watch::Receiver<bool>)>,
> = std::sync::Mutex::new(Vec::new());

/// Hold the readiness of every wake of `identity` until the returned sender
/// sends `true`.
#[cfg(test)]
pub(in crate::runtime) fn hold_wake_readiness_for_test(
    identity: &AgentIdentity,
) -> crate::tokio::sync::watch::Sender<bool> {
    let (released_tx, released_rx) = crate::tokio::sync::watch::channel(false);
    WAKE_READINESS_HOLDS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((identity.clone(), released_rx));
    released_tx
}

#[cfg(test)]
async fn wait_for_wake_readiness_release_for_test(identity: &AgentIdentity) {
    let held = WAKE_READINESS_HOLDS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(armed, _)| armed == identity)
        .map(|(_, released)| released.clone());
    if let Some(mut released) = held {
        let _ = released.wait_for(|released| *released).await;
    }
}

/// Test seam: members whose wake topology holds at its next pause between two
/// chunks until the test releases it.
#[cfg(test)]
static WAKE_TOPOLOGY_YIELD_HOLDS_FOR_TEST: std::sync::Mutex<
    Vec<(
        AgentIdentity,
        crate::tokio::sync::oneshot::Sender<()>,
        crate::tokio::sync::oneshot::Receiver<()>,
    )>,
> = std::sync::Mutex::new(Vec::new());

/// A held pause of a wake topology: `entered` fires once the pause is
/// reached; sending on `release` (or dropping it) lets the topology go on.
#[cfg(test)]
pub(in crate::runtime) struct WakeTopologyYieldHold {
    pub(in crate::runtime) entered: crate::tokio::sync::oneshot::Receiver<()>,
    pub(in crate::runtime) release: crate::tokio::sync::oneshot::Sender<()>,
}

/// Hold the next pause between two chunks of `identity`'s wake topology,
/// consumed once.
#[cfg(test)]
pub(in crate::runtime) fn hold_next_wake_topology_yield_for_test(
    identity: &AgentIdentity,
) -> WakeTopologyYieldHold {
    let (entered_tx, entered_rx) = crate::tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = crate::tokio::sync::oneshot::channel();
    WAKE_TOPOLOGY_YIELD_HOLDS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((identity.clone(), entered_tx, release_rx));
    WakeTopologyYieldHold {
        entered: entered_rx,
        release: release_tx,
    }
}

#[cfg(test)]
fn take_wake_topology_yield_hold_for_test(
    identity: &AgentIdentity,
) -> Option<(
    crate::tokio::sync::oneshot::Sender<()>,
    crate::tokio::sync::oneshot::Receiver<()>,
)> {
    let mut holds = WAKE_TOPOLOGY_YIELD_HOLDS_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let index = holds.iter().position(|(armed, ..)| armed == identity)?;
    let (_, entered, release) = holds.remove(index);
    Some((entered, release))
}

/// Test seam: members whose next wake treats one wiring edge as failed (a
/// transient failure), consumed once per arm.
#[cfg(test)]
static WAKE_EDGE_FAILURES_FOR_TEST: std::sync::Mutex<Vec<AgentIdentity>> =
    std::sync::Mutex::new(Vec::new());

/// Make the next wake of `identity` fail one of its wiring edges.
#[cfg(test)]
pub(in crate::runtime) fn fail_next_wake_edge_for_test(identity: &AgentIdentity) {
    WAKE_EDGE_FAILURES_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(identity.clone());
}

#[cfg(test)]
pub(super) fn take_wake_edge_failure_for_test(identity: &AgentIdentity) -> bool {
    let mut failures = WAKE_EDGE_FAILURES_FOR_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match failures.iter().position(|armed| armed == identity) {
        Some(index) => {
            failures.remove(index);
            true
        }
        None => false,
    }
}
