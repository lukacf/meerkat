//! Operator-instruction delivery: the shell around MobMachine's
//! operator-instruction region.
//!
//! MobMachine owns every lifecycle fact (the records, the generation, each
//! member's owed, recorded and blocked facts); this module realizes what it
//! authorizes and reads what it holds:
//!
//! - the bodies of the Active records, by digest, rebuilt from the journal;
//! - a member's snapshot at the machine's generation (the shared records and
//!   the member-targeted records that name the member, each Active for the
//!   member when its target selects the member);
//! - the snapshot a member's new session carries at birth;
//! - the delivery owner, which asks the machine to prepare each owed
//!   member's delivery and records it in the member's session: through the
//!   session's actor while it is live, store-only while it is dormant, never
//!   waking or materializing the member. Live and dormant deliveries run in
//!   separate bounded lanes, so a live session that holds its turn boundary
//!   never stalls the dormant fan-out. A placed member's session is on its
//!   member host, which records the snapshot there
//!   (`runtime::placed_member_instruction`), in a third lane;
//! - the status read.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures::FutureExt as _;
use futures::stream::{FuturesUnordered, StreamExt};
use meerkat_core::service::{SessionInstructionError, SessionInstructionRead};
use meerkat_core::time_compat::{Duration, Instant};
use meerkat_core::types::SessionId;
use meerkat_core::{
    InstructionContentDigest, InstructionRevision, InstructionSetErrorCode, InstructionSlot,
    InstructionTargetRecord, InstructionTargetRecordSlot, InstructionWriteError,
};

use super::handle::MobHandle;
use super::placed_member_instruction::{
    PlacedInstructionFailure, PlacedMemberInstructionRoute, record_placed_member_instruction_target,
};
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
/// The bound on concurrent deliveries to placed members. Each one is a
/// supervisor bridge round trip to the member's host, so a slow host never
/// stalls the local lanes.
const PLACED_DELIVERY_CONCURRENCY: usize = 16;
const FIRST_RETRY_DELAY: Duration = Duration::from_millis(200);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
const SCAN_DEBOUNCE: Duration = Duration::from_millis(20);

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/// Where the Active body of one record is.
#[derive(Debug, Clone)]
enum RecordBody {
    /// In memory: the body of a shared record (targeted at every member or
    /// at a profile), which many members' snapshots carry.
    Held(Arc<str>),
    /// In the journal row at this cursor: the body of a member-targeted
    /// record, read when the one member it names gets its snapshot. The mob
    /// holds one such record per member that has one, so it keeps where
    /// each body is, never the bodies.
    Journaled(u64),
}

