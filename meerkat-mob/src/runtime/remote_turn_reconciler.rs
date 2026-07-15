//! Durable controller-side remote-turn intent adoption.
//!
//! The private intent row is the replay-complete authority carrier. Public
//! `RemoteTurnObligationRecorded` events are an ordered, non-sensitive
//! projection. Recovery therefore joins private rows only to a dispatch in
//! the current mob epoch, reserves every accepted dispatch sequence even when
//! the public append was ambiguous, and repairs a missing public projection
//! without ever minting a replacement input id.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use meerkat_core::time_compat::{Duration, Instant};

use crate::error::MobError;
use crate::event::{MemberRef, MobEvent, MobEventKind, NewMobEvent, RemoteTurnObligationEvent};
use crate::ids::{MobId, RunId};
use crate::machines::mob_machine as mob_dsl;
use crate::run::{
    MobRunRemoteTurnIntent, MobRunRemoteTurnReceipt, MobRunRemoteTurnReceiptOutcome,
    MobRunRemoteTurnTrackedCancelTerminal,
};
use crate::store::{MobEventStore, MobRunStore};

use super::handle::MobHandle;
use super::provisioner::MobProvisioner;
use super::remote_flow_ticket::{
    OutcomeRecordDisposition, RemoteFlowTicketRegistry, TurnDirectiveDelivery,
    obligation_from_event,
};
use super::turn_executor::{FlowTurnFailureDisposition, FlowTurnOutcome};
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use tokio::sync::oneshot;

const RECONCILE_SCAN_INTERVAL: Duration = Duration::from_millis(100);
const RECONCILE_MAX_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_CONCURRENT_HOSTS: usize = 8;
const TRACKED_CANCEL_NO_EFFECT_REASON: &str =
    "member host durably cancelled recovered remote turn before runtime admission";
const TRACKED_CANCELLED_REASON: &str =
    "member host durably cancelled recovered remote turn during lifecycle quiesce";

fn machine_verified_backend_route(
    member_ref: &MemberRef,
    expected_session_id: &str,
) -> Option<MemberRef> {
    match member_ref {
        MemberRef::BackendPeer {
            session_id: Some(session_id),
            ..
        } if session_id.to_string() != expected_session_id => None,
        // Peer-only resume/rebind deliberately leaves the shell transport
        // reference sessionless. The machine binding checked by the caller is
        // authoritative; an absent optional shell copy must not strand the
        // durable same-ID intent.
        MemberRef::BackendPeer { .. } => Some(member_ref.clone()),
        MemberRef::Session { .. } => None,
    }
}

fn machine_cancel_route_is_current_for_recovery(
    state: &mob_dsl::MobMachineState,
    obligation: &RemoteTurnObligationEvent,
) -> bool {
    let identity = mob_dsl::AgentIdentity(obligation.agent_identity.to_string());
    let host_id = mob_dsl::HostId(obligation.host_id.clone());
    let generation = mob_dsl::Generation(obligation.generation.get());
    let fence_token = mob_dsl::FenceToken(obligation.fence_token.get());
    let session_id = mob_dsl::SessionId(obligation.member_session_id.clone());
    state.mob_hosts.contains(&host_id)
        && state.host_bind_phase.get(&host_id) == Some(&mob_dsl::HostBindPhase::Bound)
        && state.host_durable_sessions.get(&host_id) == Some(&true)
        && state.host_tracked_input_cancel.get(&host_id) == Some(&true)
        && state
            .host_protocol_min
            .get(&host_id)
            .is_some_and(|minimum| *minimum <= 4)
        && state
            .host_protocol_max
            .get(&host_id)
            .is_some_and(|maximum| *maximum >= 4)
        && state.member_placement.get(&identity) == Some(&host_id)
        && state
            .current_placed_spawn_host_binding_generations
            .get(&identity)
            == Some(&obligation.host_binding_generation)
        && state.host_binding_generations.get(&host_id) == Some(&obligation.host_binding_generation)
        && state.identity_runtime_generations.get(&identity) == Some(&generation)
        && state.identity_runtime_fence_tokens.get(&identity) == Some(&fence_token)
        && state.member_session_bindings.get(&identity) == Some(&session_id)
        && state
            .identity_to_runtime
            .get(&identity)
            .is_some_and(|runtime_id| state.live_runtime_ids.contains(runtime_id))
}

fn machine_route_is_current_for_replay(
    state: &mob_dsl::MobMachineState,
    obligation: &RemoteTurnObligationEvent,
) -> bool {
    let identity = mob_dsl::AgentIdentity(obligation.agent_identity.to_string());
    !state.destroy_admitted
        && machine_cancel_route_is_current_for_recovery(state, obligation)
        // An absent spawn-exec phase is the DSL's Settled state.
        && !state.spawn_exec_phase.contains_key(&identity)
        && !state.member_revival_pending.contains(&identity)
        && !state.member_materialization_failures.contains_key(&identity)
        && state.identity_to_runtime.get(&identity).is_some_and(|runtime_id| {
            // Placed PeerOnly runtimes intentionally have no local
            // member_startup_ready fact. Their exact committed host
            // session + placement/incarnation facts above are the replay
            // authority, matching fresh Record admission. The lifecycle
            // marker map is sparse: absent is the canonical Active shape;
            // only an explicit Retiring marker closes replay admission.
            state.member_state_markers.get(runtime_id)
                != Some(&mob_dsl::MobMemberState::Retiring)
        })
}

/// Full persisted identity for one controller-custody row. The dispatch
/// sequence is globally injective within an epoch, but carrying every field
/// here makes sequence reuse fail closed instead of aliasing two obligations.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RemoteTurnKey {
    pub dispatch_sequence: u64,
    pub agent_identity: String,
    pub host_id: String,
    pub host_binding_generation: u64,
    pub member_session_id: String,
    pub generation: u64,
    pub fence_token: u64,
    pub input_id: String,
    pub run_id: String,
    pub step_id: String,
}

impl RemoteTurnKey {
    pub(crate) fn from_event(obligation: &RemoteTurnObligationEvent) -> Self {
        Self {
            dispatch_sequence: obligation.dispatch_sequence,
            agent_identity: obligation.agent_identity.to_string(),
            host_id: obligation.host_id.clone(),
            host_binding_generation: obligation.host_binding_generation,
            member_session_id: obligation.member_session_id.clone(),
            generation: obligation.generation.get(),
            fence_token: obligation.fence_token.get(),
            input_id: obligation.input_id.clone(),
            run_id: obligation.run_id.to_string(),
            step_id: obligation.step_id.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct StepDispatchKey {
    run_id: String,
    step_id: String,
    agent_identity: String,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct RemoteTrackedInputKey {
    agent_identity: String,
    host_id: String,
    generation: u64,
    fence_token: u64,
    input_id: String,
}

impl From<&RemoteTurnObligationEvent> for RemoteTrackedInputKey {
    fn from(obligation: &RemoteTurnObligationEvent) -> Self {
        Self {
            agent_identity: obligation.agent_identity.to_string(),
            host_id: obligation.host_id.clone(),
            generation: obligation.generation.get(),
            fence_token: obligation.fence_token.get(),
            input_id: obligation.input_id.clone(),
        }
    }
}

impl StepDispatchKey {
    fn from_obligation(obligation: &RemoteTurnObligationEvent) -> Self {
        Self {
            run_id: obligation.run_id.to_string(),
            step_id: obligation.step_id.to_string(),
            agent_identity: obligation.agent_identity.to_string(),
            generation: obligation.generation.get(),
        }
    }
}

/// Current-epoch private material accepted for custody recovery. Historical
/// reset-epoch rows are scrubbed while preparing this value.
pub(crate) struct RemoteTurnRecoveryMaterial {
    pub intents: Vec<MobRunRemoteTurnIntent>,
    pub receipts: Vec<MobRunRemoteTurnReceipt>,
    pub deferred_run_ids: BTreeSet<RunId>,
    pub record_projection_pending: Vec<RemoteTurnObligationEvent>,
    /// Receipt-first cleanup that is safe only after the public finalizer
    /// chain has passed full custody recovery validation.
    pub finalized_privacy_cleanup: Vec<FinalizedRemoteTurnPrivacyCleanup>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum RemoteTurnPublicFinalizer {
    Acknowledged,
    Disposed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalizedRemoteTurnPrivacyCleanup {
    pub obligation: RemoteTurnObligationEvent,
    pub finalizer: RemoteTurnPublicFinalizer,
}

pub(crate) async fn apply_remote_turn_finalized_privacy_cleanup(
    run_store: Arc<dyn MobRunStore>,
    cleanup: &[FinalizedRemoteTurnPrivacyCleanup],
) -> Result<(), MobError> {
    for item in cleanup {
        let run_id = &item.obligation.run_id;
        let dispatch_sequence = item.obligation.dispatch_sequence;
        // The receipt validates through its exact intent. Keep that join
        // observable until the receipt is gone, including across crashes.
        run_store
            .delete_remote_turn_receipt(run_id, dispatch_sequence)
            .await?;
        run_store
            .delete_remote_turn_intent(run_id, dispatch_sequence)
            .await?;
    }
    Ok(())
}

pub(crate) fn validate_finalized_remote_turn_public_chain(
    epoch_events: &[MobEvent],
    dispatch_sequence: u64,
) -> Result<FinalizedRemoteTurnPrivacyCleanup, MobError> {
    let mut obligation: Option<RemoteTurnObligationEvent> = None;
    let mut recorded = false;
    let mut terminal_count = 0usize;
    let mut resolved = false;
    let mut acknowledged = false;
    let mut disposed = false;

    for event in epoch_events {
        let (candidate, phase) = match &event.kind {
            MobEventKind::RemoteTurnObligationRecorded { obligation } => (obligation, 0u8),
            MobEventKind::StepTargetCompleted {
                run_id,
                step_id,
                target,
                remote_turn_obligation: Some(obligation),
                output,
            } if obligation.dispatch_sequence == dispatch_sequence => {
                if run_id != &obligation.run_id
                    || step_id != &obligation.step_id
                    || target.identity != obligation.agent_identity
                    || target.generation != obligation.generation
                    || output.is_none()
                {
                    return Err(MobError::Internal(format!(
                        "finalized remote-turn sequence {dispatch_sequence} has an invalid completed terminal carrier"
                    )));
                }
                (obligation, 1)
            }
            MobEventKind::StepTargetFailed {
                run_id,
                step_id,
                target,
                remote_turn_obligation: Some(obligation),
                ..
            } if obligation.dispatch_sequence == dispatch_sequence => {
                if run_id != &obligation.run_id
                    || step_id != &obligation.step_id
                    || target.identity != obligation.agent_identity
                    || target.generation != obligation.generation
                {
                    return Err(MobError::Internal(format!(
                        "finalized remote-turn sequence {dispatch_sequence} has an invalid failed terminal carrier"
                    )));
                }
                (obligation, 1)
            }
            MobEventKind::RemoteTurnOutcomeResolved { obligation } => (obligation, 2),
            MobEventKind::RemoteTurnOutcomeAcknowledged { obligation } => (obligation, 3),
            MobEventKind::RemoteTurnOutcomeDisposed { obligation } => (obligation, 4),
            _ => continue,
        };
        if candidate.dispatch_sequence != dispatch_sequence {
            continue;
        }
        if obligation
            .as_ref()
            .is_some_and(|existing| existing != candidate)
        {
            return Err(MobError::Internal(format!(
                "finalized remote-turn sequence {dispatch_sequence} names conflicting obligations"
            )));
        }
        obligation = Some(candidate.clone());
        match phase {
            0 => recorded = true,
            1 => terminal_count += 1,
            2 => resolved = true,
            3 => acknowledged = true,
            4 => disposed = true,
            _ => {
                return Err(MobError::Internal(format!(
                    "finalized remote-turn sequence {dispatch_sequence} projected an unknown public custody phase {phase}"
                )));
            }
        }
    }

    let obligation = obligation.ok_or_else(|| {
        MobError::Internal(format!(
            "finalized remote-turn sequence {dispatch_sequence} has no exact obligation"
        ))
    })?;
    if !recorded || terminal_count != 1 {
        return Err(MobError::Internal(format!(
            "finalized remote-turn sequence {dispatch_sequence} requires one Record and exactly one terminal carrier"
        )));
    }
    let finalizer = match (acknowledged, disposed) {
        (true, false) if resolved => RemoteTurnPublicFinalizer::Acknowledged,
        (false, true) => RemoteTurnPublicFinalizer::Disposed,
        (true, false) => {
            return Err(MobError::Internal(format!(
                "finalized remote-turn sequence {dispatch_sequence} ACK has no Resolve predecessor"
            )));
        }
        (true, true) => {
            return Err(MobError::Internal(format!(
                "finalized remote-turn sequence {dispatch_sequence} has mutually exclusive ACK and Dispose finalizers"
            )));
        }
        (false, false) => {
            return Err(MobError::Internal(format!(
                "remote-turn sequence {dispatch_sequence} is not finalized"
            )));
        }
    };
    Ok(FinalizedRemoteTurnPrivacyCleanup {
        obligation,
        finalizer,
    })
}

fn insert_sequence_exact(
    by_sequence: &mut BTreeMap<u64, RemoteTurnObligationEvent>,
    obligation: &RemoteTurnObligationEvent,
    source: &str,
) -> Result<(), MobError> {
    match by_sequence.get(&obligation.dispatch_sequence) {
        Some(existing) if existing != obligation => Err(MobError::Internal(format!(
            "remote-turn dispatch sequence {} is reused by conflicting obligations ({source})",
            obligation.dispatch_sequence
        ))),
        Some(_) => Ok(()),
        None => {
            by_sequence.insert(obligation.dispatch_sequence, obligation.clone());
            Ok(())
        }
    }
}

fn insert_input_exact(
    by_input: &mut BTreeMap<RemoteTrackedInputKey, RemoteTurnObligationEvent>,
    obligation: &RemoteTurnObligationEvent,
    source: &str,
) -> Result<(), MobError> {
    let key = RemoteTrackedInputKey::from(obligation);
    match by_input.get(&key) {
        Some(existing) if existing != obligation => Err(MobError::Internal(format!(
            "remote-turn input id '{}' is reused by conflicting obligations ({source})",
            obligation.input_id
        ))),
        Some(_) => Ok(()),
        None => {
            by_input.insert(key, obligation.clone());
            Ok(())
        }
    }
}

fn current_epoch_dispatches(events: &[MobEvent]) -> BTreeSet<StepDispatchKey> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            MobEventKind::StepDispatched {
                run_id,
                step_id,
                target,
            } => Some(StepDispatchKey {
                run_id: run_id.to_string(),
                step_id: step_id.to_string(),
                agent_identity: target.identity.to_string(),
                generation: target.generation.get(),
            }),
            _ => None,
        })
        .collect()
}

fn current_epoch_run_ids(events: &[MobEvent]) -> BTreeSet<String> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            MobEventKind::FlowStarted { run_id, .. } => Some(run_id.to_string()),
            _ => None,
        })
        .collect()
}

