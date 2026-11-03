//! Operator-instruction delivery: the shell around MobMachine's
//! operator-instruction region.
//!
//! MobMachine owns every lifecycle fact (the records, the generation, each
//! member's owed, recorded and blocked facts); this module realizes what it
//! authorizes and reads what it holds:
//!
//! - the bodies of the Active records, by digest, rebuilt from the journal;
//! - a member's snapshot at the machine's generation (every record, Active
//!   for the member when its target selects the member);
//! - the snapshot a member's new session carries at birth;
//! - the delivery owner, which asks the machine to prepare each owed
//!   member's delivery and records it in the member's session: through the
//!   session's actor while it is live, store-only while it is dormant, never
//!   waking or materializing the member. Live and dormant deliveries run in
//!   separate bounded lanes, so a live session that holds its turn boundary
//!   never stalls the dormant fan-out;
//! - the status read.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures::FutureExt as _;
use futures::stream::{FuturesUnordered, StreamExt};
use meerkat_core::service::{SessionInstructionError, SessionInstructionRead};
use meerkat_core::time_compat::{Duration, Instant};
use meerkat_core::types::SessionId;
use meerkat_core::{
    InstructionContentDigest, InstructionRevision, InstructionSlot, InstructionTargetRecord,
    InstructionTargetRecordSlot, InstructionWriteError,
};

use super::handle::MobHandle;
use super::state::MobCommand;
use crate::error::MobError;
use crate::event::{MobEvent, MobEventKind};
use crate::ids::{AgentIdentity, MobId, ProfileName};
use crate::machines::mob_machine as mob_dsl;
use crate::operator_instruction::{
    MemberInstructionBlockReason, MemberInstructionDelivery, MemberInstructionDeliveryState,
    MemberInstructionDeliveryStatus, MemberInstructionEffectiveRead,
    MemberInstructionEffectiveSlot, MemberInstructionPendingReason, MemberInstructionResidency,
    MemberInstructionSlotState, MemberInstructionSlotStatus, MemberInstructionSlotView,
    MemberInstructionStatus, OPERATOR_INSTRUCTION_STATUS_DEFAULT_LIMIT,
    OPERATOR_INSTRUCTION_STATUS_MAX_LIMIT, OperatorInstructionError, OperatorInstructionReceipt,
    OperatorInstructionRecordStatus, OperatorInstructionStatusPage, OperatorInstructionStatusQuery,
    OperatorInstructionTarget, OperatorInstructionWrite,
};
#[cfg(target_arch = "wasm32")]
use crate::tokio;

/// The default bound on concurrent dormant (store-only) deliveries
/// ([`super::MobBuilder::with_member_instruction_delivery_concurrency`]).
pub(crate) const DEFAULT_DORMANT_DELIVERY_CONCURRENCY: usize = 16;
/// The bound on concurrent deliveries through live member sessions. Each one
/// waits for its session's turn boundary, so the lane is separate from the
/// dormant one.
const LIVE_DELIVERY_CONCURRENCY: usize = 16;
const FIRST_RETRY_DELAY: Duration = Duration::from_millis(200);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
const SCAN_DEBOUNCE: Duration = Duration::from_millis(20);

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/// The bodies of the mob's Active records, by digest: content the journal
/// keeps for replay and delivery. MobMachine holds every record's digest;
/// this store holds no lifecycle fact.
#[derive(Debug, Clone, Default)]
pub(crate) struct OperatorInstructionBodies {
    by_digest: BTreeMap<String, Arc<str>>,
}

impl OperatorInstructionBodies {
    /// The bodies the committed records of the journal's last reset epoch
    /// carry (a reset clears the records).
    pub(crate) fn from_events(events: &[MobEvent]) -> Self {
        let epoch_start = events
            .iter()
            .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
            .map_or(0, |position| position + 1);
        let mut bodies = Self::default();
        for event in &events[epoch_start..] {
            if let MobEventKind::OperatorInstructionCommitted(committed) = &event.kind
                && let (Some(digest), Some(body)) = (&committed.content_sha256, committed.body())
            {
                bodies.insert(digest, body);
            }
        }
        bodies
    }

    pub(crate) fn insert(&mut self, digest: &InstructionContentDigest, body: &str) {
        self.by_digest
            .insert(digest.as_str().to_owned(), Arc::from(body));
    }

    pub(crate) fn get(&self, digest: &str) -> Option<&str> {
        self.by_digest.get(digest).map(|body| &**body)
    }

    /// Keep only the bodies a record of `state` names.
    pub(crate) fn retain_referenced(&mut self, state: &mob_dsl::MobMachineState) {
        let referenced: BTreeSet<&String> = state.operator_instruction_digest.values().collect();
        self.by_digest
            .retain(|digest, _| referenced.contains(digest));
    }
}

/// The actor's operator-instruction shell: the bodies, and the delivery
/// bookkeeping it shares with the delivery owner and the status read.
#[derive(Debug, Clone)]
pub(crate) struct OperatorInstructionShell {
    pub(crate) bodies: OperatorInstructionBodies,
    pub(crate) delivery: Arc<OperatorInstructionDeliveryShared>,
}

/// Shell bookkeeping of the delivery owner: the last retryable failure of
/// each member's delivery (shown by the status read, never a lifecycle fact)
/// and the per-member locks a delivery holds while it writes the member's
/// session. Each lock guards the member's release epoch: a release takes the
/// lock and moves the epoch on, and a delivery prepared before that writes
/// nothing, so no delivery lands after a release.
#[derive(Debug)]
pub(crate) struct OperatorInstructionDeliveryShared {
    last_failures: std::sync::Mutex<BTreeMap<AgentIdentity, String>>,
    member_locks: std::sync::Mutex<BTreeMap<AgentIdentity, Arc<tokio::sync::Mutex<u64>>>>,
    pub(crate) dormant_concurrency: usize,
}

impl OperatorInstructionDeliveryShared {
    pub(crate) fn new(dormant_concurrency: usize) -> Arc<Self> {
        Arc::new(Self {
            last_failures: std::sync::Mutex::new(BTreeMap::new()),
            member_locks: std::sync::Mutex::new(BTreeMap::new()),
            dormant_concurrency: dormant_concurrency.max(1),
        })
    }

