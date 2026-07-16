//! Durable placed-autonomous kickoff replay.
//!
//! The committed placed-spawn carrier owns the exact first-turn payload. The
//! MobMachine owns whether that turn is still Pending or has entered
//! Resolved/ACK-pending custody. This worker joins those two authorities and
//! may therefore only resend the carrier's original input id, prompt,
//! objective, handling mode, and injected context to the exact current
//! residency. An authenticated pre-admission rejection is terminal no-effect;
//! every transport failure remains ambiguous and is retried with the same id.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use meerkat_core::time_compat::{Duration, Instant};

use crate::error::MobError;
use crate::event::MemberRef;
use crate::ids::{AgentIdentity, MobId};
use crate::machines::mob_machine as mob_dsl;
use crate::store::{MobPlacedSpawnCarrierRecord, MobRuntimeMetadataStore, PlacedSpawnCarrierPhase};
#[cfg(target_arch = "wasm32")]
use crate::tokio;

use super::handle::MobHandle;
use super::provisioner::{MobProvisioner, PlacedTurnDeliveryContext};

const RECONCILE_SCAN_INTERVAL: Duration = Duration::from_millis(100);
const RECONCILE_MAX_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_CONCURRENT_HOSTS: usize = 8;
pub(crate) const TRACKED_INPUT_CANCEL_NO_EFFECT: &str =
    "member host durably cancelled tracked kickoff before runtime admission";

#[derive(Debug)]
struct RetryState {
    delay: Duration,
    next_attempt: Instant,
}

impl RetryState {
    fn due(&self, now: Instant) -> bool {
        now >= self.next_attempt
    }

    fn record_attempt(&mut self, now: Instant) {
        self.next_attempt = now + self.delay;
        self.delay = (self.delay * 2).min(RECONCILE_MAX_RETRY_DELAY);
    }
}

impl Default for RetryState {
    fn default() -> Self {
        Self {
            delay: RECONCILE_SCAN_INTERVAL,
            next_attempt: Instant::now(),
        }
    }
}

#[derive(Debug, Clone)]
enum DeliveryDisposition {
    Accepted,
    CertifiedNoEffect(String),
}

#[derive(Debug)]
enum PendingAttempt {
    NoChange,
    DeliveryAccepted,
    CertifiedNoEffect(String),
}

struct ScanSummary {
    live: usize,
    next_retry_after: Option<Duration>,
}

/// Build the one public/machine custody tuple derivable from a committed
/// autonomous carrier. Keeping this conversion shared with builder recovery
/// prevents replay from accepting a merely same-identity event.
pub(crate) fn obligation_event_from_carrier(
    carrier: &MobPlacedSpawnCarrierRecord,
) -> Result<crate::event::PlacedKickoffObligationEvent, MobError> {
    carrier.validate().map_err(MobError::from)?;
    let PlacedSpawnCarrierPhase::Committed(committed) = &carrier.phase else {
        return Err(MobError::Internal(format!(
            "placed kickoff carrier for '{}' is not committed",
            carrier.agent_identity
        )));
    };
    let intent = carrier.kickoff_intent.as_ref().ok_or_else(|| {
        MobError::Internal(format!(
            "committed autonomous carrier for '{}' has no kickoff intent",
            carrier.agent_identity
        ))
    })?;
    Ok(crate::event::PlacedKickoffObligationEvent {
        agent_identity: AgentIdentity::from(carrier.agent_identity.as_str()),
        host_id: carrier.host_id.to_string(),
        host_binding_generation: carrier.host_binding_generation,
        member_session_id: committed.member_session_id.to_string(),
        generation: crate::ids::Generation::new(carrier.generation),
        fence_token: crate::ids::FenceToken::new(carrier.fence_token),
        input_id: intent.input_id.clone(),
        objective_id: intent.objective_id,
    })
}