fn current_epoch_obligations(
    events: &[MobEvent],
) -> Result<BTreeMap<u64, RemoteTurnObligationEvent>, MobError> {
    let mut by_sequence = BTreeMap::new();
    for event in events {
        let obligation = match &event.kind {
            MobEventKind::RemoteTurnObligationRecorded { obligation }
            | MobEventKind::RemoteTurnOutcomeResolved { obligation }
            | MobEventKind::RemoteTurnOutcomeAcknowledged { obligation }
            | MobEventKind::RemoteTurnOutcomeDisposed { obligation }
            | MobEventKind::StepTargetCompleted {
                remote_turn_obligation: Some(obligation),
                ..
            }
            | MobEventKind::StepTargetFailed {
                remote_turn_obligation: Some(obligation),
                ..
            } => obligation,
            _ => continue,
        };
        insert_sequence_exact(&mut by_sequence, obligation, "public event carrier")?;
    }
    Ok(by_sequence)
}

fn finalized_sequences(events: &[MobEvent]) -> BTreeSet<u64> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            MobEventKind::RemoteTurnOutcomeAcknowledged { obligation }
            | MobEventKind::RemoteTurnOutcomeDisposed { obligation } => {
                Some(obligation.dispatch_sequence)
            }
            _ => None,
        })
        .collect()
}