    /// The lock a member's delivery and its release share.
    pub(crate) fn member_lock(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Arc<tokio::sync::Mutex<u64>> {
        let mut locks = self
            .member_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(locks.entry(agent_identity.clone()).or_default())
    }

    fn record_failure(&self, agent_identity: &AgentIdentity, failure: String) {
        self.last_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(agent_identity.clone(), failure);
    }

    fn clear_failure(&self, agent_identity: &AgentIdentity) {
        self.last_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(agent_identity);
    }

    fn last_failure(&self, agent_identity: &AgentIdentity) -> Option<String> {
        self.last_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(agent_identity)
            .cloned()
    }

    /// Drop the bookkeeping of members `state` no longer seats. It reads the
    /// roster only for the members it holds bookkeeping for, never the whole
    /// roster.
    fn retain_members(&self, state: &mob_dsl::MobMachineState) {
        let seated = |identity: &AgentIdentity| {
            state
                .identity_to_runtime
                .contains_key(&mob_dsl::AgentIdentity::from_domain(identity))
        };
        self.last_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|identity, _| seated(identity));
        self.member_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|identity, lock| seated(identity) || Arc::strong_count(lock) > 1);
    }
}

impl OperatorInstructionShell {
    pub(crate) fn new(
        bodies: OperatorInstructionBodies,
        delivery: Arc<OperatorInstructionDeliveryShared>,
    ) -> Self {
        Self { bodies, delivery }
    }
}

// ---------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------

/// Whether the record of `key` in `state` selects `agent_identity`, a member
/// of profile `member_profile`.
fn record_selects(
    state: &mob_dsl::MobMachineState,
    key: &str,
    agent_identity: &AgentIdentity,
    member_profile: &str,
) -> bool {
    let Some(kind) = state.operator_instruction_target_kind.get(key) else {
        return false;
    };
    let profile = state
        .operator_instruction_target_profile
        .get(key)
        .map_or("", String::as_str);
    let member = state
        .operator_instruction_target_member
        .get(key)
        .map(|member| member.0.as_str());
    mob_dsl::operator_instruction_target_applies(
        *kind,
        profile,
        member,
        Some(member_profile),
        agent_identity.as_str(),
    )
}

/// The record target of `key` in `state`, in the public vocabulary.
fn record_target(
    state: &mob_dsl::MobMachineState,
    key: &str,
) -> Result<OperatorInstructionTarget, MobError> {
    let kind = state
        .operator_instruction_target_kind
        .get(key)
        .copied()
        .ok_or_else(|| {
            MobError::Internal(format!(
                "MobMachine holds the operator instruction record {key} without a target"
            ))
        })?;
    OperatorInstructionTarget::from_machine_parts(
        kind,
        state
            .operator_instruction_target_profile
            .get(key)
            .map_or("", String::as_str),
        state
            .operator_instruction_target_member
            .get(key)
            .map(|member| member.0.as_str()),
    )
}

fn machine_slot(key: &str) -> Result<InstructionSlot, MobError> {
    InstructionSlot::from_machine_key(key).map_err(|error| {
        MobError::Internal(format!(
            "MobMachine holds an operator instruction slot key that is not a slot: {error}"
        ))
    })
}

fn machine_revision(key: &str, revision: u64) -> Result<InstructionRevision, MobError> {
    InstructionRevision::new(revision).map_err(|_| {
        MobError::Internal(format!(
            "MobMachine holds the operator instruction record {key} at revision 0"
        ))
    })
}

/// One member's whole snapshot at the machine's generation: every record of
/// the mob, with its body when the record is Active and its target selects
/// the member (Absent otherwise, so a member that left a selector revokes
/// the slot).
pub(crate) fn member_instruction_record(
    state: &mob_dsl::MobMachineState,
    bodies: &OperatorInstructionBodies,
    mob_id: &MobId,
    agent_identity: &AgentIdentity,
    member_profile: &str,
) -> Result<InstructionTargetRecord, MobError> {
    let mut slots = Vec::with_capacity(state.operator_instruction_revision.len());
    for (key, revision) in &state.operator_instruction_revision {
        let slot = machine_slot(key)?;
        let revision = machine_revision(key, *revision)?;
        let selected = state.operator_instruction_effect.get(key)
            == Some(&mob_dsl::InstructionEffect::Active)
            && record_selects(state, key, agent_identity, member_profile);
        let body = if selected {
            let digest = state
                .operator_instruction_digest
                .get(key)
                .map(String::as_str)
                .unwrap_or_default();
            let body = bodies.get(digest).ok_or_else(|| {
                MobError::Internal(format!(
                    "the body of operator instruction {slot} (digest {digest}) is not in the \
                     mob's journal"
                ))
            })?;
            Some(body.to_owned())
        } else {
            None
        };
        slots.push(InstructionTargetRecordSlot {
            slot,
            revision,
            body,
        });
    }
    Ok(InstructionTargetRecord {
        mob_id: mob_id.to_string(),
        agent_identity: agent_identity.to_string(),
        generation: state.operator_instruction_generation,
        slots,
    })
}

/// The profile MobMachine holds for a seated member.
pub(crate) fn member_profile(
    state: &mob_dsl::MobMachineState,
    agent_identity: &AgentIdentity,
) -> String {
    state
        .member_profile_names
        .get(&mob_dsl::AgentIdentity::from_domain(agent_identity))
        .cloned()
        .unwrap_or_default()
}

/// How many current members the record of `key` selects.
pub(crate) fn matched_members(state: &mob_dsl::MobMachineState, key: &str) -> u64 {
    state
        .identity_to_runtime
        .keys()
        .filter(|identity| {
            let profile = state
                .member_profile_names
                .get(*identity)
                .map_or("", String::as_str);
            record_selects(
                state,
                key,
                &AgentIdentity::from(identity.0.as_str()),
                profile,
            )
        })
        .count() as u64
}

/// Carry a member's snapshot in the session its build mints: the new session
/// is born with its target, so its first run applies it, and the member's
/// owed delivery then answers "already recorded" without a write. A build
/// that continues an existing session carries nothing (that member receives
/// its snapshot by delivery), nor does a mob that never recorded a
/// generation.
pub(crate) fn carry_member_instruction_birth(
    request: &mut meerkat_core::service::CreateSessionRequest,
    state: &mob_dsl::MobMachineState,
    bodies: &OperatorInstructionBodies,
    mob_id: &MobId,
    agent_identity: &AgentIdentity,
    member_profile: &str,
) -> Result<(), MobError> {
    if state.operator_instruction_generation == 0 {
        return Ok(());
    }
    let Some(build) = request.build.as_mut() else {
        return Ok(());
    };
    if build.session_build_intent() != meerkat_core::SessionBuildIntent::Mint {
        return Ok(());
    }
    let Some(carrier) = build.resume_session.as_mut() else {
        return Ok(());
    };
    let record = member_instruction_record(state, bodies, mob_id, agent_identity, member_profile)?;
    carrier
        .record_instruction_target(record)
        .map(|_| ())
        .map_err(|error| {
            MobError::Internal(format!(
                "the new session of member {agent_identity} cannot carry its operator \
                 instruction snapshot: {error}"
            ))
        })
}

