//! Background retirement for implicit delegation mobs.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use meerkat_core::{AgentExecutionSnapshot, TurnPhase};
use meerkat_mob::{
    AgentIdentity, MobMemberKickoffPhase, MobMemberStatus, RetirementSettlement,
    RetirementSettlementWatch,
};
use meerkat_mob_mcp::MobMcpState;

use crate::mob_handle_runtime::{
    DELEGATE_IDLE_RETIRE_DISABLED_LABEL, DELEGATE_IDLE_RETIRE_SECS_LABEL,
    DelegateIdleRetireOverride, ImplicitDelegateRetirementOverrides, MobRuntime,
};
use crate::runtime::RuntimeOptions;

use super::UnifiedRuntime;

impl UnifiedRuntime {
    pub(crate) async fn configure_implicit_delegate_retirement(&self, options: &RuntimeOptions) {
        let Some(state) = self.mob_runtime.agent_mob_mcp_state() else {
            return;
        };
        let sweep_interval =
            Duration::from_millis(options.implicit_delegate_idle_sweep_interval_ms.max(1_000));
        let overrides = self.mob_runtime.implicit_delegate_retirement_overrides();
        // Re-arm the sweep for members restored across a restart: their
        // opt-ins live in the durable metadata store, not in the roster.
        if let Some(overrides) = overrides.as_ref() {
            let restored = overrides
                .attach_durable_store(Arc::clone(&self.persistent_metadata))
                .await;
            if restored > 0 {
                tracing::info!(restored, "restored idle-retire opt-ins for spawned members");
            }
        }
        // A candidate launch retires no member: the delegates it finds idle
        // are the authoritative launch's, whose sweep retires them.
        if !self.mob_runtime.speaks_for_composition() {
            tracing::info!(
                "candidate launch: the implicit delegate idle-retire sweep is not started"
            );
            return;
        }
        let task = tokio::spawn(run_implicit_delegate_retirement(
            self.mob_runtime.clone(),
            state,
            overrides,
            options
                .implicit_delegate_idle_retire_secs
                .map(Duration::from_secs),
            sweep_interval,
            Arc::clone(&self.implicit_delegate_identity_runtime),
        ));
        *self.implicit_delegate_retirement_task.lock().await = Some(task);
    }
}

async fn run_implicit_delegate_retirement(
    runtime: MobRuntime,
    state: Arc<MobMcpState>,
    per_delegate_overrides: Option<ImplicitDelegateRetirementOverrides>,
    default_idle_after: Option<Duration>,
    sweep_interval: Duration,
    identity_runtime: Arc<std::sync::RwLock<Option<Arc<crate::identity_first::IdentityRuntime>>>>,
) {
    let primary_mob_id = runtime.handle().mob_id().to_string();
    let session_service = state.session_service();
    // Each member's idle clock, with the session it was started on: a member
    // re-seated or re-woken under another session starts a fresh window.
    let mut idle_since: BTreeMap<(String, String), (meerkat_core::types::SessionId, Instant)> =
        BTreeMap::new();
    // A retirement that outlives `retire`'s wait stays owned by the mob
    // (meerkat 0.8.51): its settlement is awaited here, off the sweep, and
    // reported. Dropping the set with this task aborts the waits.
    let mut settlements = tokio::task::JoinSet::new();

    loop {
        tokio::time::sleep(sweep_interval).await;
        while settlements.try_join_next().is_some() {}
        let mut seen = BTreeSet::new();
        // Opt-ins for members of implicit delegation mobs, as held before
        // this pass looked at any roster. Their mobs are not on the primary
        // event stream, so a hand retire or a destroy there is settled here:
        // an opt-in recorded before the pass whose member the pass did not
        // find is released.
        let implicit_opt_ins_at_start = match per_delegate_overrides.as_ref() {
            Some(overrides) => overrides.bindings_outside(&primary_mob_id).await,
            None => Vec::new(),
        };
        let mob_handles = Box::pin(state.mob_handles_snapshot()).await.ok();
        let rosters_observed = mob_handles.is_some();
        for (mob_id, handle) in mob_handles.unwrap_or_default() {
            let is_primary_mob = mob_id.as_str() == primary_mob_id;
            let is_implicit_mob = Box::pin(state.is_implicit_mob(&mob_id)).await;
            if !is_primary_mob && !is_implicit_mob {
                continue;
            }
            let members = handle.list_members_observation_snapshot().await;
            #[cfg(test)]
            tests::pause_after_member_snapshot(mob_id.as_str(), &members).await;
            for member in members {
                let identity = member.agent_identity.to_string();
                let key = (mob_id.to_string(), identity.clone());
                seen.insert(key.clone());
                if member.status == MobMemberStatus::Retiring {
                    idle_since.remove(&key);
                    continue;
                }
                // An opt-in holds only for the member instance it was set
                // for; the overrides drop a stale binding (another member's)
                // and restore an opt-in released with the exact session the
                // member resumed onto, against the member's session now.
                let member_session = handle
                    .resolve_bridge_session_id(&member.agent_identity)
                    .await;
                let per_delegate_override = match per_delegate_overrides.as_ref() {
                    Some(overrides) => {
                        overrides
                            .reconcile_seated(mob_id.as_str(), &identity, member_session.as_ref())
                            .await
                    }
                    None => None,
                };
                if !idle_retirement_candidate(
                    is_primary_mob,
                    is_implicit_mob,
                    &member.labels,
                    per_delegate_override,
                ) {
                    idle_since.remove(&key);
                    continue;
                }
                let Some(idle_after) = delegate_member_idle_retire_after(
                    &member.labels,
                    per_delegate_override,
                    default_idle_after,
                ) else {
                    idle_since.remove(&key);
                    continue;
                };
                let Some(session_id) = member_session else {
                    idle_since.remove(&key);
                    continue;
                };
                let idle = match observe_member_execution(session_service.as_ref(), &session_id)
                    .await
                {
                    MemberExecution::Idle => true,
                    MemberExecution::Busy => false,
                    // Retiring a member ends the work it still owes, so a
                    // member no live session serves is idle only once it
                    // owes none: no kickoff in flight and no input of its
                    // session short of a terminal phase.
                    MemberExecution::NotLive(error) => {
                        let owes_input = tokio::time::timeout(
                            MEMBER_EXECUTION_OBSERVATION_TIMEOUT,
                            runtime.session_owes_durable_input(&session_id),
                        )
                        .await
                        .ok()
                        .flatten();
                        let owes = owes_work_while_not_live(
                            member.kickoff.as_ref().map(|kickoff| kickoff.phase),
                            owes_input,
                        );
                        if owes {
                            tracing::debug!(
                                mob_id = %mob_id,
                                agent_identity = %identity,
                                session_id = %session_id,
                                error = %error,
                                "implicit delegate idle sweep kept a member no live session serves: it still owes work"
                            );
                        }
                        !owes
                    }
                    MemberExecution::Unreadable(error) => {
                        tracing::debug!(
                            mob_id = %mob_id,
                            agent_identity = %identity,
                            session_id = %session_id,
                            error = %error,
                            "implicit delegate idle sweep skipped member after snapshot error"
                        );
                        false
                    }
                };
                if !idle {
                    idle_since.remove(&key);
                    continue;
                }
                // Retiring a member retires every member it spawned: meerkat
                // cascades along roster `spawned_by`, through members still in
                // the roster. So a member that still has a member it spawned
                // in the roster, retiring or not, is not idle: a fork it
                // started may still be running, and retiring it would kill
                // that fork with its outcome reaching nobody. A direct child
                // is enough to check, since the cascade reaches deeper members
                // only through a child still in the roster. No member status
                // is consulted: a stale observation (a child seen retiring,
                // then seated again under the same identity) cannot exempt a
                // live child.
                if handle
                    .list_all_members()
                    .await
                    .iter()
                    .any(|entry| entry.spawned_by.as_ref() == Some(&member.agent_identity))
                {
                    idle_since.remove(&key);
                    continue;
                }
                let since = match idle_since.get(&key) {
                    Some((clock_session, since)) if *clock_session == session_id => *since,
                    _ => {
                        let now = Instant::now();
                        idle_since.insert(key.clone(), (session_id.clone(), now));
                        now
                    }
                };
                if since.elapsed() < idle_after {
                    continue;
                }
                #[cfg(test)]
                tests::pause_before_retire(mob_id.as_str(), &member.agent_identity).await;
                // Retire is member-addressed: a member re-seated or re-woken
                // under another session since this pass read it is not the
                // member this clock timed.
                if handle
                    .resolve_bridge_session_id(&member.agent_identity)
                    .await
                    .as_ref()
                    != Some(&session_id)
                {
                    idle_since.remove(&key);
                    continue;
                }
                let public_alias =
                    crate::member_comms_id::runtime_alias_str(&identity).into_owned();
                let identity_runtime = identity_runtime
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                let retire_result = if is_primary_mob {
                    if let Some(identity_runtime) = identity_runtime.as_ref() {
                        if let Some(durable_identity) = identity_runtime
                            .identity_for_member_mutation(&public_alias)
                            .await
                        {
                            identity_runtime
                                .retire_member_alias_tracked(&durable_identity, &public_alias)
                                .await
                                .map_err(|error| error.to_string())
                        } else if crate::member_comms_id::is_reserved_generated_alias(&public_alias)
                        {
                            Err(format!(
                                "generated alias is not owned by IdentityRuntime: {public_alias}"
                            ))
                        } else {
                            handle
                                .retire(AgentIdentity::from(identity.as_str()))
                                .await
                                .map_err(|error| error.to_string())
                        }
                    } else if crate::member_comms_id::is_reserved_generated_alias(&public_alias) {
                        Err(format!(
                            "generated alias requires IdentityRuntime authority: {public_alias}"
                        ))
                    } else {
                        handle
                            .retire(AgentIdentity::from(identity.as_str()))
                            .await
                            .map_err(|error| error.to_string())
                    }
                } else if crate::member_comms_id::is_reserved_generated_alias(&public_alias) {
                    Err(format!(
                        "generated alias cannot be raw-retired in an implicit mob: {public_alias}"
                    ))
                } else {
                    handle
                        .retire(AgentIdentity::from(identity.as_str()))
                        .await
                        .map_err(|error| error.to_string())
                };
                match retire_result {
                    Ok(()) => {
                        // Exact on the session just retired; covers implicit
                        // mobs, whose events this runtime does not observe.
                        if let Some(overrides) = per_delegate_overrides.as_ref() {
                            overrides
                                .release_session(mob_id.as_str(), &identity, &session_id)
                                .await;
                        }
                        tracing::info!(
                            mob_id = %mob_id,
                            agent_identity = %identity,
                            idle_after_ms = idle_after.as_millis() as u64,
                            "retired idle spawned member"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            mob_id = %mob_id,
                            agent_identity = %identity,
                            error = %error,
                            "implicit delegate idle retirement did not complete"
                        );
                        // A retirement that durably started is owned by the
                        // mob until it settles: report how it settles instead
                        // of losing sight of a member that stays Retiring.
                        if let Some(watch) =
                            handle.retirement_settlement(&AgentIdentity::from(identity.as_str()))
                        {
                            settlements.spawn(report_idle_retirement_settlement(
                                mob_id.to_string(),
                                identity.clone(),
                                watch,
                            ));
                        }
                    }
                }
                idle_since.remove(&key);
            }
        }
        idle_since.retain(|key, _| seen.contains(key));
        if rosters_observed && let Some(overrides) = per_delegate_overrides.as_ref() {
            for ((mob_id, member_id), bound) in implicit_opt_ins_at_start {
                // A member being respawned can be missing from the roster for
                // a moment; its respawn carries the opt-in, so it is skipped.
                if !seen.contains(&(mob_id.clone(), member_id.clone()))
                    && !overrides.is_respawning(&mob_id, &member_id)
                {
                    overrides.clear(&mob_id, &member_id, &bound).await;
                }
            }
        }
    }
}

/// Await how an idle retirement that outlived `retire`'s wait settles, and
/// report it. A stuck retirement stays owned by the mob, so it is reported
/// with the stage it failed at and is re-driven with
/// `MobHandle::redrive_retirement` (or on mob resume); the sweep does not
/// retry it.
async fn report_idle_retirement_settlement(
    mob_id: String,
    identity: String,
    mut watch: RetirementSettlementWatch,
) {
    match watch.settled().await {
        Some(RetirementSettlement::Retired) => tracing::info!(
            mob_id = %mob_id,
            agent_identity = %identity,
            "idle spawned member retired after the retire call returned"
        ),
        Some(RetirementSettlement::Stuck { stage, cause }) => tracing::warn!(
            mob_id = %mob_id,
            agent_identity = %identity,
            stage = %stage.as_str(),
            cause = %cause,
            "idle spawned member retirement is stuck; it stays owned by the mob until \
             re-driven (MobHandle::redrive_retirement or a mob resume)"
        ),
        Some(RetirementSettlement::NotStarted { cause }) => tracing::warn!(
            mob_id = %mob_id,
            agent_identity = %identity,
            cause = %cause,
            "idle spawned member retirement did not start"
        ),
        // The actor stopped before the retirement settled.
        Some(_) | None => {}
    }
}

/// How long the sweep waits for one read of a member's execution. A session
/// answers its execution snapshot only between turns, so a member mid-turn
/// does not answer in time: it counts as busy, and the sweep moves on instead
/// of waiting for the turn to end.
const MEMBER_EXECUTION_OBSERVATION_TIMEOUT: Duration = Duration::from_millis(250);

/// A member's execution as the idle sweep sees it.
enum MemberExecution {
    Idle,
    /// Mid-turn: the runtime machine has a run open for the session, or the
    /// session did not answer within [`MEMBER_EXECUTION_OBSERVATION_TIMEOUT`].
    Busy,
    /// The execution snapshot failed, and the typed local reads say why: this
    /// process's durable store holds the session and no live actor serves it,
    /// as for a member a restart left without a live session. Idle unless it
    /// still owes work ([`owes_work_while_not_live`]).
    NotLive(meerkat_core::service::SessionError),
    Unreadable(meerkat_core::service::SessionError),
}