/// Load and validate private replay rows for the current epoch.
///
/// An intent can be current without a public Record carrier only when its
/// exact run/step/member already has the preceding current-epoch
/// `StepDispatched` event. That is the wrote-then-error/append-ambiguous case:
/// the sequence stays consumed and the missing public projection is retried.
/// Rows from a prior reset epoch are deleted rather than allowed to poison the
/// new epoch's sequence or custody state.
pub(crate) async fn prepare_remote_turn_recovery_material(
    run_store: Arc<dyn MobRunStore>,
    event_store: Arc<dyn MobEventStore>,
    mob_id: &MobId,
    epoch_events: &[MobEvent],
    repair_missing_records_before_actor_start: bool,
) -> Result<RemoteTurnRecoveryMaterial, MobError> {
    let dispatches = current_epoch_dispatches(epoch_events);
    let epoch_run_ids = current_epoch_run_ids(epoch_events);
    let finalized = finalized_sequences(epoch_events);
    let mut obligations = current_epoch_obligations(epoch_events)?;
    let finalized_chains = finalized
        .iter()
        .map(|dispatch_sequence| {
            Ok((
                *dispatch_sequence,
                validate_finalized_remote_turn_public_chain(epoch_events, *dispatch_sequence)?,
            ))
        })
        .collect::<Result<BTreeMap<_, _>, MobError>>()?;
    let mut obligations_by_input = BTreeMap::new();
    for obligation in obligations.values() {
        insert_input_exact(
            &mut obligations_by_input,
            obligation,
            "public event carrier",
        )?;
    }
    let mut publicly_recorded = epoch_events
        .iter()
        .filter_map(|event| match &event.kind {
            MobEventKind::RemoteTurnObligationRecorded { obligation } => {
                Some(obligation.dispatch_sequence)
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();

    let runs = run_store.list_runs(mob_id, None).await?;
    let mut intents = Vec::new();
    let mut receipts = Vec::new();
    let mut current_intents = BTreeMap::<u64, MobRunRemoteTurnIntent>::new();
    let mut record_projection_pending = Vec::new();
    let mut finalized_privacy_cleanup = BTreeSet::<u64>::new();

    for run in &runs {
        // Snapshot both families before cleanup. The validating receipt read
        // joins through the exact intent; deleting an acknowledged intent
        // first would turn a crash-left receipt into artificial corruption.
        let stored_intents = run_store.list_remote_turn_intents(&run.run_id).await?;
        let stored_receipts = run_store.list_remote_turn_receipts(&run.run_id).await?;
        for intent in stored_intents {
            intent.validate_for(&run.run_id, mob_id).map_err(|error| {
                MobError::Internal(format!(
                    "invalid durable remote-turn intent for run '{}': {error}",
                    run.run_id
                ))
            })?;
            let obligation = &intent.obligation;
            // Run ids are durable store primary keys and the run store is not
            // cleared by MobReset; create_run rejects reuse. Requiring both a
            // current-epoch FlowStarted and StepDispatched therefore prevents
            // an old private row from joining a coincidentally similar new
            // step/member tuple.
            let current = epoch_run_ids.contains(&obligation.run_id.to_string())
                && dispatches.contains(&StepDispatchKey::from_obligation(obligation));
            if !current {
                // Only startup owns an exclusive event/private-row snapshot.
                // A live scan runs beside flow dispatch: StepDispatched and
                // the exact private row can land after its event snapshot but
                // before this row read. Deleting from that torn view would
                // erase fresh replay custody. Skip it until the next scan;
                // the pre-actor pass remains the sole destructive scrubber.
                if repair_missing_records_before_actor_start {
                    run_store
                        .delete_remote_turn_intent(&run.run_id, obligation.dispatch_sequence)
                        .await?;
                }
                continue;
            }
            insert_sequence_exact(&mut obligations, obligation, "private intent")?;
            insert_input_exact(&mut obligations_by_input, obligation, "private intent")?;
            if !publicly_recorded.contains(&obligation.dispatch_sequence) {
                // Before actor start there is no concurrent append owner, so
                // repair must either prove the exact current-epoch Record or
                // fail while the private intent still reserves the sequence.
                // A live reconciler routes the same repair through the actor.
                if repair_missing_records_before_actor_start {
                    let append = event_store
                        .append(NewMobEvent {
                            mob_id: mob_id.clone(),
                            timestamp: None,
                            kind: MobEventKind::RemoteTurnObligationRecorded {
                                obligation: obligation.clone(),
                            },
                        })
                        .await;
                    if let Err(error) = append {
                        // Ambiguous write: accept only an exact durable reread.
                        // If absent, fail before finalized privacy cleanup so a
                        // later cold retry still has the repair authority.
                        let replay = event_store.replay_all().await?;
                        let mob_events = replay
                            .iter()
                            .filter(|event| event.mob_id == *mob_id)
                            .collect::<Vec<_>>();
                        let epoch_start = mob_events
                            .iter()
                            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
                            .map_or(0, |position| position + 1);
                        let exact = mob_events[epoch_start..].iter().any(|event| {
                            matches!(
                                &event.kind,
                                MobEventKind::RemoteTurnObligationRecorded { obligation: recorded }
                                    if recorded == obligation
                            )
                        });
                        if !exact {
                            return Err(MobError::Internal(format!(
                                "remote-turn Record projection repair failed for run '{}' sequence {} and reread proved it absent: {error}",
                                obligation.run_id, obligation.dispatch_sequence
                            )));
                        }
                    }
                    publicly_recorded.insert(obligation.dispatch_sequence);
                } else {
                    record_projection_pending.push(obligation.clone());
                }
            }
            current_intents.insert(obligation.dispatch_sequence, intent.clone());
            if finalized.contains(&obligation.dispatch_sequence) {
                finalized_privacy_cleanup.insert(obligation.dispatch_sequence);
            } else {
                intents.push(intent);
            }
        }

        for receipt in stored_receipts {
            if receipt.obligation.run_id != run.run_id {
                return Err(MobError::Internal(format!(
                    "remote-turn receipt run '{}' does not match owning run '{}'",
                    receipt.obligation.run_id, run.run_id
                )));
            }
            if receipt.obligation.dispatch_sequence == 0
                || receipt.obligation.input_id.trim().is_empty()
            {
                return Err(MobError::Internal(format!(
                    "remote-turn receipt for run '{}' has an invalid obligation key",
                    run.run_id
                )));
            }
            let current = epoch_run_ids.contains(&receipt.obligation.run_id.to_string())
                && dispatches.contains(&StepDispatchKey::from_obligation(&receipt.obligation));
            if !current {
                // As above, a live event snapshot cannot authorize destructive
                // cleanup of a private row observed later in the scan.
                if repair_missing_records_before_actor_start {
                    run_store
                        .delete_remote_turn_receipt(
                            &run.run_id,
                            receipt.obligation.dispatch_sequence,
                        )
                        .await?;
                }
                continue;
            }
            insert_sequence_exact(&mut obligations, &receipt.obligation, "private receipt")?;
            insert_input_exact(
                &mut obligations_by_input,
                &receipt.obligation,
                "private receipt",
            )?;
            let Some(intent) = current_intents.get(&receipt.obligation.dispatch_sequence) else {
                return Err(MobError::Internal(format!(
                    "current-epoch remote-turn receipt sequence {} has no exact private intent",
                    receipt.obligation.dispatch_sequence
                )));
            };
            receipt
                .validate_for(&run.run_id, mob_id, intent)
                .map_err(|error| {
                    MobError::Internal(format!(
                        "invalid durable remote-turn receipt for run '{}': {error}",
                        run.run_id
                    ))
                })?;
            if finalized.contains(&receipt.obligation.dispatch_sequence) {
                finalized_privacy_cleanup.insert(receipt.obligation.dispatch_sequence);
                continue;
            }
            receipts.push(receipt);
        }
    }

    intents.sort_by_key(|intent| intent.obligation.dispatch_sequence);
    receipts.sort_by_key(|receipt| receipt.obligation.dispatch_sequence);
    let public_terminal_run_ids = epoch_events.iter().filter_map(|event| match &event.kind {
        MobEventKind::StepTargetCompleted {
            run_id,
            remote_turn_obligation: Some(_),
            ..
        }
        | MobEventKind::StepTargetFailed {
            run_id,
            remote_turn_obligation: Some(_),
            ..
        } if epoch_run_ids.contains(&run_id.to_string()) => Some(run_id.clone()),
        _ => None,
    });
    let deferred_run_ids = intents
        .iter()
        .map(|intent| intent.obligation.run_id.clone())
        .chain(
            receipts
                .iter()
                .map(|receipt| receipt.obligation.run_id.clone()),
        )
        // ACK/Dispose may have privacy-cleaned both private rows before the
        // FlowRun reducer projects its public StepTarget terminal. Keep those
        // runs out of generic startup cancellation so the reconciler can
        // project exact single-target failures (and apply the documented
        // conservative fallback for success/multi-target cases).
        .chain(public_terminal_run_ids)
        .collect();
    let finalized_privacy_cleanup = finalized_privacy_cleanup
        .into_iter()
        .map(|dispatch_sequence| {
            finalized_chains
                .get(&dispatch_sequence)
                .cloned()
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "finalized cleanup sequence {dispatch_sequence} lost its validated public chain"
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(RemoteTurnRecoveryMaterial {
        intents,
        receipts,
        deferred_run_ids,
        record_projection_pending,
        finalized_privacy_cleanup,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CustodyPhase {
    Pending,
    Committed,
    Resolved,
    Absent,
}

fn auto_dispose_receipt_reason(outcome: &MobRunRemoteTurnReceiptOutcome) -> Option<(&str, bool)> {
    match outcome {
        MobRunRemoteTurnReceiptOutcome::Failed {
            reason,
            no_effect_proof: Some(_),
        } => Some((reason, false)),
        MobRunRemoteTurnReceiptOutcome::TrackedInputCancel { reason, .. } => Some((reason, true)),
        _ => None,
    }
}

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

fn select_due_intents(
    by_host: &BTreeMap<String, Vec<MobRunRemoteTurnIntent>>,
    retry: &mut BTreeMap<RemoteTurnKey, RetryState>,
    accepted: &BTreeSet<RemoteTurnKey>,
    row_cursor: &mut BTreeMap<String, u64>,
    host_cursor: &mut usize,
    now: Instant,
) -> Vec<MobRunRemoteTurnIntent> {
    row_cursor.retain(|host, _| by_host.contains_key(host));
    let mut hosts = by_host.keys().cloned().collect::<Vec<_>>();
    let mut rotation = 0usize;
    if !hosts.is_empty() {
        rotation = *host_cursor % hosts.len();
        hosts.rotate_left(rotation);
    }
    let mut selected = Vec::new();
    let mut last_selected_offset = None;
    for (host_offset, host) in hosts.iter().enumerate() {
        let Some(intents) = by_host.get(host) else {
            continue;
        };
        let previous = row_cursor.get(host).copied().unwrap_or(0);
        let due = intents
            .iter()
            .filter(|intent| {
                let key = RemoteTurnKey::from_event(&intent.obligation);
                !accepted.contains(&key) && retry.get(&key).is_some_and(|retry| retry.due(now))
            })
            .find(|intent| intent.obligation.dispatch_sequence > previous)
            .or_else(|| {
                intents.iter().find(|intent| {
                    let key = RemoteTurnKey::from_event(&intent.obligation);
                    !accepted.contains(&key) && retry.get(&key).is_some_and(|retry| retry.due(now))
                })
            });
        let Some(intent) = due.cloned() else {
            continue;
        };
        let key = RemoteTurnKey::from_event(&intent.obligation);
        let Some(retry_state) = retry.get_mut(&key) else {
            continue;
        };
        retry_state.record_attempt(now);
        row_cursor.insert(host.clone(), intent.obligation.dispatch_sequence);
        selected.push(intent);
        last_selected_offset = Some(host_offset);
        if selected.len() == MAX_CONCURRENT_HOSTS {
            break;
        }
    }
    if !hosts.is_empty() {
        *host_cursor =
            super::actor::advance_rotating_cursor(hosts.len(), rotation, last_selected_offset);
    }
    selected
}

struct AdoptedCompletion {
    key: RemoteTurnKey,
    intent: MobRunRemoteTurnIntent,
    outcome: Result<FlowTurnOutcome, oneshot::error::RecvError>,
}

struct AdoptedWait {
    key: RemoteTurnKey,
    intent: MobRunRemoteTurnIntent,
    receiver: oneshot::Receiver<FlowTurnOutcome>,
}

#[cfg(not(target_arch = "wasm32"))]
type AdoptedCompletionFuture = Pin<Box<dyn Future<Output = AdoptedCompletion> + Send>>;
#[cfg(target_arch = "wasm32")]
type AdoptedCompletionFuture = Pin<Box<dyn Future<Output = AdoptedCompletion>>>;

impl AdoptedWait {
    fn into_future(self) -> AdoptedCompletionFuture {
        Box::pin(async move {
            AdoptedCompletion {
                key: self.key,
                intent: self.intent,
                outcome: self.receiver.await,
            }
        })
    }
}

enum ReconcileWake {
    ActorClosed,
    Adopted(Option<AdoptedCompletion>),
    Event,
    Scan,
}

struct ScanSummary {
    live_intents: usize,
    next_retry_after: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingAttemptOutcome {
    Gone,
    DeliveryAccepted,
    DeliveryRejected,
    CancellationClosed,
    TerminalObserved,
    NoChange,
}

/// One host-lifetime reconciler for replay-complete controller intents. It is
/// intentionally a mechanical adopter: machine custody decides whether an
/// intent is Pending, the exact stored input is the only delivery it may
/// replay, and the sealed handle command owns receipt -> terminal -> Commit.
pub(crate) struct RemoteTurnIntentReconciler {
    mob_id: MobId,
    handle: MobHandle,
    provisioner: Arc<dyn MobProvisioner>,
    tickets: Arc<RemoteFlowTicketRegistry>,
    recovered_runs: BTreeSet<RunId>,
}

impl RemoteTurnIntentReconciler {
    pub(crate) async fn run_owned(
        mob_id: MobId,
        handle: MobHandle,
        provisioner: Arc<dyn MobProvisioner>,
        tickets: Arc<RemoteFlowTicketRegistry>,
        recovered_runs: BTreeSet<RunId>,
    ) {
        let reconciler = Self {
            mob_id,
            handle,
            provisioner,
            tickets,
            recovered_runs,
        };
        reconciler.run().await;
    }

    async fn run(self) {
        let mut completion_waiters = FuturesUnordered::<AdoptedCompletionFuture>::new();
        let mut adopted = BTreeSet::<RemoteTurnKey>::new();
        // A positive bridge ACK means the remote host owns the exact input.
        // Suppress further same-process delivery attempts until a terminal
        // carrier arrives. This is deliberately process-local: after a crash,
        // one exact replay is safe and recovers a lost ACK.
        let mut accepted = BTreeSet::<RemoteTurnKey>::new();
        let mut retry = BTreeMap::<RemoteTurnKey, RetryState>::new();
        let mut row_cursor = BTreeMap::<String, u64>::new();
        let mut host_cursor = 0usize;
        let mut converged_runs = BTreeSet::<RunId>::new();
        let actor_closed = self.handle.command_tx.clone();
        let mut event_rx = self.handle.events.subscribe().ok();
        let mut next_scan_after = RECONCILE_SCAN_INTERVAL;
        let mut next_scan_at = Instant::now() + next_scan_after;
        let mut idle_delay = RECONCILE_SCAN_INTERVAL;

        loop {
            let wake = tokio::select! {
                () = actor_closed.closed() => ReconcileWake::ActorClosed,
                completion = completion_waiters.next(), if !completion_waiters.is_empty() => {
                    ReconcileWake::Adopted(completion)
                }
                () = Self::wait_for_event(&mut event_rx) => ReconcileWake::Event,
                () = tokio::time::sleep(
                    next_scan_at.saturating_duration_since(Instant::now()),
                ) => ReconcileWake::Scan,
            };
            match wake {
                ReconcileWake::ActorClosed => break,
                ReconcileWake::Adopted(None) => continue,
                ReconcileWake::Adopted(Some(completion)) => {
                    adopted.remove(&completion.key);
                    accepted.remove(&completion.key);
                    retry.remove(&completion.key);
                    if let Err(error) = self
                        .finish_adopted_completion(completion, &mut converged_runs)
                        .await
                    {
                        tracing::warn!(error = %error, "remote-turn adopted terminal reconciliation remains pending");
                    }
                    next_scan_at = next_scan_at.min(Instant::now() + RECONCILE_SCAN_INTERVAL);
                }
                ReconcileWake::Event => {
                    // Structural append wakeup. A small debounce lets the
                    // immediately-following private intent write settle when
                    // the wake was StepDispatched. Use an absolute deadline
                    // and only move it earlier: a busy event stream must not
                    // postpone reconciliation forever.
                    next_scan_at = next_scan_at.min(Instant::now() + RECONCILE_SCAN_INTERVAL);
                }
                ReconcileWake::Scan => {
                    let mut new_adoptions = Vec::new();
                    let scan_result = self
                        .scan_once(
                            &mut new_adoptions,
                            &mut adopted,
                            &mut accepted,
                            &mut retry,
                            &mut row_cursor,
                            &mut host_cursor,
                            &mut converged_runs,
                        )
                        .await;
                    for adoption in new_adoptions {
                        completion_waiters.push(adoption.into_future());
                    }
                    match scan_result {
                        Ok(summary) => {
                            if summary.live_intents == 0 {
                                idle_delay = (idle_delay * 2).min(RECONCILE_MAX_RETRY_DELAY);
                                next_scan_after = idle_delay;
                            } else {
                                idle_delay = RECONCILE_SCAN_INTERVAL;
                                next_scan_after = summary
                                    .next_retry_after
                                    .unwrap_or(RECONCILE_MAX_RETRY_DELAY)
                                    .clamp(RECONCILE_SCAN_INTERVAL, RECONCILE_MAX_RETRY_DELAY);
                            }
                        }
                        Err(error) => {
                            tracing::warn!(error = %error, "remote-turn intent reconciliation scan failed; retrying");
                            next_scan_after = (next_scan_after * 2).min(RECONCILE_MAX_RETRY_DELAY);
                        }
                    }
                    next_scan_at = Instant::now() + next_scan_after;
                }
            }
        }
    }

    async fn wait_for_event(event_rx: &mut Option<crate::store::MobEventReceiver>) {
        match event_rx {
            Some(receiver) => match receiver.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    *event_rx = None;
                }
            },
            None => std::future::pending::<()>().await,
        }
    }

    async fn current_epoch_events(&self) -> Result<Vec<MobEvent>, MobError> {
        let all = self.handle.events.replay_all().await?;
        let mob_events = all
            .into_iter()
            .filter(|event| event.mob_id == self.mob_id)
            .collect::<Vec<_>>();
        let epoch_start = mob_events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |position| position + 1);
        Ok(mob_events[epoch_start..].to_vec())
    }

    // One scan advances a single cohesive recovery frontier; the mutable
    // collections are independent custody indexes, not a second state owner.
    #[allow(clippy::too_many_arguments)]
    async fn scan_once(
        &self,
        new_adoptions: &mut Vec<AdoptedWait>,
        adopted: &mut BTreeSet<RemoteTurnKey>,
        accepted: &mut BTreeSet<RemoteTurnKey>,
        retry: &mut BTreeMap<RemoteTurnKey, RetryState>,
        row_cursor: &mut BTreeMap<String, u64>,
        host_cursor: &mut usize,
        converged_runs: &mut BTreeSet<RunId>,
    ) -> Result<ScanSummary, MobError> {
        let epoch_events = self.current_epoch_events().await?;
        let material = prepare_remote_turn_recovery_material(
            self.handle.run_store.clone(),
            self.handle.events.clone(),
            &self.mob_id,
            &epoch_events,
            false,
        )
        .await?;
        for obligation in &material.record_projection_pending {
            self.handle
                .ensure_remote_turn_record(obligation.clone())
                .await?;
        }
        for cleanup in material.finalized_privacy_cleanup.clone() {
            // The scan validator proved the complete public finalizer chain;
            // the actor revalidates it, deletes receipt-first while custody
            // remains retry-visible, and only then publishes machine absence.
            self.handle
                .finalize_remote_turn_privacy_cleanup(cleanup)
                .await?;
        }
        let receipts = material
            .receipts
            .into_iter()
            .map(|receipt| (RemoteTurnKey::from_event(&receipt.obligation), receipt))
            .collect::<BTreeMap<_, _>>();
        let live_keys = material
            .intents
            .iter()
            .map(|intent| RemoteTurnKey::from_event(&intent.obligation))
            .collect::<BTreeSet<_>>();
        accepted.retain(|key| live_keys.contains(key));
        retry.retain(|key, _| live_keys.contains(key));

        let live_intents = material.intents.len();
        // Include every startup-deferred run, even when final-carrier privacy
        // cleanup removed its last private row between scans. The exact
        // machine Pending set remains the launch barrier.
        let mut convergence_candidates = self.recovered_runs.clone();
        let mut blocked_convergence_runs = BTreeSet::<RunId>::new();
        let mut due_by_host = BTreeMap::<String, Vec<MobRunRemoteTurnIntent>>::new();
        let mut cancel_only = BTreeSet::<RemoteTurnKey>::new();
        let lifecycle_quiescing = self
            .handle
            .machine_state_watch_rx
            .borrow()
            .placed_completion_lifecycle_quiescing;
        for intent in material.intents {
            let key = RemoteTurnKey::from_event(&intent.obligation);
            if let Some(receipt) = receipts.get(&key)
                && let Some((reason, tracked_cancel)) =
                    auto_dispose_receipt_reason(&receipt.outcome)
            {
                // Crash completion for either actor-owned no-effect fold. A
                // typed receipt may survive after the step terminal but before
                // its Disposed carrier. Finish it from Pending, Committed, or
                // Resolved custody without parsing presentation text.
                if tracked_cancel {
                    self.handle
                        .close_remote_turn_after_tracked_cancel(receipt.clone())
                        .await?;
                } else {
                    self.handle
                        .commit_remote_turn_receipt(receipt.clone())
                        .await?;
                }
                self.tickets.fail_armed(
                    &intent.obligation.agent_identity,
                    &intent.obligation.input_id,
                    reason.to_string(),
                );
                convergence_candidates.insert(intent.obligation.run_id.clone());
                accepted.remove(&key);
                retry.remove(&key);
                continue;
            }
            match self.custody_phase(&intent.obligation) {
                CustodyPhase::Pending => {
                    if let Some(receipt) = receipts.get(&key) {
                        self.handle
                            .commit_remote_turn_receipt(receipt.clone())
                            .await?;
                        convergence_candidates.insert(intent.obligation.run_id.clone());
                        accepted.remove(&key);
                        retry.remove(&key);
                        continue;
                    }
                    blocked_convergence_runs.insert(intent.obligation.run_id.clone());

                    // A live, nonterminal FlowEngine owns its bounded delivery
                    // attempt. Same-process adoption is allowed only after
                    // that run is terminal (the task/receiver is gone) or a
                    // lifecycle barrier needs the exact tracked-cancel path.
                    // Startup-recovered rows remain adoptable unconditionally.
                    let run = self
                        .handle
                        .run_store
                        .get_run(&intent.obligation.run_id)
                        .await?
                        .ok_or_else(|| MobError::RunNotFound(intent.obligation.run_id.clone()))?;
                    let run_terminal =
                        crate::run::mob_machine_run_status_is_terminal(&run.run_id, &run.status)?;
                    if !self.recovered_runs.contains(&intent.obligation.run_id)
                        && !run_terminal
                        && !lifecycle_quiescing
                    {
                        accepted.remove(&key);
                        retry.remove(&key);
                        self.handle
                            .ensure_pump_for_obligation(&intent.obligation.agent_identity)
                            .await?;
                        continue;
                    }

                    if run_terminal {
                        // Abort can occur after registry arm but before a
                        // RemoteTurnTicket owns/disarms it. No live run remains,
                        // so replace that orphan sender with the reconciler on
                        // first adoption. Once adopted, keep the reconciler's
                        // own stable waiter across retry scans.
                        if !adopted.contains(&key) {
                            self.tickets.disarm(
                                &intent.obligation.agent_identity,
                                &intent.obligation.input_id,
                            );
                        }
                        cancel_only.insert(key.clone());
                        // A delivery ACK predating run terminality cannot
                        // suppress the exact cancellation now owned here.
                        accepted.remove(&key);
                    }

                    if !adopted.contains(&key)
                        && let Some(receiver) = self.tickets.arm_recovered_exact(
                            &intent.obligation.agent_identity,
                            &intent.obligation.input_id,
                            &intent.obligation.run_id,
                            &intent.obligation.step_id,
                            &intent.expected_member,
                        )?
                    {
                        adopted.insert(key.clone());
                        new_adoptions.push(AdoptedWait {
                            key: key.clone(),
                            intent: intent.clone(),
                            receiver,
                        });
                    }

                    self.handle
                        .ensure_pump_for_obligation(&intent.obligation.agent_identity)
                        .await?;
                    if accepted.contains(&key)
                        && !lifecycle_quiescing
                        && !cancel_only.contains(&key)
                    {
                        retry.remove(&key);
                    } else {
                        // A lifecycle barrier overrides a prior delivery ACK:
                        // the next attempt is an exact tracked cancellation,
                        // never a duplicate delivery. Do not let the volatile
                        // ACK cache strand Pending custody during quiesce.
                        accepted.remove(&key);
                        retry.entry(key).or_default();
                        due_by_host
                            .entry(intent.obligation.host_id.clone())
                            .or_default()
                            .push(intent);
                    }
                }
                CustodyPhase::Committed | CustodyPhase::Resolved => {
                    accepted.remove(&key);
                    retry.remove(&key);
                    self.handle
                        .ensure_pump_for_obligation(&intent.obligation.agent_identity)
                        .await?;
                    convergence_candidates.insert(intent.obligation.run_id.clone());
                }
                CustodyPhase::Absent => {
                    accepted.remove(&key);
                    retry.remove(&key);
                    blocked_convergence_runs.insert(intent.obligation.run_id.clone());
                    tracing::debug!(
                        input_id = %intent.obligation.input_id,
                        dispatch_sequence = intent.obligation.dispatch_sequence,
                        "durable remote-turn intent is waiting for machine custody recovery"
                    );
                }
            }
        }

        // Select at most one due row per host. A per-host sequence cursor
        // prevents a blackholed low sequence from monopolizing its siblings;
        // the global cursor rotates the first host admitted to the bounded
        // bridge-I/O window.
        for intents in due_by_host.values_mut() {
            intents.sort_by_key(|intent| intent.obligation.dispatch_sequence);
        }
        let selected = select_due_intents(
            &due_by_host,
            retry,
            accepted,
            row_cursor,
            host_cursor,
            Instant::now(),
        );

        let mut attempts = FuturesUnordered::new();
        for intent in selected {
            let cancel_only = cancel_only.contains(&RemoteTurnKey::from_event(&intent.obligation));
            attempts.push(async move {
                let key = RemoteTurnKey::from_event(&intent.obligation);
                let result = self.reconcile_pending_intent(&intent, cancel_only).await;
                (key, intent, result)
            });
        }
        let mut attempt_error = None;
        while let Some((key, intent, result)) = attempts.next().await {
            match result {
                Ok(
                    PendingAttemptOutcome::Gone
                    | PendingAttemptOutcome::DeliveryRejected
                    | PendingAttemptOutcome::CancellationClosed
                    | PendingAttemptOutcome::TerminalObserved,
                ) => {
                    accepted.remove(&key);
                    retry.remove(&key);
                }
                Ok(PendingAttemptOutcome::DeliveryAccepted) => {
                    accepted.insert(key.clone());
                    retry.remove(&key);
                }
                Ok(PendingAttemptOutcome::NoChange) => {}
                Err(error) => {
                    tracing::debug!(
                        host_id = %intent.obligation.host_id,
                        input_id = %intent.obligation.input_id,
                        dispatch_sequence = intent.obligation.dispatch_sequence,
                        error = %error,
                        "remote-turn recovery attempt remains pending"
                    );
                    if attempt_error.is_none() {
                        attempt_error = Some(error);
                    }
                }
            }
        }
        for run_id in convergence_candidates.difference(&blocked_convergence_runs) {
            self.maybe_converge_recovered_run(run_id, converged_runs)
                .await?;
        }
        if let Some(error) = attempt_error {
            return Err(error);
        }
        let now = Instant::now();
        let next_retry_after = retry
            .values()
            .map(|state| state.next_attempt.saturating_duration_since(now))
            .min();
        Ok(ScanSummary {
            live_intents,
            next_retry_after,
        })
    }

    fn custody_phase(&self, obligation: &RemoteTurnObligationEvent) -> CustodyPhase {
        let exact = obligation_from_event(obligation);
        let state = self.handle.machine_state_watch_rx.borrow();
        if state.pending_remote_turn_outcomes.contains(&exact) {
            CustodyPhase::Pending
        } else if state.committed_remote_turn_outcomes.contains(&exact) {
            CustodyPhase::Committed
        } else if state.resolved_remote_turn_outcomes.contains(&exact) {
            CustodyPhase::Resolved
        } else {
            CustodyPhase::Absent
        }
    }

    async fn reconcile_pending_intent(
        &self,
        intent: &MobRunRemoteTurnIntent,
        cancel_only: bool,
    ) -> Result<PendingAttemptOutcome, MobError> {
        if self.custody_phase(&intent.obligation) != CustodyPhase::Pending {
            return Ok(PendingAttemptOutcome::Gone);
        }
        let lifecycle_quiescing = self
            .handle
            .machine_state_watch_rx
            .borrow()
            .placed_completion_lifecycle_quiescing;
        let cancellation_required = lifecycle_quiescing || cancel_only;
        let Some(member_ref) = self
            .exact_current_route(intent, cancellation_required)
            .await?
        else {
            return Ok(PendingAttemptOutcome::NoChange);
        };

        if cancellation_required {
            let response = self
                .provisioner
                .cancel_tracked_placed_input(
                    &member_ref,
                    &intent.expected_member,
                    &intent.obligation.input_id,
                )
                .await?;
            return match response.outcome {
                super::bridge_protocol::BridgeTrackedInputCancelOutcome::NoEffect => {
                    self.close_after_tracked_cancel(
                        intent,
                        TRACKED_CANCEL_NO_EFFECT_REASON,
                        MobRunRemoteTurnTrackedCancelTerminal::NoEffect,
                    )
                    .await?;
                    Ok(PendingAttemptOutcome::CancellationClosed)
                }
                super::bridge_protocol::BridgeTrackedInputCancelOutcome::Cancelled => {
                    self.close_after_tracked_cancel(
                        intent,
                        TRACKED_CANCELLED_REASON,
                        MobRunRemoteTurnTrackedCancelTerminal::Cancelled,
                    )
                    .await?;
                    Ok(PendingAttemptOutcome::CancellationClosed)
                }
                super::bridge_protocol::BridgeTrackedInputCancelOutcome::Terminal { record } => {
                    match self
                        .tickets
                        .consume_turn_outcome_record(&intent.expected_member, &record)
                        .await?
                    {
                        OutcomeRecordDisposition::Acknowledge => {
                            Ok(PendingAttemptOutcome::TerminalObserved)
                        }
                        OutcomeRecordDisposition::Retain | OutcomeRecordDisposition::Replay => {
                            Ok(PendingAttemptOutcome::NoChange)
                        }
                        OutcomeRecordDisposition::AcknowledgeUnclaimed
                        | OutcomeRecordDisposition::AcknowledgePlacedCompletion => {
                            Err(MobError::Internal(format!(
                                "tracked cancellation terminal for remote-turn input '{}' resolved outside its exact flow custody",
                                intent.obligation.input_id
                            )))
                        }
                    }
                }
                _ => Err(MobError::Internal(
                    "member host returned an unsupported tracked-input cancellation outcome"
                        .to_string(),
                )),
            };
        }

        match self
            .provisioner
            .deliver_turn_directive(
                &member_ref,
                TurnDirectiveDelivery {
                    input_id: intent.obligation.input_id.clone(),
                    content: intent.content.clone(),
                    handling_mode: intent.handling_mode,
                    expected_member: intent.expected_member.clone(),
                    directive: intent.directive.clone(),
                },
            )
            .await
        {
            Ok(receipt) => {
                if receipt.input_id != intent.obligation.input_id {
                    return Err(MobError::Internal(format!(
                        "remote-turn replay response changed input id '{}' to '{}'",
                        intent.obligation.input_id, receipt.input_id
                    )));
                }
                Ok(PendingAttemptOutcome::DeliveryAccepted)
            }
            Err(MobError::BridgeDeliveryRejected { cause, reason }) => {
                // Match the live actor-turn path: the durable failure reason
                // includes the typed cause display. In particular, a pre-V4
                // decode reject survives restart as
                // `turn_directive_unsupported` rather than serde prose.
                let semantic_reason = MobError::BridgeDeliveryRejected {
                    cause: cause.clone(),
                    reason,
                }
                .to_string();
                self.tickets.fail_armed_certified_no_effect(
                    &intent.obligation.agent_identity,
                    &intent.obligation.input_id,
                    semantic_reason,
                    *cause,
                );
                Ok(PendingAttemptOutcome::DeliveryRejected)
            }
            Err(error) => Err(error),
        }
    }

    async fn close_after_tracked_cancel(
        &self,
        intent: &MobRunRemoteTurnIntent,
        reason: &str,
        terminal: MobRunRemoteTurnTrackedCancelTerminal,
    ) -> Result<(), MobError> {
        self.handle
            .close_remote_turn_after_tracked_cancel(MobRunRemoteTurnReceipt {
                obligation: intent.obligation.clone(),
                outcome: MobRunRemoteTurnReceiptOutcome::TrackedInputCancel {
                    reason: reason.to_string(),
                    terminal,
                },
            })
            .await?;
        // Recovered adopters treat this as a re-scan signal. The actor has
        // already persisted the exact terminal and Disposed carrier, so no
        // volatile waiter is allowed to become an alternate authority.
        self.tickets.fail_armed(
            &intent.obligation.agent_identity,
            &intent.obligation.input_id,
            reason.to_string(),
        );
        Ok(())
    }

    async fn exact_current_route(
        &self,
        intent: &MobRunRemoteTurnIntent,
        cancellation: bool,
    ) -> Result<Option<MemberRef>, MobError> {
        let obligation = &intent.obligation;
        // The shell roster can lag member retirement and host revocation.  A
        // replay is admissible only while the machine still owns the exact
        // active route; otherwise a late background retry could resurrect a
        // released host/session binding.
        let machine_route_is_current = if cancellation {
            machine_cancel_route_is_current_for_recovery(
                &self.handle.machine_state_watch_rx.borrow(),
                obligation,
            )
        } else {
            machine_route_is_current_for_replay(
                &self.handle.machine_state_watch_rx.borrow(),
                obligation,
            )
        };
        if !machine_route_is_current {
            return Ok(None);
        }
        let entry = self
            .handle
            .roster
            .read()
            .await
            .get(&obligation.agent_identity)
            .cloned();
        let Some(entry) = entry else {
            return Ok(None);
        };
        if entry.generation != obligation.generation || entry.fence_token != obligation.fence_token
        {
            return Ok(None);
        }
        Ok(machine_verified_backend_route(
            &entry.member_ref,
            &obligation.member_session_id,
        ))
    }

    async fn finish_adopted_completion(
        &self,
        completion: AdoptedCompletion,
        converged_runs: &mut BTreeSet<RunId>,
    ) -> Result<(), MobError> {
        let outcome = match completion.outcome {
            Ok(outcome) => outcome,
            Err(_) => return Ok(()),
        };
        let receipt = self
            .receipt_from_outcome(&completion.intent, outcome)
            .await?;
        let run_id = receipt.obligation.run_id.clone();
        self.handle.commit_remote_turn_receipt(receipt).await?;
        self.maybe_converge_recovered_run(&run_id, converged_runs)
            .await
    }

    async fn receipt_from_outcome(
        &self,
        intent: &MobRunRemoteTurnIntent,
        outcome: FlowTurnOutcome,
    ) -> Result<MobRunRemoteTurnReceipt, MobError> {
        let receipt_outcome = match outcome {
            FlowTurnOutcome::Completed {
                output,
                structured_output,
            } => {
                let run = self
                    .handle
                    .run_store
                    .get_run(&intent.obligation.run_id)
                    .await?
                    .ok_or_else(|| MobError::RunNotFound(intent.obligation.run_id.clone()))?;
                let flow = self
                    .handle
                    .definition
                    .flows
                    .get(&run.flow_id)
                    .ok_or_else(|| MobError::FlowNotFound(run.flow_id.clone()))?;
                let step = flow.steps.get(&intent.obligation.step_id).ok_or_else(|| {
                    MobError::Internal(format!(
                        "recovered remote-turn step '{}' is absent from flow '{}'",
                        intent.obligation.step_id, run.flow_id
                    ))
                })?;
                match super::flow::recovered_completed_output_value(
                    &output,
                    structured_output,
                    &intent.obligation.step_id,
                    &intent.obligation.agent_identity,
                    &step.effective_output_format(),
                ) {
                    Ok(value) => MobRunRemoteTurnReceiptOutcome::Completed { value },
                    Err(reason) => MobRunRemoteTurnReceiptOutcome::Failed {
                        reason,
                        no_effect_proof: None,
                    },
                }
            }
            FlowTurnOutcome::Failed {
                reason,
                disposition,
            } => MobRunRemoteTurnReceiptOutcome::Failed {
                reason,
                no_effect_proof: match disposition {
                    FlowTurnFailureDisposition::ObservedTerminal => None,
                    FlowTurnFailureDisposition::CertifiedNoEffect { cause } => Some(cause),
                },
            },
            FlowTurnOutcome::Canceled => MobRunRemoteTurnReceiptOutcome::Failed {
                reason: "remote turn event stream closed before terminal outcome".to_string(),
                no_effect_proof: None,
            },
        };
        Ok(MobRunRemoteTurnReceipt {
            obligation: intent.obligation.clone(),
            outcome: receipt_outcome,
        })
    }

    async fn maybe_converge_recovered_run(
        &self,
        run_id: &RunId,
        converged_runs: &mut BTreeSet<RunId>,
    ) -> Result<(), MobError> {
        if !self.recovered_runs.contains(run_id) || converged_runs.contains(run_id) {
            return Ok(());
        }
        let dsl_run_id = run_id.to_string();
        let has_pending_sibling = self
            .handle
            .machine_state_watch_rx
            .borrow()
            .pending_remote_turn_outcomes
            .iter()
            .any(|obligation| obligation.run_id.0 == dsl_run_id);
        if has_pending_sibling {
            return Ok(());
        }
        // The sealed actor command invokes the existing recovered-run
        // cancellation/terminalization path. It never re-enters FlowEngine,
        // so terminal adoption cannot mint a replacement remote input.
        self.handle
            .converge_recovered_flow_run(run_id.clone())
            .await?;
        converged_runs.insert(run_id.clone());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;
    use chrono::{DateTime, Utc};
    use meerkat_contracts::wire::supervisor_bridge::{
        BridgeMemberIncarnation, BridgeTurnCorrelation, BridgeTurnDirective,
    };

    use crate::event::MobEvent;
    use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, FlowId, Generation, StepId};
    use crate::run::{MobRun, MobRunStatus};
    use crate::store::{
        InMemoryMobEventStore, InMemoryMobRunStore, MobEventReceiver, MobStoreError,
        authority_validating_mob_run_store, private,
    };

    #[test]
    fn sessionless_peer_route_remains_eligible_after_machine_exact_match() {
        let sessionless = MemberRef::BackendPeer {
            peer_id: "peer-b".to_string(),
            address: "tcp://host-b".to_string(),
            pubkey: [7; 32],
            bootstrap_token: None,
            session_id: None,
        };
        assert_eq!(
            machine_verified_backend_route(&sessionless, "member-session"),
            Some(sessionless),
            "peer-only rebind must not strand a machine-verified durable replay"
        );

        let wrong_session = MemberRef::BackendPeer {
            peer_id: "peer-b".to_string(),
            address: "tcp://host-b".to_string(),
            pubkey: [7; 32],
            bootstrap_token: None,
            session_id: Some(meerkat_core::types::SessionId::new()),
        };
        assert!(
            machine_verified_backend_route(&wrong_session, "member-session").is_none(),
            "a conflicting shell session copy must still fail closed"
        );
    }

    #[test]
    fn placed_recovery_replays_the_same_input_without_local_startup_ready() {
        let mob_id = MobId::from("placed-recovery-route");
        let run_id = RunId::new();
        let intent = test_intent(&mob_id, &run_id, 41);
        let obligation = &intent.obligation;
        let identity = mob_dsl::AgentIdentity(obligation.agent_identity.to_string());
        let host = mob_dsl::HostId(obligation.host_id.clone());
        let runtime = mob_dsl::AgentRuntimeId("remote-worker:1".to_string());
        let mut state = mob_dsl::MobMachineAuthority::new().state().clone();
        state.mob_hosts.insert(host.clone());
        state
            .host_bind_phase
            .insert(host.clone(), mob_dsl::HostBindPhase::Bound);
        state.host_durable_sessions.insert(host.clone(), true);
        state.host_tracked_input_cancel.insert(host.clone(), true);
        state.host_protocol_min.insert(host.clone(), 4);
        state.host_protocol_max.insert(host.clone(), 4);
        state
            .host_binding_generations
            .insert(host.clone(), obligation.host_binding_generation);
        state
            .current_placed_spawn_host_binding_generations
            .insert(identity.clone(), obligation.host_binding_generation);
        state
            .member_placement
            .insert(identity.clone(), host.clone());
        state.identity_runtime_generations.insert(
            identity.clone(),
            mob_dsl::Generation(obligation.generation.get()),
        );
        state.identity_runtime_fence_tokens.insert(
            identity.clone(),
            mob_dsl::FenceToken(obligation.fence_token.get()),
        );
        state.member_session_bindings.insert(
            identity.clone(),
            mob_dsl::SessionId(obligation.member_session_id.clone()),
        );
        state.identity_to_runtime.insert(identity, runtime.clone());
        state.live_runtime_ids.insert(runtime.clone());
        assert!(
            !state.member_state_markers.contains_key(&runtime),
            "active member lifecycle marker is canonically sparse"
        );
        assert!(
            !state.member_startup_ready.contains(&runtime),
            "placed PeerOnly recovery intentionally has no local startup-ready fact"
        );
        assert!(
            machine_route_is_current_for_replay(&state, obligation),
            "the exact sparse-active placed incarnation must remain eligible for same-ID recovery"
        );
        state.host_tracked_input_cancel.insert(host.clone(), false);
        assert!(
            !machine_route_is_current_for_replay(&state, obligation),
            "fresh/recovered remote-turn delivery requires exact tracked-input cancellation support"
        );
        state.host_tracked_input_cancel.insert(host.clone(), true);
        state
            .member_state_markers
            .insert(runtime.clone(), mob_dsl::MobMemberState::Retiring);
        assert!(
            !machine_route_is_current_for_replay(&state, obligation),
            "an explicit Retiring marker must close same-ID replay admission"
        );
        assert!(
            machine_cancel_route_is_current_for_recovery(&state, obligation),
            "lifecycle retirement must preserve the exact negative-delivery cancellation route"
        );
        state.member_state_markers.remove(&runtime);
        state
            .host_binding_generations
            .insert(host, obligation.host_binding_generation + 1);
        assert!(
            !machine_route_is_current_for_replay(&state, obligation),
            "a delayed G1 intent must not replay after revoke/rebind advances the host to G2"
        );

        let member_ref = MemberRef::BackendPeer {
            peer_id: "peer-b".to_string(),
            address: "tcp://host-b".to_string(),
            pubkey: [7; 32],
            bootstrap_token: None,
            session_id: None,
        };
        assert!(
            machine_verified_backend_route(&member_ref, &obligation.member_session_id).is_some()
        );
        let replay = TurnDirectiveDelivery {
            input_id: obligation.input_id.clone(),
            content: intent.content.clone(),
            handling_mode: intent.handling_mode,
            expected_member: intent.expected_member.clone(),
            directive: intent.directive.clone(),
        };
        assert_eq!(
            replay.input_id, obligation.input_id,
            "recovery must replay the durable idempotency key, never mint a replacement"
        );
    }

    struct WriteThenErrorEventStore {
        inner: InMemoryMobEventStore,
        write_record_then_error: AtomicBool,
    }

    impl WriteThenErrorEventStore {
        fn new() -> Self {
            Self {
                inner: InMemoryMobEventStore::new(),
                write_record_then_error: AtomicBool::new(false),
            }
        }

        fn fail_next_record_after_write(&self) {
            self.write_record_then_error.store(true, Ordering::SeqCst);
        }
    }

    impl private::MobEventStoreSealed for WriteThenErrorEventStore {}

    #[async_trait]
    impl MobEventStore for WriteThenErrorEventStore {
        async fn append(&self, event: NewMobEvent) -> Result<MobEvent, MobStoreError> {
            let record = matches!(
                &event.kind,
                MobEventKind::RemoteTurnObligationRecorded { .. }
            );
            let stored = self.inner.append(event).await?;
            if record && self.write_record_then_error.swap(false, Ordering::SeqCst) {
                return Err(MobStoreError::Internal(
                    "fault-injected wrote-then-error Record append".to_string(),
                ));
            }
            Ok(stored)
        }

        async fn append_terminal_event_if_absent(
            &self,
            event: NewMobEvent,
        ) -> Result<Option<MobEvent>, MobStoreError> {
            self.inner.append_terminal_event_if_absent(event).await
        }

        async fn append_batch(
            &self,
            events: Vec<NewMobEvent>,
        ) -> Result<Vec<MobEvent>, MobStoreError> {
            self.inner.append_batch(events).await
        }

        async fn poll(
            &self,
            after_cursor: u64,
            limit: usize,
        ) -> Result<Vec<MobEvent>, MobStoreError> {
            self.inner.poll(after_cursor, limit).await
        }

        async fn replay_all(&self) -> Result<Vec<MobEvent>, MobStoreError> {
            self.inner.replay_all().await
        }

        async fn latest_cursor(&self) -> Result<u64, MobStoreError> {
            self.inner.latest_cursor().await
        }

        fn subscribe(&self) -> Result<MobEventReceiver, MobStoreError> {
            self.inner.subscribe()
        }

        async fn clear(&self) -> Result<(), MobStoreError> {
            self.inner.clear().await
        }

        async fn prune(&self, older_than: DateTime<Utc>) -> Result<u64, MobStoreError> {
            self.inner.prune(older_than).await
        }
    }

    fn test_intent(mob_id: &MobId, run_id: &RunId, sequence: u64) -> MobRunRemoteTurnIntent {
        let identity = AgentIdentity::from("remote-worker");
        let step_id = StepId::from("step");
        let input_id = meerkat_core::time_compat::new_uuid_v7().to_string();
        let obligation = RemoteTurnObligationEvent {
            agent_identity: identity.clone(),
            host_id: "host-b".to_string(),
            host_binding_generation: 1,
            member_session_id: "member-session".to_string(),
            generation: Generation::new(1),
            fence_token: FenceToken::new(7),
            dispatch_sequence: sequence,
            input_id,
            run_id: run_id.clone(),
            step_id: step_id.clone(),
        };
        MobRunRemoteTurnIntent {
            expected_member: BridgeMemberIncarnation {
                mob_id: mob_id.to_string(),
                agent_identity: identity.to_string(),
                host_id: obligation.host_id.clone(),
                binding_generation: obligation.host_binding_generation,
                member_session_id: obligation.member_session_id.clone(),
                generation: obligation.generation.get(),
                fence_token: obligation.fence_token.get(),
            },
            content: meerkat_core::types::ContentInput::from("do it"),
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            directive: BridgeTurnDirective {
                correlation: BridgeTurnCorrelation {
                    run_id: run_id.to_string(),
                    step_id: step_id.to_string(),
                },
                tool_overlay: None,
            },
            obligation,
        }
    }

    #[test]
    fn dropping_adopted_completion_set_cancels_unfinished_waiters() {
        let mob_id = MobId::from("structured-adopted-waiter");
        let run_id = RunId::new();
        let intent = test_intent(&mob_id, &run_id, 41);
        let key = RemoteTurnKey::from_event(&intent.obligation);
        let (tx, receiver) = oneshot::channel();
        let waiters = FuturesUnordered::<AdoptedCompletionFuture>::new();
        waiters.push(
            AdoptedWait {
                key,
                intent,
                receiver,
            }
            .into_future(),
        );

        drop(waiters);

        assert!(
            tx.send(FlowTurnOutcome::Canceled).is_err(),
            "dropping the reconciler's structured waiter set must drop every pending receiver"
        );
    }

    #[test]
    fn remote_turn_recovery_host_window_is_bounded() {
        assert_eq!(MAX_CONCURRENT_HOSTS, 8);
    }

    #[test]
    fn both_typed_no_effect_receipts_are_replay_closed() {
        let no_effect = MobRunRemoteTurnReceiptOutcome::Failed {
            reason: "host rejected before admission".to_string(),
            no_effect_proof: Some(
                meerkat_contracts::wire::supervisor_bridge::BridgeDeliveryRejectionCause::TurnDirectiveUnsupported {
                    detail: "delivery unsupported before admission".to_string(),
                },
            ),
        };
        assert_eq!(
            auto_dispose_receipt_reason(&no_effect),
            Some(("host rejected before admission", false))
        );

        let tracked_cancel = MobRunRemoteTurnReceiptOutcome::TrackedInputCancel {
            reason: "cancelled during quiesce".to_string(),
            terminal: MobRunRemoteTurnTrackedCancelTerminal::Cancelled,
        };
        assert_eq!(
            auto_dispose_receipt_reason(&tracked_cancel),
            Some(("cancelled during quiesce", true))
        );

        let observed_failure = MobRunRemoteTurnReceiptOutcome::Failed {
            reason: "runtime failed after admission".to_string(),
            no_effect_proof: None,
        };
        assert_eq!(auto_dispose_receipt_reason(&observed_failure), None);
    }

    #[test]
    fn remote_turn_recovery_rotates_hosts_and_rows_with_one_per_host() {
        let mob_id = MobId::from("remote-turn-fair-selector");
        let run_id = RunId::new();
        let mut by_host = BTreeMap::<String, Vec<MobRunRemoteTurnIntent>>::new();
        let mut retry = BTreeMap::<RemoteTurnKey, RetryState>::new();
        let accepted = BTreeSet::new();
        for host_index in 0_u64..10 {
            let host = format!("host-{host_index:02}");
            for row in 1_u64..=2 {
                let mut intent = test_intent(&mob_id, &run_id, host_index * 10 + row);
                intent.obligation.host_id = host.clone();
                intent.expected_member.host_id = host.clone();
                retry.insert(
                    RemoteTurnKey::from_event(&intent.obligation),
                    RetryState::default(),
                );
                by_host.entry(host.clone()).or_default().push(intent);
            }
        }
        let mut row_cursor = BTreeMap::new();
        let mut host_cursor = 0usize;
        let now = Instant::now() + Duration::from_millis(1);
        let first = select_due_intents(
            &by_host,
            &mut retry,
            &accepted,
            &mut row_cursor,
            &mut host_cursor,
            now,
        );
        assert_eq!(first.len(), MAX_CONCURRENT_HOSTS);
        assert_eq!(
            first
                .iter()
                .map(|intent| intent.obligation.host_id.as_str())
                .collect::<BTreeSet<_>>()
                .len(),
            first.len(),
            "one scan may admit at most one bridge attempt per host"
        );

        let second = select_due_intents(
            &by_host,
            &mut retry,
            &accepted,
            &mut row_cursor,
            &mut host_cursor,
            now + Duration::from_secs(1),
        );
        assert_eq!(second.len(), MAX_CONCURRENT_HOSTS);
        assert_eq!(second[0].obligation.host_id, "host-08");
        let host_zero = second
            .iter()
            .find(|intent| intent.obligation.host_id == "host-00")
            .expect("global rotation returns to host-00 after the tail hosts");
        assert_eq!(
            host_zero.obligation.dispatch_sequence, 2,
            "the per-host cursor advances past the first attempted row"
        );
    }

    #[test]
    fn accepted_remote_turn_does_not_monopolize_its_host_retry_cursor() {
        let mob_id = MobId::from("remote-turn-accepted-selector");
        let run_id = RunId::new();
        let first = test_intent(&mob_id, &run_id, 1);
        let second = test_intent(&mob_id, &run_id, 2);
        let first_key = RemoteTurnKey::from_event(&first.obligation);
        let second_key = RemoteTurnKey::from_event(&second.obligation);
        let by_host = BTreeMap::from([(
            first.obligation.host_id.clone(),
            vec![first, second.clone()],
        )]);
        let mut retry = BTreeMap::from([
            (first_key.clone(), RetryState::default()),
            (second_key, RetryState::default()),
        ]);
        let accepted = BTreeSet::from([first_key]);
        let mut row_cursor = BTreeMap::new();
        let mut host_cursor = 0usize;

        let selected = select_due_intents(
            &by_host,
            &mut retry,
            &accepted,
            &mut row_cursor,
            &mut host_cursor,
            Instant::now() + Duration::from_millis(1),
        );

        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected[0].obligation.dispatch_sequence, second.obligation.dispatch_sequence,
            "an ACKed row is suppressed while its unsent sibling advances"
        );
    }

    async fn seed_run_and_dispatch(
        run_store: &Arc<dyn MobRunStore>,
        event_store: &Arc<dyn MobEventStore>,
        mob_id: &MobId,
        run_id: &RunId,
    ) {
        let step_id = StepId::from("step");
        let flow_id = FlowId::from("flow");
        let run = MobRun::authority_backed_for_steps(
            run_id.clone(),
            mob_id.clone(),
            flow_id.clone(),
            [step_id.clone()],
            MobRunStatus::Running,
            serde_json::json!({}),
        )
        .expect("authority-backed run");
        run_store.create_run(run).await.expect("create run");
        event_store
            .append(NewMobEvent {
                mob_id: mob_id.clone(),
                timestamp: None,
                kind: MobEventKind::FlowStarted {
                    run_id: run_id.clone(),
                    flow_id,
                    params: serde_json::json!({}),
                },
            })
            .await
            .expect("append FlowStarted");
        event_store
            .append(NewMobEvent {
                mob_id: mob_id.clone(),
                timestamp: None,
                kind: MobEventKind::StepDispatched {
                    run_id: run_id.clone(),
                    step_id,
                    target: AgentRuntimeId::new(
                        AgentIdentity::from("remote-worker"),
                        Generation::new(1),
                    ),
                },
            })
            .await
            .expect("append StepDispatched");
    }

    #[tokio::test]
    async fn wrote_then_error_record_is_not_duplicated_and_sequence_stays_reserved() {
        let mob_id = MobId::from("reconcile-write-error");
        let run_id = RunId::new();
        let raw_runs: Arc<dyn MobRunStore> = Arc::new(InMemoryMobRunStore::new());
        let run_store = authority_validating_mob_run_store(raw_runs);
        let concrete_events = Arc::new(WriteThenErrorEventStore::new());
        let event_store: Arc<dyn MobEventStore> = concrete_events.clone();
        seed_run_and_dispatch(&run_store, &event_store, &mob_id, &run_id).await;
        let intent = test_intent(&mob_id, &run_id, 41);
        run_store
            .put_remote_turn_intent(&run_id, &intent)
            .await
            .expect("put intent");
        concrete_events.fail_next_record_after_write();

        let first = prepare_remote_turn_recovery_material(
            run_store.clone(),
            event_store.clone(),
            &mob_id,
            &event_store.replay_all().await.expect("epoch events"),
            true,
        )
        .await
        .expect("wrote-then-error is recoverable");
        assert_eq!(first.intents[0].obligation.dispatch_sequence, 41);

        let second = prepare_remote_turn_recovery_material(
            run_store,
            event_store.clone(),
            &mob_id,
            &event_store
                .replay_all()
                .await
                .expect("replayed epoch events"),
            true,
        )
        .await
        .expect("second recovery pass");
        assert_eq!(second.intents[0].obligation.dispatch_sequence, 41);
        let record_count = event_store
            .replay_all()
            .await
            .expect("events")
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    MobEventKind::RemoteTurnObligationRecorded { .. }
                )
            })
            .count();
        assert_eq!(record_count, 1, "ambiguous write must not duplicate Record");
    }

    #[tokio::test]
    async fn actor_allocator_never_reuses_intent_or_receipt_only_sequence() {
        let mob_id = MobId::from("allocator-private-highwater");
        let run_id = RunId::new();
        let raw_runs: Arc<dyn MobRunStore> = Arc::new(InMemoryMobRunStore::new());
        let run_store = authority_validating_mob_run_store(raw_runs);
        let event_store: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
        seed_run_and_dispatch(&run_store, &event_store, &mob_id, &run_id).await;
        let intent = test_intent(&mob_id, &run_id, 41);
        run_store
            .put_remote_turn_intent(&run_id, &intent)
            .await
            .expect("put seq-41 private intent without public Record");

        let next = super::super::actor::next_remote_turn_dispatch_sequence(&run_store, &mob_id, 0)
            .await
            .expect("allocate after private intent");
        assert_eq!(next, 42, "missing/ambiguous Record cannot free sequence 41");

        let receipt = MobRunRemoteTurnReceipt {
            obligation: intent.obligation.clone(),
            outcome: MobRunRemoteTurnReceiptOutcome::Completed {
                value: serde_json::json!({"ok": true}),
            },
        };
        run_store
            .put_remote_turn_receipt(&run_id, &receipt)
            .await
            .expect("put receipt while exact intent exists");
        run_store
            .delete_remote_turn_intent(&run_id, 41)
            .await
            .expect("simulate crash-left receipt-only survivor");

        let next = super::super::actor::next_remote_turn_dispatch_sequence(&run_store, &mob_id, 0)
            .await
            .expect("allocate after receipt-only survivor");
        assert_eq!(
            next, 42,
            "receipt-only sequence 41 remains permanently burned"
        );
    }

    #[tokio::test]
    async fn stale_generation_intent_cannot_join_current_epoch_dispatch() {
        let mob_id = MobId::from("reconcile-stale-generation");
        let run_id = RunId::new();
        let raw_runs: Arc<dyn MobRunStore> = Arc::new(InMemoryMobRunStore::new());
        let run_store = authority_validating_mob_run_store(raw_runs);
        let event_store: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
        seed_run_and_dispatch(&run_store, &event_store, &mob_id, &run_id).await;

        let mut stale = test_intent(&mob_id, &run_id, 41);
        stale.obligation.generation = Generation::new(2);
        stale.expected_member.generation = 2;
        run_store
            .put_remote_turn_intent(&run_id, &stale)
            .await
            .expect("put structurally valid stale-generation intent");

        let material = prepare_remote_turn_recovery_material(
            run_store.clone(),
            event_store.clone(),
            &mob_id,
            &event_store.replay_all().await.expect("epoch events"),
            true,
        )
        .await
        .expect("stale row is scrubbed rather than adopted");
        assert!(material.intents.is_empty());
        assert!(
            run_store
                .list_remote_turn_intents(&run_id)
                .await
                .expect("remaining intents")
                .is_empty(),
            "same run/step/identity at a different generation must not seed custody or replay"
        );
    }

    #[tokio::test]
    async fn live_scan_stale_event_snapshot_preserves_fresh_private_rows() {
        let mob_id = MobId::from("reconcile-live-stale-snapshot");
        let run_id = RunId::new();
        let raw_runs: Arc<dyn MobRunStore> = Arc::new(InMemoryMobRunStore::new());
        let run_store = authority_validating_mob_run_store(raw_runs);
        let event_store: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
        seed_run_and_dispatch(&run_store, &event_store, &mob_id, &run_id).await;

        // Model a periodic scan that captured the epoch immediately before
        // this dispatch. Its later private-row read races with the actor's
        // intent/receipt persistence and therefore cannot authorize deletion.
        let stale_epoch_events = event_store
            .replay_all()
            .await
            .expect("epoch events")
            .into_iter()
            .filter(|event| !matches!(event.kind, MobEventKind::StepDispatched { .. }))
            .collect::<Vec<_>>();
        let intent = test_intent(&mob_id, &run_id, 41);
        let receipt = MobRunRemoteTurnReceipt {
            obligation: intent.obligation.clone(),
            outcome: MobRunRemoteTurnReceiptOutcome::Completed {
                value: serde_json::json!({"ok": true}),
            },
        };
        run_store
            .put_remote_turn_intent(&run_id, &intent)
            .await
            .expect("put fresh intent after event snapshot");
        run_store
            .put_remote_turn_receipt(&run_id, &receipt)
            .await
            .expect("put fresh receipt after event snapshot");

        let stale = prepare_remote_turn_recovery_material(
            run_store.clone(),
            event_store.clone(),
            &mob_id,
            &stale_epoch_events,
            false,
        )
        .await
        .expect("live stale snapshot is non-destructive");
        assert!(stale.intents.is_empty());
        assert!(stale.receipts.is_empty());
        assert_eq!(
            run_store
                .list_remote_turn_intents(&run_id)
                .await
                .expect("preserved intent"),
            vec![intent.clone()]
        );
        assert_eq!(
            run_store
                .list_remote_turn_receipts(&run_id)
                .await
                .expect("preserved receipt"),
            vec![receipt.clone()]
        );

        let fresh = prepare_remote_turn_recovery_material(
            run_store,
            event_store.clone(),
            &mob_id,
            &event_store.replay_all().await.expect("fresh epoch events"),
            false,
        )
        .await
        .expect("next live scan adopts the exact rows");
        assert_eq!(fresh.intents, vec![intent]);
        assert_eq!(fresh.receipts, vec![receipt]);
    }

    #[tokio::test]
    async fn reused_input_id_cannot_alias_two_dispatch_sequences() {
        let mob_id = MobId::from("reconcile-input-id-collision");
        let run_id = RunId::new();
        let raw_runs: Arc<dyn MobRunStore> = Arc::new(InMemoryMobRunStore::new());
        let run_store = authority_validating_mob_run_store(raw_runs);
        let event_store: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
        seed_run_and_dispatch(&run_store, &event_store, &mob_id, &run_id).await;

        let first = test_intent(&mob_id, &run_id, 41);
        let mut alias = test_intent(&mob_id, &run_id, 42);
        alias.obligation.input_id = first.obligation.input_id.clone();
        run_store
            .put_remote_turn_intent(&run_id, &first)
            .await
            .expect("put first intent");
        run_store
            .put_remote_turn_intent(&run_id, &alias)
            .await
            .expect("put individually valid colliding intent");

        let error = match prepare_remote_turn_recovery_material(
            run_store,
            event_store.clone(),
            &mob_id,
            &event_store.replay_all().await.expect("epoch events"),
            true,
        )
        .await
        {
            Ok(_) => panic!("one idempotency key cannot name two exact obligations"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("input id"));
    }

    #[tokio::test]
    async fn finalized_crash_cleanup_waits_for_public_chain_validation() {
        let mob_id = MobId::from("reconcile-final-cleanup");
        let run_id = RunId::new();
        let raw_runs: Arc<dyn MobRunStore> = Arc::new(InMemoryMobRunStore::new());
        let run_store = authority_validating_mob_run_store(raw_runs);
        let event_store: Arc<dyn MobEventStore> = Arc::new(InMemoryMobEventStore::new());
        seed_run_and_dispatch(&run_store, &event_store, &mob_id, &run_id).await;
        let intent = test_intent(&mob_id, &run_id, 9);
        let receipt = MobRunRemoteTurnReceipt {
            obligation: intent.obligation.clone(),
            outcome: MobRunRemoteTurnReceiptOutcome::Completed {
                value: serde_json::json!({"ok": true}),
            },
        };
        run_store
            .put_remote_turn_intent(&run_id, &intent)
            .await
            .expect("put intent");
        run_store
            .put_remote_turn_receipt(&run_id, &receipt)
            .await
            .expect("put receipt");
        for kind in [
            MobEventKind::RemoteTurnObligationRecorded {
                obligation: intent.obligation.clone(),
            },
            MobEventKind::StepTargetCompleted {
                run_id: run_id.clone(),
                step_id: intent.obligation.step_id.clone(),
                target: AgentRuntimeId::new(
                    intent.obligation.agent_identity.clone(),
                    intent.obligation.generation,
                ),
                output: Some(serde_json::json!({"ok": true})),
                remote_turn_obligation: Some(intent.obligation.clone()),
            },
            MobEventKind::RemoteTurnOutcomeResolved {
                obligation: intent.obligation.clone(),
            },
            MobEventKind::RemoteTurnOutcomeAcknowledged {
                obligation: intent.obligation.clone(),
            },
        ] {
            event_store
                .append(NewMobEvent {
                    mob_id: mob_id.clone(),
                    timestamp: None,
                    kind,
                })
                .await
                .expect("append final carrier");
        }

        let material = prepare_remote_turn_recovery_material(
            run_store.clone(),
            event_store.clone(),
            &mob_id,
            &event_store.replay_all().await.expect("events"),
            true,
        )
        .await
        .expect("prepare final cleanup");
        assert!(material.intents.is_empty());
        assert!(material.receipts.is_empty());
        assert_eq!(
            material.finalized_privacy_cleanup,
            vec![FinalizedRemoteTurnPrivacyCleanup {
                obligation: intent.obligation.clone(),
                finalizer: RemoteTurnPublicFinalizer::Acknowledged,
            }]
        );
        assert!(
            material.deferred_run_ids.contains(&run_id),
            "a finalized public StepTarget carrier still defers FlowRun convergence"
        );
        assert_eq!(
            run_store
                .list_remote_turn_receipts(&run_id)
                .await
                .expect("receipts before validation")
                .len(),
            1,
            "prepare must retain the receipt until the public finalizer chain is validated"
        );
        assert_eq!(
            run_store
                .list_remote_turn_intents(&run_id)
                .await
                .expect("intents before validation")
                .len(),
            1,
            "prepare must retain the receipt's exact intent join until validation"
        );

        apply_remote_turn_finalized_privacy_cleanup(
            run_store.clone(),
            &material.finalized_privacy_cleanup,
        )
        .await
        .expect("apply validated cleanup");
        assert!(
            run_store
                .list_remote_turn_receipts(&run_id)
                .await
                .expect("receipts")
                .is_empty()
        );
        assert!(
            run_store
                .list_remote_turn_intents(&run_id)
                .await
                .expect("intents")
                .is_empty()
        );
    }
}