// ---------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------

/// MobMachine's answer to one delivery request.
#[derive(Debug)]
pub(crate) enum MemberInstructionPreparation {
    /// Record this snapshot in the member's session.
    Deliver(MemberInstructionDelivery),
    /// Nothing to deliver now: the member is not owed, its seat or binding
    /// has no session yet, it is retiring, its spawn has not activated yet,
    /// it is blocked, or the mob is not running.
    NotNow,
}

/// What a member session did with a delivered snapshot.
#[derive(Debug, Clone)]
pub(crate) enum MemberInstructionDeliveryOutcome {
    /// The session holds the snapshot at `generation` (written now, or
    /// already recorded).
    Recorded {
        agent_identity: AgentIdentity,
        session_id: SessionId,
        generation: u64,
    },
    /// The session refused it for a reason no retry changes.
    Blocked {
        agent_identity: AgentIdentity,
        session_id: SessionId,
        generation: u64,
        block: mob_dsl::MemberInstructionBlock,
    },
}

/// How the delivery owner reads a session's answer.
enum DeliveryResult {
    Settled(MemberInstructionDeliveryOutcome),
    Retry(String),
}

fn classify_delivery_result(
    agent_identity: &AgentIdentity,
    session_id: &SessionId,
    generation: u64,
    result: Result<meerkat_core::InstructionRecordReceipt, SessionInstructionError>,
) -> DeliveryResult {
    let blocked = |block| {
        DeliveryResult::Settled(MemberInstructionDeliveryOutcome::Blocked {
            agent_identity: agent_identity.clone(),
            session_id: session_id.clone(),
            generation,
            block,
        })
    };
    match result {
        // The session holds a later generation under this mob's id than the
        // mob ever minted: an earlier incarnation of the mob id recorded it
        // (the mob was destroyed and a new one took its id before the old
        // target was released). The snapshot it holds is not this mob's, so
        // the delivery is not current; it is retried, and the status read
        // shows why.
        Ok(receipt) if receipt.target_generation > generation => DeliveryResult::Retry(format!(
            "the member session holds operator instruction generation {} under this mob's id, \
             above this mob's generation {generation}: an earlier mob with this id recorded it",
            receipt.target_generation
        )),
        Ok(_) => DeliveryResult::Settled(MemberInstructionDeliveryOutcome::Recorded {
            agent_identity: agent_identity.clone(),
            session_id: session_id.clone(),
            generation,
        }),
        Err(SessionInstructionError::DurabilityUnavailable) => {
            blocked(mob_dsl::MemberInstructionBlock::DurabilityUnavailable)
        }
        Err(SessionInstructionError::Refused(InstructionWriteError::OwnedBySession)) => {
            blocked(mob_dsl::MemberInstructionBlock::OwnedBySession)
        }
        Err(SessionInstructionError::Refused(InstructionWriteError::OwnedByOtherMob)) => {
            blocked(mob_dsl::MemberInstructionBlock::OwnedByOtherMob)
        }
        Err(SessionInstructionError::Refused(InstructionWriteError::SlotManagedByActivation {
            ..
        })) => blocked(mob_dsl::MemberInstructionBlock::SlotManagedByActivation),
        Err(other) => DeliveryResult::Retry(other.to_string()),
    }
}

/// One owed member's delivery, as the delivery owner schedules it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryLane {
    Live,
    Dormant,
}

#[derive(Debug)]
struct RetryState {
    owed_generation: u64,
    delay: Duration,
    next_attempt: Instant,
}

/// The members a scan may deliver to now, from one published machine state:
/// owed, in a running or stopped mob, with a born and bound session, local,
/// not retiring and with its spawn settled (a placed member is asked once,
/// and the machine blocks it). MobMachine decides each delivery when it is
/// prepared; this only schedules.
fn deliverable_members(
    state: &mob_dsl::MobMachineState,
) -> Vec<(AgentIdentity, u64, DeliveryLane)> {
    if !matches!(
        state.lifecycle_phase,
        mob_dsl::MobPhase::Running | mob_dsl::MobPhase::Stopped
    ) {
        return Vec::new();
    }
    state
        .member_instruction_owed
        .iter()
        .filter(|(identity, _)| {
            let placed = state.member_placement.contains_key(*identity);
            !state.member_seat_session_unborn.contains(*identity)
                && (state.member_session_bindings.contains_key(*identity) || placed)
                && state
                    .identity_to_runtime
                    .get(*identity)
                    .and_then(|runtime| state.member_state_markers.get(runtime))
                    != Some(&mob_dsl::MobMemberState::Retiring)
                && (placed || !state.spawn_exec_phase.contains_key(*identity))
        })
        .map(|(identity, generation)| {
            let lane = match state.member_materialization.get(identity) {
                Some(
                    mob_dsl::MemberMaterializationPhase::Live
                    | mob_dsl::MemberMaterializationPhase::Waking,
                ) => DeliveryLane::Live,
                _ => DeliveryLane::Dormant,
            };
            (AgentIdentity::from(identity.0.as_str()), *generation, lane)
        })
        .collect()
}

/// The delivery owner of one mob: an actor-lifetime task that delivers every
/// owed member's snapshot. Its wake is the machine-state watch (a write, a
/// seat, a born session and every feedback publish it), with a slow safety
/// scan and per-member retry backoff for retryable failures.
pub(crate) struct OperatorInstructionDeliveryReconciler {
    handle: MobHandle,
    shared: Arc<OperatorInstructionDeliveryShared>,
}

/// One delivery attempt's end: the member, and what its session answered
/// (`None` when MobMachine had nothing to deliver).
type DeliveryAttempt = (AgentIdentity, Option<DeliveryResult>);

#[cfg(not(target_arch = "wasm32"))]
type DeliveryFuture = std::pin::Pin<Box<dyn std::future::Future<Output = DeliveryAttempt> + Send>>;
#[cfg(target_arch = "wasm32")]
type DeliveryFuture = std::pin::Pin<Box<dyn std::future::Future<Output = DeliveryAttempt>>>;

/// One delivery wave's feedback to the actor: the members it carries and
/// whether it reached MobMachine.
type FeedbackResult = (Vec<AgentIdentity>, Result<(), MobError>);

#[cfg(not(target_arch = "wasm32"))]
type FeedbackFuture = std::pin::Pin<Box<dyn std::future::Future<Output = FeedbackResult> + Send>>;
#[cfg(target_arch = "wasm32")]
type FeedbackFuture = std::pin::Pin<Box<dyn std::future::Future<Output = FeedbackResult>>>;

/// What woke the delivery owner.
enum DeliveryWake {
    Closed,
    MachineChanged,
    Settled(DeliveryAttempt),
    Fed(FeedbackResult),
    ScanDue,
}