fn exact_cancel_route_is_current(
    state: &mob_dsl::MobMachineState,
    obligation: &mob_dsl::PlacedKickoffObligation,
) -> bool {
    let identity = &obligation.agent_identity;
    let host = &obligation.host_id;
    state.placed_carrier_binding_active_for_identity(identity)
        && state.member_placement.get(identity) == Some(host)
        && state
            .current_placed_spawn_host_binding_generations
            .get(identity)
            == Some(&obligation.host_binding_generation)
        && state.host_binding_generations.get(host) == Some(&obligation.host_binding_generation)
        && state.member_session_bindings.get(identity) == Some(&obligation.member_session_id)
        && state.identity_runtime_generations.get(identity) == Some(&obligation.generation)
        && state.identity_runtime_fence_tokens.get(identity) == Some(&obligation.fence_token)
        && state.member_kickoff_objective_ids.get(identity) == Some(&obligation.objective_id)
        && state.member_kickoff_input_ids.get(identity) == Some(&obligation.input_id)
        && state.member_runtime_modes.get(identity)
            == Some(&mob_dsl::SpawnPolicyRuntimeMode::AutonomousHost)
        && state.host_durable_sessions.get(host) == Some(&true)
        && state.host_autonomous_members.get(host) == Some(&true)
        && state.host_tracked_input_cancel.get(host) == Some(&true)
        && state
            .host_protocol_min
            .get(host)
            .is_some_and(|minimum| *minimum <= 4)
        && state
            .host_protocol_max
            .get(host)
            .is_some_and(|maximum| *maximum >= 4)
        && state
            .identity_to_runtime
            .get(identity)
            .is_some_and(|runtime_id| state.live_runtime_ids.contains(runtime_id))
}

fn exact_route_is_current(
    state: &mob_dsl::MobMachineState,
    obligation: &mob_dsl::PlacedKickoffObligation,
) -> bool {
    let identity = &obligation.agent_identity;
    !state.destroy_admitted
        && exact_cancel_route_is_current(state, obligation)
        && state
            .identity_to_runtime
            .get(identity)
            .is_some_and(|runtime_id| {
                state.member_state_markers.get(runtime_id)
                    != Some(&mob_dsl::MobMemberState::Retiring)
            })
        && !state.spawn_exec_phase.contains_key(identity)
        && !state.member_revival_pending.contains(identity)
        && !state.member_materialization_failures.contains_key(identity)
}

fn carrier_matches_obligation(
    carrier: &MobPlacedSpawnCarrierRecord,
    obligation: &mob_dsl::PlacedKickoffObligation,
) -> Result<bool, MobError> {
    let expected = obligation_event_from_carrier(carrier)?;
    Ok(super::remote_flow_ticket::placed_kickoff_obligation_from_event(&expected) == *obligation)
}

fn kickoff_phase_allows_origin(
    state: &mob_dsl::MobMachineState,
    obligation: &mob_dsl::PlacedKickoffObligation,
) -> bool {
    !super::actor::lifecycle_origin_fenced(state)
        && state
            .member_kickoff_starting
            .contains(&obligation.agent_identity)
        && !state
            .member_kickoff_cancelled
            .contains(&obligation.agent_identity)
}

fn kickoff_phase_requires_host_cancel(
    state: &mob_dsl::MobMachineState,
    obligation: &mob_dsl::PlacedKickoffObligation,
) -> bool {
    state
        .member_kickoff_cancelled
        .contains(&obligation.agent_identity)
        && state.pending_placed_kickoff_outcomes.contains(obligation)
}

fn resolved_host_cancel_can_close(
    state: &mob_dsl::MobMachineState,
    obligation: &mob_dsl::PlacedKickoffObligation,
) -> bool {
    state.resolved_placed_kickoff_outcomes.contains(obligation)
        && state
            .member_placed_kickoff_outcome_kinds
            .get(&obligation.agent_identity)
            == Some(&mob_dsl::PlacedKickoffOutcomeKind::Cancelled)
}

/// One actor-lifetime adopter for durable placed kickoff custody.
pub(crate) struct PlacedKickoffReconciler {
    mob_id: MobId,
    handle: MobHandle,
    provisioner: Arc<dyn MobProvisioner>,
    runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
}

