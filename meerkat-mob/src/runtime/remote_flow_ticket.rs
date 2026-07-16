//! RemoteFlowTicketRegistry — the ONE correlation point between remote
//! flow-turn dispatch, the member-host journal sidecar, and flow-step
//! completion (multi-host mobs §18 O2, phase 6; design-flow-spine DEC-P6F-6).
//!
//! One registry per mob, `Arc`-shared between the flow-turn executor
//! (arm/disarm) and the events-backbone poll pump (row/record/generation
//! observations). Payload reconstruction: the FROZEN wire record
//! `BridgeTurnOutcomeRecord` carries no output payload, so the step's
//! `Completed { output, structured_output }` comes from a per-member fold of
//! the pumped DURABLE event rows through the shared
//! [`meerkat_core::turn_terminal::TurnTerminalClassifier`]; the journal
//! record pins WHICH fold terminal belongs to WHICH `input_id`
//! (`terminal_seq` in the durable `StoredEvent.seq` domain — gotcha 8).
//!
//! Page contract (cross-lane seam, ADJ-P6-10): within one pumped page the
//! pump feeds `observe_member_event_row` for every row FIRST, then
//! `consume_turn_outcome_record` for each sidecar record; the sidecar serves
//! a bounded page of retained journal rows and the pump/registry no-op
//! duplicate records by `(generation, input_id)`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use super::bridge_protocol::{
    BridgeDeliveryOutcome, BridgeMemberIncarnation, BridgeTurnOutcomeAck, BridgeTurnOutcomeRecord,
    WireFlowTurnOutcome,
};
use super::turn_executor::{FlowTurnFailureDisposition, FlowTurnOutcome};
use crate::error::MobError;
use crate::event::RemoteTurnObligationEvent;
use crate::ids::{AgentIdentity, RunId, StepId};
use crate::machines::mob_machine as mob_dsl;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use meerkat_core::event::AgentEvent;
use meerkat_core::turn_terminal::{
    ClassifiedTurnTerminal, TurnTerminalClassifier, TurnTerminalKind,
};
use tokio::sync::oneshot;

/// Retained fold terminals per member (a session executes queued inputs
/// serially, so a handful of recent terminals is ample).
const RECENT_TERMINALS_PER_MEMBER: usize = 256;
/// Consumed-record dedup memory per member (late/duplicate sidecar rows).
const CONSUMED_RECORDS_PER_MEMBER: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct RemoteResidencyKey {
    mob_id: String,
    agent_identity: String,
    host_id: String,
    binding_generation: u64,
    member_session_id: String,
    generation: u64,
    fence_token: u64,
}

impl From<&BridgeMemberIncarnation> for RemoteResidencyKey {
    fn from(value: &BridgeMemberIncarnation) -> Self {
        Self {
            mob_id: value.mob_id.clone(),
            agent_identity: value.agent_identity.clone(),
            host_id: value.host_id.clone(),
            binding_generation: value.binding_generation,
            member_session_id: value.member_session_id.clone(),
            generation: value.generation,
            fence_token: value.fence_token,
        }
    }
}
/// Typed step-failure reason for a mid-step member rematerialization —
/// ONE string shared by the pump's generation watch, the controlling-side
/// respawn mark, and the dispatch-unwind attribution, so the run ledger
/// names the generation change identically on every path.
pub(crate) const REMATERIALIZED_STEP_FAILURE_REASON: &str =
    "member rematerialized at new generation during flow step";
/// Typed step-failure reason shared by an exact host-member release, its
/// actor-authored public terminal/receipt, and the matching ticket wake.
pub(crate) const MEMBER_RELEASED_STEP_FAILURE_REASON: &str =
    "remote member incarnation was durably released during flow step";
/// Typed step-failure reason for a turn whose exact host incarnation was
/// durably revoked before its terminal could be observed. The controlling
/// actor uses this one value for the public terminal, the durable receipt,
/// and the armed-ticket wake so a queued/recovered commit can compare exact
/// replay material rather than accepting a looser failure class.
pub(crate) const HOST_REVOKED_STEP_FAILURE_REASON: &str =
    "remote host was durably revoked during flow step";

// ---------------------------------------------------------------------------
// Cross-lane carriers + named hooks (consumed by exact name; ADJ-P4-9
// discipline)
// ---------------------------------------------------------------------------

/// Request carrier for the provisioner's `deliver_turn_directive` verb
/// (DEC-P6F-4). The CALLER mints and owns `input_id` (uuid v7): it is the
/// obligation key, the journal key, and the ticket key.
pub struct TurnDirectiveDelivery {
    pub input_id: String,
    pub content: meerkat_core::types::ContentInput,
    pub handling_mode: meerkat_core::types::HandlingMode,
    pub expected_member: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    pub directive: super::bridge_protocol::BridgeTurnDirective,
}

/// Actor-finalized durable intent plus the exact current route snapshot used
/// for its admission. The caller may send only this material; preloaded
/// roster entries never regain authority after the actor turn.
pub(crate) struct ReservedRemoteTurnIntent {
    pub intent: crate::run::MobRunRemoteTurnIntent,
    pub member_ref: crate::event::MemberRef,
}

/// Accept-class receipt from `deliver_turn_directive`: the delivery was
/// `Accepted` or `Deduplicated` (a `Rejected` outcome surfaces as a typed
/// `MobError` instead — never a receipt).
pub struct BridgeDeliveryOutcomeReceipt {
    pub input_id: String,
    pub canonical_input_id: Option<String>,
    pub outcome: BridgeDeliveryOutcome,
}

/// Pump-liveness hook (A17): dispatching a directive-bearing delivery must
/// keep the member's poll pump alive while the obligation is outstanding.
/// Named here by the flow lane; the events lane implements it for its
/// `MemberEventPumpManager` (`ensure_pump_for_obligation`) and installs it
/// on the executor at mob build.
pub trait RemoteTurnObligationPump: Send + Sync {
    fn ensure_pump_for_obligation(&self, identity: &AgentIdentity);
}

/// Obligation-resolve seam: the registry never touches the DSL authority
/// directly — resolution is submitted through the actor's machine-input
/// path (`ResolveRemoteTurnObligation` is per_phase + set-idempotent, so
/// duplicate resolves are harmless).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutcomeRecordDisposition {
    Retain,
    Replay,
    Acknowledge,
    /// The exact terminal fold is validated, but no durable controller
    /// custody claims this input. Kept only for legacy/orphan rows; ordinary
    /// placed AwaitTurnCompletion records machine custody before sending and
    /// therefore uses `AcknowledgePlacedCompletion` instead.
    AcknowledgeUnclaimed,
    /// A machine-owned ordinary placed completion was durably resolved.
    /// The pump must resolve its exact volatile waiter (if still present)
    /// before queuing the host ACK; machine custody survives either way.
    AcknowledgePlacedCompletion,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait RemoteTurnObligationResolver: Send + Sync {
    async fn resolve(
        &self,
        expected_member: &BridgeMemberIncarnation,
        record: &BridgeTurnOutcomeRecord,
    ) -> Result<OutcomeRecordDisposition, MobError>;

    async fn acknowledge(
        &self,
        expected_member: &BridgeMemberIncarnation,
        ack: &BridgeTurnOutcomeAck,
    ) -> Result<(), MobError>;
}

