//! Durable cleanup for ordinary placed `SubmitWork(TurnCompleted)` calls.
//!
//! The caller promise is deliberately absent from this worker.  MobMachine
//! custody survives caller drop, timeout, and controller restart; the sole
//! member-event pump owns terminal folding and ACK confirmation.  The one
//! alternate terminal authority is the authenticated, exact
//! `CancelTrackedMemberInput::Terminal` receipt, whose wire contract carries
//! the retained host record verbatim.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use meerkat_core::time_compat::{Duration, Instant};

use crate::error::MobError;
use crate::event::{MemberRef, PlacedCompletionClosureEvent};
use crate::ids::{AgentIdentity, MobId};
use crate::machines::mob_machine as mob_dsl;
#[cfg(target_arch = "wasm32")]
use crate::tokio;

use super::handle::MobHandle;
use super::provisioner::MobProvisioner;

const SCAN_INTERVAL: Duration = Duration::from_millis(100);
const MAX_SCAN_INTERVAL: Duration = Duration::from_secs(5);
const MAX_CONCURRENT_HOSTS: usize = 8;

#[derive(Debug)]
struct RetryState {
    delay: Duration,
    next_attempt: Instant,
}

#[cfg(test)]
mod tests {
    use super::MAX_CONCURRENT_HOSTS;

    #[test]
    fn cleanup_host_window_is_bounded() {
        assert_eq!(MAX_CONCURRENT_HOSTS, 8);
    }
}

impl RetryState {
    fn due(&self, now: Instant) -> bool {
        now >= self.next_attempt
    }

    fn record_attempt(&mut self, now: Instant) {
        self.next_attempt = now + self.delay;
        self.delay = (self.delay * 2).min(MAX_SCAN_INTERVAL);
    }
}