/// The delivery owner's loop state.
#[derive(Default)]
struct DeliveryLanes {
    live: FuturesUnordered<DeliveryFuture>,
    dormant: FuturesUnordered<DeliveryFuture>,
    feedback: FuturesUnordered<FeedbackFuture>,
    /// Members with a delivery in a lane.
    in_flight: BTreeSet<AgentIdentity>,
    /// Members whose outcome is on its way to MobMachine: not delivered
    /// again until it lands.
    feeding: BTreeSet<AgentIdentity>,
    retry: BTreeMap<AgentIdentity, RetryState>,
}

impl OperatorInstructionDeliveryReconciler {
    pub(crate) async fn run_owned(
        handle: MobHandle,
        shared: Arc<OperatorInstructionDeliveryShared>,
    ) {
        Self { handle, shared }.run().await;
    }

    async fn run(self) {
        let actor_closed = self.handle.command_tx.clone();
        let mut machine_changes = Some(self.handle.machine_state_changes());
        let mut lanes = DeliveryLanes::default();
        let mut deadline = super::ReconcileScanDeadline::first_scan(Instant::now(), SCAN_DEBOUNCE);
        loop {
            let wake = tokio::select! {
                () = actor_closed.closed() => DeliveryWake::Closed,
                () = super::wait_for_machine_state_change(&mut machine_changes) => {
                    DeliveryWake::MachineChanged
                }
                Some(settled) = lanes.live.next(), if !lanes.live.is_empty() => {
                    DeliveryWake::Settled(settled)
                }
                Some(settled) = lanes.dormant.next(), if !lanes.dormant.is_empty() => {
                    DeliveryWake::Settled(settled)
                }
                Some(fed) = lanes.feedback.next(), if !lanes.feedback.is_empty() => {
                    DeliveryWake::Fed(fed)
                }
                () = tokio::time::sleep(deadline.sleep_duration(Instant::now())) => {
                    DeliveryWake::ScanDue
                }
            };
            match wake {
                DeliveryWake::Closed => break,
                DeliveryWake::MachineChanged => {
                    deadline.pull_earlier(Instant::now(), SCAN_DEBOUNCE);
                }
                DeliveryWake::Settled(settled) => {
                    self.settle(settled, &mut lanes);
                    deadline.pull_earlier(Instant::now(), SCAN_DEBOUNCE);
                }
                DeliveryWake::Fed((members, result)) => {
                    for member in &members {
                        lanes.feeding.remove(member);
                    }
                    if let Err(error) = result {
                        // The obligations stay owed: back off, then deliver
                        // again (a session that recorded answers without a
                        // write).
                        tracing::debug!(
                            error = %error,
                            "operator instruction delivery feedback did not reach the mob"
                        );
                        let owed = self.owed_generations();
                        for member in &members {
                            back_off(&mut lanes.retry, &owed, member);
                        }
                    }
                    deadline.pull_earlier(Instant::now(), SCAN_DEBOUNCE);
                }
                DeliveryWake::ScanDue => {
                    let next = self.scan(&mut lanes);
                    deadline.rearm(Instant::now(), next);
                }
            }
        }
    }

    /// The generation each owed member is owed, from the published state.
    fn owed_generations(&self) -> BTreeMap<AgentIdentity, u64> {
        let state = self.handle.machine_state_watch_rx.borrow();
        state
            .member_instruction_owed
            .iter()
            .map(|(identity, generation)| (AgentIdentity::from(identity.0.as_str()), *generation))
            .collect()
    }

    /// Start every deliverable member's delivery the lanes have room for,
    /// and answer when the next scan is due.
    fn scan(&self, lanes: &mut DeliveryLanes) -> Duration {
        let candidates = {
            let state = self.handle.machine_state_watch_rx.borrow();
            self.shared.retain_members(&state);
            // A mob with nothing owed and nothing under way has nothing to
            // scan: the next machine change or the safety interval wakes it.
            if state.member_instruction_owed.is_empty()
                && lanes.in_flight.is_empty()
                && lanes.feeding.is_empty()
                && lanes.retry.is_empty()
            {
                return super::RECONCILE_SAFETY_INTERVAL;
            }
            deliverable_members(&state)
        };
        let owed = candidates
            .iter()
            .map(|(identity, generation, _)| (identity.clone(), *generation))
            .collect::<BTreeMap<_, _>>();
        // A retry is for one obligation: a later generation, or a member no
        // longer owed, starts over.
        lanes
            .retry
            .retain(|identity, state| owed.get(identity) == Some(&state.owed_generation));
        let now = Instant::now();
        for (identity, generation, lane) in candidates {
            if lanes.in_flight.contains(&identity) || lanes.feeding.contains(&identity) {
                continue;
            }
            if lanes.retry.get(&identity).is_some_and(|state| {
                state.owed_generation == generation && now < state.next_attempt
            }) {
                continue;
            }
            let (pool, bound) = match lane {
                DeliveryLane::Live => (&mut lanes.live, LIVE_DELIVERY_CONCURRENCY),
                DeliveryLane::Dormant => (&mut lanes.dormant, self.shared.dormant_concurrency),
            };
            if pool.len() >= bound {
                continue;
            }
            lanes.in_flight.insert(identity.clone());
            pool.push(Box::pin(Self::deliver_one(
                self.handle.clone(),
                Arc::clone(&self.shared),
                identity,
            )));
        }
        lanes
            .retry
            .values()
            .map(|state| state.next_attempt.saturating_duration_since(now))
            .min()
            .unwrap_or(super::RECONCILE_SAFETY_INTERVAL)
            .max(SCAN_DEBOUNCE)
            .min(super::RECONCILE_SAFETY_INTERVAL)
    }

    /// One member's delivery: MobMachine prepares it, then the member's
    /// session records the sealed snapshot. `None` when the machine had
    /// nothing to deliver.
    async fn deliver_one(
        handle: MobHandle,
        shared: Arc<OperatorInstructionDeliveryShared>,
        agent_identity: AgentIdentity,
    ) -> DeliveryAttempt {
        let lock = shared.member_lock(&agent_identity);
        let epoch = *lock.lock().await;
        let delivery = match handle
            .prepare_member_instruction_delivery(&agent_identity)
            .await
        {
            Ok(MemberInstructionPreparation::Deliver(delivery)) => delivery,
            Ok(MemberInstructionPreparation::NotNow) => return (agent_identity, None),
            Err(error) => {
                return (
                    agent_identity,
                    Some(DeliveryResult::Retry(error.to_string())),
                );
            }
        };
        let session_id = delivery.session_id().clone();
        let generation = delivery.generation();
        let result = {
            // The member's release waits for this write; the lock is let go
            // before the feedback goes to the actor. A release that landed
            // since this delivery began moved the epoch on: nothing is
            // written after it.
            let held = lock.lock().await;
            if *held != epoch {
                return (agent_identity, None);
            }
            Arc::clone(&handle.session_service)
                .record_member_instruction_target(delivery)
                .await
        };
        let result = classify_delivery_result(&agent_identity, &session_id, generation, result);
        (agent_identity, Some(result))
    }