impl PlacedKickoffReconciler {
    pub(crate) async fn run_owned(
        mob_id: MobId,
        handle: MobHandle,
        provisioner: Arc<dyn MobProvisioner>,
        runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    ) {
        Self {
            mob_id,
            handle,
            provisioner,
            runtime_metadata,
        }
        .run()
        .await;
    }

    async fn run(self) {
        let actor_closed = self.handle.command_tx.clone();
        let mut event_rx = self.handle.events.subscribe().ok();
        let mut retry = BTreeMap::<mob_dsl::PlacedKickoffObligation, RetryState>::new();
        let mut dispositions =
            BTreeMap::<mob_dsl::PlacedKickoffObligation, DeliveryDisposition>::new();
        let mut row_cursor = BTreeMap::<mob_dsl::HostId, mob_dsl::PlacedKickoffObligation>::new();
        let mut host_cursor = 0usize;
        let mut next_scan_after = RECONCILE_SCAN_INTERVAL;

        loop {
            tokio::select! {
                () = actor_closed.closed() => break,
                () = tokio::time::sleep(next_scan_after) => {},
                () = wait_for_event(&mut event_rx) => {},
            }
            match self
                .scan_once(
                    &mut retry,
                    &mut dispositions,
                    &mut row_cursor,
                    &mut host_cursor,
                )
                .await
            {
                Ok(summary) => {
                    next_scan_after = if summary.live == 0 {
                        (next_scan_after * 2).min(RECONCILE_MAX_RETRY_DELAY)
                    } else {
                        summary
                            .next_retry_after
                            .unwrap_or(RECONCILE_MAX_RETRY_DELAY)
                            .clamp(RECONCILE_SCAN_INTERVAL, RECONCILE_MAX_RETRY_DELAY)
                    };
                }
                Err(error) => {
                    tracing::warn!(
                        mob_id = %self.mob_id,
                        error = %error,
                        "placed kickoff reconciliation scan failed; exact replay remains pending"
                    );
                    next_scan_after = (next_scan_after * 2).min(RECONCILE_MAX_RETRY_DELAY);
                }
            }
        }
    }