/// Production resolver over a cloned `MobHandle` (detached submit — the
/// registry's consume path is synchronous pump code).
pub(crate) struct MobHandleObligationResolver {
    pub(crate) handle: super::handle::MobHandle,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl RemoteTurnObligationResolver for MobHandleObligationResolver {
    async fn resolve(
        &self,
        expected_member: &BridgeMemberIncarnation,
        record: &BridgeTurnOutcomeRecord,
    ) -> Result<OutcomeRecordDisposition, MobError> {
        let (phase, obligation) = self.lookup(expected_member, record);
        match remote_outcome_resolve_plan(phase, obligation)? {
            RemoteOutcomeResolvePlan::ResolvePlacedKickoff(obligation) => {
                self.handle
                    .resolve_placed_kickoff_outcome(
                        placed_kickoff_obligation_event(&obligation)?,
                        record.clone(),
                    )
                    .await?;
                Ok(OutcomeRecordDisposition::Acknowledge)
            }
            RemoteOutcomeResolvePlan::ResolvePlacedCompletion(obligation) => {
                self.handle
                    .resolve_placed_completion_outcome(
                        super::placed_completion_reconciler::obligation_event(&obligation)?,
                        record.clone(),
                    )
                    .await?;
                Ok(OutcomeRecordDisposition::AcknowledgePlacedCompletion)
            }
            RemoteOutcomeResolvePlan::Retain => Ok(OutcomeRecordDisposition::Retain),
            RemoteOutcomeResolvePlan::AcknowledgeUnclaimed => {
                Ok(OutcomeRecordDisposition::AcknowledgeUnclaimed)
            }
            RemoteOutcomeResolvePlan::Acknowledge => Ok(OutcomeRecordDisposition::Acknowledge),
            RemoteOutcomeResolvePlan::ResolveFlow(obligation) => {
                self.handle
                    .resolve_remote_turn_outcome(obligation, record.clone())
                    .await?;
                Ok(OutcomeRecordDisposition::Acknowledge)
            }
        }
    }

    async fn acknowledge(
        &self,
        expected_member: &BridgeMemberIncarnation,
        ack: &BridgeTurnOutcomeAck,
    ) -> Result<(), MobError> {
        let (phase, obligation) = self.lookup_key(expected_member, &ack.input_id);
        match (phase, obligation) {
            (RemoteOutcomeCustody::Resolved, Some(RemoteOutcomeObligation::Flow(obligation))) => {
                self.handle
                    .acknowledge_remote_turn_outcome(obligation, ack.clone())
                    .await?;
                Ok(())
            }
            (
                RemoteOutcomeCustody::Resolved,
                Some(RemoteOutcomeObligation::PlacedKickoff(obligation)),
            ) => {
                self.handle
                    .acknowledge_placed_kickoff_outcome(
                        placed_kickoff_obligation_event(&obligation)?,
                        ack.clone(),
                    )
                    .await?;
                Ok(())
            }
            (
                RemoteOutcomeCustody::Resolved,
                Some(RemoteOutcomeObligation::PlacedCompletion(obligation)),
            ) => {
                self.handle
                    .acknowledge_placed_completion_outcome(
                        super::placed_completion_reconciler::obligation_event(&obligation)?,
                        ack.clone(),
                    )
                    .await?;
                Ok(())
            }
            (RemoteOutcomeCustody::Resolved, None) => Err(MobError::Internal(
                "resolved remote outcome custody lookup returned no obligation".to_string(),
            )),
            (RemoteOutcomeCustody::Pending | RemoteOutcomeCustody::Committed, _) => {
                Err(MobError::Internal(format!(
                    "host ACK confirmation arrived before exact remote outcome resolve for '{}'",
                    ack.input_id
                )))
            }
            (RemoteOutcomeCustody::Absent, _) => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteOutcomeCustody {
    Pending,
    Committed,
    Resolved,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteOutcomeObligation {
    Flow(mob_dsl::RemoteTurnObligation),
    PlacedKickoff(mob_dsl::PlacedKickoffObligation),
    PlacedCompletion(mob_dsl::PlacedCompletionObligation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteOutcomeResolvePlan {
    Retain,
    AcknowledgeUnclaimed,
    Acknowledge,
    ResolveFlow(mob_dsl::RemoteTurnObligation),
    ResolvePlacedKickoff(mob_dsl::PlacedKickoffObligation),
    ResolvePlacedCompletion(mob_dsl::PlacedCompletionObligation),
}

fn remote_outcome_resolve_plan(
    phase: RemoteOutcomeCustody,
    obligation: Option<RemoteOutcomeObligation>,
) -> Result<RemoteOutcomeResolvePlan, MobError> {
    match (phase, obligation) {
        (
            RemoteOutcomeCustody::Pending,
            Some(RemoteOutcomeObligation::PlacedKickoff(obligation)),
        ) => Ok(RemoteOutcomeResolvePlan::ResolvePlacedKickoff(obligation)),
        (
            RemoteOutcomeCustody::Pending,
            Some(RemoteOutcomeObligation::PlacedCompletion(obligation)),
        ) => Ok(RemoteOutcomeResolvePlan::ResolvePlacedCompletion(
            obligation,
        )),
        (RemoteOutcomeCustody::Pending, _) => Ok(RemoteOutcomeResolvePlan::Retain),
        (RemoteOutcomeCustody::Absent, None) => Ok(RemoteOutcomeResolvePlan::AcknowledgeUnclaimed),
        (RemoteOutcomeCustody::Absent, Some(_)) => Err(MobError::Internal(
            "absent remote outcome custody lookup returned an obligation".to_string(),
        )),
        (
            RemoteOutcomeCustody::Resolved,
            Some(RemoteOutcomeObligation::PlacedCompletion(obligation)),
        ) => Ok(RemoteOutcomeResolvePlan::ResolvePlacedCompletion(
            obligation,
        )),
        (RemoteOutcomeCustody::Resolved, Some(_)) => Ok(RemoteOutcomeResolvePlan::Acknowledge),
        (RemoteOutcomeCustody::Committed, Some(RemoteOutcomeObligation::Flow(obligation))) => {
            Ok(RemoteOutcomeResolvePlan::ResolveFlow(obligation))
        }
        (RemoteOutcomeCustody::Committed, Some(RemoteOutcomeObligation::PlacedKickoff(_))) => Err(
            MobError::Internal("placed kickoff entered unsupported committed custody".to_string()),
        ),
        (RemoteOutcomeCustody::Committed, Some(RemoteOutcomeObligation::PlacedCompletion(_))) => {
            Err(MobError::Internal(
                "placed completion entered unsupported committed custody".to_string(),
            ))
        }
        (RemoteOutcomeCustody::Committed, None) => Err(MobError::Internal(
            "committed remote-turn custody lookup returned no obligation".to_string(),
        )),
        (RemoteOutcomeCustody::Resolved, None) => Ok(RemoteOutcomeResolvePlan::Retain),
    }
}

impl MobHandleObligationResolver {
    fn lookup(
        &self,
        expected_member: &BridgeMemberIncarnation,
        record: &BridgeTurnOutcomeRecord,
    ) -> (RemoteOutcomeCustody, Option<RemoteOutcomeObligation>) {
        if record.generation != expected_member.generation
            || record.fence_token != expected_member.fence_token
        {
            return (RemoteOutcomeCustody::Absent, None);
        }
        self.lookup_key(expected_member, &record.input_id)
    }

    fn lookup_key(
        &self,
        expected_member: &BridgeMemberIncarnation,
        input_id: &str,
    ) -> (RemoteOutcomeCustody, Option<RemoteOutcomeObligation>) {
        let state = self.handle.machine_state_watch_rx.borrow();
        let matches = |obligation: &&mob_dsl::RemoteTurnObligation| {
            obligation.agent_identity.0 == expected_member.agent_identity
                && obligation.host_id.0 == expected_member.host_id
                && obligation.host_binding_generation == expected_member.binding_generation
                && obligation.member_session_id.0 == expected_member.member_session_id
                && obligation.input_id.0 == input_id
                && obligation.generation.0 == expected_member.generation
                && obligation.fence_token.0 == expected_member.fence_token
        };
        for (phase, set) in [
            (
                RemoteOutcomeCustody::Pending,
                &state.pending_remote_turn_outcomes,
            ),
            (
                RemoteOutcomeCustody::Committed,
                &state.committed_remote_turn_outcomes,
            ),
            (
                RemoteOutcomeCustody::Resolved,
                &state.resolved_remote_turn_outcomes,
            ),
        ] {
            if let Some(obligation) = set.iter().find(matches) {
                return (
                    phase,
                    Some(RemoteOutcomeObligation::Flow(obligation.clone())),
                );
            }
        }
        let matches_kickoff = |obligation: &&mob_dsl::PlacedKickoffObligation| {
            obligation.agent_identity.0 == expected_member.agent_identity
                && obligation.host_id.0 == expected_member.host_id
                && obligation.host_binding_generation == expected_member.binding_generation
                && obligation.member_session_id.0 == expected_member.member_session_id
                && obligation.input_id.0 == input_id
                && obligation.generation.0 == expected_member.generation
                && obligation.fence_token.0 == expected_member.fence_token
        };
        for (phase, set) in [
            (
                RemoteOutcomeCustody::Pending,
                &state.pending_placed_kickoff_outcomes,
            ),
            (
                RemoteOutcomeCustody::Resolved,
                &state.resolved_placed_kickoff_outcomes,
            ),
        ] {
            if let Some(obligation) = set.iter().find(matches_kickoff) {
                return (
                    phase,
                    Some(RemoteOutcomeObligation::PlacedKickoff(obligation.clone())),
                );
            }
        }
        let matches_completion = |obligation: &&mob_dsl::PlacedCompletionObligation| {
            obligation.agent_identity.0 == expected_member.agent_identity
                && obligation.host_id.0 == expected_member.host_id
                && obligation.host_binding_generation == expected_member.binding_generation
                && obligation.member_session_id.0 == expected_member.member_session_id
                && obligation.input_id.0 == input_id
                && obligation.generation.0 == expected_member.generation
                && obligation.fence_token.0 == expected_member.fence_token
        };
        for (phase, set) in [
            (
                RemoteOutcomeCustody::Pending,
                &state.pending_placed_completion_outcomes,
            ),
            (
                RemoteOutcomeCustody::Resolved,
                &state.resolved_placed_completion_outcomes,
            ),
        ] {
            if let Some(obligation) = set.iter().find(matches_completion) {
                return (
                    phase,
                    Some(RemoteOutcomeObligation::PlacedCompletion(
                        obligation.clone(),
                    )),
                );
            }
        }
        (RemoteOutcomeCustody::Absent, None)
    }
}

pub(crate) fn obligation_event(
    obligation: &mob_dsl::RemoteTurnObligation,
) -> Result<RemoteTurnObligationEvent, MobError> {
    let run_id = obligation.run_id.0.parse::<RunId>().map_err(|error| {
        MobError::Internal(format!(
            "remote-turn obligation has invalid run id '{}': {error}",
            obligation.run_id.0
        ))
    })?;
    Ok(RemoteTurnObligationEvent {
        agent_identity: AgentIdentity::from(obligation.agent_identity.0.as_str()),
        host_id: obligation.host_id.0.clone(),
        host_binding_generation: obligation.host_binding_generation,
        member_session_id: obligation.member_session_id.0.clone(),
        generation: crate::ids::Generation::new(obligation.generation.0),
        fence_token: crate::ids::FenceToken::new(obligation.fence_token.0),
        dispatch_sequence: obligation.dispatch_sequence,
        input_id: obligation.input_id.0.clone(),
        run_id,
        step_id: StepId::from(obligation.step_id.0.as_str()),
    })
}

pub(crate) fn obligation_from_event(
    obligation: &RemoteTurnObligationEvent,
) -> mob_dsl::RemoteTurnObligation {
    mob_dsl::RemoteTurnObligation {
        agent_identity: mob_dsl::AgentIdentity(obligation.agent_identity.to_string()),
        host_id: mob_dsl::HostId(obligation.host_id.clone()),
        host_binding_generation: obligation.host_binding_generation,
        member_session_id: mob_dsl::SessionId(obligation.member_session_id.clone()),
        generation: mob_dsl::Generation(obligation.generation.get()),
        fence_token: mob_dsl::FenceToken(obligation.fence_token.get()),
        dispatch_sequence: obligation.dispatch_sequence,
        input_id: mob_dsl::InputId(obligation.input_id.clone()),
        run_id: mob_dsl::RunId::from(obligation.run_id.to_string()),
        step_id: mob_dsl::StepId::from(obligation.step_id.to_string()),
    }
}

pub(crate) fn placed_kickoff_obligation_event(
    obligation: &mob_dsl::PlacedKickoffObligation,
) -> Result<crate::event::PlacedKickoffObligationEvent, MobError> {
    let input_uuid = uuid::Uuid::parse_str(&obligation.input_id.0).map_err(|error| {
        MobError::Internal(format!(
            "placed-kickoff obligation has invalid input id '{}': {error}",
            obligation.input_id.0
        ))
    })?;
    if input_uuid.to_string() != obligation.input_id.0 {
        return Err(MobError::Internal(format!(
            "placed-kickoff obligation input id '{}' is not canonical",
            obligation.input_id.0
        )));
    }
    let objective_id = uuid::Uuid::parse_str(&obligation.objective_id).map_err(|error| {
        MobError::Internal(format!(
            "placed-kickoff obligation has invalid objective id '{}': {error}",
            obligation.objective_id
        ))
    })?;
    if objective_id.to_string() != obligation.objective_id {
        return Err(MobError::Internal(format!(
            "placed-kickoff obligation objective id '{}' is not canonical",
            obligation.objective_id
        )));
    }
    Ok(crate::event::PlacedKickoffObligationEvent {
        agent_identity: AgentIdentity::from(obligation.agent_identity.0.as_str()),
        host_id: obligation.host_id.0.clone(),
        host_binding_generation: obligation.host_binding_generation,
        member_session_id: obligation.member_session_id.0.clone(),
        generation: crate::ids::Generation::new(obligation.generation.0),
        fence_token: crate::ids::FenceToken::new(obligation.fence_token.0),
        input_id: obligation.input_id.0.clone(),
        objective_id: meerkat_core::interaction::ObjectiveId(objective_id),
    })
}

pub(crate) fn placed_kickoff_obligation_from_event(
    obligation: &crate::event::PlacedKickoffObligationEvent,
) -> mob_dsl::PlacedKickoffObligation {
    mob_dsl::PlacedKickoffObligation {
        agent_identity: mob_dsl::AgentIdentity(obligation.agent_identity.to_string()),
        host_id: mob_dsl::HostId(obligation.host_id.clone()),
        host_binding_generation: obligation.host_binding_generation,
        member_session_id: mob_dsl::SessionId(obligation.member_session_id.clone()),
        generation: mob_dsl::Generation(obligation.generation.get()),
        fence_token: mob_dsl::FenceToken(obligation.fence_token.get()),
        input_id: mob_dsl::InputId(obligation.input_id.clone()),
        objective_id: obligation.objective_id.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

struct ArmedTicket {
    run_id: RunId,
    step_id: StepId,
    /// Exact authority incarnation under which the directive was dispatched.
    /// A fence rotation can supersede a ticket without changing runtime id or
    /// generation.
    baseline_incarnation: RemoteResidencyKey,
    /// Recovery adopters may commit only an exact host terminal or a typed
    /// certified-no-effect rejection. Shell-synthesized rematerialization /
    /// removal failures close their waiter without manufacturing a receipt.
    recovered_adopter: bool,
    tx: oneshot::Sender<FlowTurnOutcome>,
}

#[derive(Debug, Clone)]
struct FoldTerminal {
    classified: ClassifiedTurnTerminal,
    /// Interaction terminals carry their canonical correlation in the event
    /// row itself. Preserve it so a same-kind sidecar for another input can
    /// never resolve the wrong waiter or obligation.
    interaction_id: Option<meerkat_core::interaction::InteractionId>,
    /// The classifier intentionally gives both `ExtractionFailed` and an
    /// interaction-scoped `InteractionFailed { ExtractionFailed }` the same
    /// typed kind. Preserve the original event family so only the latter may
    /// cross the tracked-Peer interaction wire projection.
    interaction_scoped_extraction_failure: bool,
}

#[derive(Default)]
struct MemberLane {
    classifier: TurnTerminalClassifier,
    /// Full host/member residency of the rows currently being folded.
    fold_incarnation: Option<RemoteResidencyKey>,
    /// Highest durable seq folded so far (within `fold_incarnation`).
    fold_seq: Option<u64>,
    /// Recent rebuildable fold terminals keyed by exact incarnation + seq.
    /// Sidecar-announced seqs are pinned until exact ACK confirmation, so an
    /// unrelated terminal burst can never turn eviction into false authority.
    recent_terminals: VecDeque<(RemoteResidencyKey, u64, FoldTerminal)>,
    pinned_terminal_seqs: BTreeSet<(RemoteResidencyKey, u64)>,
    /// Durable rows the owning host proved could not cross the bridge byte
    /// budget. A matching sidecar still carries terminal kind and bounded
    /// failure detail, so failed turns can resolve without waiting for a
    /// payload row that will never arrive.
    omitted_event_seqs: BTreeSet<(RemoteResidencyKey, u64)>,
    /// Records whose exact terminal was safely classified/omitted. This is a
    /// cache only; machine custody decides ACK eligibility.
    fold_ready_records: BTreeSet<(RemoteResidencyKey, u64, String)>,
    armed: BTreeMap<String, ArmedTicket>,
    consumed: BTreeSet<(RemoteResidencyKey, String)>,
    consumed_order: VecDeque<(RemoteResidencyKey, String)>,
    /// Controlling-side respawn mark (T-F6), a generation floor: set to the
    /// RETIRING incarnation's generation by
    /// [`RemoteFlowTicketRegistry::note_member_rematerializing`] strictly
    /// before the respawn's remote release tears the member transport;
    /// cleared by the first generation observation ABOVE the floor (the
    /// replacement's pages — residual old-incarnation pages never clear
    /// it). While set, a dispatch-failure unwind attributes its fault to
    /// the rematerialization instead of the raw comms error.
    rematerializing_floor: Option<u64>,
}

/// See the module docs. API names are the adjudicated cross-lane seam
/// (FLAG-P6F-5): `arm`, `disarm`, `observe_member_event_row`,
/// `consume_turn_outcome_record`, `observe_member_generation`.
pub struct RemoteFlowTicketRegistry {
    resolver: Arc<dyn RemoteTurnObligationResolver>,
    members: Mutex<BTreeMap<AgentIdentity, MemberLane>>,
}

impl RemoteFlowTicketRegistry {
    pub(crate) fn new(resolver: Arc<dyn RemoteTurnObligationResolver>) -> Self {
        Self {
            resolver,
            members: Mutex::new(BTreeMap::new()),
        }
    }

    fn lane<'a>(
        members: &'a mut BTreeMap<AgentIdentity, MemberLane>,
        target: &AgentIdentity,
    ) -> &'a mut MemberLane {
        members.entry(target.clone()).or_default()
    }

    /// Executor-side: arm a ticket for one dispatched directive; returns the
    /// completion receiver the `RemoteTurnTicket` awaits.
    pub fn arm_exact(
        &self,
        target: &AgentIdentity,
        input_id: &str,
        run_id: &RunId,
        step_id: &StepId,
        expected_member: &BridgeMemberIncarnation,
    ) -> oneshot::Receiver<FlowTurnOutcome> {
        let (tx, rx) = oneshot::channel();
        let mut members = self.members_guard();
        let lane = Self::lane(&mut members, target);
        if let Some(reason) = Self::arm_rejection_reason(target, lane, expected_member) {
            let _ = tx.send(FlowTurnOutcome::Failed {
                reason,
                disposition: FlowTurnFailureDisposition::ObservedTerminal,
            });
            return rx;
        }
        lane.armed.insert(
            input_id.to_string(),
            ArmedTicket {
                run_id: run_id.clone(),
                step_id: step_id.clone(),
                baseline_incarnation: expected_member.into(),
                recovered_adopter: false,
                tx,
            },
        );
        rx
    }

    /// Recovery-side idempotent arm. A live FlowEngine ticket always wins;
    /// the reconciler must never replace its sender. Reusing an input id with
    /// different run/step/incarnation material is a hard correlation error.
    pub(crate) fn arm_recovered_exact(
        &self,
        target: &AgentIdentity,
        input_id: &str,
        run_id: &RunId,
        step_id: &StepId,
        expected_member: &BridgeMemberIncarnation,
    ) -> Result<Option<oneshot::Receiver<FlowTurnOutcome>>, MobError> {
        let mut members = self.members_guard();
        let lane = Self::lane(&mut members, target);
        if Self::arm_rejection_reason(target, lane, expected_member).is_some() {
            // A recovery adopter may commit only an exact host terminal or a
            // certified-no-effect rejection. Close the channel instead of
            // manufacturing an ObservedTerminal receipt for a stale shell
            // arm; the reconciler treats RecvError as "rescan, no receipt".
            let (_tx, rx) = oneshot::channel();
            return Ok(Some(rx));
        }
        if let Some(existing) = lane.armed.get(input_id) {
            if existing.run_id != *run_id
                || existing.step_id != *step_id
                || existing.baseline_incarnation != RemoteResidencyKey::from(expected_member)
            {
                return Err(MobError::Internal(format!(
                    "remote-turn input id '{input_id}' is already armed for a different exact obligation"
                )));
            }
            return Ok(None);
        }
        let (tx, rx) = oneshot::channel();
        lane.armed.insert(
            input_id.to_string(),
            ArmedTicket {
                run_id: run_id.clone(),
                step_id: step_id.clone(),
                baseline_incarnation: expected_member.into(),
                recovered_adopter: true,
                tx,
            },
        );
        Ok(Some(rx))
    }

    /// Validate arming against the currently installed pump residency while
    /// holding the same member-lane mutex used for insertion. `None` remains
    /// valid during bootstrap, before the first pump install. Once a
    /// residency is installed, a delayed reserve reply can never arm an old
    /// tuple after promotion has already swept it.
    fn arm_rejection_reason(
        target: &AgentIdentity,
        lane: &MemberLane,
        expected_member: &BridgeMemberIncarnation,
    ) -> Option<String> {
        if target.as_str() != expected_member.agent_identity {
            return Some(format!(
                "remote flow ticket target '{}' does not match expected residency identity '{}'",
                target, expected_member.agent_identity
            ));
        }
        let expected = RemoteResidencyKey::from(expected_member);
        if lane
            .fold_incarnation
            .as_ref()
            .is_some_and(|installed| installed != &expected)
        {
            return Some(REMATERIALIZED_STEP_FAILURE_REASON.to_string());
        }
        None
    }

    #[cfg(test)]
    fn test_member(
        target: &AgentIdentity,
        generation: u64,
        fence_token: u64,
    ) -> BridgeMemberIncarnation {
        BridgeMemberIncarnation {
            mob_id: "test-mob".to_string(),
            agent_identity: target.to_string(),
            host_id: "host-a".to_string(),
            binding_generation: 1,
            member_session_id: "session-a".to_string(),
            generation,
            fence_token,
        }
    }

    #[cfg(test)]
    fn arm(
        &self,
        target: &AgentIdentity,
        input_id: &str,
        run_id: &RunId,
        step_id: &StepId,
    ) -> oneshot::Receiver<FlowTurnOutcome> {
        let expected_member = Self::test_member(target, 1, 7);
        self.install_member_residency(&expected_member);
        self.arm_exact(target, input_id, run_id, step_id, &expected_member)
    }

    /// Executor-side (timeout / dispatch-failure unwind): drop the armed
    /// ticket. Idempotent; a later record for the key is a display-only
    /// no-op consume.
    pub fn disarm(&self, target: &AgentIdentity, input_id: &str) {
        let mut members = self.members_guard();
        let lane = Self::lane(&mut members, target);
        lane.armed.remove(input_id);
    }

    /// Pump-side: one durable event row, in durable-seq order, rows BEFORE
    /// the page's sidecar records.
    pub fn install_member_residency(&self, expected_member: &BridgeMemberIncarnation) {
        let target = AgentIdentity::from(expected_member.agent_identity.as_str());
        let mut members = self.members_guard();
        let lane = Self::lane(&mut members, &target);
        Self::install_residency(&target, lane, expected_member);
    }

    pub fn observe_member_event_row_exact(
        &self,
        expected_member: &BridgeMemberIncarnation,
        seq: u64,
        event: &AgentEvent,
    ) {
        let target = AgentIdentity::from(expected_member.agent_identity.as_str());
        let mut members = self.members_guard();
        let lane = Self::lane(&mut members, &target);
        if !Self::accepts_residency(&target, lane, expected_member) {
            return;
        }

        lane.fold_seq = Some(seq);
        if let Some(classified) = lane.classifier.observe(event) {
            let interaction_id = match event {
                AgentEvent::InteractionComplete { interaction_id, .. }
                | AgentEvent::InteractionCallbackPending { interaction_id, .. }
                | AgentEvent::InteractionFailed { interaction_id, .. } => Some(*interaction_id),
                _ => None,
            };
            let interaction_scoped_extraction_failure = matches!(
                event,
                AgentEvent::InteractionFailed {
                    reason: meerkat_core::event::InteractionFailureReason::ExtractionFailed { .. },
                    ..
                }
            );
            lane.recent_terminals.push_back((
                expected_member.into(),
                seq,
                FoldTerminal {
                    classified,
                    interaction_id,
                    interaction_scoped_extraction_failure,
                },
            ));
            Self::trim_rebuildable_terminals(lane);
        }
    }

    /// Pump-side: advance the fold past one typed oversized-row omission.
    /// The sidecar is consumed on the next successful page; retaining the
    /// exact seq lets failed terminals resolve from its bounded detail.
    pub fn observe_omitted_member_event_row_exact(
        &self,
        expected_member: &BridgeMemberIncarnation,
        seq: u64,
    ) {
        let target = AgentIdentity::from(expected_member.agent_identity.as_str());
        let mut members = self.members_guard();
        let lane = Self::lane(&mut members, &target);
        if !Self::accepts_residency(&target, lane, expected_member) {
            return;
        }
        lane.fold_seq = Some(lane.fold_seq.map_or(seq, |folded| folded.max(seq)));
        lane.omitted_event_seqs
            .insert((expected_member.into(), seq));
    }

    /// Pre-observe sidecar correlation before folding the page's rows. This
    /// pins exact terminal seqs without consuming or acknowledging anything.
    pub fn observe_turn_outcome_page(
        &self,
        expected_member: &BridgeMemberIncarnation,
        event_frontier_exclusive: u64,
        outcomes_complete: bool,
        records: &[BridgeTurnOutcomeRecord],
    ) {
        let target = AgentIdentity::from(expected_member.agent_identity.as_str());
        let mut members = self.members_guard();
        let lane = Self::lane(&mut members, &target);
        if !Self::accepts_residency(&target, lane, expected_member) {
            return;
        }
        let residency = RemoteResidencyKey::from(expected_member);
        let page_terminal_seqs = records
            .iter()
            .filter(|record| {
                record.generation == expected_member.generation
                    && record.fence_token == expected_member.fence_token
            })
            .map(|record| record.terminal_seq)
            .collect::<BTreeSet<_>>();
        if outcomes_complete {
            // Oversized non-terminal rows have no journal sidecar and must
            // not accumulate forever under a long-lived observation tap. A
            // complete sidecar page proves which omitted rows below this
            // event frontier are currently terminal. Delayed journal commit
            // remains safe: machine custody freezes the durable cursor, so a
            // later sidecar miss requests replay and re-observes the typed
            // omission before ACK.
            lane.omitted_event_seqs.retain(|(row_residency, seq)| {
                row_residency != &residency
                    || *seq >= event_frontier_exclusive
                    || page_terminal_seqs.contains(seq)
            });
        }
        for record in records {
            if record.generation == expected_member.generation
                && record.fence_token == expected_member.fence_token
                && !matches!(record.outcome, WireFlowTurnOutcome::ChannelClosed)
            {
                lane.pinned_terminal_seqs
                    .insert((residency.clone(), record.terminal_seq));
            }
        }
    }

    /// Pump-side: consume one journal sidecar only after its exact terminal
    /// is safely folded/omitted. Machine custody, not this cache, decides ACK.
    pub async fn consume_turn_outcome_record(
        &self,
        expected_member: &BridgeMemberIncarnation,
        record: &BridgeTurnOutcomeRecord,
    ) -> Result<OutcomeRecordDisposition, MobError> {
        let target = AgentIdentity::from(expected_member.agent_identity.as_str());
        let fold = {
            let mut members = self.members_guard();
            let lane = Self::lane(&mut members, &target);
            if !Self::accepts_residency(&target, lane, expected_member) {
                FoldRecordDisposition::Retain
            } else {
                Self::consume_record(&target, lane, expected_member, record)
            }
        };
        match fold {
            FoldRecordDisposition::Retain => return Ok(OutcomeRecordDisposition::Retain),
            FoldRecordDisposition::Replay => return Ok(OutcomeRecordDisposition::Replay),
            FoldRecordDisposition::Ready => {}
        }
        self.resolver.resolve(expected_member, record).await
    }

    /// A successful poll response proves the host applied the exact ACK.
    pub async fn acknowledge_turn_outcome(
        &self,
        expected_member: &BridgeMemberIncarnation,
        ack: &BridgeTurnOutcomeAck,
    ) -> Result<(), MobError> {
        if ack.generation != expected_member.generation
            || ack.fence_token != expected_member.fence_token
        {
            return Ok(());
        }
        let target = AgentIdentity::from(expected_member.agent_identity.as_str());
        {
            let members = self.members_guard();
            let Some(lane) = members.get(&target) else {
                return Ok(());
            };
            if !Self::accepts_residency(&target, lane, expected_member) {
                return Ok(());
            }
        }
        self.resolver.acknowledge(expected_member, ack).await?;
        let mut members = self.members_guard();
        let lane = Self::lane(&mut members, &target);
        if !Self::accepts_residency(&target, lane, expected_member) {
            return Ok(());
        }
        let residency = RemoteResidencyKey::from(expected_member);
        let dedup_key = (residency.clone(), ack.input_id.clone());
        Self::mark_consumed(lane, dedup_key);
        let terminal_seqs = lane
            .fold_ready_records
            .iter()
            .filter(|(record_residency, _, input_id)| {
                record_residency == &residency && input_id == &ack.input_id
            })
            .map(|(record_residency, seq, _)| (record_residency.clone(), *seq))
            .collect::<Vec<_>>();
        lane.fold_ready_records
            .retain(|(record_residency, _, input_id)| {
                record_residency != &residency || input_id != &ack.input_id
            });
        for key in terminal_seqs {
            lane.pinned_terminal_seqs.remove(&key);
            lane.omitted_event_seqs.remove(&key);
        }
        Self::trim_rebuildable_terminals(lane);
        Ok(())
    }

    /// Pump-side: the page's `generation` fact — the fail-fast signal for
    /// §18.8:1004 (soundness itself rests on `input_id` uniqueness plus the
    /// journal's generation scoping; this only shortens the wait).
    pub fn observe_member_generation_exact(&self, expected_member: &BridgeMemberIncarnation) {
        let target = AgentIdentity::from(expected_member.agent_identity.as_str());
        let mut members = self.members_guard();
        let lane = Self::lane(&mut members, &target);
        let _ = Self::accepts_residency(&target, lane, expected_member);
    }

    pub fn rewind_member_fold_exact(&self, expected_member: &BridgeMemberIncarnation) {
        let target = AgentIdentity::from(expected_member.agent_identity.as_str());
        let mut members = self.members_guard();
        let lane = Self::lane(&mut members, &target);
        if !Self::accepts_residency(&target, lane, expected_member) {
            return;
        }
        let residency = RemoteResidencyKey::from(expected_member);
        lane.fold_seq = None;
        lane.classifier = TurnTerminalClassifier::new();
        lane.recent_terminals
            .retain(|(record_residency, _, _)| record_residency != &residency);
    }

    #[cfg(test)]
    fn observe_member_event_row(
        &self,
        target: &AgentIdentity,
        generation: u64,
        seq: u64,
        event: &AgentEvent,
    ) {
        let expected_member = Self::test_member(target, generation, 7);
        self.observe_member_event_row_exact(&expected_member, seq, event);
    }

    #[cfg(test)]
    fn observe_omitted_member_event_row(&self, target: &AgentIdentity, generation: u64, seq: u64) {
        let expected_member = Self::test_member(target, generation, 7);
        self.observe_omitted_member_event_row_exact(&expected_member, seq);
    }

    #[cfg(test)]
    fn observe_member_generation(&self, target: &AgentIdentity, generation: u64) {
        let expected_member = Self::test_member(target, generation, 7);
        self.install_member_residency(&expected_member);
        self.observe_member_generation_exact(&expected_member);
    }

    #[cfg(test)]
    fn rewind_member_fold(&self, target: &AgentIdentity, generation: u64) {
        let expected_member = Self::test_member(target, generation, 7);
        self.rewind_member_fold_exact(&expected_member);
    }

    #[cfg(test)]
    fn observe_turn_outcome_page_for_test(
        &self,
        target: &AgentIdentity,
        records: &[BridgeTurnOutcomeRecord],
    ) {
        let expected_member = Self::test_member(target, 1, 7);
        self.observe_turn_outcome_page(&expected_member, u64::MAX, false, records);
    }

    #[cfg(test)]
    async fn consume_turn_outcome_record_for_test(
        &self,
        target: &AgentIdentity,
        record: &BridgeTurnOutcomeRecord,
    ) -> Result<OutcomeRecordDisposition, MobError> {
        let expected_member = Self::test_member(target, record.generation, record.fence_token);
        self.consume_turn_outcome_record(&expected_member, record)
            .await
    }

    pub fn fail_armed(&self, target: &AgentIdentity, input_id: &str, reason: String) {
        self.fail_armed_with_disposition(
            target,
            input_id,
            reason,
            FlowTurnFailureDisposition::ObservedTerminal,
        );
    }

    pub fn fail_armed_certified_no_effect(
        &self,
        target: &AgentIdentity,
        input_id: &str,
        reason: String,
        cause: meerkat_contracts::wire::supervisor_bridge::BridgeDeliveryRejectionCause,
    ) {
        self.fail_armed_with_disposition(
            target,
            input_id,
            reason,
            FlowTurnFailureDisposition::CertifiedNoEffect { cause },
        );
    }

    fn fail_armed_with_disposition(
        &self,
        target: &AgentIdentity,
        input_id: &str,
        reason: String,
        disposition: FlowTurnFailureDisposition,
    ) {
        let mut members = self.members_guard();
        let lane = Self::lane(&mut members, target);
        if let Some(ticket) = lane.armed.remove(input_id) {
            if ticket.recovered_adopter
                && matches!(&disposition, FlowTurnFailureDisposition::ObservedTerminal)
            {
                // Dropping the sender tells the reconciler to re-scan. Only
                // the sidecar fold or a typed no-effect proof may mint its
                // durable receipt.
                return;
            }
            let _ = ticket.tx.send(FlowTurnOutcome::Failed {
                reason,
                disposition,
            });
        }
    }

    /// Controlling-side respawn mark (T-F6): the placed respawn calls this
    /// STRICTLY BEFORE its remote release tears the member transport. Fails
    /// every armed ticket typed with the SAME reason the pump's generation
    /// watch produces (resolving their obligations), and floors the lane at
    /// the retiring incarnation's generation so a dispatch-failure unwind
    /// caused by the teardown attributes typed via
    /// [`Self::is_member_rematerializing`].
    pub(crate) fn note_member_rematerializing(&self, target: &AgentIdentity, old_generation: u64) {
        let mut members = self.members_guard();
        let lane = Self::lane(&mut members, target);
        lane.rematerializing_floor = Some(old_generation);
        let armed = std::mem::take(&mut lane.armed);
        for (input_id, ticket) in armed {
            tracing::warn!(
                target = %target,
                input_id = %input_id,
                run_id = %ticket.run_id,
                step_id = %ticket.step_id,
                old_generation,
                "placed member rematerializing mid flow step; failing the step typed"
            );
            if !ticket.recovered_adopter {
                let _ = ticket.tx.send(FlowTurnOutcome::Failed {
                    reason: REMATERIALIZED_STEP_FAILURE_REASON.to_string(),
                    disposition: FlowTurnFailureDisposition::ObservedTerminal,
                });
            }
        }
    }

    /// Dispatch-unwind consult (T-F6): is the member marked rematerializing
    /// (respawn mark set, not yet superseded by an above-floor generation
    /// observation)?
    pub(crate) fn is_member_rematerializing(&self, target: &AgentIdentity) -> bool {
        self.members_guard()
            .get(target)
            .is_some_and(|lane| lane.rematerializing_floor.is_some())
    }

    /// Member-removal sweep (retire/release/destroy realization): drop the
    /// member's lane, failing any still-armed tickets typed and resolving
    /// their obligations — a removed member can never serve a terminal.
    /// The respawn path must NOT drop the lane: rematerialization
    /// attribution (the generation watch + the respawn mark) lives on it.
    pub(crate) fn drop_lane(&self, target: &AgentIdentity) {
        let mut members = self.members_guard();
        let Some(lane) = members.remove(target) else {
            return;
        };
        for (input_id, ticket) in lane.armed {
            tracing::warn!(
                target = %target,
                input_id = %input_id,
                run_id = %ticket.run_id,
                step_id = %ticket.step_id,
                "member removed mid flow step; failing the step typed"
            );
            if !ticket.recovered_adopter {
                let _ = ticket.tx.send(FlowTurnOutcome::Failed {
                    reason: MEMBER_RELEASED_STEP_FAILURE_REASON.to_string(),
                    disposition: FlowTurnFailureDisposition::ObservedTerminal,
                });
            }
        }
    }

    fn members_guard(&self) -> std::sync::MutexGuard<'_, BTreeMap<AgentIdentity, MemberLane>> {
        self.members
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn install_residency(
        target: &AgentIdentity,
        lane: &mut MemberLane,
        expected_member: &BridgeMemberIncarnation,
    ) {
        let observed = RemoteResidencyKey::from(expected_member);
        if lane.fold_incarnation.as_ref() == Some(&observed) {
            return;
        }
        // The respawn mark lapses at the first observation above its floor:
        // the anticipated rematerialization has now been observed and the
        // pump's generation watch owns attribution from here.
        if lane
            .rematerializing_floor
            .is_some_and(|floor| expected_member.generation > floor)
        {
            lane.rematerializing_floor = None;
        }
        let stale: Vec<String> = lane
            .armed
            .iter()
            .filter(|(_, ticket)| ticket.baseline_incarnation != observed)
            .map(|(input_id, _)| input_id.clone())
            .collect();
        for input_id in stale {
            if let Some(ticket) = lane.armed.remove(&input_id) {
                tracing::warn!(
                    target = %target,
                    input_id = %input_id,
                    run_id = %ticket.run_id,
                    step_id = %ticket.step_id,
                    expected_member = ?expected_member,
                    "placed member residency changed mid flow step; failing the step typed"
                );
                if !ticket.recovered_adopter {
                    let _ = ticket.tx.send(FlowTurnOutcome::Failed {
                        reason: REMATERIALIZED_STEP_FAILURE_REASON.to_string(),
                        disposition: FlowTurnFailureDisposition::ObservedTerminal,
                    });
                }
            }
        }
        lane.fold_incarnation = Some(observed);
        lane.fold_seq = None;
        lane.classifier = TurnTerminalClassifier::new();
        lane.recent_terminals.clear();
        lane.pinned_terminal_seqs.clear();
        lane.omitted_event_seqs.clear();
        lane.fold_ready_records.clear();
        lane.consumed.clear();
        lane.consumed_order.clear();
    }

    fn accepts_residency(
        target: &AgentIdentity,
        lane: &MemberLane,
        expected_member: &BridgeMemberIncarnation,
    ) -> bool {
        let observed = RemoteResidencyKey::from(expected_member);
        if lane.fold_incarnation.as_ref() == Some(&observed) {
            return true;
        }
        tracing::debug!(
            target = %target,
            expected_member = ?expected_member,
            current_residency = ?lane.fold_incarnation,
            "dropping remote outcome observation outside the installed residency"
        );
        false
    }

    fn consume_record(
        target: &AgentIdentity,
        lane: &mut MemberLane,
        expected_member: &BridgeMemberIncarnation,
        record: &BridgeTurnOutcomeRecord,
    ) -> FoldRecordDisposition {
        let record_incarnation = RemoteResidencyKey::from(expected_member);
        if record.generation != expected_member.generation
            || record.fence_token != expected_member.fence_token
        {
            return FoldRecordDisposition::Retain;
        }
        if let Some(ticket) = lane.armed.get(&record.input_id)
            && ticket.baseline_incarnation != record_incarnation
        {
            tracing::warn!(
                target = %target,
                input_id = %record.input_id,
                record_incarnation = ?record_incarnation,
                dispatched_incarnation = ?ticket.baseline_incarnation,
                "retaining remote outcome whose authority tuple does not match the armed ticket"
            );
            return FoldRecordDisposition::Retain;
        }
        match lane.fold_incarnation.as_ref() {
            Some(current) if current != &record_incarnation => {
                tracing::warn!(
                    target = %target,
                    input_id = %record.input_id,
                    record_incarnation = ?record_incarnation,
                    fold_incarnation = ?current,
                    "retaining remote outcome outside the exact event-fold incarnation"
                );
                return FoldRecordDisposition::Retain;
            }
            None => return FoldRecordDisposition::Retain,
            Some(_) => {}
        }
        let dedup_key = (record_incarnation.clone(), record.input_id.clone());
        if lane.consumed.contains(&dedup_key) {
            tracing::debug!(
                target = %target,
                input_id = %record.input_id,
                "host replayed an already ACK-confirmed turn outcome"
            );
            return FoldRecordDisposition::Ready;
        }

        let ready_key = (
            record_incarnation.clone(),
            record.terminal_seq,
            record.input_id.clone(),
        );
        if lane.fold_ready_records.contains(&ready_key) {
            if !lane.armed.contains_key(&record.input_id) {
                return FoldRecordDisposition::Ready;
            }
            // Recovery may arm after the pump already folded this exact
            // terminal but before machine custody could advance. Rehydrate
            // that late adopter from the still-pinned fold row (or from the
            // replayed oversized-row sidecar) instead of returning Ready
            // without ever delivering its outcome.
            let replayed_outcome = if lane
                .omitted_event_seqs
                .contains(&(record_incarnation.clone(), record.terminal_seq))
            {
                Some(outcome_without_event_row(record))
            } else {
                lane.recent_terminals
                    .iter()
                    .find(|(residency, seq, terminal)| {
                        residency == &record_incarnation
                            && *seq == record.terminal_seq
                            && terminal_agrees_with_wire(terminal, &record.outcome)
                            && terminal_interaction_id_agrees_with_record(terminal, record)
                    })
                    .map(|(_, _, terminal)| {
                        FlowTurnOutcome::from(terminal.classified.outcome.clone())
                    })
            };
            if let Some(outcome) = replayed_outcome {
                if let Some(ticket) = lane.armed.remove(&record.input_id) {
                    let _ = ticket.tx.send(outcome);
                }
                return FoldRecordDisposition::Ready;
            }
            // A pinned Ready marker without its validating fold material is
            // not sufficient to manufacture a terminal. Fall through to the
            // ordinary replay/mismatch classification below.
        }

        // Legacy ChannelClosed sidecars carry only mechanical waiter loss and
        // no durable interaction terminal. Never ACK or resolve them: retain
        // Pending until recovery observes the exact event-store terminal.
        if matches!(record.outcome, WireFlowTurnOutcome::ChannelClosed) {
            return FoldRecordDisposition::Retain;
        }

        if lane
            .omitted_event_seqs
            .contains(&(record_incarnation.clone(), record.terminal_seq))
        {
            if let Some(ticket) = lane.armed.remove(&record.input_id) {
                let _ = ticket.tx.send(outcome_without_event_row(record));
            }
            lane.fold_ready_records.insert(ready_key);
            return FoldRecordDisposition::Ready;
        }

        // Attribution: the fold terminal at record.terminal_seq is the
        // payload for this ticket.
        let fold_terminal = lane
            .recent_terminals
            .iter()
            .find(|(residency, seq, _)| {
                residency == &record_incarnation && *seq == record.terminal_seq
            })
            .map(|(_, _, terminal)| terminal.clone());

        match fold_terminal {
            Some(terminal) => {
                if !terminal_agrees_with_wire(&terminal, &record.outcome)
                    || !terminal_interaction_id_agrees_with_record(&terminal, record)
                {
                    // Two owners disagreeing is a typed failure naming both
                    // kinds. It is NOT ACK-safe: retain the host row for
                    // replay/diagnosis instead of laundering disagreement.
                    tracing::error!(
                        target = %target,
                        input_id = %record.input_id,
                        fold_kind = %terminal.classified.kind,
                        record_kind = ?record.outcome,
                        "turn terminal mismatch between event fold and journal record"
                    );
                    if let Some(ticket) = lane.armed.remove(&record.input_id) {
                        let _ = ticket.tx.send(FlowTurnOutcome::Failed {
                            reason: format!(
                                "turn terminal mismatch: event fold classified '{}' but the \
                                 journal record carries '{:?}'",
                                terminal.classified.kind, record.outcome
                            ),
                            disposition: FlowTurnFailureDisposition::ObservedTerminal,
                        });
                    }
                    return FoldRecordDisposition::Retain;
                }
                if let Some(ticket) = lane.armed.remove(&record.input_id) {
                    let _ = ticket
                        .tx
                        .send(FlowTurnOutcome::from(terminal.classified.outcome));
                }
                lane.fold_ready_records.insert(ready_key);
                FoldRecordDisposition::Ready
            }
            None => {
                let fold_passed = lane
                    .fold_seq
                    .is_some_and(|folded| folded >= record.terminal_seq)
                    && lane.fold_incarnation.as_ref() == Some(&record_incarnation);
                if fold_passed {
                    // A sidecar can lag beyond the rebuildable cache. Replay
                    // from the frozen durable cursor with this seq pinned;
                    // never turn eviction into a fabricated failure/ACK.
                    FoldRecordDisposition::Replay
                } else {
                    FoldRecordDisposition::Retain
                }
            }
        }
    }

    fn trim_rebuildable_terminals(lane: &mut MemberLane) {
        while lane.recent_terminals.len() > RECENT_TERMINALS_PER_MEMBER {
            let Some(index) = lane
                .recent_terminals
                .iter()
                .position(|(residency, seq, _)| {
                    !lane
                        .pinned_terminal_seqs
                        .contains(&(residency.clone(), *seq))
                })
            else {
                break;
            };
            lane.recent_terminals.remove(index);
        }
    }

    fn mark_consumed(lane: &mut MemberLane, key: (RemoteResidencyKey, String)) {
        if lane.consumed.insert(key.clone()) {
            lane.consumed_order.push_back(key);
            while lane.consumed_order.len() > CONSUMED_RECORDS_PER_MEMBER {
                if let Some(evicted) = lane.consumed_order.pop_front() {
                    lane.consumed.remove(&evicted);
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FoldRecordDisposition {
    Retain,
    Replay,
    Ready,
}

fn outcome_without_event_row(record: &BridgeTurnOutcomeRecord) -> FlowTurnOutcome {
    let failed_reason =
        |detail: &meerkat_contracts::wire::supervisor_bridge::WireFlowFailureDetail| {
            if detail.truncated {
                format!(
                    "{} [remote failure detail truncated from {} UTF-8 bytes]",
                    detail.text, detail.original_utf8_bytes
                )
            } else {
                detail.text.clone()
            }
        };
    match &record.outcome {
        WireFlowTurnOutcome::ExtractionFailed { detail }
        | WireFlowTurnOutcome::RunFailed { detail }
        | WireFlowTurnOutcome::InteractionFailed { detail } => FlowTurnOutcome::Failed {
            reason: failed_reason(detail),
            disposition: FlowTurnFailureDisposition::ObservedTerminal,
        },
        WireFlowTurnOutcome::ChannelClosed => FlowTurnOutcome::Canceled,
        outcome => FlowTurnOutcome::Failed {
            reason: format!(
                "remote terminal event row at durable seq {} exceeded the bridge budget; \
                 payload for {outcome:?} is unavailable",
                record.terminal_seq
            ),
            disposition: FlowTurnFailureDisposition::ObservedTerminal,
        },
    }
}

/// Agreement between the shared classifier's exact fold terminal and the
/// journal record's wire outcome (code-only mapping beside the registry —
/// the frozen wire enum is untouched).
fn terminal_agrees_with_wire(terminal: &FoldTerminal, wire: &WireFlowTurnOutcome) -> bool {
    let failure_detail = match wire {
        WireFlowTurnOutcome::ExtractionFailed { detail }
        | WireFlowTurnOutcome::RunFailed { detail }
        | WireFlowTurnOutcome::InteractionFailed { detail } => Some(detail),
        _ => None,
    };
    let Some(detail) = failure_detail else {
        // Successful/callback/channel-closed wire variants carry no payload;
        // their strict family discriminant is the entire agreement contract.
        return kinds_agree(terminal.classified.kind, wire);
    };

    // An interaction-scoped extraction failure is durably carried by an
    // `InteractionFailed` event. The shared classifier preserves its typed
    // extraction kind, but tracked plain Peer custody MUST project that exact
    // event family onto InteractionFailed; accepting ordinary
    // `ExtractionFailed` here would erase the interaction correlation.
    let family_agrees = if terminal.interaction_scoped_extraction_failure {
        terminal.classified.kind == TurnTerminalKind::ExtractionFailed
            && matches!(wire, WireFlowTurnOutcome::InteractionFailed { .. })
    } else {
        kinds_agree(terminal.classified.kind, wire)
    };
    if !family_agrees {
        return false;
    }
    let meerkat_core::turn_terminal::TurnTerminalOutcome::Failed { reason } =
        &terminal.classified.outcome
    else {
        return false;
    };
    failure_detail_agrees(reason, detail)
}

fn terminal_interaction_id_agrees_with_record(
    terminal: &FoldTerminal,
    record: &BridgeTurnOutcomeRecord,
) -> bool {
    let interaction_wire = matches!(
        &record.outcome,
        WireFlowTurnOutcome::InteractionComplete
            | WireFlowTurnOutcome::InteractionCallbackPending
            | WireFlowTurnOutcome::InteractionFailed { .. }
    );
    match (terminal.interaction_id, interaction_wire) {
        (Some(interaction_id), true) => interaction_id.0.to_string() == record.input_id,
        (None, false) => true,
        _ => false,
    }
}

fn failure_detail_agrees(
    reason: &str,
    detail: &meerkat_contracts::wire::supervisor_bridge::WireFlowFailureDetail,
) -> bool {
    let original_utf8_bytes = u64::try_from(reason.len()).unwrap_or(u64::MAX);
    detail.original_utf8_bytes == original_utf8_bytes
        && if detail.truncated {
            detail.text.len() < reason.len() && reason.starts_with(&detail.text)
        } else {
            detail.text == *reason
        }
}

/// Strict 1:1 terminal-family agreement. The only admitted cross-family
/// projection lives in [`terminal_agrees_with_wire`] above.
fn kinds_agree(kind: TurnTerminalKind, wire: &WireFlowTurnOutcome) -> bool {
    matches!(
        (kind, wire),
        (
            TurnTerminalKind::RunCompleted,
            WireFlowTurnOutcome::RunCompleted
        ) | (
            TurnTerminalKind::ExtractionSucceeded,
            WireFlowTurnOutcome::ExtractionSucceeded
        ) | (
            TurnTerminalKind::ExtractionFailed,
            WireFlowTurnOutcome::ExtractionFailed { .. }
        ) | (
            TurnTerminalKind::RunFailed,
            WireFlowTurnOutcome::RunFailed { .. }
        ) | (
            TurnTerminalKind::InteractionComplete,
            WireFlowTurnOutcome::InteractionComplete
        ) | (
            TurnTerminalKind::InteractionCallbackPending,
            WireFlowTurnOutcome::InteractionCallbackPending
        ) | (
            TurnTerminalKind::InteractionFailed,
            WireFlowTurnOutcome::InteractionFailed { .. }
        ) | (
            TurnTerminalKind::ChannelClosed,
            WireFlowTurnOutcome::ChannelClosed
        )
    )
}

// ---------------------------------------------------------------------------
// U-5 — registry semantics (pure in-process; no harness)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::types::SessionId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct RecordingResolver {
        resolved: Mutex<Vec<(AgentIdentity, String)>>,
        count: AtomicUsize,
        acknowledged: AtomicUsize,
    }

    #[derive(Default)]
    struct UnclaimedResolver {
        resolved: AtomicUsize,
        acknowledged: AtomicUsize,
    }

    #[async_trait]
    impl RemoteTurnObligationResolver for RecordingResolver {
        async fn resolve(
            &self,
            expected_member: &BridgeMemberIncarnation,
            record: &BridgeTurnOutcomeRecord,
        ) -> Result<OutcomeRecordDisposition, MobError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.resolved
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    AgentIdentity::from(expected_member.agent_identity.as_str()),
                    record.input_id.clone(),
                ));
            Ok(OutcomeRecordDisposition::Acknowledge)
        }

        async fn acknowledge(
            &self,
            _expected_member: &BridgeMemberIncarnation,
            _ack: &BridgeTurnOutcomeAck,
        ) -> Result<(), MobError> {
            self.acknowledged.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[async_trait]
    impl RemoteTurnObligationResolver for UnclaimedResolver {
        async fn resolve(
            &self,
            _expected_member: &BridgeMemberIncarnation,
            _record: &BridgeTurnOutcomeRecord,
        ) -> Result<OutcomeRecordDisposition, MobError> {
            self.resolved.fetch_add(1, Ordering::SeqCst);
            Ok(OutcomeRecordDisposition::AcknowledgeUnclaimed)
        }

        async fn acknowledge(
            &self,
            _expected_member: &BridgeMemberIncarnation,
            _ack: &BridgeTurnOutcomeAck,
        ) -> Result<(), MobError> {
            self.acknowledged.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn registry() -> (RemoteFlowTicketRegistry, Arc<RecordingResolver>) {
        let resolver = Arc::new(RecordingResolver::default());
        (
            RemoteFlowTicketRegistry::new(Arc::clone(&resolver) as Arc<_>),
            resolver,
        )
    }

    fn unclaimed_registry() -> (RemoteFlowTicketRegistry, Arc<UnclaimedResolver>) {
        let resolver = Arc::new(UnclaimedResolver::default());
        (
            RemoteFlowTicketRegistry::new(Arc::clone(&resolver) as Arc<_>),
            resolver,
        )
    }

    fn target() -> AgentIdentity {
        AgentIdentity::from("placed-worker")
    }

    fn run_completed(result: &str) -> AgentEvent {
        AgentEvent::RunCompleted {
            session_id: SessionId::new(),
            result: result.to_string(),
            structured_output: None,
            extraction_required: false,
            usage: meerkat_core::Usage::default(),
            terminal_cause_kind: None,
        }
    }

    fn record(input_id: &str, generation: u64, terminal_seq: u64) -> BridgeTurnOutcomeRecord {
        BridgeTurnOutcomeRecord {
            input_id: input_id.to_string(),
            generation,
            fence_token: 7,
            terminal_seq,
            outcome: WireFlowTurnOutcome::RunCompleted,
        }
    }

    #[test]
    fn resolve_plan_never_misclassifies_known_pending_custody_as_unclaimed() {
        let flow = mob_dsl::RemoteTurnObligation {
            input_id: mob_dsl::InputId("flow-input".to_string()),
            ..Default::default()
        };
        let kickoff = mob_dsl::PlacedKickoffObligation {
            input_id: mob_dsl::InputId("kickoff-input".to_string()),
            ..Default::default()
        };
        let completion = mob_dsl::PlacedCompletionObligation {
            input_id: mob_dsl::InputId("completion-input".to_string()),
            ..Default::default()
        };

        assert_eq!(
            remote_outcome_resolve_plan(RemoteOutcomeCustody::Absent, None)
                .expect("an exact absent lookup is the unclaimed case"),
            RemoteOutcomeResolvePlan::AcknowledgeUnclaimed
        );
        assert_eq!(
            remote_outcome_resolve_plan(
                RemoteOutcomeCustody::Pending,
                Some(RemoteOutcomeObligation::Flow(flow)),
            )
            .expect("pending flow custody remains retained"),
            RemoteOutcomeResolvePlan::Retain
        );
        assert_eq!(
            remote_outcome_resolve_plan(
                RemoteOutcomeCustody::Pending,
                Some(RemoteOutcomeObligation::PlacedKickoff(kickoff.clone())),
            )
            .expect("pending kickoff custody resolves through its machine path"),
            RemoteOutcomeResolvePlan::ResolvePlacedKickoff(kickoff)
        );
        assert_eq!(
            remote_outcome_resolve_plan(
                RemoteOutcomeCustody::Pending,
                Some(RemoteOutcomeObligation::PlacedCompletion(
                    completion.clone(),
                )),
            )
            .expect("pending placed completion resolves durably before waiter fanout"),
            RemoteOutcomeResolvePlan::ResolvePlacedCompletion(completion.clone())
        );
        assert_eq!(
            remote_outcome_resolve_plan(
                RemoteOutcomeCustody::Resolved,
                Some(RemoteOutcomeObligation::PlacedCompletion(
                    completion.clone(),
                )),
            )
            .expect("resolved placed completion still needs waiter fanout and host ACK"),
            RemoteOutcomeResolvePlan::ResolvePlacedCompletion(completion)
        );
    }

    #[tokio::test]
    async fn fold_validated_interaction_without_machine_custody_is_unclaimed_ack_ready() {
        let (registry, resolver) = unclaimed_registry();
        let target = target();
        let expected_member = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        let interaction_uuid = uuid::Uuid::from_u128(101);
        registry.install_member_residency(&expected_member);
        registry.observe_member_event_row_exact(
            &expected_member,
            5,
            &AgentEvent::InteractionComplete {
                interaction_id: meerkat_core::interaction::InteractionId(interaction_uuid),
                result: "done".to_string(),
                structured_output: None,
            },
        );
        let sidecar = BridgeTurnOutcomeRecord {
            input_id: interaction_uuid.to_string(),
            generation: expected_member.generation,
            fence_token: expected_member.fence_token,
            terminal_seq: 5,
            outcome: WireFlowTurnOutcome::InteractionComplete,
        };

        assert_eq!(
            registry
                .consume_turn_outcome_record(&expected_member, &sidecar)
                .await
                .expect("the exact event fold validates the sidecar"),
            OutcomeRecordDisposition::AcknowledgeUnclaimed
        );
        assert_eq!(resolver.resolved.load(Ordering::SeqCst), 1);

        registry
            .acknowledge_turn_outcome(
                &expected_member,
                &BridgeTurnOutcomeAck {
                    generation: sidecar.generation,
                    fence_token: sidecar.fence_token,
                    input_id: sidecar.input_id,
                },
            )
            .await
            .expect("host ACK confirmation closes unclaimed fold custody");
        assert_eq!(resolver.acknowledged.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn same_kind_interaction_sidecar_cannot_cross_input_correlation() {
        let (registry, resolver) = unclaimed_registry();
        let target = target();
        let expected_member = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        let event_interaction = uuid::Uuid::from_u128(109);
        registry.install_member_residency(&expected_member);
        registry.observe_member_event_row_exact(
            &expected_member,
            5,
            &AgentEvent::InteractionComplete {
                interaction_id: meerkat_core::interaction::InteractionId(event_interaction),
                result: "done".to_string(),
                structured_output: None,
            },
        );
        let wrong_input = BridgeTurnOutcomeRecord {
            input_id: uuid::Uuid::from_u128(110).to_string(),
            generation: expected_member.generation,
            fence_token: expected_member.fence_token,
            terminal_seq: 5,
            outcome: WireFlowTurnOutcome::InteractionComplete,
        };

        assert_eq!(
            registry
                .consume_turn_outcome_record(&expected_member, &wrong_input)
                .await
                .expect("same-kind cross-input sidecar is retained"),
            OutcomeRecordDisposition::Retain
        );
        assert_eq!(
            resolver.resolved.load(Ordering::SeqCst),
            0,
            "cross-input correlation cannot reach machine or volatile waiter resolution"
        );
    }

    #[tokio::test]
    async fn omitted_interaction_without_machine_custody_is_unclaimed_ack_ready() {
        let (registry, resolver) = unclaimed_registry();
        let target = target();
        let expected_member = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        let interaction_uuid = uuid::Uuid::from_u128(102);
        registry.install_member_residency(&expected_member);
        registry.observe_omitted_member_event_row_exact(&expected_member, 9);
        let sidecar = BridgeTurnOutcomeRecord {
            input_id: interaction_uuid.to_string(),
            generation: expected_member.generation,
            fence_token: expected_member.fence_token,
            terminal_seq: 9,
            outcome: WireFlowTurnOutcome::InteractionFailed {
                detail: meerkat_contracts::wire::supervisor_bridge::WireFlowFailureDetail::complete(
                    "provider failed".to_string(),
                ),
            },
        };

        assert_eq!(
            registry
                .consume_turn_outcome_record(&expected_member, &sidecar)
                .await
                .expect("the typed omission validates the bounded sidecar"),
            OutcomeRecordDisposition::AcknowledgeUnclaimed
        );
        assert_eq!(resolver.resolved.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn complete_sidecar_page_prunes_only_unclaimed_nonterminal_omissions() {
        let (registry, _resolver) = registry();
        let target = target();
        let expected_member = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        registry.install_member_residency(&expected_member);
        registry.observe_omitted_member_event_row_exact(&expected_member, 5);
        registry.observe_omitted_member_event_row_exact(&expected_member, 6);
        let terminal = BridgeTurnOutcomeRecord {
            input_id: uuid::Uuid::from_u128(103).to_string(),
            generation: expected_member.generation,
            fence_token: expected_member.fence_token,
            terminal_seq: 6,
            outcome: WireFlowTurnOutcome::InteractionComplete,
        };

        registry.observe_turn_outcome_page(
            &expected_member,
            7,
            true,
            std::slice::from_ref(&terminal),
        );

        let members = registry.members_guard();
        let lane = members.get(&target).expect("installed member lane");
        let residency = RemoteResidencyKey::from(&expected_member);
        assert!(
            !lane.omitted_event_seqs.contains(&(residency.clone(), 5)),
            "a complete page proves the oversized nonterminal has no sidecar"
        );
        assert!(
            lane.omitted_event_seqs.contains(&(residency, 6)),
            "the exact sidecar keeps its omitted terminal pinned through ACK"
        );
    }

    /// Rows-then-sidecar page order: the fold terminal at `terminal_seq`
    /// becomes the armed ticket's payload.
    #[tokio::test]
    async fn rows_then_sidecar_attributes_payload() {
        let (registry, resolver) = registry();
        let target = target();
        let rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));

        registry.observe_member_event_row(
            &target,
            1,
            4,
            &AgentEvent::TurnStarted { turn_number: 1 },
        );
        registry.observe_member_event_row(&target, 1, 5, &run_completed("payload"));
        registry
            .consume_turn_outcome_record_for_test(&target, &record("input-1", 1, 5))
            .await
            .expect("consume sidecar");

        match rx.await.expect("armed ticket resolves") {
            FlowTurnOutcome::Completed { output, .. } => assert_eq!(output, "payload"),
            other => panic!("expected completed outcome, got {other:?}"),
        }
        assert_eq!(
            resolver.count.load(Ordering::SeqCst),
            1,
            "obligation resolved once"
        );
    }

    #[tokio::test]
    async fn tracked_peer_extraction_failure_agrees_with_interaction_wire_family() {
        let (good_registry, resolver) = registry();
        let target = target();
        let expected_member = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        good_registry.install_member_residency(&expected_member);
        let reason = meerkat_core::event::InteractionFailureReason::ExtractionFailed {
            last_output: "raw".to_string(),
            attempts: 2,
            reason: "invalid schema".to_string(),
        };
        let rendered = reason.to_string();
        let interaction_id = meerkat_core::interaction::InteractionId(uuid::Uuid::from_u128(108));
        good_registry.observe_member_event_row_exact(
            &expected_member,
            5,
            &AgentEvent::InteractionFailed {
                interaction_id,
                reason,
            },
        );
        let sidecar = BridgeTurnOutcomeRecord {
            input_id: interaction_id.0.to_string(),
            generation: 1,
            fence_token: 7,
            terminal_seq: 5,
            outcome: WireFlowTurnOutcome::InteractionFailed {
                detail: meerkat_contracts::wire::supervisor_bridge::WireFlowFailureDetail::complete(
                    rendered.clone(),
                ),
            },
        };

        assert_eq!(
            good_registry
                .consume_turn_outcome_record(&expected_member, &sidecar)
                .await
                .expect("tracked Peer extraction sidecar resolves"),
            OutcomeRecordDisposition::Acknowledge
        );
        assert_eq!(resolver.count.load(Ordering::SeqCst), 1);

        let (mismatched_registry, mismatched_resolver) = registry();
        mismatched_registry.install_member_residency(&expected_member);
        mismatched_registry.observe_member_event_row_exact(
            &expected_member,
            5,
            &AgentEvent::InteractionFailed {
                interaction_id,
                reason: meerkat_core::event::InteractionFailureReason::ExtractionFailed {
                    last_output: "raw".to_string(),
                    attempts: 2,
                    reason: "invalid schema".to_string(),
                },
            },
        );
        let mut mismatched_rendered = rendered;
        let last = mismatched_rendered
            .pop()
            .expect("rendered extraction failure is non-empty");
        mismatched_rendered.push(if last == 'x' { 'y' } else { 'x' });
        let mismatched_sidecar = BridgeTurnOutcomeRecord {
            outcome: WireFlowTurnOutcome::InteractionFailed {
                detail: meerkat_contracts::wire::supervisor_bridge::WireFlowFailureDetail::complete(
                    mismatched_rendered,
                ),
            },
            ..sidecar
        };
        assert_eq!(
            mismatched_registry
                .consume_turn_outcome_record(&expected_member, &mismatched_sidecar)
                .await
                .expect("mismatched extraction sidecar is retained"),
            OutcomeRecordDisposition::Retain
        );
        assert_eq!(mismatched_resolver.count.load(Ordering::SeqCst), 0);

        let (ordinary_registry, ordinary_resolver) = registry();
        ordinary_registry.install_member_residency(&expected_member);
        ordinary_registry.observe_member_event_row_exact(
            &expected_member,
            5,
            &AgentEvent::ExtractionFailed {
                session_id: SessionId::new(),
                last_output: "raw".to_string(),
                attempts: 2,
                reason: "invalid schema".to_string(),
            },
        );
        let ordinary_sidecar = BridgeTurnOutcomeRecord {
            input_id: "input-1".to_string(),
            generation: 1,
            fence_token: 7,
            terminal_seq: 5,
            outcome: WireFlowTurnOutcome::InteractionFailed {
                detail: meerkat_contracts::wire::supervisor_bridge::WireFlowFailureDetail::complete(
                    "structured output extraction failed after 2 attempt(s): invalid schema; last_output=\"raw\""
                        .to_string(),
                ),
            },
        };
        assert_eq!(
            ordinary_registry
                .consume_turn_outcome_record(&expected_member, &ordinary_sidecar)
                .await
                .expect("ordinary extraction cannot cross into the Peer interaction family"),
            OutcomeRecordDisposition::Retain
        );
        assert_eq!(ordinary_resolver.count.load(Ordering::SeqCst), 0);
        assert!(
            !kinds_agree(
                TurnTerminalKind::InteractionFailed,
                &WireFlowTurnOutcome::ExtractionFailed {
                    detail:
                        meerkat_contracts::wire::supervisor_bridge::WireFlowFailureDetail::complete(
                            "not extraction-authorized".to_string(),
                        ),
                },
            ),
            "compatibility must remain one-way"
        );
    }

    #[test]
    fn failed_terminal_details_require_exact_or_compacted_prefix_agreement() {
        use meerkat_contracts::wire::supervisor_bridge::WireFlowFailureDetail;
        use meerkat_core::turn_terminal::TurnTerminalOutcome;

        fn wire_failure(
            kind: TurnTerminalKind,
            detail: WireFlowFailureDetail,
        ) -> WireFlowTurnOutcome {
            match kind {
                TurnTerminalKind::ExtractionFailed => {
                    WireFlowTurnOutcome::ExtractionFailed { detail }
                }
                TurnTerminalKind::RunFailed => WireFlowTurnOutcome::RunFailed { detail },
                TurnTerminalKind::InteractionFailed => {
                    WireFlowTurnOutcome::InteractionFailed { detail }
                }
                other => panic!("{other} is not a detailed failure family"),
            }
        }

        let reason = "provider exploded".to_string();
        let conflicting = "xrovider exploded".to_string();
        assert_eq!(reason.len(), conflicting.len());
        for kind in [
            TurnTerminalKind::ExtractionFailed,
            TurnTerminalKind::RunFailed,
            TurnTerminalKind::InteractionFailed,
        ] {
            let terminal = FoldTerminal {
                classified: ClassifiedTurnTerminal {
                    kind,
                    outcome: TurnTerminalOutcome::Failed {
                        reason: reason.clone(),
                    },
                },
                interaction_id: None,
                interaction_scoped_extraction_failure: false,
            };
            assert!(terminal_agrees_with_wire(
                &terminal,
                &wire_failure(kind, WireFlowFailureDetail::complete(reason.clone())),
            ));
            assert!(terminal_agrees_with_wire(
                &terminal,
                &wire_failure(
                    kind,
                    WireFlowFailureDetail {
                        text: "provider".to_string(),
                        original_utf8_bytes: u64::try_from(reason.len())
                            .expect("test reason length fits u64"),
                        truncated: true,
                    },
                ),
            ));
            assert!(!terminal_agrees_with_wire(
                &terminal,
                &wire_failure(kind, WireFlowFailureDetail::complete(conflicting.clone()),),
            ));
            assert!(!terminal_agrees_with_wire(
                &terminal,
                &wire_failure(
                    kind,
                    WireFlowFailureDetail {
                        text: "xrovider".to_string(),
                        original_utf8_bytes: u64::try_from(reason.len())
                            .expect("test reason length fits u64"),
                        truncated: true,
                    },
                ),
            ));
        }

        let tracked_peer_extraction = FoldTerminal {
            classified: ClassifiedTurnTerminal {
                kind: TurnTerminalKind::ExtractionFailed,
                outcome: TurnTerminalOutcome::Failed {
                    reason: reason.clone(),
                },
            },
            interaction_id: Some(meerkat_core::interaction::InteractionId(
                uuid::Uuid::from_u128(1),
            )),
            interaction_scoped_extraction_failure: true,
        };
        assert!(terminal_agrees_with_wire(
            &tracked_peer_extraction,
            &WireFlowTurnOutcome::InteractionFailed {
                detail: WireFlowFailureDetail {
                    text: "provider".to_string(),
                    original_utf8_bytes: u64::try_from(reason.len())
                        .expect("test reason length fits u64"),
                    truncated: true,
                },
            },
        ));
        assert!(
            !terminal_agrees_with_wire(
                &tracked_peer_extraction,
                &WireFlowTurnOutcome::ExtractionFailed {
                    detail: WireFlowFailureDetail::complete(reason.clone()),
                },
            ),
            "an interaction-scoped extraction failure cannot lose its interaction family"
        );
        assert!(!terminal_agrees_with_wire(
            &tracked_peer_extraction,
            &WireFlowTurnOutcome::InteractionFailed {
                detail: WireFlowFailureDetail {
                    text: "xrovider".to_string(),
                    original_utf8_bytes: u64::try_from(reason.len())
                        .expect("test reason length fits u64"),
                    truncated: true,
                },
            },
        ));
    }

    #[tokio::test]
    async fn wrong_generation_same_input_never_completes_armed_ticket() {
        let (registry, resolver) = registry();
        let target = target();
        let expected_member = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        registry.install_member_residency(&expected_member);
        let mut rx = registry.arm_exact(
            &target,
            "input-1",
            &RunId::new(),
            &StepId::from("s1"),
            &expected_member,
        );
        registry.observe_member_event_row_exact(&expected_member, 5, &run_completed("payload"));

        let disposition = registry
            .consume_turn_outcome_record_for_test(&target, &record("input-1", 2, 5))
            .await
            .expect("wrong-generation sidecar is retained");
        assert_eq!(disposition, OutcomeRecordDisposition::Retain);
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(resolver.count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn recovered_arm_never_replaces_live_flow_ticket() {
        let (registry, _resolver) = registry();
        let target = target();
        let run_id = RunId::new();
        let step_id = StepId::from("s1");
        let expected_member = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        registry.install_member_residency(&expected_member);
        let mut live = registry.arm_exact(&target, "input-1", &run_id, &step_id, &expected_member);
        let adopted = registry
            .arm_recovered_exact(&target, "input-1", &run_id, &step_id, &expected_member)
            .expect("matching recovery arm");
        assert!(
            adopted.is_none(),
            "live FlowEngine ticket must remain owner"
        );
        assert!(matches!(
            live.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn recovered_arm_refuses_shell_synthesized_terminal_receipt() {
        let (registry, _resolver) = registry();
        let target = target();
        let run_id = RunId::new();
        let step_id = StepId::from("s1");
        let expected_member = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        registry.install_member_residency(&expected_member);
        let receiver = registry
            .arm_recovered_exact(&target, "input-1", &run_id, &step_id, &expected_member)
            .expect("arm recovered")
            .expect("new recovered waiter");
        registry.fail_armed(
            &target,
            "input-1",
            REMATERIALIZED_STEP_FAILURE_REASON.to_string(),
        );
        assert!(
            receiver.await.is_err(),
            "rematerialization shell failure closes adopter without minting an observed-terminal outcome"
        );
    }

    #[tokio::test]
    async fn wrong_fence_same_input_never_completes_armed_ticket() {
        let (registry, resolver) = registry();
        let target = target();
        let expected_member = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        registry.install_member_residency(&expected_member);
        let mut rx = registry.arm_exact(
            &target,
            "input-1",
            &RunId::new(),
            &StepId::from("s1"),
            &expected_member,
        );
        registry.observe_member_event_row_exact(&expected_member, 5, &run_completed("payload"));
        let mut wrong = record("input-1", 1, 5);
        wrong.fence_token = 8;

        let disposition = registry
            .consume_turn_outcome_record_for_test(&target, &wrong)
            .await
            .expect("wrong-fence sidecar is retained");
        assert_eq!(disposition, OutcomeRecordDisposition::Retain);
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(resolver.count.load(Ordering::SeqCst), 0);
    }

    /// Record-before-fold: buffered until the rows catch up, then attributed.
    #[tokio::test]
    async fn record_before_fold_buffers_until_rows_arrive() {
        let (registry, resolver) = registry();
        let target = target();
        let mut rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));

        registry
            .consume_turn_outcome_record_for_test(&target, &record("input-1", 1, 5))
            .await
            .expect("record ahead is retained");
        assert!(
            rx.try_recv().is_err(),
            "no payload before the fold reaches seq 5"
        );

        registry.observe_member_event_row(&target, 1, 5, &run_completed("late-rows"));
        registry
            .consume_turn_outcome_record_for_test(&target, &record("input-1", 1, 5))
            .await
            .expect("replayed sidecar joins after rows");
        match rx.await.expect("ticket resolves after the fold catches up") {
            FlowTurnOutcome::Completed { output, .. } => assert_eq!(output, "late-rows"),
            other => panic!("expected completed outcome, got {other:?}"),
        }
        assert_eq!(resolver.count.load(Ordering::SeqCst), 1);
    }

    /// An immutable failure event too large for the bridge is skipped typed
    /// by the pump. Its bounded journal sidecar still resolves the ticket at
    /// the exact omitted seq instead of waiting for the normal turn timeout.
    #[tokio::test]
    async fn oversized_failed_event_resolves_from_bounded_sidecar() {
        let (registry, resolver) = registry();
        let target = target();
        let rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));

        registry.observe_omitted_member_event_row(&target, 1, 5);
        registry
            .consume_turn_outcome_record_for_test(
                &target,
                &BridgeTurnOutcomeRecord {
                    input_id: "input-1".to_string(),
                    generation: 1,
                    fence_token: 7,
                    terminal_seq: 5,
                    outcome: WireFlowTurnOutcome::RunFailed {
                        detail: meerkat_contracts::wire::supervisor_bridge::WireFlowFailureDetail {
                            text: "provider exploded".to_string(),
                            original_utf8_bytes: 200_000,
                            truncated: true,
                        },
                    },
                },
            )
            .await
            .expect("consume omitted terminal sidecar");

        match rx.await.expect("ticket resolves from sidecar") {
            FlowTurnOutcome::Failed { reason, .. } => {
                assert!(reason.contains("provider exploded"));
                assert!(reason.contains("200000 UTF-8 bytes"));
            }
            other => panic!("expected failed outcome, got {other:?}"),
        }
        assert_eq!(resolver.count.load(Ordering::SeqCst), 1);
    }

    /// Fold gap mid-extraction-pair: the shared classifier's missing-pending
    /// fallback (`structured_output.to_string()`) is the payload — no
    /// backfill API, no second read path.
    #[tokio::test]
    async fn fold_gap_uses_classifier_missing_pending_fallback() {
        let (registry, _resolver) = registry();
        let target = target();
        let rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));

        // The pending RunCompleted row was consumed by a previous pump
        // incarnation; the fold starts at the extraction terminal.
        registry.observe_member_event_row(
            &target,
            1,
            9,
            &AgentEvent::ExtractionSucceeded {
                session_id: SessionId::new(),
                structured_output: serde_json::json!({"v": 7}),
                schema_warnings: None,
            },
        );
        registry
            .consume_turn_outcome_record_for_test(
                &target,
                &BridgeTurnOutcomeRecord {
                    input_id: "input-1".to_string(),
                    generation: 1,
                    fence_token: 7,
                    terminal_seq: 9,
                    outcome: WireFlowTurnOutcome::ExtractionSucceeded,
                },
            )
            .await
            .expect("consume extraction sidecar");

        match rx.await.expect("ticket resolves") {
            FlowTurnOutcome::Completed {
                output,
                structured_output,
            } => {
                assert_eq!(output, serde_json::json!({"v": 7}).to_string());
                assert_eq!(structured_output, Some(serde_json::json!({"v": 7})));
            }
            other => panic!("expected completed outcome, got {other:?}"),
        }
    }

    /// Fold kind and record kind disagreeing is a typed failure naming both
    /// owners — never a silent preference.
    #[tokio::test]
    async fn kind_mismatch_is_typed_failure() {
        let (registry, _resolver) = registry();
        let target = target();
        let rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));

        registry.observe_member_event_row(&target, 1, 5, &run_completed("payload"));
        registry.consume_turn_outcome_record_for_test(
            &target,
            &BridgeTurnOutcomeRecord {
                input_id: "input-1".to_string(),
                generation: 1,
                fence_token: 7,
                terminal_seq: 5,
                outcome: WireFlowTurnOutcome::RunFailed {
                    detail: meerkat_contracts::wire::supervisor_bridge::WireFlowFailureDetail::complete(
                        "journal disagrees".to_string(),
                    ),
                },
            },
        )
        .await
        .expect("consume mismatched sidecar");

        match rx.await.expect("ticket resolves") {
            FlowTurnOutcome::Failed { reason, .. } => {
                assert!(reason.contains("mismatch"), "names the mismatch: {reason}");
                assert!(
                    reason.contains("run_completed"),
                    "names the fold kind: {reason}"
                );
                assert!(
                    reason.contains("RunFailed"),
                    "names the record kind: {reason}"
                );
            }
            other => panic!("expected typed mismatch failure, got {other:?}"),
        }
    }

    /// A late record after disarm is a display-only no-op consume (still
    /// resolving the obligation, set-idempotent); a duplicate record no-ops.
    #[tokio::test]
    async fn late_record_after_disarm_is_noop_consume() {
        let (registry, resolver) = registry();
        let target = target();
        let mut rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));
        registry.disarm(&target, "input-1");
        // Disarm is idempotent.
        registry.disarm(&target, "input-1");

        registry.observe_member_event_row(&target, 1, 5, &run_completed("orphan"));
        registry
            .consume_turn_outcome_record_for_test(&target, &record("input-1", 1, 5))
            .await
            .expect("consume sidecar after disarm");
        // The obligation still resolves (timeout path already resolved it
        // too — the transition is set-idempotent).
        assert_eq!(resolver.count.load(Ordering::SeqCst), 1);
        assert!(
            matches!(rx.try_recv(), Err(oneshot::error::TryRecvError::Closed)),
            "the disarmed ticket's sender was dropped without a payload"
        );

        // Until the poll response confirms the host ACK, replay remains
        // eligible and exact resolution is retried.
        registry
            .consume_turn_outcome_record_for_test(&target, &record("input-1", 1, 5))
            .await
            .expect("duplicate sidecar remains ACK eligible");
        assert_eq!(resolver.count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn replayed_ready_terminal_rehydrates_late_recovered_adopter() {
        let (registry, _resolver) = registry();
        let target = target();
        let run_id = RunId::new();
        let step_id = StepId::from("s1");
        let expected_member = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        registry.install_member_residency(&expected_member);
        registry.observe_member_event_row_exact(
            &expected_member,
            5,
            &run_completed("recovered output"),
        );
        let sidecar = record("input-1", 1, 5);
        registry
            .consume_turn_outcome_record(&expected_member, &sidecar)
            .await
            .expect("first pre-arm fold caches the exact ready terminal");

        let receiver = registry
            .arm_recovered_exact(&target, "input-1", &run_id, &step_id, &expected_member)
            .expect("late recovered arm is exact")
            .expect("late recovered arm owns a waiter");
        registry
            .consume_turn_outcome_record(&expected_member, &sidecar)
            .await
            .expect("exact terminal replay rehydrates the late adopter");
        assert!(matches!(
            receiver
                .await
                .expect("late adopter receives cached terminal"),
            FlowTurnOutcome::Completed { .. }
        ));
    }

    /// Generation advance past a ticket's baseline fails the step typed
    /// BEFORE any timeout. Flow persistence commits the obligation later.
    #[tokio::test]
    async fn generation_watch_fails_stale_tickets_typed() {
        let (registry, resolver) = registry();
        let target = target();

        // First page observed at generation 1, THEN the ticket arms.
        registry.observe_member_generation(&target, 1);
        let rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));

        // The pump observes the rematerialized incarnation.
        registry.observe_member_generation(&target, 2);
        match rx.await.expect("ticket resolves") {
            FlowTurnOutcome::Failed { reason, .. } => {
                assert_eq!(
                    reason,
                    "member rematerialized at new generation during flow step"
                );
            }
            other => panic!("expected typed generation failure, got {other:?}"),
        }
        assert_eq!(resolver.count.load(Ordering::SeqCst), 0);

        // Old-generation records never attribute forward: a gen-1 record for
        // the failed key is a no-op consume now.
        registry.observe_member_event_row(&target, 2, 1, &run_completed("new-gen"));
        registry
            .consume_turn_outcome_record_for_test(&target, &record("input-1", 1, 5))
            .await
            .expect("lower-generation sidecar is retained");
        assert_eq!(
            resolver.count.load(Ordering::SeqCst),
            0,
            "lower-generation pages are dropped entirely"
        );
    }

    #[tokio::test]
    async fn promoted_binding_install_fails_old_ticket_without_replacement_pump() {
        let (registry, _resolver) = registry();
        let target = target();
        let old = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        registry.install_member_residency(&old);
        let receiver = registry.arm_exact(
            &target,
            "input-g1",
            &RunId::new(),
            &StepId::from("s1"),
            &old,
        );
        let promoted = BridgeMemberIncarnation {
            binding_generation: old.binding_generation + 1,
            ..old
        };

        // This is the sink transition performed by the actor's promotion
        // barrier after the G1 pump is joined. No G2 pump is needed to make
        // the old ticket terminal promptly.
        registry.install_member_residency(&promoted);

        match receiver.await.expect("old ticket is failed by promotion") {
            FlowTurnOutcome::Failed { reason, .. } => {
                assert_eq!(reason, REMATERIALIZED_STEP_FAILURE_REASON);
            }
            other => panic!("expected typed residency-change failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn late_old_binding_arm_after_promotion_fails_immediately() {
        let (registry, resolver) = registry();
        let target = target();
        let old = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        let promoted = BridgeMemberIncarnation {
            binding_generation: old.binding_generation + 1,
            ..old.clone()
        };
        registry.install_member_residency(&promoted);

        // Models a reserve(G1) reply arriving after the actor's promotion
        // barrier has already installed G2.
        let live = registry.arm_exact(
            &target,
            "late-live-g1",
            &RunId::new(),
            &StepId::from("s1"),
            &old,
        );
        match live.await.expect("late live arm fails typed") {
            FlowTurnOutcome::Failed { reason, .. } => {
                assert_eq!(reason, REMATERIALIZED_STEP_FAILURE_REASON);
            }
            other => panic!("expected typed late-arm failure, got {other:?}"),
        }

        let recovered = registry
            .arm_recovered_exact(
                &target,
                "late-recovered-g1",
                &RunId::new(),
                &StepId::from("s2"),
                &old,
            )
            .expect("late recovered arm is classified")
            .expect("late recovered arm returns a terminal receiver");
        assert!(
            recovered.await.is_err(),
            "a stale recovery adopter closes without synthesizing an observed terminal"
        );
        assert_eq!(
            resolver.count.load(Ordering::SeqCst),
            0,
            "late recovered arming cannot resolve custody or mint a receipt"
        );
        assert!(
            registry
                .members_guard()
                .get(&target)
                .expect("installed lane")
                .armed
                .is_empty(),
            "neither delayed G1 arm may survive in the G2 lane"
        );
    }

    #[tokio::test]
    async fn stale_binding_ack_cannot_prune_current_residency_state() {
        let (registry, resolver) = registry();
        let target = target();
        let old = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        registry.install_member_residency(&old);
        let current = BridgeMemberIncarnation {
            binding_generation: old.binding_generation + 1,
            ..old.clone()
        };
        registry.install_member_residency(&current);
        let receiver = registry.arm_exact(
            &target,
            "same-input",
            &RunId::new(),
            &StepId::from("s2"),
            &current,
        );
        registry.observe_member_event_row_exact(&current, 5, &run_completed("current"));
        let sidecar = record("same-input", current.generation, 5);
        assert_eq!(
            registry
                .consume_turn_outcome_record(&current, &sidecar)
                .await
                .expect("current sidecar consumes"),
            OutcomeRecordDisposition::Acknowledge,
        );
        let stale_ack = BridgeTurnOutcomeAck {
            generation: old.generation,
            fence_token: old.fence_token,
            input_id: "same-input".to_string(),
        };
        registry
            .acknowledge_turn_outcome(&old, &stale_ack)
            .await
            .expect("stale ACK is ignored");
        assert_eq!(
            resolver.acknowledged.load(Ordering::SeqCst),
            0,
            "a stale residency must not reach the machine ACK resolver"
        );

        match receiver.await.expect("current ticket remains resolved") {
            FlowTurnOutcome::Completed { output, .. } => assert_eq!(output, "current"),
            other => panic!("expected current completed outcome, got {other:?}"),
        }
        let members = registry.members_guard();
        let lane = members.get(&target).expect("current lane");
        assert!(
            lane.fold_ready_records.iter().any(|(residency, _, input)| {
                residency == &RemoteResidencyKey::from(&current) && input == "same-input"
            }),
            "old ACK cannot prune the current residency's ready record"
        );
    }

    /// Controlling-side respawn mark: armed tickets fail typed immediately,
    /// the obligation remains pending for durable flow failure commit, and the mark stays visible to the dispatch
    /// unwind until a generation observation ABOVE the floor clears it —
    /// residual old-generation pages never do.
    #[tokio::test]
    async fn rematerializing_mark_fails_armed_tickets_and_reports_to_unwind() {
        let (registry, resolver) = registry();
        let target = target();
        registry.observe_member_generation(&target, 1);
        let rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));

        registry.note_member_rematerializing(&target, 1);
        match rx.await.expect("ticket resolves") {
            FlowTurnOutcome::Failed { reason, .. } => {
                assert_eq!(reason, REMATERIALIZED_STEP_FAILURE_REASON);
            }
            other => panic!("expected typed rematerialization failure, got {other:?}"),
        }
        assert_eq!(resolver.count.load(Ordering::SeqCst), 0);
        assert!(
            registry.is_member_rematerializing(&target),
            "the unwind check reports the mark"
        );

        // An old-generation residual page does NOT clear the mark.
        registry.observe_member_generation(&target, 1);
        assert!(registry.is_member_rematerializing(&target));

        // The replacement's above-floor observation clears it.
        registry.observe_member_generation(&target, 2);
        assert!(!registry.is_member_rematerializing(&target));
    }

    #[tokio::test]
    async fn host_revoke_wakes_only_the_exact_ticket_with_the_canonical_failure() {
        let (registry, resolver) = registry();
        let target = target();
        let expected_member = RemoteFlowTicketRegistry::test_member(&target, 1, 7);
        registry.install_member_residency(&expected_member);
        let rx = registry.arm_exact(
            &target,
            "host-revoke-input",
            &RunId::new(),
            &StepId::from("s1"),
            &expected_member,
        );

        registry.fail_armed(
            &target,
            "other-input",
            HOST_REVOKED_STEP_FAILURE_REASON.to_string(),
        );
        registry.fail_armed(
            &target,
            "host-revoke-input",
            HOST_REVOKED_STEP_FAILURE_REASON.to_string(),
        );
        match rx.await.expect("the exact host-revoked ticket resolves") {
            FlowTurnOutcome::Failed {
                reason,
                disposition,
            } => {
                assert_eq!(reason, HOST_REVOKED_STEP_FAILURE_REASON);
                assert_eq!(disposition, FlowTurnFailureDisposition::ObservedTerminal);
            }
            other => panic!("expected typed host-revoke failure, got {other:?}"),
        }
        // Recovery can have durable custody without a live ticket. Waking an
        // absent exact key is intentionally a no-op; the durable terminal +
        // Disposed replay seam remains the recovery authority.
        registry.fail_armed(
            &target,
            "host-revoke-input",
            HOST_REVOKED_STEP_FAILURE_REASON.to_string(),
        );
        assert_eq!(resolver.count.load(Ordering::SeqCst), 0);
    }

    /// Member removal drops the lane and fails still-armed tickets typed;
    /// exact host release owns custody disposal.
    #[tokio::test]
    async fn drop_lane_fails_armed_tickets_typed() {
        let (registry, resolver) = registry();
        let target = target();
        let rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));

        registry.drop_lane(&target);
        match rx.await.expect("ticket resolves") {
            FlowTurnOutcome::Failed { reason, .. } => {
                assert_eq!(reason, MEMBER_RELEASED_STEP_FAILURE_REASON);
            }
            other => panic!("expected typed removal failure, got {other:?}"),
        }
        assert_eq!(resolver.count.load(Ordering::SeqCst), 0);
        assert!(
            !registry.is_member_rematerializing(&target),
            "a dropped lane carries no marks"
        );
    }

    /// A legacy ChannelClosed sidecar is mechanical waiter loss, so it cannot
    /// resolve or become ACK-ready without a durable interaction terminal.
    #[tokio::test]
    async fn channel_closed_record_retains_pending_ticket() {
        let (registry, _resolver) = registry();
        let target = target();
        let rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));

        let disposition = registry
            .consume_turn_outcome_record_for_test(
                &target,
                &BridgeTurnOutcomeRecord {
                    input_id: "input-1".to_string(),
                    generation: 1,
                    fence_token: 7,
                    terminal_seq: 0,
                    outcome: WireFlowTurnOutcome::ChannelClosed,
                },
            )
            .await
            .expect("consume channel-closed sidecar");
        assert_eq!(disposition, OutcomeRecordDisposition::Retain);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), rx)
                .await
                .is_err(),
            "mechanical ChannelClosed must retain the exact ticket Pending"
        );
    }

    /// The fold passing an evicted/missing terminal requests replay from the
    /// frozen durable cursor and never fabricates failure/ACK authority.
    #[tokio::test]
    async fn fold_past_terminal_seq_without_terminal_fails_loud() {
        let (registry, _resolver) = registry();
        let target = target();
        let rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));