    /// Collect one settled delivery and every other one already settled,
    /// and feed their outcomes to MobMachine in one batch (one journal row
    /// for the recorded ones). The feedback runs beside the lanes, so a
    /// delivery in flight keeps moving while the actor answers it.
    fn settle(&self, first: DeliveryAttempt, lanes: &mut DeliveryLanes) {
        let mut settled = vec![first];
        while let Some(Some(next)) = lanes.live.next().now_or_never() {
            settled.push(next);
        }
        while let Some(Some(next)) = lanes.dormant.next().now_or_never() {
            settled.push(next);
        }
        let owed = self.owed_generations();
        let mut outcomes = Vec::new();
        let mut members = Vec::new();
        for (identity, result) in settled {
            lanes.in_flight.remove(&identity);
            match result {
                // MobMachine had nothing to deliver now.
                None => back_off(&mut lanes.retry, &owed, &identity),
                Some(DeliveryResult::Settled(outcome)) => {
                    lanes.retry.remove(&identity);
                    self.shared.clear_failure(&identity);
                    lanes.feeding.insert(identity.clone());
                    members.push(identity);
                    outcomes.push(outcome);
                }
                Some(DeliveryResult::Retry(failure)) => {
                    tracing::debug!(
                        agent_identity = %identity,
                        failure = %failure,
                        "operator instruction delivery remains owed and will retry"
                    );
                    self.shared.record_failure(&identity, failure);
                    back_off(&mut lanes.retry, &owed, &identity);
                }
            }
        }
        if outcomes.is_empty() {
            return;
        }
        let handle = self.handle.clone();
        lanes.feedback.push(Box::pin(async move {
            let result = handle.settle_member_instruction_deliveries(outcomes).await;
            (members, result)
        }));
    }
}

/// A member still owed waits before its next attempt, longer each time, so
/// an obligation that cannot land now never spins; one no longer owed is
/// forgotten.
fn back_off(
    retry: &mut BTreeMap<AgentIdentity, RetryState>,
    owed: &BTreeMap<AgentIdentity, u64>,
    identity: &AgentIdentity,
) {
    let Some(owed_generation) = owed.get(identity).copied() else {
        retry.remove(identity);
        return;
    };
    let now = Instant::now();
    let state = retry.entry(identity.clone()).or_insert(RetryState {
        owed_generation,
        delay: FIRST_RETRY_DELAY,
        next_attempt: now,
    });
    if state.owed_generation != owed_generation {
        state.owed_generation = owed_generation;
        state.delay = FIRST_RETRY_DELAY;
    }
    state.next_attempt = now + state.delay;
    state.delay = (state.delay * 2).min(MAX_RETRY_DELAY);
}

// ---------------------------------------------------------------------------
// Handle surface
// ---------------------------------------------------------------------------

impl MobHandle {
    /// Write one operator instruction: the slot's body at a revision, for
    /// the members the target selects (every member, a profile's members, or
    /// one member). The receipt returns once the committed record is durable
    /// in the mob's journal; every member whose snapshot it changes is owed
    /// it, and the mob records it in their sessions asynchronously (a
    /// dormant member's session store-only, without waking it). Each
    /// member's session applies it at its next fresh run start. Host verb
    /// (`AdminHost`); never an agent tool.
    ///
    /// # Errors
    ///
    /// [`OperatorInstructionError::Refused`] for a typed refusal (a body
    /// digest that is not the body's, a stale or conflicting revision, the
    /// slot limit, the per-slot body limit, the mob's active budget),
    /// [`OperatorInstructionError::MobTerminal`] for a completed or destroyed
    /// mob, and [`OperatorInstructionError::Mob`] when the mob cannot answer.
    pub async fn set_operator_instruction(
        &self,
        write: OperatorInstructionWrite,
    ) -> Result<OperatorInstructionReceipt, OperatorInstructionError> {
        write.target.validate()?;
        let computed = InstructionContentDigest::for_body(&write.body);
        if let Some(declared) = &write.content_sha256
            && *declared != computed
        {
            return Err(OperatorInstructionError::Refused(
                InstructionWriteError::DigestMismatch {
                    declared: declared.clone(),
                    computed,
                },
            ));
        }
        self.send_actor_command(|reply_tx| MobCommand::SetOperatorInstruction {
            write: Box::new(write),
            digest: computed,
            reply_tx,
        })
        .await?
    }

    /// Revoke one operator instruction at a higher revision: the record
    /// keeps its target, and its members are owed the revoke. Host verb
    /// (`AdminHost`).
    ///
    /// # Errors
    ///
    /// As [`Self::set_operator_instruction`]; an unknown slot is refused
    /// typed.
    pub async fn revoke_operator_instruction(
        &self,
        slot: InstructionSlot,
        revision: InstructionRevision,
    ) -> Result<OperatorInstructionReceipt, OperatorInstructionError> {
        self.send_actor_command(|reply_tx| MobCommand::RevokeOperatorInstruction {
            slot,
            revision,
            reply_tx,
        })
        .await?
    }

