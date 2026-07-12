//! Member-host observation seam (multi-host mobs §7.4, DEC-P6E-2).
//!
//! The member runtime's comms drain serves three supervisor bridge commands
//! (`ReadMemberHistory`, `PollMemberEvents`, and the directed-turn half of
//! `DeliverMemberInput`) but cannot read what those arms need: session
//! history lives behind the daemon's session service, durable event reads
//! live behind the facade service's event log, and generation + turn-outcome
//! journal facts are `MobHostBindingAuthority` facts on the host daemon.
//! This module defines the injected host trait the drain resolves — the
//! `SessionLlmReconfigureHost` precedent — plus the carrier types and the
//! per-session tracked-turn journal seam (`TrackedTurnJournal`,
//! DEC-P6F-9's registration contract; the meerkat-mob host lane implements
//! both and the daemon installs them).
//!
//! Sequence-domain law (plan gotcha 8): every `seq` in this module is the
//! owning host's durable `StoredEvent.seq` (or, for ephemeral hosts, the
//! host-owned generation-scoped ring seq that substitutes for it). The
//! session-task and per-stream counters never leak into these carriers.

use std::sync::Arc;

use meerkat_contracts::wire::supervisor_bridge::{
    BridgeDeliveryRejectionCause, BridgeMemberIncarnation, BridgeTrackedInputCancelOutcome,
    BridgeTurnOutcomeAck, BridgeTurnOutcomeRecord, WireFlowTurnOutcome,
};
use meerkat_core::event::{AgentEvent, EventEnvelope};
use meerkat_core::service::SessionHistoryPage;
use meerkat_core::time_compat::Duration;
use meerkat_core::types::SessionId;

use crate::completion::CompletionHandle;

/// Maximum standalone JSON size of one durable directed-turn terminal row.
/// This keeps one provider/error detail from permanently bricking the
/// journal or an otherwise bounded bridge page.
pub const MAX_TURN_OUTCOME_RECORD_BYTES: usize = 64 * 1024;

/// Typed failure vocabulary for member observation serving.
#[derive(Debug, thiserror::Error)]
pub enum MemberObservationError {
    /// The addressed host-member residency changed while the observation was
    /// being served. Maps to the wire `StaleFence` authority rejection.
    #[error("stale member observation residency: {reason}")]
    StaleIncarnation { reason: String },
    /// The requested cursor position was pruned from the retained window
    /// (structurally ring-only in v1 — a durable log never overruns). Maps
    /// to the wire `BridgeRejectionCause::StaleCursor`.
    #[error("cursor overran the retained window (watermark {watermark}, generation {generation})")]
    StaleCursor { watermark: u64, generation: u64 },
    /// The cursor names a generation this host never issued (`g > current`;
    /// restore-from-backup / split-brain shape). Fail closed — never serve
    /// under a future-generation cursor (FLAG-P6E-12). Maps to the wire
    /// `BridgeRejectionCause::Internal` with this diagnostic.
    #[error("cursor generation {requested} is ahead of current generation {current}")]
    FutureGenerationCursor { requested: u64, current: u64 },
    /// The observation substrate cannot serve this session right now
    /// (unknown session, missing durable log, halted projection). Maps to
    /// the wire `BridgeRejectionCause::Unavailable` (ADJ-P4-7 vocabulary).
    #[error("member observation unavailable: {reason}")]
    Unavailable { reason: String },
    /// A single outcome row cannot fit the independently bounded outcome
    /// protocol. Rejected before it reaches durable storage.
    #[error("turn outcome record is {encoded_bytes} bytes (maximum {max_bytes})")]
    OutcomeRecordTooLarge {
        encoded_bytes: usize,
        max_bytes: usize,
    },
    /// An invariant was violated while serving. Maps to `Internal`.
    #[error("member observation internal fault: {reason}")]
    Internal { reason: String },
}

/// Domain twin of the wire `BridgeEventCursor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberObservationCursor {
    /// Start at the live tail (skip history).
    Tail,
    /// Resume from a recorded `(generation, seq)` position.
    At { generation: u64, seq: u64 },
}