/// The bodies of the mob's Active records, by record slot: content the
/// journal keeps for replay and delivery. A shared record's body is held; a
/// member-targeted record's body stays in its journal row and is read for
/// its member's snapshot. MobMachine holds every record's digest; this store
/// holds no lifecycle fact, and a body is used only at the digest the
/// machine holds for its slot.
#[derive(Debug, Clone, Default)]
pub(crate) struct OperatorInstructionBodies {
    by_slot: BTreeMap<String, (String, RecordBody)>,
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
            if let MobEventKind::OperatorInstructionCommitted(committed) = &event.kind {
                match (&committed.content_sha256, committed.body()) {
                    (Some(digest), Some(body))
                        if committed.effect == meerkat_core::InstructionEffect::Active =>
                    {
                        bodies.record_active(
                            &committed.slot.machine_key(),
                            digest,
                            body,
                            &committed.target,
                            event.cursor,
                        );
                    }
                    _ => bodies.record_revoked(&committed.slot.machine_key()),
                }
            }
        }
        bodies
    }

    /// Follow a committed Active record of `key`, journaled at `cursor`: a
    /// shared record's body is held, a member-targeted record's located.
    pub(crate) fn record_active(
        &mut self,
        key: &str,
        digest: &InstructionContentDigest,
        body: &str,
        target: &OperatorInstructionTarget,
        cursor: u64,
    ) {
        let body = match target {
            OperatorInstructionTarget::Member { .. } => RecordBody::Journaled(cursor),
            OperatorInstructionTarget::Mob | OperatorInstructionTarget::Profile { .. } => {
                RecordBody::Held(Arc::from(body))
            }
        };
        self.by_slot
            .insert(key.to_owned(), (digest.as_str().to_owned(), body));
    }

    /// Follow a committed revoke of `key`: an Absent record keeps no body.
    pub(crate) fn record_revoked(&mut self, key: &str) {
        self.by_slot.remove(key);
    }

    /// Drop what a slot `state` no longer records left behind (a reset
    /// clears the records). It scans only when something is left over.
    pub(crate) fn retain_recorded(&mut self, state: &mob_dsl::MobMachineState) {
        if self.by_slot.len() > state.operator_instruction_revision.len() {
            self.by_slot
                .retain(|key, _| state.operator_instruction_revision.contains_key(key));
        }
    }

    /// The body of the record of `key` at `digest`: a held body, or a
    /// located one read from `journal`, the mob's events in cursor order
    /// (a restore at a start has them; a delivery resolves its member's
    /// bodies first, see [`Self::member_journal_reads`]).
    fn body<'a>(
        &'a self,
        key: &str,
        digest: &str,
        journal: Option<&'a [MobEvent]>,
    ) -> Option<&'a str> {
        let (recorded_digest, body) = self.by_slot.get(key)?;
        if recorded_digest != digest {
            return None;
        }
        match body {
            RecordBody::Held(body) => Some(body.as_ref()),
            RecordBody::Journaled(cursor) => {
                let journal = journal?;
                let event = journal
                    .binary_search_by_key(cursor, |event| event.cursor)
                    .ok()
                    .and_then(|index| journal.get(index))
                    .or_else(|| journal.iter().find(|event| event.cursor == *cursor))?;
                journaled_body(event, key, digest)
            }
        }
    }

    /// The journal rows `agent_identity`'s snapshot needs read: for each
    /// Active member-targeted record that names it and whose body this
    /// store locates, the slot key, the digest and the row's cursor. A
    /// member holds few records of its own, whatever the mob holds.
    pub(crate) fn member_journal_reads(
        &self,
        state: &mob_dsl::MobMachineState,
        agent_identity: &AgentIdentity,
    ) -> Vec<(String, String, u64)> {
        let mut reads = Vec::new();
        for (key, member) in &state.operator_instruction_target_member {
            if member.0.as_str() != agent_identity.as_str()
                || state.operator_instruction_effect.get(key)
                    != Some(&mob_dsl::InstructionEffect::Active)
            {
                continue;
            }
            if let (Some(digest), Some((recorded_digest, RecordBody::Journaled(cursor)))) = (
                state.operator_instruction_digest.get(key),
                self.by_slot.get(key),
            ) && digest == recorded_digest
            {
                reads.push((key.clone(), digest.clone(), *cursor));
            }
        }
        reads
    }

    /// The bodies one member's snapshot reads: the held bodies of the shared
    /// records, and `read`, the member's own bodies as its journal rows
    /// hold them (slot key, digest, body). What a snapshot built off the
    /// mob's actor is handed, so it carries no other member's body.
    pub(crate) fn for_member(&self, read: Vec<(String, String, Arc<str>)>) -> Self {
        let mut by_slot: BTreeMap<String, (String, RecordBody)> = self
            .by_slot
            .iter()
            .filter(|(_, (_, body))| matches!(body, RecordBody::Held(_)))
            .map(|(key, entry)| (key.clone(), entry.clone()))
            .collect();
        for (key, digest, body) in read {
            by_slot.insert(key, (digest, RecordBody::Held(body)));
        }
        Self { by_slot }
    }

    /// How many bytes of bodies this store holds in memory.
    #[cfg(test)]
    pub(crate) fn held_body_bytes(&self) -> usize {
        self.by_slot
            .values()
            .map(|(_, body)| match body {
                RecordBody::Held(body) => body.len(),
                RecordBody::Journaled(_) => 0,
            })
            .sum()
    }
}

/// The body `event` journals for the record of `key` at `digest`, when it is
/// that record's committed row.
fn journaled_body<'a>(event: &'a MobEvent, key: &str, digest: &str) -> Option<&'a str> {
    match &event.kind {
        MobEventKind::OperatorInstructionCommitted(committed)
            if committed.slot.machine_key() == key
                && committed
                    .content_sha256
                    .as_ref()
                    .is_some_and(|recorded| recorded.as_str() == digest) =>
        {
            committed.body()
        }
        _ => None,
    }
}