/// Read whether the member running `session_id` is idle, never waiting for a
/// running turn to end. The runtime machine the member runs on answers
/// without queueing behind the turn; a member it reports running is busy.
/// Otherwise the session's execution snapshot decides, bounded: a session
/// busy with a turn does not answer, which reads as busy too.
async fn observe_member_execution(
    session_service: &dyn meerkat_mob::MobSessionService,
    session_id: &meerkat_core::types::SessionId,
) -> MemberExecution {
    use meerkat_runtime::SessionServiceRuntimeExt as _;

    let runtime = match session_service.acquire_runtime_adapter(None) {
        Ok(runtime) => runtime,
        Err(error) => {
            return MemberExecution::Unreadable(
                crate::mob_handle_runtime::runtime_adapter_acquisition_session_error(error),
            );
        }
    };
    if let Some(runtime) = runtime
        && let Ok(Ok(meerkat_runtime::RuntimeState::Running)) = tokio::time::timeout(
            MEMBER_EXECUTION_OBSERVATION_TIMEOUT,
            runtime.runtime_state(session_id),
        )
        .await
    {
        return MemberExecution::Busy;
    }
    match tokio::time::timeout(
        MEMBER_EXECUTION_OBSERVATION_TIMEOUT,
        session_service.execution_snapshot(session_id),
    )
    .await
    {
        Ok(Ok(Some(snapshot))) if delegate_execution_is_idle(&snapshot) => MemberExecution::Idle,
        Ok(Ok(Some(_))) | Err(_) => MemberExecution::Busy,
        Ok(Ok(None)) => MemberExecution::Idle,
        Ok(Err(error)) => not_live_or_unreadable(session_service, session_id, error).await,
    }
}

/// A member whose execution snapshot failed, read with typed local reads and
/// never by the error's variant. Only a session this process's durable store
/// holds and no live actor serves is [`MemberExecution::NotLive`]. A session
/// the store does not hold (a placed member's session is resident on its
/// host, or the session is gone), a live session, and a read that fails or
/// does not answer in time stay [`MemberExecution::Unreadable`].
async fn not_live_or_unreadable(
    session_service: &dyn meerkat_mob::MobSessionService,
    session_id: &meerkat_core::types::SessionId,
    error: meerkat_core::service::SessionError,
) -> MemberExecution {
    let live = tokio::time::timeout(
        MEMBER_EXECUTION_OBSERVATION_TIMEOUT,
        session_service.live_session_actor_registered(session_id),
    )
    .await;
    if !matches!(live, Ok(Ok(false))) {
        return MemberExecution::Unreadable(error);
    }
    let persisted = tokio::time::timeout(
        MEMBER_EXECUTION_OBSERVATION_TIMEOUT,
        session_service.load_persisted_session_metadata(session_id),
    )
    .await;
    if matches!(persisted, Ok(Ok(Some(_)))) {
        MemberExecution::NotLive(error)
    } else {
        MemberExecution::Unreadable(error)
    }
}

/// Whether a member no live session serves still owes work: its kickoff has
/// not resolved, or its session holds an input short of a terminal phase. An
/// input read that did not answer owes too, never a guessed `false`.
fn owes_work_while_not_live(
    kickoff: Option<MobMemberKickoffPhase>,
    owes_durable_input: Option<bool>,
) -> bool {
    kickoff.is_some_and(kickoff_is_pending) || owes_durable_input != Some(false)
}

/// A kickoff that has not resolved: not yet started, starting, or waiting on
/// its callback. A phase this build does not know counts as pending.
fn kickoff_is_pending(phase: MobMemberKickoffPhase) -> bool {
    !matches!(
        phase,
        MobMemberKickoffPhase::Started
            | MobMemberKickoffPhase::Failed
            | MobMemberKickoffPhase::Cancelled
    )
}

fn delegate_execution_is_idle(snapshot: &AgentExecutionSnapshot) -> bool {
    turn_phase_is_idle(snapshot.turn_phase)
}

fn idle_retirement_candidate(
    is_primary_mob: bool,
    is_implicit_mob: bool,
    labels: &std::collections::BTreeMap<String, String>,
    per_delegate_override: Option<DelegateIdleRetireOverride>,
) -> bool {
    if is_implicit_mob {
        return true;
    }
    is_primary_mob
        && (per_delegate_override.is_some() || labels.contains_key(DELEGATE_IDLE_RETIRE_SECS_LABEL))
}

fn delegate_member_idle_retire_after(
    labels: &std::collections::BTreeMap<String, String>,
    per_delegate_override: Option<DelegateIdleRetireOverride>,
    default_idle_after: Option<Duration>,
) -> Option<Duration> {
    match per_delegate_override {
        Some(DelegateIdleRetireOverride::Disabled) => return None,
        Some(DelegateIdleRetireOverride::Seconds(seconds)) => {
            return Some(Duration::from_secs(seconds));
        }
        // Opted in without a number: the runtime default decides, and a
        // runtime with retirement disabled keeps it disabled.
        Some(DelegateIdleRetireOverride::RuntimeDefault) => return default_idle_after,
        None => {}
    }
    match labels
        .get(DELEGATE_IDLE_RETIRE_SECS_LABEL)
        .map(String::as_str)
    {
        Some(value) if value.eq_ignore_ascii_case(DELEGATE_IDLE_RETIRE_DISABLED_LABEL) => None,
        Some(value) => value
            .parse::<u64>()
            .ok()
            .map(Duration::from_secs)
            .or(default_idle_after),
        None => default_idle_after,
    }
}