    /// The mob's operator-instruction status: its records and, for one page
    /// of members, each member's residency, delivery state, and per slot the
    /// member's desired view, its session's recorded target and the committed
    /// entry that put it in effect. Reads the committed session documents
    /// and never wakes or materializes a member; a member whose session
    /// cannot be read is reported with its reason, and the page still
    /// answers. Read verb (`ReadHistory`).
    ///
    /// # Errors
    ///
    /// [`OperatorInstructionError::Refused`] for a limit outside 1 to 200,
    /// [`OperatorInstructionError::Mob`] when the mob cannot answer.
    pub async fn operator_instruction_status(
        &self,
        query: OperatorInstructionStatusQuery,
    ) -> Result<OperatorInstructionStatusPage, OperatorInstructionError> {
        let limit = query
            .limit
            .unwrap_or(OPERATOR_INSTRUCTION_STATUS_DEFAULT_LIMIT);
        if limit == 0 || limit > OPERATOR_INSTRUCTION_STATUS_MAX_LIMIT {
            return Err(OperatorInstructionError::Refused(
                InstructionWriteError::InvalidRequest(format!(
                    "a status page holds 1 to {OPERATOR_INSTRUCTION_STATUS_MAX_LIMIT} members"
                )),
            ));
        }
        self.admit_control_scope(mob_dsl::ControlScope::ReadHistory)
            .await?;
        let state = self.machine_state_watch_rx.borrow().clone();
        let desired = desired_records(&state, &query)?;
        let mut members = state
            .identity_to_runtime
            .keys()
            .map(|identity| AgentIdentity::from(identity.0.as_str()))
            .filter(|identity| {
                query
                    .agent_identity
                    .as_ref()
                    .is_none_or(|wanted| wanted == identity)
            })
            .filter(|identity| {
                query
                    .profile
                    .as_ref()
                    .is_none_or(|profile| member_profile(&state, identity) == profile.as_str())
            })
            .collect::<Vec<_>>();
        members.sort();
        let total = members.len();
        let page = members
            .into_iter()
            .skip(query.offset)
            .take(limit)
            .collect::<Vec<_>>();
        let next_offset = (query.offset.saturating_add(page.len()) < total)
            .then(|| query.offset.saturating_add(page.len()));
        let mut reads = self.read_page_member_instructions(&state, &page).await;
        let mut statuses = Vec::with_capacity(page.len());
        for identity in page {
            let read = reads.remove(&identity);
            statuses.push(
                self.member_instruction_status(&state, &desired, &identity, read)
                    .await,
            );
        }
        Ok(OperatorInstructionStatusPage {
            mob_id: self.mob_id().clone(),
            generation: state.operator_instruction_generation,
            desired,
            members: statuses,
            next_offset,
        })
    }

    /// Read the operator-instruction state of a page's member sessions in
    /// one call, so the session service resolves each model's lowering once
    /// for the page. An unborn seat and a member without a session binding
    /// are not read.
    async fn read_page_member_instructions(
        &self,
        state: &mob_dsl::MobMachineState,
        page: &[AgentIdentity],
    ) -> BTreeMap<AgentIdentity, Result<SessionInstructionRead, SessionInstructionError>> {
        let mut members = Vec::new();
        let mut session_ids = Vec::new();
        for identity in page {
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
            if state.member_seat_session_unborn.contains(&dsl_identity) {
                continue;
            }
            if let Some(session_id) = state
                .member_session_bindings
                .get(&dsl_identity)
                .and_then(|session| SessionId::parse(&session.0).ok())
            {
                members.push(identity.clone());
                session_ids.push(session_id);
            }
        }
        if session_ids.is_empty() {
            return BTreeMap::new();
        }
        let mut reads = self
            .session_service
            .read_member_instructions(&session_ids)
            .await
            .into_iter();
        members
            .into_iter()
            .map(|identity| {
                let read = reads.next().unwrap_or_else(|| {
                    Err(SessionInstructionError::Session(
                        meerkat_core::service::SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(
                                "the session service answered fewer operator instruction reads \
                                 than the page asked for"
                                    .to_string(),
                            ),
                        ),
                    ))
                });
                (identity, read)
            })
            .collect()
    }

    async fn member_instruction_status(
        &self,
        state: &mob_dsl::MobMachineState,
        desired: &[OperatorInstructionRecordStatus],
        identity: &AgentIdentity,
        fetched: Option<Result<SessionInstructionRead, SessionInstructionError>>,
    ) -> MemberInstructionStatus {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        let profile = member_profile(state, identity);
        let unborn = state.member_seat_session_unborn.contains(&dsl_identity);
        let residency = if unborn {
            MemberInstructionResidency::Unborn
        } else {
            match state.member_materialization.get(&dsl_identity) {
                Some(
                    mob_dsl::MemberMaterializationPhase::Live
                    | mob_dsl::MemberMaterializationPhase::Waking,
                ) => MemberInstructionResidency::Live,
                _ => MemberInstructionResidency::Dormant,
            }
        };
        let recorded_generation = state
            .member_instruction_recorded
            .get(&dsl_identity)
            .copied();
        let delivery_state =
            if let Some(block) = state.member_instruction_blocked.get(&dsl_identity) {
                MemberInstructionDeliveryState::Blocked {
                    reason: block_reason(*block),
                }
            } else if let Some(generation) = state.member_instruction_owed.get(&dsl_identity) {
                MemberInstructionDeliveryState::Owed {
                    generation: *generation,
                    last_failure: self.operator_instruction_delivery.last_failure(identity),
                }
            } else {
                MemberInstructionDeliveryState::Current
            };
        let session_id = state
            .member_session_bindings
            .get(&dsl_identity)
            .and_then(|session| SessionId::parse(&session.0).ok());
        let (read, effective_read) = match (fetched, unborn) {
            // An unborn seat has no session to read: nothing is effective.
            (_, true) => (None, MemberInstructionEffectiveRead::Ok),
            (Some(Ok(read)), false) => (Some(read), MemberInstructionEffectiveRead::Ok),
            (Some(Err(error)), false) => (
                None,
                MemberInstructionEffectiveRead::Unreadable {
                    reason: error.to_string(),
                },
            ),
            (None, false) => (
                None,
                MemberInstructionEffectiveRead::Unreadable {
                    reason: "the member has no session binding".to_string(),
                },
            ),
        };
        let live_channel_open = self.member_live_channel_open(session_id.as_ref()).await;
        // The session's resolved lowering, as its service reads it; without
        // one, the model's resolved capabilities (a dormant member's from
        // the catalog, read-only) tell whether it lowers only inside the
        // leading System prefix.
        let lowering = match read.as_ref().and_then(|read| read.lowering) {
            Some(lowering) => lowering,
            None => {
                if !unborn && self.member_lowers_leading_prefix_only(identity).await {
                    meerkat_core::InstructionLowering::LeadingPrefixOnly
                } else {
                    meerkat_core::InstructionLowering::MidConversation
                }
            }
        };
        let slots = desired
            .iter()
            .map(|record| {
                member_slot_status(
                    record,
                    identity,
                    &profile,
                    read.as_ref(),
                    &delivery_state,
                    unborn,
                    lowering,
                )
            })
            .collect();
        MemberInstructionStatus {
            agent_identity: identity.clone(),
            profile: ProfileName::from(profile.as_str()),
            residency,
            delivery: MemberInstructionDeliveryStatus {
                recorded_generation,
                state: delivery_state,
            },
            slots,
            effective_read,
            live_channel_open,
        }
    }

    #[cfg(feature = "openai-live")]
    async fn member_live_channel_open(&self, session_id: Option<&SessionId>) -> bool {
        match (&self.runtime_adapter, session_id) {
            (Some(runtime), Some(session_id)) => runtime
                .live_active_channel_for_session(session_id)
                .await
                .is_some(),
            _ => false,
        }
    }

    #[cfg(not(feature = "openai-live"))]
    async fn member_live_channel_open(&self, _session_id: Option<&SessionId>) -> bool {
        false
    }

    /// Whether a member's model takes System rows only inside the leading
    /// System prefix, from its resolved capabilities (a live member's
    /// registration, a dormant member's model from the catalog).
    async fn member_lowers_leading_prefix_only(&self, identity: &AgentIdentity) -> bool {
        match self.member_status(identity).await {
            Ok(snapshot) => snapshot
                .resolved_capabilities
                .as_ref()
                .is_some_and(|capabilities| !capabilities.mid_conversation_system_messages),
            Err(_) => false,
        }
    }

    /// MobMachine prepares one owed member's delivery.
    pub(crate) async fn prepare_member_instruction_delivery(
        &self,
        agent_identity: &AgentIdentity,
    ) -> Result<MemberInstructionPreparation, MobError> {
        self.send_actor_command(|reply_tx| MobCommand::PrepareMemberInstructionDelivery {
            agent_identity: agent_identity.clone(),
            reply_tx,
        })
        .await?
    }

    /// Feed what member sessions did with delivered snapshots back to
    /// MobMachine.
    pub(crate) async fn settle_member_instruction_deliveries(
        &self,
        outcomes: Vec<MemberInstructionDeliveryOutcome>,
    ) -> Result<(), MobError> {
        self.send_actor_command(|reply_tx| MobCommand::SettleMemberInstructionDeliveries {
            outcomes,
            reply_tx,
        })
        .await?
    }
}