/// One served page of a member's durable event stream (DEC-P6E-4/5/18).
#[derive(Debug)]
pub struct MemberEventsWindow {
    /// Generation the page was served at — the self-describing seq-domain
    /// reset signal (§14.6).
    pub generation: u64,
    /// Materialization fence paired with `generation`. A same-generation
    /// fence rotation is a distinct observation incarnation.
    pub fence_token: u64,
    /// `(durable seq, envelope)` rows in seq order.
    pub rows: Vec<(u64, EventEnvelope<AgentEvent>)>,
    /// Exact read floor used for this page. Response-size trimming may drop
    /// every event row and must then resume from this value, never skip.
    pub from_seq: u64,
    /// Cursor seq to resume from (durable seq domain).
    pub next_seq: u64,
    /// Highest durable seq the owning host has recorded.
    pub watermark: u64,
    /// Bounded page of retained, unacknowledged turn-outcome journal rows for
    /// the member's current generation.
    pub turn_outcomes: Vec<BridgeTurnOutcomeRecord>,
    /// Whether every currently retained unacknowledged row fit this outcome
    /// page. This does not claim that no delayed commit can appear later.
    pub outcomes_complete: bool,
}

/// One served transcript page (DEC-P6E-6).
#[derive(Debug)]
pub struct MemberHistoryWindow {
    /// Generation the page was read at.
    pub generation: u64,
    /// Domain transcript page; the drain arm projects it through the ONE
    /// wire projection (`WireMemberHistoryPageBody::try_from_history_page`).
    pub page: SessionHistoryPage,
}

/// Authority witness and bounded-read controls for one member event poll.
///
/// Keeping these facts together prevents callers from accidentally pairing
/// acknowledgement rows or cursor controls with a different member
/// incarnation while leaving the addressed session explicit on the host
/// seam.
#[derive(Debug, Clone, Copy)]
pub struct MemberEventsPollRequest<'a> {
    /// Exact resident member incarnation authorized to serve the page.
    pub expected_member: &'a BridgeMemberIncarnation,
    /// Durable event cursor in the owning host's sequence domain.
    pub cursor: MemberObservationCursor,
    /// Maximum event rows requested by the caller.
    pub max: u32,
    /// Bounded long-poll duration.
    pub wait: Duration,
    /// Exact terminal outcome acknowledgements to apply before serving.
    pub outcome_acks: &'a [BridgeTurnOutcomeAck],
    /// Maximum retained terminal outcomes requested by the caller.
    pub max_outcomes: u32,
}

/// Pre-accept event window for a directed turn (DEC-P6E-16's atomicity
/// letter: the subscription exists BEFORE `accept_input_with_completion`
/// runs, so no terminal can slip between accept and watch).
pub struct DirectedTurnWindow {
    /// Full host residency captured with the durable Pending reservation.
    /// Every later cancel/admit/record uses this tuple rather than re-deriving
    /// authority from a possibly replaced session projection.
    pub expected_member: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    /// Live session-events subscription opened before acceptance.
    pub subscription: meerkat_core::comms::EventStream,
    /// Durable watermark at open (+1 = first seq that can belong to this
    /// window). The `terminal_seq` binder scans forward from here.
    pub window_start: u64,
    /// Member generation at open.
    pub generation: u64,
    /// Materialization fence paired with `generation`.
    pub fence_token: u64,
    /// Exact directed-turn input id reserved before runtime acceptance.
    pub input_id: String,
    /// Whether the host persisted/replayed Pending or found an already
    /// terminal exact key. Terminal replay needs no watcher.
    pub tracking: DirectedTurnTracking,
}

