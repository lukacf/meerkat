//! Controlling-side member event pump (multi-host mobs §7.4, DEC-P6E-9..13,
//! 18, 19).
//!
//! One sequential `PollMemberEvents` long-poll loop per (mob, placed member),
//! owned by [`MemberEventPumpManager`] beside the mob actor. Every pumped
//! page fans out to: (a) `AttributedEvent` taps (per-member and mob-wide
//! subscriptions, the router merge), (b) the flow lane's
//! [`RemoteFlowTicketRegistry`] via the [`RemoteTurnOutcomeSink`] seam
//! (rows FIRST, then the bounded retained-outcome sidecar — ADJ-P6-10 page
//! order), and
//! (c) [`RemoteCompletionWaiters`] keyed on the full host/member residency
//! plus `interaction_id`, resolved only from the fold-validated Interaction
//! sidecar for the `spawn_turn_completed_reply` upgrade (DEC-P6E-19).
//!
//! Pump lifecycle (A17, DEC-P6E-11): a member's pump RUNS while either an
//! event subscription is authorized (a live tap) OR a remote-turn
//! obligation / completion waiter / unconfirmed outcome ACK is outstanding;
//! it stops after an idle grace, at member removal, or at manager shutdown.
//! Pump-task death resumes from the durable cursor record (DEC-P6E-10 —
//! plain records, no witness permit; ADJ-P6-6).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use tokio::sync::{Notify, Semaphore, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use meerkat_contracts::wire::WireEventRow;
use meerkat_contracts::wire::supervisor_bridge::BRIDGE_TURN_OUTCOME_ACK_MAX;
use meerkat_core::event::{AgentEvent, EventEnvelope, EventSourceIdentity, StreamTruncationReason};
use meerkat_core::interaction::InteractionId;

use super::bridge_protocol::{
    BridgeCommand, BridgeEventCursor, BridgeHostRuntimeIncarnation, BridgeMemberEventsPage,
    BridgeMemberIncarnation, BridgePollEventsPayload, BridgeProtocolVersion, BridgeRejectionCause,
    BridgeTurnOutcomeAck, BridgeTurnOutcomeRecord,
};
use super::remote_flow_ticket::{OutcomeRecordDisposition, RemoteFlowTicketRegistry};
use crate::MobError;
use crate::event::AttributedEvent;
use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, ProfileName};
use crate::store::{MobMemberEventCursorRecord, MobRuntimeMetadataStore};

/// Poll request budget: `wait_ms` is at most 10s (250ms for custody) and
/// remains well below this 30s bridge timeout (DEC-P6E-24).
const POLL_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Server-side long-poll window requested per page.
const POLL_WAIT_MS: u32 = 10_000;
/// Custody/completion pumps use a short poll window. They retain a reserved
/// permit lane below, so a large observation-only fan-out cannot consume the
/// fixed completion budget solely in the semaphore queue.
const PRIORITY_POLL_WAIT_MS: u32 = 250;
/// Page row budget.
const POLL_MAX_ROWS: u32 = 256;
/// Independently acknowledged terminal rows requested per page.
const POLL_MAX_OUTCOMES: u32 = 8;
/// Idle grace before a pump with no taps/obligations stops (A17).
const PUMP_IDLE_STOP: Duration = Duration::from_secs(30);
/// Per-mob concurrent poll cap. V1 reserves two slots for machine custody /
/// completion waiters and lets observation-only long polls use the other six.
/// The sum stays at the fixed resource ceiling of eight; §12 still makes
/// outbound connection pooling / host-batched observation a later transport
/// optimization.
const MAX_CONCURRENT_MEMBER_POLLS: usize = 8;
const PRIORITY_MEMBER_POLL_PERMITS: usize = 2;
const OBSERVATION_MEMBER_POLL_PERMITS: usize =
    MAX_CONCURRENT_MEMBER_POLLS - PRIORITY_MEMBER_POLL_PERMITS;
/// Backoff bounds for unreachable/unavailable hosts.
const POLL_BACKOFF_FLOOR: Duration = Duration::from_secs(1);
const POLL_BACKOFF_CEIL: Duration = Duration::from_secs(30);
/// Tap capacity; a full tap drops THAT consumer, never blocks the pump.
const TAP_CAPACITY: usize = 256;
/// Typed failure reason resolved into completion waiters when their
/// member's pump stops before the remote turn completed: the awaiting
/// completion task holds the waiter registry's `Arc`, so an unresolved
/// sender can never drop on its own — every pump-stop path must fail its
/// waiters explicitly.
pub(crate) const PUMP_STOPPED_WAITER_REASON: &str =
    "member event pump stopped before the remote turn completed";
fn remote_cursor_successor(watermark: u64) -> Result<u64, MobError> {
    watermark.checked_add(1).ok_or_else(|| {
        MobError::Internal(
            "member event pump cannot advance beyond an exhausted remote event watermark"
                .to_string(),
        )
    })
}

/// Decode the bridge row without inventing sequence values. The durable
/// store sequence can contain gaps and is independent from `envelope.seq`.
fn into_durable_event_rows(
    events: Vec<WireEventRow>,
) -> impl Iterator<Item = (u64, EventEnvelope<AgentEvent>)> {
    events
        .into_iter()
        .map(|row| (row.durable_seq, row.envelope))
}

/// Validate one decoded page as an atomic unit before any fold, ACK,
/// fan-out, reachability, or cursor state is mutated. The remote member is an
/// authenticated authority, but malformed pagination must still fail closed:
/// accepting a valid prefix while advancing to an attacker-selected frontier
/// can make durable terminal evidence unrecoverable.
fn validate_member_events_page(
    requested_cursor: BridgeEventCursor,
    expected_member: &BridgeMemberIncarnation,
    page: &BridgeMemberEventsPage,
) -> Result<(), MobError> {
    if (page.generation, page.fence_token)
        != (expected_member.generation, expected_member.fence_token)
    {
        return Err(MobError::Internal(format!(
            "member events page changed exact incarnation: expected generation/fence {}/{}, got {}/{}",
            expected_member.generation,
            expected_member.fence_token,
            page.generation,
            page.fence_token
        )));
    }
    if page.events.len() > usize::try_from(POLL_MAX_ROWS).unwrap_or(usize::MAX)
        || page.turn_outcomes.len() > usize::try_from(POLL_MAX_OUTCOMES).unwrap_or(usize::MAX)
    {
        return Err(MobError::Internal(format!(
            "member events page exceeded requested bounds: rows={}/{POLL_MAX_ROWS}, outcomes={}/{POLL_MAX_OUTCOMES}",
            page.events.len(),
            page.turn_outcomes.len()
        )));
    }

    let requested_seq = match requested_cursor {
        BridgeEventCursor::At { generation, seq } => {
            if generation != page.generation {
                return Err(MobError::Internal(format!(
                    "member events page left requested generation domain {generation} for {}",
                    page.generation
                )));
            }
            seq
        }
        // The controller pump always owns an exact durable frontier. `Tail`
        // is intentionally unsupported here: an empty Tail page cannot prove
        // an exact successor and accepting its self-reported `next_seq` would
        // let a malformed peer skip retained terminal evidence.
        BridgeEventCursor::Tail => {
            return Err(MobError::Internal(
                "member event pump cannot validate an inexact Tail cursor".to_string(),
            ));
        }
    };
    if page.from_seq == 0 {
        return Err(MobError::Internal(
            "member events page resolved a zero durable-sequence floor".to_string(),
        ));
    }
    if page.from_seq < requested_seq {
        return Err(MobError::Internal(format!(
            "member events page resolved floor {} regressed below requested cursor {requested_seq}",
            page.from_seq
        )));
    }
    if page.from_seq > page.next_seq {
        return Err(MobError::Internal(format!(
            "member events page resolved floor {} is beyond frontier {}",
            page.from_seq, page.next_seq
        )));
    }
    let mut previous_seq = None;
    for row in &page.events {
        if row.durable_seq < page.from_seq {
            return Err(MobError::Internal(format!(
                "member events page regressed below resolved floor: row {}, floor {}",
                row.durable_seq, page.from_seq
            )));
        }
        if previous_seq.is_some_and(|previous| row.durable_seq <= previous) {
            return Err(MobError::Internal(format!(
                "member events page rows are not strictly increasing at {}",
                row.durable_seq
            )));
        }
        previous_seq = Some(row.durable_seq);
    }
    let expected_next = match previous_seq {
        Some(last) => last.checked_add(1).ok_or_else(|| {
            MobError::Internal(
                "member events page cannot advance beyond exhausted sequence space".to_string(),
            )
        })?,
        None => page.from_seq,
    };
    if page.next_seq != expected_next {
        return Err(MobError::Internal(format!(
            "member events page frontier {} does not equal exact successor {expected_next}",
            page.next_seq
        )));
    }
    let max_frontier = page.watermark.saturating_add(1);
    if page.next_seq > max_frontier || previous_seq.is_some_and(|last| last > page.watermark) {
        return Err(MobError::Internal(format!(
            "member events page frontier/watermark are inconsistent: next={}, watermark={}",
            page.next_seq, page.watermark
        )));
    }

    let mut outcome_keys = std::collections::BTreeSet::new();
    let mut terminal_seqs = std::collections::BTreeSet::new();
    for record in &page.turn_outcomes {
        if (record.generation, record.fence_token) != (page.generation, page.fence_token) {
            return Err(MobError::Internal(format!(
                "member events page mixed outcome incarnation for '{}'",
                record.input_id
            )));
        }
        let input_uuid = uuid::Uuid::parse_str(&record.input_id).map_err(|error| {
            MobError::Internal(format!(
                "member events page outcome input id '{}' is not a UUID: {error}",
                record.input_id
            ))
        })?;
        if input_uuid.is_nil() || input_uuid.to_string() != record.input_id {
            return Err(MobError::Internal(format!(
                "member events page outcome input id '{}' is not canonical and non-nil",
                record.input_id
            )));
        }
        if record.terminal_seq >= page.next_seq {
            return Err(MobError::Internal(format!(
                "member events page outcome '{}' points beyond frontier {}",
                record.input_id, page.next_seq
            )));
        }
        if !outcome_keys.insert((record.generation, record.input_id.clone()))
            || !terminal_seqs.insert(record.terminal_seq)
        {
            return Err(MobError::Internal(format!(
                "member events page contains duplicate outcome key/terminal for '{}'",
                record.input_id
            )));
        }
    }
    Ok(())
}

fn initial_member_cursor(
    records: Vec<MobMemberEventCursorRecord>,
    expected_member: &BridgeMemberIncarnation,
) -> BridgeEventCursor {
    records
        .into_iter()
        .find(|record| {
            record.agent_identity == expected_member.agent_identity
                && record.host_id == expected_member.host_id
                && record.host_binding_generation == Some(expected_member.binding_generation)
                && record.member_session_id == expected_member.member_session_id
                && record.generation == expected_member.generation
                && record.fence_token == Some(expected_member.fence_token)
        })
        .map_or(
            BridgeEventCursor::At {
                generation: expected_member.generation,
                seq: 1,
            },
            |record| BridgeEventCursor::At {
                generation: record.generation,
                seq: record.next_seq,
            },
        )
}

// ---------------------------------------------------------------------------
// Cross-lane seams
// ---------------------------------------------------------------------------