fn block_reason(block: mob_dsl::MemberInstructionBlock) -> MemberInstructionBlockReason {
    match block {
        mob_dsl::MemberInstructionBlock::MemberHostUnsupported => {
            MemberInstructionBlockReason::MemberHostUnsupported
        }
        mob_dsl::MemberInstructionBlock::SlotManagedByActivation => {
            MemberInstructionBlockReason::SlotManagedByActivation
        }
        mob_dsl::MemberInstructionBlock::DurabilityUnavailable => {
            MemberInstructionBlockReason::DurabilityUnavailable
        }
        mob_dsl::MemberInstructionBlock::OwnedBySession => {
            MemberInstructionBlockReason::OwnedBySession
        }
        mob_dsl::MemberInstructionBlock::OwnedByOtherMob => {
            MemberInstructionBlockReason::OwnedByOtherMob
        }
    }
}

fn core_effect(effect: mob_dsl::InstructionEffect) -> meerkat_core::InstructionEffect {
    match effect {
        mob_dsl::InstructionEffect::Absent => meerkat_core::InstructionEffect::Absent,
        mob_dsl::InstructionEffect::Active => meerkat_core::InstructionEffect::Active,
    }
}

/// The mob's records a status query names, sorted by slot.
fn desired_records(
    state: &mob_dsl::MobMachineState,
    query: &OperatorInstructionStatusQuery,
) -> Result<Vec<OperatorInstructionRecordStatus>, MobError> {
    let mut desired = Vec::new();
    for (key, revision) in &state.operator_instruction_revision {
        let slot = machine_slot(key)?;
        if query
            .namespace
            .as_ref()
            .is_some_and(|namespace| *namespace != slot.namespace)
            || query.key.as_ref().is_some_and(|wanted| *wanted != slot.key)
        {
            continue;
        }
        let effect = core_effect(
            state
                .operator_instruction_effect
                .get(key)
                .copied()
                .unwrap_or_default(),
        );
        let active = effect == meerkat_core::InstructionEffect::Active;
        desired.push(OperatorInstructionRecordStatus {
            revision: machine_revision(key, *revision)?,
            effect,
            target: record_target(state, key)?,
            content_sha256: if active {
                state
                    .operator_instruction_digest
                    .get(key)
                    .and_then(|digest| InstructionContentDigest::new(digest.clone()).ok())
            } else {
                None
            },
            body_bytes: active
                .then(|| state.operator_instruction_body_bytes.get(key).copied())
                .flatten(),
            slot,
        });
    }
    desired.sort_by(|left, right| left.slot.cmp(&right.slot));
    Ok(desired)
}