/// How many journal rows [`read_journaled_bodies`] has read in this process:
/// a row's probe that a step read none.
#[cfg(test)]
pub(crate) static JOURNALED_BODY_ROW_READS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Read from the mob's journal the member-targeted bodies a snapshot needs
/// ([`OperatorInstructionBodies::member_journal_reads`]): one row per
/// record, by its cursor. A row that is not the record's committed row at
/// its digest fails the read, typed, so a snapshot never carries another
/// body.
pub(crate) async fn read_journaled_bodies(
    events: &dyn crate::store::MobEventStore,
    reads: Vec<(String, String, u64)>,
) -> Result<Vec<(String, String, Arc<str>)>, MobError> {
    let mut read = Vec::with_capacity(reads.len());
    for (key, digest, cursor) in reads {
        #[cfg(test)]
        JOURNALED_BODY_ROW_READS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let rows = events
            .poll(cursor.saturating_sub(1), 1)
            .await
            .map_err(MobError::from)?;
        let body = rows
            .first()
            .filter(|event| event.cursor == cursor)
            .and_then(|event| journaled_body(event, &key, &digest))
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "the body of operator instruction {key} (digest {digest}) is not in the \
                     mob's journal at row {cursor}"
                ))
            })?;
        read.push((key, digest, Arc::from(body)));
    }
    Ok(read)
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

    pub(crate) fn record_failure(&self, agent_identity: &AgentIdentity, failure: String) {
        self.last_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(agent_identity.clone(), failure);
    }

    pub(crate) fn clear_failure(&self, agent_identity: &AgentIdentity) {
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

/// Whether the record of `key` in `state` is member-targeted at a member
/// other than `agent_identity`: such a record is in that member's snapshot
/// only.
fn record_targets_another_member(
    state: &mob_dsl::MobMachineState,
    key: &str,
    agent_identity: &AgentIdentity,
) -> bool {
    state.operator_instruction_target_kind.get(key) == Some(&mob_dsl::InstructionTargetKind::Member)
        && state
            .operator_instruction_target_member
            .get(key)
            .is_some_and(|member| member.0.as_str() != agent_identity.as_str())
}

/// One member's whole snapshot at the machine's generation: the shared
/// records (every member's, or a profile's) and the member-targeted records
/// that name this member, each with its body when it is Active and its
/// target selects the member (Absent otherwise, so a member that left a
/// profile's selector revokes the slot). Another member's member-targeted
/// record is not in it: a record that moves to another member leaves this
/// member's snapshot, and its session revokes a slot its target no longer
/// names. So a snapshot's size follows the shared records and the member's
/// own, never the number of other members that hold a record.
///
/// `bodies` holds the shared records' bodies. The member's own bodies come
/// from it too when the caller resolved them
/// ([`OperatorInstructionBodies::for_member`]), or from `journal`, the mob's
/// events in cursor order, at a start that has them in hand.
pub(crate) fn member_instruction_record(
    state: &mob_dsl::MobMachineState,
    bodies: &OperatorInstructionBodies,
    journal: Option<&[MobEvent]>,
    mob_id: &MobId,
    agent_identity: &AgentIdentity,
    member_profile: &str,
) -> Result<InstructionTargetRecord, MobError> {
    let mut slots = Vec::new();
    for (key, revision) in &state.operator_instruction_revision {
        if record_targets_another_member(state, key, agent_identity) {
            continue;
        }
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
            let body = bodies.body(key, digest, journal).ok_or_else(|| {
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

/// How many current members the record of `key` selects. A member-targeted
/// record selects its one member when it is seated: it is looked up, not
/// searched for among the seats.
pub(crate) fn matched_members(state: &mob_dsl::MobMachineState, key: &str) -> u64 {
    if state.operator_instruction_target_kind.get(key)
        == Some(&mob_dsl::InstructionTargetKind::Member)
    {
        return state
            .operator_instruction_target_member
            .get(key)
            .is_some_and(|member| state.identity_to_runtime.contains_key(member))
            .into();
    }
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

/// Whether a member's build carries its snapshot at birth: the mob has
/// recorded a generation and the build mints its session. A build that
/// continues an existing session carries nothing (that member receives its
/// snapshot by delivery).
pub(crate) fn birth_carries_member_instructions(
    request: &meerkat_core::service::CreateSessionRequest,
    state: &mob_dsl::MobMachineState,
) -> bool {
    state.operator_instruction_generation != 0
        && request.build.as_ref().is_some_and(|build| {
            build.session_build_intent() == meerkat_core::SessionBuildIntent::Mint
                && build.resume_session.is_some()
        })
}

/// Carry a member's snapshot in the session its build mints: the new session
/// is born with its target, so its first run applies it, and the member's
/// owed delivery then answers "already recorded" without a write. A build
/// that continues an existing session carries nothing (that member receives
/// its snapshot by delivery), nor does a mob that never recorded a
/// generation. `bodies` and `journal` are as
/// [`member_instruction_record`] reads them.
pub(crate) fn carry_member_instruction_birth(
    request: &mut meerkat_core::service::CreateSessionRequest,
    state: &mob_dsl::MobMachineState,
    bodies: &OperatorInstructionBodies,
    journal: Option<&[MobEvent]>,
    mob_id: &MobId,
    agent_identity: &AgentIdentity,
    member_profile: &str,
) -> Result<(), MobError> {
    if !birth_carries_member_instructions(request, state) {
        return Ok(());
    }
    let Some(carrier) = request
        .build
        .as_mut()
        .and_then(|build| build.resume_session.as_mut())
    else {
        return Ok(());
    };
    let record = member_instruction_record(
        state,
        bodies,
        journal,
        mob_id,
        agent_identity,
        member_profile,
    )?;
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
    /// Record this snapshot in a placed member's session, on its member
    /// host over the supervisor bridge.
    DeliverPlaced {
        delivery: MemberInstructionDelivery,
        route: PlacedMemberInstructionRoute,
    },
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
pub(crate) enum DeliveryResult {
    Settled(MemberInstructionDeliveryOutcome),
    Retry(String),
}

/// Record one member's prepared snapshot in its local session and classify
/// the answer, as the delivery owner does: under the member's delivery lock,
/// and only while the member's release epoch is still `epoch`, the one read
/// before the snapshot was prepared (a release that landed since moved it
/// on, and nothing is written after it: `None`). A member's wake records
/// what the member is owed through this before the member is Live.
pub(crate) async fn record_prepared_member_instruction_target(
    session_service: Arc<dyn super::session_service::MobSessionService>,
    lock: Arc<tokio::sync::Mutex<u64>>,
    epoch: u64,
    agent_identity: AgentIdentity,
    delivery: MemberInstructionDelivery,
) -> Option<DeliveryResult> {
    let session_id = delivery.session_id().clone();
    let generation = delivery.generation();
    let held = lock.lock().await;
    if *held != epoch {
        return None;
    }
    let result = classify_delivery_result(
        &agent_identity,
        &session_id,
        generation,
        session_service
            .record_member_instruction_target(delivery)
            .await,
    );
    drop(held);
    Some(result)
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

/// How the delivery owner reads a placed member's host's answer: as a local
/// session's for a record or a typed refusal; a member host that does not
/// serve the command blocks the member `member_host_unsupported`; another
/// incarnation on the host, or a round trip that did not land, is retried.
fn classify_placed_delivery_result(
    agent_identity: &AgentIdentity,
    session_id: &SessionId,
    generation: u64,
    result: Result<meerkat_core::InstructionRecordReceipt, PlacedInstructionFailure>,
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
        Ok(receipt) => {
            classify_delivery_result(agent_identity, session_id, generation, Ok(receipt))
        }
        Err(PlacedInstructionFailure::HostUnsupported) => {
            blocked(mob_dsl::MemberInstructionBlock::MemberHostUnsupported)
        }
        // A record that cannot fit the comms frame can never reach the
        // member host: the member is blocked until a write changes its
        // snapshot, not retried.
        Err(PlacedInstructionFailure::Oversized { bytes }) => {
            tracing::warn!(
                member = %agent_identity,
                bytes,
                "a placed member's operator-instruction record is past the bridge frame budget; \
                 the member is blocked member_host_unsupported until a write changes its snapshot"
            );
            blocked(mob_dsl::MemberInstructionBlock::MemberHostUnsupported)
        }
        Err(PlacedInstructionFailure::Refused(code)) => match code {
            // A governed member host takes no operator-instruction record
            // without prepared authorization, which nothing mints for one
            // yet: final, not retried.
            InstructionSetErrorCode::PreparedAuthorizationRequired => {
                blocked(mob_dsl::MemberInstructionBlock::MemberHostUnsupported)
            }
            InstructionSetErrorCode::DurabilityUnavailable => {
                blocked(mob_dsl::MemberInstructionBlock::DurabilityUnavailable)
            }
            InstructionSetErrorCode::OwnedBySession => {
                blocked(mob_dsl::MemberInstructionBlock::OwnedBySession)
            }
            InstructionSetErrorCode::OwnedByOtherMob => {
                blocked(mob_dsl::MemberInstructionBlock::OwnedByOtherMob)
            }
            InstructionSetErrorCode::SlotManagedByActivation => {
                blocked(mob_dsl::MemberInstructionBlock::SlotManagedByActivation)
            }
            other => DeliveryResult::Retry(format!(
                "the member's session on its member host refused the record: {other:?}"
            )),
        },
        Err(PlacedInstructionFailure::Stale) => DeliveryResult::Retry(
            "the member host holds another incarnation of the member".to_string(),
        ),
        Err(PlacedInstructionFailure::Failed(error)) => DeliveryResult::Retry(format!(
            "the record did not reach the member's host: {error}"
        )),
    }
}

/// One owed member's delivery, as the delivery owner schedules it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryLane {
    Live,
    Dormant,
    Placed,
}

#[derive(Debug)]
struct RetryState {
    owed_generation: u64,
    delay: Duration,
    next_attempt: Instant,
}

/// The members a scan may deliver to now, from one published machine state:
/// owed, in a running or stopped mob, with a born and bound session, not
/// retiring and with its spawn settled, local or placed alike (a placed
/// member's delivery goes to its member host). MobMachine decides each
/// delivery when it is prepared; this only schedules.
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
            !state.member_seat_session_unborn.contains(*identity)
                && state.member_session_bindings.contains_key(*identity)
                && state
                    .identity_to_runtime
                    .get(*identity)
                    .and_then(|runtime| state.member_state_markers.get(runtime))
                    != Some(&mob_dsl::MobMemberState::Retiring)
                && !state.spawn_exec_phase.contains_key(*identity)
        })
        .map(|(identity, generation)| {
            let lane = if state.member_placement.contains_key(identity) {
                DeliveryLane::Placed
            } else {
                match state.member_materialization.get(identity) {
                    Some(
                        mob_dsl::MemberMaterializationPhase::Live
                        | mob_dsl::MemberMaterializationPhase::Waking,
                    ) => DeliveryLane::Live,
                    _ => DeliveryLane::Dormant,
                }
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
    placed: FuturesUnordered<DeliveryFuture>,
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
                Some(settled) = lanes.placed.next(), if !lanes.placed.is_empty() => {
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
                DeliveryLane::Placed => (&mut lanes.placed, PLACED_DELIVERY_CONCURRENCY),
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
    /// session records the sealed snapshot (a placed member's on its member
    /// host). `None` when the machine had nothing to deliver.
    async fn deliver_one(
        handle: MobHandle,
        shared: Arc<OperatorInstructionDeliveryShared>,
        agent_identity: AgentIdentity,
    ) -> DeliveryAttempt {
        let lock = shared.member_lock(&agent_identity);
        let epoch = *lock.lock().await;
        let (delivery, route) = match handle
            .prepare_member_instruction_delivery(&agent_identity)
            .await
        {
            Ok(MemberInstructionPreparation::Deliver(delivery)) => (delivery, None),
            Ok(MemberInstructionPreparation::DeliverPlaced { delivery, route }) => {
                (delivery, Some(route))
            }
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
        // The member's release waits for this write; the lock is let go
        // before the feedback goes to the actor. A release that landed since
        // this delivery began moved the epoch on: nothing is written after
        // it.
        let held = lock.lock().await;
        if *held != epoch {
            return (agent_identity, None);
        }
        let result = match route {
            None => classify_delivery_result(
                &agent_identity,
                &session_id,
                generation,
                Arc::clone(&handle.session_service)
                    .record_member_instruction_target(delivery)
                    .await,
            ),
            Some(route) => classify_placed_delivery_result(
                &agent_identity,
                &session_id,
                generation,
                record_placed_member_instruction_target(
                    &handle.supervisor_bridge,
                    &route,
                    &delivery,
                )
                .await,
            ),
        };
        drop(held);
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
        while let Some(Some(next)) = lanes.placed.next().now_or_never() {
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
    /// per-slot body limit, the slot limit and the active budget of a
    /// member's snapshot),
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

    /// How many members are owed an operator-instruction snapshot their
    /// sessions have not recorded, from the mob machine's published state:
    /// no session is read. A non-authoritative composition (a host's
    /// candidate launch) delivers none of them, so a host can say how many
    /// it leaves to the authoritative launch.
    #[must_use]
    pub fn operator_instruction_owed_members(&self) -> usize {
        self.machine_state_watch_rx
            .borrow()
            .member_instruction_owed
            .len()
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
    /// for the page. An unborn seat, a member without a session binding and
    /// a placed member (its session is on its member host) are not read.
    async fn read_page_member_instructions(
        &self,
        state: &mob_dsl::MobMachineState,
        page: &[AgentIdentity],
    ) -> BTreeMap<AgentIdentity, Result<SessionInstructionRead, SessionInstructionError>> {
        let mut members = Vec::new();
        let mut session_ids = Vec::new();
        for identity in page {
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
            if state.member_seat_session_unborn.contains(&dsl_identity)
                || state.member_placement.contains_key(&dsl_identity)
            {
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
        let member_host = state.member_placement.get(&dsl_identity);
        let (read, effective_read) = match (fetched, unborn, member_host) {
            // An unborn seat has no session to read: nothing is effective.
            (_, true, _) => (None, MemberInstructionEffectiveRead::Ok),
            // A placed member's session is on its member host, which this
            // read does not reach: its delivery facts are the mob's.
            (_, false, Some(host)) => (
                None,
                MemberInstructionEffectiveRead::Unreadable {
                    reason: format!(
                        "the member's session is on member host {}, which this read does not \
                         reach; the delivery state is the mob's",
                        host.as_str()
                    ),
                },
            ),
            (Some(Ok(read)), false, None) => (Some(read), MemberInstructionEffectiveRead::Ok),
            (Some(Err(error)), false, None) => (
                None,
                MemberInstructionEffectiveRead::Unreadable {
                    reason: error.to_string(),
                },
            ),
            (None, false, None) => (
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
        // The records of the member's snapshot: the shared records and its
        // own. Another member's member-targeted record is listed only while
        // this member's session still records it or has it in effect (a
        // record that moved away, until the session revokes it).
        let slots = desired
            .iter()
            .filter(|record| match &record.target {
                OperatorInstructionTarget::Member { agent_identity }
                    if agent_identity != identity =>
                {
                    read.as_ref().is_some_and(|read| {
                        read.target
                            .as_ref()
                            .is_some_and(|target| target.slot(&record.slot).is_some())
                            || read.effective.slot(&record.slot).is_some_and(|slot| {
                                slot.effect() == meerkat_core::InstructionEffect::Active
                            })
                    })
                }
                _ => true,
            })
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
    } else if recorded.is_some_and(|view| view != desired)
        || (recorded.is_none()
            && (desired.effect == meerkat_core::InstructionEffect::Active
                || matches!(delivery, MemberInstructionDeliveryState::Owed { .. })))
    {
        // A slot the session's target does not name is recorded as Absent
        // for this member once its delivery landed: a snapshot leaves out a
        // record that does not concern its member.
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

    fn committed_row(
        cursor: u64,
        key: &str,
        revision: u64,
        body: Option<&str>,
        target: OperatorInstructionTarget,
        generation: u64,
    ) -> MobEvent {
        let slot = InstructionSlot {
            namespace: InstructionNamespace::new("app.example").unwrap(),
            key: InstructionKey::new(key).unwrap(),
        };
        MobEvent {
            cursor,
            timestamp: chrono::Utc::now(),
            mob_id: MobId::from("mob"),
            kind: MobEventKind::OperatorInstructionCommitted(
                crate::operator_instruction::OperatorInstructionCommittedEvent {
                    slot,
                    revision: InstructionRevision::new(revision).unwrap(),
                    effect: if body.is_some() {
                        meerkat_core::InstructionEffect::Active
                    } else {
                        meerkat_core::InstructionEffect::Absent
                    },
                    content_sha256: body.map(InstructionContentDigest::for_body),
                    body_bytes: body.map_or(0, |body| body.len() as u64),
                    target,
                    generation,
                    body: body.map(str::to_owned),
                },
            ),
        }
    }

    fn member(name: &str) -> OperatorInstructionTarget {
        OperatorInstructionTarget::Member {
            agent_identity: AgentIdentity::from(name),
        }
    }

    fn slot_key(key: &str) -> String {
        InstructionSlot {
            namespace: InstructionNamespace::new("app.example").unwrap(),
            key: InstructionKey::new(key).unwrap(),
        }
        .machine_key()
    }

    /// The stored form of a committed record and of a reset names what the
    /// storage doctor's census of a mob's totals reads without this crate
    /// (`meerkat_store::doctor::operator_instruction_mob_totals`): the
    /// kind's type tag, the slot, the effect and the body bytes.
    #[test]
    fn the_stored_rows_name_what_the_storage_doctor_counts() {
        let stored = |event: &MobEvent| -> serde_json::Value {
            serde_json::from_slice(
                &crate::event::encode_stored_mob_event(event).expect("the row encodes"),
            )
            .expect("the stored row is JSON")
        };
        let active = stored(&committed_row(
            4,
            "agent-a",
            1,
            Some("You are agent a."),
            member("a"),
            1,
        ));
        let kind = &active["event"]["kind"];
        assert_eq!(kind["type"], "operator_instruction_committed");
        assert_eq!(kind["slot"]["namespace"], "app.example");
        assert_eq!(kind["slot"]["key"], "agent-a");
        assert_eq!(kind["effect"], "active");
        assert_eq!(kind["body_bytes"], 16);
        let revoked = stored(&committed_row(5, "agent-a", 2, None, member("a"), 2));
        assert_eq!(revoked["event"]["kind"]["effect"], "absent");
        assert_eq!(revoked["event"]["kind"]["body_bytes"], 0);
        let reset = stored(&MobEvent {
            cursor: 6,
            timestamp: chrono::Utc::now(),
            mob_id: MobId::from("mob"),
            kind: MobEventKind::MobReset,
        });
        assert_eq!(reset["event"]["kind"]["type"], "mob_reset");
    }

    /// The body store holds the shared records' bodies and only locates the
    /// member-targeted ones: a member's body is read from its journal row
    /// for that member's snapshot, a revoked or superseded body is gone, a
    /// record that becomes shared is held, and a body is never served at
    /// another digest than the one asked for.
    #[test]
    fn the_body_store_holds_shared_bodies_and_locates_member_bodies() {
        let shared = "Answer formally.";
        let own_a = "You are agent a. ".repeat(64);
        let own_b_first = "You are agent b.";
        let own_b = "You are agent b, revised.";
        let journal = vec![
            committed_row(
                3,
                "tone",
                1,
                Some(shared),
                OperatorInstructionTarget::Mob,
                1,
            ),
            committed_row(5, "agent-a", 1, Some(&own_a), member("a"), 2),
            committed_row(6, "agent-b", 1, Some(own_b_first), member("b"), 3),
            committed_row(9, "agent-b", 2, Some(own_b), member("b"), 4),
            committed_row(11, "agent-c", 1, Some("You are agent c."), member("c"), 5),
            committed_row(12, "agent-c", 2, None, member("c"), 6),
        ];
        let bodies = OperatorInstructionBodies::from_events(&journal);
        assert_eq!(
            bodies.held_body_bytes(),
            shared.len(),
            "only the shared record's body is in memory"
        );
        let digest = |body: &str| InstructionContentDigest::for_body(body).as_str().to_owned();
        // The shared body is served without a journal.
        assert_eq!(
            bodies.body(&slot_key("tone"), &digest(shared), None),
            Some(shared)
        );
        // A member's body needs its journal row, and is served at its
        // current digest only.
        assert_eq!(
            bodies.body(&slot_key("agent-a"), &digest(&own_a), None),
            None
        );
        assert_eq!(
            bodies.body(&slot_key("agent-a"), &digest(&own_a), Some(&journal)),
            Some(own_a.as_str())
        );
        assert_eq!(
            bodies.body(&slot_key("agent-b"), &digest(own_b), Some(&journal)),
            Some(own_b)
        );
        assert_eq!(
            bodies.body(&slot_key("agent-b"), &digest(own_b_first), Some(&journal)),
            None,
            "a superseded body is not served"
        );
        assert_eq!(
            bodies.body(
                &slot_key("agent-c"),
                &digest("You are agent c."),
                Some(&journal)
            ),
            None,
            "a revoked record keeps no body"
        );

        // The rows one member's snapshot reads are its own Active records'.
        let mut state = mob_dsl::MobMachineState::default();
        for (key, body, target) in [("agent-a", own_a.as_str(), "a"), ("agent-b", own_b, "b")] {
            let key = slot_key(key);
            state.operator_instruction_revision.insert(key.clone(), 1);
            state
                .operator_instruction_effect
                .insert(key.clone(), mob_dsl::InstructionEffect::Active);
            state
                .operator_instruction_digest
                .insert(key.clone(), digest(body));
            state
                .operator_instruction_target_kind
                .insert(key.clone(), mob_dsl::InstructionTargetKind::Member);
            state.operator_instruction_target_member.insert(
                key,
                mob_dsl::AgentIdentity::from_domain(&AgentIdentity::from(target)),
            );
        }
        let reads = bodies.member_journal_reads(&state, &AgentIdentity::from("b"));
        assert_eq!(
            reads,
            vec![(slot_key("agent-b"), digest(own_b), 9)],
            "member b reads its own row, at the cursor of its current revision"
        );
        assert!(
            bodies
                .member_journal_reads(&state, &AgentIdentity::from("z"))
                .is_empty(),
            "a member without a record of its own reads no row"
        );
        let for_b = bodies.for_member(vec![(slot_key("agent-b"), digest(own_b), Arc::from(own_b))]);
        assert_eq!(
            for_b.body(&slot_key("agent-b"), &digest(own_b), None),
            Some(own_b)
        );
        assert_eq!(
            for_b.body(&slot_key("tone"), &digest(shared), None),
            Some(shared)
        );
        assert_eq!(
            for_b.body(&slot_key("agent-a"), &digest(&own_a), Some(&journal)),
            None,
            "member b's bodies carry no other member's"
        );
        assert_eq!(for_b.held_body_bytes(), shared.len() + own_b.len());

        // A member's record that becomes a shared record is held; a reset
        // epoch starts empty.
        let mut retargeted = journal.clone();
        retargeted.push(committed_row(
            13,
            "agent-a",
            2,
            Some(&own_a),
            OperatorInstructionTarget::Mob,
            7,
        ));
        assert_eq!(
            OperatorInstructionBodies::from_events(&retargeted).held_body_bytes(),
            shared.len() + own_a.len()
        );
        let mut reset = journal;
        reset.push(MobEvent {
            cursor: 20,
            timestamp: chrono::Utc::now(),
            mob_id: MobId::from("mob"),
            kind: MobEventKind::MobReset,
        });
        let after_reset = OperatorInstructionBodies::from_events(&reset);
        assert_eq!(after_reset.held_body_bytes(), 0);
        assert_eq!(
            after_reset.body(&slot_key("tone"), &digest(shared), Some(&reset)),
            None
        );
    }

    /// A placed member's host answer reads like a local session's: a record
    /// settles it, and so does a refusal no retry changes; a member host that
    /// does not serve the record, a governed one (the record needs prepared
    /// authorization) and a record too large for the comms frame block the
    /// member `member_host_unsupported`; another incarnation on the host, any
    /// other refusal and a round trip that did not land leave it owed and
    /// are retried.
    #[test]
    fn a_placed_members_host_answer_settles_blocks_or_retries() {
        let identity = AgentIdentity::from("placed");
        let session_id = SessionId::new();
        let classify = |result| classify_placed_delivery_result(&identity, &session_id, 3, result);
        assert!(matches!(
            classify(Ok(InstructionRecordReceipt {
                disposition: InstructionRecordDisposition::Recorded,
                target_generation: 3,
            })),
            DeliveryResult::Settled(MemberInstructionDeliveryOutcome::Recorded {
                generation: 3,
                ..
            })
        ));
        for (failure, expected) in [
            (
                PlacedInstructionFailure::HostUnsupported,
                mob_dsl::MemberInstructionBlock::MemberHostUnsupported,
            ),
            (
                PlacedInstructionFailure::Refused(
                    InstructionSetErrorCode::PreparedAuthorizationRequired,
                ),
                mob_dsl::MemberInstructionBlock::MemberHostUnsupported,
            ),
            (
                PlacedInstructionFailure::Oversized { bytes: 1_100_000 },
                mob_dsl::MemberInstructionBlock::MemberHostUnsupported,
            ),
            (
                PlacedInstructionFailure::Refused(InstructionSetErrorCode::OwnedBySession),
                mob_dsl::MemberInstructionBlock::OwnedBySession,
            ),
            (
                PlacedInstructionFailure::Refused(InstructionSetErrorCode::OwnedByOtherMob),
                mob_dsl::MemberInstructionBlock::OwnedByOtherMob,
            ),
            (
                PlacedInstructionFailure::Refused(InstructionSetErrorCode::SlotManagedByActivation),
                mob_dsl::MemberInstructionBlock::SlotManagedByActivation,
            ),
        ] {
            assert!(
                matches!(
                    classify(Err(failure)),
                    DeliveryResult::Settled(MemberInstructionDeliveryOutcome::Blocked {
                        generation: 3,
                        block,
                        ..
                    }) if block == expected
                ),
                "{expected:?}"
            );
        }
        // A store that cannot take the record now (`durability_unavailable`)
        // leaves the member owed, as every failure but the three owner
        // refusals does: this path no longer produces the block kind.
        for failure in [
            PlacedInstructionFailure::Stale,
            PlacedInstructionFailure::Refused(InstructionSetErrorCode::MalformedInstructionHistory),
            PlacedInstructionFailure::Refused(InstructionSetErrorCode::DurabilityUnavailable),
            PlacedInstructionFailure::Failed(MobError::Internal(
                "the round trip timed out".to_string(),
            )),
        ] {
            assert!(matches!(classify(Err(failure)), DeliveryResult::Retry(_)));
        }
    }

    /// A local session's answer blocks the member only for the three
    /// refusals an operator has to clear (the session, another mob or an
    /// activation owns the target or a slot). Every other failure leaves the
    /// member owed and is retried: a store that cannot take the record now
    /// (`durability_unavailable`) included, so no answer of a session
    /// produces the `durability_unavailable` block kind any more.
    #[test]
    fn a_local_sessions_answer_blocks_only_on_the_three_owner_refusals() {
        use meerkat_core::service::{SessionError, SessionInstructionError};
        let identity = AgentIdentity::from("member");
        let session_id = SessionId::new();
        let classify = |result| classify_delivery_result(&identity, &session_id, 3, result);
        for (refusal, expected) in [
            (
                InstructionWriteError::OwnedBySession,
                mob_dsl::MemberInstructionBlock::OwnedBySession,
            ),
            (
                InstructionWriteError::OwnedByOtherMob,
                mob_dsl::MemberInstructionBlock::OwnedByOtherMob,
            ),
        ] {
            assert!(
                matches!(
                    classify(Err(SessionInstructionError::Refused(refusal))),
                    DeliveryResult::Settled(MemberInstructionDeliveryOutcome::Blocked {
                        generation: 3,
                        block,
                        ..
                    }) if block == expected
                ),
                "{expected:?}"
            );
        }
        for failure in [
            SessionInstructionError::DurabilityUnavailable,
            SessionInstructionError::Session(SessionError::NotFound {
                id: session_id.clone(),
            }),
            SessionInstructionError::Refused(InstructionWriteError::OwnedByMob),
        ] {
            let shown = failure.to_string();
            assert!(
                matches!(classify(Err(failure)), DeliveryResult::Retry(_)),
                "{shown} leaves the member owed"
            );
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