/// Subscription-free facts needed to serialize and revalidate final runtime
/// admission. This owned projection is safe to retain across the async trait
/// boundary without requiring [`DirectedTurnWindow`]'s event stream to be
/// `Sync`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectedTurnAdmissionRequest {
    /// Full host residency captured with the durable Pending reservation.
    pub expected_member: BridgeMemberIncarnation,
    /// First durable sequence that can belong to this directed-turn window.
    pub window_start: u64,
    /// Member generation captured when the window opened.
    pub generation: u64,
    /// Materialization fence paired with `generation`.
    pub fence_token: u64,
    /// Exact directed-turn input id reserved before runtime acceptance.
    pub input_id: String,
}

impl DirectedTurnWindow {
    /// Snapshot the exact durable reservation facts used by the final
    /// admission revalidation. The live subscription remains owned by this
    /// window for the terminal watcher.
    #[must_use]
    pub fn admission_request(&self) -> DirectedTurnAdmissionRequest {
        DirectedTurnAdmissionRequest {
            expected_member: self.expected_member.clone(),
            window_start: self.window_start,
            generation: self.generation,
            fence_token: self.fence_token,
            input_id: self.input_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectedTurnTracking {
    /// This request created the durable Pending row and may cancel it after
    /// proving its own runtime admission had no effect.
    PendingFresh,
    /// A previous request already owned durable Pending custody. Failure of
    /// this retry cannot prove the original request had no effect, so it must
    /// never cancel the row or return a definite no-effect rejection.
    PendingReplay,
    TerminalReplay,
}

/// Opaque ownership of the host's exact-key delivery/cancellation admission
/// interval. The comms drain retains this value across runtime acceptance;
/// its drop releases the host-side mutex. The payload is deliberately opaque
/// so runtime code cannot inspect or forge host synchronization state.
pub struct DirectedTurnAdmissionPermit {
    _guard: Box<dyn Send>,
}

impl DirectedTurnAdmissionPermit {
    #[doc(hidden)]
    pub fn new(guard: impl Send + 'static) -> Self {
        Self {
            _guard: Box::new(guard),
        }
    }
}

/// Result of atomically locking and revalidating a previously opened Pending
/// window. `TerminalReplay` means cancellation/terminal custody won the race
/// and runtime admission must not run.
pub enum DirectedTurnAdmissionDecision {
    Admit(DirectedTurnAdmissionPermit),
    TerminalReplay,
}

impl std::fmt::Debug for DirectedTurnWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectedTurnWindow")
            .field("expected_member", &self.expected_member)
            .field("window_start", &self.window_start)
            .field("generation", &self.generation)
            .field("fence_token", &self.fence_token)
            .field("input_id", &self.input_id)
            .field("tracking", &self.tracking)
            .finish_non_exhaustive()
    }
}

/// Admission handoff for one directed (tracked) turn: either a flow directive
/// or a plain interaction with explicit outcome custody. Carries everything the
/// observation host's watcher needs to classify the terminal, bind
/// `terminal_seq`, and journal the outcome (DEC-P6E-16/17).
pub struct DirectedTurnAdmission {
    /// Canonical accepted input id — the journal / dedup / obligation key
    /// (`AcceptOutcome::{Accepted,Deduplicated}` canonical id, which equals
    /// the delivery `input_id` by idempotency-key construction).
    pub input_id: String,
    /// Pre-accept event window (subscription + admission watermark).
    pub window: DirectedTurnWindow,
    /// The completion handle the drain used to discard (`comms_drain.rs`
    /// `_completion_handle`); generated-authority terminal truth.
    /// `None` means runtime authority proved the accepted/deduplicated input
    /// was already terminal during admission. The host still reconstructs the
    /// terminal exhaustively from the original durable window.
    pub completion_handle: Option<CompletionHandle>,
    /// Per-session journal seam registered at residency establishment.
    pub journal: Arc<dyn TrackedTurnJournal>,
}

/// Typed reject for a tracked-turn admission that cannot be tracked.
/// Structural absence maps to the legacy-named `TurnDirectiveUnsupported`; a
/// saturated durable completion journal maps to `OutcomeJournalFull` before
/// runtime acceptance.
#[derive(Debug, thiserror::Error)]
#[error("tracked turn unsupported: {detail}")]
pub struct DirectedTurnReject {
    pub cause: BridgeDeliveryRejectionCause,
    pub detail: String,
    /// Whether this rejection proves no request with the same durable key was
    /// previously accepted. `false` must surface as an outer ambiguous bridge
    /// failure, never `BridgeDeliveryOutcome::Rejected`.
    pub definite_no_effect: bool,
}

impl DirectedTurnReject {
    #[must_use]
    pub fn unsupported(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            cause: BridgeDeliveryRejectionCause::TurnDirectiveUnsupported {
                detail: detail.clone(),
            },
            detail,
            definite_no_effect: true,
        }
    }