/// Page-shaped outcome sink (the events lane's seam name, DEC-P6E-18); the
/// flow lane's [`RemoteFlowTicketRegistry`] is the production impl. Page
/// contract: `observe_member_generation`, pre-pin the page sidecar via
/// `observe_turn_outcome_page`, then every row via `observe_member_event_row`
/// (durable-seq order), THEN the sidecar via `consume_turn_outcome_record`.
/// A typed oversized-row rejection advances the same fold through
/// `observe_omitted_member_event_row` before the next page delivers its
/// sidecar.
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub(crate) trait RemoteTurnOutcomeSink: Send + Sync {
    /// Install the one exact residency whose observations may mutate this
    /// member's outcome fold. Pump replacement calls this only after the old
    /// task has crossed its abort+join barrier.
    fn install_member_residency(&self, expected_member: &BridgeMemberIncarnation);
    fn observe_member_generation(&self, expected_member: &BridgeMemberIncarnation);
    fn rewind_member_fold(&self, expected_member: &BridgeMemberIncarnation);
    fn observe_turn_outcome_page(
        &self,
        expected_member: &BridgeMemberIncarnation,
        event_frontier_exclusive: u64,
        outcomes_complete: bool,
        records: &[BridgeTurnOutcomeRecord],
    );
    fn observe_member_event_row(
        &self,
        expected_member: &BridgeMemberIncarnation,
        seq: u64,
        event: &AgentEvent,
    );
    fn observe_omitted_member_event_row(&self, expected_member: &BridgeMemberIncarnation, seq: u64);
    async fn consume_turn_outcome_record(
        &self,
        expected_member: &BridgeMemberIncarnation,
        record: &BridgeTurnOutcomeRecord,
    ) -> Result<OutcomeRecordDisposition, MobError>;
    async fn acknowledge_turn_outcome(
        &self,
        expected_member: &BridgeMemberIncarnation,
        ack: &BridgeTurnOutcomeAck,
    ) -> Result<(), MobError>;
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl RemoteTurnOutcomeSink for RemoteFlowTicketRegistry {
    fn install_member_residency(&self, expected_member: &BridgeMemberIncarnation) {
        RemoteFlowTicketRegistry::install_member_residency(self, expected_member);
    }
    fn observe_member_generation(&self, expected_member: &BridgeMemberIncarnation) {
        RemoteFlowTicketRegistry::observe_member_generation_exact(self, expected_member);
    }
    fn rewind_member_fold(&self, expected_member: &BridgeMemberIncarnation) {
        RemoteFlowTicketRegistry::rewind_member_fold_exact(self, expected_member);
    }
    fn observe_member_event_row(
        &self,
        expected_member: &BridgeMemberIncarnation,
        seq: u64,
        event: &AgentEvent,
    ) {
        RemoteFlowTicketRegistry::observe_member_event_row_exact(self, expected_member, seq, event);
    }
    fn observe_omitted_member_event_row(
        &self,
        expected_member: &BridgeMemberIncarnation,
        seq: u64,
    ) {
        RemoteFlowTicketRegistry::observe_omitted_member_event_row_exact(
            self,
            expected_member,
            seq,
        );
    }
    fn observe_turn_outcome_page(
        &self,
        expected_member: &BridgeMemberIncarnation,
        event_frontier_exclusive: u64,
        outcomes_complete: bool,
        records: &[BridgeTurnOutcomeRecord],
    ) {
        RemoteFlowTicketRegistry::observe_turn_outcome_page(
            self,
            expected_member,
            event_frontier_exclusive,
            outcomes_complete,
            records,
        );
    }
    async fn consume_turn_outcome_record(
        &self,
        expected_member: &BridgeMemberIncarnation,
        record: &BridgeTurnOutcomeRecord,
    ) -> Result<OutcomeRecordDisposition, MobError> {
        RemoteFlowTicketRegistry::consume_turn_outcome_record(self, expected_member, record).await
    }
    async fn acknowledge_turn_outcome(
        &self,
        expected_member: &BridgeMemberIncarnation,
        ack: &BridgeTurnOutcomeAck,
    ) -> Result<(), MobError> {
        RemoteFlowTicketRegistry::acknowledge_turn_outcome(self, expected_member, ack).await
    }
}

// ---------------------------------------------------------------------------
// Remote completion waiters (DEC-P6E-19)
// ---------------------------------------------------------------------------

/// Terminal fact resolved for one awaited remote interaction.
#[derive(Debug, Clone)]
pub(crate) enum RemoteInteractionTerminal {
    Complete,
    CallbackPending {
        tool_name: Option<String>,
    },
    Failed {
        reason: String,
    },
    /// Durable proof that the tracked input has no remaining terminal
    /// custody. This is control-plane closure, not a synthesized member
    /// `InteractionFailed` terminal and not pump infrastructure loss.
    Closed {
        closure: crate::event::PlacedCompletionClosureEvent,
    },
    /// Exact release/disposal proof won the member lifecycle race before a
    /// turn terminal. Distinct from member failure and pump loss.
    Disposed,
}

/// Waiter registry keyed by the full placed residency plus interaction id:
/// the member-side input's `InteractionId` is the caller-minted stable payload
/// input id, so delivery retry/dedup cannot change the pumped terminal's
/// correlation. A registered waiter is an A17 keep-alive condition on its
/// member's pump, and the exact fold-validated Interaction sidecar resolves
/// it before entering the ACK chain — completion never depends on a console
/// watching. Flow steps do NOT ride this registry (their terminal is the O2
/// journal sidecar).
/// Recently-resolved terminal memory remains a defensive replay aid. The
/// production path registers before sending and therefore does not depend on
/// this bounded cache for correctness.
const RESOLVED_TERMINAL_MEMORY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct MemberResidencyKey {
    mob_id: String,
    agent_identity: String,
    host_id: String,
    binding_generation: u64,
    member_session_id: String,
    generation: u64,
    fence_token: u64,
}

impl From<&BridgeMemberIncarnation> for MemberResidencyKey {
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct WaiterKey {
    residency: MemberResidencyKey,
    interaction_id: uuid::Uuid,
}

#[derive(Default)]
struct WaiterState {
    waiters: HashMap<WaiterKey, Vec<oneshot::Sender<RemoteInteractionTerminal>>>,
    resolved: HashMap<WaiterKey, RemoteInteractionTerminal>,
    resolved_order: VecDeque<WaiterKey>,
}

#[derive(Default)]
pub(crate) struct RemoteCompletionWaiters {
    state: StdMutex<WaiterState>,
}

impl RemoteCompletionWaiters {
    pub(crate) fn register(
        &self,
        expected_member: &BridgeMemberIncarnation,
        interaction_id: InteractionId,
    ) -> oneshot::Receiver<RemoteInteractionTerminal> {
        let (tx, rx) = oneshot::channel();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = WaiterKey {
            residency: expected_member.into(),
            interaction_id: interaction_id.0,
        };
        if let Some(terminal) = state.resolved.get(&key).cloned() {
            let _ = tx.send(terminal);
            return rx;
        }
        state.waiters.entry(key).or_default().push(tx);
        rx
    }

    fn outstanding_for(&self, expected_member: &BridgeMemberIncarnation) -> bool {
        let expected = MemberResidencyKey::from(expected_member);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.waiters.retain(|_, senders| {
            senders.retain(|sender| !sender.is_closed());
            !senders.is_empty()
        });
        state.waiters.keys().any(|key| key.residency == expected)
    }

    fn outstanding_exact(&self, expected_member: &BridgeMemberIncarnation, input_id: &str) -> bool {
        let Ok(interaction_id) = uuid::Uuid::parse_str(input_id) else {
            return true;
        };
        let key = WaiterKey {
            residency: expected_member.into(),
            interaction_id,
        };
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.waiters.retain(|_, senders| {
            senders.retain(|sender| !sender.is_closed());
            !senders.is_empty()
        });
        state.waiters.contains_key(&key)
    }

    fn resolve(
        &self,
        expected_member: &BridgeMemberIncarnation,
        interaction_id: InteractionId,
        terminal: &RemoteInteractionTerminal,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = WaiterKey {
            residency: expected_member.into(),
            interaction_id: interaction_id.0,
        };
        if let Some(senders) = state.waiters.remove(&key) {
            for sender in senders {
                let _ = sender.send(terminal.clone());
            }
        }
        if !state.resolved.contains_key(&key) {
            state.resolved.insert(key.clone(), terminal.clone());
            state.resolved_order.push_back(key);
            while state.resolved_order.len() > RESOLVED_TERMINAL_MEMORY {
                if let Some(evicted) = state.resolved_order.pop_front() {
                    state.resolved.remove(&evicted);
                }
            }
        }
    }

    /// Close every waiter registered for the exact residency without
    /// fabricating an interaction terminal. Invoked from every pump-stop path
    /// (eagerly at `stop_pump`/`stop_all`, and unconditionally at pump-task
    /// exit): receiver `Err` is infrastructure/custody loss, while
    /// `RemoteInteractionTerminal::Failed` is reserved exclusively for a
    /// fold-validated member `InteractionFailed` sidecar.
    pub(crate) fn fail_all_for(&self, expected_member: &BridgeMemberIncarnation) {
        let expected = MemberResidencyKey::from(expected_member);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let keys: Vec<WaiterKey> = state
            .waiters
            .keys()
            .filter(|key| key.residency == expected)
            .cloned()
            .collect();
        for key in keys {
            state.waiters.remove(&key);
        }
    }
}

/// Remote completion context handed to the SubmitWork completion task
/// (DEC-P6E-19): the waiter registry plus the awaited incarnation.
#[derive(Clone)]
pub(crate) struct RemoteCompletionContext {
    manager: Arc<MemberEventPumpManager>,
    agent_identity: AgentIdentity,
    expected_member: BridgeMemberIncarnation,
}

impl RemoteCompletionContext {
    pub(crate) fn register(
        &self,
        interaction_id: InteractionId,
    ) -> Result<oneshot::Receiver<RemoteInteractionTerminal>, &'static str> {
        self.manager.register_completion_waiter_for_active_pump(
            &self.agent_identity,
            &self.expected_member,
            interaction_id,
        )
    }
}

/// A17 obligation liveness probe: does the MACHINE (`pending_remote_turn_
/// outcomes`) still hold an outstanding remote-turn obligation for this
/// member? The pump must keep polling until the obligation RESOLVES — an
/// empty sidecar page only means the outcome has not landed yet, never
/// that the obligation lapsed.
pub(crate) trait RemoteObligationProbe: Send + Sync {
    fn has_pending_obligation(&self, expected_member: &BridgeMemberIncarnation) -> bool;
    fn resolved_outcome_acks(
        &self,
        expected_member: &BridgeMemberIncarnation,
    ) -> Vec<BridgeTurnOutcomeAck>;
}

/// Production probe: watch-read over the mob machine state.
pub(crate) struct HandleObligationProbe {
    pub(crate) handle: super::handle::MobHandle,
}

/// Actor-serialized boot-incarnation barrier. A successful remote page is
/// not reachability evidence until the MobMachine-owned route intent has
/// been re-realized for the host process that served it.
#[async_trait]
pub(crate) trait HostRuntimeIncarnationObserver: Send + Sync {
    async fn observe(
        &self,
        expected_member: &BridgeMemberIncarnation,
        runtime_incarnation: BridgeHostRuntimeIncarnation,
    ) -> Result<(), MobError>;
}

pub(crate) struct HandleHostRuntimeIncarnationObserver {
    pub(crate) handle: super::handle::MobHandle,
}

#[async_trait]
impl HostRuntimeIncarnationObserver for HandleHostRuntimeIncarnationObserver {
    async fn observe(
        &self,
        expected_member: &BridgeMemberIncarnation,
        runtime_incarnation: BridgeHostRuntimeIncarnation,
    ) -> Result<(), MobError> {
        self.handle
            .observe_host_runtime_incarnation(expected_member.clone(), runtime_incarnation)
            .await
    }
}

impl RemoteObligationProbe for HandleObligationProbe {
    fn has_pending_obligation(&self, expected_member: &BridgeMemberIncarnation) -> bool {
        self.handle
            .remote_turn_obligation_for_member_observation(expected_member)
    }

    fn resolved_outcome_acks(
        &self,
        expected_member: &BridgeMemberIncarnation,
    ) -> Vec<BridgeTurnOutcomeAck> {
        self.handle
            .remote_turn_ack_pending_observation(expected_member)
    }
}

/// The flow lane's A17 pump-liveness hook (`RemoteTurnObligationPump`),
/// realized over the actor's machine-fact material derivation: the poke is
/// a routed internal command (the executor's dispatch runs on the flow
/// task; the derive needs actor state).
pub(crate) struct HandleObligationPump {
    pub(crate) handle: super::handle::MobHandle,
}

impl super::remote_flow_ticket::RemoteTurnObligationPump for HandleObligationPump {
    fn ensure_pump_for_obligation(&self, identity: &AgentIdentity) {
        let handle = self.handle.clone();
        let identity = identity.clone();
        tokio::spawn(async move {
            if let Err(error) = handle.ensure_pump_for_obligation(&identity).await {
                tracing::warn!(
                    agent_identity = %identity,
                    error = %error,
                    "obligation pump-liveness poke failed; the step resolves via the timeout ladder"
                );
            }
        });
    }
}

/// Project an authenticated interaction sidecar onto the volatile completion
/// waiter vocabulary. The pump path calls this only after fold + durable
/// machine Resolve; the tracked-cancel alternate calls it after its exact
/// authenticated record + durable Resolve. Successful/callback records carry
/// no output because the public promise needs only terminality. Failed detail
/// retains the host's explicit truncation diagnostic.
fn interaction_terminal_from_record(
    record: &BridgeTurnOutcomeRecord,
) -> Result<Option<(InteractionId, RemoteInteractionTerminal)>, MobError> {
    let terminal = match &record.outcome {
        super::bridge_protocol::WireFlowTurnOutcome::InteractionComplete => {
            RemoteInteractionTerminal::Complete
        }
        super::bridge_protocol::WireFlowTurnOutcome::InteractionCallbackPending => {
            RemoteInteractionTerminal::CallbackPending { tool_name: None }
        }
        super::bridge_protocol::WireFlowTurnOutcome::InteractionFailed { detail } => {
            let reason = if detail.truncated {
                format!(
                    "{} [remote failure detail truncated from {} UTF-8 bytes]",
                    detail.text, detail.original_utf8_bytes
                )
            } else {
                detail.text.clone()
            };
            RemoteInteractionTerminal::Failed { reason }
        }
        // Flow terminals have no completion waiter. Once the shared fold has
        // validated them and the machine explicitly reports Absent, their
        // unclaimed disposition is still ACK-safe without UUID correlation.
        _ => return Ok(None),
    };
    let parsed = uuid::Uuid::parse_str(&record.input_id).map_err(|error| {
        MobError::Internal(format!(
            "validated interaction turn outcome has invalid input id '{}': {error}",
            record.input_id
        ))
    })?;
    if parsed.is_nil() || parsed.to_string() != record.input_id {
        return Err(MobError::Internal(format!(
            "validated interaction turn outcome input id '{}' is not canonical and non-nil",
            record.input_id
        )));
    }
    let interaction_id = InteractionId(parsed);
    Ok(Some((interaction_id, terminal)))
}

fn resolve_interaction_waiter_from_record(
    waiters: &RemoteCompletionWaiters,
    expected_member: &BridgeMemberIncarnation,
    record: &BridgeTurnOutcomeRecord,
) -> Result<(), MobError> {
    if let Some((interaction_id, terminal)) = interaction_terminal_from_record(record)? {
        waiters.resolve(expected_member, interaction_id, &terminal);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Pump material + manager
// ---------------------------------------------------------------------------

/// Static per-incarnation pump material, derived by the ACTOR from
/// machine-recorded facts + the roster's shell transport composition
/// (DEC-P6E-9: the placed-delivery peer derivation).
#[derive(Clone)]
pub(crate) struct MemberPumpMaterial {
    pub agent_identity: AgentIdentity,
    /// Authoritative MobMachine placement owner. Never inferred from peer
    /// transport identity/address.
    pub host_id: String,
    pub expected_member: super::bridge_protocol::BridgeMemberIncarnation,
    pub runtime_id: AgentRuntimeId,
    pub fence_token: FenceToken,
    pub role: ProfileName,
    pub peer: meerkat_core::comms::TrustedPeerDescriptor,
}

struct PumpEntry {
    cancel: CancellationToken,
    /// Unique pump incarnation (a respawn replacement re-inserts under the
    /// same identity; the dying task must only remove ITS entry).
    incarnation: u64,
    /// Full host/member residency authority. No subset of this tuple is a
    /// valid pump, waiter, cursor, fold, fan-out, or ACK boundary.
    expected_member: BridgeMemberIncarnation,
    /// The task has left its polling loop and is waiting to publish its exit
    /// under `pump_transition`. An exiting entry is not a live pump lease:
    /// an ensure racing that final barrier must replace it instead of
    /// returning success just before the old task removes the entry.
    exiting: bool,
    runtime_id: AgentRuntimeId,
    peer: meerkat_core::comms::TrustedPeerDescriptor,
    /// Live event taps; closed receivers are pruned on send.
    taps: Vec<mpsc::Sender<AttributedEvent>>,
    /// Explicit obligation keep-alive (A17): set by
    /// `ensure_pump_for_obligation`, cleared when the machine's
    /// `pending_remote_turn_outcomes` re-derivation says so at resume; the
    /// per-page idle check consults the sink-lane obligation count via this
    /// flag plus registered completion waiters.
    obligation_keepalive: bool,
}

struct ManagerState {
    pumps: BTreeMap<AgentIdentity, PumpEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemberPollClass {
    /// Machine remote-turn/kickoff custody or a completion waiter is live.
    Priority,
    /// Event observation taps only.
    Observation,
}

/// Owns every pump task for one mob (DEC-P6E-11). Handle owned by the
/// `MobActor`; all pump tasks are detached — the actor loop never awaits a
/// bridge round-trip (ADJ-P4-12).
pub(crate) struct MemberEventPumpManager {
    mob_id: crate::MobId,
    bridge: Arc<super::MobSupervisorBridge>,
    store: Arc<dyn MobRuntimeMetadataStore>,
    host_runtime_observer: Arc<dyn HostRuntimeIncarnationObserver>,
    sink: OnceLock<Arc<dyn RemoteTurnOutcomeSink>>,
    obligation_probe: OnceLock<Arc<dyn RemoteObligationProbe>>,
    reachability_observations: OnceLock<Arc<super::handle::ReachabilityObservations>>,
    waiters: Arc<RemoteCompletionWaiters>,
    priority_poll_permits: Arc<Semaphore>,
    observation_poll_permits: Arc<Semaphore>,
    /// One weak one-permit lane per authoritative host and global poll class.
    /// Every pump crosses its host lane before queuing for the corresponding
    /// global permits, so an arbitrarily large blackholed fan-out on one host
    /// contributes at most one global contender in either class. Weak entries
    /// are pruned by the pump loop and do not retain departed host lifecycles.
    priority_host_poll_lanes: StdMutex<BTreeMap<String, Weak<Semaphore>>>,
    observation_host_poll_lanes: StdMutex<BTreeMap<String, Weak<Semaphore>>>,
    /// Serializes pump install/replacement/stop barriers. A replacement is
    /// not published until the previous task has been aborted and joined.
    pump_transition: tokio::sync::Mutex<()>,
    state: StdMutex<ManagerState>,
    /// Every spawned pump remains join-owned until it has completed or the
    /// manager's shutdown barrier aborts and joins it. Keeping handles outside
    /// `PumpEntry` lets a pump remove its own current-incarnation entry without
    /// detaching its task handle.
    pump_tasks: StdMutex<BTreeMap<u64, tokio::task::JoinHandle<()>>>,
    /// Closed by the actor-exit barrier before it drains state/tasks. All
    /// production pump creation is actor-serialized, but this guard makes the
    /// ownership rule explicit and prevents a future cross-task caller from
    /// racing a shutdown into a newly detached pump.
    accepting_pumps: std::sync::atomic::AtomicBool,
    /// Wakes idling pumps when taps/waiters/obligations change.
    liveness_changed: Arc<Notify>,
    next_incarnation: std::sync::atomic::AtomicU64,
}

impl MemberEventPumpManager {
    pub(crate) fn new(
        mob_id: crate::MobId,
        bridge: Arc<super::MobSupervisorBridge>,
        store: Arc<dyn MobRuntimeMetadataStore>,
        host_runtime_observer: Arc<dyn HostRuntimeIncarnationObserver>,
    ) -> Self {
        Self {
            mob_id,
            bridge,
            store,
            host_runtime_observer,
            sink: OnceLock::new(),
            obligation_probe: OnceLock::new(),
            reachability_observations: OnceLock::new(),
            waiters: Arc::new(RemoteCompletionWaiters::default()),
            priority_poll_permits: Arc::new(Semaphore::new(PRIORITY_MEMBER_POLL_PERMITS)),
            observation_poll_permits: Arc::new(Semaphore::new(OBSERVATION_MEMBER_POLL_PERMITS)),
            priority_host_poll_lanes: StdMutex::new(BTreeMap::new()),
            observation_host_poll_lanes: StdMutex::new(BTreeMap::new()),
            pump_transition: tokio::sync::Mutex::new(()),
            state: StdMutex::new(ManagerState {
                pumps: BTreeMap::new(),
            }),
            pump_tasks: StdMutex::new(BTreeMap::new()),
            accepting_pumps: std::sync::atomic::AtomicBool::new(true),
            liveness_changed: Arc::new(Notify::new()),
            next_incarnation: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Install the flow lane's outcome sink (one registry per mob; the
    /// first install wins).
    pub(crate) fn install_outcome_sink(&self, sink: Arc<dyn RemoteTurnOutcomeSink>) {
        if self.sink.set(Arc::clone(&sink)).is_ok() {
            let residencies = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pumps
                .values()
                .map(|entry| entry.expected_member.clone())
                .collect::<Vec<_>>();
            for residency in residencies {
                sink.install_member_residency(&residency);
            }
        }
    }

    /// Install the machine-fact obligation probe (A17 liveness truth; the
    /// first install wins).
    pub(crate) fn install_obligation_probe(&self, probe: Arc<dyn RemoteObligationProbe>) {
        let _ = self.obligation_probe.set(probe);
    }

    pub(crate) fn install_reachability_observations(
        &self,
        observations: Arc<super::handle::ReachabilityObservations>,
    ) {
        let _ = self.reachability_observations.set(observations);
    }

    fn has_pending_obligation(&self, expected_member: &BridgeMemberIncarnation) -> bool {
        self.obligation_probe
            .get()
            .is_some_and(|probe| probe.has_pending_obligation(expected_member))
    }

    /// Cursor advancement is stricter than pump liveness. Observation taps
    /// may let their durable cursor advance, but an exact completion waiter
    /// must retain the event-fold checkpoint just like machine custody until
    /// its validated sidecar has become a local ACK obligation.
    fn has_cursor_custody(
        &self,
        expected_member: &BridgeMemberIncarnation,
        pending_outcome_acks: &[BridgeTurnOutcomeAck],
    ) -> bool {
        !pending_outcome_acks.is_empty()
            || self.has_pending_obligation(expected_member)
            || self.waiters.outstanding_for(expected_member)
    }

    fn resolved_outcome_acks(
        &self,
        expected_member: &BridgeMemberIncarnation,
    ) -> Vec<BridgeTurnOutcomeAck> {
        self.obligation_probe.get().map_or_else(Vec::new, |probe| {
            probe
                .resolved_outcome_acks(expected_member)
                .into_iter()
                .filter(|ack| {
                    !self
                        .waiters
                        .outstanding_exact(expected_member, &ack.input_id)
                })
                .collect()
        })
    }

    /// Select the bounded poll lane from persistent liveness facts. A pump
    /// waiting in the observation semaphore is woken on any liveness change
    /// and re-runs this classification before it can send.
    fn poll_class(
        &self,
        identity: &AgentIdentity,
        expected_member: &BridgeMemberIncarnation,
    ) -> MemberPollClass {
        let local_priority = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.pumps.get(identity).is_some_and(|entry| {
                !entry.exiting
                    && &entry.expected_member == expected_member
                    && (entry.obligation_keepalive || self.waiters.outstanding_for(expected_member))
            })
        };
        if local_priority || self.has_pending_obligation(expected_member) {
            MemberPollClass::Priority
        } else {
            MemberPollClass::Observation
        }
    }

    fn poll_lane(&self, class: MemberPollClass) -> (Arc<Semaphore>, u32) {
        match class {
            MemberPollClass::Priority => (
                Arc::clone(&self.priority_poll_permits),
                PRIORITY_POLL_WAIT_MS,
            ),
            MemberPollClass::Observation => {
                (Arc::clone(&self.observation_poll_permits), POLL_WAIT_MS)
            }
        }
    }

    fn host_poll_lane(&self, class: MemberPollClass, host_id: &str) -> Arc<Semaphore> {
        let lanes = match class {
            MemberPollClass::Priority => &self.priority_host_poll_lanes,
            MemberPollClass::Observation => &self.observation_host_poll_lanes,
        };
        let mut lanes = lanes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        lanes.retain(|_, lane| lane.strong_count() > 0);
        if let Some(lane) = lanes.get(host_id).and_then(Weak::upgrade) {
            return lane;
        }
        let lane = Arc::new(Semaphore::new(1));
        lanes.insert(host_id.to_string(), Arc::downgrade(&lane));
        lane
    }

    fn prune_host_poll_lanes(&self) {
        for lanes in [
            &self.priority_host_poll_lanes,
            &self.observation_host_poll_lanes,
        ] {
            lanes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|_, lane| lane.strong_count() > 0);
        }
    }

    pub(crate) fn completion_waiters(&self) -> Arc<RemoteCompletionWaiters> {
        Arc::clone(&self.waiters)
    }

    pub(crate) fn resolve_placed_completion_waiter_closed(
        &self,
        expected_member: &BridgeMemberIncarnation,
        input_id: &str,
        closure: crate::event::PlacedCompletionClosureEvent,
    ) -> Result<(), MobError> {
        let parsed = uuid::Uuid::parse_str(input_id).map_err(|error| {
            MobError::Internal(format!(
                "placed completion closure has invalid input id '{input_id}': {error}"
            ))
        })?;
        if parsed.is_nil() || parsed.to_string() != input_id {
            return Err(MobError::Internal(format!(
                "placed completion closure input id '{input_id}' is not canonical and non-nil"
            )));
        }
        self.waiters.resolve(
            expected_member,
            InteractionId(parsed),
            &RemoteInteractionTerminal::Closed { closure },
        );
        Ok(())
    }

    pub(crate) fn resolve_placed_completion_waiter_disposed(
        &self,
        expected_member: &BridgeMemberIncarnation,
        input_id: &str,
    ) -> Result<(), MobError> {
        let parsed = uuid::Uuid::parse_str(input_id).map_err(|error| {
            MobError::Internal(format!(
                "disposed placed completion has invalid input id '{input_id}': {error}"
            ))
        })?;
        if parsed.is_nil() || parsed.to_string() != input_id {
            return Err(MobError::Internal(format!(
                "disposed placed completion input id '{input_id}' is not canonical and non-nil"
            )));
        }
        self.waiters.resolve(
            expected_member,
            InteractionId(parsed),
            &RemoteInteractionTerminal::Disposed,
        );
        Ok(())
    }

    /// Fan out an authenticated placed-completion terminal only after its
    /// machine Resolve commit. The ordinary pump path has already validated
    /// the event fold; the exact tracked-cancel Terminal response is the one
    /// alternate wire-authorized source of the same retained record.
    pub(crate) fn resolve_placed_completion_waiter(
        &self,
        expected_member: &BridgeMemberIncarnation,
        record: &BridgeTurnOutcomeRecord,
    ) -> Result<(), MobError> {
        resolve_interaction_waiter_from_record(&self.waiters, expected_member, record)
    }

    fn reap_finished_pump_tasks(&self) {
        self.pump_tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|_, task| !task.is_finished());
    }

    /// Capture a completion registration lease for the exact active pump
    /// tuple. The detached SubmitWork task registers through this context
    /// before the remote send, closing the fast-terminal race.
    pub(crate) fn remote_completion_context(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_member: &BridgeMemberIncarnation,
    ) -> Option<RemoteCompletionContext> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = state.pumps.get(identity)?;
        if entry.exiting || &entry.expected_member != expected_member {
            return None;
        }
        Some(RemoteCompletionContext {
            manager: Arc::clone(self),
            agent_identity: identity.clone(),
            expected_member: expected_member.clone(),
        })
    }

    /// Atomically validate the pre-send pump lease and register its waiter
    /// while holding the pump-state lock. Pump exit/removal uses the same
    /// lock, so registration either joins a live exact tuple or fails promptly;
    /// it can never land just after that tuple's final fail sweep.
    fn register_completion_waiter_for_active_pump(
        &self,
        identity: &AgentIdentity,
        expected_member: &BridgeMemberIncarnation,
        interaction_id: InteractionId,
    ) -> Result<oneshot::Receiver<RemoteInteractionTerminal>, &'static str> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = state.pumps.get(identity) else {
            return Err("member event pump stopped before completion waiter registration");
        };
        if entry.exiting {
            return Err("member event pump stopped before completion waiter registration");
        }
        if &entry.expected_member != expected_member {
            return Err(
                "member event pump incarnation changed before completion waiter registration",
            );
        }
        let receiver = self.waiters.register(expected_member, interaction_id);
        drop(state);
        self.liveness_changed.notify_waiters();
        Ok(receiver)
    }

    /// Old task exit transfers exact-tuple waiters to a replacement pump
    /// (for example an address-only refresh). If no matching replacement is
    /// current, fail only the exiting tuple's waiters while state remains
    /// locked against delayed registration/new installation.
    fn fail_waiters_if_no_matching_pump(
        &self,
        identity: &AgentIdentity,
        expected_member: &BridgeMemberIncarnation,
    ) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let transferred = state
            .pumps
            .get(identity)
            .is_some_and(|entry| !entry.exiting && &entry.expected_member == expected_member);
        if !transferred {
            self.waiters.fail_all_for(expected_member);
        }
        drop(state);
    }

    /// Ensure a pump runs for `material`'s member (idempotent; a changed
    /// exact `(runtime id, fence)` — respawn or same-generation fencing —
    /// replaces the pump so attribution follows the new incarnation).
    pub(crate) async fn ensure_pump(self: &Arc<Self>, material: MemberPumpMaterial) {
        let _transition = self.pump_transition.lock().await;
        self.reap_finished_pump_tasks();
        if !self
            .accepting_pumps
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        let replaced = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(existing) = state.pumps.get_mut(&material.agent_identity)
                && !existing.exiting
                && existing.expected_member == material.expected_member
                && existing.runtime_id == material.runtime_id
                && existing.peer == material.peer
            {
                // `ensure_pump` is the obligation/completion lane (tap
                // subscriptions use `ensure_pump_with_tap`). Publish its
                // keep-alive in the same state-lock linearization as the
                // idempotent decision, so an idle-stop decision cannot land
                // in the gap before the actor's explicit liveness poke.
                existing.obligation_keepalive = true;
                self.liveness_changed.notify_waiters();
                return;
            }
            state.pumps.remove(&material.agent_identity)
        };
        let (mut inherited_taps, rewind_same_residency) = if let Some(entry) = replaced {
            entry.cancel.cancel();
            let task = self
                .pump_tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&entry.incarnation);
            if let Some(task) = task {
                task.abort();
                let _ = task.await;
            }
            if entry.expected_member == material.expected_member {
                (entry.taps, true)
            } else {
                self.waiters.fail_all_for(&entry.expected_member);
                (Vec::new(), false)
            }
        } else {
            (Vec::new(), false)
        };
        inherited_taps.retain(|tap| !tap.is_closed());
        if !self
            .accepting_pumps
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.waiters.fail_all_for(&material.expected_member);
            return;
        }
        if let Some(sink) = self.sink.get() {
            sink.install_member_residency(&material.expected_member);
            if rewind_same_residency {
                // A same-residency replacement (most commonly an address
                // refresh) reloads the frozen durable cursor. Rewind the
                // rebuildable fold to that same checkpoint before the new
                // task replays rows; retaining the old live fold would feed
                // duplicate rows through the stateful classifier.
                sink.rewind_member_fold(&material.expected_member);
            }
        }
        let cancel = CancellationToken::new();
        let incarnation = self
            .next_incarnation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.pumps.insert(
                material.agent_identity.clone(),
                PumpEntry {
                    cancel: cancel.clone(),
                    incarnation,
                    expected_member: material.expected_member.clone(),
                    exiting: false,
                    runtime_id: material.runtime_id.clone(),
                    peer: material.peer.clone(),
                    taps: inherited_taps,
                    // `ensure_pump` is the obligation/completion lane. Keep
                    // the fresh task polling immediately; the per-page
                    // machine probe clears this once no custody remains.
                    obligation_keepalive: true,
                },
            );
        }
        let manager = Arc::clone(self);
        let mut pump_tasks = self
            .pump_tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self
            .accepting_pumps
            .load(std::sync::atomic::Ordering::Acquire)
        {
            cancel.cancel();
            drop(pump_tasks);
            self.remove_entry(&material.agent_identity, incarnation);
            self.waiters.fail_all_for(&material.expected_member);
            return;
        }
        let task = tokio::spawn(async move {
            run_member_pump(manager, material, cancel, incarnation).await;
        });
        pump_tasks.insert(incarnation, task);
    }

    /// Whether a pump entry currently exists for `identity` (any liveness).
    pub(crate) fn pump_exists(&self, identity: &AgentIdentity) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pumps
            .contains_key(identity)
    }

    /// A17 pump-liveness poke (the flow lane's named hook realization).
    pub(crate) fn mark_obligation_keepalive(&self, identity: &AgentIdentity) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = state.pumps.get_mut(identity) {
            entry.obligation_keepalive = true;
        }
        drop(state);
        self.liveness_changed.notify_waiters();
    }

    /// Subscribe a tap of `AttributedEvent`s for one member (DEC-P6E-12).
    /// Returns `None` when no pump is running (caller ensures first).
    pub(crate) fn subscribe_tap(
        &self,
        identity: &AgentIdentity,
    ) -> Option<mpsc::Receiver<AttributedEvent>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let entry = state.pumps.get_mut(identity)?;
        if entry.exiting {
            return None;
        }
        let (tx, rx) = mpsc::channel(TAP_CAPACITY);
        entry.taps.push(tx);
        drop(state);
        self.liveness_changed.notify_waiters();
        Some(rx)
    }

    /// Ensure a pump AND open a tap in ONE state transition: a tap opened
    /// after a separate ensure can miss the fresh pump's first pages (the
    /// loop starts polling as soon as it spawns). Respawn replacement
    /// matches [`Self::ensure_pump`].
    pub(crate) async fn ensure_pump_with_tap(
        self: &Arc<Self>,
        material: MemberPumpMaterial,
    ) -> mpsc::Receiver<AttributedEvent> {
        let (tx, rx) = mpsc::channel(TAP_CAPACITY);
        let _transition = self.pump_transition.lock().await;
        self.reap_finished_pump_tasks();
        if !self
            .accepting_pumps
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return rx;
        }
        let replaced = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(existing) = state.pumps.get_mut(&material.agent_identity)
                && !existing.exiting
                && existing.expected_member == material.expected_member
                && existing.runtime_id == material.runtime_id
                && existing.peer == material.peer
            {
                existing.taps.push(tx);
                drop(state);
                self.liveness_changed.notify_waiters();
                return rx;
            }
            state.pumps.remove(&material.agent_identity)
        };
        let (mut inherited_taps, inherited_keepalive, rewind_same_residency) =
            if let Some(entry) = replaced {
                entry.cancel.cancel();
                let task = self
                    .pump_tasks
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&entry.incarnation);
                if let Some(task) = task {
                    task.abort();
                    let _ = task.await;
                }
                if entry.expected_member == material.expected_member {
                    (entry.taps, entry.obligation_keepalive, true)
                } else {
                    self.waiters.fail_all_for(&entry.expected_member);
                    (Vec::new(), false, false)
                }
            } else {
                (Vec::new(), false, false)
            };
        inherited_taps.retain(|tap| !tap.is_closed());
        inherited_taps.push(tx);
        if !self
            .accepting_pumps
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.waiters.fail_all_for(&material.expected_member);
            return rx;
        }
        if let Some(sink) = self.sink.get() {
            sink.install_member_residency(&material.expected_member);
            if rewind_same_residency {
                sink.rewind_member_fold(&material.expected_member);
            }
        }
        let cancel = CancellationToken::new();
        let incarnation = self
            .next_incarnation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.pumps.insert(
                material.agent_identity.clone(),
                PumpEntry {
                    cancel: cancel.clone(),
                    incarnation,
                    expected_member: material.expected_member.clone(),
                    exiting: false,
                    runtime_id: material.runtime_id.clone(),
                    peer: material.peer.clone(),
                    taps: inherited_taps,
                    obligation_keepalive: inherited_keepalive,
                },
            );
        }
        let manager = Arc::clone(self);
        let mut pump_tasks = self
            .pump_tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self
            .accepting_pumps
            .load(std::sync::atomic::Ordering::Acquire)
        {
            cancel.cancel();
            drop(pump_tasks);
            self.remove_entry(&material.agent_identity, incarnation);
            self.waiters.fail_all_for(&material.expected_member);
            return rx;
        }
        let task = tokio::spawn(async move {
            run_member_pump(manager, material, cancel, incarnation).await;
        });
        pump_tasks.insert(incarnation, task);
        rx
    }

    /// Stop one member's pump (retire/release/destroy realization) and
    /// delete its durable cursor.
    pub(crate) async fn stop_pump(&self, identity: &AgentIdentity) {
        let _transition = self.pump_transition.lock().await;
        let entry = {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pumps
                .remove(identity)
        };
        if let Some(entry) = entry {
            entry.cancel.cancel();
            // Fail waiters eagerly: the cancelled task also fails them at
            // exit, but the stop must not depend on that task being polled.
            self.waiters.fail_all_for(&entry.expected_member);
            let task = self
                .pump_tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&entry.incarnation);
            if let Some(task) = task {
                task.abort();
                let _ = task.await;
            }
        }
        if let Err(error) = self
            .store
            .delete_member_event_cursor(&self.mob_id, identity.as_str())
            .await
        {
            tracing::warn!(
                mob_id = %self.mob_id,
                agent_identity = %identity,
                error = %error,
                "member event cursor delete failed at pump stop"
            );
        }
    }

    /// Promotion barrier for a committed placed carrier. The actor calls this
    /// after the durable carrier CAS proves the new host binding generation,
    /// but before publishing that generation into MobMachine. If an old pump
    /// exists, its full residency must match `expected_member`; the task is
    /// cancelled, aborted, and joined before this returns. Cursor deletion is
    /// unnecessary because full-residency cursor matching makes the old row
    /// ineligible for the promoted pump.
    pub(crate) async fn stop_exact_pump_and_join(
        &self,
        identity: &AgentIdentity,
        expected_member: &BridgeMemberIncarnation,
        promoted_member: &BridgeMemberIncarnation,
    ) -> Result<(), MobError> {
        let _transition = self.pump_transition.lock().await;
        self.reap_finished_pump_tasks();
        if expected_member.mob_id != promoted_member.mob_id
            || expected_member.agent_identity != promoted_member.agent_identity
            || expected_member.host_id != promoted_member.host_id
            || expected_member.member_session_id != promoted_member.member_session_id
            || expected_member.generation != promoted_member.generation
            || expected_member.fence_token != promoted_member.fence_token
            || promoted_member.binding_generation <= expected_member.binding_generation
            || promoted_member.agent_identity != identity.as_str()
        {
            return Err(MobError::Internal(format!(
                "invalid pump promotion barrier for '{identity}': old residency {expected_member:?}, promoted residency {promoted_member:?}"
            )));
        }
        let entry = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match state.pumps.get(identity) {
                Some(entry) if &entry.expected_member != expected_member => {
                    return Err(MobError::Internal(format!(
                        "cannot prove old pump quiescence for '{identity}': active residency {:?} differs from expected {:?}",
                        entry.expected_member, expected_member
                    )));
                }
                Some(_) => state.pumps.remove(identity),
                None => None,
            }
        };
        if let Some(entry) = entry {
            entry.cancel.cancel();
            self.waiters.fail_all_for(expected_member);
            let task = self
                .pump_tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&entry.incarnation)
                .ok_or_else(|| {
                    MobError::Internal(format!(
                        "cannot prove old pump quiescence for '{identity}': task {} is not join-owned",
                        entry.incarnation
                    ))
                })?;
            task.abort();
            let _ = task.await;
        }
        let old_still_live = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pumps
            .get(identity)
            .is_some_and(|entry| &entry.expected_member == expected_member);
        if old_still_live {
            return Err(MobError::Internal(format!(
                "old pump for '{identity}' was republished during its promotion barrier"
            )));
        }
        // The durable carrier CAS has already promoted this residency, while
        // MobMachine publication is still pending. Supersede the ticket fold
        // now, after old-task quiescence, so G1 armed tickets fail promptly
        // even when no G2 pump is started immediately. A later G2 ensure is
        // an exact idempotent install.
        if let Some(sink) = self.sink.get() {
            sink.install_member_residency(promoted_member);
        }
        Ok(())
    }

    /// Stop every pump and join every pump task (mob shutdown/destroy).
    ///
    /// Cancellation alone is not an ownership barrier: a task may still be
    /// inside a host poll and could fold/persist a response after the actor has
    /// otherwise fail-stopped. Abort+join makes actor closure a hard boundary.
    pub(crate) async fn stop_all_and_join(&self) {
        let _transition = self.pump_transition.lock().await;
        self.accepting_pumps
            .store(false, std::sync::atomic::Ordering::Release);
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entries: Vec<PumpEntry> = std::mem::take(&mut state.pumps).into_values().collect();
            for entry in entries {
                entry.cancel.cancel();
                // Fail waiters eagerly: the cancelled task also fails them at
                // exit, but the stop must not depend on that task being polled.
                self.waiters.fail_all_for(&entry.expected_member);
            }
        }
        // Keep both synchronous mutex guards in lexical scopes that end before
        // the join barrier. `MobActor::run` is spawned onto Tokio and therefore
        // must remain `Send` across every await.
        let tasks = {
            let mut tasks = self
                .pump_tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *tasks)
                .into_values()
                .collect::<Vec<_>>()
        };
        for task in &tasks {
            task.abort();
        }
        for task in tasks {
            let _ = task.await;
        }
    }

    /// Fan one attributed event out to the member's taps; a full/closed tap
    /// is dropped with a lag notice — the pump is never blocked (the
    /// router's own capacity discipline).
    fn fan_out(
        &self,
        identity: &AgentIdentity,
        expected_member: &BridgeMemberIncarnation,
        incarnation: u64,
        event: &AttributedEvent,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = state.pumps.get_mut(identity) else {
            return;
        };
        if entry.incarnation != incarnation || &entry.expected_member != expected_member {
            return;
        }
        entry.taps.retain(|tap| match tap.try_send(event.clone()) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(
                    agent_identity = %identity,
                    "member event tap lagged; dropping the slow consumer's tap"
                );
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    }

    /// Publish a synthetic, typed gap marker before advancing a cursor past
    /// events that subscribers can no longer receive.
    fn fan_out_truncation(
        &self,
        material: &MemberPumpMaterial,
        incarnation: u64,
        seq: u64,
        reason: StreamTruncationReason,
    ) {
        let envelope = EventEnvelope::new_with_source(
            EventSourceIdentity::runtime(material.runtime_id.to_string()),
            seq,
            Some(self.mob_id.to_string()),
            AgentEvent::StreamTruncated { reason },
        );
        self.fan_out(
            &material.agent_identity,
            &material.expected_member,
            incarnation,
            &AttributedEvent {
                source: material.runtime_id.clone(),
                source_fence_token: material.fence_token,
                role: material.role.clone(),
                envelope,
            },
        );
    }

    /// A17 liveness: does anything keep this member's pump alive?
    fn is_live(&self, identity: &AgentIdentity, expected_member: &BridgeMemberIncarnation) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = state.pumps.get_mut(identity) else {
            return false;
        };
        if entry.exiting || &entry.expected_member != expected_member {
            return false;
        }
        entry.taps.retain(|tap| !tap.is_closed());
        if entry.obligation_keepalive
            || !entry.taps.is_empty()
            || self.waiters.outstanding_for(expected_member)
        {
            return true;
        }
        drop(state);
        // A17 recovery trigger: durable obligations re-derived from machine
        // state keep the pump alive even when no explicit poke arrived
        // (controlling restart with recovered `pending_remote_turn_outcomes`).
        self.has_pending_obligation(expected_member)
    }

    /// Linearize an idle-stop decision against taps, completion-waiter
    /// registration, and obligation ensures. Callers first arm the shared
    /// liveness notification, then use this exact state-lock transition at
    /// the grace boundary. A concurrent registration either lands before
    /// this check (and keeps the pump live) or observes `exiting` and fails /
    /// replaces the dying lease; it can never succeed into the final sweep.
    fn begin_idle_exit(
        &self,
        identity: &AgentIdentity,
        expected_member: &BridgeMemberIncarnation,
        incarnation: u64,
    ) -> bool {
        if self.has_pending_obligation(expected_member) {
            return false;
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(entry) = state.pumps.get_mut(identity) else {
            return true;
        };
        if entry.incarnation != incarnation || &entry.expected_member != expected_member {
            return true;
        }
        entry.taps.retain(|tap| !tap.is_closed());
        if entry.obligation_keepalive
            || !entry.taps.is_empty()
            || self.waiters.outstanding_for(expected_member)
        {
            return false;
        }
        entry.exiting = true;
        true
    }

    fn clear_obligation_keepalive(
        &self,
        identity: &AgentIdentity,
        expected_member: &BridgeMemberIncarnation,
        incarnation: u64,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = state.pumps.get_mut(identity)
            && entry.incarnation == incarnation
            && &entry.expected_member == expected_member
        {
            entry.obligation_keepalive = false;
        }
    }

    fn remove_entry(&self, identity: &AgentIdentity, incarnation: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Only remove OUR incarnation (a respawn replacement may have
        // re-inserted under the same identity).
        if let Some(entry) = state.pumps.get(identity)
            && entry.incarnation == incarnation
        {
            state.pumps.remove(identity);
        }
    }

    fn is_current_incarnation(
        &self,
        identity: &AgentIdentity,
        expected_member: &BridgeMemberIncarnation,
        incarnation: u64,
    ) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pumps
            .get(identity)
            .is_some_and(|entry| {
                !entry.exiting
                    && entry.incarnation == incarnation
                    && &entry.expected_member == expected_member
            })
    }

    /// Persist cursor progress only while the exact pump remains current.
    /// The transition gate is held across the store await, so replacement or
    /// promotion cannot publish a new residency while an old cursor write is
    /// still executing (including SQLite's blocking worker).
    async fn persist_cursor_if_current(
        &self,
        identity: &AgentIdentity,
        expected_member: &BridgeMemberIncarnation,
        incarnation: u64,
        record: &MobMemberEventCursorRecord,
    ) -> Result<bool, MobError> {
        let _transition = self.pump_transition.lock().await;
        if !self.is_current_incarnation(identity, expected_member, incarnation) {
            return Ok(false);
        }
        self.store
            .put_member_event_cursor(&self.mob_id, record)
            .await?;
        Ok(true)
    }
}