/// One slot's status for one member.
#[allow(clippy::too_many_arguments)]
fn member_slot_status(
    record: &OperatorInstructionRecordStatus,
    identity: &AgentIdentity,
    profile: &str,
    read: Option<&SessionInstructionRead>,
    delivery: &MemberInstructionDeliveryState,
    unborn: bool,
    lowering: meerkat_core::InstructionLowering,
) -> MemberInstructionSlotStatus {
    let selected = record.target.selects(identity, &ProfileName::from(profile));
    let desired = MemberInstructionSlotView {
        revision: record.revision,
        effect: if record.effect == meerkat_core::InstructionEffect::Active && selected {
            meerkat_core::InstructionEffect::Active
        } else {
            meerkat_core::InstructionEffect::Absent
        },
    };
    let recorded =
        read.and_then(|read| read.target.as_ref())
            .and_then(|target| {
                target.slots().iter().find(|slot| {
                    slot.namespace == record.slot.namespace && slot.key == record.slot.key
                })
            })
            .map(|slot| MemberInstructionSlotView {
                revision: slot.revision,
                effect: slot.effect,
            });
    let effective = read
        .and_then(|read| read.effective.slot(&record.slot))
        .map(|slot| MemberInstructionEffectiveSlot {
            revision: slot.revision(),
            effect: slot.effect(),
            entry_sequence: slot.entry_sequence(),
            kind: slot.entry_kind(),
            run_id: slot.run_id().cloned(),
            message_index: slot.message_index(),
        });
    let in_effect = effective.as_ref().is_some_and(|effective| {
        effective.revision == desired.revision && effective.effect == desired.effect
    });
    let state_of_slot = if desired.effect == meerkat_core::InstructionEffect::Absent
        && recorded.is_none_or(|view| view.effect == meerkat_core::InstructionEffect::Absent)
        && effective
            .as_ref()
            .is_none_or(|effective| effective.effect == meerkat_core::InstructionEffect::Absent)
    {
        MemberInstructionSlotState::NotTargeted
    } else if in_effect {
        MemberInstructionSlotState::Effective
    } else if let MemberInstructionDeliveryState::Blocked { reason } = delivery {
        MemberInstructionSlotState::Blocked { reason: *reason }
    } else if unborn {
        MemberInstructionSlotState::Pending {
            reason: MemberInstructionPendingReason::SessionUnborn,
        }
    } else if recorded != Some(desired) {
        MemberInstructionSlotState::Pending {
            reason: MemberInstructionPendingReason::DeliveryOwed,
        }
    } else {
        MemberInstructionSlotState::Pending {
            reason: match lowering {
                meerkat_core::InstructionLowering::Unavailable => {
                    MemberInstructionPendingReason::LoweringUnavailable
                }
                meerkat_core::InstructionLowering::LeadingPrefixOnly => {
                    MemberInstructionPendingReason::LeadingPrefixOnlyLowering
                }
                meerkat_core::InstructionLowering::MidConversation => {
                    MemberInstructionPendingReason::AwaitingRunStart
                }
            },
        }
    };
    MemberInstructionSlotStatus {
        slot: record.slot.clone(),
        desired,
        recorded,
        effective,
        state: state_of_slot,
    }
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

/// The machine signal that replays a committed record.
pub(crate) fn recover_operator_instruction_signal(
    committed: &crate::operator_instruction::OperatorInstructionCommittedEvent,
) -> mob_dsl::MobMachineSignal {
    let (target_kind, target_profile, target_member) = committed.target.machine_parts();
    mob_dsl::MobMachineSignal::RecoverOperatorInstruction {
        slot: committed.slot.machine_key(),
        revision: committed.revision.get(),
        effect: match committed.effect {
            meerkat_core::InstructionEffect::Absent => mob_dsl::InstructionEffect::Absent,
            meerkat_core::InstructionEffect::Active => mob_dsl::InstructionEffect::Active,
        },
        digest: committed
            .content_sha256
            .as_ref()
            .map(|digest| digest.as_str().to_owned())
            .unwrap_or_default(),
        body_bytes: committed.body_bytes,
        target_kind,
        target_profile,
        target_member: target_member.map(mob_dsl::AgentIdentity),
        generation: committed.generation,
    }
}

/// The machine signals that replay one delivery wave's recorded targets, for
/// the session each member is bound to at that point of the replay.
pub(crate) fn recover_member_instruction_target_signals(
    state: &mob_dsl::MobMachineState,
    recorded: &crate::operator_instruction::MemberInstructionTargetsRecordedEvent,
) -> Vec<mob_dsl::MobMachineSignal> {
    recorded
        .members
        .iter()
        .filter_map(|member| {
            let identity = mob_dsl::AgentIdentity::from_domain(&member.agent_identity);
            let session_id = state.member_session_bindings.get(&identity)?.clone();
            Some(mob_dsl::MobMachineSignal::RecordMemberInstructionTarget {
                agent_identity: identity,
                session_id,
                generation: member.generation,
            })
        })
        .collect()
}

/// The operator-instruction generation the mob had reached when its last
/// reset committed: the epoch replay sees only the records after that reset,
/// and a reset moves a generation it has past it. Zero for a journal with no
/// reset or no record before its last reset.
pub(crate) fn operator_instruction_generation_highwater(events: &[MobEvent]) -> u64 {
    let Some(last_reset) = events
        .iter()
        .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
    else {
        return 0;
    };
    let mut generation = 0_u64;
    for event in &events[..=last_reset] {
        match &event.kind {
            MobEventKind::OperatorInstructionCommitted(committed) => {
                generation = generation.max(committed.generation);
            }
            MobEventKind::MobReset if generation > 0 => {
                generation = generation.saturating_add(1);
            }
            _ => {}
        }
    }
    generation
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use meerkat_core::{
        InstructionKey, InstructionNamespace, InstructionRecordDisposition,
        InstructionRecordReceipt, InstructionTargetRecordSlot, Session,
    };

    fn tone() -> InstructionSlot {
        InstructionSlot {
            namespace: InstructionNamespace::new("app.example").unwrap(),
            key: InstructionKey::new("tone").unwrap(),
        }
    }

    /// A receipt whose target generation is above the delivery's is never
    /// current: an earlier mob with this id recorded it, so the delivery is
    /// retried with the reason. A receipt at the delivery's own generation
    /// records it.
    #[test]
    fn a_receipt_above_the_delivered_generation_is_retried_not_current() {
        let identity = AgentIdentity::from("member");
        let session_id = SessionId::new();
        let receipt = |target_generation| {
            Ok(InstructionRecordReceipt {
                disposition: InstructionRecordDisposition::AlreadyRecorded,
                target_generation,
            })
        };
        match classify_delivery_result(&identity, &session_id, 2, receipt(5)) {
            DeliveryResult::Retry(reason) => assert!(reason.contains("generation 5"), "{reason}"),
            DeliveryResult::Settled(_) => {
                panic!("a later generation under this mob's id is not current")
            }
        }
        assert!(matches!(
            classify_delivery_result(&identity, &session_id, 2, receipt(2)),
            DeliveryResult::Settled(MemberInstructionDeliveryOutcome::Recorded {
                generation: 2,
                ..
            })
        ));
    }

    /// A slot the member's session recorded as desired but not yet in effect
    /// says what it waits for: a model that cannot carry operator
    /// instructions, one that lowers them only inside the leading System
    /// prefix, or the next fresh run start.
    #[test]
    fn a_recorded_slot_waiting_on_its_lowering_says_why() {
        let identity = AgentIdentity::from("member");
        let mut session = Session::new();
        session
            .record_instruction_target(InstructionTargetRecord {
                mob_id: "mob".to_string(),
                agent_identity: "member".to_string(),
                generation: 1,
                slots: vec![InstructionTargetRecordSlot {
                    slot: tone(),
                    revision: InstructionRevision::new(1).unwrap(),
                    body: Some("Be brief.".to_string()),
                }],
            })
            .expect("the snapshot records");
        let read = SessionInstructionRead {
            target: session
                .instruction_target_state()
                .expect("the target reads"),
            effective: meerkat_core::effective_operator_instructions(
                session.id(),
                session.messages(),
            )
            .expect("nothing is in effect yet"),
            lowering: None,
        };
        let record = OperatorInstructionRecordStatus {
            slot: tone(),
            revision: InstructionRevision::new(1).unwrap(),
            effect: meerkat_core::InstructionEffect::Active,
            target: OperatorInstructionTarget::Mob,
            content_sha256: None,
            body_bytes: Some(9),
        };
        for (lowering, reason) in [
            (
                meerkat_core::InstructionLowering::Unavailable,
                MemberInstructionPendingReason::LoweringUnavailable,
            ),
            (
                meerkat_core::InstructionLowering::LeadingPrefixOnly,
                MemberInstructionPendingReason::LeadingPrefixOnlyLowering,
            ),
            (
                meerkat_core::InstructionLowering::MidConversation,
                MemberInstructionPendingReason::AwaitingRunStart,
            ),
        ] {
            let status = member_slot_status(
                &record,
                &identity,
                "worker",
                Some(&read),
                &MemberInstructionDeliveryState::Current,
                false,
                lowering,
            );
            assert_eq!(
                status.state,
                MemberInstructionSlotState::Pending { reason },
                "{lowering:?}"
            );
        }
    }
}