    #[must_use]
    pub fn ambiguous(detail: impl Into<String>) -> Self {
        let detail = detail.into();
        Self {
            cause: BridgeDeliveryRejectionCause::TurnDirectiveUnsupported {
                detail: detail.clone(),
            },
            detail,
            definite_no_effect: false,
        }
    }

    #[must_use]
    pub fn outcome_journal_full(retained: usize, limit: usize) -> Self {
        let retained = u32::try_from(retained).unwrap_or(u32::MAX);
        let limit = u32::try_from(limit).unwrap_or(u32::MAX);
        Self {
            cause: BridgeDeliveryRejectionCause::OutcomeJournalFull { retained, limit },
            detail: format!(
                "directed-turn outcome journal is full ({retained}/{limit}); consume and acknowledge outcomes before submitting more work"
            ),
            definite_no_effect: true,
        }
    }
}

/// Machine-wide injected observation host (DEC-P6E-2). Implemented by the
/// mob host daemon (`HostMemberObservation` in meerkat-mob) over the facade
/// service's durable event log, the daemon session service, and the host
/// binding authority's generation + journal facts. Absent host ⇒ the
/// observation arms reply typed `Unavailable` (never `Unsupported`: the
/// command IS served on this engine; the composition lacks the substrate).
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait MemberObservationHost: Send + Sync {
    /// Current member generation for a resident session
    /// (`MobHostBindingAuthority` fact).
    async fn member_generation(&self, session: &SessionId) -> Result<u64, MemberObservationError>;

    /// Transcript page. `from_index: None` = tail-addressed when `limit`
    /// is present, full-from-zero otherwise (DEC-P6E-6).
    async fn read_history(
        &self,
        session: &SessionId,
        from_index: Option<u64>,
        limit: Option<u32>,
    ) -> Result<MemberHistoryWindow, MemberObservationError>;

    /// Bounded long-poll over the member's event log (DEC-P6E-4/5).
    async fn poll_events(
        &self,
        session: &SessionId,
        request: MemberEventsPollRequest<'_>,
    ) -> Result<MemberEventsWindow, MemberObservationError>;

    /// Open the pre-accept event window for a directed turn (DEC-P6E-16
    /// step 2 — MUST run before `accept_input_with_completion`).
    async fn open_directed_turn_window(
        &self,
        session: &SessionId,
        expected_member: &BridgeMemberIncarnation,
        input_id: &str,
    ) -> Result<DirectedTurnWindow, DirectedTurnReject>;

    /// Cancel Pending only when the caller has proven the runtime did not
    /// accept the effect. Ambiguous admission errors must retain the row for
    /// replay/recovery.
    async fn cancel_directed_turn_window(
        &self,
        session: &SessionId,
        window: DirectedTurnWindow,
    ) -> Result<(), DirectedTurnReject>;

    /// Serialize runtime acceptance against exact-key cancellation, then
    /// re-probe durable host authority while the lock is held. A caller may
    /// enter runtime acceptance only with the returned permit alive.
    async fn lock_and_revalidate_directed_turn_admission(
        &self,
        session: &SessionId,
        request: DirectedTurnAdmissionRequest,
    ) -> Result<DirectedTurnAdmissionDecision, DirectedTurnReject> {
        let _ = (session, request);
        Err(DirectedTurnReject::ambiguous(
            "member observation host has no exact-key admission lock".to_string(),
        ))
    }

    /// Level-triggered exact-key cancellation. Implementations durably block
    /// delayed delivery before certifying no-effect/cancellation and return a
    /// terminal only after runtime quiescence.
    async fn cancel_tracked_member_input(
        &self,
        session: &SessionId,
        expected_member: &BridgeMemberIncarnation,
        input_id: &str,
    ) -> Result<BridgeTrackedInputCancelOutcome, MemberObservationError> {
        let _ = (session, expected_member, input_id);
        Err(MemberObservationError::Unavailable {
            reason: "member observation host has no tracked-input cancellation authority"
                .to_string(),
        })
    }

    /// O2: adopt an accepted directive-bearing or explicitly interaction-
    /// tracked delivery as a tracked turn.
    /// Spawns the terminal watcher (classify → bind `terminal_seq` →
    /// `RecordTurnOutcome` through the journal seam).
    async fn admit_directed_turn(
        &self,
        session: &SessionId,
        admission: DirectedTurnAdmission,
    ) -> Result<(), DirectedTurnReject>;
}