/// One pump loop (DEC-P6E-9). Sequential long-polls; the long-poll IS the
/// pacing. Sidecar consumption dedups by `(generation, input_id)` at the
/// SINK (set semantics); obligation resolution is the sink's
/// (registry-owned) responsibility plus idempotent machine resolve.
async fn run_member_pump(
    manager: Arc<MemberEventPumpManager>,
    material: MemberPumpMaterial,
    cancel: CancellationToken,
    incarnation: u64,
) {
    let identity = material.agent_identity.clone();
    tracing::debug!(
        agent_identity = %identity,
        runtime_id = %material.runtime_id,
        incarnation,
        "member pump started"
    );
    // First cursor: the persisted record (its generation may be stale — the
    // server resolves per DEC-P6E-4); an obligation-started pump must not
    // skip history, so absent a record it starts At{gen 0, seq 1} and lets
    // the server's stale-generation rule serve the current domain from 1.
    let mut durable_cursor: BridgeEventCursor = match manager
        .store
        .list_member_event_cursors(&manager.mob_id)
        .await
    {
        Ok(records) => initial_member_cursor(records, &material.expected_member),
        Err(error) => {
            tracing::warn!(
                mob_id = %manager.mob_id,
                agent_identity = %identity,
                error = %error,
                "member event cursor read failed; starting from the seq-domain floor"
            );
            BridgeEventCursor::At {
                generation: material.runtime_id.generation.get(),
                seq: 1,
            }
        }
    };
    let mut cursor = durable_cursor;
    // Trust the recipient once up front (comms admission); per-send trust
    // churn is the provisioner's concern, not the pump's.
    if let Err(error) = manager.bridge.trust_recipient(&material.peer).await {
        tracing::warn!(
            agent_identity = %identity,
            error = %error,
            "member pump could not install recipient trust; retrying per poll"
        );
    }
    let mut backoff = POLL_BACKOFF_FLOOR;
    let mut idle_since: Option<meerkat_core::time_compat::Instant> = None;
    let mut last_incarnation: Option<(u64, u64)> = None;
    // Exact IDs consumed from the previous successful page. They are sent on
    // the next sequential poll and retained across ambiguous failures. A pump
    // restart simply replays the still-durable rows through the sink dedup.
    let mut pending_outcome_acks = manager.resolved_outcome_acks(&material.expected_member);

    'pump: loop {
        manager.prune_host_poll_lanes();
        // `notify_waiters()` wakes only registered listeners. Arm before the
        // liveness read so a tap/waiter/obligation landing between that read
        // and the select cannot strand this pump for the full idle grace.
        let mut liveness_changed = std::pin::pin!(manager.liveness_changed.notified());
        liveness_changed.as_mut().enable();
        if cancel.is_cancelled() {
            break;
        }
        // A17 stop condition with idle grace.
        if !pending_outcome_acks.is_empty() || manager.is_live(&identity, &material.expected_member)
        {
            idle_since = None;
        } else {
            let since = idle_since.get_or_insert_with(meerkat_core::time_compat::Instant::now);
            if since.elapsed() >= PUMP_IDLE_STOP {
                if manager.begin_idle_exit(&identity, &material.expected_member, incarnation) {
                    break;
                }
                idle_since = None;
                continue;
            }
            tokio::select! {
                () = cancel.cancelled() => break,
                () = liveness_changed.as_mut() => continue,
                () = tokio::time::sleep(PUMP_IDLE_STOP) => continue,
            }
        }

        let poll_class = if pending_outcome_acks.is_empty() {
            manager.poll_class(&identity, &material.expected_member)
        } else {
            // A fold-validated terminal is not complete until the host has
            // confirmed its exact ACK. Keep that final hop on the reserved
            // short-poll lane even though resolving the volatile waiter
            // removed it from the waiter registry.
            MemberPollClass::Priority
        };
        let host_lane = manager.host_poll_lane(poll_class, &material.host_id);
        let host_poll_permit = tokio::select! {
            () = liveness_changed.as_mut() => continue,
            permit = host_lane.acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            },
            () = cancel.cancelled() => break,
        };
        let (poll_permits, poll_wait_ms) = manager.poll_lane(poll_class);
        let permit = tokio::select! {
            // A tap-only pump can become custody-critical while queued behind
            // other long polls. Reclassify onto the reserved short-poll lane
            // instead of waiting out that observation backlog.
            () = liveness_changed.as_mut() => continue,
            permit = poll_permits.acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break,
            },
            () = cancel.cancelled() => break,
        };

        let ack_batch_len = pending_outcome_acks.len().min(BRIDGE_TURN_OUTCOME_ACK_MAX);
        let requested_cursor = cursor;
        let page = poll_once(
            &manager,
            &material,
            cursor,
            &pending_outcome_acks[..ack_batch_len],
            poll_wait_ms,
        )
        .await;
        drop(permit);
        drop(host_poll_permit);

        // A release/rematerialization can replace this pump while its poll is
        // in flight. Never fold, ACK, fan out, or persist a response after
        // cancellation or incarnation loss.
        if cancel.is_cancelled()
            || !manager.is_current_incarnation(&identity, &material.expected_member, incarnation)
        {
            break;
        }

        match page {
            Ok(page) => {
                if let Err(error) =
                    validate_member_events_page(requested_cursor, &material.expected_member, &page)
                {
                    if let Some(observations) = manager.reachability_observations.get() {
                        observations.mark_member_poll_failure(&identity, false);
                    }
                    tracing::error!(
                        agent_identity = %identity,
                        error = %error,
                        backoff_ms = backoff.as_millis() as u64,
                        "rejecting malformed member events page atomically"
                    );
                    // A malformed response is not proof that custody can be
                    // abandoned. Preserve cursor, ACK batch, fold state, and
                    // waiter leases, then retry the same exact request after
                    // bounded backoff. A later valid page must converge
                    // without requiring a fresh `ensure` call.
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(POLL_BACKOFF_CEIL);
                    continue;
                }
                let page_incarnation = (page.generation, page.fence_token);
                let material_incarnation = (
                    material.runtime_id.generation.get(),
                    material.fence_token.get(),
                );
                // The host response is self-describing. A same-generation
                // fence rotation is a new authority incarnation even though
                // `runtime_id` is unchanged; never fold or ACK such a page
                // through stale material.
                if page_incarnation != material_incarnation {
                    tracing::warn!(
                        agent_identity = %identity,
                        page_generation = page.generation,
                        page_fence_token = page.fence_token,
                        material_generation = material_incarnation.0,
                        material_fence_token = material_incarnation.1,
                        "stopping member pump on an exact-incarnation page mismatch"
                    );
                    break;
                }
                if last_incarnation.is_some_and(|current| page_incarnation < current) {
                    tracing::warn!(
                        agent_identity = %identity,
                        page_generation = page.generation,
                        page_fence_token = page.fence_token,
                        current_incarnation = ?last_incarnation,
                        "dropping lower-incarnation member event page"
                    );
                    continue;
                }
                if let Err(error) = manager
                    .host_runtime_observer
                    .observe(&material.expected_member, page.runtime_incarnation)
                    .await
                {
                    if let Some(observations) = manager.reachability_observations.get() {
                        observations.mark_member_poll_failure(&identity, false);
                    }
                    tracing::warn!(
                        agent_identity = %identity,
                        host = %material.host_id,
                        runtime_incarnation = %page.runtime_incarnation,
                        error = %error,
                        backoff_ms = backoff.as_millis() as u64,
                        "member event page could not cross the host runtime-incarnation barrier"
                    );
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(POLL_BACKOFF_CEIL);
                    continue;
                }
                if cancel.is_cancelled()
                    || !manager.is_current_incarnation(
                        &identity,
                        &material.expected_member,
                        incarnation,
                    )
                {
                    break;
                }
                if let Some(observations) = manager.reachability_observations.get() {
                    observations.mark_member_progress(&identity);
                }
                backoff = POLL_BACKOFF_FLOOR;
                tracing::trace!(
                    agent_identity = %identity,
                    generation = page.generation,
                    fence_token = page.fence_token,
                    rows = page.events.len(),
                    outcomes = page.turn_outcomes.len(),
                    next_seq = page.next_seq,
                    "member pump page"
                );
                let incarnation_changed =
                    last_incarnation.is_some_and(|current| current < page_incarnation);
                if incarnation_changed {
                    tracing::debug!(
                        agent_identity = %identity,
                        generation = page.generation,
                        fence_token = page.fence_token,
                        "member pump observed an incarnation boundary"
                    );
                }
                last_incarnation = Some(page_incarnation);
                let sink = manager.sink.get().cloned();

                // This successful response proves the host applied every ACK
                // carried by the request. Persist controller-side ACK
                // confirmation before allowing the durable event cursor to
                // advance. Failures retain and resend the exact ACK.
                let sent_acks = pending_outcome_acks
                    .drain(..ack_batch_len)
                    .collect::<Vec<_>>();
                if let Some(sink) = sink.as_ref() {
                    for ack in sent_acks {
                        if let Err(error) = sink
                            .acknowledge_turn_outcome(&material.expected_member, &ack)
                            .await
                        {
                            tracing::warn!(
                                agent_identity = %identity,
                                input_id = %ack.input_id,
                                error = %error,
                                "host applied turn-outcome ACK but controller confirmation failed; resending"
                            );
                            pending_outcome_acks.push(ack);
                        }
                        if cancel.is_cancelled()
                            || !manager.is_current_incarnation(
                                &identity,
                                &material.expected_member,
                                incarnation,
                            )
                        {
                            break 'pump;
                        }
                    }
                    if cancel.is_cancelled()
                        || !manager.is_current_incarnation(
                            &identity,
                            &material.expected_member,
                            incarnation,
                        )
                    {
                        break 'pump;
                    }
                    sink.observe_member_generation(&material.expected_member);
                    // Pin exact sidecar seqs before folding this page. This
                    // is observation only; no consume/ACK occurs before rows.
                    sink.observe_turn_outcome_page(
                        &material.expected_member,
                        page.next_seq,
                        page.outcomes_complete,
                        &page.turn_outcomes,
                    );
                } else {
                    let mut restored = sent_acks;
                    restored.append(&mut pending_outcome_acks);
                    pending_outcome_acks = restored;
                }
                for (seq, envelope) in into_durable_event_rows(page.events) {
                    if cancel.is_cancelled()
                        || !manager.is_current_incarnation(
                            &identity,
                            &material.expected_member,
                            incarnation,
                        )
                    {
                        break 'pump;
                    }
                    if let Some(sink) = sink.as_ref() {
                        sink.observe_member_event_row(
                            &material.expected_member,
                            seq,
                            &envelope.payload,
                        );
                    }
                    manager.fan_out(
                        &identity,
                        &material.expected_member,
                        incarnation,
                        &AttributedEvent {
                            source: material.runtime_id.clone(),
                            source_fence_token: material.fence_token,
                            role: material.role.clone(),
                            envelope,
                        },
                    );
                }
                // Sidecar AFTER rows (the adjudicated page order).
                let mut replay_required = false;
                if let Some(sink) = sink.as_ref() {
                    for record in &page.turn_outcomes {
                        match sink
                            .consume_turn_outcome_record(&material.expected_member, record)
                            .await
                        {
                            Ok(OutcomeRecordDisposition::Acknowledge) => {
                                pending_outcome_acks.push(BridgeTurnOutcomeAck {
                                    generation: record.generation,
                                    fence_token: record.fence_token,
                                    input_id: record.input_id.clone(),
                                });
                            }
                            Ok(OutcomeRecordDisposition::AcknowledgeUnclaimed) => {
                                if let Err(error) = resolve_interaction_waiter_from_record(
                                    &manager.waiters,
                                    &material.expected_member,
                                    record,
                                ) {
                                    tracing::error!(
                                        agent_identity = %identity,
                                        input_id = %record.input_id,
                                        error = %error,
                                        "unclaimed remote outcome failed exact waiter correlation; retaining host row"
                                    );
                                    continue;
                                }
                                pending_outcome_acks.push(BridgeTurnOutcomeAck {
                                    generation: record.generation,
                                    fence_token: record.fence_token,
                                    input_id: record.input_id.clone(),
                                });
                            }
                            Ok(OutcomeRecordDisposition::AcknowledgePlacedCompletion) => {
                                if let Err(error) = resolve_interaction_waiter_from_record(
                                    &manager.waiters,
                                    &material.expected_member,
                                    record,
                                ) {
                                    tracing::error!(
                                        agent_identity = %identity,
                                        input_id = %record.input_id,
                                        error = %error,
                                        "placed completion failed exact waiter correlation; retaining host row"
                                    );
                                    continue;
                                }
                                pending_outcome_acks.push(BridgeTurnOutcomeAck {
                                    generation: record.generation,
                                    fence_token: record.fence_token,
                                    input_id: record.input_id.clone(),
                                });
                            }
                            Ok(OutcomeRecordDisposition::Replay) => replay_required = true,
                            Ok(OutcomeRecordDisposition::Retain) => {}
                            Err(error) => {
                                tracing::warn!(
                                    agent_identity = %identity,
                                    input_id = %record.input_id,
                                    error = %error,
                                    "remote outcome resolve failed; retaining host row for replay"
                                );
                            }
                        }
                        if cancel.is_cancelled()
                            || !manager.is_current_incarnation(
                                &identity,
                                &material.expected_member,
                                incarnation,
                            )
                        {
                            break 'pump;
                        }
                    }
                }
                // Machine state is the restart source. Union in ACK-pending
                // custody so a response lost before this process observed it
                // is resent even when the host no longer returns the row.
                pending_outcome_acks
                    .extend(manager.resolved_outcome_acks(&material.expected_member));
                pending_outcome_acks.sort_by(|a, b| {
                    (a.generation, a.fence_token, &a.input_id).cmp(&(
                        b.generation,
                        b.fence_token,
                        &b.input_id,
                    ))
                });
                pending_outcome_acks.dedup_by(|a, b| {
                    a.generation == b.generation
                        && a.fence_token == b.fence_token
                        && a.input_id == b.input_id
                });
                // The explicit keep-alive poke lapses only when the MACHINE
                // no longer holds a pending obligation for this member: an
                // empty sidecar page just means the outcome has not landed
                // YET (the turn may still be running member-side) — the
                // obligation stays outstanding until resolved (A17).
                if !manager.has_pending_obligation(&material.expected_member) {
                    manager.clear_obligation_keepalive(
                        &identity,
                        &material.expected_member,
                        incarnation,
                    );
                }
                cursor = BridgeEventCursor::At {
                    generation: page.generation,
                    seq: page.next_seq,
                };
                if replay_required {
                    if let Some(sink) = sink.as_ref() {
                        sink.rewind_member_fold(&material.expected_member);
                    }
                    cursor = durable_cursor;
                } else if !manager
                    .has_cursor_custody(&material.expected_member, &pending_outcome_acks)
                {
                    // Recheck machine custody after every awaited fold/resolve
                    // before writing. A concurrent Record-before-send keeps
                    // the durable cursor frozen while live progress continues.
                    if cancel.is_cancelled()
                        || !manager.is_current_incarnation(
                            &identity,
                            &material.expected_member,
                            incarnation,
                        )
                    {
                        break;
                    }
                    let record = MobMemberEventCursorRecord {
                        agent_identity: identity.to_string(),
                        host_id: material.host_id.clone(),
                        host_binding_generation: Some(material.expected_member.binding_generation),
                        member_session_id: material.expected_member.member_session_id.clone(),
                        generation: page.generation,
                        fence_token: Some(page.fence_token),
                        next_seq: page.next_seq,
                    };
                    match manager
                        .persist_cursor_if_current(
                            &identity,
                            &material.expected_member,
                            incarnation,
                            &record,
                        )
                        .await
                    {
                        Ok(true) => durable_cursor = cursor,
                        Ok(false) => break,
                        Err(error) => {
                            tracing::warn!(
                                agent_identity = %identity,
                                error = %error,
                                "member event cursor write failed; resume will re-read"
                            );
                        }
                    }
                }
                // Long-poll IS the pacing: loop immediately.
            }
            Err(MobError::BridgeCommandRejected {
                cause:
                    BridgeRejectionCause::StaleCursor {
                        watermark,
                        generation,
                    },
                ..
            }) => {
                let next_seq = match remote_cursor_successor(watermark) {
                    Ok(next_seq) => next_seq,
                    Err(error) => {
                        if let Some(observations) = manager.reachability_observations.get() {
                            observations.mark_member_poll_failure(&identity, false);
                        }
                        tracing::error!(
                            agent_identity = %identity,
                            watermark,
                            generation,
                            error = %error,
                            "member pump stopped at exhausted remote event watermark"
                        );
                        break;
                    }
                };
                // Ring overrun (ephemeral host): restart from the watermark
                // and label the gap (§9 row).
                tracing::warn!(
                    agent_identity = %identity,
                    watermark,
                    generation,
                    "member event cursor overran the retained window; restarting from watermark"
                );
                manager.fan_out_truncation(
                    &material,
                    incarnation,
                    next_seq,
                    StreamTruncationReason::RemoteCursorOverrun { watermark },
                );
                cursor = BridgeEventCursor::At {
                    generation,
                    seq: next_seq,
                };
                let record = MobMemberEventCursorRecord {
                    agent_identity: identity.to_string(),
                    host_id: material.host_id.clone(),
                    host_binding_generation: Some(material.expected_member.binding_generation),
                    member_session_id: material.expected_member.member_session_id.clone(),
                    generation,
                    fence_token: Some(material.fence_token.get()),
                    next_seq,
                };
                if !manager.has_cursor_custody(&material.expected_member, &pending_outcome_acks)
                    && !cancel.is_cancelled()
                    && manager.is_current_incarnation(
                        &identity,
                        &material.expected_member,
                        incarnation,
                    )
                {
                    match manager
                        .persist_cursor_if_current(
                            &identity,
                            &material.expected_member,
                            incarnation,
                            &record,
                        )
                        .await
                    {
                        Ok(true) => durable_cursor = cursor,
                        Ok(false) => break,
                        Err(error) => {
                            tracing::warn!(
                                agent_identity = %identity,
                                error = %error,
                                "member event cursor write failed after overrun restart"
                            );
                        }
                    }
                }
            }
            Err(MobError::BridgeCommandRejected {
                cause:
                    BridgeRejectionCause::OversizedEvent {
                        generation,
                        durable_seq,
                        next_seq,
                        encoded_bytes,
                        max_bytes,
                    },
                reason,
            }) => {
                // Deterministic poison-row disposition: the owning host has
                // proved this immutable envelope cannot cross the transport.
                // Record a typed gap and advance exactly one durable row;
                // retrying the same cursor would livelock forever.
                tracing::error!(
                    agent_identity = %identity,
                    generation,
                    durable_seq,
                    encoded_bytes,
                    max_bytes,
                    reason = %reason,
                    "member event omitted because one durable row exceeds the bridge budget"
                );
                manager.fan_out_truncation(
                    &material,
                    incarnation,
                    durable_seq,
                    StreamTruncationReason::OversizedRemoteEvent {
                        durable_seq,
                        encoded_bytes,
                        max_bytes,
                    },
                );
                if let Some(sink) = manager.sink.get()
                    && !cancel.is_cancelled()
                    && manager.is_current_incarnation(
                        &identity,
                        &material.expected_member,
                        incarnation,
                    )
                {
                    sink.observe_omitted_member_event_row(&material.expected_member, durable_seq);
                }
                cursor = BridgeEventCursor::At {
                    generation,
                    seq: next_seq,
                };
                // Do not persist the poison-row advance in the rejection
                // turn itself. The following successful page carries the
                // independently projected sidecar (if this was a tracked
                // terminal) and can establish ACK custody first. For an
                // ordinary oversized row, that next page advances the same
                // cursor normally; a crash in this one-poll window merely
                // replays the deterministic typed omission once.
            }
            Err(MobError::BridgeCommandRejected {
                cause:
                    cause @ (BridgeRejectionCause::NotBound
                    | BridgeRejectionCause::StaleSupervisor
                    | BridgeRejectionCause::SenderMismatch),
                reason,
            }) => {
                if let Some(observations) = manager.reachability_observations.get() {
                    observations.mark_member_poll_failure(&identity, false);
                }
                // Authority-class reject: observation never mutates
                // membership (§7.5) — stop and surface typed via log; the
                // rebind/rotation lanes own recovery.
                tracing::error!(
                    agent_identity = %identity,
                    cause = ?cause,
                    reason = %reason,
                    "member pump stopped on an authority-class rejection"
                );
                break;
            }
            Err(MobError::BridgeCommandRejected {
                cause: cause @ BridgeRejectionCause::Internal,
                reason,
            }) => {
                if let Some(observations) = manager.reachability_observations.get() {
                    observations.mark_member_poll_failure(&identity, false);
                }
                // Permanent-reject class (e.g. a future-generation cursor
                // against a restored-from-backup member): retrying the SAME
                // cursor can never be accepted, so a fixed-interval retry
                // loop cannot self-heal — stop typed instead of polling
                // forever. Outstanding waiters fail at the loop exit below.
                tracing::error!(
                    agent_identity = %identity,
                    cursor = ?cursor,
                    cause = ?cause,
                    reason = %reason,
                    "member pump stopped on a permanently-rejected cursor"
                );
                break;
            }
            Err(error) => {
                // Unreachable / unavailable / timeout: update the shared
                // observer-local projection, then back off. No membership
                // fact is mutated by this liveness observation.
                if let Some(observations) = manager.reachability_observations.get() {
                    observations.mark_member_poll_failure(
                        &identity,
                        matches!(error, MobError::BridgeRequestTimedOut { .. }),
                    );
                }
                tracing::debug!(
                    agent_identity = %identity,
                    error = %error,
                    backoff_ms = backoff.as_millis() as u64,
                    "member poll failed; backing off"
                );
                tokio::select! {
                    () = cancel.cancelled() => break,
                    () = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(POLL_BACKOFF_CEIL);
            }
        }
    }
    manager.prune_host_poll_lanes();
    // Withdraw the live-pump lease before waiting for the transition
    // barrier. Without this marker, an ensure can acquire the barrier first,
    // see the still-published exact tuple, return idempotent success, and
    // then lose the pump when this task removes the entry immediately after.
    {
        let mut state = manager
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(entry) = state.pumps.get_mut(&identity)
            && entry.incarnation == incarnation
            && entry.expected_member == material.expected_member
        {
            entry.exiting = true;
        }
    }
    // Publish task exit under the same transition barrier used by
    // replacement/promotion. There are no awaits after this lock is
    // acquired, so observing the entry absent after acquiring the barrier
    // proves the task's final poll has returned (or an owner aborted+joined
    // it while holding the barrier).
    let _transition = manager.pump_transition.lock().await;
    manager.remove_entry(&identity, incarnation);
    // A dying pump fails its exact-tuple waiters unless a replacement pump
    // with the same runtime+fence is already current (address-only refresh):
    // that replacement inherits the waiters. Different-fence/generation or
    // no-replacement exits fail only the old tuple.
    manager.fail_waiters_if_no_matching_pump(&identity, &material.expected_member);
}