    async fn cancel_pending_obligation(
        &self,
        obligation: &mob_dsl::PlacedKickoffObligation,
    ) -> Result<Option<super::bridge_protocol::BridgeTrackedInputCancelResponse>, MobError> {
        let carrier = self
            .runtime_metadata
            .load_placed_spawn(&self.mob_id, &obligation.agent_identity.0)
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "cancelled placed kickoff '{}' has no canonical carrier",
                    obligation.agent_identity.0
                ))
            })?;
        carrier.validate_for_mob(&self.mob_id)?;
        if !carrier_matches_obligation(&carrier, obligation)? {
            return Err(MobError::Internal(format!(
                "cancelled placed kickoff '{}' does not exactly match its canonical carrier",
                obligation.agent_identity.0
            )));
        }
        let expected_member = super::bridge_protocol::BridgeMemberIncarnation {
            mob_id: self.mob_id.to_string(),
            agent_identity: obligation.agent_identity.0.clone(),
            host_id: obligation.host_id.0.clone(),
            binding_generation: obligation.host_binding_generation,
            member_session_id: obligation.member_session_id.0.clone(),
            generation: obligation.generation.0,
            fence_token: obligation.fence_token.0,
        };
        let identity = AgentIdentity::from(obligation.agent_identity.0.as_str());
        let current_state = self.handle.machine_state_watch_rx.borrow().clone();
        if !kickoff_phase_requires_host_cancel(&current_state, obligation) {
            return Ok(None);
        }
        if !exact_cancel_route_is_current(&current_state, obligation) {
            return Err(MobError::Internal(format!(
                "placed kickoff cancellation '{}' retains machine custody without its exact current route",
                obligation.input_id.0
            )));
        }
        if current_state
            .host_tracked_input_cancel
            .get(&obligation.host_id)
            != Some(&true)
        {
            return Err(MobError::Internal(format!(
                "placed autonomous kickoff '{}' entered durable custody without tracked_input_cancel host capability",
                obligation.agent_identity.0
            )));
        }
        let entry = self.handle.roster.read().await.get(&identity).cloned();
        let Some(entry) = entry else {
            return Err(MobError::Internal(format!(
                "actionable placed kickoff cancellation for '{}' has no roster route",
                obligation.agent_identity.0
            )));
        };
        if entry.generation.get() != obligation.generation.0
            || entry.fence_token.get() != obligation.fence_token.0
        {
            return Err(MobError::Internal(format!(
                "actionable placed kickoff cancellation for '{}' has roster generation/fence {}/{} but custody requires {}/{}",
                obligation.agent_identity.0,
                entry.generation.get(),
                entry.fence_token.get(),
                obligation.generation.0,
                obligation.fence_token.0,
            )));
        }
        let member_ref = match &entry.member_ref {
            MemberRef::BackendPeer {
                session_id: Some(session_id),
                ..
            } if session_id.to_string() != obligation.member_session_id.0 => {
                return Err(MobError::Internal(format!(
                    "actionable placed kickoff cancellation for '{}' has roster session '{}' but custody requires '{}'",
                    obligation.agent_identity.0, session_id, obligation.member_session_id.0,
                )));
            }
            MemberRef::BackendPeer { .. } => entry.member_ref.clone(),
            MemberRef::Session { .. } => {
                return Err(MobError::Internal(format!(
                    "cancelled placed kickoff '{}' has a local session route",
                    obligation.agent_identity.0
                )));
            }
        };
        self.provisioner
            .cancel_tracked_placed_input(&member_ref, &expected_member, &obligation.input_id.0)
            .await
            .map(Some)
    }

    async fn reconcile_pending_obligation(
        &self,
        obligation: &mob_dsl::PlacedKickoffObligation,
    ) -> Result<PendingAttempt, MobError> {
        let current_state = self.handle.machine_state_watch_rx.borrow().clone();
        if !current_state
            .pending_placed_kickoff_outcomes
            .contains(obligation)
        {
            return Ok(PendingAttempt::NoChange);
        }

        if kickoff_phase_requires_host_cancel(&current_state, obligation) {
            let Some(response) = self.cancel_pending_obligation(obligation).await? else {
                return Ok(PendingAttempt::NoChange);
            };
            let event = super::remote_flow_ticket::placed_kickoff_obligation_event(obligation)?;
            match response.outcome {
                super::bridge_protocol::BridgeTrackedInputCancelOutcome::NoEffect => {
                    self.handle
                        .reject_placed_kickoff_before_admission(
                            event,
                            TRACKED_INPUT_CANCEL_NO_EFFECT.to_string(),
                        )
                        .await?;
                }
                super::bridge_protocol::BridgeTrackedInputCancelOutcome::Cancelled => {
                    self.handle.resolve_placed_kickoff_cancelled(event).await?;
                }
                super::bridge_protocol::BridgeTrackedInputCancelOutcome::Terminal { record } => {
                    self.handle
                        .resolve_placed_kickoff_outcome(event, record)
                        .await?;
                }
                _ => {
                    return Err(MobError::Internal(
                        "member host returned an unsupported tracked-input cancellation outcome"
                            .to_string(),
                    ));
                }
            }
            return Ok(PendingAttempt::NoChange);
        }

        if !kickoff_phase_allows_origin(&current_state, obligation) {
            return Ok(PendingAttempt::NoChange);
        }
        if !exact_route_is_current(&current_state, obligation) {
            return Err(MobError::Internal(format!(
                "placed kickoff origin '{}' retains machine custody without its exact current route",
                obligation.input_id.0
            )));
        }

        let carrier = self
            .runtime_metadata
            .load_placed_spawn(&self.mob_id, &obligation.agent_identity.0)
            .await?
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "pending placed kickoff '{}' has no canonical carrier",
                    obligation.agent_identity.0
                ))
            })?;
        carrier.validate_for_mob(&self.mob_id)?;
        if !carrier_matches_obligation(&carrier, obligation)? {
            return Err(MobError::Internal(format!(
                "pending placed kickoff '{}' does not exactly match its canonical carrier",
                obligation.agent_identity.0
            )));
        }
        let intent = carrier.kickoff_intent.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "pending placed kickoff '{}' lost its exact intent",
                obligation.agent_identity.0
            ))
        })?;
        let request = meerkat_core::service::StartTurnRequest {
            injected_context: intent.injected_context.clone(),
            prompt: intent.prompt.clone(),
            system_prompt: None,
            event_tx: None,
            runtime: meerkat_core::service::StartTurnRuntimeSemantics::new(
                intent.handling_mode,
                None,
                Vec::new(),
                Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        transcript_identity: meerkat_core::types::TranscriptMessageIdentity {
                            interaction_id: None,
                            run_id: None,
                            objective_id: Some(intent.objective_id),
                        },
                        ..Default::default()
                    },
                ),
            ),
        };
        let expected_member = super::bridge_protocol::BridgeMemberIncarnation {
            mob_id: self.mob_id.to_string(),
            agent_identity: obligation.agent_identity.0.clone(),
            host_id: obligation.host_id.0.clone(),
            binding_generation: obligation.host_binding_generation,
            member_session_id: obligation.member_session_id.0.clone(),
            generation: obligation.generation.0,
            fence_token: obligation.fence_token.0,
        };
        let identity = AgentIdentity::from(obligation.agent_identity.0.as_str());
        let current_state = self.handle.machine_state_watch_rx.borrow().clone();
        if !current_state
            .pending_placed_kickoff_outcomes
            .contains(obligation)
            || !kickoff_phase_allows_origin(&current_state, obligation)
        {
            return Ok(PendingAttempt::NoChange);
        }
        if !exact_route_is_current(&current_state, obligation) {
            return Err(MobError::Internal(format!(
                "placed kickoff origin '{}' retained custody but lost its exact route before roster resolution",
                obligation.input_id.0
            )));
        }
        let entry = self.handle.roster.read().await.get(&identity).cloned();
        let Some(entry) = entry else {
            return Err(MobError::Internal(format!(
                "actionable placed kickoff origin for '{}' has no roster route",
                obligation.agent_identity.0
            )));
        };
        if entry.generation.get() != obligation.generation.0
            || entry.fence_token.get() != obligation.fence_token.0
        {
            return Err(MobError::Internal(format!(
                "actionable placed kickoff origin for '{}' has roster generation/fence {}/{} but custody requires {}/{}",
                obligation.agent_identity.0,
                entry.generation.get(),
                entry.fence_token.get(),
                obligation.generation.0,
                obligation.fence_token.0,
            )));
        }
        let member_ref = match &entry.member_ref {
            MemberRef::BackendPeer {
                session_id: Some(session_id),
                ..
            } if session_id.to_string() != obligation.member_session_id.0 => {
                return Err(MobError::Internal(format!(
                    "actionable placed kickoff origin for '{}' has roster session '{}' but custody requires '{}'",
                    obligation.agent_identity.0, session_id, obligation.member_session_id.0,
                )));
            }
            MemberRef::BackendPeer { .. } => entry.member_ref.clone(),
            MemberRef::Session { .. } => {
                return Err(MobError::Internal(format!(
                    "pending placed kickoff '{}' has a local session route",
                    obligation.agent_identity.0
                )));
            }
        };

        // This is the last origin read before remote I/O. Cancellation commits
        // its durable machine state before the reconciler ever sends a cancel.
        // A cancellation racing immediately after this read is arbitrated by
        // the host's durable same-key tombstone: delivery-first is cancelled
        // or returns terminal truth, while cancel-first makes this delayed
        // delivery a no-effect replay. No controller-wide lock spans network
        // I/O, so a blackholed host cannot close another host's lane.
        let current_state = self.handle.machine_state_watch_rx.borrow().clone();
        if !current_state
            .pending_placed_kickoff_outcomes
            .contains(obligation)
            || !kickoff_phase_allows_origin(&current_state, obligation)
        {
            return Ok(PendingAttempt::NoChange);
        }
        if !exact_route_is_current(&current_state, obligation) {
            return Err(MobError::Internal(format!(
                "placed kickoff origin '{}' retained custody but lost its exact route before delivery",
                obligation.input_id.0
            )));
        }
        match self
            .provisioner
            .start_turn_with_correlation(
                &member_ref,
                request,
                Some(PlacedTurnDeliveryContext {
                    input_id: obligation.input_id.0.clone(),
                    transcript_interaction_id: Some(obligation.input_id.0.clone()),
                    expected_member,
                    outcome_tracking: Some(
                        super::bridge_protocol::BridgeOutcomeTracking::Interaction,
                    ),
                }),
            )
            .await
        {
            Ok(receipt) if receipt.as_deref() == Some(obligation.input_id.0.as_str()) => {
                Ok(PendingAttempt::DeliveryAccepted)
            }
            Ok(receipt) => Err(MobError::Internal(format!(
                "placed kickoff replay changed correlation '{}' to {receipt:?}",
                obligation.input_id.0
            ))),
            Err(error @ MobError::BridgeDeliveryRejected { .. }) => {
                let semantic_error = error.to_string();
                let event = super::remote_flow_ticket::placed_kickoff_obligation_event(obligation)?;
                self.handle
                    .reject_placed_kickoff_before_admission(event, semantic_error.clone())
                    .await?;
                Ok(PendingAttempt::CertifiedNoEffect(semantic_error))
            }
            Err(error) => Err(error),
        }
    }

    async fn scan_once(
        &self,
        retry: &mut BTreeMap<mob_dsl::PlacedKickoffObligation, RetryState>,
        dispositions: &mut BTreeMap<mob_dsl::PlacedKickoffObligation, DeliveryDisposition>,
        row_cursor: &mut BTreeMap<mob_dsl::HostId, mob_dsl::PlacedKickoffObligation>,
        host_cursor: &mut usize,
    ) -> Result<ScanSummary, MobError> {
        let state = self.handle.machine_state_watch_rx.borrow().clone();
        let pending = state
            .pending_placed_kickoff_outcomes
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let resolved = state
            .resolved_placed_kickoff_outcomes
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let live = pending
            .iter()
            .chain(&resolved)
            .cloned()
            .collect::<BTreeSet<_>>();
        let pending_set = pending.iter().cloned().collect::<BTreeSet<_>>();
        retry.retain(|obligation, _| pending_set.contains(obligation));
        dispositions.retain(|obligation, _| pending_set.contains(obligation));

        // Resolved custody still needs the member pump: it carries the ACK on
        // the next poll and only a confirmed host response may close custody.
        for obligation in &resolved {
            if resolved_host_cancel_can_close(&state, obligation) {
                // `Cancelled` outcome kind is minted only from the exact,
                // replay-stable CancelTrackedMemberInput host receipt. Unlike
                // a turn terminal row, its durable host tombstone has no poll
                // ACK protocol, so the structural cancellation carrier itself
                // authorizes controller-side custody closure after restart.
                let event = super::remote_flow_ticket::placed_kickoff_obligation_event(obligation)?;
                if let Err(error) = self
                    .handle
                    .acknowledge_placed_kickoff_outcome(
                        event,
                        super::bridge_protocol::BridgeTurnOutcomeAck {
                            generation: obligation.generation.0,
                            fence_token: obligation.fence_token.0,
                            input_id: obligation.input_id.0.clone(),
                        },
                    )
                    .await
                {
                    tracing::debug!(
                        host_id = %obligation.host_id.0,
                        input_id = %obligation.input_id.0,
                        error = %error,
                        "placed kickoff cancellation closure remains pending"
                    );
                }
                continue;
            }
            let identity = AgentIdentity::from(obligation.agent_identity.0.as_str());
            if let Err(error) = self.handle.ensure_pump_for_obligation(&identity).await {
                tracing::debug!(
                    host_id = %obligation.host_id.0,
                    input_id = %obligation.input_id.0,
                    error = %error,
                    "resolved placed kickoff pump ensure failed; independent rows continue"
                );
            }
        }

        let mut by_host = BTreeMap::<mob_dsl::HostId, Vec<mob_dsl::PlacedKickoffObligation>>::new();
        for obligation in pending {
            let identity = AgentIdentity::from(obligation.agent_identity.0.as_str());
            if let Err(error) = self.handle.ensure_pump_for_obligation(&identity).await {
                tracing::debug!(
                    host_id = %obligation.host_id.0,
                    input_id = %obligation.input_id.0,
                    error = %error,
                    "placed kickoff pump ensure failed; independent rows continue"
                );
            }

            if kickoff_phase_requires_host_cancel(&state, &obligation) {
                retry.entry(obligation.clone()).or_default();
                // Public cancellation permanently switches this obligation to
                // the host-cancel lane. It must never fall through to the
                // work-delivery path, even after a transport failure.
                by_host
                    .entry(obligation.host_id.clone())
                    .or_default()
                    .push(obligation);
                continue;
            }

            if let Some(disposition) = dispositions.get(&obligation).cloned() {
                match disposition {
                    DeliveryDisposition::Accepted => continue,
                    DeliveryDisposition::CertifiedNoEffect(error) => {
                        let event = super::remote_flow_ticket::placed_kickoff_obligation_event(
                            &obligation,
                        )?;
                        self.handle
                            .reject_placed_kickoff_before_admission(event, error)
                            .await?;
                        continue;
                    }
                }
            }

            // Stop/cancel closes the authority to originate work while
            // retaining Pending custody: a delivery may already have been
            // admitted and its host sidecar must still be drained, but an
            // unsent kickoff must never begin after cancellation.
            if !kickoff_phase_allows_origin(&state, &obligation) {
                retry.remove(&obligation);
                continue;
            }

            retry.entry(obligation.clone()).or_default();
            by_host
                .entry(obligation.host_id.clone())
                .or_default()
                .push(obligation);
        }

        // Admit at most one due row per host and a fixed number of hosts per
        // round. Both cursors rotate: a blackholed low row cannot monopolize
        // its host, and a large mob cannot open unbounded bridge I/O or starve
        // a healthy tail host behind overlapping windows.
        for obligations in by_host.values_mut() {
            obligations.sort();
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
            let previous = row_cursor.get(host);
            let due = obligations
                .iter()
                .filter(|obligation| retry.get(*obligation).is_some_and(|retry| retry.due(now)))
                .find(|obligation| previous.is_none_or(|previous| *obligation > previous))
                .or_else(|| {
                    obligations.iter().find(|obligation| {
                        retry.get(*obligation).is_some_and(|retry| retry.due(now))
                    })
                });
            let Some(obligation) = due.cloned() else {
                continue;
            };
            let retry_state = retry.get_mut(&obligation).ok_or_else(|| {
                MobError::Internal(format!(
                    "placed kickoff retry candidate for '{}' lost its retry state before admission",
                    obligation.agent_identity.0
                ))
            })?;
            retry_state.record_attempt(now);
            row_cursor.insert(host.clone(), obligation.clone());
            selected.push(obligation);
            last_selected_offset = Some(host_offset);
            if selected.len() == MAX_CONCURRENT_HOSTS {
                break;
            }
        }
        if !hosts.is_empty() {
            *host_cursor =
                super::actor::advance_rotating_cursor(hosts.len(), rotation, last_selected_offset);
        }

        let mut attempts = FuturesUnordered::new();
        for obligation in selected {
            attempts.push(async move {
                let result = self.reconcile_pending_obligation(&obligation).await;
                (obligation, result)
            });
        }
        while let Some((obligation, result)) = attempts.next().await {
            match result {
                Ok(PendingAttempt::NoChange) => {}
                Ok(PendingAttempt::DeliveryAccepted) => {
                    dispositions.insert(obligation.clone(), DeliveryDisposition::Accepted);
                    retry.remove(&obligation);
                }
                Ok(PendingAttempt::CertifiedNoEffect(error)) => {
                    dispositions.insert(
                        obligation.clone(),
                        DeliveryDisposition::CertifiedNoEffect(error),
                    );
                    retry.remove(&obligation);
                }
                Err(error) => {
                    tracing::debug!(
                        host_id = %obligation.host_id.0,
                        agent_identity = %obligation.agent_identity.0,
                        input_id = %obligation.input_id.0,
                        error = %error,
                        "placed kickoff reconciliation remains pending and will retry"
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
            next_retry_after,
        })
    }
}

async fn wait_for_event(event_rx: &mut Option<crate::store::MobEventReceiver>) {
    match event_rx {
        Some(receiver) => match receiver.recv().await {
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => *event_rx = None,
        },
        None => std::future::pending::<()>().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_host_window_is_bounded() {
        assert_eq!(MAX_CONCURRENT_HOSTS, 8);
    }

    #[test]
    fn cancellation_closes_origin_without_erasing_pending_custody() {
        let identity = mob_dsl::AgentIdentity("placed-worker".to_string());
        let obligation = mob_dsl::PlacedKickoffObligation {
            agent_identity: identity.clone(),
            ..Default::default()
        };
        let mut state = mob_dsl::MobMachineState::default();
        state.member_kickoff_starting.insert(identity.clone());
        state
            .pending_placed_kickoff_outcomes
            .insert(obligation.clone());
        assert!(kickoff_phase_allows_origin(&state, &obligation));

        state.member_kickoff_starting.remove(&identity);
        state.member_kickoff_cancelled.insert(identity.clone());
        assert!(state.pending_placed_kickoff_outcomes.contains(&obligation));
        assert!(!kickoff_phase_allows_origin(&state, &obligation));
        assert!(kickoff_phase_requires_host_cancel(&state, &obligation));

        state.pending_placed_kickoff_outcomes.remove(&obligation);
        state
            .resolved_placed_kickoff_outcomes
            .insert(obligation.clone());
        state
            .member_placed_kickoff_outcome_kinds
            .insert(identity, mob_dsl::PlacedKickoffOutcomeKind::Cancelled);
        assert!(resolved_host_cancel_can_close(&state, &obligation));
    }

    #[test]
    fn durable_cancellation_supersedes_a_stale_origin_observation() {
        let identity = mob_dsl::AgentIdentity("placed-worker".to_string());
        let obligation = mob_dsl::PlacedKickoffObligation {
            agent_identity: identity.clone(),
            ..Default::default()
        };
        let mut state = mob_dsl::MobMachineState::default();
        state.member_kickoff_starting.insert(identity.clone());
        state
            .pending_placed_kickoff_outcomes
            .insert(obligation.clone());
        let stale_origin_observation = kickoff_phase_allows_origin(&state, &obligation);
        assert!(stale_origin_observation);

        // Stop commits this transition before any exact host cancellation.
        // A send that already crossed its final read is safe because the host
        // tombstones this same input id; every later scan must instead enter
        // the cancel lane without waiting on a controller-global mutex.
        state.member_kickoff_starting.remove(&identity);
        state.member_kickoff_cancelled.insert(identity);
        assert!(!kickoff_phase_allows_origin(&state, &obligation));
        assert!(kickoff_phase_requires_host_cancel(&state, &obligation));
        assert!(
            state.pending_placed_kickoff_outcomes.contains(&obligation),
            "Stop retains custody for a possible already-admitted host terminal"
        );
    }

    #[test]
    fn typed_lifecycle_intent_fences_origin_before_member_cancel_replay() {
        let identity = mob_dsl::AgentIdentity("placed-worker".to_string());
        let obligation = mob_dsl::PlacedKickoffObligation {
            agent_identity: identity.clone(),
            ..Default::default()
        };
        let mut state = mob_dsl::MobMachineState::default();
        state.member_kickoff_starting.insert(identity);
        state
            .pending_placed_kickoff_outcomes
            .insert(obligation.clone());
        assert!(kickoff_phase_allows_origin(&state, &obligation));

        state.placed_completion_lifecycle_quiescing = true;
        state.placed_completion_lifecycle_intent =
            Some(mob_dsl::PlacedCompletionLifecycleIntentKind::Stop);
        assert!(!kickoff_phase_allows_origin(&state, &obligation));
        assert!(
            state.pending_placed_kickoff_outcomes.contains(&obligation),
            "the intent fences resend without erasing exact cleanup custody"
        );
    }
}