/// Small read adapter over the facade service's durable event log
/// (`PersistentSessionService::{event_log_read_from, event_log_latest_seq}`)
/// so the daemon can hand the observation host durable reads without
/// widening `MobSessionService` (DEC-P6E-2).
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait DurableEventLogRead: Send + Sync {
    /// Rows from `from_seq` onward as `(durable seq, envelope)`, or `None`
    /// when the composition has no durable event projection (Memory-backend
    /// realms — the ring substitutes, DEC-P6E-5).
    async fn read_from(
        &self,
        session: &SessionId,
        from_seq: u64,
        max_rows: usize,
    ) -> Result<Option<Vec<(u64, EventEnvelope<AgentEvent>)>>, MemberObservationError>;

    /// Highest durable seq recorded for the session (`None` = no durable
    /// projection composed).
    async fn latest_seq(&self, session: &SessionId) -> Result<Option<u64>, MemberObservationError>;
}

/// One recorded tracked-turn terminal (the journal write carrier). The
/// coarse machine kind is derived from `outcome` by the journal impl; the
/// full wire outcome is retained verbatim as sidecar presentation material
/// recorded once, at classification time, by the one shared classifier.
#[derive(Debug, Clone)]
pub struct TrackedTurnOutcomeRecord {
    pub input_id: String,
    pub generation: u64,
    pub fence_token: u64,
    /// Durable `StoredEvent.seq` of the turn's terminal event (gotcha 8 —
    /// never a watermark approximation, never a `CompletionFeed` seq).
    pub terminal_seq: u64,
    pub outcome: WireFlowTurnOutcome,
}

/// Per-session tracked-turn journal seam (DEC-P6F-9's registration
/// contract, cross-lane name). Implemented in meerkat-mob's host lane,
/// wrapping the generated `MobHostBindingAuthority` record path plus the
/// member's current generation + fence facts; registered by the host daemon at
/// residency establishment (`register_tracked_turn_journal`). No
/// registration ⇒ tracked deliveries reject
/// `TurnDirectiveUnsupported` — the capability gate is structural.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait TrackedTurnJournal: Send + Sync {
    /// Complete residency tuple validated before durable Pending/admission.
    fn member_incarnation(
        &self,
    ) -> &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation;

    /// The member generation this residency was established at.
    fn member_generation(&self) -> u64;

    /// The materialization fence paired with [`Self::member_generation`].
    fn member_fence_token(&self) -> u64;

    /// Durably record one tracked turn's terminal outcome through the
    /// generated `MobHostBindingAuthority` wrapper (witness discipline —
    /// the shell never mutates journal maps directly). Idempotent:
    /// redelivery converges on the machine's `RecordTurnOutcomeReplay`.
    async fn record_turn_outcome(
        &self,
        record: TrackedTurnOutcomeRecord,
    ) -> Result<(), MemberObservationError>;
}