impl Default for RetryState {
    fn default() -> Self {
        Self {
            delay: SCAN_INTERVAL,
            next_attempt: Instant::now(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttemptOutcome {
    /// The exact machine custody row disappeared before the attempt ran.
    Gone,
    /// The pump was poked, but no synchronous semantic transition occurred.
    Poked,
    /// CancelTrackedInput produced and committed an exact terminal transition.
    Progress,
}

struct ScanSummary {
    live: usize,
    progress: usize,
    next_retry_after: Option<Duration>,
}

pub(crate) fn obligation_event(
    obligation: &mob_dsl::PlacedCompletionObligation,
) -> Result<crate::event::PlacedCompletionObligationEvent, MobError> {
    let parsed = uuid::Uuid::parse_str(&obligation.input_id.0)
        .map_err(|_| MobError::Internal("placed completion input_id is not a UUID".to_string()))?;
    if parsed.is_nil() || parsed.to_string() != obligation.input_id.0 {
        return Err(MobError::Internal(
            "placed completion input_id must be a canonical non-nil UUID".to_string(),
        ));
    }
    Ok(crate::event::PlacedCompletionObligationEvent {
        agent_identity: AgentIdentity::from(obligation.agent_identity.0.as_str()),
        host_id: obligation.host_id.0.clone(),
        host_binding_generation: obligation.host_binding_generation,
        member_session_id: obligation.member_session_id.0.clone(),
        generation: crate::ids::Generation::new(obligation.generation.0),
        fence_token: crate::ids::FenceToken::new(obligation.fence_token.0),
        dispatch_sequence: obligation.dispatch_sequence,
        input_id: obligation.input_id.0.clone(),
    })
}

pub(crate) fn obligation_from_event(
    event: &crate::event::PlacedCompletionObligationEvent,
) -> Result<mob_dsl::PlacedCompletionObligation, MobError> {
    let parsed = uuid::Uuid::parse_str(&event.input_id).map_err(|_| {
        MobError::InvalidPlacedInteractionId {
            interaction_id: event.input_id.clone(),
        }
    })?;
    if parsed.is_nil() || parsed.to_string() != event.input_id || event.dispatch_sequence == 0 {
        return Err(MobError::InvalidPlacedInteractionId {
            interaction_id: event.input_id.clone(),
        });
    }
    Ok(mob_dsl::PlacedCompletionObligation {
        agent_identity: mob_dsl::AgentIdentity::from_domain(&event.agent_identity),
        host_id: mob_dsl::HostId(event.host_id.clone()),
        host_binding_generation: event.host_binding_generation,
        member_session_id: mob_dsl::SessionId(event.member_session_id.clone()),
        generation: mob_dsl::Generation(event.generation.get()),
        fence_token: mob_dsl::FenceToken(event.fence_token.get()),
        dispatch_sequence: event.dispatch_sequence,
        input_id: mob_dsl::InputId(event.input_id.clone()),
    })
}

fn exact_route_is_current(
    state: &mob_dsl::MobMachineState,
    obligation: &mob_dsl::PlacedCompletionObligation,
) -> bool {
    let identity = &obligation.agent_identity;
    let host = &obligation.host_id;
    state.member_placement.get(identity) == Some(host)
        && state
            .current_placed_spawn_host_binding_generations
            .get(identity)
            == Some(&obligation.host_binding_generation)
        && state.host_binding_generations.get(host) == Some(&obligation.host_binding_generation)
        && state.member_session_bindings.get(identity) == Some(&obligation.member_session_id)
        && state.identity_runtime_generations.get(identity) == Some(&obligation.generation)
        && state.identity_runtime_fence_tokens.get(identity) == Some(&obligation.fence_token)
        && state.host_durable_sessions.get(host) == Some(&true)
        && state.host_tracked_input_cancel.get(host) == Some(&true)
        && state
            .host_protocol_min
            .get(host)
            .is_some_and(|minimum| *minimum <= 4)
        && state
            .host_protocol_max
            .get(host)
            .is_some_and(|maximum| *maximum >= 4)
}

fn expected_member(
    mob_id: &MobId,
    obligation: &mob_dsl::PlacedCompletionObligation,
) -> super::bridge_protocol::BridgeMemberIncarnation {
    super::bridge_protocol::BridgeMemberIncarnation {
        mob_id: mob_id.to_string(),
        agent_identity: obligation.agent_identity.0.clone(),
        host_id: obligation.host_id.0.clone(),
        binding_generation: obligation.host_binding_generation,
        member_session_id: obligation.member_session_id.0.clone(),
        generation: obligation.generation.0,
        fence_token: obligation.fence_token.0,
    }
}

/// Actor-lifetime adopter for completion cleanup custody.
pub(crate) struct PlacedCompletionReconciler {
    mob_id: MobId,
    handle: MobHandle,
    provisioner: Arc<dyn MobProvisioner>,
}

impl PlacedCompletionReconciler {
    pub(crate) async fn run_owned(
        mob_id: MobId,
        handle: MobHandle,
        provisioner: Arc<dyn MobProvisioner>,
    ) {
        Self {
            mob_id,
            handle,
            provisioner,
        }
        .run()
        .await;
    }

    async fn run(self) {
        let actor_closed = self.handle.command_tx.clone();
        let mut delay = SCAN_INTERVAL;
        let mut retry = BTreeMap::<mob_dsl::PlacedCompletionObligation, RetryState>::new();
        let mut row_cursor = BTreeMap::<mob_dsl::HostId, u64>::new();
        let mut host_cursor = 0usize;
        loop {
            tokio::select! {
                () = actor_closed.closed() => break,
                () = tokio::time::sleep(delay) => {},
            }
            match self
                .scan_once(&mut retry, &mut row_cursor, &mut host_cursor)
                .await
            {
                Ok(summary) if summary.live == 0 => {
                    delay = (delay * 2).min(MAX_SCAN_INTERVAL);
                }
                Ok(summary) if summary.progress > 0 => delay = SCAN_INTERVAL,
                Ok(summary) => {
                    delay = summary
                        .next_retry_after
                        .unwrap_or(MAX_SCAN_INTERVAL)
                        .clamp(SCAN_INTERVAL, MAX_SCAN_INTERVAL);
                }
                Err(error) => {
                    tracing::debug!(
                        mob_id = %self.mob_id,
                        error = %error,
                        "placed completion cleanup remains pending"
                    );
                    delay = (delay * 2).min(MAX_SCAN_INTERVAL);
                }
            }
        }
    }

    async fn scan_once(
        &self,
        retry: &mut BTreeMap<mob_dsl::PlacedCompletionObligation, RetryState>,
        row_cursor: &mut BTreeMap<mob_dsl::HostId, u64>,
        host_cursor: &mut usize,
    ) -> Result<ScanSummary, MobError> {
        let state = self.handle.machine_state_watch_rx.borrow().clone();
        let pending = state
            .cancel_requested_placed_completion_outcomes
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let resolved = state
            .resolved_placed_completion_outcomes
            .iter()
            .cloned()
            .collect::<Vec<_>>();

        let live = pending
            .iter()
            .chain(&resolved)
            .cloned()
            .collect::<BTreeSet<_>>();
        retry.retain(|obligation, _| live.contains(obligation));
        for obligation in &live {
            retry.entry(obligation.clone()).or_default();
        }

        // Every host contributes at most one due row per round. The per-host
        // sequence cursor prevents one permanently blackholed low sequence
        // from monopolizing its siblings, while the global cursor rotates the
        // first host admitted to the bounded concurrency window.
        let mut by_host =
            BTreeMap::<mob_dsl::HostId, Vec<mob_dsl::PlacedCompletionObligation>>::new();
        for obligation in live.iter().cloned() {
            by_host
                .entry(obligation.host_id.clone())
                .or_default()
                .push(obligation);
        }
        for obligations in by_host.values_mut() {
            obligations.sort_by_key(|obligation| obligation.dispatch_sequence);
        }
        row_cursor.retain(|host, _| by_host.contains_key(host));

        let now = Instant::now();
        let mut hosts = by_host.keys().cloned().collect::<Vec<_>>();
        let mut rotation = 0usize;
        if !hosts.is_empty() {
            rotation = *host_cursor % hosts.len();
            hosts.rotate_left(rotation);
        }
        let mut selected = Vec::new();
        let mut last_selected_offset = None;
        for (host_offset, host) in hosts.iter().enumerate() {
            let Some(obligations) = by_host.get(host) else {
                continue;
            };
            let previous = row_cursor.get(host).copied().unwrap_or(0);
            let due = obligations
                .iter()
                .filter(|obligation| retry.get(*obligation).is_some_and(|retry| retry.due(now)))
                .find(|obligation| obligation.dispatch_sequence > previous)
                .or_else(|| {
                    obligations.iter().find(|obligation| {
                        retry.get(*obligation).is_some_and(|retry| retry.due(now))
                    })
                });
            let Some(obligation) = due.cloned() else {
                continue;
            };
            let Some(retry_state) = retry.get_mut(&obligation) else {
                continue;
            };
            retry_state.record_attempt(now);
            row_cursor.insert(host.clone(), obligation.dispatch_sequence);
            selected.push(obligation);
            last_selected_offset = Some(host_offset);
            if selected.len() == MAX_CONCURRENT_HOSTS {
                break;
            }
        }
        if !hosts.is_empty() {
            // Advance past the last host actually admitted, including any
            // skipped not-due hosts. Advancing by one would overlap most of a
            // full window and make a high-index healthy host wait behind each
            // blackholed timeout almost serially.
            *host_cursor =
                super::actor::advance_rotating_cursor(hosts.len(), rotation, last_selected_offset);
        }

        let mut attempts = FuturesUnordered::new();
        for obligation in selected {
            attempts.push(async move {
                let result = self.reconcile_once(&obligation).await;
                (obligation, result)
            });
        }
        let mut progress = 0usize;
        while let Some((obligation, result)) = attempts.next().await {
            match result {
                Ok(AttemptOutcome::Progress) => {
                    progress += 1;
                    retry.remove(&obligation);
                }
                Ok(AttemptOutcome::Gone) => {
                    retry.remove(&obligation);
                }
                Ok(AttemptOutcome::Poked) => {}
                Err(error) => {
                    tracing::debug!(
                        host_id = %obligation.host_id.0,
                        agent_identity = %obligation.agent_identity.0,
                        input_id = %obligation.input_id.0,
                        error = %error,
                        "placed completion reconciliation remains pending and will retry"
                    );
                }
            }
        }

        let now = Instant::now();
        let next_retry_after = retry
            .values()
            .map(|state| state.next_attempt.saturating_duration_since(now))
            .min();
        Ok(ScanSummary {
            live: live.len(),
            progress,
            next_retry_after,
        })
    }

    async fn reconcile_once(
        &self,
        obligation: &mob_dsl::PlacedCompletionObligation,
    ) -> Result<AttemptOutcome, MobError> {
        let state = self.handle.machine_state_watch_rx.borrow().clone();
        let pending_cancel = state
            .cancel_requested_placed_completion_outcomes
            .contains(obligation);
        let resolved = state
            .resolved_placed_completion_outcomes
            .contains(obligation);
        if !pending_cancel && !resolved {
            return Ok(AttemptOutcome::Gone);
        }
        if !exact_route_is_current(&state, obligation) {
            return Err(MobError::Internal(format!(
                "placed completion '{}' retains machine custody without its exact current route",
                obligation.input_id.0
            )));
        }
        let identity = AgentIdentity::from(obligation.agent_identity.0.as_str());
        self.handle.ensure_pump_for_obligation(&identity).await?;
        if resolved {
            // The one sequential pump owns ACK resend and confirmation. A
            // successful ensure is a poke, not semantic progress; backoff is
            // retained until the watch state proves ACK or release disposal.
            return Ok(AttemptOutcome::Poked);
        }
        let Some(entry) = self.handle.roster.read().await.get(&identity).cloned() else {
            return Err(MobError::Internal(format!(
                "placed completion '{}' retains machine custody without a roster entry",
                obligation.input_id.0
            )));
        };
        if entry.generation.get() != obligation.generation.0
            || entry.fence_token.get() != obligation.fence_token.0
        {
            return Err(MobError::Internal(format!(
                "placed completion '{}' roster incarnation is stale",
                obligation.input_id.0
            )));
        }
        let member_ref = match &entry.member_ref {
            MemberRef::BackendPeer {
                session_id: Some(session_id),
                ..
            } if session_id.to_string() != obligation.member_session_id.0 => {
                return Err(MobError::Internal(format!(
                    "placed completion '{}' roster route targets a stale member session",
                    obligation.input_id.0
                )));
            }
            MemberRef::BackendPeer { .. } => entry.member_ref.clone(),
            MemberRef::Session { .. } => {
                return Err(MobError::Internal(
                    "placed completion custody has a local session route".to_string(),
                ));
            }
        };
        let response = self
            .provisioner
            .cancel_tracked_placed_input(
                &member_ref,
                &expected_member(&self.mob_id, obligation),
                &obligation.input_id.0,
            )
            .await?;
        let event = obligation_event(obligation)?;
        match response.outcome {
            super::bridge_protocol::BridgeTrackedInputCancelOutcome::NoEffect => {
                self.handle
                    .close_placed_completion_outcome(
                        event,
                        PlacedCompletionClosureEvent::HostNoEffect,
                    )
                    .await?;
            }
            super::bridge_protocol::BridgeTrackedInputCancelOutcome::Cancelled => {
                self.handle
                    .close_placed_completion_outcome(
                        event,
                        PlacedCompletionClosureEvent::HostCancelled,
                    )
                    .await?;
            }
            super::bridge_protocol::BridgeTrackedInputCancelOutcome::Terminal { record } => {
                // The bridge validator authenticated the exact residency,
                // input id, generation, and fence.  Persist machine Resolve
                // before the pump is allowed to resolve a waiter or queue ACK.
                self.handle
                    .resolve_placed_completion_outcome(event, record)
                    .await?;
            }
            _ => {
                return Err(MobError::Internal(
                    "member host returned an unsupported tracked-input cancellation outcome"
                        .to_string(),
                ));
            }
        }
        Ok(AttemptOutcome::Progress)
    }
}