        registry.observe_member_event_row(
            &target,
            1,
            6,
            &AgentEvent::TurnStarted { turn_number: 1 },
        );
        let disposition = registry
            .consume_turn_outcome_record_for_test(&target, &record("input-1", 1, 5))
            .await
            .expect("fold miss requests replay");
        assert_eq!(disposition, OutcomeRecordDisposition::Replay);
        let mut rx = rx;
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn sidecar_announced_fold_survives_more_than_256_unrelated_terminals() {
        let (registry, _resolver) = registry();
        let target = target();
        let rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));
        let sidecar = record("input-1", 1, 1);
        // The page contract establishes its authenticated incarnation before
        // sidecars are pre-observed. A mixed sidecar must never advance/reset
        // the fold by itself.
        registry.observe_member_generation(&target, 1);
        registry.observe_turn_outcome_page_for_test(&target, std::slice::from_ref(&sidecar));
        registry.observe_member_event_row(&target, 1, 1, &run_completed("pinned"));
        for seq in 2..=302 {
            registry.observe_member_event_row(
                &target,
                1,
                seq,
                &run_completed(&format!("unrelated-{seq}")),
            );
        }

        registry
            .consume_turn_outcome_record_for_test(&target, &sidecar)
            .await
            .expect("pinned sidecar resolves");
        match rx.await.expect("pinned terminal ticket resolves") {
            FlowTurnOutcome::Completed { output, .. } => assert_eq!(output, "pinned"),
            other => panic!("expected pinned completed outcome, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mixed_incarnation_sidecar_cannot_advance_authenticated_page_fold() {
        let (registry, _resolver) = registry();
        let target = target();
        let rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));
        let current = record("input-1", 1, 1);
        let mixed_higher = record("other-input", 2, 2);

        registry.observe_member_generation(&target, 1);
        registry.observe_turn_outcome_page_for_test(&target, &[mixed_higher, current.clone()]);
        registry.observe_member_event_row(&target, 1, 1, &run_completed("current"));
        assert_eq!(
            registry
                .consume_turn_outcome_record_for_test(&target, &current)
                .await
                .expect("current-incarnation sidecar consumes"),
            OutcomeRecordDisposition::Acknowledge,
        );
        match rx.await.expect("current-incarnation ticket resolves") {
            FlowTurnOutcome::Completed { output, .. } => assert_eq!(output, "current"),
            other => panic!("expected current completed outcome, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn late_sidecar_after_256_terminal_eviction_replays_frozen_fold() {
        let (registry, _resolver) = registry();
        let target = target();
        let mut rx = registry.arm(&target, "input-1", &RunId::new(), &StepId::from("s1"));
        let sidecar = record("input-1", 1, 1);
        registry.observe_member_event_row(&target, 1, 1, &run_completed("replayed"));
        for seq in 2..=302 {
            registry.observe_member_event_row(
                &target,
                1,
                seq,
                &run_completed(&format!("unrelated-{seq}")),
            );
        }

        registry.observe_turn_outcome_page_for_test(&target, std::slice::from_ref(&sidecar));
        assert_eq!(
            registry
                .consume_turn_outcome_record_for_test(&target, &sidecar)
                .await
                .expect("late sidecar classification"),
            OutcomeRecordDisposition::Replay,
        );
        assert!(matches!(
            rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));

        registry.rewind_member_fold(&target, 1);
        registry.observe_member_event_row(&target, 1, 1, &run_completed("replayed"));
        assert_eq!(
            registry
                .consume_turn_outcome_record_for_test(&target, &sidecar)
                .await
                .expect("sidecar after fold replay"),
            OutcomeRecordDisposition::Acknowledge,
        );
        match rx.await.expect("replayed terminal resolves") {
            FlowTurnOutcome::Completed { output, .. } => assert_eq!(output, "replayed"),
            other => panic!("expected replayed completed outcome, got {other:?}"),
        }
    }
}