pub(crate) fn turn_phase_is_idle(phase: TurnPhase) -> bool {
    // Meerkat 0.7 removed `TurnPhase::is_terminal`; idle means ready or any
    // terminal phase (completed/failed/cancelled, matching the old predicate).
    matches!(
        phase,
        TurnPhase::Ready | TurnPhase::Completed | TurnPhase::Failed | TurnPhase::Cancelled
    )
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn implicit_delegate_turn_phase_idle_classification() {
        assert!(turn_phase_is_idle(TurnPhase::Ready));
        assert!(turn_phase_is_idle(TurnPhase::Completed));
        assert!(turn_phase_is_idle(TurnPhase::Failed));
        assert!(turn_phase_is_idle(TurnPhase::Cancelled));

        assert!(!turn_phase_is_idle(TurnPhase::ApplyingPrimitive));
        assert!(!turn_phase_is_idle(TurnPhase::CallingLlm));
        assert!(!turn_phase_is_idle(TurnPhase::WaitingForOps));
        assert!(!turn_phase_is_idle(TurnPhase::DrainingBoundary));
        assert!(!turn_phase_is_idle(TurnPhase::Extracting));
        assert!(!turn_phase_is_idle(TurnPhase::ErrorRecovery));
        assert!(!turn_phase_is_idle(TurnPhase::Cancelling));
    }

    #[tokio::test]
    async fn runtime_acquisition_refusal_is_unreadable_before_idle_snapshot_fallback() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let reads = Arc::new(AtomicUsize::new(0));
        let service =
            crate::mob_handle_runtime::acquisition_observation_probe(true, Arc::clone(&reads));
        let session_id = meerkat_core::SessionId::new();
        assert!(matches!(
            observe_member_execution(service.as_ref(), &session_id).await,
            MemberExecution::Unreadable(meerkat_core::SessionError::RuntimeUnavailable {
                reason: meerkat_runtime::traits::ControllerReadinessFailure::AuthorityChanged,
            })
        ));
        assert_eq!(
            reads.load(Ordering::SeqCst),
            0,
            "a refused runtime owner must not fall back to an idle session snapshot"
        );
    }

    #[tokio::test]
    async fn absent_runtime_adapter_preserves_idle_snapshot_fallback() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let reads = Arc::new(AtomicUsize::new(0));
        let service =
            crate::mob_handle_runtime::acquisition_observation_probe(false, Arc::clone(&reads));
        let session_id = meerkat_core::SessionId::new();
        assert!(matches!(
            observe_member_execution(service.as_ref(), &session_id).await,
            MemberExecution::Idle
        ));
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "a service without a runtime owner must still consult its idle session snapshot"
        );
    }

    #[test]
    fn idle_retirement_candidates_require_primary_mob_opt_in() {
        let no_labels = std::collections::BTreeMap::new();
        let labeled = std::collections::BTreeMap::from([(
            DELEGATE_IDLE_RETIRE_SECS_LABEL.to_string(),
            "300".to_string(),
        )]);

        assert!(idle_retirement_candidate(false, true, &no_labels, None,));
        assert!(idle_retirement_candidate(true, false, &labeled, None,));
        assert!(idle_retirement_candidate(
            true,
            false,
            &no_labels,
            Some(DelegateIdleRetireOverride::Seconds(60)),
        ));
        assert!(idle_retirement_candidate(
            true,
            false,
            &no_labels,
            Some(DelegateIdleRetireOverride::RuntimeDefault),
        ));
        assert!(!idle_retirement_candidate(true, false, &no_labels, None,));
    }

    /// A fork child opted in on the runtime default follows that default
    /// exactly: the configured timeout when there is one, and no retirement
    /// when the runtime disabled it. A label never overrides the opt-in.
    #[test]
    fn runtime_default_override_follows_the_runtime_default() {
        let no_labels = std::collections::BTreeMap::new();
        assert_eq!(
            delegate_member_idle_retire_after(
                &no_labels,
                Some(DelegateIdleRetireOverride::RuntimeDefault),
                Some(Duration::from_mins(5))
            ),
            Some(Duration::from_mins(5))
        );
        assert_eq!(
            delegate_member_idle_retire_after(
                &no_labels,
                Some(DelegateIdleRetireOverride::RuntimeDefault),
                None
            ),
            None
        );
        let labels = std::collections::BTreeMap::from([(
            DELEGATE_IDLE_RETIRE_SECS_LABEL.to_string(),
            "12".to_string(),
        )]);
        assert_eq!(
            delegate_member_idle_retire_after(
                &labels,
                Some(DelegateIdleRetireOverride::RuntimeDefault),
                Some(Duration::from_mins(5))
            ),
            Some(Duration::from_mins(5))
        );
    }

    #[test]
    fn implicit_delegate_idle_retire_label_overrides_runtime_default() {
        let labels = std::collections::BTreeMap::from([(
            DELEGATE_IDLE_RETIRE_SECS_LABEL.to_string(),
            "12".to_string(),
        )]);

        assert_eq!(
            delegate_member_idle_retire_after(&labels, None, Some(Duration::from_mins(5))),
            Some(Duration::from_secs(12))
        );
    }

    #[test]
    fn implicit_delegate_idle_retire_label_can_disable_member_retirement() {
        let labels = std::collections::BTreeMap::from([(
            DELEGATE_IDLE_RETIRE_SECS_LABEL.to_string(),
            DELEGATE_IDLE_RETIRE_DISABLED_LABEL.to_string(),
        )]);

        assert_eq!(
            delegate_member_idle_retire_after(&labels, None, Some(Duration::from_mins(5))),
            None
        );
    }

    #[test]
    fn implicit_delegate_idle_retire_call_override_wins_over_label() {
        let labels = std::collections::BTreeMap::from([(
            DELEGATE_IDLE_RETIRE_SECS_LABEL.to_string(),
            "12".to_string(),
        )]);

        assert_eq!(
            delegate_member_idle_retire_after(
                &labels,
                Some(DelegateIdleRetireOverride::Seconds(9)),
                Some(Duration::from_mins(5))
            ),
            Some(Duration::from_secs(9))
        );
    }

    #[test]
    fn implicit_delegate_idle_retire_call_override_can_disable_retirement() {
        assert_eq!(
            delegate_member_idle_retire_after(
                &std::collections::BTreeMap::new(),
                Some(DelegateIdleRetireOverride::Disabled),
                Some(Duration::from_mins(5))
            ),
            None
        );
    }

    #[test]
    fn implicit_delegate_idle_retire_invalid_label_uses_runtime_default() {
        let labels = std::collections::BTreeMap::from([(
            DELEGATE_IDLE_RETIRE_SECS_LABEL.to_string(),
            "eventually".to_string(),
        )]);

        assert_eq!(
            delegate_member_idle_retire_after(&labels, None, Some(Duration::from_mins(5))),
            Some(Duration::from_mins(5))
        );
    }

    #[test]
    fn implicit_delegate_idle_retire_uses_runtime_default_when_unlabeled() {
        assert_eq!(
            delegate_member_idle_retire_after(
                &std::collections::BTreeMap::new(),
                None,
                Some(Duration::from_mins(5))
            ),
            Some(Duration::from_mins(5))
        );
    }

    #[tokio::test]
    async fn running_sweeper_observes_identity_authority_attached_after_runtime_bootstrap() {
        use crate::identity_first::{
            AgentAddressability, AgentIdentity as DurableIdentity, DurabilityPolicy,
            DurableAgentSpec, IdentityFirstRuntimeContext, IdentityLifecycleState, IdentityRuntime,
            IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge,
            RosterContext, RosterError, RosterProvider,
        };
        use crate::{
            DiscoverySpec, InMemoryMetadataStore, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
        };

        struct FixedRoster(Vec<DurableAgentSpec>);

        #[async_trait::async_trait]
        impl RosterProvider for FixedRoster {
            async fn roster(
                &self,
                _context: &RosterContext,
            ) -> Result<Vec<DurableAgentSpec>, RosterError> {
                Ok(self.0.clone())
            }
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "late-bound-retirement"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#,
        )
        .expect("mob definition");
        let mob_spec = MobBootstrapSpec::ephemeral(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            temp.path().to_path_buf(),
            4,
            None,
        )
        .expect("build the idle retirement fixture ephemeral spec")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let mut runtime = UnifiedRuntime::bootstrap_with_options(
            mob_spec,
            MetaConfig {
                modules: Vec::new(),
                discovery: DiscoverySpec {
                    namespace: "late-bound-retirement".to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            },
            Vec::new(),
            Duration::from_secs(2),
            RuntimeOptions {
                implicit_delegate_idle_retire_secs: Some(300),
                implicit_delegate_idle_sweep_interval_ms: 1_000,
                ..RuntimeOptions::default()
            },
            Arc::new(InMemoryMetadataStore::new()),
        )
        .await
        .expect("bootstrap runtime");
        assert!(runtime.identity_runtime().is_none());
        assert!(
            runtime
                .implicit_delegate_retirement_task
                .lock()
                .await
                .is_some(),
            "the sweeper must be running before identity authority attaches"
        );

        let identity = DurableIdentity::parse("domain:late-bound").expect("identity");
        // Seated at the roster row of its durable identity, as identity-first
        // materialization seats it: Core's journal binds the member there.
        let roster_member =
            crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let handle = runtime.mob_handle();
        handle
            .ensure_member(
                meerkat_mob::SpawnMemberSpec::new(
                    meerkat_mob::ProfileName::from("worker"),
                    roster_member.clone(),
                )
                .with_labels(BTreeMap::from([(
                    DELEGATE_IDLE_RETIRE_SECS_LABEL.to_string(),
                    "0".to_string(),
                )])),
            )
            .await
            .expect("spawn pre-attached identity member");

        let continuity_store =
            Arc::new(LocalContinuityStore::in_memory().expect("late-bound continuity store"));
        let lease_provider = Arc::new(LocalLeaseProvider::new());
        let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store,
            lease_provider,
            runtime_instance_id: "late-bound-retirement".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(MobSessionBridge::new(handle))),
            default_timeout: None,
        }));
        let spec = DurableAgentSpec {
            identity: identity.clone(),
            profile: meerkat_mob::ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        };
        identity_runtime.register(spec.clone()).await;
        assert_eq!(
            identity_runtime
                .status(&identity)
                .await
                .expect("status")
                .state,
            IdentityLifecycleState::Active,
            "Core holds the seated member before the sweeper runs"
        );
        runtime.attach_identity_first_context(Arc::new(IdentityFirstRuntimeContext::new(
            Arc::clone(&identity_runtime),
            Arc::new(FixedRoster(vec![spec])),
            None,
            None,
            Some(runtime.mob_handle().definition().clone()),
        )));

        // Retired through the identity authority: Core no longer holds the
        // member, so the identity reads Dormant.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if identity_runtime
                    .status(&identity)
                    .await
                    .is_ok_and(|status| status.state == IdentityLifecycleState::Dormant)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("late-bound sweeper never retired through identity authority");

        runtime.shutdown().await;
    }

    fn bound(
        policy: DelegateIdleRetireOverride,
        session_id: Option<&meerkat_core::types::SessionId>,
    ) -> crate::mob_handle_runtime::BoundIdleRetireOverride {
        crate::mob_handle_runtime::BoundIdleRetireOverride {
            policy,
            session_id: session_id.cloned(),
            recorded_at: chrono::Utc::now(),
        }
    }

    /// The policy and session an opt-in holds, for comparisons that do not
    /// care when it was recorded.
    fn view(
        bound: Option<crate::mob_handle_runtime::BoundIdleRetireOverride>,
    ) -> Option<(
        DelegateIdleRetireOverride,
        Option<meerkat_core::types::SessionId>,
    )> {
        bound.map(|bound| (bound.policy, bound.session_id))
    }

    fn record(
        mob_id: &str,
        member_id: &str,
        session_id: meerkat_core::types::SessionId,
        policy: DelegateIdleRetireOverride,
    ) -> crate::MemberIdleRetireOverrideRecord {
        crate::MemberIdleRetireOverrideRecord {
            mob_id: mob_id.to_string(),
            member_id: member_id.to_string(),
            session_id,
            policy,
            recorded_at: chrono::Utc::now(),
        }
    }

    /// The attach restores the bound opt-ins a previous process recorded and
    /// persists the bound opt-ins set before it (unbound ones, which only
    /// standalone wiring produces, are never persisted).
    #[tokio::test]
    async fn attach_restores_bound_opt_ins_and_persists_earlier_ones() {
        use crate::{PersistentMetadataStore, SqliteMetadataStore};
        use meerkat_core::types::SessionId;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("metadata.sqlite3");
        let (previous, early) = (SessionId::new(), SessionId::new());
        SqliteMetadataStore::open(&path)
            .expect("open metadata store")
            .set_member_idle_retire_override(&record(
                "mob-a",
                "fork-child",
                previous.clone(),
                DelegateIdleRetireOverride::Seconds(300),
            ))
            .await
            .expect("previous process row");

        let overrides = ImplicitDelegateRetirementOverrides::default();
        overrides
            .insert_for_test(
                "mob-a",
                "early-fork",
                bound(DelegateIdleRetireOverride::RuntimeDefault, Some(&early)),
            )
            .await;
        overrides
            .insert_for_test(
                "mob-a",
                "unbound",
                bound(DelegateIdleRetireOverride::Seconds(1), None),
            )
            .await;
        let restored = overrides
            .attach_durable_store(Arc::new(
                SqliteMetadataStore::open(&path).expect("reopen metadata store"),
            ))
            .await;
        assert_eq!(restored, 1);
        assert_eq!(
            view(overrides.get_bound("mob-a", "fork-child").await),
            Some((
                DelegateIdleRetireOverride::Seconds(300),
                Some(previous.clone())
            ))
        );

        let persisted = SqliteMetadataStore::open(&path)
            .expect("probe metadata store")
            .load_member_idle_retire_overrides()
            .await
            .expect("load");
        let members: Vec<&str> = persisted.iter().map(|r| r.member_id.as_str()).collect();
        assert_eq!(members, vec!["early-fork", "fork-child"]);
    }

    /// A `disabled` opt-in row an older Meta persisted (from a model's
    /// `idle_retire_secs: null`, before null meant "same as omitting") still
    /// loads and still keeps its member out of retirement: the sweep counts
    /// the member as opted in and finds no idle window.
    #[tokio::test]
    async fn a_persisted_disabled_opt_in_still_loads_and_disables_retirement() {
        use crate::SqliteMetadataStore;
        use meerkat_core::types::SessionId;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("metadata.sqlite3");
        drop(SqliteMetadataStore::open(&path).expect("create metadata store"));
        let session = SessionId::new();
        // The row exactly as an older version wrote it (no `recorded_at`).
        rusqlite::Connection::open(&path)
            .expect("open metadata file")
            .execute(
                "INSERT INTO meta_metadata (mob_id, key, value) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    "mob-a",
                    "member_idle_retire/helper",
                    serde_json::json!({"session_id": session, "policy": "disabled"}).to_string()
                ],
            )
            .expect("insert older disabled row");

        let overrides = ImplicitDelegateRetirementOverrides::default();
        let restored = overrides
            .attach_durable_store(Arc::new(
                SqliteMetadataStore::open(&path).expect("reopen metadata store"),
            ))
            .await;
        assert_eq!(restored, 1);
        let policy = overrides
            .reconcile_seated("mob-a", "helper", Some(&session))
            .await;
        assert_eq!(policy, Some(DelegateIdleRetireOverride::Disabled));

        let labels = std::collections::BTreeMap::new();
        assert!(idle_retirement_candidate(true, false, &labels, policy));
        assert_eq!(
            delegate_member_idle_retire_after(&labels, policy, Some(Duration::from_mins(5))),
            None
        );
    }

    /// An unreadable store restores nothing but still receives the opt-ins
    /// set before the attach.
    #[tokio::test]
    async fn attach_to_an_unreadable_store_still_persists_earlier_opt_ins() {
        use crate::{MemberIdleRetireOverrideRecord, MetadataStoreError, PersistentMetadataStore};
        use meerkat_core::types::SessionId;

        #[derive(Default)]
        struct UnreadableStore {
            written: std::sync::Mutex<Vec<MemberIdleRetireOverrideRecord>>,
        }

        #[async_trait::async_trait]
        impl PersistentMetadataStore for UnreadableStore {
            async fn get_subscription_cursor(
                &self,
                _mob_id: &str,
            ) -> Result<Option<u64>, MetadataStoreError> {
                Ok(None)
            }
            async fn set_subscription_cursor(
                &self,
                _mob_id: &str,
                _cursor: u64,
            ) -> Result<(), MetadataStoreError> {
                Ok(())
            }
            async fn load_member_idle_retire_overrides(
                &self,
            ) -> Result<Vec<MemberIdleRetireOverrideRecord>, MetadataStoreError> {
                Err(MetadataStoreError::Io("disk on fire".to_string()))
            }
            async fn set_member_idle_retire_override(
                &self,
                record: &MemberIdleRetireOverrideRecord,
            ) -> Result<(), MetadataStoreError> {
                self.written
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(record.clone());
                Ok(())
            }
        }

        let early = SessionId::new();
        let overrides = ImplicitDelegateRetirementOverrides::default();
        overrides
            .insert_for_test(
                "mob-a",
                "early-fork",
                bound(DelegateIdleRetireOverride::Seconds(60), Some(&early)),
            )
            .await;
        let store = Arc::new(UnreadableStore::default());
        let restored = overrides
            .attach_durable_store(Arc::clone(&store) as Arc<dyn PersistentMetadataStore>)
            .await;
        assert_eq!(restored, 0);
        let written = store
            .written
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let written: Vec<_> = written
            .into_iter()
            .map(|record| {
                (
                    record.mob_id,
                    record.member_id,
                    record.session_id,
                    record.policy,
                )
            })
            .collect();
        assert_eq!(
            written,
            vec![(
                "mob-a".to_string(),
                "early-fork".to_string(),
                early,
                DelegateIdleRetireOverride::Seconds(60),
            )]
        );
    }

    /// Boot a primary-mob runtime whose idle sweep runs every
    /// `sweep_interval_ms` with no runtime default (only a recorded opt-in
    /// makes a primary-mob member a candidate), on the given metadata file.
    fn sweeping_definition(mob_id: &str) -> meerkat_mob::MobDefinition {
        meerkat_mob::MobDefinition::from_toml(&format!(
            r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"

[profiles.worker.tools]
comms = true
"#
        ))
        .expect("mob definition")
    }

    fn sweeping_options() -> crate::MobBootstrapOptions {
        crate::MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        }
    }

    /// The idle-retirement runtime, its sessions in memory only.
    async fn boot_sweeping_runtime(
        mob_id: &str,
        state_root: &std::path::Path,
        metadata_path: &std::path::Path,
        sweep_interval_ms: u64,
    ) -> UnifiedRuntime {
        let mob_spec = crate::MobBootstrapSpec::ephemeral(
            sweeping_definition(mob_id),
            meerkat_mob::MobStorage::in_memory(),
            state_root.to_path_buf(),
            4,
            None,
        )
        .expect("build the idle retirement fixture ephemeral spec")
        .with_options(sweeping_options());
        bootstrap_sweeping(mob_spec, mob_id, metadata_path, sweep_interval_ms).await
    }

    /// The idle-retirement runtime over Core's persistent session service,
    /// whose sessions outlive their members.
    async fn boot_persistent_sweeping_runtime(
        mob_id: &str,
        state_root: &std::path::Path,
        metadata_path: &std::path::Path,
        sweep_interval_ms: u64,
    ) -> UnifiedRuntime {
        std::fs::create_dir_all(state_root).expect("state dir");
        let session_store: Arc<dyn meerkat::SessionStore> = Arc::new(
            meerkat_store::SqliteSessionStore::open(state_root.join("sessions.db"))
                .expect("session store"),
        );
        let runtime_store: Arc<dyn meerkat_runtime::RuntimeStore> = Arc::new(
            meerkat_runtime::store::SqliteRuntimeStore::new(state_root.join("runtime.sqlite"))
                .expect("runtime store"),
        );
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let factory = meerkat::AgentFactory::new(state_root).comms(true);
        let mut inner_builder =
            meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
        inner_builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
            session_store.clone(),
        )));
        inner_builder.default_blob_store = Some(blob_store.clone());
        let jobs: Arc<dyn meerkat::DetachedJobStore> =
            Arc::new(meerkat::MemoryDetachedJobStore::new());
        inner_builder.default_detached_job_store = Some(Arc::clone(&jobs));
        let mob_tools_slot = Arc::clone(&inner_builder.default_mob_tools);
        let delivery = crate::MobRuntimeDelivery::new(Arc::clone(&runtime_store), jobs);
        let adapter = Arc::new(
            meerkat_runtime::MeerkatMachine::persistent(
                Arc::clone(&runtime_store),
                Arc::clone(&blob_store),
            )
            .expect("runtime owner"),
        );
        let concrete = Arc::new(
            meerkat_session::PersistentSessionService::new(
                inner_builder,
                16,
                session_store,
                runtime_store,
                blob_store,
            )
            .with_canonical_runtime_adapter(adapter.clone()),
        );
        let mut mob_spec = crate::MobBootstrapSpec::new(
            sweeping_definition(mob_id),
            meerkat_mob::MobStorage::in_memory(),
            concrete.clone(),
        )
        .with_session_runtime_adapter(adapter)
        .expect("acquire the fixture session runtime owner")
        .with_options(sweeping_options())
        .with_runtime_delivery(delivery)
        .with_agent_mob_tools(mob_tools_slot)
        .expect("install the agent mob tools");
        mob_spec.committed_boundary_recoverer = Some(concrete);
        bootstrap_sweeping(mob_spec, mob_id, metadata_path, sweep_interval_ms).await
    }

    async fn bootstrap_sweeping(
        mob_spec: crate::MobBootstrapSpec,
        mob_id: &str,
        metadata_path: &std::path::Path,
        sweep_interval_ms: u64,
    ) -> UnifiedRuntime {
        use crate::{DiscoverySpec, MetaConfig, SqliteMetadataStore};

        UnifiedRuntime::bootstrap_with_options(
            mob_spec,
            MetaConfig {
                modules: Vec::new(),
                discovery: DiscoverySpec {
                    namespace: mob_id.to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            },
            Vec::new(),
            Duration::from_secs(2),
            RuntimeOptions {
                implicit_delegate_idle_retire_secs: None,
                implicit_delegate_idle_sweep_interval_ms: sweep_interval_ms,
                ..RuntimeOptions::default()
            },
            Arc::new(SqliteMetadataStore::open(metadata_path).expect("open metadata store")),
        )
        .await
        .expect("bootstrap runtime")
    }

    async fn seat(handle: &meerkat_mob::MobHandle, member: &str) -> meerkat_core::types::SessionId {
        handle
            .ensure_member(meerkat_mob::SpawnMemberSpec::new(
                meerkat_mob::ProfileName::from("worker"),
                AgentIdentity::from(member),
            ))
            .await
            .expect("seat member");
        handle
            .resolve_bridge_session_id(&AgentIdentity::from(member))
            .await
            .expect("seated member session")
    }

    async fn is_live(handle: &meerkat_mob::MobHandle, id: &str) -> bool {
        handle
            .list_members_including_retiring()
            .await
            .iter()
            .any(|member| {
                member.agent_identity.as_str() == id && member.status != MobMemberStatus::Retiring
            })
    }

    async fn persisted_members(metadata_path: &std::path::Path) -> Vec<String> {
        use crate::{PersistentMetadataStore, SqliteMetadataStore};

        SqliteMetadataStore::open(metadata_path)
            .expect("probe metadata store")
            .load_member_idle_retire_overrides()
            .await
            .expect("load opt-ins")
            .into_iter()
            .map(|record| record.member_id)
            .collect()
    }

    /// Regression (review of #442): an opt-in whose member was retired
    /// outside the sweep must not apply to a later member seated under the
    /// same id. Two real runtimes share one metadata file: the first opts
    /// `helper` in and retires it by hand (which now releases the row at the
    /// source), the second seats a new `helper` that never opted in. A row a
    /// crash between the retire and its release would leave behind is
    /// written back; the second sweep drops it on its first visit and leaves
    /// the new member alone.
    #[tokio::test]
    async fn sweep_never_applies_a_stale_opt_in_to_a_reused_member_id() {
        const MOB_ID: &str = "reused-member-id";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");

        // First process: its sweep never runs during the test.
        let first = boot_sweeping_runtime(
            MOB_ID,
            &temp.path().join("first"),
            &metadata_path,
            3_600_000,
        )
        .await;
        let first_handle = first.mob_handle();
        let first_session = seat(&first_handle, "helper").await;
        let overrides = first
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        overrides
            .set(MOB_ID, "helper", DelegateIdleRetireOverride::Seconds(0))
            .await;
        assert_eq!(
            view(overrides.get_bound(MOB_ID, "helper").await),
            Some((
                DelegateIdleRetireOverride::Seconds(0),
                Some(first_session.clone())
            )),
            "the opt-in binds to the member's session"
        );
        assert_eq!(persisted_members(&metadata_path).await, vec!["helper"]);
        // Retired outside the sweep (the forker's mob_retire_member, an
        // operator retire): the retirement releases the opt-in row.
        first_handle
            .retire(AgentIdentity::from("helper"))
            .await
            .expect("retire helper by hand");
        tokio::time::timeout(Duration::from_secs(10), async {
            while !persisted_members(&metadata_path).await.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the hand retire did not release the opt-in row");
        first.shutdown().await;
        drop(first_handle);
        drop(first);
        // What a crash between the retire and the release leaves behind.
        {
            use crate::PersistentMetadataStore;
            crate::SqliteMetadataStore::open(&metadata_path)
                .expect("reopen metadata store")
                .set_member_idle_retire_override(&record(
                    MOB_ID,
                    "helper",
                    first_session.clone(),
                    DelegateIdleRetireOverride::Seconds(0),
                ))
                .await
                .expect("crash leftover row");
        }
        assert_eq!(persisted_members(&metadata_path).await, vec!["helper"]);

        // Second process: a new `helper`, seated without any opt-in.
        let second =
            boot_sweeping_runtime(MOB_ID, &temp.path().join("second"), &metadata_path, 1_000).await;
        let second_handle = second.mob_handle();
        let second_session = seat(&second_handle, "helper").await;
        assert_ne!(second_session, first_session);

        tokio::time::timeout(Duration::from_secs(10), async {
            while !persisted_members(&metadata_path).await.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the sweep never dropped the stale opt-in");
        assert!(
            is_live(&second_handle, "helper").await,
            "a member that never opted in must not be idle-retired by a stale opt-in"
        );
        assert_eq!(
            second
                .mob_runtime
                .implicit_delegate_retirement_overrides()
                .expect("overrides")
                .get(MOB_ID, "helper")
                .await,
            None
        );
        second.shutdown().await;
    }

    /// A restored opt-in whose member still runs the bound session re-arms
    /// the sweep: the member is retired and its opt-in cleared, a member
    /// nobody opted in stays. The restore is the attach bootstrap runs, over
    /// a row written for the seated member's session. `set` records nothing
    /// for a member that is not seated or not in a swept mob.
    #[tokio::test]
    async fn sweep_retires_a_restored_member_whose_session_still_matches() {
        use crate::{PersistentMetadataStore, SqliteMetadataStore};

        const MOB_ID: &str = "restored-member";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        // Nothing is a sweep candidate until the opt-in is restored below.
        let runtime =
            boot_sweeping_runtime(MOB_ID, &temp.path().join("state"), &metadata_path, 1_000).await;
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");

        overrides
            .set(MOB_ID, "ghost", DelegateIdleRetireOverride::Seconds(0))
            .await;
        overrides
            .set(
                "explicit-mob",
                "worker",
                DelegateIdleRetireOverride::Seconds(0),
            )
            .await;
        assert_eq!(overrides.get(MOB_ID, "ghost").await, None, "not seated");
        assert_eq!(
            overrides.get("explicit-mob", "worker").await,
            None,
            "not in a swept mob"
        );
        assert!(persisted_members(&metadata_path).await.is_empty());

        let fork_session = seat(&handle, "fork-child").await;
        seat(&handle, "bystander").await;
        SqliteMetadataStore::open(&metadata_path)
            .expect("previous process store")
            .set_member_idle_retire_override(&record(
                MOB_ID,
                "fork-child",
                fork_session,
                DelegateIdleRetireOverride::Seconds(0),
            ))
            .await
            .expect("row for the seated member");
        let restored = overrides
            .attach_durable_store(Arc::new(
                SqliteMetadataStore::open(&metadata_path).expect("reopen metadata store"),
            ))
            .await;
        assert_eq!(restored, 1);

        tokio::time::timeout(Duration::from_secs(10), async {
            while is_live(&handle, "fork-child").await {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the sweep never retired the member whose opt-in was restored");
        assert!(is_live(&handle, "bystander").await);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !persisted_members(&metadata_path).await.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the retired member's opt-in was not cleared");

        runtime.shutdown().await;
    }

    /// A candidate launch starts no idle-retire sweep. The member whose
    /// restored opt-in the authoritative sweep retires within its first
    /// interval (the row above) stays live on a candidate through several
    /// intervals, and its opt-in stays recorded for the authoritative launch.
    #[tokio::test]
    async fn a_candidate_launch_retires_no_idle_member() {
        use crate::{PersistentMetadataStore, SqliteMetadataStore};

        const MOB_ID: &str = "candidate-idle-member";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let mob_spec = crate::MobBootstrapSpec::ephemeral(
            sweeping_definition(MOB_ID),
            meerkat_mob::MobStorage::in_memory(),
            temp.path().join("state"),
            4,
            None,
        )
        .expect("build the candidate fixture spec")
        .with_options(sweeping_options())
        .with_composition_authority(
            crate::mob_composition_manifest::CompositionAuthority::NonAuthoritative,
        );
        let runtime = bootstrap_sweeping(mob_spec, MOB_ID, &metadata_path, 1_000).await;
        assert!(!runtime.speaks_for_composition());
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let fork_session = seat(&handle, "fork-child").await;
        SqliteMetadataStore::open(&metadata_path)
            .expect("previous process store")
            .set_member_idle_retire_override(&record(
                MOB_ID,
                "fork-child",
                fork_session,
                DelegateIdleRetireOverride::Seconds(0),
            ))
            .await
            .expect("row for the seated member");
        let restored = overrides
            .attach_durable_store(Arc::new(
                SqliteMetadataStore::open(&metadata_path).expect("reopen metadata store"),
            ))
            .await;
        assert_eq!(restored, 1);

        tokio::time::sleep(Duration::from_millis(3_500)).await;
        assert!(
            is_live(&handle, "fork-child").await,
            "a candidate launch retires no idle member"
        );
        assert_eq!(
            persisted_members(&metadata_path).await,
            vec!["fork-child"],
            "the member's opt-in stays recorded for the authoritative launch"
        );

        runtime.shutdown().await;
    }

    /// A registered identity over `runtime`'s mob, its member seated: the
    /// identity runtime, the identity, its roster member and the member's
    /// session. The bridge is wired as the gateways wire it, over the
    /// runtime's session service.
    async fn seated_identity(
        runtime: &mut UnifiedRuntime,
        mob_id: &str,
    ) -> (
        Arc<crate::identity_first::IdentityRuntime>,
        crate::identity_first::AgentIdentity,
        AgentIdentity,
        meerkat_core::types::SessionId,
    ) {
        use crate::identity_first::{
            AgentAddressability, AgentIdentity as DurableIdentity, DurabilityPolicy,
            DurableAgentSpec, IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig,
            LocalContinuityStore, LocalLeaseProvider, MobSessionBridge, RosterContext, RosterError,
            RosterProvider,
        };

        struct FixedRoster(Vec<DurableAgentSpec>);

        #[async_trait::async_trait]
        impl RosterProvider for FixedRoster {
            async fn roster(
                &self,
                _context: &RosterContext,
            ) -> Result<Vec<DurableAgentSpec>, RosterError> {
                Ok(self.0.clone())
            }
        }

        let handle = runtime.mob_handle();
        let identity = DurableIdentity::parse("domain:respawning").expect("identity");
        let roster_member =
            crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        // Seated the way an identity-first spawn seats it: the
        // `agent_identity` label names the identity.
        handle
            .ensure_member(
                meerkat_mob::SpawnMemberSpec::new(
                    meerkat_mob::ProfileName::from("worker"),
                    roster_member.clone(),
                )
                .with_labels(std::collections::BTreeMap::from([(
                    "agent_identity".to_string(),
                    identity.as_str().to_string(),
                )])),
            )
            .await
            .expect("seat identity member");
        let session = handle
            .resolve_bridge_session_id(&roster_member)
            .await
            .expect("member session");
        let service = runtime
            .mob_runtime
            .session_service()
            .cloned()
            .expect("the runtime's session service");
        let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(
                LocalContinuityStore::in_memory().expect("continuity store"),
            ),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: mob_id.to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(MobSessionBridge::with_session_service(
                handle.clone(),
                service,
            ))),
            default_timeout: None,
        }));
        let spec = DurableAgentSpec {
            identity: identity.clone(),
            profile: meerkat_mob::ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        };
        identity_runtime.register(spec.clone()).await;
        runtime.attach_identity_first_context(Arc::new(IdentityFirstRuntimeContext::new(
            Arc::clone(&identity_runtime),
            Arc::new(FixedRoster(vec![spec])),
            None,
            None,
            Some(runtime.mob_handle().definition().clone()),
        )));
        (identity_runtime, identity, roster_member, session)
    }

    /// An identity-runtime respawn retires the identity's Core member and
    /// resumes it onto the same session, so the member's idle-retire opt-in,
    /// which the retire releases, is restored on the resumed session. The
    /// runtime's sessions outlive their members, as a respawn needs.
    #[tokio::test]
    async fn identity_runtime_respawn_keeps_the_opt_in() {
        const MOB_ID: &str = "identity-respawn-keeps-opt-in";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let mut runtime = boot_persistent_sweeping_runtime(
            MOB_ID,
            &temp.path().join("state"),
            &metadata_path,
            3_600_000,
        )
        .await;
        let handle = runtime.mob_handle();
        let (identity_runtime, identity, roster_member, before) =
            seated_identity(&mut runtime, MOB_ID).await;

        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let policy = DelegateIdleRetireOverride::Seconds(3600);
        overrides.set(MOB_ID, roster_member.as_str(), policy).await;
        assert_eq!(
            view(overrides.get_bound(MOB_ID, roster_member.as_str()).await),
            Some((policy, Some(before.clone())))
        );

        identity_runtime
            .respawn(&identity)
            .await
            .expect("identity runtime respawn");
        let after = handle
            .resolve_bridge_session_id(&roster_member)
            .await
            .expect("respawned session");
        assert_eq!(
            after, before,
            "an identity respawn resumes the same session"
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while view(overrides.get_bound(MOB_ID, roster_member.as_str()).await)
                != Some((policy, Some(before.clone())))
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the identity respawn did not keep the opt-in on the resumed session");
        runtime.shutdown().await;
    }

    /// On a runtime whose sessions live in memory only, the retire a respawn
    /// starts with would end the session's only copy, and nothing could
    /// resume it. The respawn refuses typed before it retires anything: the
    /// member stays live on its session, its opt-in untouched.
    #[tokio::test]
    async fn identity_runtime_respawn_refuses_before_its_retire_when_sessions_live_in_memory() {
        const MOB_ID: &str = "identity-respawn-in-memory-refused";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let mut runtime = boot_sweeping_runtime(
            MOB_ID,
            &temp.path().join("state"),
            &metadata_path,
            3_600_000,
        )
        .await;
        let handle = runtime.mob_handle();
        let (identity_runtime, identity, roster_member, before) =
            seated_identity(&mut runtime, MOB_ID).await;
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let policy = DelegateIdleRetireOverride::Seconds(3600);
        overrides.set(MOB_ID, roster_member.as_str(), policy).await;

        let refused = identity_runtime.respawn(&identity).await;
        assert!(
            matches!(
                &refused,
                Err(crate::identity_first::IdentityRuntimeError::InvalidState {
                    state: crate::identity_first::IdentityLifecycleState::Active,
                    operation,
                    ..
                }) if operation.contains("lives only in memory")
            ),
            "the respawn refuses typed before its retire: {refused:?}"
        );
        assert_eq!(
            handle.resolve_bridge_session_id(&roster_member).await,
            Some(before.clone()),
            "the member is still live on its session"
        );
        assert_eq!(
            view(overrides.get_bound(MOB_ID, roster_member.as_str()).await),
            Some((policy, Some(before))),
            "nothing released the opt-in"
        );
        runtime.shutdown().await;
    }

    /// On a runtime whose sessions live in memory only, a retired member's
    /// session is gone with it. The next wake reads that no authority knows
    /// the session and starts the identity's fresh life (the Absent rule),
    /// naming the replaced session, instead of retrying a resume that can
    /// never succeed.
    #[tokio::test]
    async fn a_retired_identity_of_an_in_memory_runtime_wakes_on_a_fresh_session() {
        const MOB_ID: &str = "identity-in-memory-retire-wake";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let mut runtime = boot_sweeping_runtime(
            MOB_ID,
            &temp.path().join("state"),
            &metadata_path,
            3_600_000,
        )
        .await;
        let (identity_runtime, identity, _roster_member, before) =
            seated_identity(&mut runtime, MOB_ID).await;

        identity_runtime
            .retire(&identity)
            .await
            .expect("retire the identity");
        identity_runtime
            .materialize(&identity)
            .await
            .expect("the wake starts a fresh life");
        let status = identity_runtime
            .status(&identity)
            .await
            .expect("identity status");
        assert_eq!(
            status.state,
            crate::identity_first::IdentityLifecycleState::Active
        );
        assert!(
            status
                .session_id
                .as_ref()
                .is_some_and(|session| *session != before),
            "the identity runs a fresh session: {status:?}"
        );
        assert_eq!(
            status
                .replaced_session
                .as_ref()
                .map(|replaced| replaced.session_id.clone()),
            Some(before),
            "the status names the session the wake replaced"
        );
        runtime.shutdown().await;
    }

    /// A respawn's rebind carries the opt-in whichever way it races the
    /// retirement of the old session: a live opt-in moves, a just-released
    /// one is restored, a late release of the old session is a no-op, and a
    /// rebind from a session the opt-in never belonged to moves nothing.
    #[tokio::test]
    async fn rebind_carries_the_opt_in_regardless_of_retirement_order() {
        use meerkat_core::types::SessionId;

        let (old, new, unrelated) = (SessionId::new(), SessionId::new(), SessionId::new());
        let policy = DelegateIdleRetireOverride::Seconds(3600);

        // Rebind first, then the old session's retirement arrives.
        let overrides = ImplicitDelegateRetirementOverrides::default();
        overrides
            .insert_for_test("mob-a", "fork", bound(policy, Some(&old)))
            .await;
        assert!(overrides.rebind("mob-a", "fork", &old, new.clone()).await);
        overrides.release_session("mob-a", "fork", &old).await;
        assert_eq!(
            view(overrides.get_bound("mob-a", "fork").await),
            Some((policy, Some(new.clone())))
        );

        // The old session's retirement first, then the rebind.
        let overrides = ImplicitDelegateRetirementOverrides::default();
        overrides
            .insert_for_test("mob-a", "fork", bound(policy, Some(&old)))
            .await;
        overrides.release_session("mob-a", "fork", &old).await;
        assert_eq!(overrides.get_bound("mob-a", "fork").await, None);
        assert!(overrides.rebind("mob-a", "fork", &old, new.clone()).await);
        assert_eq!(
            view(overrides.get_bound("mob-a", "fork").await),
            Some((policy, Some(new.clone())))
        );

        // A rebind from a session the opt-in never belonged to (a reused id
        // respawned) moves nothing.
        let overrides = ImplicitDelegateRetirementOverrides::default();
        overrides
            .insert_for_test("mob-a", "fork", bound(policy, Some(&old)))
            .await;
        assert!(!overrides.rebind("mob-a", "fork", &unrelated, new).await);
        assert_eq!(
            view(overrides.get_bound("mob-a", "fork").await),
            Some((policy, Some(old.clone())))
        );
    }

    async fn persisted_sessions(
        metadata_path: &std::path::Path,
    ) -> Vec<(String, meerkat_core::types::SessionId)> {
        use crate::{PersistentMetadataStore, SqliteMetadataStore};

        SqliteMetadataStore::open(metadata_path)
            .expect("probe metadata store")
            .load_member_idle_retire_overrides()
            .await
            .expect("load opt-ins")
            .into_iter()
            .map(|record| (record.member_id, record.session_id))
            .collect()
    }

    /// Review of #442: a respawned member keeps its opt-in. Meta's respawn
    /// surfaces carry it to the respawned session, in memory and durably;
    /// the old session's retirement does not take it away.
    #[tokio::test]
    async fn respawn_carries_the_opt_in_to_the_respawned_session() {
        const MOB_ID: &str = "respawn-carries-opt-in";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let runtime = boot_sweeping_runtime(
            MOB_ID,
            &temp.path().join("state"),
            &metadata_path,
            3_600_000,
        )
        .await;
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let before = seat(&handle, "fork-child").await;
        let policy = DelegateIdleRetireOverride::Seconds(3600);
        overrides.set(MOB_ID, "fork-child", policy).await;

        let member = AgentIdentity::from("fork-child");
        crate::mob_handle_runtime::respawn_carrying_idle_retire_opt_in(
            Some(&overrides),
            &handle,
            &member,
            handle.respawn(member.clone(), None),
            Result::is_ok,
        )
        .await
        .expect("respawn");
        let after = handle
            .resolve_bridge_session_id(&member)
            .await
            .expect("respawned session");
        assert_ne!(after, before, "a respawn mints a new session");

        // However the old session's retirement interleaved, it is a no-op
        // for the carried opt-in.
        overrides
            .release_session(MOB_ID, "fork-child", &before)
            .await;
        assert_eq!(
            view(overrides.get_bound(MOB_ID, "fork-child").await),
            Some((policy, Some(after.clone())))
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while persisted_sessions(&metadata_path).await
                != vec![("fork-child".to_string(), after.clone())]
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the durable opt-in was not rebound to the respawned session");
        runtime.shutdown().await;
    }

    /// A reset (a respawn that starts a fresh member) and a hand retire both
    /// release the opt-in: the retirement of its session, observed on the
    /// mob event stream whatever surface caused it, clears the row.
    #[tokio::test]
    async fn reset_and_hand_retire_release_the_opt_in_row() {
        const MOB_ID: &str = "retire-releases-opt-in";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let runtime = boot_sweeping_runtime(
            MOB_ID,
            &temp.path().join("state"),
            &metadata_path,
            3_600_000,
        )
        .await;
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        for member in ["reset-me", "retire-me"] {
            seat(&handle, member).await;
            overrides
                .set(MOB_ID, member, DelegateIdleRetireOverride::Seconds(3600))
                .await;
        }
        assert_eq!(
            persisted_members(&metadata_path).await,
            vec!["reset-me", "retire-me"]
        );

        // Reset: respawned without carrying the opt-in.
        let reset = AgentIdentity::from("reset-me");
        crate::mob_handle_runtime::respawn_carrying_idle_retire_opt_in(
            None,
            &handle,
            &reset,
            handle.respawn(reset.clone(), None),
            Result::is_ok,
        )
        .await
        .expect("reset respawn");
        // Hand retire, outside the sweep.
        handle
            .retire(AgentIdentity::from("retire-me"))
            .await
            .expect("hand retire");

        tokio::time::timeout(Duration::from_secs(10), async {
            while !persisted_members(&metadata_path).await.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the retired sessions' opt-in rows were not cleared");
        assert_eq!(overrides.get(MOB_ID, "reset-me").await, None);
        assert_eq!(overrides.get(MOB_ID, "retire-me").await, None);
        runtime.shutdown().await;
    }

    fn mob_event(
        kind: meerkat_mob::MobEventKind,
        at: chrono::DateTime<chrono::Utc>,
    ) -> meerkat_mob::MobEvent {
        meerkat_mob::MobEvent {
            cursor: 1,
            timestamp: at,
            mob_id: meerkat_mob::MobId::from("mob-a"),
            kind,
        }
    }

    /// Reset and destroy events release exactly the opt-ins recorded before
    /// them: an opt-in recorded after the event (the event stream lagging a
    /// quick re-seat) is kept.
    #[tokio::test]
    async fn reset_and_destroy_release_only_opt_ins_recorded_before_them() {
        use meerkat_core::types::SessionId;

        let before = chrono::Utc::now() - chrono::Duration::seconds(60);
        let after = chrono::Utc::now() + chrono::Duration::seconds(60);
        let policy = DelegateIdleRetireOverride::Seconds(3600);
        let overrides = ImplicitDelegateRetirementOverrides::default();
        let at = |at: chrono::DateTime<chrono::Utc>| {
            crate::mob_handle_runtime::BoundIdleRetireOverride {
                recorded_at: at,
                ..bound(policy, Some(&SessionId::new()))
            }
        };
        overrides
            .insert_for_test("mob-a", "old-1", at(before))
            .await;
        overrides.insert_for_test("mob-a", "new-1", at(after)).await;
        overrides
            .insert_for_test("mob-b", "old-2", at(before))
            .await;
        let now = chrono::Utc::now();

        overrides
            .observe_mob_event(
                "mob-a",
                &mob_event(meerkat_mob::MobEventKind::MobDestroying, now),
            )
            .await;
        assert_eq!(overrides.get("mob-a", "old-1").await, None);
        assert_eq!(
            overrides.get("mob-a", "new-1").await,
            Some(policy),
            "an opt-in recorded after the destroy belongs to a later member"
        );
        assert_eq!(
            overrides.get("mob-b", "old-2").await,
            Some(policy),
            "another mob is untouched"
        );

        overrides
            .insert_for_test("mob-a", "reset-me", at(before))
            .await;
        overrides
            .insert_for_test("mob-a", "reseated", at(after))
            .await;
        let reset = |member: &str| {
            mob_event(
                meerkat_mob::MobEventKind::MemberReset {
                    agent_identity: AgentIdentity::from(member),
                    previous_generation: meerkat_mob::ids::Generation::new(0),
                    new_generation: meerkat_mob::ids::Generation::new(1),
                    fence_token: meerkat_mob::ids::FenceToken::new(1),
                    agent_runtime_id: meerkat_mob::ids::AgentRuntimeId::new(
                        AgentIdentity::from(member),
                        meerkat_mob::ids::Generation::new(1),
                    ),
                },
                now,
            )
        };
        overrides
            .observe_mob_event("mob-a", &reset("reset-me"))
            .await;
        overrides
            .observe_mob_event("mob-a", &reset("reseated"))
            .await;
        assert_eq!(overrides.get("mob-a", "reset-me").await, None);
        assert_eq!(overrides.get("mob-a", "reseated").await, Some(policy));
    }

    /// A member retired and then resumed onto the exact same session (an
    /// operator re-attach, a repair's retire-then-resume) gets its opt-in
    /// back; a different session does not.
    #[tokio::test]
    async fn resuming_onto_the_same_session_restores_the_released_opt_in() {
        use meerkat_core::types::SessionId;

        let (session, other) = (SessionId::new(), SessionId::new());
        let policy = DelegateIdleRetireOverride::Seconds(3600);
        let overrides = ImplicitDelegateRetirementOverrides::default();
        overrides
            .insert_for_test("mob-a", "fork", bound(policy, Some(&session)))
            .await;
        overrides.release_session("mob-a", "fork", &session).await;
        assert_eq!(overrides.get("mob-a", "fork").await, None);

        assert_eq!(
            overrides
                .reconcile_seated("mob-a", "fork", Some(&other))
                .await,
            None,
            "a member on another session is another instance"
        );
        assert_eq!(
            overrides
                .reconcile_seated("mob-a", "fork", Some(&session))
                .await,
            Some(policy)
        );
        assert_eq!(
            view(overrides.get_bound("mob-a", "fork").await),
            Some((policy, Some(session)))
        );
    }

    /// Review of #442 (hardening): opt-ins recorded for members of implicit
    /// delegation mobs are released when their member is retired by hand or
    /// their mob is destroyed. Those mobs are not on the primary event
    /// stream; the sweep settles them.
    #[tokio::test]
    async fn implicit_mob_retire_and_destroy_release_their_opt_ins() {
        const MOB_ID: &str = "implicit-mob-release";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let runtime =
            boot_sweeping_runtime(MOB_ID, &temp.path().join("state"), &metadata_path, 1_000).await;
        let state = runtime
            .mob_runtime
            .agent_mob_mcp_state()
            .expect("agent mob state");
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let owner = meerkat_core::types::SessionId::new().to_string();
        let (implicit, _) = state
            .ensure_implicit_mob_for_model(&owner, "gpt-5.5", None)
            .await
            .expect("implicit mob");
        for helper in ["helper-a", "helper-b"] {
            state
                .mob_spawn(
                    &implicit,
                    meerkat_mob::ProfileName::from("delegate"),
                    AgentIdentity::from(helper),
                    Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                    None,
                    None,
                )
                .await
                .expect("seat helper");
            overrides
                .set(
                    implicit.as_str(),
                    helper,
                    DelegateIdleRetireOverride::Seconds(3600),
                )
                .await;
        }
        assert_eq!(
            persisted_members(&metadata_path).await,
            vec!["helper-a", "helper-b"]
        );

        state
            .handle_for(&implicit)
            .await
            .expect("implicit handle")
            .retire(AgentIdentity::from("helper-a"))
            .await
            .expect("hand retire");
        tokio::time::timeout(Duration::from_secs(10), async {
            while persisted_members(&metadata_path).await != vec!["helper-b"] {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the hand-retired helper's opt-in was not released");

        state
            .destroy_bridge_session_mobs(&owner)
            .await
            .expect("destroy the implicit mob");
        tokio::time::timeout(Duration::from_secs(10), async {
            while !persisted_members(&metadata_path).await.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the destroyed implicit mob's opt-ins were not released");
        runtime.shutdown().await;
    }

    /// A member retired by hand and then resumed onto its exact session gets
    /// its opt-in back (the retirement released it; the resume restores it),
    /// as `meta/attach_existing_session` does on a persistent runtime.
    #[tokio::test]
    async fn retire_then_resume_onto_the_same_session_keeps_the_opt_in() {
        use crate::{DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec};

        const MOB_ID: &str = "resume-same-session";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let state_root = temp.path().join("state");
        std::fs::create_dir_all(&state_root).expect("state root");
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{MOB_ID}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n\
             [profiles.worker.tools]\ncomms = true\n"
        ))
        .expect("mob definition");
        let spec = MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            state_root,
            4,
        )
        .await
        .expect("persistent spec")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
        });
        let runtime = UnifiedRuntime::bootstrap_with_options(
            spec,
            MetaConfig {
                modules: Vec::new(),
                discovery: DiscoverySpec {
                    namespace: MOB_ID.to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            },
            Vec::new(),
            Duration::from_secs(2),
            RuntimeOptions {
                implicit_delegate_idle_retire_secs: None,
                implicit_delegate_idle_sweep_interval_ms: 1_000,
                ..RuntimeOptions::default()
            },
            Arc::new(crate::SqliteMetadataStore::open(&metadata_path).expect("metadata store")),
        )
        .await
        .expect("bootstrap persistent runtime");
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let session = seat(&handle, "fork-child").await;
        let policy = DelegateIdleRetireOverride::Seconds(3600);
        overrides.set(MOB_ID, "fork-child", policy).await;

        let member = AgentIdentity::from("fork-child");
        handle.retire(member.clone()).await.expect("hand retire");
        tokio::time::timeout(Duration::from_secs(10), async {
            while !persisted_members(&metadata_path).await.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the retirement did not release the opt-in");

        Box::pin(
            handle.spawn_spec(
                meerkat_mob::SpawnMemberSpec::new(
                    meerkat_mob::ProfileName::from("worker"),
                    member.clone(),
                )
                .with_launch_mode(meerkat_mob::launch::MemberLaunchMode::Resume {
                    bridge_session_id: session.clone(),
                    resume_from_role: None,
                }),
            ),
        )
        .await
        .expect("resume onto the same session");
        assert_eq!(
            handle.resolve_bridge_session_id(&member).await,
            Some(session.clone()),
            "the member is back on its exact session"
        );
        tokio::time::timeout(Duration::from_secs(10), async {
            while persisted_sessions(&metadata_path).await
                != vec![("fork-child".to_string(), session.clone())]
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the resumed member's opt-in was not restored");
        assert_eq!(overrides.get(MOB_ID, "fork-child").await, Some(policy));
        runtime.shutdown().await;
    }

    /// Review of #447: two respawns of the same member at once (a console
    /// double click, an operator and the agent's `mob_respawn` together)
    /// keep the opt-in on the member's final session, round after round.
    #[tokio::test]
    async fn concurrent_double_respawns_keep_the_opt_in() {
        const MOB_ID: &str = "concurrent-double-respawn";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let runtime =
            boot_sweeping_runtime(MOB_ID, &temp.path().join("state"), &metadata_path, 1_000).await;
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        seat(&handle, "fork-child").await;
        let policy = DelegateIdleRetireOverride::Seconds(3600);
        overrides.set(MOB_ID, "fork-child", policy).await;
        let member = AgentIdentity::from("fork-child");

        for round in 0..10 {
            let (first, second) = tokio::join!(
                crate::mob_handle_runtime::respawn_carrying_idle_retire_opt_in(
                    Some(&overrides),
                    &handle,
                    &member,
                    handle.respawn(member.clone(), None),
                    Result::is_ok,
                ),
                crate::mob_handle_runtime::respawn_carrying_idle_retire_opt_in(
                    Some(&overrides),
                    &handle,
                    &member,
                    handle.respawn(member.clone(), None),
                    Result::is_ok,
                ),
            );
            assert!(
                first.is_ok() || second.is_ok(),
                "round {round}: at least one respawn lands"
            );
            let current = handle
                .resolve_bridge_session_id(&member)
                .await
                .expect("respawned session");
            tokio::time::timeout(Duration::from_secs(5), async {
                while view(overrides.get_bound(MOB_ID, "fork-child").await)
                    != Some((policy, Some(current.clone())))
                    || persisted_sessions(&metadata_path).await
                        != vec![("fork-child".to_string(), current.clone())]
                {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .await
            .unwrap_or_else(|_| {
                panic!("round {round}: the opt-in did not stay on the respawned session")
            });
        }
        runtime.shutdown().await;
    }

    /// Review of #447: a respawning request dropped at any point (a client
    /// disconnect) may lose the respawned member's own opt-in (the
    /// documented leak-direction limit), but never leaves an opt-in that a
    /// DIFFERENT member seated under the id would pick up.
    #[tokio::test]
    async fn a_mid_respawn_request_drop_never_makes_another_member_retirable() {
        const MOB_ID: &str = "mid-respawn-drop";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let runtime = boot_sweeping_runtime(
            MOB_ID,
            &temp.path().join("state"),
            &metadata_path,
            3_600_000,
        )
        .await;
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let policy = DelegateIdleRetireOverride::Seconds(0);

        for (round, delay_us) in [0_u64, 100, 1_000, 3_000, 10_000, 30_000, 100_000]
            .into_iter()
            .enumerate()
        {
            let id = format!("drop-{round}");
            let member = AgentIdentity::from(id.as_str());
            seat(&handle, &id).await;
            overrides.set(MOB_ID, id.as_str(), policy).await;

            let _ = tokio::time::timeout(
                Duration::from_micros(delay_us),
                crate::mob_handle_runtime::respawn_carrying_idle_retire_opt_in(
                    Some(&overrides),
                    &handle,
                    &member,
                    handle.respawn(member.clone(), None),
                    Result::is_ok,
                ),
            )
            .await;
            // Let a respawn the drop did not cancel finish in the actor.
            let settled = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let live = handle
                        .list_members_including_retiring()
                        .await
                        .into_iter()
                        .find(|entry| entry.agent_identity.as_str() == id)
                        .is_some_and(|entry| entry.status != MobMemberStatus::Retiring);
                    if live && let Some(session) = handle.resolve_bridge_session_id(&member).await {
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        if handle.resolve_bridge_session_id(&member).await == Some(session.clone())
                        {
                            return session;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("round {round}: the member never settled"));

            // Whatever survived applies to this member instance or to no one.
            if let Some(bound) = overrides.get_bound(MOB_ID, &id).await {
                let honoured = overrides
                    .reconcile_seated(MOB_ID, &id, Some(&settled))
                    .await;
                assert!(
                    honoured.is_none() || bound.session_id.as_ref() == Some(&settled),
                    "round {round}: an opt-in applied to a session it was not bound to"
                );
            }

            // A different member seated under the same id never inherits it.
            handle.retire(member.clone()).await.expect("hand retire");
            let fresh = seat(&handle, &id).await;
            assert_eq!(
                overrides.reconcile_seated(MOB_ID, &id, Some(&fresh)).await,
                None,
                "round {round}: a different member under the id became retirable"
            );
            assert_eq!(overrides.get_bound(MOB_ID, &id).await, None);
        }
        runtime.shutdown().await;
    }

    /// The implicit-mob release skips a member whose Meta respawn is in
    /// flight: it can be missing from the roster for a moment, and its
    /// respawn carries the opt-in.
    #[tokio::test]
    async fn implicit_mob_release_skips_a_member_being_respawned() {
        const MOB_ID: &str = "implicit-respawn-skip";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let runtime =
            boot_sweeping_runtime(MOB_ID, &temp.path().join("state"), &metadata_path, 1_000).await;
        let state = runtime
            .mob_runtime
            .agent_mob_mcp_state()
            .expect("agent mob state");
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let owner = meerkat_core::types::SessionId::new().to_string();
        let (implicit, _) = state
            .ensure_implicit_mob_for_model(&owner, "gpt-5.5", None)
            .await
            .expect("implicit mob");
        state
            .mob_spawn(
                &implicit,
                meerkat_mob::ProfileName::from("delegate"),
                AgentIdentity::from("helper"),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .expect("seat helper");
        overrides
            .set(
                implicit.as_str(),
                "helper",
                DelegateIdleRetireOverride::Disabled,
            )
            .await;

        // The helper drops out of the roster while its respawn is in flight.
        let in_flight = overrides.respawn_in_flight(implicit.as_str(), "helper");
        state
            .handle_for(&implicit)
            .await
            .expect("implicit handle")
            .retire(AgentIdentity::from("helper"))
            .await
            .expect("helper leaves the roster");
        tokio::time::sleep(Duration::from_millis(2_500)).await;
        assert_eq!(
            persisted_members(&metadata_path).await,
            vec!["helper"],
            "a member being respawned keeps its opt-in through the gap"
        );

        drop(in_flight);
        tokio::time::timeout(Duration::from_secs(10), async {
            while !persisted_members(&metadata_path).await.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("once no respawn is in flight, the gone helper's opt-in is released");
        runtime.shutdown().await;
    }

    /// Deterministic companion: a request dropped after its respawn committed
    /// but before its rebind leaves the opt-in bound to the gone session
    /// (here nothing releases it: this runtime has no event hook). A member
    /// seated later under the id never picks it up; the stale binding is
    /// dropped instead.
    #[tokio::test]
    async fn a_respawn_whose_rebind_never_ran_leaves_nothing_for_another_member() {
        use crate::{MobBootstrapOptions, MobBootstrapSpec};

        const MOB_ID: &str = "rebind-never-ran";
        let temp = tempfile::tempdir().expect("tempdir");
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{MOB_ID}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n\
             [profiles.worker.tools]\ncomms = true\n"
        ))
        .expect("mob definition");
        let runtime = crate::mob_handle_runtime::MobRuntime::bootstrap(
            MobBootstrapSpec::ephemeral(
                definition,
                meerkat_mob::MobStorage::in_memory(),
                temp.path().to_path_buf(),
                4,
                None,
            )
            .expect("build the idle retirement fixture ephemeral spec")
            .with_options(MobBootstrapOptions {
                allow_ephemeral_sessions: true,
                notify_orchestrator_on_resume: true,
                default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
            }),
        )
        .await
        .expect("bootstrap runtime");
        let handle = runtime.handle();
        let overrides = runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let member = AgentIdentity::from("fork-child");
        let original = seat(&handle, "fork-child").await;
        overrides
            .set(MOB_ID, "fork-child", DelegateIdleRetireOverride::Seconds(0))
            .await;

        // The respawn commits; the request that would have rebound is gone.
        handle.respawn(member.clone(), None).await.expect("respawn");
        assert_eq!(
            view(overrides.get_bound(MOB_ID, "fork-child").await),
            Some((DelegateIdleRetireOverride::Seconds(0), Some(original))),
            "the opt-in is left bound to the gone session"
        );
        handle.retire(member.clone()).await.expect("hand retire");
        let fresh = seat(&handle, "fork-child").await;
        assert_eq!(
            overrides
                .reconcile_seated(MOB_ID, "fork-child", Some(&fresh))
                .await,
            None,
            "a different member under the id must not become retirable"
        );
        assert_eq!(overrides.get_bound(MOB_ID, "fork-child").await, None);
        let _ = handle.shutdown().await;
    }

    /// Review of #447: another surface retires x and seats a new x (which
    /// never opted in) after a Meta respawn read x's incarnation, ahead of
    /// the respawn in the actor queue. The respawn then succeeds on the new
    /// x. The member now under the id is not the successor of the one read
    /// before, so the opt-in is not carried and the new x is never
    /// idle-retired.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_retire_and_reseat_queued_ahead_of_a_respawn_never_carry_the_opt_in() {
        const MOB_ID: &str = "staged-reseat-respawn";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let runtime =
            boot_sweeping_runtime(MOB_ID, &temp.path().join("state"), &metadata_path, 1_000).await;
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let member = AgentIdentity::from("x");
        let original = seat(&handle, "x").await;
        overrides
            .set(MOB_ID, "x", DelegateIdleRetireOverride::Seconds(1))
            .await;

        let staged = handle.clone();
        let staged_member = member.clone();
        let respawned = crate::mob_handle_runtime::respawn_carrying_idle_retire_opt_in(
            Some(&overrides),
            &handle,
            &member,
            async move {
                // Ahead of the respawn in the actor queue: another surface
                // retires x and seats a new x.
                staged.retire(staged_member.clone()).await.expect("retire");
                seat(&staged, staged_member.as_str()).await;
                staged.respawn(staged_member, None).await
            },
            Result::is_ok,
        )
        .await;
        respawned.expect("the respawn of the new x succeeds");
        let current = handle
            .resolve_bridge_session_id(&member)
            .await
            .expect("the new x is seated");
        assert_ne!(current, original);
        assert!(
            overrides
                .get_bound(MOB_ID, "x")
                .await
                .is_none_or(|bound| bound.session_id.as_ref() != Some(&current)),
            "the new x must not hold the old x's opt-in"
        );
        assert!(
            !persisted_sessions(&metadata_path)
                .await
                .iter()
                .any(|(_, session)| session == &current),
            "no durable opt-in row for the new x"
        );
        // Several sweep passes: the new x stays.
        tokio::time::sleep(Duration::from_millis(3_500)).await;
        assert!(is_live(&handle, "x").await, "the new x was idle-retired");
        assert_eq!(
            handle.resolve_bridge_session_id(&member).await,
            Some(current)
        );
        runtime.shutdown().await;
    }

    /// A respawn that fails carries nothing, even when the member under the
    /// id changed while it ran (here another surface retired x and seated a
    /// new x first).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_failed_respawn_never_carries_the_opt_in() {
        const MOB_ID: &str = "staged-failed-respawn";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let runtime =
            boot_sweeping_runtime(MOB_ID, &temp.path().join("state"), &metadata_path, 1_000).await;
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let member = AgentIdentity::from("x");
        seat(&handle, "x").await;
        overrides
            .set(MOB_ID, "x", DelegateIdleRetireOverride::Seconds(1))
            .await;

        let staged = handle.clone();
        let staged_member = member.clone();
        let outcome: Result<(), &str> =
            crate::mob_handle_runtime::respawn_carrying_idle_retire_opt_in(
                Some(&overrides),
                &handle,
                &member,
                async move {
                    staged.retire(staged_member.clone()).await.expect("retire");
                    seat(&staged, staged_member.as_str()).await;
                    Err("respawn rejected")
                },
                Result::is_ok,
            )
            .await;
        assert!(outcome.is_err());
        let fresh = handle
            .resolve_bridge_session_id(&member)
            .await
            .expect("the new x is seated");
        assert!(
            overrides
                .get_bound(MOB_ID, "x")
                .await
                .is_none_or(|bound| bound.session_id.as_ref() != Some(&fresh)),
            "the new x must not hold the old x's opt-in"
        );
        assert!(
            !persisted_sessions(&metadata_path)
                .await
                .iter()
                .any(|(_, session)| session == &fresh),
            "no durable opt-in row for the new x"
        );
        tokio::time::sleep(Duration::from_millis(3_500)).await;
        assert!(is_live(&handle, "x").await, "the new x was idle-retired");
        runtime.shutdown().await;
    }

    /// An LLM whose turns hold until the gate opens: a member's turn stays
    /// running until then.
    struct GatedLlmClient {
        gate: tokio::sync::watch::Receiver<bool>,
    }

    impl meerkat_client::LlmClient for GatedLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a meerkat_client::LlmRequest,
        ) -> std::pin::Pin<
            Box<
                dyn futures::Stream<
                        Item = Result<meerkat_client::LlmEvent, meerkat_client::LlmError>,
                    > + Send
                    + 'a,
            >,
        > {
            use futures::StreamExt as _;

            let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
                request,
                meerkat_core::Provider::OpenAI,
                meerkat_core::types::StopReason::EndTurn,
            );
            let events = vec![
                Ok(meerkat_client::LlmEvent::TextDelta {
                    delta: "done".to_string(),
                    meta: None,
                }),
                Ok(usage),
                Ok(done),
            ];
            let mut gate = self.gate.clone();
            Box::pin(
                futures::stream::once(async move {
                    let _ = gate.wait_for(|open| *open).await;
                    futures::stream::iter(events)
                })
                .flatten(),
            )
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        fn health_check<'life0, 'async_trait>(
            &'life0 self,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = Result<(), meerkat_client::LlmError>>
                    + Send
                    + 'async_trait,
            >,
        >
        where
            'life0: 'async_trait,
            Self: 'async_trait,
        {
            Box::pin(async { Ok(()) })
        }
    }

    /// A persistent runtime (forks need durable transcript authority) whose
    /// members all run on a [`GatedLlmClient`] behind `gate`, with a 1s idle
    /// sweep and no runtime default opt-in.
    async fn boot_gated_persistent_runtime(
        mob_id: &str,
        root: &std::path::Path,
        gate: tokio::sync::watch::Receiver<bool>,
    ) -> UnifiedRuntime {
        use crate::{DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec};

        let state_root = root.join("state");
        std::fs::create_dir_all(&state_root).expect("state root");
        let definition = meerkat_mob::MobDefinition::from_toml(&format!(
            "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\n\
             [profiles.worker.tools]\ncomms = true\n"
        ))
        .expect("mob definition");
        let spec = MobBootstrapSpec::persistent(
            definition,
            meerkat_mob::MobStorage::in_memory(),
            crate::mob_composition_manifest::MobStorageProvenance::declared_ephemeral(),
            state_root,
            4,
        )
        .await
        .expect("persistent spec")
        .with_options(MobBootstrapOptions {
            allow_ephemeral_sessions: true,
            notify_orchestrator_on_resume: true,
            default_llm_client: Some(Arc::new(GatedLlmClient { gate })),
        });
        UnifiedRuntime::bootstrap_with_options(
            spec,
            MetaConfig {
                modules: Vec::new(),
                discovery: DiscoverySpec {
                    namespace: mob_id.to_string(),
                    modules: Vec::new(),
                },
                pre_spawn: Vec::new(),
            },
            Vec::new(),
            Duration::from_secs(2),
            RuntimeOptions {
                implicit_delegate_idle_retire_secs: None,
                implicit_delegate_idle_sweep_interval_ms: 1_000,
                ..RuntimeOptions::default()
            },
            Arc::new(
                crate::SqliteMetadataStore::open(root.join("metadata.sqlite3"))
                    .expect("metadata store"),
            ),
        )
        .await
        .expect("bootstrap persistent runtime")
    }

    /// Seat `member` turn-driven: it runs no kickoff turn, so it is idle from
    /// the start.
    async fn seat_turn_driven(handle: &meerkat_mob::MobHandle, member: &str) {
        let mut spec = meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("worker"),
            AgentIdentity::from(member),
        );
        spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
        handle
            .ensure_member(spec)
            .await
            .expect("seat turn-driven member");
    }

    /// meerkat #1190 cascades a retire to everything the member spawned. A
    /// member C that is idle while its own fork D still runs must not be
    /// idle-retired: that would kill D with its outcome reaching nobody. Once
    /// D finishes and is idle-retired itself, C is idle-retired.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_member_whose_fork_still_runs_is_not_idle_retired() {
        const MOB_ID: &str = "idle-forker";
        let temp = tempfile::tempdir().expect("tempdir");
        let (open_gate, gate) = tokio::sync::watch::channel(false);
        let runtime = boot_gated_persistent_runtime(MOB_ID, temp.path(), gate).await;
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let c = AgentIdentity::from("c");
        let d = AgentIdentity::from("d");
        // Turn-driven, so C runs no kickoff turn (the gate would hold it).
        seat_turn_driven(&handle, "c").await;

        // C forks D from its own turn; D's turn holds until the gate opens.
        let mut fork =
            meerkat_mob::SpawnMemberSpec::new(meerkat_mob::ProfileName::from("worker"), d.clone());
        fork.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
        fork.initial_message = Some(meerkat_core::ContentInput::Text("long work".to_string()));
        let (_forked, run) = handle
            .fork_member_then_run_detached(
                &c,
                fork,
                None,
                "fork_result",
                256,
                meerkat_core::DurableForkSourceAdmission::CallerTurn,
                None,
                None,
            )
            .await
            .expect("c forks d");
        assert_eq!(
            handle
                .get_member(&d)
                .await
                .expect("roster read")
                .expect("d is seated")
                .spawned_by,
            Some(c.clone())
        );
        // Both opted in, as fork children are: D's opt-in must not retire it
        // while it runs, and C's must not while it owns D.
        overrides
            .set(MOB_ID, "c", DelegateIdleRetireOverride::Seconds(1))
            .await;
        overrides
            .set(MOB_ID, "d", DelegateIdleRetireOverride::Seconds(1))
            .await;

        // Several sweep passes past both idle windows while D runs.
        tokio::time::sleep(Duration::from_millis(3_500)).await;
        assert!(is_live(&handle, "d").await, "d was retired while running");
        assert!(
            is_live(&handle, "c").await,
            "c was idle-retired while its fork still runs"
        );

        // D finishes and is idle-retired first; then C, which no longer owns
        // a live member, is idle-retired too.
        open_gate.send(true).expect("open the gate");
        tokio::time::timeout(Duration::from_secs(10), run.outcome())
            .await
            .expect("d finishes")
            .expect("d reached an outcome");
        let order = tokio::time::timeout(Duration::from_secs(15), async {
            let mut d_gone_while_c_live = false;
            loop {
                let (c_live, d_live) = (is_live(&handle, "c").await, is_live(&handle, "d").await);
                d_gone_while_c_live |= !d_live && c_live;
                if !c_live && !d_live {
                    return d_gone_while_c_live;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("c and d are idle-retired once d finishes");
        assert!(
            order,
            "d must go first: retiring c would have cascaded to it"
        );
        runtime.shutdown().await;
    }

    /// Checks steer, they do not queue: a member mid-turn does not hold up
    /// the sweep. The busy member comes first in the roster and its turn
    /// never ends here; the idle member behind it is still idle-retired on
    /// time, and the busy one is left alone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_member_mid_turn_does_not_hold_up_the_sweep() {
        const MOB_ID: &str = "sweep-busy-member";
        let temp = tempfile::tempdir().expect("tempdir");
        let (open_gate, gate) = tokio::sync::watch::channel(false);
        let runtime = boot_gated_persistent_runtime(MOB_ID, temp.path(), gate).await;
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        // Autonomous: its kickoff turn starts at once and holds on the gate.
        seat(&handle, "a-busy").await;
        seat_turn_driven(&handle, "b-idle").await;
        overrides
            .set(MOB_ID, "a-busy", DelegateIdleRetireOverride::Seconds(1))
            .await;
        overrides
            .set(MOB_ID, "b-idle", DelegateIdleRetireOverride::Seconds(1))
            .await;

        tokio::time::timeout(Duration::from_secs(6), async {
            while is_live(&handle, "b-idle").await {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the idle member waited behind the busy member's turn");
        assert!(
            is_live(&handle, "a-busy").await,
            "a member mid-turn was idle-retired"
        );
        open_gate.send(true).expect("open the gate");
        runtime.shutdown().await;
    }

    /// Wait until `member`'s run is open (typed member status), re-reading
    /// only after one of its events or a machine-state change; both are
    /// subscribed before the first read. The deadline only bounds a broken
    /// fixture and fails with a clear message.
    async fn await_member_run_open(handle: &meerkat_mob::MobHandle, member: &AgentIdentity) {
        use futures::StreamExt as _;
        let mut events = handle
            .subscribe_agent_events(member)
            .await
            .expect("subscribe to the member's events");
        let mut changes = handle.machine_state_changes();
        let open = async {
            while !handle
                .member_status(member)
                .await
                .ok()
                .and_then(|status| status.progress)
                .is_some_and(|progress| progress.run_state == meerkat_mob::MemberRunState::RunOpen)
            {
                tokio::select! {
                    event = events.next() => assert!(
                        event.is_some(),
                        "the member's event stream ended before its run opened"
                    ),
                    changed = changes.changed() => assert!(
                        changed.is_ok(),
                        "the mob actor stopped before the member's run opened"
                    ),
                }
            }
        };
        assert!(
            tokio::time::timeout(Duration::from_secs(30), open)
                .await
                .is_ok(),
            "the member's run did not open within 30 s"
        );
    }

    /// One sweep pass's member snapshot: each member's identity and status.
    type SweepSnapshot = Vec<(AgentIdentity, MobMemberStatus)>;

    /// Holds sweep passes over one mob right after their member snapshot.
    struct SweepPause {
        reached: tokio::sync::mpsc::UnboundedSender<SweepSnapshot>,
        release: Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<()>>>,
    }

    static SWEEP_PAUSES: std::sync::Mutex<BTreeMap<String, SweepPause>> =
        std::sync::Mutex::new(BTreeMap::new());

    /// Called by the sweep once it has taken `mob_id`'s member snapshot: when
    /// a test paused that mob, report the pass with the snapshot it took and
    /// wait for its release.
    pub(super) async fn pause_after_member_snapshot(
        mob_id: &str,
        members: &[meerkat_mob::runtime::MobMemberListEntry],
    ) {
        let pause = SWEEP_PAUSES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(mob_id)
            .map(|pause| (pause.reached.clone(), Arc::clone(&pause.release)));
        let Some((reached, release)) = pause else {
            return;
        };
        let snapshot = members
            .iter()
            .map(|member| (member.agent_identity.clone(), member.status))
            .collect();
        if reached.send(snapshot).is_ok() {
            let _ = release.lock().await.recv().await;
        }
    }

    /// Hold every sweep pass over `mob_id` right after its member snapshot:
    /// each pass reports its snapshot on the returned receiver and waits for
    /// one release on the returned sender. [`unpause_sweep`] plus dropping the
    /// sender lets passes run freely again.
    fn pause_sweep(
        mob_id: &str,
    ) -> (
        tokio::sync::mpsc::UnboundedReceiver<SweepSnapshot>,
        tokio::sync::mpsc::UnboundedSender<()>,
    ) {
        let (reached, reached_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release) = tokio::sync::mpsc::unbounded_channel();
        SWEEP_PAUSES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                mob_id.to_string(),
                SweepPause {
                    reached,
                    release: Arc::new(tokio::sync::Mutex::new(release)),
                },
            );
        (reached_rx, release_tx)
    }

    fn unpause_sweep(mob_id: &str) {
        SWEEP_PAUSES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(mob_id);
    }

    /// Holds sweep passes over one mob right before they retire a member.
    static RETIRE_PAUSES: std::sync::Mutex<BTreeMap<String, SweepPause>> =
        std::sync::Mutex::new(BTreeMap::new());

    /// Called by the sweep right before it retires `member` of `mob_id`: when
    /// a test paused that mob's retires, report the member and wait for its
    /// release.
    pub(super) async fn pause_before_retire(mob_id: &str, member: &AgentIdentity) {
        let pause = RETIRE_PAUSES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(mob_id)
            .map(|pause| (pause.reached.clone(), Arc::clone(&pause.release)));
        let Some((reached, release)) = pause else {
            return;
        };
        if reached
            .send(vec![(member.clone(), MobMemberStatus::Active)])
            .is_ok()
        {
            let _ = release.lock().await.recv().await;
        }
    }

    /// Hold every retire the sweep is about to make in `mob_id`: each reports
    /// its member on the returned receiver and waits for one release on the
    /// returned sender. [`unpause_retire`] lets retires run freely again.
    fn pause_retire(
        mob_id: &str,
    ) -> (
        tokio::sync::mpsc::UnboundedReceiver<SweepSnapshot>,
        tokio::sync::mpsc::UnboundedSender<()>,
    ) {
        let (reached, reached_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release) = tokio::sync::mpsc::unbounded_channel();
        RETIRE_PAUSES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                mob_id.to_string(),
                SweepPause {
                    reached,
                    release: Arc::new(tokio::sync::Mutex::new(release)),
                },
            );
        (reached_rx, release_tx)
    }

    fn unpause_retire(mob_id: &str) {
        RETIRE_PAUSES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(mob_id);
    }

    /// The idle-retirement runtime as a gateway composes it over a durable
    /// root: persistent mob storage, Core's persistent session service and
    /// runtime store, and the opt-ins in `<root>/metadata.sqlite3`. A second
    /// boot over the same root restores the roster, the sessions and the
    /// opt-ins. No identity-first activation runs, so nothing revives a
    /// restored member's session. Members run on `client`.
    async fn boot_restartable_sweeping_runtime(
        mob_id: &str,
        root: &std::path::Path,
        sweep_interval_ms: u64,
        client: Arc<dyn meerkat_client::LlmClient>,
    ) -> UnifiedRuntime {
        let state_root = root.join("state");
        std::fs::create_dir_all(&state_root).expect("state root");
        let (storage, provenance) =
            crate::mob_composition_manifest::persistent_mob_storage(root.join("mob.sqlite3"))
                .expect("open persistent mob storage");
        let spec = crate::MobBootstrapSpec::persistent(
            sweeping_definition(mob_id),
            storage,
            provenance,
            state_root,
            16,
        )
        .await
        .expect("compose persistent Meta stores")
        // Both boots run in one process: a durable child-mob default would
        // hold the state root's child mobs for the process lifetime.
        .with_child_mob_storage(crate::ChildMobStorage::ProcessBound)
        .with_options(crate::MobBootstrapOptions {
            default_llm_client: Some(client),
            ..sweeping_options()
        });
        bootstrap_sweeping(
            spec,
            mob_id,
            &root.join("metadata.sqlite3"),
            sweep_interval_ms,
        )
        .await
    }

    /// L11: a member a restart left without a live session, that owes no
    /// work, is retired after its window like any idle member. The first boot
    /// seats `worker` with an opt-in and `bystander` without one, and never
    /// sweeps. While the worker's kickoff turn is held on the model, the
    /// durable store read finds its open input under the worker's session;
    /// once the turn ends it finds none. The second boot over the same
    /// durable root restores both members, and nothing revives their
    /// sessions: the store holds the worker's session and no live actor
    /// serves it, so its execution snapshot fails. The sweep retires `worker`
    /// and clears its opt-in; `bystander` stays.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_restart_retires_an_opted_in_worker_no_live_session_serves() {
        const MOB_ID: &str = "restart-not-live";
        let temp = tempfile::tempdir().expect("tempdir");
        let metadata_path = temp.path().join("metadata.sqlite3");
        let (open_gate, gate) = tokio::sync::watch::channel(false);
        let first = boot_restartable_sweeping_runtime(
            MOB_ID,
            temp.path(),
            3_600_000,
            Arc::new(GatedLlmClient { gate }),
        )
        .await;
        let handle = first.mob_handle();
        // Autonomous: the worker's kickoff turn starts at once and holds on
        // the gate.
        let worker_session = seat(&handle, "worker").await;
        await_member_run_open(&handle, &AgentIdentity::from("worker")).await;
        assert_eq!(
            first
                .mob_runtime
                .session_owes_durable_input(&worker_session)
                .await,
            Some(true),
            "the store read does not find the held kickoff turn's input under the worker's session"
        );
        open_gate.send(true).expect("open the gate");
        tokio::time::timeout(Duration::from_secs(30), async {
            while first
                .mob_runtime
                .session_owes_durable_input(&worker_session)
                .await
                != Some(false)
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the worker's input stayed open after its turn ended");
        seat(&handle, "bystander").await;
        first
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides")
            .set(MOB_ID, "worker", DelegateIdleRetireOverride::Seconds(1))
            .await;
        assert_eq!(persisted_members(&metadata_path).await, vec!["worker"]);
        drop(handle);
        first.shutdown().await;
        drop(first);

        let second = boot_restartable_sweeping_runtime(
            MOB_ID,
            temp.path(),
            1_000,
            Arc::new(meerkat_client::TestClient::default()),
        )
        .await;
        let handle = second.mob_handle();
        let service = second
            .mob_runtime
            .agent_mob_mcp_state()
            .expect("agent mob tool state")
            .session_service();
        let worker = AgentIdentity::from("worker");
        assert_eq!(
            handle.resolve_bridge_session_id(&worker).await,
            Some(worker_session.clone()),
            "the restored worker keeps its session"
        );
        assert!(
            matches!(
                service.live_session_actor_registered(&worker_session).await,
                Ok(false)
            ),
            "no live actor serves the restored worker's session"
        );
        assert!(
            matches!(
                service
                    .load_persisted_session_metadata(&worker_session)
                    .await,
                Ok(Some(_))
            ),
            "this process's store holds the restored worker's session"
        );
        tokio::time::timeout(Duration::from_secs(30), async {
            while is_live(&handle, "worker").await {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("the sweep never retired the restored worker no live session serves");
        assert!(
            is_live(&handle, "bystander").await,
            "a member without an opt-in stays"
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while !persisted_members(&metadata_path).await.is_empty() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the retired worker's opt-in was not cleared");
        drop(handle);
        second.shutdown().await;
    }

    /// C1/C5: a failed execution snapshot is never idle by its error alone.
    /// A session this process's store does not hold (a placed member's
    /// session is resident on its host), a session a live actor serves, and a
    /// live-actor read that fails all stay unreadable.
    #[tokio::test]
    async fn a_failed_snapshot_of_a_session_not_held_here_or_live_is_never_idle() {
        for (live, persisted, case) in [
            (
                Some(false),
                false,
                "a session this process's store does not hold",
            ),
            (Some(true), true, "a session a live actor serves"),
            (None, true, "a live-actor read that fails"),
        ] {
            let service = crate::mob_handle_runtime::failed_snapshot_probe(live, persisted);
            let observed =
                observe_member_execution(service.as_ref(), &meerkat_core::SessionId::new()).await;
            assert!(
                matches!(
                    observed,
                    MemberExecution::Unreadable(meerkat_core::SessionError::NotFound { .. })
                ),
                "{case} must stay unreadable"
            );
        }
    }

    /// C1: a failed execution snapshot of a session this process's store
    /// holds and no live actor serves reads not live, so the owed-work check
    /// decides it.
    #[tokio::test]
    async fn a_failed_snapshot_of_a_held_session_no_live_actor_serves_reads_not_live() {
        let service = crate::mob_handle_runtime::failed_snapshot_probe(Some(false), true);
        assert!(matches!(
            observe_member_execution(service.as_ref(), &meerkat_core::SessionId::new()).await,
            MemberExecution::NotLive(meerkat_core::SessionError::NotFound { .. })
        ));
    }

    /// C2: a member no live session serves owes work while its kickoff has
    /// not resolved, while an input of its session is open, and when the
    /// input read did not answer. Only a resolved kickoff, or none, with no
    /// open input leaves it idle.
    #[test]
    fn a_member_no_live_session_serves_owes_work_while_its_kickoff_or_an_input_is_open() {
        use MobMemberKickoffPhase as Phase;
        assert!(!owes_work_while_not_live(None, Some(false)));
        for resolved in [Phase::Started, Phase::Failed, Phase::Cancelled] {
            assert!(
                !owes_work_while_not_live(Some(resolved), Some(false)),
                "{resolved:?}"
            );
        }
        for pending in [Phase::Pending, Phase::Starting, Phase::CallbackPending] {
            assert!(
                owes_work_while_not_live(Some(pending), Some(false)),
                "{pending:?}"
            );
        }
        assert!(owes_work_while_not_live(None, Some(true)), "an open input");
        assert!(
            owes_work_while_not_live(None, None),
            "an input read that did not answer"
        );
    }

    /// C2: a session's durable inputs owe work while any row is short of a
    /// terminal phase; a row that does not decode is inconclusive.
    #[test]
    fn durable_inputs_owe_work_until_every_row_is_terminal() {
        use crate::mob_handle_runtime::durable_inputs_owe_work;
        use meerkat_runtime::input_state::{InputLifecycleState as Phase, StoredInputState};
        use meerkat_runtime::store::InputStateRow;
        let row = |phase| {
            let mut state = StoredInputState::new_accepted(meerkat_core::lifecycle::InputId::new());
            state.seed.phase = phase;
            InputStateRow::Decoded(Box::new(state))
        };
        let terminal = [
            Phase::Consumed,
            Phase::Superseded,
            Phase::Coalesced,
            Phase::Abandoned,
        ];
        assert_eq!(durable_inputs_owe_work(&[]), Some(false));
        assert_eq!(durable_inputs_owe_work(&terminal.map(row)), Some(false));
        for open in [
            Phase::Accepted,
            Phase::Queued,
            Phase::Staged,
            Phase::Applied,
            Phase::AppliedPendingConsumption,
        ] {
            let mut rows = terminal.map(row).to_vec();
            rows.push(row(open));
            assert_eq!(durable_inputs_owe_work(&rows), Some(true), "{open:?}");
        }
        let corrupt = InputStateRow::Corrupt {
            input_id: "unreadable".to_string(),
            detail: "does not decode".to_string(),
        };
        assert_eq!(
            durable_inputs_owe_work(&[row(Phase::Consumed), corrupt]),
            None
        );
    }

    /// C4: an idle clock belongs to the session it started on. Pass 1 starts
    /// the worker's clock; pass 2 is held while the worker is re-seated under
    /// a new session, until the old clock is past the window. Pass 2 must
    /// start a fresh window for the new session, so pass 3 still finds the
    /// worker seated.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_member_reseated_inside_its_window_is_not_retired_on_the_old_clock() {
        const MOB_ID: &str = "reseated-inside-window";
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = boot_sweeping_runtime(
            MOB_ID,
            &temp.path().join("state"),
            &temp.path().join("metadata.sqlite3"),
            1_000,
        )
        .await;
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let (mut reached, release) = pause_sweep(MOB_ID);
        let first = seat(&handle, "worker").await;
        overrides
            .set(MOB_ID, "worker", DelegateIdleRetireOverride::Seconds(3))
            .await;
        let pass = Duration::from_secs(30);
        // Pass 1 sees the worker idle and starts its clock.
        tokio::time::timeout(pass, reached.recv())
            .await
            .expect("pass 1");
        release.send(()).expect("release pass 1");
        // Pass 2 is held while the worker is re-seated under a new session.
        tokio::time::timeout(pass, reached.recv())
            .await
            .expect("pass 2");
        let member = AgentIdentity::from("worker");
        crate::mob_handle_runtime::respawn_carrying_idle_retire_opt_in(
            Some(&overrides),
            &handle,
            &member,
            handle.respawn(member.clone(), None),
            Result::is_ok,
        )
        .await
        .expect("respawn");
        let second = handle
            .resolve_bridge_session_id(&member)
            .await
            .expect("the re-seated worker's session");
        assert_ne!(second, first, "a respawn mints a new session");
        // Past the window on the old clock, well inside it on a new one.
        tokio::time::sleep(Duration::from_secs(4)).await;
        release.send(()).expect("release pass 2");
        // Pass 3 starts only after pass 2 finished.
        tokio::time::timeout(pass, reached.recv())
            .await
            .expect("pass 3");
        assert!(
            is_live(&handle, "worker").await,
            "pass 2 retired the re-seated worker on its old session's clock"
        );
        assert_eq!(
            handle.resolve_bridge_session_id(&member).await,
            Some(second)
        );
        unpause_sweep(MOB_ID);
        drop(release);
        runtime.shutdown().await;
    }

    /// C4: retire is member-addressed, so the sweep reads the member's session
    /// again right before it: a member re-seated between the pass's read and
    /// its retire is not the member the clock timed. Here the retire is held
    /// while the worker is re-seated; the pass must leave the new incarnation
    /// seated, and the next pass finds it there.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_member_reseated_before_its_retire_is_not_retired_on_the_old_session() {
        const MOB_ID: &str = "reseated-before-retire";
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = boot_sweeping_runtime(
            MOB_ID,
            &temp.path().join("state"),
            &temp.path().join("metadata.sqlite3"),
            1_000,
        )
        .await;
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let (mut retiring, release_retire) = pause_retire(MOB_ID);
        let first = seat(&handle, "worker").await;
        overrides
            .set(MOB_ID, "worker", DelegateIdleRetireOverride::Seconds(0))
            .await;
        let wait = Duration::from_secs(30);
        let held = tokio::time::timeout(wait, retiring.recv())
            .await
            .expect("the sweep reached the worker's retire")
            .expect("a held retire");
        assert_eq!(held[0].0.as_str(), "worker");
        let member = AgentIdentity::from("worker");
        crate::mob_handle_runtime::respawn_carrying_idle_retire_opt_in(
            Some(&overrides),
            &handle,
            &member,
            handle.respawn(member.clone(), None),
            Result::is_ok,
        )
        .await
        .expect("respawn");
        let second = handle
            .resolve_bridge_session_id(&member)
            .await
            .expect("the re-seated worker's session");
        assert_ne!(second, first, "a respawn mints a new session");
        // The next pass is held right after its snapshot: by then the held
        // pass has finished.
        let (mut reached, release) = pause_sweep(MOB_ID);
        release_retire.send(()).expect("release the held retire");
        tokio::time::timeout(wait, reached.recv())
            .await
            .expect("the next pass");
        assert!(
            is_live(&handle, "worker").await,
            "the held pass retired the worker re-seated under a new session"
        );
        assert_eq!(
            handle.resolve_bridge_session_id(&member).await,
            Some(second)
        );
        unpause_retire(MOB_ID);
        drop(release_retire);
        unpause_sweep(MOB_ID);
        drop(release);
        runtime.shutdown().await;
    }

    /// `source` forks turn-driven `child` from its own turn; the child's
    /// turn holds on the runtime's gate.
    async fn fork_held_child(
        handle: &meerkat_mob::MobHandle,
        source: &AgentIdentity,
        child: &AgentIdentity,
    ) -> meerkat_mob::ForkChildRun {
        let mut fork = meerkat_mob::SpawnMemberSpec::new(
            meerkat_mob::ProfileName::from("worker"),
            child.clone(),
        );
        fork.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
        fork.initial_message = Some(meerkat_core::ContentInput::Text("long work".to_string()));
        let (_forked, run) = handle
            .fork_member_then_run_detached(
                source,
                fork,
                None,
                "fork_result",
                256,
                meerkat_core::DurableForkSourceAdmission::CallerTurn,
                None,
                None,
            )
            .await
            .expect("fork the child");
        run
    }

    /// Review of #448: the sweep once exempted a child it saw retiring in the
    /// pass's member snapshot, by identity, while it checked the forker
    /// against a fresh roster. An admitted respawn of the child shows its
    /// predecessor as retiring to that snapshot, then seats the successor
    /// under the same identity and `spawned_by` before the forker is
    /// checked: the live successor was exempted, and retiring the forker
    /// cascaded into it. Here the pass is held right after its snapshot
    /// while the respawn completes: the forker and the successor both stay.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_child_respawned_mid_pass_keeps_its_forker() {
        const MOB_ID: &str = "respawned-mid-pass";
        let temp = tempfile::tempdir().expect("tempdir");
        let (open_gate, gate) = tokio::sync::watch::channel(false);
        let runtime = boot_gated_persistent_runtime(MOB_ID, temp.path(), gate).await;
        let handle = runtime.mob_handle();
        let overrides = runtime
            .mob_runtime
            .implicit_delegate_retirement_overrides()
            .expect("overrides");
        let c = AgentIdentity::from("c");
        let d = AgentIdentity::from("d");
        seat_turn_driven(&handle, "c").await;

        // C forks D, whose turn holds. The fork returns once D's turn input is
        // admitted, not once its run is open: a retire that finds the input
        // still queued has no run to cancel, the queued turn then opens and
        // holds, and the retire cannot reach its turn-finalization boundary.
        // Wait for D's run to be open (typed status) before the respawn.
        let _run = fork_held_child(&handle, &c, &d).await;
        await_member_run_open(&handle, &d).await;
        let predecessor = handle
            .get_member(&d)
            .await
            .expect("roster read")
            .expect("d is seated")
            .agent_runtime_id;
        // A respawn of D is admitted and waits out meerkat's cooperative
        // grace (about 2s) with the predecessor retiring.
        let (mut reached, release) = pause_sweep(MOB_ID);
        let mut changes = handle.machine_state_changes();
        let respawn = tokio::spawn({
            let handle = handle.clone();
            let d = d.clone();
            async move { handle.respawn(d, None).await }
        });
        let retiring = async {
            while !handle
                .list_members_including_retiring()
                .await
                .iter()
                .any(|member| {
                    member.agent_identity == d && member.status == MobMemberStatus::Retiring
                })
            {
                assert!(
                    changes.changed().await.is_ok(),
                    "the mob actor stopped before d's predecessor was retiring"
                );
            }
        };
        assert!(
            tokio::time::timeout(Duration::from_secs(30), retiring)
                .await
                .is_ok(),
            "d's predecessor did not start retiring within 30 s of the respawn"
        );

        // Every pass reports the snapshot it took. Release the passes whose
        // snapshot found D live (they leave C alone) and hold the first whose
        // snapshot shows the predecessor retiring. While it is held, the
        // respawn seats the successor, and C falls due at once.
        loop {
            let snapshot = reached.recv().await.expect("a sweep pass");
            if snapshot
                .iter()
                .any(|(member, status)| member == &d && *status == MobMemberStatus::Retiring)
            {
                break;
            }
            release.send(()).expect("release a pass that found d live");
        }
        assert!(
            !respawn.is_finished(),
            "the respawn must still be retiring d when the held pass takes its snapshot"
        );
        respawn
            .await
            .expect("respawn task")
            .expect("the respawn seats d's successor");
        let successor = handle
            .get_member(&d)
            .await
            .expect("roster read")
            .expect("d's successor is seated");
        assert_ne!(successor.agent_runtime_id, predecessor);
        assert_eq!(successor.spawned_by, Some(c.clone()));
        overrides
            .set(MOB_ID, "c", DelegateIdleRetireOverride::Seconds(0))
            .await;

        // The held pass resumes, then later passes run. A pass retires inline,
        // so the next pass reaching the pause proves the previous one finished
        // every decision it made: the held pass and two full passes after it.
        for _ in 0..3 {
            release.send(()).expect("release the held pass");
            reached.recv().await.expect("the next sweep pass");
        }
        unpause_sweep(MOB_ID);
        drop(release);
        assert!(
            is_live(&handle, "d").await,
            "d's successor was killed by retiring c"
        );
        assert!(
            is_live(&handle, "c").await,
            "c was idle-retired while it owns d's successor"
        );
        open_gate.send(true).expect("open the gate");
        runtime.shutdown().await;
    }
}