async fn poll_once(
    manager: &MemberEventPumpManager,
    material: &MemberPumpMaterial,
    cursor: BridgeEventCursor,
    outcome_acks: &[BridgeTurnOutcomeAck],
    wait_ms: u32,
) -> Result<BridgeMemberEventsPage, MobError> {
    let authority = manager.bridge.authority().await;
    let sup_spec = manager
        .bridge
        .supervisor_spec_for_recipient(&material.peer)
        .await?;
    let command = BridgeCommand::PollMemberEvents(BridgePollEventsPayload {
        supervisor: sup_spec.into(),
        epoch: authority.epoch,
        protocol_version: BridgeProtocolVersion::V4,
        expected_member: material.expected_member.clone(),
        cursor,
        max: Some(POLL_MAX_ROWS),
        outcome_acks: outcome_acks.to_vec(),
        max_outcomes: Some(POLL_MAX_OUTCOMES),
        wait_ms: Some(wait_ms),
    });
    let value = manager
        .bridge
        .send_bridge_command(&material.peer, &command, POLL_REQUEST_TIMEOUT)
        .await?;
    super::bridge_protocol::decode_bridge_payload(&command, value, "poll member events")
}

// ---------------------------------------------------------------------------
// T-E4 in-crate pin (DEC-P6E-19): SubmitWork completion for placed members
// rides the pumped interaction terminal via `RemoteCompletionWaiters` —
// waiter resolution ordering both directions (register-then-resolve and
// resolve-then-register), never a dispatch-ack degrade.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{AgentIdentity, AgentRuntimeId, RunId, StepId};
    use crate::runtime::remote_flow_ticket::RemoteTurnObligationResolver;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct AcceptHostRuntimeIncarnation;

    #[async_trait]
    impl HostRuntimeIncarnationObserver for AcceptHostRuntimeIncarnation {
        async fn observe(
            &self,
            _expected_member: &BridgeMemberIncarnation,
            _runtime_incarnation: BridgeHostRuntimeIncarnation,
        ) -> Result<(), MobError> {
            Ok(())
        }
    }

    fn accepting_host_runtime_observer() -> Arc<dyn HostRuntimeIncarnationObserver> {
        Arc::new(AcceptHostRuntimeIncarnation)
    }

    const TEST_HOST_ID: &str = "host-a";

    #[derive(Default)]
    struct TestOutcomeResolver {
        resolved: AtomicUsize,
        acknowledged: AtomicUsize,
    }

    #[derive(Default)]
    struct RecordingOutcomeSink {
        rewinds: AtomicUsize,
    }

    struct StaticAckProbe {
        ack: BridgeTurnOutcomeAck,
    }

    impl RemoteObligationProbe for StaticAckProbe {
        fn has_pending_obligation(&self, _expected_member: &BridgeMemberIncarnation) -> bool {
            true
        }

        fn resolved_outcome_acks(
            &self,
            _expected_member: &BridgeMemberIncarnation,
        ) -> Vec<BridgeTurnOutcomeAck> {
            vec![self.ack.clone()]
        }
    }

    #[async_trait]
    impl RemoteTurnOutcomeSink for RecordingOutcomeSink {
        fn install_member_residency(&self, _expected_member: &BridgeMemberIncarnation) {}

        fn observe_member_generation(&self, _expected_member: &BridgeMemberIncarnation) {}

        fn rewind_member_fold(&self, _expected_member: &BridgeMemberIncarnation) {
            self.rewinds.fetch_add(1, Ordering::SeqCst);
        }

        fn observe_turn_outcome_page(
            &self,
            _expected_member: &BridgeMemberIncarnation,
            _event_frontier_exclusive: u64,
            _outcomes_complete: bool,
            _records: &[BridgeTurnOutcomeRecord],
        ) {
        }

        fn observe_member_event_row(
            &self,
            _expected_member: &BridgeMemberIncarnation,
            _seq: u64,
            _event: &AgentEvent,
        ) {
        }

        fn observe_omitted_member_event_row(
            &self,
            _expected_member: &BridgeMemberIncarnation,
            _seq: u64,
        ) {
        }

        async fn consume_turn_outcome_record(
            &self,
            _expected_member: &BridgeMemberIncarnation,
            _record: &BridgeTurnOutcomeRecord,
        ) -> Result<OutcomeRecordDisposition, MobError> {
            Ok(OutcomeRecordDisposition::Retain)
        }

        async fn acknowledge_turn_outcome(
            &self,
            _expected_member: &BridgeMemberIncarnation,
            _ack: &BridgeTurnOutcomeAck,
        ) -> Result<(), MobError> {
            Ok(())
        }
    }

    #[async_trait]
    impl RemoteTurnObligationResolver for TestOutcomeResolver {
        async fn resolve(
            &self,
            _expected_member: &BridgeMemberIncarnation,
            _record: &BridgeTurnOutcomeRecord,
        ) -> Result<OutcomeRecordDisposition, MobError> {
            self.resolved.fetch_add(1, Ordering::SeqCst);
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

    fn test_member(
        identity: &AgentIdentity,
        runtime_id: &AgentRuntimeId,
        fence_token: FenceToken,
    ) -> BridgeMemberIncarnation {
        test_member_with_residency(identity, runtime_id, fence_token, 1, "session-a")
    }

    fn test_member_with_residency(
        identity: &AgentIdentity,
        runtime_id: &AgentRuntimeId,
        fence_token: FenceToken,
        binding_generation: u64,
        member_session_id: &str,
    ) -> BridgeMemberIncarnation {
        BridgeMemberIncarnation {
            mob_id: "test-mob".to_string(),
            agent_identity: identity.to_string(),
            host_id: TEST_HOST_ID.to_string(),
            binding_generation,
            member_session_id: member_session_id.to_string(),
            generation: runtime_id.generation.get(),
            fence_token: fence_token.get(),
        }
    }

    #[test]
    fn exhausted_remote_watermark_has_no_wrapped_or_replayed_successor() {
        assert_eq!(remote_cursor_successor(u64::MAX - 1).unwrap(), u64::MAX);
        let error = remote_cursor_successor(u64::MAX)
            .expect_err("MAX watermark must stop the pump instead of reusing MAX or zero");
        assert!(
            error
                .to_string()
                .contains("exhausted remote event watermark")
        );
    }

    #[test]
    fn durable_event_rows_preserve_non_contiguous_store_sequences() {
        let rows = vec![
            WireEventRow {
                durable_seq: 1,
                envelope: EventEnvelope::new(
                    "worker-gap",
                    10,
                    Some("mob-gap".to_string()),
                    AgentEvent::TurnStarted { turn_number: 1 },
                ),
            },
            WireEventRow {
                durable_seq: 42,
                envelope: EventEnvelope::new(
                    "worker-gap",
                    11,
                    Some("mob-gap".to_string()),
                    AgentEvent::TurnStarted { turn_number: 2 },
                ),
            },
        ];

        let decoded: Vec<(u64, u64)> = into_durable_event_rows(rows)
            .map(|(durable_seq, envelope)| (durable_seq, envelope.seq))
            .collect();
        assert_eq!(
            decoded,
            vec![(1, 10), (42, 11)],
            "the pump must preserve durable gaps and keep envelope seq separate"
        );
    }

    fn page_validation_member() -> BridgeMemberIncarnation {
        let identity = AgentIdentity::from("page-validation-worker");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        test_member(&identity, &runtime_id, FenceToken::new(7))
    }

    fn page_validation_row(seq: u64) -> WireEventRow {
        let turn_number = u32::try_from(seq).expect("test event sequence fits a turn number");
        WireEventRow {
            durable_seq: seq,
            envelope: EventEnvelope::new(
                "page-validation-worker",
                seq,
                Some("page-validation-mob".to_string()),
                AgentEvent::TurnStarted { turn_number },
            ),
        }
    }

    fn page_validation_outcome(
        member: &BridgeMemberIncarnation,
        input: u128,
        terminal_seq: u64,
    ) -> BridgeTurnOutcomeRecord {
        BridgeTurnOutcomeRecord {
            input_id: uuid::Uuid::from_u128(input).to_string(),
            generation: member.generation,
            fence_token: member.fence_token,
            terminal_seq,
            outcome: super::super::bridge_protocol::WireFlowTurnOutcome::InteractionComplete,
        }
    }

    fn page_validation_page(
        member: &BridgeMemberIncarnation,
        rows: Vec<WireEventRow>,
        next_seq: u64,
        watermark: u64,
    ) -> BridgeMemberEventsPage {
        let from_seq = rows.first().map_or(next_seq, |row| row.durable_seq);
        BridgeMemberEventsPage {
            runtime_incarnation: BridgeHostRuntimeIncarnation::new(),
            generation: member.generation,
            fence_token: member.fence_token,
            events: rows,
            from_seq,
            next_seq,
            watermark,
            turn_outcomes: Vec::new(),
            outcomes_complete: true,
        }
    }

    #[test]
    fn member_page_validation_accepts_gapped_rows_with_exact_frontier() {
        let member = page_validation_member();
        let mut page = page_validation_page(
            &member,
            vec![page_validation_row(5), page_validation_row(9)],
            10,
            12,
        );
        page.turn_outcomes
            .push(page_validation_outcome(&member, 201, 9));
        validate_member_events_page(
            BridgeEventCursor::At {
                generation: member.generation,
                seq: 5,
            },
            &member,
            &page,
        )
        .expect("gapped durable rows remain legal when ordered and exactly fronted");
    }

    #[test]
    fn member_page_validation_accepts_empty_page_at_host_resolved_generation_floor() {
        let member = page_validation_member();
        let mut page = page_validation_page(&member, Vec::new(), 101, 100);
        page.from_seq = 101;

        validate_member_events_page(
            BridgeEventCursor::At {
                generation: member.generation,
                seq: 1,
            },
            &member,
            &page,
        )
        .expect("an idle same-session Resume advances to the host-resolved generation floor");

        page.next_seq = 102;
        assert!(
            validate_member_events_page(
                BridgeEventCursor::At {
                    generation: member.generation,
                    seq: 1,
                },
                &member,
                &page,
            )
            .is_err(),
            "an empty page cannot advance beyond its authenticated resolved floor"
        );
    }

    #[test]
    fn member_page_validation_rejects_resolved_floor_regression_and_pre_floor_rows() {
        let member = page_validation_member();
        let mut regressed = page_validation_page(&member, Vec::new(), 4, 8);
        regressed.from_seq = 4;
        assert!(
            validate_member_events_page(
                BridgeEventCursor::At {
                    generation: member.generation,
                    seq: 5,
                },
                &member,
                &regressed,
            )
            .is_err()
        );

        let mut pre_floor = page_validation_page(&member, vec![page_validation_row(100)], 101, 101);
        pre_floor.from_seq = 101;
        assert!(
            validate_member_events_page(
                BridgeEventCursor::At {
                    generation: member.generation,
                    seq: 1,
                },
                &member,
                &pre_floor,
            )
            .is_err()
        );
    }

    #[test]
    fn member_page_validation_rejects_cursor_regression_and_frontier_jump() {
        let member = page_validation_member();
        let regression = page_validation_page(&member, vec![page_validation_row(4)], 5, 5);
        assert!(
            validate_member_events_page(
                BridgeEventCursor::At {
                    generation: member.generation,
                    seq: 5,
                },
                &member,
                &regression,
            )
            .is_err()
        );

        let jump = page_validation_page(&member, vec![page_validation_row(5)], 7, 7);
        assert!(
            validate_member_events_page(
                BridgeEventCursor::At {
                    generation: member.generation,
                    seq: 5,
                },
                &member,
                &jump,
            )
            .is_err()
        );
    }

    #[test]
    fn member_page_validation_rejects_inexact_tail_and_recovers_on_next_valid_page() {
        let member = page_validation_member();
        let malformed = page_validation_page(&member, vec![page_validation_row(5)], 8, 8);
        assert!(
            validate_member_events_page(
                BridgeEventCursor::At {
                    generation: member.generation,
                    seq: 5,
                },
                &member,
                &malformed,
            )
            .is_err()
        );

        // Validation is deliberately stateless: rejecting one page cannot
        // poison the unchanged cursor or force pump replacement. Retrying the
        // same request with a valid page is immediately admissible.
        let valid = page_validation_page(&member, vec![page_validation_row(5)], 6, 8);
        validate_member_events_page(
            BridgeEventCursor::At {
                generation: member.generation,
                seq: 5,
            },
            &member,
            &valid,
        )
        .expect("a valid retry at the same cursor must converge");

        assert!(validate_member_events_page(BridgeEventCursor::Tail, &member, &valid).is_err());
    }

    #[test]
    fn member_page_validation_rejects_row_order_duplicates_and_bad_watermark() {
        let member = page_validation_member();
        let reversed = page_validation_page(
            &member,
            vec![page_validation_row(6), page_validation_row(5)],
            6,
            6,
        );
        assert!(
            validate_member_events_page(
                BridgeEventCursor::At {
                    generation: member.generation,
                    seq: 5,
                },
                &member,
                &reversed,
            )
            .is_err()
        );

        let duplicate = page_validation_page(
            &member,
            vec![page_validation_row(5), page_validation_row(5)],
            6,
            6,
        );
        assert!(
            validate_member_events_page(
                BridgeEventCursor::At {
                    generation: member.generation,
                    seq: 5,
                },
                &member,
                &duplicate,
            )
            .is_err()
        );

        let bad_watermark = page_validation_page(&member, vec![page_validation_row(5)], 6, 4);
        assert!(
            validate_member_events_page(
                BridgeEventCursor::At {
                    generation: member.generation,
                    seq: 5,
                },
                &member,
                &bad_watermark,
            )
            .is_err()
        );
    }

    #[test]
    fn member_page_validation_rejects_duplicate_or_mixed_sidecars_atomically() {
        let member = page_validation_member();
        let mut duplicate = page_validation_page(&member, vec![page_validation_row(5)], 6, 5);
        let record = page_validation_outcome(&member, 202, 5);
        duplicate.turn_outcomes = vec![record.clone(), record];
        assert!(
            validate_member_events_page(
                BridgeEventCursor::At {
                    generation: member.generation,
                    seq: 5,
                },
                &member,
                &duplicate,
            )
            .is_err()
        );

        let mut mixed = page_validation_page(&member, vec![page_validation_row(5)], 6, 5);
        mixed.turn_outcomes = vec![BridgeTurnOutcomeRecord {
            generation: member.generation + 1,
            ..page_validation_outcome(&member, 203, 5)
        }];
        assert!(
            validate_member_events_page(
                BridgeEventCursor::At {
                    generation: member.generation,
                    seq: 5,
                },
                &member,
                &mixed,
            )
            .is_err()
        );
    }

    #[test]
    fn member_page_validation_enforces_requested_size_bounds() {
        let member = page_validation_member();
        let rows = (1..=u64::from(POLL_MAX_ROWS) + 1)
            .map(page_validation_row)
            .collect::<Vec<_>>();
        let page = page_validation_page(
            &member,
            rows,
            u64::from(POLL_MAX_ROWS) + 2,
            u64::from(POLL_MAX_ROWS) + 1,
        );
        assert!(
            validate_member_events_page(
                BridgeEventCursor::At {
                    generation: member.generation,
                    seq: 1,
                },
                &member,
                &page,
            )
            .is_err()
        );
    }

    #[test]
    fn same_generation_new_fence_does_not_reuse_old_high_cursor() {
        let identity = AgentIdentity::from("w-refenced-cursor");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let stale = MobMemberEventCursorRecord {
            agent_identity: identity.to_string(),
            host_id: TEST_HOST_ID.to_string(),
            host_binding_generation: Some(1),
            member_session_id: "session-a".to_string(),
            generation: runtime_id.generation.get(),
            fence_token: Some(7),
            next_seq: 100,
        };
        let expected_fence_8 = test_member(&identity, &runtime_id, FenceToken::new(8));
        let expected_fence_7 = test_member(&identity, &runtime_id, FenceToken::new(7));

        assert_eq!(
            initial_member_cursor(vec![stale.clone()], &expected_fence_8),
            BridgeEventCursor::At {
                generation: runtime_id.generation.get(),
                seq: 1,
            }
        );
        assert_eq!(
            initial_member_cursor(vec![stale], &expected_fence_7),
            BridgeEventCursor::At {
                generation: runtime_id.generation.get(),
                seq: 100,
            }
        );
    }

    #[test]
    fn host_move_resets_cursor_but_same_host_address_refresh_retains_it() {
        let identity = AgentIdentity::from("w-moved-cursor");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let persisted = MobMemberEventCursorRecord {
            agent_identity: identity.to_string(),
            host_id: TEST_HOST_ID.to_string(),
            host_binding_generation: Some(1),
            member_session_id: "session-a".to_string(),
            generation: runtime_id.generation.get(),
            fence_token: Some(7),
            next_seq: 144,
        };
        let expected = test_member(&identity, &runtime_id, FenceToken::new(7));
        let moved = BridgeMemberIncarnation {
            host_id: "host-b".to_string(),
            ..expected.clone()
        };

        assert_eq!(
            initial_member_cursor(vec![persisted.clone()], &expected),
            BridgeEventCursor::At {
                generation: runtime_id.generation.get(),
                seq: 144,
            },
            "same-host transport address refresh retains the exact cursor domain"
        );
        assert_eq!(
            initial_member_cursor(vec![persisted], &moved),
            BridgeEventCursor::At {
                generation: runtime_id.generation.get(),
                seq: 1,
            },
            "moving the same generation/fence to another host starts its event domain at one"
        );
    }

    #[test]
    fn legacy_cursor_without_binding_and_session_is_never_reused() {
        let identity = AgentIdentity::from("w-legacy-cursor");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let expected = test_member(&identity, &runtime_id, FenceToken::new(7));
        let legacy: MobMemberEventCursorRecord = serde_json::from_value(serde_json::json!({
            "agent_identity": identity.to_string(),
            "host_id": TEST_HOST_ID,
            "generation": runtime_id.generation.get(),
            "fence_token": 7,
            "next_seq": 999
        }))
        .expect("legacy cursor JSON remains readable");
        assert_eq!(legacy.host_binding_generation, None);
        assert!(legacy.member_session_id.is_empty());

        assert_eq!(
            initial_member_cursor(vec![legacy], &expected),
            BridgeEventCursor::At {
                generation: runtime_id.generation.get(),
                seq: 1,
            }
        );
    }

    fn interaction(uuid: u128) -> InteractionId {
        InteractionId(uuid::Uuid::from_u128(uuid))
    }

    async fn completion_test_manager(name: &str) -> Arc<MemberEventPumpManager> {
        let authority = crate::store::SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let mob_id = crate::MobId::from(name);
        let bridge = Arc::new(
            super::super::MobSupervisorBridge::new(&mob_id, authority, None)
                .await
                .expect("supervisor bridge builds"),
        );
        let store: Arc<dyn MobRuntimeMetadataStore> =
            Arc::new(crate::store::InMemoryMobRuntimeMetadataStore::new());
        Arc::new(MemberEventPumpManager::new(
            mob_id,
            bridge,
            store,
            accepting_host_runtime_observer(),
        ))
    }

    #[tokio::test]
    async fn replacement_ack_derivation_waits_for_exact_live_completion_observer() {
        let manager = completion_test_manager("replacement-ack-waiter-gate").await;
        let identity = AgentIdentity::from("w-replacement-ack");
        let runtime_id = AgentRuntimeId::initial(identity);
        let expected_member = test_member(&runtime_id.identity, &runtime_id, FenceToken::new(7));
        let interaction_id = interaction(0x867);
        let ack = BridgeTurnOutcomeAck {
            generation: expected_member.generation,
            fence_token: expected_member.fence_token,
            input_id: interaction_id.0.to_string(),
        };
        manager.install_obligation_probe(Arc::new(StaticAckProbe { ack: ack.clone() }));

        let waiter = manager.waiters.register(&expected_member, interaction_id);
        assert!(
            manager.resolved_outcome_acks(&expected_member).is_empty(),
            "a replacement pump must not derive/send an ACK before exact waiter fanout"
        );

        manager.waiters.resolve(
            &expected_member,
            interaction_id,
            &RemoteInteractionTerminal::Closed {
                closure: crate::event::PlacedCompletionClosureEvent::HostCancelled,
            },
        );
        assert!(matches!(
            waiter.await,
            Ok(RemoteInteractionTerminal::Closed {
                closure: crate::event::PlacedCompletionClosureEvent::HostCancelled
            })
        ));
        assert_eq!(manager.resolved_outcome_acks(&expected_member), vec![ack]);

        let dropped_id = interaction(0x868);
        let dropped_ack = BridgeTurnOutcomeAck {
            input_id: dropped_id.0.to_string(),
            ..manager.resolved_outcome_acks(&expected_member)[0].clone()
        };
        let dropped = manager.waiters.register(&expected_member, dropped_id);
        drop(dropped);
        // Closed receivers are pruned and cannot freeze durable ACK custody.
        assert!(
            !manager
                .waiters
                .outstanding_exact(&expected_member, &dropped_ack.input_id)
        );
    }

    fn completion_test_peer(
        name: &str,
        endpoint: &str,
    ) -> meerkat_core::comms::TrustedPeerDescriptor {
        meerkat_core::comms::TrustedPeerDescriptor {
            peer_id: meerkat_core::comms::PeerId::new(),
            name: meerkat_core::comms::PeerName::new(name).expect("valid peer name"),
            address: meerkat_core::comms::PeerAddress::new(
                meerkat_core::comms::PeerTransport::Tcp,
                endpoint,
            ),
            pubkey: [0u8; 32],
        }
    }

    fn install_test_pump(
        manager: &MemberEventPumpManager,
        identity: &AgentIdentity,
        runtime_id: &AgentRuntimeId,
        fence_token: FenceToken,
        incarnation: u64,
        peer: meerkat_core::comms::TrustedPeerDescriptor,
    ) -> BridgeMemberIncarnation {
        let expected_member = test_member(identity, runtime_id, fence_token);
        manager
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pumps
            .insert(
                identity.clone(),
                PumpEntry {
                    cancel: CancellationToken::new(),
                    incarnation,
                    expected_member: expected_member.clone(),
                    exiting: false,
                    runtime_id: runtime_id.clone(),
                    peer,
                    taps: Vec::new(),
                    obligation_keepalive: false,
                },
            );
        expected_member
    }

    #[tokio::test]
    async fn waiter_resolves_on_validated_interaction_terminal() {
        let waiters = RemoteCompletionWaiters::default();
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("w-e4"));
        let fence_token = FenceToken::new(1);
        let expected_member = test_member(&runtime_id.identity, &runtime_id, fence_token);
        let waiter = waiters.register(&expected_member, interaction(1));

        waiters.resolve(
            &expected_member,
            interaction(1),
            &RemoteInteractionTerminal::Complete,
        );
        assert!(
            matches!(waiter.await, Ok(RemoteInteractionTerminal::Complete)),
            "the registered waiter resolves from the validated terminal"
        );
    }

    #[tokio::test]
    async fn unclaimed_interaction_sidecar_resolves_only_the_exact_waiter() {
        let waiters = RemoteCompletionWaiters::default();
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("w-unclaimed-sidecar"));
        let expected_member = test_member(&runtime_id.identity, &runtime_id, FenceToken::new(7));
        let other_member = BridgeMemberIncarnation {
            member_session_id: "other-session".to_string(),
            ..expected_member.clone()
        };
        let interaction_id = interaction(104);
        let waiter = waiters.register(&expected_member, interaction_id);
        let unrelated = waiters.register(&other_member, interaction_id);
        let record = BridgeTurnOutcomeRecord {
            input_id: interaction_id.0.to_string(),
            generation: expected_member.generation,
            fence_token: expected_member.fence_token,
            terminal_seq: 11,
            outcome: super::super::bridge_protocol::WireFlowTurnOutcome::InteractionComplete,
        };

        resolve_interaction_waiter_from_record(&waiters, &expected_member, &record)
            .expect("validated sidecar has canonical waiter correlation");

        assert!(matches!(
            waiter.await,
            Ok(RemoteInteractionTerminal::Complete)
        ));
        assert!(
            waiters.outstanding_for(&other_member),
            "the same interaction id at another residency remains isolated"
        );
        drop(unrelated);
    }

    #[tokio::test]
    async fn omitted_unclaimed_failure_preserves_bounded_detail_for_waiter() {
        let waiters = RemoteCompletionWaiters::default();
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("w-omitted-sidecar"));
        let expected_member = test_member(&runtime_id.identity, &runtime_id, FenceToken::new(7));
        let interaction_id = interaction(105);
        let waiter = waiters.register(&expected_member, interaction_id);
        let record = BridgeTurnOutcomeRecord {
            input_id: interaction_id.0.to_string(),
            generation: expected_member.generation,
            fence_token: expected_member.fence_token,
            terminal_seq: 12,
            outcome: super::super::bridge_protocol::WireFlowTurnOutcome::InteractionFailed {
                detail: meerkat_contracts::wire::supervisor_bridge::WireFlowFailureDetail {
                    text: "provider exploded".to_string(),
                    original_utf8_bytes: 200_000,
                    truncated: true,
                },
            },
        };

        resolve_interaction_waiter_from_record(&waiters, &expected_member, &record)
            .expect("bounded failure sidecar correlates exactly");

        match waiter.await {
            Ok(RemoteInteractionTerminal::Failed { reason }) => {
                assert!(reason.contains("provider exploded"));
                assert!(reason.contains("200000 UTF-8 bytes"));
            }
            other => panic!("omitted failure must resolve typed, got {other:?}"),
        }
    }

    #[test]
    fn restart_orphan_sidecar_is_safe_without_a_volatile_waiter() {
        let waiters = RemoteCompletionWaiters::default();
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("w-orphan-sidecar"));
        let expected_member = test_member(&runtime_id.identity, &runtime_id, FenceToken::new(7));
        let record = BridgeTurnOutcomeRecord {
            input_id: interaction(106).0.to_string(),
            generation: expected_member.generation,
            fence_token: expected_member.fence_token,
            terminal_seq: 13,
            outcome: super::super::bridge_protocol::WireFlowTurnOutcome::InteractionComplete,
        };

        resolve_interaction_waiter_from_record(&waiters, &expected_member, &record)
            .expect("a controller-restart orphan has no waiter to resolve");
        assert!(!waiters.outstanding_for(&expected_member));

        let non_interaction = BridgeTurnOutcomeRecord {
            input_id: "legacy-non-uuid".to_string(),
            outcome: super::super::bridge_protocol::WireFlowTurnOutcome::RunCompleted,
            ..record
        };
        resolve_interaction_waiter_from_record(&waiters, &expected_member, &non_interaction)
            .expect("non-interaction unclaimed rows need no waiter UUID");
    }

    #[tokio::test]
    async fn pending_unclaimed_ack_keeps_cursor_custody_after_waiter_resolution() {
        let manager = completion_test_manager("unclaimed-ack-cursor-custody").await;
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("w-ack-custody"));
        let expected_member = test_member(&runtime_id.identity, &runtime_id, FenceToken::new(7));
        let interaction_id = interaction(107);
        let waiter = manager.waiters.register(&expected_member, interaction_id);
        assert!(manager.has_cursor_custody(&expected_member, &[]));

        let record = BridgeTurnOutcomeRecord {
            input_id: interaction_id.0.to_string(),
            generation: expected_member.generation,
            fence_token: expected_member.fence_token,
            terminal_seq: 14,
            outcome: super::super::bridge_protocol::WireFlowTurnOutcome::InteractionComplete,
        };
        resolve_interaction_waiter_from_record(&manager.waiters, &expected_member, &record)
            .expect("validated sidecar resolves the live promise");
        assert!(matches!(
            waiter.await,
            Ok(RemoteInteractionTerminal::Complete)
        ));
        assert!(!manager.waiters.outstanding_for(&expected_member));

        let pending_ack = BridgeTurnOutcomeAck {
            generation: record.generation,
            fence_token: record.fence_token,
            input_id: record.input_id,
        };
        assert!(
            manager.has_cursor_custody(&expected_member, std::slice::from_ref(&pending_ack)),
            "removing the waiter cannot release the durable fold checkpoint before exact ACK"
        );
        assert!(
            !manager.has_cursor_custody(&expected_member, &[]),
            "the cursor may advance only after the host confirms the ACK"
        );
    }

    #[tokio::test]
    async fn waiter_registered_after_terminal_resolves_from_memory() {
        let waiters = RemoteCompletionWaiters::default();
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("w-e4"));
        let fence_token = FenceToken::new(1);
        let expected_member = test_member(&runtime_id.identity, &runtime_id, fence_token);

        // Defensive replay memory also covers a terminal that is re-observed
        // before a replacement completion task registers its exact waiter.
        waiters.resolve(
            &expected_member,
            interaction(2),
            &RemoteInteractionTerminal::Failed {
                reason: "scripted failure".to_string(),
            },
        );
        let waiter = waiters.register(&expected_member, interaction(2));
        match waiter.await {
            Ok(RemoteInteractionTerminal::Failed { reason }) => {
                assert_eq!(reason, "scripted failure");
            }
            other => panic!("late-registered waiter must resolve from memory, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn pre_registered_waiter_survives_more_than_terminal_memory_capacity() {
        let waiters = RemoteCompletionWaiters::default();
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("w-churn"));
        let fence_token = FenceToken::new(1);
        let expected_member = test_member(&runtime_id.identity, &runtime_id, fence_token);
        let target = interaction(5000);
        let waiter = waiters.register(&expected_member, target);

        for value in 1..=(RESOLVED_TERMINAL_MEMORY as u128 + 32) {
            waiters.resolve(
                &expected_member,
                interaction(value),
                &RemoteInteractionTerminal::Complete,
            );
        }
        waiters.resolve(
            &expected_member,
            target,
            &RemoteInteractionTerminal::Complete,
        );
        assert!(matches!(
            waiter.await,
            Ok(RemoteInteractionTerminal::Complete)
        ));
    }

    #[test]
    fn dropped_waiter_receiver_does_not_keep_pump_alive() {
        let waiters = RemoteCompletionWaiters::default();
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("w-aborted"));
        let fence_token = FenceToken::new(1);
        let expected_member = test_member(&runtime_id.identity, &runtime_id, fence_token);
        let receiver = waiters.register(&expected_member, interaction(9000));
        drop(receiver);

        assert!(
            !waiters.outstanding_for(&expected_member),
            "closed waiter senders are pruned from the pump liveness predicate"
        );
    }

    #[tokio::test]
    async fn fail_all_for_closes_waiters_as_infrastructure_loss() {
        let waiters = RemoteCompletionWaiters::default();
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("w-stop"));
        let other = AgentRuntimeId::initial(AgentIdentity::from("w-other"));
        let fence_token = FenceToken::new(1);
        let expected_member = test_member(&runtime_id.identity, &runtime_id, fence_token);
        let other_member = test_member(&other.identity, &other, fence_token);
        let waiter = waiters.register(&expected_member, interaction(10));
        let unrelated = waiters.register(&other_member, interaction(10));

        waiters.fail_all_for(&expected_member);
        assert!(
            waiter.await.is_err(),
            "pump stop must close the channel, not synthesize InteractionFailed"
        );
        assert!(
            waiters.outstanding_for(&other_member),
            "waiters for other runtime incarnations are untouched"
        );
        drop(unrelated);
    }

    /// A pump stop closes outstanding SubmitWork completion waiters promptly
    /// as infrastructure loss — never a synthetic member terminal or hang.
    #[tokio::test]
    async fn stop_pump_fails_outstanding_completion_waiters() {
        let authority = crate::store::SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let mob_id = crate::MobId::from("pump-stop-test");
        let bridge = Arc::new(
            super::super::MobSupervisorBridge::new(&mob_id, authority, None)
                .await
                .expect("supervisor bridge builds"),
        );
        let store: Arc<dyn MobRuntimeMetadataStore> =
            Arc::new(crate::store::InMemoryMobRuntimeMetadataStore::new());
        let manager = Arc::new(MemberEventPumpManager::new(
            mob_id,
            bridge,
            store,
            accepting_host_runtime_observer(),
        ));

        let identity = AgentIdentity::from("w-stop");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let expected_member = test_member(&identity, &runtime_id, FenceToken::new(1));
        manager
            .ensure_pump(MemberPumpMaterial {
                agent_identity: identity.clone(),
                host_id: TEST_HOST_ID.to_string(),
                expected_member: expected_member.clone(),
                runtime_id: runtime_id.clone(),
                fence_token: FenceToken::new(1),
                role: ProfileName::from("worker"),
                peer: meerkat_core::comms::TrustedPeerDescriptor {
                    peer_id: meerkat_core::comms::PeerId::new(),
                    name: meerkat_core::comms::PeerName::new("w-stop").expect("valid peer name"),
                    address: meerkat_core::comms::PeerAddress::new(
                        meerkat_core::comms::PeerTransport::Inproc,
                        "w-stop",
                    ),
                    pubkey: [0u8; 32],
                },
            })
            .await;
        let waiter = manager
            .completion_waiters()
            .register(&expected_member, interaction(11));

        manager.stop_pump(&identity).await;
        match tokio::time::timeout(Duration::from_secs(5), waiter).await {
            Ok(Err(_)) => {}
            other => panic!("stop_pump must close the registered waiter promptly, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn promotion_barrier_fails_old_flow_ticket_without_starting_new_pump() {
        let manager = completion_test_manager("pump-promotion-ticket").await;
        let registry = Arc::new(RemoteFlowTicketRegistry::new(Arc::new(
            TestOutcomeResolver::default(),
        )));
        let identity = AgentIdentity::from("w-promoted");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let old_member = install_test_pump(
            &manager,
            &identity,
            &runtime_id,
            FenceToken::new(7),
            1,
            completion_test_peer("w-promoted", "127.0.0.1:4040"),
        );
        manager
            .pump_tasks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(1, tokio::spawn(std::future::pending::<()>()));
        manager.install_outcome_sink(Arc::clone(&registry) as Arc<dyn RemoteTurnOutcomeSink>);
        let receiver = registry.arm_exact(
            &identity,
            "old-input",
            &RunId::new(),
            &StepId::from("s1"),
            &old_member,
        );
        let promoted_member = BridgeMemberIncarnation {
            binding_generation: old_member.binding_generation + 1,
            ..old_member.clone()
        };

        manager
            .stop_exact_pump_and_join(&identity, &old_member, &promoted_member)
            .await
            .expect("promotion barrier joins G1 and installs G2");

        match receiver.await.expect("G1 ticket fails at the barrier") {
            crate::runtime::turn_executor::FlowTurnOutcome::Failed { reason, .. } => {
                assert_eq!(
                    reason,
                    crate::runtime::remote_flow_ticket::REMATERIALIZED_STEP_FAILURE_REASON
                );
            }
            other => panic!("expected typed residency-change failure, got {other:?}"),
        }
        assert!(
            !manager
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pumps
                .contains_key(&identity),
            "the barrier does not start a replacement pump"
        );
    }

    #[tokio::test]
    async fn same_generation_higher_fence_replaces_in_flight_pump() {
        let authority = crate::store::SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let mob_id = crate::MobId::from("pump-fence-test");
        let bridge = Arc::new(
            super::super::MobSupervisorBridge::new(&mob_id, authority, None)
                .await
                .expect("supervisor bridge builds"),
        );
        let store: Arc<dyn MobRuntimeMetadataStore> =
            Arc::new(crate::store::InMemoryMobRuntimeMetadataStore::new());
        let manager = Arc::new(MemberEventPumpManager::new(
            mob_id,
            bridge,
            store,
            accepting_host_runtime_observer(),
        ));
        let stale_incarnation = manager
            .next_incarnation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let identity = AgentIdentity::from("w-fenced");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let stale_cancel = CancellationToken::new();
        let stale_member = test_member(&identity, &runtime_id, FenceToken::new(7));
        let replacement_member = test_member(&identity, &runtime_id, FenceToken::new(8));
        let peer = meerkat_core::comms::TrustedPeerDescriptor {
            peer_id: meerkat_core::comms::PeerId::new(),
            name: meerkat_core::comms::PeerName::new("w-fenced").expect("valid peer name"),
            address: meerkat_core::comms::PeerAddress::new(
                meerkat_core::comms::PeerTransport::Inproc,
                "w-fenced",
            ),
            pubkey: [0u8; 32],
        };
        manager
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pumps
            .insert(
                identity.clone(),
                PumpEntry {
                    cancel: stale_cancel.clone(),
                    incarnation: stale_incarnation,
                    expected_member: stale_member,
                    exiting: false,
                    runtime_id: runtime_id.clone(),
                    peer: peer.clone(),
                    taps: Vec::new(),
                    obligation_keepalive: true,
                },
            );

        let replacement_material = MemberPumpMaterial {
            agent_identity: identity.clone(),
            host_id: TEST_HOST_ID.to_string(),
            expected_member: replacement_member.clone(),
            runtime_id: runtime_id.clone(),
            fence_token: FenceToken::new(8),
            role: ProfileName::from("worker"),
            peer,
        };
        manager.ensure_pump(replacement_material.clone()).await;

        assert!(stale_cancel.is_cancelled(), "old-fence poll is canceled");
        let state = manager
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let replacement = state.pumps.get(&identity).expect("replacement pump");
        assert_eq!(replacement.runtime_id, runtime_id);
        assert_eq!(replacement.expected_member, replacement_member);
        assert_ne!(replacement.incarnation, stale_incarnation);
        drop(state);
        manager.stop_all_and_join().await;
        assert!(
            manager
                .pump_tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "stop barrier joins and removes every tracked pump task"
        );
        manager.ensure_pump(replacement_material).await;
        assert!(
            manager
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pumps
                .is_empty(),
            "shutdown gate forbids a pump from being created after the join barrier"
        );
    }

    #[tokio::test]
    async fn same_tuple_address_refresh_replaces_stale_transport_pump() {
        let authority = crate::store::SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let mob_id = crate::MobId::from("pump-address-refresh-test");
        let bridge = Arc::new(
            super::super::MobSupervisorBridge::new(&mob_id, authority, None)
                .await
                .expect("supervisor bridge builds"),
        );
        let store: Arc<dyn MobRuntimeMetadataStore> =
            Arc::new(crate::store::InMemoryMobRuntimeMetadataStore::new());
        let manager = Arc::new(MemberEventPumpManager::new(
            mob_id,
            bridge,
            store,
            accepting_host_runtime_observer(),
        ));
        let stale_incarnation = manager
            .next_incarnation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let identity = AgentIdentity::from("w-refreshed");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let stale_cancel = CancellationToken::new();
        let expected_member = test_member(&identity, &runtime_id, FenceToken::new(7));
        let stale_peer = meerkat_core::comms::TrustedPeerDescriptor {
            peer_id: meerkat_core::comms::PeerId::new(),
            name: meerkat_core::comms::PeerName::new("w-refreshed").expect("valid peer name"),
            address: meerkat_core::comms::PeerAddress::new(
                meerkat_core::comms::PeerTransport::Tcp,
                "127.0.0.1:4001",
            ),
            pubkey: [3u8; 32],
        };
        let mut refreshed_peer = stale_peer.clone();
        refreshed_peer.address = meerkat_core::comms::PeerAddress::new(
            meerkat_core::comms::PeerTransport::Tcp,
            "127.0.0.1:4002",
        );
        manager
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pumps
            .insert(
                identity.clone(),
                PumpEntry {
                    cancel: stale_cancel.clone(),
                    incarnation: stale_incarnation,
                    expected_member: expected_member.clone(),
                    exiting: false,
                    runtime_id: runtime_id.clone(),
                    peer: stale_peer,
                    taps: Vec::new(),
                    obligation_keepalive: true,
                },
            );

        manager
            .ensure_pump(MemberPumpMaterial {
                agent_identity: identity.clone(),
                host_id: TEST_HOST_ID.to_string(),
                expected_member,
                runtime_id,
                fence_token: FenceToken::new(7),
                role: ProfileName::from("worker"),
                peer: refreshed_peer.clone(),
            })
            .await;

        assert!(
            stale_cancel.is_cancelled(),
            "stale-address poll is canceled"
        );
        let state = manager
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let replacement = state.pumps.get(&identity).expect("replacement pump");
        assert_eq!(replacement.peer, refreshed_peer);
        assert_ne!(replacement.incarnation, stale_incarnation);
        drop(state);
        manager.stop_all_and_join().await;
        assert!(
            manager
                .pump_tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "stop barrier joins and removes every tracked pump task"
        );
    }

    #[tokio::test]
    async fn ensure_replaces_exact_tuple_whose_task_is_exiting() {
        let manager = completion_test_manager("pump-exit-ensure-race").await;
        let identity = AgentIdentity::from("w-exit-race");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let fence_token = FenceToken::new(7);
        let peer = completion_test_peer("w-exit-race", "127.0.0.1:4027");
        let stale_incarnation = manager
            .next_incarnation
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let expected_member = install_test_pump(
            &manager,
            &identity,
            &runtime_id,
            fence_token,
            stale_incarnation,
            peer.clone(),
        );
        {
            let mut state = manager
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state
                .pumps
                .get_mut(&identity)
                .expect("installed pump")
                .exiting = true;
        }

        manager
            .ensure_pump(MemberPumpMaterial {
                agent_identity: identity.clone(),
                host_id: TEST_HOST_ID.to_string(),
                expected_member: expected_member.clone(),
                runtime_id,
                fence_token,
                role: ProfileName::from("worker"),
                peer,
            })
            .await;

        {
            let state = manager
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let replacement = state.pumps.get(&identity).expect("replacement pump");
            assert_eq!(replacement.expected_member, expected_member);
            assert_ne!(replacement.incarnation, stale_incarnation);
            assert!(
                !replacement.exiting,
                "ensure must publish a live replacement instead of accepting the dying lease"
            );
        }
        manager.stop_all_and_join().await;
    }

    #[tokio::test]
    async fn same_residency_replacement_rewinds_fold_before_cursor_replay() {
        let manager = completion_test_manager("pump-refresh-fold-rewind").await;
        let sink = Arc::new(RecordingOutcomeSink::default());
        manager.install_outcome_sink(Arc::clone(&sink) as Arc<dyn RemoteTurnOutcomeSink>);
        let identity = AgentIdentity::from("w-refresh-fold");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let fence_token = FenceToken::new(7);
        let expected_member = install_test_pump(
            &manager,
            &identity,
            &runtime_id,
            fence_token,
            1,
            completion_test_peer("w-refresh-fold", "127.0.0.1:4028"),
        );
        let refreshed_peer = completion_test_peer("w-refresh-fold", "127.0.0.1:4029");

        manager
            .ensure_pump(MemberPumpMaterial {
                agent_identity: identity,
                host_id: TEST_HOST_ID.to_string(),
                expected_member,
                runtime_id,
                fence_token,
                role: ProfileName::from("worker"),
                peer: refreshed_peer,
            })
            .await;

        assert_eq!(
            sink.rewinds.load(Ordering::SeqCst),
            1,
            "the replacement reloads the durable cursor, so its rebuildable fold must rewind once"
        );
        manager.stop_all_and_join().await;
    }

    #[tokio::test]
    async fn saturated_idle_long_polls_cannot_block_the_reserved_custody_lane() {
        let manager = completion_test_manager("pump-priority-fairness").await;
        let identity = AgentIdentity::from("w-priority-fairness");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let expected_member = install_test_pump(
            &manager,
            &identity,
            &runtime_id,
            FenceToken::new(7),
            1,
            completion_test_peer("w-priority-fairness", "127.0.0.1:4033"),
        );
        assert_eq!(
            manager.poll_class(&identity, &expected_member),
            MemberPollClass::Observation
        );
        manager.mark_obligation_keepalive(&identity);
        assert_eq!(
            manager.poll_class(&identity, &expected_member),
            MemberPollClass::Priority,
            "an obligated member is routed away from the idle long-poll queue"
        );
        manager.clear_obligation_keepalive(&identity, &expected_member, 1);
        let waiter = manager
            .remote_completion_context(&identity, &expected_member)
            .expect("exact pump completion lease")
            .register(interaction(58))
            .expect("completion waiter registration");
        assert_eq!(
            manager.poll_class(&identity, &expected_member),
            MemberPollClass::Priority,
            "a completion waiter independently selects the reserved lane"
        );
        let mut idle_permits = Vec::new();
        for _ in 0..OBSERVATION_MEMBER_POLL_PERMITS {
            idle_permits.push(
                Arc::clone(&manager.observation_poll_permits)
                    .acquire_owned()
                    .await
                    .expect("observation permit"),
            );
        }
        assert!(
            Arc::clone(&manager.observation_poll_permits)
                .try_acquire_owned()
                .is_err(),
            "the idle observation lane is deliberately saturated"
        );

        let priority_permit = Arc::clone(&manager.priority_poll_permits)
            .try_acquire_owned()
            .expect("idle long polls cannot queue ahead of the reserved custody lane");
        assert_eq!(
            manager.poll_lane(MemberPollClass::Priority).1,
            PRIORITY_POLL_WAIT_MS,
            "custody polls hold the reserved permit only for the short window"
        );
        assert_eq!(
            OBSERVATION_MEMBER_POLL_PERMITS + PRIORITY_MEMBER_POLL_PERMITS,
            MAX_CONCURRENT_MEMBER_POLLS,
            "reserved fairness must not raise the fixed per-mob resource ceiling"
        );
        drop(priority_permit);
        drop(idle_permits);
        drop(waiter);
    }

    #[tokio::test]
    async fn blackholed_host_fanout_cannot_monopolize_global_custody_queue() {
        let manager = completion_test_manager("pump-host-priority-fairness").await;
        let host_a_lane = manager.host_poll_lane(MemberPollClass::Priority, "host-a");
        let host_a_permit = Arc::clone(&host_a_lane)
            .acquire_owned()
            .await
            .expect("first Host A contender enters its host lane");
        let host_a_global = Arc::clone(&manager.priority_poll_permits)
            .acquire_owned()
            .await
            .expect("Host A occupies one global custody slot");

        for _ in 0..512 {
            let same_host_lane = manager.host_poll_lane(MemberPollClass::Priority, "host-a");
            assert!(Arc::ptr_eq(&host_a_lane, &same_host_lane));
            assert!(
                same_host_lane.try_acquire_owned().is_err(),
                "every additional Host A member remains behind the one-per-host gate"
            );
        }

        let host_b_lane = manager.host_poll_lane(MemberPollClass::Priority, "host-b");
        let host_b_permit = Arc::clone(&host_b_lane)
            .try_acquire_owned()
            .expect("healthy Host B enters an independent host lane");
        let host_b_global = Arc::clone(&manager.priority_poll_permits)
            .try_acquire_owned()
            .expect("Host A cannot queue into Host B's reserved global slot");
        assert!(
            Arc::clone(&manager.priority_poll_permits)
                .try_acquire_owned()
                .is_err(),
            "the per-host gate preserves the fixed two-slot global custody ceiling"
        );

        drop(host_a_global);
        drop(host_b_global);
        drop(host_a_permit);
        drop(host_b_permit);
        drop(host_a_lane);
        drop(host_b_lane);
        manager.prune_host_poll_lanes();
        assert!(
            manager
                .priority_host_poll_lanes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "weak host lanes are removed after their final contender exits"
        );
    }

    #[tokio::test]
    async fn blackholed_observation_host_cannot_starve_healthy_host() {
        let manager = completion_test_manager("pump-host-observation-fairness").await;
        let host_a_lane = manager.host_poll_lane(MemberPollClass::Observation, "host-a");
        let host_a_permit = Arc::clone(&host_a_lane)
            .acquire_owned()
            .await
            .expect("first Host A observation enters its host lane");
        let host_a_global = Arc::clone(&manager.observation_poll_permits)
            .acquire_owned()
            .await
            .expect("Host A occupies one observation slot");

        let (entered_tx, mut entered_rx) = mpsc::unbounded_channel();
        let mut host_a_tasks = Vec::new();
        for _ in 0..128 {
            let manager = Arc::clone(&manager);
            let entered_tx = entered_tx.clone();
            host_a_tasks.push(tokio::spawn(async move {
                let _host = manager
                    .host_poll_lane(MemberPollClass::Observation, "host-a")
                    .acquire_owned()
                    .await
                    .expect("Host A lane remains open");
                let _global = Arc::clone(&manager.observation_poll_permits)
                    .acquire_owned()
                    .await
                    .expect("global observation lane remains open");
                let _ = entered_tx.send("host-a");
            }));
        }
        tokio::task::yield_now().await;
        assert!(
            matches!(entered_rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "Host A fan-out cannot queue past its one-per-host gate"
        );

        let manager_b = Arc::clone(&manager);
        let entered_b = entered_tx.clone();
        let host_b_task = tokio::spawn(async move {
            let _host = manager_b
                .host_poll_lane(MemberPollClass::Observation, "host-b")
                .acquire_owned()
                .await
                .expect("Host B lane remains open");
            let _global = Arc::clone(&manager_b.observation_poll_permits)
                .acquire_owned()
                .await
                .expect("Host B reaches a global observation slot");
            let _ = entered_b.send("host-b");
        });
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), entered_rx.recv())
                .await
                .expect("healthy Host B is not queued behind Host A")
                .expect("Host B task reports entry"),
            "host-b"
        );
        host_b_task.await.expect("Host B task joins");

        for task in &host_a_tasks {
            task.abort();
        }
        for task in host_a_tasks {
            let _ = task.await;
        }
        drop(host_a_global);
        drop(host_a_permit);
        drop(host_a_lane);
        let released = manager
            .host_poll_lane(MemberPollClass::Observation, "host-a")
            .try_acquire_owned()
            .expect("cancelling queued contenders releases Host A's lane");
        drop(released);
        manager.prune_host_poll_lanes();
    }

    #[tokio::test]
    async fn waiter_key_is_scoped_by_runtime_incarnation() {
        let waiters = RemoteCompletionWaiters::default();
        let identity = AgentIdentity::from("w-e4");
        let gen1 = AgentRuntimeId::initial(identity.clone());
        let gen2 = AgentRuntimeId::new(
            identity,
            gen1.generation
                .next()
                .expect("test generation has a successor"),
        );

        // A terminal from the OLD incarnation never resolves a waiter keyed
        // to the new one (generation rides the runtime id).
        let fence_token = FenceToken::new(1);
        let gen1_member = test_member(&gen1.identity, &gen1, fence_token);
        let gen2_member = test_member(&gen2.identity, &gen2, fence_token);
        let waiter = waiters.register(&gen2_member, interaction(3));
        waiters.resolve(
            &gen1_member,
            interaction(3),
            &RemoteInteractionTerminal::Complete,
        );
        assert!(
            waiters.outstanding_for(&gen2_member),
            "cross-incarnation terminals must not resolve the waiter"
        );
        drop(waiter);
    }

    #[tokio::test]
    async fn waiter_key_is_scoped_by_authoritative_host() {
        let waiters = RemoteCompletionWaiters::default();
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("w-host-waiter"));
        let fence_token = FenceToken::new(7);
        let interaction_id = interaction(33);
        let host_a = test_member(&runtime_id.identity, &runtime_id, fence_token);
        let host_b = BridgeMemberIncarnation {
            host_id: "host-b".to_string(),
            ..host_a.clone()
        };
        let waiter = waiters.register(&host_b, interaction_id);

        waiters.resolve(
            &host_a,
            interaction_id,
            &RemoteInteractionTerminal::Complete,
        );
        assert!(
            waiters.outstanding_for(&host_b),
            "an old host terminal cannot resolve the moved host's waiter"
        );
        drop(waiter);
    }

    #[tokio::test]
    async fn waiter_key_is_scoped_by_binding_generation_and_member_session() {
        let waiters = RemoteCompletionWaiters::default();
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("w-residency-waiter"));
        let fence_token = FenceToken::new(7);
        let current = test_member_with_residency(
            &runtime_id.identity,
            &runtime_id,
            fence_token,
            2,
            "session-b",
        );
        let old_binding = BridgeMemberIncarnation {
            binding_generation: 1,
            ..current.clone()
        };
        let old_session = BridgeMemberIncarnation {
            member_session_id: "session-a".to_string(),
            ..current.clone()
        };
        let interaction_id = interaction(34);
        let waiter = waiters.register(&current, interaction_id);

        waiters.resolve(
            &old_binding,
            interaction_id,
            &RemoteInteractionTerminal::Complete,
        );
        waiters.resolve(
            &old_session,
            interaction_id,
            &RemoteInteractionTerminal::Complete,
        );
        assert!(
            waiters.outstanding_for(&current),
            "old binding generation and old session terminals cannot cross residency"
        );
        waiters.resolve(
            &current,
            interaction_id,
            &RemoteInteractionTerminal::Complete,
        );
        assert!(matches!(
            waiter.await,
            Ok(RemoteInteractionTerminal::Complete)
        ));
    }

    #[tokio::test]
    async fn old_pump_exit_cannot_fail_or_resolve_same_generation_new_fence_waiter() {
        let waiters = RemoteCompletionWaiters::default();
        let runtime_id = AgentRuntimeId::initial(AgentIdentity::from("w-fence-waiter"));
        let old_fence = FenceToken::new(7);
        let new_fence = FenceToken::new(8);
        let interaction_id = interaction(4);
        let old_member = test_member(&runtime_id.identity, &runtime_id, old_fence);
        let new_member = test_member(&runtime_id.identity, &runtime_id, new_fence);
        let waiter = waiters.register(&new_member, interaction_id);

        waiters.fail_all_for(&old_member);
        waiters.resolve(
            &old_member,
            interaction_id,
            &RemoteInteractionTerminal::Failed {
                reason: "stale old-fence terminal".to_string(),
            },
        );
        assert!(
            waiters.outstanding_for(&new_member),
            "old pump exit and stale terminal must not touch the new-fence waiter"
        );

        waiters.resolve(
            &new_member,
            interaction_id,
            &RemoteInteractionTerminal::Complete,
        );
        assert!(matches!(
            waiter.await,
            Ok(RemoteInteractionTerminal::Complete)
        ));
    }

    #[tokio::test]
    async fn delayed_completion_registration_after_pump_exit_fails_promptly() {
        let manager = completion_test_manager("delayed-waiter-lease").await;
        let identity = AgentIdentity::from("w-delayed");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let fence_token = FenceToken::new(7);
        let expected_member = install_test_pump(
            &manager,
            &identity,
            &runtime_id,
            fence_token,
            1,
            completion_test_peer("w-delayed", "127.0.0.1:4010"),
        );
        let wrong_host = BridgeMemberIncarnation {
            host_id: "host-b".to_string(),
            ..expected_member.clone()
        };
        assert!(
            manager
                .remote_completion_context(&identity, &wrong_host)
                .is_none(),
            "same runtime/fence on the wrong authoritative host cannot mint a waiter lease"
        );
        let context = manager
            .remote_completion_context(&identity, &expected_member)
            .expect("active exact pump mints a completion lease");

        manager.remove_entry(&identity, 1);
        manager.fail_waiters_if_no_matching_pump(&identity, &expected_member);
        assert_eq!(
            context
                .register(interaction(5))
                .expect_err("a delayed receipt cannot register after the final fail sweep"),
            "member event pump stopped before completion waiter registration"
        );
    }

    #[tokio::test]
    async fn dropped_production_context_waiter_releases_pump_liveness() {
        let manager = completion_test_manager("dropped-context-waiter").await;
        let identity = AgentIdentity::from("w-dropped-context");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let fence_token = FenceToken::new(7);
        let expected_member = install_test_pump(
            &manager,
            &identity,
            &runtime_id,
            fence_token,
            1,
            completion_test_peer("w-dropped-context", "127.0.0.1:4015"),
        );
        let context = manager
            .remote_completion_context(&identity, &expected_member)
            .expect("active exact pump mints a completion context");
        let receiver = context
            .register(interaction(55))
            .expect("production context registers through the atomic pump lease");
        drop(receiver);

        assert!(
            !manager.is_live(&identity, &expected_member),
            "an aborted completion task cannot keep the production pump alive"
        );
    }

    #[tokio::test]
    async fn idle_exit_is_linearized_against_completion_waiter_registration() {
        let manager = completion_test_manager("idle-exit-waiter-linearization").await;
        let identity = AgentIdentity::from("w-idle-exit");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let expected_member = install_test_pump(
            &manager,
            &identity,
            &runtime_id,
            FenceToken::new(7),
            1,
            completion_test_peer("w-idle-exit", "127.0.0.1:4017"),
        );
        let context = manager
            .remote_completion_context(&identity, &expected_member)
            .expect("active pump lease");
        let first = interaction(56);
        let receiver = context.register(first).expect("waiter registers first");
        assert!(
            !manager.begin_idle_exit(&identity, &expected_member, 1),
            "a waiter registered before the idle decision keeps the pump live"
        );
        drop(receiver);

        assert!(manager.begin_idle_exit(&identity, &expected_member, 1));
        assert_eq!(
            context
                .register(interaction(57))
                .expect_err("a waiter cannot register after the exact idle-exit decision"),
            "member event pump stopped before completion waiter registration"
        );
    }

    #[tokio::test]
    async fn same_tuple_address_refresh_transfers_completion_waiter() {
        let manager = completion_test_manager("same-tuple-waiter-transfer").await;
        let identity = AgentIdentity::from("w-transfer");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let fence_token = FenceToken::new(7);
        let expected_member = install_test_pump(
            &manager,
            &identity,
            &runtime_id,
            fence_token,
            1,
            completion_test_peer("w-transfer", "127.0.0.1:4020"),
        );
        let context = manager
            .remote_completion_context(&identity, &expected_member)
            .expect("old pump lease");
        let waiter = context
            .register(interaction(6))
            .expect("old exact pump accepts waiter");

        let replacement_member = install_test_pump(
            &manager,
            &identity,
            &runtime_id,
            fence_token,
            2,
            completion_test_peer("w-transfer", "127.0.0.1:4021"),
        );
        assert_eq!(replacement_member, expected_member);
        manager.fail_waiters_if_no_matching_pump(&identity, &expected_member);
        assert!(manager.waiters.outstanding_for(&expected_member));
        manager.waiters.resolve(
            &expected_member,
            interaction(6),
            &RemoteInteractionTerminal::Complete,
        );
        assert!(matches!(
            waiter.await,
            Ok(RemoteInteractionTerminal::Complete)
        ));
    }

    #[tokio::test]
    async fn different_fence_replacement_fails_only_old_completion_waiter() {
        let manager = completion_test_manager("different-fence-waiter-failure").await;
        let identity = AgentIdentity::from("w-fence-transfer");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let old_fence = FenceToken::new(7);
        let new_fence = FenceToken::new(8);
        let old_member = install_test_pump(
            &manager,
            &identity,
            &runtime_id,
            old_fence,
            1,
            completion_test_peer("w-fence-transfer", "127.0.0.1:4030"),
        );
        let old_context = manager
            .remote_completion_context(&identity, &old_member)
            .expect("old pump lease");
        let old_waiter = old_context
            .register(interaction(7))
            .expect("old pump accepts waiter");

        let new_member = install_test_pump(
            &manager,
            &identity,
            &runtime_id,
            new_fence,
            2,
            completion_test_peer("w-fence-transfer", "127.0.0.1:4031"),
        );
        manager.fail_waiters_if_no_matching_pump(&identity, &old_member);
        assert!(
            old_waiter.await.is_err(),
            "different fence must close old waiter as custody loss"
        );
        assert!(!manager.waiters.outstanding_for(&old_member));
        assert!(
            manager
                .remote_completion_context(&identity, &new_member)
                .is_some(),
            "new-fence pump remains independently leasable"
        );
    }

    #[tokio::test]
    async fn stale_in_flight_binding_page_cannot_mutate_fanout_cursor_or_ack() {
        let manager = completion_test_manager("stale-binding-page").await;
        let identity = AgentIdentity::from("w-stale-page");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let old_member =
            test_member_with_residency(&identity, &runtime_id, FenceToken::new(7), 1, "session-a");
        let current_member = BridgeMemberIncarnation {
            binding_generation: 2,
            ..old_member.clone()
        };
        let peer = completion_test_peer("w-stale-page", "127.0.0.1:4050");
        let (tap, mut events) = mpsc::channel(1);
        manager
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pumps
            .insert(
                identity.clone(),
                PumpEntry {
                    cancel: CancellationToken::new(),
                    incarnation: 2,
                    expected_member: current_member.clone(),
                    exiting: false,
                    runtime_id: runtime_id.clone(),
                    peer,
                    taps: vec![tap],
                    obligation_keepalive: false,
                },
            );
        let resolver = Arc::new(TestOutcomeResolver::default());
        let registry = Arc::new(RemoteFlowTicketRegistry::new(
            Arc::clone(&resolver) as Arc<dyn RemoteTurnObligationResolver>
        ));
        manager.install_outcome_sink(Arc::clone(&registry) as Arc<dyn RemoteTurnOutcomeSink>);
        let mut receiver = registry.arm_exact(
            &identity,
            "stale-input",
            &RunId::new(),
            &StepId::from("s1"),
            &current_member,
        );
        let stale_event = AgentEvent::RunCompleted {
            session_id: meerkat_core::types::SessionId::new(),
            result: "stale".to_string(),
            structured_output: None,
            extraction_required: false,
            usage: meerkat_core::Usage::default(),
            terminal_cause_kind: None,
        };
        registry.observe_member_event_row_exact(&old_member, 5, &stale_event);
        let stale_record = BridgeTurnOutcomeRecord {
            input_id: "stale-input".to_string(),
            generation: old_member.generation,
            fence_token: old_member.fence_token,
            terminal_seq: 5,
            outcome: super::super::bridge_protocol::WireFlowTurnOutcome::RunCompleted,
        };
        assert_eq!(
            registry
                .consume_turn_outcome_record(&old_member, &stale_record)
                .await
                .expect("stale record is ignored"),
            OutcomeRecordDisposition::Retain
        );
        registry
            .acknowledge_turn_outcome(
                &old_member,
                &BridgeTurnOutcomeAck {
                    generation: old_member.generation,
                    fence_token: old_member.fence_token,
                    input_id: "stale-input".to_string(),
                },
            )
            .await
            .expect("stale ACK is ignored");
        assert_eq!(resolver.resolved.load(Ordering::SeqCst), 0);
        assert_eq!(resolver.acknowledged.load(Ordering::SeqCst), 0);
        assert!(matches!(
            receiver.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));

        manager.fan_out(
            &identity,
            &old_member,
            1,
            &AttributedEvent {
                source: runtime_id.clone(),
                source_fence_token: FenceToken::new(old_member.fence_token),
                role: ProfileName::from("worker"),
                envelope: EventEnvelope::new(
                    runtime_id.to_string(),
                    5,
                    Some(manager.mob_id.to_string()),
                    stale_event,
                ),
            },
        );
        assert!(matches!(
            events.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        let stale_cursor = MobMemberEventCursorRecord {
            agent_identity: identity.to_string(),
            host_id: old_member.host_id.clone(),
            host_binding_generation: Some(old_member.binding_generation),
            member_session_id: old_member.member_session_id.clone(),
            generation: old_member.generation,
            fence_token: Some(old_member.fence_token),
            next_seq: 6,
        };
        assert!(
            !manager
                .persist_cursor_if_current(&identity, &old_member, 1, &stale_cursor)
                .await
                .expect("stale cursor validation")
        );
        assert!(
            manager
                .store
                .list_member_event_cursors(&manager.mob_id)
                .await
                .expect("list cursors")
                .is_empty(),
            "stale page cannot persist a cursor in the replacement residency"
        );
    }

    #[tokio::test]
    async fn remote_cursor_gaps_are_fanned_out_as_typed_truncation_events() {
        let authority = crate::store::SupervisorAuthorityRecord::generate(
            meerkat_contracts::wire::supervisor_bridge::supervisor_bridge_current_protocol_version(
            ),
        );
        let mob_id = crate::MobId::from("pump-gap-test");
        let bridge = Arc::new(
            super::super::MobSupervisorBridge::new(&mob_id, authority, None)
                .await
                .expect("supervisor bridge builds"),
        );
        let store: Arc<dyn MobRuntimeMetadataStore> =
            Arc::new(crate::store::InMemoryMobRuntimeMetadataStore::new());
        let manager =
            MemberEventPumpManager::new(mob_id, bridge, store, accepting_host_runtime_observer());
        let identity = AgentIdentity::from("w-gap");
        let runtime_id = AgentRuntimeId::initial(identity.clone());
        let expected_member = test_member(&identity, &runtime_id, FenceToken::new(7));
        let peer = meerkat_core::comms::TrustedPeerDescriptor {
            peer_id: meerkat_core::comms::PeerId::new(),
            name: meerkat_core::comms::PeerName::new("w-gap").expect("valid peer name"),
            address: meerkat_core::comms::PeerAddress::new(
                meerkat_core::comms::PeerTransport::Inproc,
                "w-gap",
            ),
            pubkey: [0u8; 32],
        };
        let (tap, mut events) = mpsc::channel(2);
        manager
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pumps
            .insert(
                identity.clone(),
                PumpEntry {
                    cancel: CancellationToken::new(),
                    incarnation: 1,
                    expected_member: expected_member.clone(),
                    exiting: false,
                    runtime_id: runtime_id.clone(),
                    peer: peer.clone(),
                    taps: vec![tap],
                    obligation_keepalive: false,
                },
            );
        let material = MemberPumpMaterial {
            agent_identity: identity,
            host_id: TEST_HOST_ID.to_string(),
            expected_member,
            runtime_id,
            fence_token: FenceToken::new(7),
            role: ProfileName::from("worker"),
            peer,
        };

        manager.fan_out_truncation(
            &material,
            1,
            42,
            StreamTruncationReason::RemoteCursorOverrun { watermark: 41 },
        );
        manager.fan_out_truncation(
            &material,
            1,
            99,
            StreamTruncationReason::OversizedRemoteEvent {
                durable_seq: 99,
                encoded_bytes: 900_000,
                max_bytes: 524_288,
            },
        );

        let first = events.recv().await.expect("overrun marker");
        assert!(matches!(
            first.envelope.payload,
            AgentEvent::StreamTruncated {
                reason: StreamTruncationReason::RemoteCursorOverrun { watermark: 41 }
            }
        ));
        let second = events.recv().await.expect("oversized marker");
        assert!(matches!(
            second.envelope.payload,
            AgentEvent::StreamTruncated {
                reason: StreamTruncationReason::OversizedRemoteEvent {
                    durable_seq: 99,
                    encoded_bytes: 900_000,
                    max_bytes: 524_288,
                }
            }
        ));
    }
}
