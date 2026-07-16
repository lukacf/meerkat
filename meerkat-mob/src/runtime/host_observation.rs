//! Member-host observation substrate (multi-host mobs §7.4, DEC-P6E-2/4/5/6/16/17/18).
//!
//! `HostMemberObservation` implements the runtime's injected
//! [`MemberObservationHost`] seam over three inputs the daemon composes:
//!
//! 1. the facade service's durable event log (via [`DurableEventLogRead`]) —
//!    the single read authority for served event pages and `terminal_seq`
//!    binding (gotcha 8: the durable `StoredEvent.seq` domain only);
//! 2. the daemon session service (history reads + session-events wake
//!    subscriptions — the live stream is never the served payload);
//! 3. the host actor's observation projection (session → mob/identity/
//!    generation facts + retained turn-outcome journal rows) and its
//!    journal record channel (`RecordTurnOutcome` through the generated
//!    `MobHostBindingAuthority` wrapper on the actor task).
//!
//! Ephemeral realms (no durable event projection) degrade to a bounded
//! per-resident-session ring behind the same seam (DEC-P6E-5): a host-owned
//! generation-scoped seq domain that dies with the process — exactly the
//! declared-degradation contract (`durable_sessions=false` was declared at
//! bind). Directive-bearing turns never reach ephemeral hosts
//! (`RejectedHostIncapable` at dispatch), so the ring never carries a
//! `turn_outcomes` sidecar.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use futures::StreamExt as _;
use tokio::sync::{Notify, mpsc, oneshot, watch};

use meerkat_contracts::wire::supervisor_bridge::{
    BRIDGE_TURN_OUTCOME_ACK_MAX, BridgeDeliveryRejectionCause, BridgeTrackedInputCancelOutcome,
    BridgeTurnOutcomeAck, BridgeTurnOutcomeRecord, WireFlowFailureDetail, WireFlowTurnOutcome,
};
use meerkat_core::event::{AgentEvent, EventEnvelope, EventSourceIdentity};
use meerkat_core::interaction::InteractionId;
use meerkat_core::service::{SessionHistoryQuery, SessionServiceHistoryExt};
use meerkat_core::types::SessionId;
use meerkat_runtime::member_observation::{
    DirectedTurnAdmission, DirectedTurnAdmissionDecision, DirectedTurnAdmissionPermit,
    DirectedTurnAdmissionRequest, DirectedTurnReject, DirectedTurnTracking, DirectedTurnWindow,
    DurableEventLogRead, MAX_TURN_OUTCOME_RECORD_BYTES, MemberEventsPollRequest,
    MemberEventsWindow, MemberHistoryWindow, MemberObservationCursor, MemberObservationError,
    MemberObservationHost, TrackedTurnJournal, TrackedTurnOutcomeRecord,
};

use super::session_service::MobSessionService;

/// Default ephemeral ring capacity per resident session (DEC-P6E-24).
pub(crate) const MEMBER_EVENT_RING_CAPACITY: usize = 1024;

/// Server page cap for history serving (DEC-P6E-6/24).
pub(crate) const HISTORY_PAGE_MAX: usize = 256;

/// Independent terminal-sidecar page ceiling. Count bounding is backed by
/// response byte bounding in the comms drain.
pub(crate) const TURN_OUTCOME_PAGE_MAX: usize = 64;

/// Hard per-member bound on durable, unacknowledged directed-turn terminals.
/// Each row is independently capped at 64 KiB, so retained completion
/// authority is bounded to at most 16 MiB plus small row framing per member.
pub(crate) const TURN_OUTCOME_RETAINED_MAX: usize = 256;

/// Long-poll fallback re-read tick (DEC-P6E-4).
const POLL_FALLBACK_TICK: Duration = Duration::from_millis(250);

/// Bounded liveness window after generated completion authority resolves.
/// A projection that misses this budget leaves the durable Pending row in
/// place for recovery; elapsed time is never rewritten into `ChannelClosed`.
const POST_COMPLETION_TERMINAL_BIND_TIMEOUT: Duration = Duration::from_secs(30);

/// Retry cadence for accepted Pending reconciliation. Backoff is bounded:
/// runtime registration/readiness has no dedicated host signal, so a Pending
/// projection keeps a low-rate retry alive until exact acceptance can attach.
const PENDING_RECOVERY_RETRY_MIN: Duration = Duration::from_millis(100);
const PENDING_RECOVERY_RETRY_MAX: Duration = Duration::from_secs(5);

/// Bounded durable scan page while locating the exact terminal envelope.
const TERMINAL_SEQ_SCAN_PAGE: usize = 256;

const EVENT_SEQUENCE_EXHAUSTED: &str =
    "member event sequence space is exhausted; no successor cursor can be represented";

fn checked_event_sequence_successor(
    sequence: u64,
    context: &'static str,
) -> Result<u64, MemberObservationError> {
    sequence
        .checked_add(1)
        .ok_or_else(|| MemberObservationError::Internal {
            reason: format!("{context}: {EVENT_SEQUENCE_EXHAUSTED}"),
        })
}

fn directed_turn_window_start(watermark: u64) -> Result<u64, Box<DirectedTurnReject>> {
    let window_start = watermark.checked_add(1).ok_or_else(|| {
        Box::new(DirectedTurnReject::unsupported(format!(
            "cannot reserve directed-turn Pending before runtime acceptance: {EVENT_SEQUENCE_EXHAUSTED}"
        )))
    })?;
    // A structured-output turn can minimally emit RunStarted, the
    // extraction-required RunCompleted, then ExtractionSucceeded/Failed. The
    // final terminal must also have a representable exclusive frontier for
    // sidecar eligibility and controller cursor advancement. Reserving any
    // smaller suffix would create Pending custody that cannot converge even
    // for that minimal event-bearing shape. (Arbitrary display/tool events
    // may consume more; sequence exhaustion remains a fail-closed boundary.)
    window_start
        .checked_add(1)
        .and_then(|run_completed| run_completed.checked_add(1))
        .and_then(|extraction_terminal| extraction_terminal.checked_add(1))
        .ok_or_else(|| {
        Box::new(DirectedTurnReject::unsupported(format!(
            "cannot reserve directed-turn Pending before runtime acceptance: no terminal event frontier remains after window start {window_start}"
        )))
    })?;
    Ok(window_start)
}

// ---------------------------------------------------------------------------
// Host-actor-owned projection + record channel carriers
// ---------------------------------------------------------------------------

/// Per-session observation facts, published by the host actor as a dogma-#13
/// watch projection of `MobHostBindingAuthority` state (actor = sole writer).
#[derive(Debug, Clone)]
pub struct SessionObservationFacts {
    /// Exact host residency that owns every fact in this projection row.
    pub incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    pub mob_id: String,
    pub agent_identity: String,
    /// Current member generation (`materialized_generations` fact).
    pub generation: u64,
    /// Materialization fence paired with `generation`.
    pub fence_token: u64,
    /// First durable seq belonging to this generation. Resume adoption can
    /// reuse a session log whose earlier rows belong to prior generations.
    pub generation_start_seq: u64,
    /// Durable pre-accept reservations for the exact current residency.
    pub pending_turns: Vec<PendingTurnObservation>,
    /// Retained, unacknowledged turn-outcome journal rows for the CURRENT
    /// generation, in durable commit order.
    pub turn_outcomes: Vec<BridgeTurnOutcomeRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTurnObservation {
    pub input_id: String,
    pub generation: u64,
    pub fence_token: u64,
    pub window_start: u64,
}

/// Watch-published observation projection (session id → facts).
#[derive(Debug, Clone, Default)]
pub struct HostObservationProjection {
    pub sessions: BTreeMap<String, SessionObservationFacts>,
}

/// One journal write request routed onto the host actor task (the actor
/// exclusively owns the generated authority; the watcher never touches it).
pub struct HostTurnOutcomeRecordRequest {
    pub expected_member: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    pub record: TrackedTurnOutcomeRecord,
    pub reply: oneshot::Sender<Result<(), String>>,
}

/// Exact outcome acknowledgements routed onto the actor task. The actor is
/// the sole generated-authority/persistence owner, so observation serving
/// cannot prune its watch projection directly.
pub struct HostTurnOutcomeAckRequest {
    pub expected_member: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    pub acks: Vec<BridgeTurnOutcomeAck>,
    pub reply: oneshot::Sender<Result<(), String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPendingReservationReply {
    Reserved {
        window_start: u64,
    },
    Replayed {
        window_start: u64,
    },
    TerminalReplay,
    /// Replay-only arbitration found no exact Pending/terminal key. The
    /// observation layer must complete fresh preflight before asking the
    /// actor to reserve.
    FreshRequired,
    JournalFull,
    Stale,
}

pub enum HostTurnOutcomePendingRequest {
    Reserve {
        expected_member: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        generation: u64,
        fence_token: u64,
        input_id: String,
        /// `Some` permits a fresh reservation at this window. `None` is an
        /// actor-linearized replay-only probe used before every preflight.
        fresh_window_start: Option<u64>,
        reply: oneshot::Sender<Result<HostPendingReservationReply, String>>,
    },
    Cancel {
        expected_member: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        generation: u64,
        fence_token: u64,
        input_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    CancelTracked {
        expected_member: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        generation: u64,
        fence_token: u64,
        input_id: String,
        runtime_input_present: bool,
        reply: oneshot::Sender<Result<HostTrackedInputCancelReply, String>>,
    },
    CompleteTrackedCancel {
        expected_member: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        generation: u64,
        fence_token: u64,
        input_id: String,
        reply: oneshot::Sender<Result<HostTrackedInputCancelReply, String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostTrackedInputCancelReply {
    NoEffect,
    Cancelling,
    Cancelled,
    Terminal(BridgeTurnOutcomeRecord),
    Stale,
    Unreserved,
}

/// Per-session tracked-turn journal seam implementation: carries the
/// residency facts and forwards records onto the host actor's channel
/// (DEC-P6F-9's `TrackedTurnJournal`, registered via
/// `meerkat_runtime::comms_drain::register_tracked_turn_journal`).
pub struct HostTrackedTurnJournal {
    incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    record_tx: mpsc::Sender<HostTurnOutcomeRecordRequest>,
}

impl HostTrackedTurnJournal {
    pub fn new(
        incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        record_tx: mpsc::Sender<HostTurnOutcomeRecordRequest>,
    ) -> Self {
        Self {
            incarnation,
            record_tx,
        }
    }
}

#[async_trait::async_trait]
impl TrackedTurnJournal for HostTrackedTurnJournal {
    fn member_incarnation(
        &self,
    ) -> &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
        &self.incarnation
    }

    fn member_generation(&self) -> u64 {
        self.incarnation.generation
    }

    fn member_fence_token(&self) -> u64 {
        self.incarnation.fence_token
    }

    async fn record_turn_outcome(
        &self,
        record: TrackedTurnOutcomeRecord,
    ) -> Result<(), MemberObservationError> {
        let encoded_bytes = serde_json::to_vec(&BridgeTurnOutcomeRecord {
            input_id: record.input_id.clone(),
            generation: record.generation,
            fence_token: record.fence_token,
            terminal_seq: record.terminal_seq,
            outcome: record.outcome.clone(),
        })
        .map_err(|error| MemberObservationError::Internal {
            reason: format!("turn outcome size validation failed: {error}"),
        })?
        .len();
        if encoded_bytes > MAX_TURN_OUTCOME_RECORD_BYTES {
            return Err(MemberObservationError::OutcomeRecordTooLarge {
                encoded_bytes,
                max_bytes: MAX_TURN_OUTCOME_RECORD_BYTES,
            });
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        self.record_tx
            .send(HostTurnOutcomeRecordRequest {
                expected_member: self.incarnation.clone(),
                record,
                reply: reply_tx,
            })
            .await
            .map_err(|_| MemberObservationError::Unavailable {
                reason: "host actor journal channel closed".to_string(),
            })?;
        match reply_rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(detail)) => Err(MemberObservationError::Internal { reason: detail }),
            Err(_) => Err(MemberObservationError::Unavailable {
                reason: "host actor dropped the journal record reply".to_string(),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// Terminal-kind mappings (code-only, beside the consumer — DEC-P6F-8)
// ---------------------------------------------------------------------------

/// Project a shared-classifier terminal onto the frozen wire outcome enum.
fn wire_outcome_from_terminal(
    tracking_kind: DirectedTurnTrackingKind,
    kind: meerkat_core::turn_terminal::TurnTerminalKind,
    outcome: &meerkat_core::turn_terminal::TurnTerminalOutcome,
) -> Result<WireFlowTurnOutcome, String> {
    use meerkat_core::turn_terminal::{TurnTerminalKind as K, TurnTerminalOutcome as O};
    let detail = match outcome {
        O::Failed { reason } => reason.clone(),
        O::Completed { .. } => String::new(),
    };
    if tracking_kind == DirectedTurnTrackingKind::PeerInteraction {
        return match kind {
            K::InteractionComplete => Ok(WireFlowTurnOutcome::InteractionComplete),
            K::InteractionCallbackPending => Ok(WireFlowTurnOutcome::InteractionCallbackPending),
            // The core classifier deliberately preserves the ExtractionFailed
            // kind for an interaction-scoped extraction failure. Placed plain
            // Peer custody nevertheless promises the Interaction* terminal
            // family, so retain its exact reason under InteractionFailed.
            K::InteractionFailed | K::ExtractionFailed => {
                Ok(WireFlowTurnOutcome::InteractionFailed {
                    detail: WireFlowFailureDetail::complete(detail),
                })
            }
            other => Err(format!(
                "tracked peer interaction carried non-interaction terminal {other}"
            )),
        };
    }
    Ok(match kind {
        K::RunCompleted => WireFlowTurnOutcome::RunCompleted,
        K::ExtractionSucceeded => WireFlowTurnOutcome::ExtractionSucceeded,
        K::ExtractionFailed => WireFlowTurnOutcome::ExtractionFailed {
            detail: WireFlowFailureDetail::complete(detail),
        },
        K::RunFailed => WireFlowTurnOutcome::RunFailed {
            detail: WireFlowFailureDetail::complete(detail),
        },
        K::InteractionComplete => WireFlowTurnOutcome::InteractionComplete,
        K::InteractionCallbackPending => WireFlowTurnOutcome::InteractionCallbackPending,
        K::InteractionFailed => WireFlowTurnOutcome::InteractionFailed {
            detail: WireFlowFailureDetail::complete(detail),
        },
        K::ChannelClosed => WireFlowTurnOutcome::ChannelClosed,
    })
}

#[derive(Debug, Clone, Copy)]
enum FailedWireTerminal {
    Extraction,
    Run,
    Interaction,
}

impl FailedWireTerminal {
    fn outcome(self, detail: WireFlowFailureDetail) -> WireFlowTurnOutcome {
        match self {
            Self::Extraction => WireFlowTurnOutcome::ExtractionFailed { detail },
            Self::Run => WireFlowTurnOutcome::RunFailed { detail },
            Self::Interaction => WireFlowTurnOutcome::InteractionFailed { detail },
        }
    }
}

fn tracked_turn_record_wire_len(record: &TrackedTurnOutcomeRecord) -> Option<usize> {
    serde_json::to_vec(&BridgeTurnOutcomeRecord {
        input_id: record.input_id.clone(),
        generation: record.generation,
        fence_token: record.fence_token,
        terminal_seq: record.terminal_seq,
        outcome: record.outcome.clone(),
    })
    .ok()
    .map(|encoded| encoded.len())
}

/// JSON byte length of a string's content, excluding its surrounding quotes.
/// This mirrors serde_json's compact formatter: named two-byte escapes for
/// quote, backslash, and the five short controls; six-byte `\\u00xx` escapes
/// for the remaining C0 controls; UTF-8 verbatim otherwise.
fn json_char_content_len(ch: char) -> usize {
    match ch {
        '"' | '\\' | '\u{08}' | '\u{09}' | '\u{0a}' | '\u{0c}' | '\u{0d}' => 2,
        '\u{00}'..='\u{1f}' => 6,
        _ => ch.len_utf8(),
    }
}

fn json_string_content_len(text: &str) -> usize {
    text.chars().fold(0usize, |total, ch| {
        total.saturating_add(json_char_content_len(ch))
    })
}

#[allow(clippy::too_many_arguments)] // exact bounded failure-sidecar carrier fields
fn failed_record(
    input_id: &str,
    generation: u64,
    fence_token: u64,
    terminal_seq: u64,
    terminal: FailedWireTerminal,
    text: String,
    original_utf8_bytes: u64,
    truncated: bool,
) -> TrackedTurnOutcomeRecord {
    TrackedTurnOutcomeRecord {
        input_id: input_id.to_string(),
        generation,
        fence_token,
        terminal_seq,
        outcome: terminal.outcome(WireFlowFailureDetail {
            text,
            original_utf8_bytes,
            truncated,
        }),
    }
}

/// Compact only a failed terminal's presentation detail, before the journal
/// seam. The terminal discriminant and exact record/ack identity stay fixed.
///
/// `open_directed_turn_window` preflights the largest empty failure framing,
/// so an empty UTF-8 prefix is always representable here after the effect.
fn compact_tracked_turn_outcome_record(
    record: TrackedTurnOutcomeRecord,
) -> TrackedTurnOutcomeRecord {
    let TrackedTurnOutcomeRecord {
        input_id,
        generation,
        fence_token,
        terminal_seq,
        outcome,
    } = record;
    let (terminal, detail) = match outcome {
        WireFlowTurnOutcome::ExtractionFailed { detail } => {
            (FailedWireTerminal::Extraction, detail)
        }
        WireFlowTurnOutcome::RunFailed { detail } => (FailedWireTerminal::Run, detail),
        WireFlowTurnOutcome::InteractionFailed { detail } => {
            (FailedWireTerminal::Interaction, detail)
        }
        outcome => {
            return TrackedTurnOutcomeRecord {
                input_id,
                generation,
                fence_token,
                terminal_seq,
                outcome,
            };
        }
    };

    // The member host is the first compaction authority. Recompute the
    // metadata from source text instead of trusting an upstream annotation.
    let original_text = detail.text;
    let original_utf8_bytes = u64::try_from(original_text.len()).unwrap_or(u64::MAX);
    // Measure the complete form as bounded empty framing + exact JSON string
    // content length. Never clone or serialize the untrusted full detail:
    // provider errors may be much larger than the durable protocol budget.
    let complete_framing = failed_record(
        &input_id,
        generation,
        fence_token,
        terminal_seq,
        terminal,
        String::new(),
        original_utf8_bytes,
        false,
    );
    let complete_framing_bytes =
        tracked_turn_record_wire_len(&complete_framing).unwrap_or(usize::MAX);
    let complete_content_bytes = json_string_content_len(&original_text);
    if complete_framing_bytes
        .checked_add(complete_content_bytes)
        .is_some_and(|len| len <= MAX_TURN_OUTCOME_RECORD_BYTES)
    {
        return failed_record(
            &input_id,
            generation,
            fence_token,
            terminal_seq,
            terminal,
            original_text,
            original_utf8_bytes,
            false,
        );
    }

    let empty = failed_record(
        &input_id,
        generation,
        fence_token,
        terminal_seq,
        terminal,
        String::new(),
        original_utf8_bytes,
        true,
    );
    let framing_bytes = tracked_turn_record_wire_len(&empty).unwrap_or(usize::MAX);
    let content_budget = MAX_TURN_OUTCOME_RECORD_BYTES.saturating_sub(framing_bytes);
    let mut retained_content_bytes = 0usize;
    let mut retained_utf8_bytes = 0usize;
    for (start, ch) in original_text.char_indices() {
        let encoded_ch_bytes = json_char_content_len(ch);
        let Some(next_content_bytes) = retained_content_bytes.checked_add(encoded_ch_bytes) else {
            break;
        };
        if next_content_bytes > content_budget {
            break;
        }
        retained_content_bytes = next_content_bytes;
        retained_utf8_bytes = start + ch.len_utf8();
    }

    failed_record(
        &input_id,
        generation,
        fence_token,
        terminal_seq,
        terminal,
        original_text[..retained_utf8_bytes].to_string(),
        original_utf8_bytes,
        true,
    )
}

/// Prove before runtime acceptance that even the widest failure framing can
/// be journaled with an empty detail prefix. This protects the post-effect
/// path from an uncompactable attacker-controlled input id.
fn directed_turn_record_framing_fits(input_id: &str, generation: u64, fence_token: u64) -> bool {
    let widest_empty_id_framing = [
        FailedWireTerminal::Extraction,
        FailedWireTerminal::Run,
        FailedWireTerminal::Interaction,
    ]
    .into_iter()
    .filter_map(|terminal| {
        let record = failed_record(
            "",
            generation,
            fence_token,
            u64::MAX,
            terminal,
            String::new(),
            u64::MAX,
            true,
        );
        tracked_turn_record_wire_len(&record)
    })
    .max()
    .unwrap_or(usize::MAX);
    widest_empty_id_framing
        .checked_add(json_string_content_len(input_id))
        .is_some_and(|len| len <= MAX_TURN_OUTCOME_RECORD_BYTES)
}

// ---------------------------------------------------------------------------
// Ephemeral ring (DEC-P6E-5)
// ---------------------------------------------------------------------------

struct RingState {
    generation: u64,
    capacity: usize,
    /// Next host-owned seq to assign (domain starts at 1 per
    /// `(session, generation)`).
    next_seq: u64,
    /// Oldest retained seq (advances on eviction → `StaleCursor`).
    oldest_retained: u64,
    /// The feeder observed an event for which no successor cursor could be
    /// represented. Once set, serving fails explicitly instead of dropping,
    /// wrapping, or replaying the boundary row.
    exhausted: bool,
    rows: VecDeque<(u64, EventEnvelope<AgentEvent>)>,
}

impl RingState {
    fn push(&mut self, envelope: EventEnvelope<AgentEvent>) -> Result<(), MemberObservationError> {
        let seq = self.next_seq;
        let next_seq = match checked_event_sequence_successor(seq, "ephemeral member event ring") {
            Ok(next_seq) => next_seq,
            Err(error) => {
                self.exhausted = true;
                return Err(error);
            }
        };
        self.next_seq = next_seq;
        self.rows.push_back((seq, envelope));
        while self.rows.len() > self.capacity {
            if let Some((evicted_seq, _)) = self.rows.pop_front() {
                self.oldest_retained = match checked_event_sequence_successor(
                    evicted_seq,
                    "ephemeral member event ring eviction",
                ) {
                    Ok(oldest_retained) => oldest_retained,
                    Err(error) => {
                        self.exhausted = true;
                        return Err(error);
                    }
                };
            }
        }
        Ok(())
    }
}

struct SessionEventRing {
    state: std::sync::Mutex<RingState>,
    notify: Arc<Notify>,
    feeder: tokio::task::JoinHandle<()>,
}

impl Drop for SessionEventRing {
    fn drop(&mut self) {
        self.feeder.abort();
    }
}

impl SessionEventRing {
    fn snapshot_from(
        &self,
        from_seq: u64,
        max: usize,
    ) -> Result<(Vec<(u64, EventEnvelope<AgentEvent>)>, u64, u64), MemberObservationError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.exhausted {
            return Err(MemberObservationError::Internal {
                reason: format!("ephemeral member event ring: {EVENT_SEQUENCE_EXHAUSTED}"),
            });
        }
        let watermark = state.next_seq.saturating_sub(1);
        if from_seq < state.oldest_retained {
            return Err(MemberObservationError::StaleCursor {
                watermark,
                generation: state.generation,
            });
        }
        if from_seq
            > checked_event_sequence_successor(watermark, "ephemeral member event ring cursor")?
        {
            // A cursor ahead of this ring incarnation (daemon restart within
            // a generation): the host-owned seq domain restarted, so the
            // consumer must restart from the current watermark and label the
            // gap — the declared ephemeral degradation.
            return Err(MemberObservationError::StaleCursor {
                watermark,
                generation: state.generation,
            });
        }
        let rows = state
            .rows
            .iter()
            .filter(|(seq, _)| *seq >= from_seq)
            .take(max)
            .cloned()
            .collect();
        Ok((rows, watermark, state.generation))
    }

    fn reset_for_generation(&self, generation: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.generation != generation {
            state.generation = generation;
            state.next_seq = 1;
            state.oldest_retained = 1;
            state.exhausted = false;
            state.rows.clear();
        }
    }
}

// ---------------------------------------------------------------------------
// HostMemberObservation
// ---------------------------------------------------------------------------

/// The daemon-composed observation substrate (DEC-P6E-2).
pub struct HostMemberObservation {
    session_service: Arc<dyn MobSessionService>,
    durable_log: Option<Arc<dyn DurableEventLogRead>>,
    projection: watch::Receiver<HostObservationProjection>,
    pending_tx: mpsc::Sender<HostTurnOutcomePendingRequest>,
    outcome_ack_tx: mpsc::Sender<HostTurnOutcomeAckRequest>,
    event_ring_capacity: usize,
    rings: tokio::sync::Mutex<HashMap<SessionId, Arc<SessionEventRing>>>,
    pending_recovery_started: AtomicBool,
    shutting_down: AtomicBool,
    directed_turn_tasks: tokio::sync::Mutex<tokio::task::JoinSet<()>>,
    active_turn_watchers: Arc<StdMutex<HashSet<ActiveTurnWatcherKey>>>,
    /// Exact-key lock shared by the final delivery-admission interval and the
    /// controlling cancel command. Weak values keep the table bounded once no
    /// operation retains a key.
    admission_locks:
        StdMutex<HashMap<DirectedTurnAdmissionKey, std::sync::Weak<tokio::sync::Mutex<()>>>>,
}

/// Explicit composition-owned lifetime for the observation substrate's
/// background recovery and directed-turn watcher tasks.
pub struct HostMemberObservationTaskOwner {
    observation: Arc<HostMemberObservation>,
    pending_recovery: Option<tokio::task::JoinHandle<()>>,
}

impl HostMemberObservationTaskOwner {
    pub async fn shutdown(mut self) {
        self.observation
            .shutting_down
            .store(true, Ordering::Release);
        if let Some(task) = self.pending_recovery.take() {
            task.abort();
            let _ = task.await;
        }
        self.observation.shutdown_owned_tasks().await;
    }
}

impl Drop for HostMemberObservationTaskOwner {
    fn drop(&mut self) {
        self.observation
            .shutting_down
            .store(true, Ordering::Release);
        if let Some(task) = &self.pending_recovery {
            task.abort();
        }
        if let Ok(mut tasks) = self.observation.directed_turn_tasks.try_lock() {
            tasks.abort_all();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ActiveTurnWatcherKey {
    session_id: SessionId,
    generation: u64,
    fence_token: u64,
    input_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DirectedTurnAdmissionKey {
    session_id: SessionId,
    mob_id: String,
    agent_identity: String,
    host_id: String,
    binding_generation: u64,
    generation: u64,
    fence_token: u64,
    input_id: String,
}

impl DirectedTurnAdmissionKey {
    fn new(
        session_id: &SessionId,
        expected: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        input_id: &str,
    ) -> Self {
        Self {
            session_id: session_id.clone(),
            mob_id: expected.mob_id.clone(),
            agent_identity: expected.agent_identity.clone(),
            host_id: expected.host_id.clone(),
            binding_generation: expected.binding_generation,
            generation: expected.generation,
            fence_token: expected.fence_token,
            input_id: input_id.to_string(),
        }
    }
}

struct ActiveTurnWatcherClaim {
    key: ActiveTurnWatcherKey,
    active: Arc<StdMutex<HashSet<ActiveTurnWatcherKey>>>,
}

impl ActiveTurnWatcherClaim {
    fn try_acquire(
        active: &Arc<StdMutex<HashSet<ActiveTurnWatcherKey>>>,
        key: ActiveTurnWatcherKey,
    ) -> Option<Self> {
        let inserted = active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.clone());
        inserted.then(|| Self {
            key,
            active: Arc::clone(active),
        })
    }
}

impl Drop for ActiveTurnWatcherClaim {
    fn drop(&mut self) {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.key);
    }
}

async fn run_pending_recovery_reconciler<F, Fut>(
    mut projection: watch::Receiver<HostObservationProjection>,
    mut recover_once: F,
) where
    F: FnMut() -> Fut,
    Fut: Future<Output = usize>,
{
    let mut backoff = PENDING_RECOVERY_RETRY_MIN;
    loop {
        let pending_count = recover_once().await;
        if pending_count == 0 {
            if projection.changed().await.is_err() {
                return;
            }
            backoff = PENDING_RECOVERY_RETRY_MIN;
            continue;
        }

        tokio::select! {
            changed = projection.changed() => {
                if changed.is_err() {
                    return;
                }
                backoff = PENDING_RECOVERY_RETRY_MIN;
            }
            () = tokio::time::sleep(backoff) => {
                backoff = backoff
                    .saturating_mul(2)
                    .min(PENDING_RECOVERY_RETRY_MAX);
            }
        }
    }
}

fn validate_outcome_ack_batch(
    outcome_acks: &[BridgeTurnOutcomeAck],
) -> Result<Vec<BridgeTurnOutcomeAck>, MemberObservationError> {
    if outcome_acks.len() > BRIDGE_TURN_OUTCOME_ACK_MAX {
        return Err(MemberObservationError::Internal {
            reason: format!(
                "turn-outcome acknowledgement batch has {} rows (maximum {})",
                outcome_acks.len(),
                BRIDGE_TURN_OUTCOME_ACK_MAX
            ),
        });
    }
    Ok(outcome_acks.to_vec())
}

impl HostMemberObservation {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        durable_log: Option<Arc<dyn DurableEventLogRead>>,
        projection: watch::Receiver<HostObservationProjection>,
        pending_tx: mpsc::Sender<HostTurnOutcomePendingRequest>,
        outcome_ack_tx: mpsc::Sender<HostTurnOutcomeAckRequest>,
    ) -> Self {
        Self {
            session_service,
            durable_log,
            projection,
            pending_tx,
            outcome_ack_tx,
            event_ring_capacity: MEMBER_EVENT_RING_CAPACITY,
            rings: tokio::sync::Mutex::new(HashMap::new()),
            pending_recovery_started: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            directed_turn_tasks: tokio::sync::Mutex::new(tokio::task::JoinSet::new()),
            active_turn_watchers: Arc::new(StdMutex::new(HashSet::new())),
            admission_locks: StdMutex::new(HashMap::new()),
        }
    }

    /// Override the per-session ephemeral event window advertised by the
    /// daemon's `live_buffer_events` configuration. A non-zero type keeps an
    /// accidentally inert ring unrepresentable for embedders as well as the
    /// CLI composition.
    #[must_use]
    pub fn with_event_ring_capacity(mut self, capacity: NonZeroUsize) -> Self {
        self.event_ring_capacity = capacity.get();
        self
    }

    async fn lock_directed_turn_key(
        &self,
        session: &SessionId,
        expected: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        input_id: &str,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        let key = DirectedTurnAdmissionKey::new(session, expected, input_id);
        let lock = {
            let mut locks = self
                .admission_locks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            locks.retain(|_, lock| lock.strong_count() > 0);
            match locks.get(&key).and_then(std::sync::Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(tokio::sync::Mutex::new(()));
                    locks.insert(key, Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.lock_owned().await
    }

    /// Start the durable Pending reconciler and return the observation task
    /// owner to the daemon composition. Transient runtime/subscription/journal
    /// failures are retried while Pending remains projected, without requiring
    /// another host restart or controller redelivery. The daemon must call
    /// [`HostMemberObservationTaskOwner::shutdown`] in reverse composition
    /// order.
    #[cfg(feature = "runtime-adapter")]
    #[must_use]
    pub async fn recover_pending_turns(self: &Arc<Self>) -> Option<HostMemberObservationTaskOwner> {
        if self.pending_recovery_started.swap(true, Ordering::AcqRel) {
            return None;
        }
        let observation = Arc::clone(self);
        let pending_recovery = tokio::spawn(async move {
            observation.pending_recovery_loop().await;
        });
        Some(HostMemberObservationTaskOwner {
            observation: Arc::clone(self),
            pending_recovery: Some(pending_recovery),
        })
    }

    async fn shutdown_owned_tasks(&self) {
        let mut tasks = self.directed_turn_tasks.lock().await;
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        drop(tasks);
        // Dropping rings aborts their feeder handles through SessionEventRing's
        // RAII backstop after no directed watcher can acquire another ring.
        self.rings.lock().await.clear();
    }

    #[cfg(feature = "runtime-adapter")]
    async fn pending_recovery_loop(self: Arc<Self>) {
        let projection = self.projection.clone();
        run_pending_recovery_reconciler(projection, move || {
            let observation = Arc::clone(&self);
            async move { observation.recover_pending_turns_once().await }
        })
        .await;
    }

    /// Reattach watchers for one snapshot of durable Pending rows. The
    /// runtime's restart-safe idempotency ledger distinguishes pre-accept
    /// Pending (`None`, retain) from accepted work (`Some`). Nonterminal work
    /// is re-submitted only to obtain a Deduplicated completion handle;
    /// terminal work is reconstructed from its original durable window.
    #[cfg(feature = "runtime-adapter")]
    async fn recover_pending_turns_once(&self) -> usize {
        use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;

        let Some(adapter) = self.session_service.runtime_adapter() else {
            return 0;
        };
        let projection = self.projection.borrow().clone();
        let mut pending_count = 0usize;
        for (session_text, facts) in projection.sessions {
            let Ok(session) = SessionId::parse(&session_text) else {
                tracing::error!(
                    session_id = %session_text,
                    "cannot recover Pending directed turns for an invalid session id"
                );
                continue;
            };
            for pending in facts.pending_turns {
                pending_count = pending_count.saturating_add(1);
                if pending.generation != facts.generation
                    || pending.fence_token != facts.fence_token
                    || facts.incarnation.generation != facts.generation
                    || facts.incarnation.fence_token != facts.fence_token
                    || facts.incarnation.member_session_id != session_text
                {
                    tracing::error!(
                        session_id = %session,
                        input_id = %pending.input_id,
                        pending_generation = pending.generation,
                        pending_fence_token = pending.fence_token,
                        projected_incarnation = ?facts.incarnation,
                        "Pending recovery projection key differs from its exact current residency; retaining Pending"
                    );
                    continue;
                }
                let watcher_key = ActiveTurnWatcherKey {
                    session_id: session.clone(),
                    generation: pending.generation,
                    fence_token: pending.fence_token,
                    input_id: pending.input_id.clone(),
                };
                if self
                    .active_turn_watchers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .contains(&watcher_key)
                {
                    continue;
                }
                let stored = match adapter
                    .input_state_by_idempotency_key(&session, &pending.input_id)
                    .await
                {
                    Ok(stored) => stored,
                    Err(error) => {
                        tracing::error!(
                            session_id = %session,
                            input_id = %pending.input_id,
                            error = %error,
                            "durable Pending recovery could not query runtime acceptance; retaining Pending"
                        );
                        continue;
                    }
                };
                let Some(stored) = stored else {
                    // Crash after Pending persist but before accept. Pending is
                    // not acceptance evidence; preserve it for an exact retry.
                    tracing::debug!(
                        session_id = %session,
                        input_id = %pending.input_id,
                        "Pending has no durable runtime acceptance; retaining for controller retry"
                    );
                    continue;
                };
                if let Err(reason) =
                    DirectedTurnRuntimeAttribution::from_stored(&pending.input_id, &stored)
                {
                    tracing::error!(
                        session_id = %session,
                        input_id = %pending.input_id,
                        reason = %reason,
                        "durable Pending recovery found a mismatched runtime input binding; retaining Pending"
                    );
                    continue;
                }
                let Some(journal) = adapter.tracked_turn_journal(&session) else {
                    tracing::error!(
                        session_id = %session,
                        input_id = %pending.input_id,
                        "accepted Pending recovery found no tracked-turn journal; retaining Pending"
                    );
                    continue;
                };
                if journal.member_incarnation() != &facts.incarnation {
                    tracing::error!(
                        session_id = %session,
                        input_id = %pending.input_id,
                        expected = ?facts.incarnation,
                        journal = ?journal.member_incarnation(),
                        "accepted Pending recovery found a journal for a different residency; retaining Pending"
                    );
                    continue;
                }
                let subscription = self
                    .directed_turn_wake_stream(&session, &pending.input_id)
                    .await;

                let completion_handle = if stored.seed.terminal_outcome.is_some() {
                    None
                } else {
                    let Some(persisted_input) = stored.state.persisted_input.clone() else {
                        tracing::error!(
                            session_id = %session,
                            input_id = %pending.input_id,
                            "accepted nonterminal Pending lacks persisted input; retaining Pending"
                        );
                        continue;
                    };
                    match adapter
                        .accept_input_with_completion_for_member_residency(
                            &session,
                            persisted_input,
                            Some(&facts.incarnation),
                        )
                        .await
                    {
                        Ok((
                            meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. },
                            handle,
                        )) if existing_id == stored.state.input_id => handle,
                        Ok((
                            meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. },
                            _,
                        )) => {
                            tracing::error!(
                                session_id = %session,
                                input_id = %pending.input_id,
                                expected_input_id = %stored.state.input_id,
                                actual_input_id = %existing_id,
                                "Pending recovery deduplicated onto a different runtime input; retaining Pending"
                            );
                            continue;
                        }
                        Ok((meerkat_runtime::AcceptOutcome::Accepted { input_id, .. }, _)) => {
                            tracing::error!(
                                session_id = %session,
                                input_id = %pending.input_id,
                                accepted_input_id = %input_id,
                                "durable idempotency query found an input but recovery re-accepted it; retaining Pending"
                            );
                            continue;
                        }
                        Ok((meerkat_runtime::AcceptOutcome::Rejected { reason }, _)) => {
                            tracing::error!(
                                session_id = %session,
                                input_id = %pending.input_id,
                                reason = %reason,
                                "Pending recovery runtime rejected its persisted input; retaining Pending"
                            );
                            continue;
                        }
                        Ok((other, _)) => {
                            tracing::error!(
                                session_id = %session,
                                input_id = %pending.input_id,
                                outcome = ?other,
                                "Pending recovery received an unknown runtime admission outcome; retaining Pending"
                            );
                            continue;
                        }
                        Err(error) => {
                            tracing::error!(
                                session_id = %session,
                                input_id = %pending.input_id,
                                error = %error,
                                "Pending recovery could not reattach completion; retaining Pending"
                            );
                            continue;
                        }
                    }
                };

                let admission = DirectedTurnAdmission {
                    input_id: pending.input_id.clone(),
                    window: DirectedTurnWindow {
                        expected_member: facts.incarnation.clone(),
                        subscription,
                        window_start: pending.window_start,
                        generation: pending.generation,
                        fence_token: pending.fence_token,
                        input_id: pending.input_id,
                        tracking: DirectedTurnTracking::PendingReplay,
                    },
                    completion_handle,
                    journal,
                };
                if let Err(error) = self.admit_directed_turn(&session, admission).await {
                    tracing::error!(
                        session_id = %session,
                        error = %error,
                        "accepted Pending recovery could not attach watcher; retaining Pending"
                    );
                }
            }
        }
        pending_count
    }

    #[cfg(not(feature = "runtime-adapter"))]
    #[must_use]
    pub async fn recover_pending_turns(self: &Arc<Self>) -> Option<HostMemberObservationTaskOwner> {
        if self.pending_recovery_started.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some(HostMemberObservationTaskOwner {
            observation: Arc::clone(self),
            pending_recovery: None,
        })
    }

    async fn reserve_pending(
        &self,
        facts: &SessionObservationFacts,
        input_id: &str,
        fresh_window_start: Option<u64>,
    ) -> Result<HostPendingReservationReply, DirectedTurnReject> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.pending_tx
            .send(HostTurnOutcomePendingRequest::Reserve {
                expected_member: facts.incarnation.clone(),
                generation: facts.generation,
                fence_token: facts.fence_token,
                input_id: input_id.to_string(),
                fresh_window_start,
                reply: reply_tx,
            })
            .await
            .map_err(|_| {
                DirectedTurnReject::ambiguous(
                    "host actor Pending reservation channel closed".to_string(),
                )
            })?;
        reply_rx
            .await
            .map_err(|_| {
                DirectedTurnReject::ambiguous(
                    "host actor dropped the Pending reservation reply".to_string(),
                )
            })?
            .map_err(DirectedTurnReject::ambiguous)
    }

    async fn directed_turn_wake_stream(
        &self,
        session: &SessionId,
        input_id: &str,
    ) -> meerkat_core::EventStream {
        match MobSessionService::subscribe_session_events(self.session_service.as_ref(), session)
            .await
        {
            Ok(subscription) => subscription,
            Err(error) => {
                // Durable scanning plus the bounded fallback tick is the
                // terminal payload authority. A wake-stream failure can
                // delay it, but must not turn an existing Pending retry into
                // a false definite-no-effect rejection.
                tracing::warn!(
                    session_id = %session,
                    input_id,
                    error = %error,
                    "directed turn has no live event wake stream; using durable polling"
                );
                Box::pin(futures::stream::empty())
            }
        }
    }

    async fn cancel_pending(
        &self,
        expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        generation: u64,
        fence_token: u64,
        input_id: String,
        tracking: DirectedTurnTracking,
    ) -> Result<(), DirectedTurnReject> {
        match tracking {
            DirectedTurnTracking::TerminalReplay => return Ok(()),
            DirectedTurnTracking::PendingReplay => {
                return Err(DirectedTurnReject::unsupported(
                    "cannot cancel replayed Pending: this retry does not prove the original delivery had no effect",
                ));
            }
            DirectedTurnTracking::PendingFresh => {}
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        self.pending_tx
            .send(HostTurnOutcomePendingRequest::Cancel {
                expected_member: expected_member.clone(),
                generation,
                fence_token,
                input_id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| {
                DirectedTurnReject::unsupported(
                    "host actor Pending cancel channel closed".to_string(),
                )
            })?;
        reply_rx
            .await
            .map_err(|_| {
                DirectedTurnReject::unsupported(
                    "host actor dropped the Pending cancel reply".to_string(),
                )
            })?
            .map_err(DirectedTurnReject::unsupported)
    }

    async fn actor_cancel_tracked_input(
        &self,
        expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        input_id: &str,
        runtime_input_present: bool,
    ) -> Result<HostTrackedInputCancelReply, MemberObservationError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.pending_tx
            .send(HostTurnOutcomePendingRequest::CancelTracked {
                expected_member: expected_member.clone(),
                generation: expected_member.generation,
                fence_token: expected_member.fence_token,
                input_id: input_id.to_string(),
                runtime_input_present,
                reply: reply_tx,
            })
            .await
            .map_err(|_| MemberObservationError::Unavailable {
                reason: "host actor tracked-input cancel channel closed".to_string(),
            })?;
        match reply_rx.await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(reason)) => Err(MemberObservationError::Internal { reason }),
            Err(_) => Err(MemberObservationError::Unavailable {
                reason: "host actor dropped the tracked-input cancel reply".to_string(),
            }),
        }
    }

    async fn actor_complete_tracked_input_cancel(
        &self,
        expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        input_id: &str,
    ) -> Result<HostTrackedInputCancelReply, MemberObservationError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.pending_tx
            .send(HostTurnOutcomePendingRequest::CompleteTrackedCancel {
                expected_member: expected_member.clone(),
                generation: expected_member.generation,
                fence_token: expected_member.fence_token,
                input_id: input_id.to_string(),
                reply: reply_tx,
            })
            .await
            .map_err(|_| MemberObservationError::Unavailable {
                reason: "host actor tracked-input cancel completion channel closed".to_string(),
            })?;
        match reply_rx.await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(reason)) => Err(MemberObservationError::Internal { reason }),
            Err(_) => Err(MemberObservationError::Unavailable {
                reason: "host actor dropped the tracked-input cancel completion reply".to_string(),
            }),
        }
    }

    /// A runtime terminal wins an exact-key cancellation race, but the wire
    /// terminal remains owned by the host journal. Wait until the watcher has
    /// moved this key out of Pending (or an already-recorded cancel/ACK has
    /// done so) before asking the actor to classify cancellation. Mutating
    /// Pending to `Cancelling` while the runtime is already naturally
    /// terminal would discard the real outcome.
    async fn await_runtime_terminal_host_custody(
        &self,
        session: &SessionId,
        expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        input_id: &str,
    ) -> Result<(), MemberObservationError> {
        let mut projection = self.projection.clone();
        let deadline = tokio::time::Instant::now() + POST_COMPLETION_TERMINAL_BIND_TIMEOUT;
        loop {
            let pending = {
                let snapshot = projection.borrow();
                let facts = snapshot.sessions.get(&session.to_string()).ok_or_else(|| {
                    MemberObservationError::StaleIncarnation {
                        reason: format!(
                            "tracked-input runtime terminal lost resident session '{session}' before host custody"
                        ),
                    }
                })?;
                if &facts.incarnation != expected_member {
                    return Err(MemberObservationError::StaleIncarnation {
                        reason: format!(
                            "tracked-input runtime terminal expected {expected_member:?}; host projection is {:?}",
                            facts.incarnation
                        ),
                    });
                }
                facts.pending_turns.iter().any(|pending| {
                    pending.input_id == input_id
                        && pending.generation == expected_member.generation
                        && pending.fence_token == expected_member.fence_token
                })
            };
            if !pending {
                return Ok(());
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Err(MemberObservationError::Unavailable {
                    reason: format!(
                        "runtime input '{input_id}' is terminal but host outcome custody did not converge"
                    ),
                });
            }
            match tokio::time::timeout(deadline - now, projection.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => {
                    return Err(MemberObservationError::Unavailable {
                        reason:
                            "host observation projection closed while waiting for terminal custody"
                                .to_string(),
                    });
                }
                Err(_) => {
                    return Err(MemberObservationError::Unavailable {
                        reason: format!(
                            "runtime input '{input_id}' is terminal but host outcome custody did not converge"
                        ),
                    });
                }
            }
        }
    }

    async fn acknowledge_outcomes(
        &self,
        facts: &SessionObservationFacts,
        outcome_acks: &[BridgeTurnOutcomeAck],
    ) -> Result<(), MemberObservationError> {
        let acks = validate_outcome_ack_batch(outcome_acks)?;
        if acks.is_empty() {
            return Ok(());
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        self.outcome_ack_tx
            .send(HostTurnOutcomeAckRequest {
                expected_member: facts.incarnation.clone(),
                acks,
                reply: reply_tx,
            })
            .await
            .map_err(|_| MemberObservationError::Unavailable {
                reason: "host actor outcome acknowledgement channel closed".to_string(),
            })?;
        match reply_rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(reason)) => Err(MemberObservationError::Internal { reason }),
            Err(_) => Err(MemberObservationError::Unavailable {
                reason: "host actor dropped the outcome acknowledgement reply".to_string(),
            }),
        }
    }

    fn outcome_page(
        facts: &SessionObservationFacts,
        max_outcomes: u32,
        event_frontier_exclusive: u64,
    ) -> (Vec<BridgeTurnOutcomeRecord>, bool) {
        let limit = usize::try_from(max_outcomes)
            .unwrap_or(TURN_OUTCOME_PAGE_MAX)
            .clamp(1, TURN_OUTCOME_PAGE_MAX);
        let eligible = facts
            .turn_outcomes
            .iter()
            .filter(|record| record.terminal_seq < event_frontier_exclusive);
        let page: Vec<_> = eligible.clone().take(limit).cloned().collect();
        let complete = facts.turn_outcomes.len() == page.len() && eligible.count() == page.len();
        (page, complete)
    }

    fn session_facts(
        &self,
        session: &SessionId,
    ) -> Result<SessionObservationFacts, MemberObservationError> {
        let facts = self
            .projection
            .borrow()
            .sessions
            .get(&session.to_string())
            .cloned()
            .ok_or_else(|| MemberObservationError::Unavailable {
                reason: format!("session '{session}' is not a resident materialized member"),
            })?;
        if facts.incarnation.member_session_id != session.to_string()
            || facts.incarnation.generation != facts.generation
            || facts.incarnation.fence_token != facts.fence_token
        {
            return Err(MemberObservationError::Internal {
                reason: format!(
                    "session '{session}' observation facts disagree with their incarnation: {:?}",
                    facts.incarnation
                ),
            });
        }
        Ok(facts)
    }

    /// Resolve the durable substrate, or `None` for ring-backed realms.
    async fn durable_watermark(
        &self,
        session: &SessionId,
    ) -> Result<Option<u64>, MemberObservationError> {
        let Some(log) = self.durable_log.as_ref() else {
            return Ok(None);
        };
        log.latest_seq(session).await
    }

    async fn ring_for(
        &self,
        session: &SessionId,
        generation: u64,
    ) -> Result<Arc<SessionEventRing>, MemberObservationError> {
        let mut rings = self.rings.lock().await;
        if let Some(ring) = rings.get(session) {
            ring.reset_for_generation(generation);
            return Ok(Arc::clone(ring));
        }
        let stream =
            MobSessionService::subscribe_session_events(self.session_service.as_ref(), session)
                .await
                .map_err(|error| MemberObservationError::Unavailable {
                    reason: format!("session event subscription failed for ring backing: {error}"),
                })?;
        let notify = Arc::new(Notify::new());
        let state = std::sync::Mutex::new(RingState {
            generation,
            capacity: self.event_ring_capacity,
            next_seq: 1,
            oldest_retained: 1,
            exhausted: false,
            // The configured value is an upper bound, not an instruction to
            // reserve that much memory for every resident session up front.
            rows: VecDeque::new(),
        });
        let ring = Arc::new_cyclic(|weak: &std::sync::Weak<SessionEventRing>| {
            let weak = weak.clone();
            let notify_task = Arc::clone(&notify);
            let feeder = tokio::spawn(async move {
                let mut stream = stream;
                while let Some(envelope) = stream.next().await {
                    let Some(ring) = weak.upgrade() else { break };
                    {
                        let mut state = ring
                            .state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if let Err(error) = state.push(envelope) {
                            tracing::error!(
                                generation = state.generation,
                                error = %error,
                                "ephemeral member event ring exhausted; stopping feeder"
                            );
                            notify_task.notify_waiters();
                            break;
                        }
                    }
                    notify_task.notify_waiters();
                }
            });
            SessionEventRing {
                state,
                notify,
                feeder,
            }
        });
        rings.insert(session.clone(), Arc::clone(&ring));
        Ok(ring)
    }

    /// Resolve the effective read floor for one poll (DEC-P6E-4 cursor
    /// matrix). `current_generation` = G, `watermark` = W.
    fn resolve_cursor_floor(
        cursor: MemberObservationCursor,
        current_generation: u64,
        generation_start_seq: u64,
        watermark: u64,
    ) -> Result<u64, MemberObservationError> {
        match cursor {
            MemberObservationCursor::Tail => Ok(checked_event_sequence_successor(
                watermark,
                "member event Tail cursor",
            )?
            .max(generation_start_seq)),
            MemberObservationCursor::At { generation, seq } => {
                match generation.cmp(&current_generation) {
                    std::cmp::Ordering::Equal => Ok(seq.max(generation_start_seq)),
                    // Respawn happened: serve the CURRENT generation from its
                    // domain start; the page's generation is the
                    // self-describing reset signal (FLAG-P6E-6) — no reject,
                    // no second round trip.
                    std::cmp::Ordering::Less => Ok(generation_start_seq),
                    std::cmp::Ordering::Greater => {
                        Err(MemberObservationError::FutureGenerationCursor {
                            requested: generation,
                            current: current_generation,
                        })
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)] // one serving call's facts, not a reusable carrier
    async fn poll_events_durable(
        &self,
        session: &SessionId,
        log: &Arc<dyn DurableEventLogRead>,
        cursor: MemberObservationCursor,
        max: u32,
        max_outcomes: u32,
        wait: Duration,
        facts: &SessionObservationFacts,
        initial_watermark: u64,
    ) -> Result<MemberEventsWindow, MemberObservationError> {
        let from_seq = Self::resolve_cursor_floor(
            cursor,
            facts.generation,
            facts.generation_start_seq,
            initial_watermark,
        )?;
        let max = usize::try_from(max).unwrap_or(usize::MAX).max(1);
        let deadline = tokio::time::Instant::now() + wait;
        // Long-poll wake: subscribe the live session stream as the SIGNAL and
        // re-read the DURABLE log after each wake — the durable log is the
        // single read authority; detached projection lag costs one extra
        // wake/read cycle (DEC-P6E-4).
        let mut wake =
            MobSessionService::subscribe_session_events(self.session_service.as_ref(), session)
                .await
                .ok();
        loop {
            // Outcomes are published through the host projection rather than
            // the durable event log. Refresh that sidecar on every long-poll
            // pass; otherwise a terminal outcome committed after the initial
            // request snapshot cannot resolve this poll when no later event
            // row wakes it. Keep the refresh fenced to the exact incarnation.
            let current_facts = self.session_facts(session)?;
            if current_facts.incarnation != facts.incarnation {
                return Err(MemberObservationError::StaleIncarnation {
                    reason: format!(
                        "poll expected {:?}; host projection changed during durable poll to {:?}",
                        facts.incarnation, current_facts.incarnation
                    ),
                });
            }
            let rows_all = log
                .read_from(session, from_seq, max)
                .await?
                .ok_or_else(|| MemberObservationError::Unavailable {
                    reason: "durable event projection disappeared mid-poll".to_string(),
                })?;
            let watermark = match log.latest_seq(session).await? {
                Some(watermark) => watermark,
                None => initial_watermark,
            };
            let rows: Vec<(u64, EventEnvelope<AgentEvent>)> = rows_all;
            let next_seq = match rows.last() {
                Some((seq, _)) => {
                    checked_event_sequence_successor(*seq, "durable member event page frontier")?
                }
                // Preserve the exact pre-read floor. Advancing a Tail
                // cursor to a later watermark after an empty read can
                // skip an event that committed between those calls.
                None => from_seq,
            };
            let (turn_outcomes, outcomes_complete) =
                Self::outcome_page(&current_facts, max_outcomes, next_seq);
            if !rows.is_empty()
                || !turn_outcomes.is_empty()
                || tokio::time::Instant::now() >= deadline
            {
                return Ok(MemberEventsWindow {
                    generation: current_facts.generation,
                    fence_token: current_facts.fence_token,
                    rows,
                    from_seq,
                    next_seq,
                    watermark,
                    turn_outcomes,
                    outcomes_complete,
                });
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let tick = remaining.min(POLL_FALLBACK_TICK);
            match wake.as_mut() {
                Some(stream) => {
                    tokio::select! {
                        item = stream.next() => {
                            if item.is_none() {
                                // Stream ended (session teardown): a finished
                                // stream must never be re-polled — fall back
                                // to the tick for the rest of the window.
                                wake = None;
                            }
                        }
                        () = tokio::time::sleep(tick) => {}
                    }
                }
                None => tokio::time::sleep(tick).await,
            }
        }
    }

    async fn poll_events_ring(
        &self,
        session: &SessionId,
        cursor: MemberObservationCursor,
        max: u32,
        wait: Duration,
        facts: &SessionObservationFacts,
    ) -> Result<MemberEventsWindow, MemberObservationError> {
        let ring = self.ring_for(session, facts.generation).await?;
        let max = usize::try_from(max).unwrap_or(usize::MAX).max(1);
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let watermark_now = {
                let state = ring
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state.next_seq.saturating_sub(1)
            };
            let from_seq = Self::resolve_cursor_floor(cursor, facts.generation, 1, watermark_now)?;
            let notified = ring.notify.notified();
            let (rows, watermark, generation) = ring.snapshot_from(from_seq, max)?;
            if !rows.is_empty() || tokio::time::Instant::now() >= deadline {
                let next_seq = match rows.last() {
                    Some((seq, _)) => checked_event_sequence_successor(
                        *seq,
                        "ephemeral member event page frontier",
                    )?,
                    None => match cursor {
                        MemberObservationCursor::Tail => checked_event_sequence_successor(
                            watermark,
                            "ephemeral member event Tail frontier",
                        )?,
                        MemberObservationCursor::At { .. } => from_seq,
                    },
                };
                return Ok(MemberEventsWindow {
                    generation,
                    fence_token: facts.fence_token,
                    rows,
                    from_seq,
                    next_seq,
                    watermark,
                    // Directive-bearing turns never reach ephemeral hosts
                    // (`RejectedHostIncapable`); the ring carries no sidecar.
                    turn_outcomes: Vec::new(),
                    outcomes_complete: true,
                });
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let tick = remaining.min(POLL_FALLBACK_TICK);
            tokio::select! {
                () = notified => {}
                () = tokio::time::sleep(tick) => {}
            }
        }
    }
}

#[async_trait::async_trait]
impl MemberObservationHost for HostMemberObservation {
    async fn member_generation(&self, session: &SessionId) -> Result<u64, MemberObservationError> {
        Ok(self.session_facts(session)?.generation)
    }

    async fn read_history(
        &self,
        session: &SessionId,
        from_index: Option<u64>,
        limit: Option<u32>,
    ) -> Result<MemberHistoryWindow, MemberObservationError> {
        let facts = self.session_facts(session)?;
        let requested_limit = limit.map(|value| usize::try_from(value).unwrap_or(HISTORY_PAGE_MAX));
        let effective_limit = requested_limit
            .unwrap_or(HISTORY_PAGE_MAX)
            .clamp(1, HISTORY_PAGE_MAX);
        let offset = match (from_index, requested_limit) {
            (Some(index), _) => usize::try_from(index).unwrap_or(usize::MAX),
            // Tail addressing (DEC-P6E-6, FLAG-P6E-5): absent `from_index`
            // with a limit serves the LAST `limit` messages; the page carries
            // the real offset + message_count so client offset math is exact.
            (None, Some(tail_limit)) => {
                let view = self.session_service.read(session).await.map_err(|error| {
                    MemberObservationError::Unavailable {
                        reason: format!("session metadata read failed: {error}"),
                    }
                })?;
                view.state
                    .message_count
                    .saturating_sub(tail_limit.clamp(1, HISTORY_PAGE_MAX))
            }
            // `from_index: None, limit: None` = full transcript from 0,
            // server-capped per page (`next_index`/`complete` continue).
            (None, None) => 0,
        };
        let page = SessionServiceHistoryExt::read_history(
            self.session_service.as_ref(),
            session,
            SessionHistoryQuery {
                offset,
                limit: Some(effective_limit),
            },
        )
        .await
        .map_err(|error| MemberObservationError::Unavailable {
            reason: format!("session history read failed: {error}"),
        })?;
        let facts_after_read = self.session_facts(session)?;
        if facts_after_read.incarnation != facts.incarnation {
            return Err(MemberObservationError::StaleIncarnation {
                reason: format!(
                    "history residency changed during read: started {:?}, completed {:?}",
                    facts.incarnation, facts_after_read.incarnation
                ),
            });
        }
        Ok(MemberHistoryWindow {
            generation: facts.generation,
            page,
        })
    }

    async fn poll_events(
        &self,
        session: &SessionId,
        request: MemberEventsPollRequest<'_>,
    ) -> Result<MemberEventsWindow, MemberObservationError> {
        let MemberEventsPollRequest {
            expected_member,
            cursor,
            max,
            wait,
            outcome_acks,
            max_outcomes,
        } = request;
        let facts_before_ack = self.session_facts(session)?;
        if &facts_before_ack.incarnation != expected_member {
            return Err(MemberObservationError::StaleIncarnation {
                reason: format!(
                    "poll expected {expected_member:?}; host projection is {:?}",
                    facts_before_ack.incarnation
                ),
            });
        }
        self.acknowledge_outcomes(&facts_before_ack, outcome_acks)
            .await?;
        // The actor publishes the pruned projection before completing the
        // acknowledgement request, so this re-read cannot replay an already
        // acknowledged row in the same response.
        let facts = self.session_facts(session)?;
        if &facts.incarnation != expected_member {
            return Err(MemberObservationError::StaleIncarnation {
                reason: format!(
                    "poll expected {expected_member:?}; host projection changed to {:?}",
                    facts.incarnation
                ),
            });
        }
        let page = match self.durable_watermark(session).await? {
            Some(watermark) => {
                let log =
                    self.durable_log
                        .clone()
                        .ok_or_else(|| MemberObservationError::Internal {
                            reason: "durable watermark without a durable log".to_string(),
                        })?;
                self.poll_events_durable(
                    session,
                    &log,
                    cursor,
                    max,
                    max_outcomes,
                    wait,
                    &facts,
                    watermark,
                )
                .await?
            }
            None => {
                self.poll_events_ring(session, cursor, max, wait, &facts)
                    .await?
            }
        };
        // A long-poll may outlive this residency. Never label rows observed
        // after rematerialization with the captured old generation/fence:
        // the controlling pump treats a matching page tuple as authority.
        let facts_after_poll = self.session_facts(session)?;
        if &facts_after_poll.incarnation != expected_member {
            return Err(MemberObservationError::StaleIncarnation {
                reason: format!(
                    "poll expected {expected_member:?}; host projection changed during poll to {:?}",
                    facts_after_poll.incarnation
                ),
            });
        }
        Ok(page)
    }

    async fn open_directed_turn_window(
        &self,
        session: &SessionId,
        expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        input_id: &str,
    ) -> Result<DirectedTurnWindow, DirectedTurnReject> {
        let facts = self
            .session_facts(session)
            .map_err(|error| DirectedTurnReject::unsupported(error.to_string()))?;
        if &facts.incarnation != expected_member {
            return Err(DirectedTurnReject {
                cause: BridgeDeliveryRejectionCause::StaleMemberIncarnation {
                    current: facts.incarnation,
                },
                detail: format!(
                    "directed-turn reserve expected stale member incarnation {expected_member:?}"
                ),
                definite_no_effect: true,
            });
        }

        // Always let the actor adjudicate the exact durable key before any
        // fresh-only preflight. A retry may arrive while its original request
        // is already accepted/running; local framing, log, or subscription
        // failure cannot be laundered into a definite no-effect rejection.
        match self.reserve_pending(&facts, input_id, None).await? {
            HostPendingReservationReply::Replayed { window_start } => {
                return Ok(DirectedTurnWindow {
                    expected_member: facts.incarnation.clone(),
                    subscription: self.directed_turn_wake_stream(session, input_id).await,
                    window_start,
                    generation: facts.generation,
                    fence_token: facts.fence_token,
                    input_id: input_id.to_string(),
                    tracking: DirectedTurnTracking::PendingReplay,
                });
            }
            HostPendingReservationReply::TerminalReplay => {
                return Ok(DirectedTurnWindow {
                    expected_member: facts.incarnation.clone(),
                    subscription: Box::pin(futures::stream::empty()),
                    // Terminal replay never scans this window.
                    window_start: facts.generation_start_seq,
                    generation: facts.generation,
                    fence_token: facts.fence_token,
                    input_id: input_id.to_string(),
                    tracking: DirectedTurnTracking::TerminalReplay,
                });
            }
            HostPendingReservationReply::FreshRequired => {}
            HostPendingReservationReply::JournalFull => {
                return Err(DirectedTurnReject::outcome_journal_full(
                    TURN_OUTCOME_RETAINED_MAX,
                    TURN_OUTCOME_RETAINED_MAX,
                ));
            }
            HostPendingReservationReply::Stale => {
                return Err(DirectedTurnReject::unsupported(format!(
                    "directed-turn residency ({}, {}) is no longer current",
                    facts.generation, facts.fence_token
                )));
            }
            HostPendingReservationReply::Reserved { .. } => {
                return Err(DirectedTurnReject::unsupported(
                    "replay-only directed-turn arbitration unexpectedly reserved a fresh key",
                ));
            }
        }

        if !directed_turn_record_framing_fits(input_id, facts.generation, facts.fence_token) {
            return Err(DirectedTurnReject::unsupported(
                "directed-turn input id leaves no room for the bounded terminal journal framing",
            ));
        }
        let watermark = self
            .durable_watermark(session)
            .await
            .map_err(|error| DirectedTurnReject::unsupported(error.to_string()))?
            .ok_or_else(|| {
                DirectedTurnReject::unsupported(
                    "host has no durable event log; tracked turns need the durable outcome journal",
                )
            })?;
        let fresh_window_start = directed_turn_window_start(watermark).map_err(|error| *error)?;
        // The stream is a latency hint, not outcome authority. Open it before
        // Fresh reserve when available; otherwise durable polling preserves
        // the pre-accept window without inventing a no-effect result.
        let subscription = self.directed_turn_wake_stream(session, input_id).await;
        let reservation = self
            .reserve_pending(&facts, input_id, Some(fresh_window_start))
            .await?;
        let (tracking, window_start) = match reservation {
            HostPendingReservationReply::Reserved { window_start } => {
                (DirectedTurnTracking::PendingFresh, window_start)
            }
            HostPendingReservationReply::Replayed { window_start } => {
                (DirectedTurnTracking::PendingReplay, window_start)
            }
            HostPendingReservationReply::TerminalReplay => {
                (DirectedTurnTracking::TerminalReplay, watermark)
            }
            HostPendingReservationReply::FreshRequired => {
                return Err(DirectedTurnReject::unsupported(
                    "fresh directed-turn reservation unexpectedly remained replay-only",
                ));
            }
            HostPendingReservationReply::JournalFull => {
                return Err(DirectedTurnReject::outcome_journal_full(
                    TURN_OUTCOME_RETAINED_MAX,
                    TURN_OUTCOME_RETAINED_MAX,
                ));
            }
            HostPendingReservationReply::Stale => {
                return Err(DirectedTurnReject::unsupported(format!(
                    "directed-turn residency ({}, {}) is no longer current",
                    facts.generation, facts.fence_token
                )));
            }
        };
        Ok(DirectedTurnWindow {
            expected_member: facts.incarnation.clone(),
            subscription,
            window_start,
            generation: facts.generation,
            fence_token: facts.fence_token,
            input_id: input_id.to_string(),
            tracking,
        })
    }

    async fn cancel_directed_turn_window(
        &self,
        session: &SessionId,
        window: DirectedTurnWindow,
    ) -> Result<(), DirectedTurnReject> {
        let expected_member = window.expected_member.clone();
        let generation = window.generation;
        let fence_token = window.fence_token;
        let input_id = window.input_id.clone();
        let tracking = window.tracking;
        drop(window);
        self.cancel_pending(
            &expected_member,
            generation,
            fence_token,
            input_id,
            tracking,
        )
        .await
    }

    async fn lock_and_revalidate_directed_turn_admission(
        &self,
        session: &SessionId,
        request: DirectedTurnAdmissionRequest,
    ) -> Result<DirectedTurnAdmissionDecision, DirectedTurnReject> {
        let guard = self
            .lock_directed_turn_key(session, &request.expected_member, &request.input_id)
            .await;
        let facts = self
            .session_facts(session)
            .map_err(|error| DirectedTurnReject::ambiguous(error.to_string()))?;
        if facts.incarnation != request.expected_member
            || facts.generation != request.generation
            || facts.fence_token != request.fence_token
        {
            return Err(DirectedTurnReject::ambiguous(format!(
                "directed-turn residency changed before final admission revalidation: reserved {:?}, current {:?}",
                request.expected_member, facts.incarnation
            )));
        }
        match self
            .reserve_pending(&facts, &request.input_id, None)
            .await?
        {
            HostPendingReservationReply::Replayed { window_start }
                if window_start == request.window_start =>
            {
                Ok(DirectedTurnAdmissionDecision::Admit(
                    DirectedTurnAdmissionPermit::new(guard),
                ))
            }
            HostPendingReservationReply::TerminalReplay => {
                drop(guard);
                Ok(DirectedTurnAdmissionDecision::TerminalReplay)
            }
            HostPendingReservationReply::Replayed { window_start } => {
                Err(DirectedTurnReject::ambiguous(format!(
                    "directed-turn Pending window changed from {} to {window_start}",
                    request.window_start
                )))
            }
            HostPendingReservationReply::Stale => Err(DirectedTurnReject::ambiguous(
                "directed-turn residency became stale before final admission".to_string(),
            )),
            HostPendingReservationReply::FreshRequired => Err(DirectedTurnReject::ambiguous(
                "directed-turn Pending disappeared before final admission".to_string(),
            )),
            HostPendingReservationReply::Reserved { .. } => Err(DirectedTurnReject::ambiguous(
                "final admission revalidation unexpectedly created Pending".to_string(),
            )),
            HostPendingReservationReply::JournalFull => Err(DirectedTurnReject::ambiguous(
                "directed-turn Pending disappeared into journal-capacity classification"
                    .to_string(),
            )),
        }
    }

    async fn cancel_tracked_member_input(
        &self,
        session: &SessionId,
        expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        input_id: &str,
    ) -> Result<BridgeTrackedInputCancelOutcome, MemberObservationError> {
        let _guard = self
            .lock_directed_turn_key(session, expected_member, input_id)
            .await;
        let facts = self.session_facts(session)?;
        if &facts.incarnation != expected_member {
            return Err(MemberObservationError::StaleIncarnation {
                reason: format!(
                    "tracked-input cancel expected {expected_member:?}; host projection is {:?}",
                    facts.incarnation
                ),
            });
        }

        #[cfg(feature = "runtime-adapter")]
        {
            use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;

            let adapter = self.session_service.runtime_adapter().ok_or_else(|| {
                MemberObservationError::Unavailable {
                    reason: "host has no runtime adapter for tracked-input cancellation"
                        .to_string(),
                }
            })?;
            let runtime_input = adapter
                .input_state_by_idempotency_key(session, input_id)
                .await
                .map_err(|error| MemberObservationError::Unavailable {
                    reason: format!(
                        "tracked-input cancellation could not query runtime custody: {error}"
                    ),
                })?;
            let runtime_input_present = runtime_input.is_some();
            if runtime_input
                .as_ref()
                .is_some_and(|stored| stored.seed.terminal_outcome.is_some())
            {
                // Ensure recovery is not merely configured but has actually
                // had a chance to attach the terminal watcher. The ordinary
                // background loop remains the liveness owner; this focused
                // pass closes the restart-to-first-cancel race.
                let _ = self.recover_pending_turns_once().await;
                self.await_runtime_terminal_host_custody(session, expected_member, input_id)
                    .await?;
            }
            match self
                .actor_cancel_tracked_input(expected_member, input_id, runtime_input_present)
                .await?
            {
                HostTrackedInputCancelReply::NoEffect => {
                    return Ok(BridgeTrackedInputCancelOutcome::NoEffect);
                }
                HostTrackedInputCancelReply::Cancelled => {
                    return Ok(BridgeTrackedInputCancelOutcome::Cancelled);
                }
                HostTrackedInputCancelReply::Terminal(record) => {
                    return Ok(BridgeTrackedInputCancelOutcome::Terminal { record });
                }
                HostTrackedInputCancelReply::Stale => {
                    return Err(MemberObservationError::StaleIncarnation {
                        reason: "tracked-input cancellation residency is no longer current"
                            .to_string(),
                    });
                }
                HostTrackedInputCancelReply::Unreserved => {
                    return Err(MemberObservationError::Internal {
                        reason: format!(
                            "runtime input '{input_id}' exists without host Pending/terminal/cancel custody"
                        ),
                    });
                }
                HostTrackedInputCancelReply::Cancelling => {}
            }

            // The durable `Cancelling` receipt is already installed, so every
            // delayed Deliver is blocked before this potentially slow runtime
            // convergence. A retry resumes this exact level-triggered step.
            adapter
                .cancel_tracked_input_for_member_incarnation(session, input_id, expected_member)
                .await
                .map_err(|error| MemberObservationError::Unavailable {
                    reason: format!(
                        "tracked-input runtime cancellation has not converged: {error}"
                    ),
                })?;
            match self
                .actor_complete_tracked_input_cancel(expected_member, input_id)
                .await?
            {
                HostTrackedInputCancelReply::Cancelled => {
                    Ok(BridgeTrackedInputCancelOutcome::Cancelled)
                }
                HostTrackedInputCancelReply::NoEffect => {
                    Ok(BridgeTrackedInputCancelOutcome::NoEffect)
                }
                HostTrackedInputCancelReply::Terminal(record) => {
                    Ok(BridgeTrackedInputCancelOutcome::Terminal { record })
                }
                other => Err(MemberObservationError::Internal {
                    reason: format!(
                        "tracked-input cancellation completion did not reach a terminal: {other:?}"
                    ),
                }),
            }
        }

        #[cfg(not(feature = "runtime-adapter"))]
        {
            let _ = (session, expected_member, input_id);
            Err(MemberObservationError::Unavailable {
                reason: "tracked-input cancellation requires the runtime-adapter feature"
                    .to_string(),
            })
        }
    }

    async fn admit_directed_turn(
        &self,
        session: &SessionId,
        admission: DirectedTurnAdmission,
    ) -> Result<(), DirectedTurnReject> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(DirectedTurnReject::unsupported(
                "member observation is shutting down; retaining the directed turn Pending",
            ));
        }
        let log = self.durable_log.clone().ok_or_else(|| {
            DirectedTurnReject::unsupported(
                "host has no durable event log; tracked turns need the durable outcome journal",
            )
        })?;
        if admission.window.input_id != admission.input_id {
            return Err(DirectedTurnReject::unsupported(
                "directed-turn admission input id differs from its durable reservation",
            ));
        }
        let facts = self
            .session_facts(session)
            .map_err(|error| DirectedTurnReject::unsupported(error.to_string()))?;
        if admission.window.expected_member != facts.incarnation {
            return Err(DirectedTurnReject::unsupported(format!(
                "directed-turn admission residency changed: reserved {:?}, current {:?}",
                admission.window.expected_member, facts.incarnation,
            )));
        }
        if admission.window.tracking == DirectedTurnTracking::TerminalReplay {
            return Ok(());
        }
        let watcher_key = ActiveTurnWatcherKey {
            session_id: session.clone(),
            generation: admission.window.generation,
            fence_token: admission.window.fence_token,
            input_id: admission.input_id.clone(),
        };
        let Some(watcher_claim) =
            ActiveTurnWatcherClaim::try_acquire(&self.active_turn_watchers, watcher_key)
        else {
            // Live redelivery and the background reconciler may race. The
            // exact key already has a watcher, so dropping this duplicate
            // completion handle is safe and does not create a second writer.
            return Ok(());
        };

        #[cfg(feature = "runtime-adapter")]
        {
            use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;

            let adapter = self.session_service.runtime_adapter().ok_or_else(|| {
                DirectedTurnReject::unsupported(
                    "host has no runtime adapter for durable directed-turn attribution",
                )
            })?;
            let stored = adapter
                .input_state_by_idempotency_key(session, &admission.input_id)
                .await
                .map_err(|error| {
                    DirectedTurnReject::unsupported(format!(
                        "cannot verify directed-turn runtime attribution: {error}"
                    ))
                })?
                .ok_or_else(|| {
                    DirectedTurnReject::unsupported(
                        "accepted directed turn is absent from the runtime idempotency ledger",
                    )
                })?;
            let attribution =
                DirectedTurnRuntimeAttribution::from_stored(&admission.input_id, &stored)
                    .map_err(DirectedTurnReject::unsupported)?;
            let session = session.clone();
            let mut tasks = self.directed_turn_tasks.lock().await;
            if self.shutting_down.load(Ordering::Acquire) {
                return Err(DirectedTurnReject::unsupported(
                    "member observation began shutting down before watcher attachment; retaining the directed turn Pending",
                ));
            }
            while let Some(completion) = tasks.try_join_next() {
                if let Err(error) = completion {
                    tracing::warn!(
                        error = %error,
                        "directed-turn watcher task ended without a clean join"
                    );
                }
            }
            tasks.spawn(run_directed_turn_watcher(
                session,
                admission,
                log,
                adapter,
                attribution,
                watcher_claim,
            ));
            Ok(())
        }

        #[cfg(not(feature = "runtime-adapter"))]
        {
            let _ = (session, admission, log, watcher_claim);
            Err(DirectedTurnReject::unsupported(
                "durable directed-turn attribution requires the runtime-adapter feature",
            ))
        }
    }
}

#[derive(Debug, Clone)]
struct DirectedTurnRuntimeAttribution {
    runtime_input_id: meerkat_core::lifecycle::InputId,
    admission_sequence: u64,
    last_boundary_sequence: Option<u64>,
    terminal_outcome: Option<meerkat_runtime::input_state::InputTerminalOutcome>,
    phase: meerkat_runtime::input_state::InputLifecycleState,
    expected_content: meerkat_core::types::ContentInput,
    tracking_kind: DirectedTurnTrackingKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectedTurnTrackingKind {
    FlowStep,
    PeerInteraction,
}

impl DirectedTurnRuntimeAttribution {
    /// Prove that the idempotency lookup, the stored shell, and the persisted
    /// runtime input all name the exact Pending obligation. A Pending row is
    /// deliberately not acceptance evidence; only this runtime-owned binding
    /// can authorize recovery.
    fn from_stored(
        pending_input_id: &str,
        stored: &meerkat_runtime::input_state::StoredInputState,
    ) -> Result<Self, String> {
        use meerkat_runtime::input_state::{InputLifecycleState, InputTerminalOutcome};

        let shell_key = stored
            .state
            .idempotency_key
            .as_ref()
            .ok_or_else(|| "runtime input has no stored idempotency key".to_string())?;
        if shell_key.0 != pending_input_id {
            return Err(format!(
                "runtime idempotency binding '{}' differs from Pending '{}'",
                shell_key.0, pending_input_id
            ));
        }
        let persisted = stored
            .state
            .persisted_input
            .as_ref()
            .ok_or_else(|| "runtime input has no persisted replay payload".to_string())?;
        if persisted.id() != &stored.state.input_id {
            return Err(format!(
                "persisted runtime input id '{}' differs from stored shell '{}'",
                persisted.id(),
                stored.state.input_id
            ));
        }
        let header_key = persisted
            .header()
            .idempotency_key
            .as_ref()
            .ok_or_else(|| "persisted runtime input has no idempotency key".to_string())?;
        if header_key.0 != pending_input_id {
            return Err(format!(
                "persisted input key '{}' differs from Pending '{}'",
                header_key.0, pending_input_id
            ));
        }
        if persisted.header().durability != meerkat_runtime::input::InputDurability::Durable {
            return Err("directed-turn runtime input is not durable".to_string());
        }
        let expected_content = meerkat_runtime::input::directed_input_run_started_content(
            persisted,
        )
        .map_err(|reason| {
            format!(
                "directed-turn idempotency binding points at an invalid {:?} input: {reason}",
                persisted.kind()
            )
        })?;
        let tracking_kind = match persisted {
            meerkat_runtime::input::Input::FlowStep(_) => DirectedTurnTrackingKind::FlowStep,
            meerkat_runtime::input::Input::Peer(_) => DirectedTurnTrackingKind::PeerInteraction,
            other => {
                return Err(format!(
                    "directed-turn validation admitted unsupported {:?} input",
                    other.kind()
                ));
            }
        };
        let admission_sequence = stored.seed.admission_sequence.ok_or_else(|| {
            "accepted directed-turn runtime input has no admission sequence".to_string()
        })?;

        let phase_matches_terminal = matches!(
            (&stored.seed.phase, &stored.seed.terminal_outcome),
            (
                InputLifecycleState::Consumed,
                Some(InputTerminalOutcome::Consumed)
            ) | (
                InputLifecycleState::Superseded,
                Some(InputTerminalOutcome::Superseded { .. })
            ) | (
                InputLifecycleState::Coalesced,
                Some(InputTerminalOutcome::Coalesced { .. })
            ) | (
                InputLifecycleState::Abandoned,
                Some(InputTerminalOutcome::Abandoned { .. })
            )
        );
        let phase_is_terminal = matches!(
            stored.seed.phase,
            InputLifecycleState::Consumed
                | InputLifecycleState::Superseded
                | InputLifecycleState::Coalesced
                | InputLifecycleState::Abandoned
        );
        if phase_is_terminal != stored.seed.terminal_outcome.is_some()
            || (phase_is_terminal && !phase_matches_terminal)
        {
            return Err(format!(
                "runtime phase {:?} and terminal outcome {:?} disagree",
                stored.seed.phase, stored.seed.terminal_outcome
            ));
        }
        if matches!(
            stored.seed.terminal_outcome,
            Some(InputTerminalOutcome::Consumed)
        ) && (stored.seed.last_run_id.is_none() || stored.seed.last_boundary_sequence.is_none())
        {
            return Err(
                "consumed directed-turn runtime input lacks its run/boundary attribution"
                    .to_string(),
            );
        }

        Ok(Self {
            runtime_input_id: stored.state.input_id.clone(),
            admission_sequence,
            last_boundary_sequence: stored.seed.last_boundary_sequence,
            terminal_outcome: stored.seed.terminal_outcome.clone(),
            phase: stored.seed.phase,
            expected_content,
            tracking_kind,
        })
    }

    fn same_binding(&self, next: &Self) -> bool {
        self.runtime_input_id == next.runtime_input_id
            && self.admission_sequence == next.admission_sequence
            && self.expected_content == next.expected_content
            && self.tracking_kind == next.tracking_kind
    }

    fn is_terminal(&self) -> bool {
        self.terminal_outcome.is_some()
    }
}

#[derive(Debug)]
enum CompletionTerminalExpectation {
    Completed {
        output: String,
        structured_output: Option<serde_json::Value>,
    },
    Failed {
        kind: Option<meerkat_core::turn_terminal::TurnTerminalKind>,
        reason: String,
    },
}

impl CompletionTerminalExpectation {
    fn from_completion(outcome: meerkat_runtime::completion::CompletionOutcome) -> Self {
        use meerkat_core::turn_terminal::TurnTerminalKind;
        use meerkat_runtime::completion::CompletionOutcome;

        match outcome {
            CompletionOutcome::Completed(result) => {
                let result = *result;
                if let Some(extraction) = result.extraction_error {
                    Self::Failed {
                        kind: Some(TurnTerminalKind::ExtractionFailed),
                        reason: format!(
                            "structured output extraction failed after {} attempt(s): {}; last_output={:?}",
                            extraction.attempts, extraction.reason, extraction.last_output
                        ),
                    }
                } else {
                    Self::Completed {
                        output: result.text,
                        structured_output: result.structured_output,
                    }
                }
            }
            CompletionOutcome::CompletedWithoutResult => Self::Completed {
                output: String::new(),
                structured_output: None,
            },
            CompletionOutcome::CallbackPending { tool_name, args } => Self::Failed {
                kind: Some(TurnTerminalKind::InteractionCallbackPending),
                reason: format!("callback pending for tool '{tool_name}': {args}"),
            },
            CompletionOutcome::Cancelled => Self::Failed {
                kind: None,
                reason: "cancelled".to_string(),
            },
            CompletionOutcome::Abandoned { reason, .. }
            | CompletionOutcome::AbandonedWithError { reason, .. }
            | CompletionOutcome::RuntimeTerminated { reason, .. } => {
                Self::Failed { kind: None, reason }
            }
            CompletionOutcome::CompletedWithFinalizationFailure { error } => Self::Failed {
                kind: Some(TurnTerminalKind::InteractionFailed),
                reason: error
                    .detail
                    .unwrap_or_else(|| "turn finalization failed".to_string()),
            },
        }
    }

    fn matches(&self, terminal: &meerkat_core::turn_terminal::ClassifiedTurnTerminal) -> bool {
        use meerkat_core::turn_terminal::TurnTerminalOutcome;

        match (self, &terminal.outcome) {
            (
                Self::Completed {
                    output: expected_output,
                    structured_output: expected_structured,
                },
                TurnTerminalOutcome::Completed {
                    output,
                    structured_output,
                },
            ) => output == expected_output && structured_output == expected_structured,
            (
                Self::Failed {
                    kind: expected_kind,
                    reason: expected_reason,
                },
                TurnTerminalOutcome::Failed { reason },
            ) => {
                expected_kind.is_none_or(|kind| kind == terminal.kind) && reason == expected_reason
            }
            _ => false,
        }
    }
}

#[derive(Debug)]
struct DurableTerminalCandidate {
    terminal: meerkat_core::turn_terminal::ClassifiedTurnTerminal,
    terminal_seq: u64,
    run_started_in_window: bool,
    run_matches_expected_content: bool,
    interaction_id: Option<InteractionId>,
}

struct DurableTerminalScan {
    terminals: Vec<DurableTerminalCandidate>,
    matching_run_starts: usize,
    watermark: u64,
}

/// Exhaustively fold one immutable durable snapshot from the original
/// pre-accept window. Every terminal is retained: the completion/runtime
/// attribution selector decides which one belongs to this exact input, so a
/// prior turn projected after window-open cannot steal the obligation.
async fn scan_durable_terminals(
    session: &SessionId,
    window_start: u64,
    expected_content: &meerkat_core::types::ContentInput,
    log: &Arc<dyn DurableEventLogRead>,
) -> Result<DurableTerminalScan, MemberObservationError> {
    let watermark = log.latest_seq(session).await?.unwrap_or(0);
    if watermark < window_start {
        return Ok(DurableTerminalScan {
            terminals: Vec::new(),
            matching_run_starts: 0,
            watermark,
        });
    }
    let mut classifier = meerkat_core::turn_terminal::TurnTerminalClassifier::default();
    let mut from_seq = window_start;
    let mut terminals = Vec::new();
    let mut matching_run_starts = 0usize;
    let mut run_started_in_window = false;
    let mut run_matches_expected_content = false;
    while from_seq <= watermark {
        let rows = log
            .read_from(session, from_seq, TERMINAL_SEQ_SCAN_PAGE)
            .await?
            .ok_or_else(|| MemberObservationError::Unavailable {
                reason: "durable event projection disappeared during terminal reconstruction"
                    .to_string(),
            })?;
        let mut last_scanned = None;
        for (durable_seq, envelope) in rows {
            if durable_seq > watermark {
                break;
            }
            last_scanned = Some(durable_seq);
            if let AgentEvent::RunStarted { input, .. } = &envelope.payload {
                classifier = meerkat_core::turn_terminal::TurnTerminalClassifier::default();
                run_started_in_window = true;
                run_matches_expected_content = input.content() == Some(expected_content);
                matching_run_starts =
                    matching_run_starts.saturating_add(usize::from(run_matches_expected_content));
            }
            let interaction_id = match &envelope.source {
                EventSourceIdentity::Interaction { interaction_id } => Some(*interaction_id),
                _ => None,
            };
            // Exact interaction receipts are an independent durable lane. Do
            // not feed them through (or reset) the active run classifier: an
            // earlier input's receipt can be projected after the next
            // identical-content RunStarted and before that next run finishes.
            // Mixing the lanes would steal the next run's bracket.
            if let Some(interaction_id) = interaction_id {
                let mut receipt_classifier =
                    meerkat_core::turn_terminal::TurnTerminalClassifier::default();
                let Some(terminal) = receipt_classifier.observe(&envelope.payload) else {
                    return Err(MemberObservationError::Internal {
                        reason: format!(
                            "interaction-sourced durable row at seq {durable_seq} is not terminal"
                        ),
                    });
                };
                terminals.push(DurableTerminalCandidate {
                    terminal,
                    terminal_seq: durable_seq,
                    run_started_in_window: false,
                    run_matches_expected_content: false,
                    interaction_id: Some(interaction_id),
                });
                continue;
            }
            if let Some(terminal) = classifier.observe(&envelope.payload) {
                terminals.push(DurableTerminalCandidate {
                    terminal,
                    terminal_seq: durable_seq,
                    run_started_in_window,
                    run_matches_expected_content,
                    interaction_id: None,
                });
                classifier = meerkat_core::turn_terminal::TurnTerminalClassifier::default();
                run_started_in_window = false;
                run_matches_expected_content = false;
            }
        }
        let Some(last_scanned) = last_scanned else {
            return Err(MemberObservationError::Internal {
                reason: format!(
                    "durable terminal scan returned no row at seq {from_seq} below watermark {watermark}"
                ),
            });
        };
        if last_scanned == watermark {
            break;
        }
        from_seq =
            checked_event_sequence_successor(last_scanned, "durable directed-turn terminal scan")?;
    }
    Ok(DurableTerminalScan {
        terminals,
        matching_run_starts,
        watermark,
    })
}

enum TerminalSelection<'a> {
    Selected(&'a DurableTerminalCandidate),
    AwaitMore,
    Ambiguous(&'static str),
}

fn select_completion_terminal<'a>(
    scan: &'a DurableTerminalScan,
    expectation: &CompletionTerminalExpectation,
    attribution: &DirectedTurnRuntimeAttribution,
    interaction_id: InteractionId,
) -> TerminalSelection<'a> {
    let exact = scan
        .terminals
        .iter()
        .filter(|candidate| candidate.interaction_id == Some(interaction_id))
        .collect::<Vec<_>>();
    let exact = match exact.as_slice() {
        [candidate] if expectation.matches(&candidate.terminal) => *candidate,
        [_, _, ..] => {
            return TerminalSelection::Ambiguous(
                "multiple durable interaction terminals name one directed input",
            );
        }
        [_] => {
            return TerminalSelection::Ambiguous(
                "exact durable interaction terminal disagrees with completion authority",
            );
        }
        [] => return TerminalSelection::AwaitMore,
    };

    // A tracked plain Peer owns the interaction-scoped publication itself.
    // Its neighboring run terminal belongs to the provider run family and is
    // not the host-journal contract this controller requested. FlowStep keeps
    // the legacy run-bracket selection below.
    if attribution.tracking_kind == DirectedTurnTrackingKind::PeerInteraction {
        return TerminalSelection::Selected(exact);
    }

    // The exact interaction receipt is the immutable attribution anchor. A
    // payload-equal run may begin or finish after that receipt while this
    // durable window is still being scanned; such a later run cannot own this
    // directed input. Prefer the nearest matching run terminal strictly before
    // the receipt, irrespective of how many identical-content runs exist later
    // in the window.
    if let Some(candidate) = scan
        .terminals
        .iter()
        .filter(|candidate| candidate.terminal_seq < exact.terminal_seq)
        .filter(|candidate| candidate.interaction_id.is_none())
        .filter(|candidate| candidate.run_matches_expected_content)
        .filter(|candidate| expectation.matches(&candidate.terminal))
        .max_by_key(|candidate| candidate.terminal_seq)
    {
        return TerminalSelection::Selected(candidate);
    }

    // A nonzero boundary means the input joined a run that was already live
    // when Pending opened; that run has no RunStarted inside the window. The
    // exact receipt still bounds attribution, so choose the nearest matching
    // unbracketed terminal before it and never a later payload-equal turn.
    if attribution
        .last_boundary_sequence
        .is_some_and(|sequence| sequence > 0)
        && let Some(candidate) = scan
            .terminals
            .iter()
            .filter(|candidate| candidate.terminal_seq < exact.terminal_seq)
            .filter(|candidate| candidate.interaction_id.is_none())
            .filter(|candidate| expectation.matches(&candidate.terminal))
            .filter(|candidate| !candidate.run_started_in_window)
            .max_by_key(|candidate| candidate.terminal_seq)
    {
        return TerminalSelection::Selected(candidate);
    }

    // Callback-pending, cancellation, and other runless terminals have no
    // preceding run terminal. Their exact interaction row is itself the
    // canonical terminal and remains safe because it is identity-bound.
    TerminalSelection::Selected(exact)
}

fn select_recovered_terminal<'a>(
    scan: &'a DurableTerminalScan,
    attribution: &DirectedTurnRuntimeAttribution,
    interaction_id: InteractionId,
) -> TerminalSelection<'a> {
    let exact = scan
        .terminals
        .iter()
        .filter(|candidate| candidate.interaction_id == Some(interaction_id))
        .collect::<Vec<_>>();
    let exact = match exact.as_slice() {
        [candidate] => *candidate,
        [_, _, ..] => {
            return TerminalSelection::Ambiguous(
                "multiple durable interaction terminals name one recovered input",
            );
        }
        [] => return TerminalSelection::AwaitMore,
    };

    if attribution.tracking_kind == DirectedTurnTrackingKind::PeerInteraction {
        return TerminalSelection::Selected(exact);
    }

    if let Some(candidate) = scan
        .terminals
        .iter()
        .filter(|candidate| candidate.terminal_seq < exact.terminal_seq)
        .filter(|candidate| candidate.interaction_id.is_none())
        .filter(|candidate| candidate.run_matches_expected_content)
        .filter(|candidate| candidate.terminal.outcome == exact.terminal.outcome)
        .max_by_key(|candidate| candidate.terminal_seq)
    {
        return TerminalSelection::Selected(candidate);
    }
    if attribution
        .last_boundary_sequence
        .is_some_and(|sequence| sequence > 0)
        && let Some(candidate) = scan
            .terminals
            .iter()
            .filter(|candidate| candidate.terminal_seq < exact.terminal_seq)
            .filter(|candidate| candidate.interaction_id.is_none())
            .filter(|candidate| candidate.terminal.outcome == exact.terminal.outcome)
            .filter(|candidate| !candidate.run_started_in_window)
            .max_by_key(|candidate| candidate.terminal_seq)
    {
        return TerminalSelection::Selected(candidate);
    }
    TerminalSelection::Selected(exact)
}

async fn refresh_terminal_attribution(
    adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    session: &SessionId,
    pending_input_id: &str,
    original: &DirectedTurnRuntimeAttribution,
    deadline: tokio::time::Instant,
) -> Option<DirectedTurnRuntimeAttribution> {
    use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;

    loop {
        match adapter
            .input_state_by_idempotency_key(session, pending_input_id)
            .await
        {
            Ok(Some(stored)) => {
                match DirectedTurnRuntimeAttribution::from_stored(pending_input_id, &stored) {
                    Ok(next) if !original.same_binding(&next) => {
                        tracing::error!(
                            session_id = %session,
                            input_id = %pending_input_id,
                            "runtime attribution changed while a directed turn was completing; retaining Pending"
                        );
                        return None;
                    }
                    Ok(next) if next.is_terminal() => return Some(next),
                    Ok(_) => {}
                    Err(reason) => {
                        tracing::error!(
                            session_id = %session,
                            input_id = %pending_input_id,
                            reason = %reason,
                            "runtime attribution became invalid while a directed turn was completing; retaining Pending"
                        );
                        return None;
                    }
                }
            }
            Ok(None) => {
                tracing::error!(
                    session_id = %session,
                    input_id = %pending_input_id,
                    "accepted directed turn disappeared from the runtime idempotency ledger; retaining Pending"
                );
                return None;
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %session,
                    input_id = %pending_input_id,
                    error = %error,
                    "retrying durable runtime attribution query"
                );
            }
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            tracing::error!(
                session_id = %session,
                input_id = %pending_input_id,
                "runtime terminal attribution did not become durable within the bind budget; retaining Pending"
            );
            return None;
        }
        tokio::time::sleep(remaining.min(POLL_FALLBACK_TICK)).await;
    }
}

async fn record_directed_turn_terminal(
    session: &SessionId,
    input_id: &str,
    generation: u64,
    fence_token: u64,
    terminal_seq: u64,
    outcome: WireFlowTurnOutcome,
    journal: &Arc<dyn TrackedTurnJournal>,
) {
    let record = compact_tracked_turn_outcome_record(TrackedTurnOutcomeRecord {
        input_id: input_id.to_string(),
        generation,
        fence_token,
        terminal_seq,
        outcome,
    });
    if let Err(error) = journal.record_turn_outcome(record).await {
        tracing::error!(
            session_id = %session,
            input_id = %input_id,
            error = %error,
            "tracked-turn journal record failed; the step resolves via the controlling timeout ladder"
        );
    }
}

/// The tracked-turn watcher (DEC-P6E-16/17). Pending pins the original
/// pre-accept durable window, while generated completion authority and the
/// durable runtime input ledger jointly bind the exact terminal. Projection
/// latency can only delay/retain Pending; it can never mint `ChannelClosed`.
async fn run_directed_turn_watcher(
    session: SessionId,
    admission: DirectedTurnAdmission,
    log: Arc<dyn DurableEventLogRead>,
    adapter: Arc<meerkat_runtime::MeerkatMachine>,
    initial_attribution: DirectedTurnRuntimeAttribution,
    _watcher_claim: ActiveTurnWatcherClaim,
) {
    use meerkat_runtime::completion::CompletionWaitError;
    use meerkat_runtime::input_state::InputTerminalOutcome;

    let DirectedTurnAdmission {
        input_id,
        window,
        completion_handle,
        journal,
    } = admission;
    let DirectedTurnWindow {
        expected_member,
        mut subscription,
        window_start,
        generation,
        fence_token,
        input_id: reserved_input_id,
        tracking: _,
    } = window;
    if journal.member_incarnation() != &expected_member {
        tracing::error!(
            session_id = %session,
            input_id = %input_id,
            expected = ?expected_member,
            journal = ?journal.member_incarnation(),
            "directed-turn watcher journal incarnation differs from its reserved residency"
        );
        return;
    }
    if input_id != reserved_input_id {
        tracing::error!(
            session_id = %session,
            input_id = %input_id,
            reserved_input_id = %reserved_input_id,
            "directed-turn watcher reservation mismatch"
        );
        return;
    }
    let interaction_id = match uuid::Uuid::parse_str(&input_id) {
        Ok(id) => InteractionId(id),
        Err(error) => {
            tracing::error!(
                session_id = %session,
                input_id = %input_id,
                error = %error,
                "directed-turn stable input id is not an interaction UUID"
            );
            return;
        }
    };

    let completion_result = match completion_handle {
        Some(handle) => Some(handle.try_wait().await),
        None => None,
    };
    if matches!(
        &completion_result,
        Some(Err(
            CompletionWaitError::AttachmentReplaced | CompletionWaitError::AuthorityUnavailable(_)
        ))
    ) {
        if let Some(Err(error)) = &completion_result {
            tracing::error!(
                session_id = %session,
                input_id = %input_id,
                error = %error,
                "completion authority unavailable; recording no terminal and retaining Pending"
            );
        }
        return;
    }

    // ChannelClosed is mechanical waiter loss, never a public terminal fact.
    // Publish nothing: the durable tracked turn remains Pending and recovery
    // reattaches to the exact interaction terminal outbox/event row.
    if matches!(
        &completion_result,
        Some(Err(CompletionWaitError::ChannelClosed))
    ) {
        tracing::warn!(
            session_id = %session,
            input_id = %input_id,
            "completion waiter channel closed mechanically; retaining tracked turn Pending"
        );
        return;
    }

    let bind_deadline = tokio::time::Instant::now() + POST_COMPLETION_TERMINAL_BIND_TIMEOUT;
    let attribution = if completion_result.is_some() {
        let Some(attribution) = refresh_terminal_attribution(
            &adapter,
            &session,
            &input_id,
            &initial_attribution,
            bind_deadline,
        )
        .await
        else {
            return;
        };
        attribution
    } else {
        initial_attribution
    };
    let expectation = match completion_result {
        Some(Ok(outcome)) => Some(CompletionTerminalExpectation::from_completion(outcome)),
        Some(Err(error)) => {
            tracing::error!(
                session_id = %session,
                input_id = %input_id,
                error = %error,
                "unclassified completion wait failure; recording nothing and retaining Pending"
            );
            return;
        }
        None => {
            if !matches!(
                attribution.terminal_outcome,
                Some(InputTerminalOutcome::Consumed)
            ) {
                tracing::warn!(
                    session_id = %session,
                    input_id = %input_id,
                    phase = ?attribution.phase,
                    terminal_outcome = ?attribution.terminal_outcome,
                    "recovered directed turn has no event-bearing consumed terminal; retaining Pending"
                );
                return;
            }
            None
        }
    };

    let mut subscription_open = true;
    let mut traced_watermark = None;
    loop {
        match scan_durable_terminals(&session, window_start, &attribution.expected_content, &log)
            .await
        {
            Ok(scan) => {
                if traced_watermark != Some(scan.watermark) {
                    tracing::trace!(
                        session_id = %session,
                        input_id = %input_id,
                        generation,
                        fence_token,
                        window_start,
                        watermark = scan.watermark,
                        admission_sequence = attribution.admission_sequence,
                        boundary_sequence = ?attribution.last_boundary_sequence,
                        expected_content = ?attribution.expected_content,
                        matching_run_starts = scan.matching_run_starts,
                        terminals = ?scan.terminals,
                        expectation = ?expectation,
                        "directed-turn durable terminal scan"
                    );
                    traced_watermark = Some(scan.watermark);
                }
                let selection = if let Some(expectation) = &expectation {
                    select_completion_terminal(&scan, expectation, &attribution, interaction_id)
                } else {
                    select_recovered_terminal(&scan, &attribution, interaction_id)
                };
                match selection {
                    TerminalSelection::Selected(candidate) => {
                        let outcome = match wire_outcome_from_terminal(
                            attribution.tracking_kind,
                            candidate.terminal.kind,
                            &candidate.terminal.outcome,
                        ) {
                            Ok(outcome) => outcome,
                            Err(reason) => {
                                tracing::error!(
                                    session_id = %session,
                                    input_id = %input_id,
                                    tracking_kind = ?attribution.tracking_kind,
                                    terminal_kind = %candidate.terminal.kind,
                                    reason,
                                    "selected directed terminal violates its persisted tracking family; retaining Pending"
                                );
                                return;
                            }
                        };
                        record_directed_turn_terminal(
                            &session,
                            &input_id,
                            generation,
                            fence_token,
                            candidate.terminal_seq,
                            outcome,
                            &journal,
                        )
                        .await;
                        return;
                    }
                    TerminalSelection::Ambiguous(reason) => {
                        tracing::error!(
                            session_id = %session,
                            input_id = %input_id,
                            reason,
                            watermark = scan.watermark,
                            admission_sequence = attribution.admission_sequence,
                            boundary_sequence = ?attribution.last_boundary_sequence,
                            "durable terminal attribution is ambiguous; recording nothing and retaining Pending"
                        );
                        return;
                    }
                    TerminalSelection::AwaitMore => {}
                }
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %session,
                    input_id = %input_id,
                    error = %error,
                    "durable terminal reconstruction retrying within bind window"
                );
            }
        }
        let remaining = bind_deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            tracing::error!(
                session_id = %session,
                input_id = %input_id,
                "no exact durable terminal appeared within the bind budget; recording nothing and retaining Pending"
            );
            return;
        }
        let tick = remaining.min(POLL_FALLBACK_TICK);
        if subscription_open {
            tokio::select! {
                event = subscription.next() => {
                    if event.is_none() {
                        subscription_open = false;
                    }
                }
                () = tokio::time::sleep(tick) => {}
            }
        } else {
            tokio::time::sleep(tick).await;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const PENDING_ONE: &str = "00000000-0000-4000-8000-000000000001";
    const ACCEPTED_BEFORE_WATCHER: &str = "00000000-0000-4000-8000-000000000002";

    #[derive(Default)]
    struct BoundarySessionService {
        subscription_calls: std::sync::atomic::AtomicUsize,
        history_gate: Option<BoundaryHistoryGate>,
    }

    struct BoundaryHistoryGate {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    fn unused_session_error() -> meerkat_core::service::SessionError {
        meerkat_core::service::SessionError::Unsupported(
            "boundary test session service must not be called".to_string(),
        )
    }

    #[async_trait::async_trait]
    impl meerkat_core::service::SessionService for BoundarySessionService {
        async fn create_session(
            &self,
            _req: meerkat_core::service::CreateSessionRequest,
        ) -> Result<meerkat_core::RunResult, meerkat_core::service::SessionError> {
            Err(unused_session_error())
        }

        async fn start_turn(
            &self,
            _id: &SessionId,
            _req: meerkat_core::service::StartTurnRequest,
        ) -> Result<meerkat_core::RunResult, meerkat_core::service::SessionError> {
            Err(unused_session_error())
        }

        async fn interrupt(
            &self,
            _id: &SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            Err(unused_session_error())
        }

        async fn read(
            &self,
            _id: &SessionId,
        ) -> Result<meerkat_core::service::SessionView, meerkat_core::service::SessionError>
        {
            Err(unused_session_error())
        }

        async fn list(
            &self,
            _query: meerkat_core::service::SessionQuery,
        ) -> Result<Vec<meerkat_core::service::SessionSummary>, meerkat_core::service::SessionError>
        {
            Err(unused_session_error())
        }

        async fn archive(
            &self,
            _id: &SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            Err(unused_session_error())
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::service::SessionServiceCommsExt for BoundarySessionService {}

    #[async_trait::async_trait]
    impl meerkat_core::service::SessionServiceControlExt for BoundarySessionService {
        async fn append_system_context(
            &self,
            _id: &SessionId,
            _req: meerkat_core::service::AppendSystemContextRequest,
        ) -> Result<
            meerkat_core::service::AppendSystemContextResult,
            meerkat_core::service::SessionControlError,
        > {
            Err(unused_session_error().into())
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::service::SessionServiceHistoryExt for BoundarySessionService {
        async fn read_history(
            &self,
            id: &SessionId,
            query: meerkat_core::service::SessionHistoryQuery,
        ) -> Result<meerkat_core::service::SessionHistoryPage, meerkat_core::service::SessionError>
        {
            if let Some(gate) = &self.history_gate {
                gate.entered.notify_one();
                gate.release.notified().await;
                return Ok(meerkat_core::service::SessionHistoryPage::from_messages(
                    id.clone(),
                    &[],
                    query,
                ));
            }
            Err(unused_session_error())
        }
    }

    #[async_trait::async_trait]
    impl MobSessionService for BoundarySessionService {
        async fn create_session_under_runtime_turn_boundary(
            &self,
            req: meerkat_core::service::CreateSessionRequest,
        ) -> Result<meerkat_core::RunResult, meerkat_core::service::SessionError> {
            meerkat_core::service::SessionService::create_session(self, req).await
        }

        async fn archive_with_mob_lifecycle_authority_under_runtime_turn_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            self.archive_with_mob_lifecycle_authority(session_id).await
        }

        async fn discard_live_session_under_runtime_turn_boundary(
            &self,
            session_id: &SessionId,
        ) -> Result<(), meerkat_core::service::SessionError> {
            self.discard_live_session(session_id).await
        }

        async fn subscribe_session_events(
            &self,
            _session_id: &SessionId,
        ) -> Result<meerkat_core::EventStream, meerkat_core::StreamError> {
            self.subscription_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(meerkat_core::StreamError::Internal(
                "boundary test subscribed past sequence exhaustion".to_string(),
            ))
        }
    }

    #[tokio::test]
    async fn observation_task_owner_aborts_and_joins_directed_watchers() {
        struct DropFlag(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let (_projection_tx, projection_rx) = watch::channel(HostObservationProjection::default());
        let (pending_tx, _pending_rx) = mpsc::channel(1);
        let (outcome_ack_tx, _outcome_ack_rx) = mpsc::channel(1);
        let observation = Arc::new(HostMemberObservation::new(
            Arc::new(BoundarySessionService::default()),
            None,
            projection_rx,
            pending_tx,
            outcome_ack_tx,
        ));
        let owner = observation
            .recover_pending_turns()
            .await
            .expect("first composition owns the observation tasks");
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (started_tx, started_rx) = oneshot::channel();
        observation.directed_turn_tasks.lock().await.spawn({
            let dropped = Arc::clone(&dropped);
            async move {
                let _drop_flag = DropFlag(dropped);
                let _ = started_tx.send(());
                std::future::pending::<()>().await;
            }
        });
        started_rx.await.expect("owned watcher started");

        owner.shutdown().await;

        assert!(observation.shutting_down.load(Ordering::Acquire));
        assert!(dropped.load(Ordering::SeqCst));
        assert!(observation.directed_turn_tasks.lock().await.is_empty());
    }

    struct FixedWatermarkLog(u64);

    #[async_trait::async_trait]
    impl DurableEventLogRead for FixedWatermarkLog {
        async fn read_from(
            &self,
            _session: &SessionId,
            _from_seq: u64,
            _max_rows: usize,
        ) -> Result<Option<Vec<(u64, EventEnvelope<AgentEvent>)>>, MemberObservationError> {
            Err(MemberObservationError::Internal {
                reason: "boundary test read past sequence exhaustion".to_string(),
            })
        }

        async fn latest_seq(
            &self,
            _session: &SessionId,
        ) -> Result<Option<u64>, MemberObservationError> {
            Ok(Some(self.0))
        }
    }

    struct BlockingEmptyLog {
        entered: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait::async_trait]
    impl DurableEventLogRead for BlockingEmptyLog {
        async fn read_from(
            &self,
            _session: &SessionId,
            _from_seq: u64,
            _max_rows: usize,
        ) -> Result<Option<Vec<(u64, EventEnvelope<AgentEvent>)>>, MemberObservationError> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(Some(Vec::new()))
        }

        async fn latest_seq(
            &self,
            _session: &SessionId,
        ) -> Result<Option<u64>, MemberObservationError> {
            Ok(Some(0))
        }
    }

    struct RowsLog(Vec<(u64, EventEnvelope<AgentEvent>)>);

    #[async_trait::async_trait]
    impl DurableEventLogRead for RowsLog {
        async fn read_from(
            &self,
            _session: &SessionId,
            from_seq: u64,
            max_rows: usize,
        ) -> Result<Option<Vec<(u64, EventEnvelope<AgentEvent>)>>, MemberObservationError> {
            Ok(Some(
                self.0
                    .iter()
                    .filter(|(sequence, _)| *sequence >= from_seq)
                    .take(max_rows)
                    .cloned()
                    .collect(),
            ))
        }

        async fn latest_seq(
            &self,
            _session: &SessionId,
        ) -> Result<Option<u64>, MemberObservationError> {
            Ok(self.0.last().map(|(sequence, _)| *sequence))
        }
    }

    fn durable_event(
        sequence: u64,
        source: EventSourceIdentity,
        payload: AgentEvent,
    ) -> (u64, EventEnvelope<AgentEvent>) {
        (
            sequence,
            EventEnvelope::new_with_source(source, sequence, None, payload),
        )
    }

    fn incarnation(
        session: &SessionId,
        generation: u64,
        fence_token: u64,
    ) -> meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
        meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
            mob_id: "mob-exhausted".to_string(),
            agent_identity: "worker".to_string(),
            host_id: "host-a".to_string(),
            binding_generation: 1,
            member_session_id: session.to_string(),
            generation,
            fence_token,
        }
    }

    fn run_started(session: &SessionId, content: &str) -> AgentEvent {
        AgentEvent::RunStarted {
            session_id: session.clone(),
            input: meerkat_core::types::RunInput::Content {
                content: meerkat_core::types::ContentInput::Text(content.to_string()),
            },
        }
    }

    fn run_completed(session: &SessionId, output: &str) -> AgentEvent {
        AgentEvent::RunCompleted {
            session_id: session.clone(),
            result: output.to_string(),
            structured_output: None,
            extraction_required: false,
            usage: meerkat_core::types::Usage::default(),
            terminal_cause_kind: None,
        }
    }

    fn interaction_completed(interaction_id: InteractionId, output: &str) -> AgentEvent {
        AgentEvent::InteractionComplete {
            interaction_id,
            result: output.to_string(),
            structured_output: None,
        }
    }

    fn interaction(value: u128) -> InteractionId {
        InteractionId(uuid::Uuid::from_u128(value))
    }

    fn completed_candidate(
        terminal_seq: u64,
        output: &str,
        run_started_in_window: bool,
        run_matches_expected_content: bool,
    ) -> DurableTerminalCandidate {
        DurableTerminalCandidate {
            terminal: meerkat_core::turn_terminal::ClassifiedTurnTerminal {
                kind: meerkat_core::turn_terminal::TurnTerminalKind::RunCompleted,
                outcome: meerkat_core::turn_terminal::TurnTerminalOutcome::Completed {
                    output: output.to_string(),
                    structured_output: None,
                },
            },
            terminal_seq,
            run_started_in_window,
            run_matches_expected_content,
            interaction_id: None,
        }
    }

    fn interaction_completed_candidate(
        terminal_seq: u64,
        output: &str,
        interaction_id: InteractionId,
    ) -> DurableTerminalCandidate {
        DurableTerminalCandidate {
            terminal: meerkat_core::turn_terminal::ClassifiedTurnTerminal {
                kind: meerkat_core::turn_terminal::TurnTerminalKind::InteractionComplete,
                outcome: meerkat_core::turn_terminal::TurnTerminalOutcome::Completed {
                    output: output.to_string(),
                    structured_output: None,
                },
            },
            terminal_seq,
            run_started_in_window: false,
            run_matches_expected_content: false,
            interaction_id: Some(interaction_id),
        }
    }

    fn terminal_candidate(
        terminal_seq: u64,
        terminal: meerkat_core::turn_terminal::ClassifiedTurnTerminal,
        run_matches_expected_content: bool,
        interaction_id: Option<InteractionId>,
    ) -> DurableTerminalCandidate {
        DurableTerminalCandidate {
            terminal,
            terminal_seq,
            run_started_in_window: interaction_id.is_none(),
            run_matches_expected_content,
            interaction_id,
        }
    }

    fn terminal_attribution(boundary_sequence: u64) -> DirectedTurnRuntimeAttribution {
        DirectedTurnRuntimeAttribution {
            runtime_input_id: meerkat_core::lifecycle::InputId::new(),
            admission_sequence: 12,
            last_boundary_sequence: Some(boundary_sequence),
            terminal_outcome: Some(meerkat_runtime::input_state::InputTerminalOutcome::Consumed),
            phase: meerkat_runtime::input_state::InputLifecycleState::Consumed,
            expected_content: meerkat_core::types::ContentInput::Text("current".to_string()),
            tracking_kind: DirectedTurnTrackingKind::FlowStep,
        }
    }

    fn peer_terminal_attribution(boundary_sequence: u64) -> DirectedTurnRuntimeAttribution {
        DirectedTurnRuntimeAttribution {
            tracking_kind: DirectedTurnTrackingKind::PeerInteraction,
            ..terminal_attribution(boundary_sequence)
        }
    }

    fn stored_directed_turn(
        pending_input_id: &str,
    ) -> meerkat_runtime::input_state::StoredInputState {
        let input = meerkat_runtime::mob_adapter::create_tracked_flow_step_input(
            "step-1",
            meerkat_core::types::ContentInput::Text("current".to_string()),
            "run-1",
            None,
            pending_input_id,
        )
        .expect("test directed input id is a UUID");
        let mut stored =
            meerkat_runtime::input_state::StoredInputState::new_accepted(input.id().clone());
        stored.state.idempotency_key = Some(meerkat_runtime::identifiers::IdempotencyKey::new(
            pending_input_id,
        ));
        stored.state.persisted_input = Some(input);
        stored.seed.phase = meerkat_runtime::input_state::InputLifecycleState::Queued;
        stored.seed.admission_sequence = Some(12);
        stored
    }

    fn stored_directed_peer_turn(
        pending_input_id: &str,
    ) -> meerkat_runtime::input_state::StoredInputState {
        let stable =
            uuid::Uuid::parse_str(pending_input_id).expect("test directed peer input id is a UUID");
        let input = meerkat_runtime::input::Input::Peer(meerkat_runtime::input::PeerInput {
            directed_interaction_id: Some(InteractionId(stable)),
            objective_id: Some(meerkat_core::interaction::ObjectiveId::new()),
            injected_context: vec![meerkat_core::types::ContentInput::Text(
                "private ambient context".to_string(),
            )],
            sender_taint: None,
            header: meerkat_runtime::input::InputHeader {
                id: meerkat_core::lifecycle::InputId::from_uuid(stable),
                timestamp: chrono::Utc::now(),
                source: meerkat_runtime::input::InputOrigin::Peer {
                    peer_id: uuid::Uuid::from_u128(7).to_string(),
                    display_identity: Some("supervisor".to_string()),
                    runtime_id: Some(meerkat_runtime::identifiers::LogicalRuntimeId::new(
                        "rt:session:placed",
                    )),
                },
                durability: meerkat_runtime::input::InputDurability::Durable,
                visibility: meerkat_runtime::input::InputVisibility::default(),
                idempotency_key: Some(meerkat_runtime::identifiers::IdempotencyKey::new(
                    pending_input_id,
                )),
                supersession_key: None,
                correlation_id: Some(meerkat_runtime::identifiers::CorrelationId::from_uuid(
                    stable,
                )),
            },
            convention: Some(meerkat_runtime::input::PeerConvention::Message),
            content: meerkat_core::types::ContentInput::Blocks(vec![
                meerkat_core::types::ContentBlock::Text {
                    text: "durable placed kickoff".to_string(),
                },
                meerkat_core::types::ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: meerkat_core::types::ImageData::Inline {
                        data: "abc123".to_string(),
                    },
                },
            ]),
            payload: None,
            handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
        });
        let mut stored =
            meerkat_runtime::input_state::StoredInputState::new_accepted(input.id().clone());
        stored.state.idempotency_key = Some(meerkat_runtime::identifiers::IdempotencyKey::new(
            pending_input_id,
        ));
        stored.state.persisted_input = Some(input);
        stored.seed.phase = meerkat_runtime::input_state::InputLifecycleState::Queued;
        stored.seed.admission_sequence = Some(13);
        stored
    }

    fn outcome(input_id: &str) -> BridgeTurnOutcomeRecord {
        BridgeTurnOutcomeRecord {
            input_id: input_id.to_string(),
            generation: 3,
            fence_token: 5,
            terminal_seq: 7,
            outcome: WireFlowTurnOutcome::RunCompleted,
        }
    }

    fn ack(index: usize) -> BridgeTurnOutcomeAck {
        BridgeTurnOutcomeAck {
            generation: 3,
            fence_token: 5,
            input_id: format!("input-{index}"),
        }
    }

    #[tokio::test]
    async fn poll_ack_is_pinned_to_full_incarnation_and_rejects_post_ack_cutover() {
        let session = SessionId::new();
        let g1 = incarnation(&session, 3, 5);
        let mut g2 = g1.clone();
        g2.binding_generation = 2;
        g2.generation = 4;
        g2.fence_token = 6;
        let facts =
            |incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation| {
                SessionObservationFacts {
                    mob_id: incarnation.mob_id.clone(),
                    agent_identity: incarnation.agent_identity.clone(),
                    generation: incarnation.generation,
                    fence_token: incarnation.fence_token,
                    incarnation,
                    generation_start_seq: 1,
                    pending_turns: Vec::new(),
                    turn_outcomes: Vec::new(),
                }
            };
        let (projection_tx, projection_rx) = watch::channel(HostObservationProjection {
            sessions: BTreeMap::from([(session.to_string(), facts(g1.clone()))]),
        });
        let (pending_tx, _pending_rx) = mpsc::channel(1);
        let (outcome_ack_tx, mut outcome_ack_rx) = mpsc::channel(1);
        let observation = Arc::new(HostMemberObservation::new(
            Arc::new(BoundarySessionService::default()),
            None,
            projection_rx,
            pending_tx,
            outcome_ack_tx,
        ));
        let poll_observation = Arc::clone(&observation);
        let poll_session = session.clone();
        let poll_g1 = g1.clone();
        let poll = tokio::spawn(async move {
            poll_observation
                .poll_events(
                    &poll_session,
                    MemberEventsPollRequest {
                        expected_member: &poll_g1,
                        cursor: MemberObservationCursor::Tail,
                        max: 1,
                        wait: Duration::ZERO,
                        outcome_acks: &[BridgeTurnOutcomeAck {
                            generation: poll_g1.generation,
                            fence_token: poll_g1.fence_token,
                            input_id: "input-g1".to_string(),
                        }],
                        max_outcomes: 1,
                    },
                )
                .await
        });
        let request = tokio::time::timeout(Duration::from_secs(1), outcome_ack_rx.recv())
            .await
            .expect("poll must send exact ACK request")
            .expect("ACK channel remains open");
        assert_eq!(request.expected_member, g1);
        assert_eq!(request.acks.len(), 1);
        projection_tx.send_replace(HostObservationProjection {
            sessions: BTreeMap::from([(session.to_string(), facts(g2))]),
        });
        request
            .reply
            .send(Ok(()))
            .expect("release poll after G2 projection publication");
        let error = poll
            .await
            .expect("poll task must not panic")
            .expect_err("G1 poll cannot serve a G2 page after ACK");
        assert!(matches!(
            error,
            MemberObservationError::StaleIncarnation { .. }
        ));
    }

    #[tokio::test]
    async fn durable_poll_rejects_generation_and_same_generation_fence_cutover_during_wait() {
        for same_generation_fence_rotation in [false, true] {
            let session = SessionId::new();
            let g1 = incarnation(&session, 3, 5);
            let mut replacement = g1.clone();
            replacement.fence_token = 6;
            if !same_generation_fence_rotation {
                replacement.binding_generation = 2;
                replacement.generation = 4;
            }
            let facts = |incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation| {
                SessionObservationFacts {
                    mob_id: incarnation.mob_id.clone(),
                    agent_identity: incarnation.agent_identity.clone(),
                    generation: incarnation.generation,
                    fence_token: incarnation.fence_token,
                    incarnation,
                    generation_start_seq: 1,
                    pending_turns: Vec::new(),
                    turn_outcomes: Vec::new(),
                }
            };
            let (projection_tx, projection_rx) = watch::channel(HostObservationProjection {
                sessions: BTreeMap::from([(session.to_string(), facts(g1.clone()))]),
            });
            let entered = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let log = Arc::new(BlockingEmptyLog {
                entered: Arc::clone(&entered),
                release: Arc::clone(&release),
            });
            let (pending_tx, _pending_rx) = mpsc::channel(1);
            let (outcome_ack_tx, _outcome_ack_rx) = mpsc::channel(1);
            let observation = Arc::new(HostMemberObservation::new(
                Arc::new(BoundarySessionService::default()),
                Some(log),
                projection_rx,
                pending_tx,
                outcome_ack_tx,
            ));
            let poll_observation = Arc::clone(&observation);
            let poll_session = session.clone();
            let poll_g1 = g1.clone();
            let poll = tokio::spawn(async move {
                poll_observation
                    .poll_events(
                        &poll_session,
                        MemberEventsPollRequest {
                            expected_member: &poll_g1,
                            cursor: MemberObservationCursor::Tail,
                            max: 1,
                            wait: Duration::ZERO,
                            outcome_acks: &[],
                            max_outcomes: 1,
                        },
                    )
                    .await
            });

            tokio::time::timeout(Duration::from_secs(1), entered.notified())
                .await
                .expect("poll must enter the durable read");
            projection_tx.send_replace(HostObservationProjection {
                sessions: BTreeMap::from([(session.to_string(), facts(replacement))]),
            });
            release.notify_one();
            let error = poll
                .await
                .expect("poll task must not panic")
                .expect_err("a cutover during poll must reject the old page");
            assert!(
                matches!(error, MemberObservationError::StaleIncarnation { .. }),
                "same_generation_fence_rotation={same_generation_fence_rotation}: {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn history_read_rejects_generation_and_same_generation_fence_cutover_during_wait() {
        for same_generation_fence_rotation in [false, true] {
            let session = SessionId::new();
            let g1 = incarnation(&session, 3, 5);
            let mut replacement = g1.clone();
            replacement.fence_token = 6;
            if !same_generation_fence_rotation {
                replacement.binding_generation = 2;
                replacement.generation = 4;
            }
            let facts = |incarnation: meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation| {
                SessionObservationFacts {
                    mob_id: incarnation.mob_id.clone(),
                    agent_identity: incarnation.agent_identity.clone(),
                    generation: incarnation.generation,
                    fence_token: incarnation.fence_token,
                    incarnation,
                    generation_start_seq: 1,
                    pending_turns: Vec::new(),
                    turn_outcomes: Vec::new(),
                }
            };
            let (projection_tx, projection_rx) = watch::channel(HostObservationProjection {
                sessions: BTreeMap::from([(session.to_string(), facts(g1))]),
            });
            let entered = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let service = Arc::new(BoundarySessionService {
                history_gate: Some(BoundaryHistoryGate {
                    entered: Arc::clone(&entered),
                    release: Arc::clone(&release),
                }),
                ..BoundarySessionService::default()
            });
            let (pending_tx, _pending_rx) = mpsc::channel(1);
            let (outcome_ack_tx, _outcome_ack_rx) = mpsc::channel(1);
            let observation = Arc::new(HostMemberObservation::new(
                service,
                None,
                projection_rx,
                pending_tx,
                outcome_ack_tx,
            ));
            let read_observation = Arc::clone(&observation);
            let read_session = session.clone();
            let read = tokio::spawn(async move {
                read_observation
                    .read_history(&read_session, Some(0), Some(1))
                    .await
            });

            tokio::time::timeout(Duration::from_secs(1), entered.notified())
                .await
                .expect("history read must enter the session service");
            projection_tx.send_replace(HostObservationProjection {
                sessions: BTreeMap::from([(session.to_string(), facts(replacement))]),
            });
            release.notify_one();
            let error = read
                .await
                .expect("history task must not panic")
                .expect_err("a cutover during history read must reject the old page");
            assert!(
                matches!(error, MemberObservationError::StaleIncarnation { .. }),
                "same_generation_fence_rotation={same_generation_fence_rotation}: {error:?}"
            );
        }
    }

    #[test]
    fn outcome_ack_batch_accepts_exact_shared_ceiling_and_rejects_oversize_atomically() {
        let exact: Vec<_> = (0..BRIDGE_TURN_OUTCOME_ACK_MAX).map(ack).collect();
        assert_eq!(
            validate_outcome_ack_batch(&exact)
                .expect("exact shared ACK ceiling is accepted")
                .len(),
            BRIDGE_TURN_OUTCOME_ACK_MAX
        );

        let oversized: Vec<_> = (0..=BRIDGE_TURN_OUTCOME_ACK_MAX).map(ack).collect();
        let error = validate_outcome_ack_batch(&oversized)
            .expect_err("one ACK over the shared ceiling rejects the whole batch");
        assert!(matches!(error, MemberObservationError::Internal { .. }));
    }

    #[test]
    fn exhausted_directed_turn_watermark_rejects_before_pending_reservation() {
        assert_eq!(
            directed_turn_window_start(u64::MAX - 4).unwrap(),
            u64::MAX - 3
        );
        for watermark in [u64::MAX - 3, u64::MAX - 2, u64::MAX - 1, u64::MAX] {
            let error = directed_turn_window_start(watermark).expect_err(
                "an unresumable terminal frontier must reject before subscription, Reserve, or runtime accept",
            );
            assert!(error.detail.contains("before runtime acceptance"));
        }
    }

    #[tokio::test]
    async fn directed_turn_sequence_exhaustion_uses_actor_replay_only_arbitration() {
        for watermark in [u64::MAX - 3, u64::MAX - 2, u64::MAX - 1, u64::MAX] {
            let session = SessionId::new();
            let service = Arc::new(BoundarySessionService::default());
            let (projection_tx, projection_rx) = watch::channel(HostObservationProjection {
                sessions: BTreeMap::from([(
                    session.to_string(),
                    SessionObservationFacts {
                        incarnation: incarnation(&session, 7, 11),
                        mob_id: "mob-exhausted".to_string(),
                        agent_identity: "worker".to_string(),
                        generation: 7,
                        fence_token: 11,
                        generation_start_seq: 1,
                        pending_turns: Vec::new(),
                        turn_outcomes: Vec::new(),
                    },
                )]),
            });
            let (pending_tx, mut pending_rx) = mpsc::channel(1);
            let (outcome_ack_tx, _outcome_ack_rx) = mpsc::channel(1);
            let observation = HostMemberObservation::new(
                Arc::clone(&service) as Arc<dyn MobSessionService>,
                Some(Arc::new(FixedWatermarkLog(watermark))),
                projection_rx,
                pending_tx,
                outcome_ack_tx,
            );

            let expected_member = incarnation(&session, 7, 11);
            let open = observation.open_directed_turn_window(
                &session,
                &expected_member,
                "input-exhausted",
            );
            let arbitrate = async {
                let request = pending_rx
                    .recv()
                    .await
                    .expect("exhaustion must ask the actor for replay-only arbitration");
                let HostTurnOutcomePendingRequest::Reserve {
                    fresh_window_start,
                    input_id,
                    reply,
                    ..
                } = request
                else {
                    panic!("exhaustion must issue Reserve arbitration")
                };
                assert_eq!(fresh_window_start, None);
                assert_eq!(input_id, "input-exhausted");
                reply
                    .send(Ok(HostPendingReservationReply::FreshRequired))
                    .expect("observation awaits arbitration reply");
            };
            let (result, ()) = tokio::join!(open, arbitrate);
            let error = result.expect_err("fresh exhausted key must reject before Pending");
            assert!(error.detail.contains("before runtime acceptance"));
            assert_eq!(
                service
                    .subscription_calls
                    .load(std::sync::atomic::Ordering::SeqCst),
                0,
                "subscription is downstream of the sequence-capacity check"
            );
            assert!(matches!(
                pending_rx.try_recv(),
                Err(mpsc::error::TryRecvError::Empty)
            ));
            drop(projection_tx);
        }
    }

    #[tokio::test]
    async fn pending_replay_precedes_log_and_subscription_preflight_and_cannot_be_cancelled() {
        let session = SessionId::new();
        let expected_member = incarnation(&session, 7, 11);
        let service = Arc::new(BoundarySessionService::default());
        let (_projection_tx, projection_rx) = watch::channel(HostObservationProjection {
            sessions: BTreeMap::from([(
                session.to_string(),
                SessionObservationFacts {
                    incarnation: expected_member.clone(),
                    mob_id: expected_member.mob_id.clone(),
                    agent_identity: expected_member.agent_identity.clone(),
                    generation: 7,
                    fence_token: 11,
                    generation_start_seq: 1,
                    pending_turns: vec![PendingTurnObservation {
                        input_id: "existing-pending".to_string(),
                        generation: 7,
                        fence_token: 11,
                        window_start: 42,
                    }],
                    turn_outcomes: Vec::new(),
                },
            )]),
        });
        let (pending_tx, mut pending_rx) = mpsc::channel(2);
        let (outcome_ack_tx, _outcome_ack_rx) = mpsc::channel(1);
        let observation = HostMemberObservation::new(
            Arc::clone(&service) as Arc<dyn MobSessionService>,
            // A replay must not depend on fresh durable-log preflight.
            None,
            projection_rx,
            pending_tx,
            outcome_ack_tx,
        );
        let open =
            observation.open_directed_turn_window(&session, &expected_member, "existing-pending");
        let arbitrate = async {
            let HostTurnOutcomePendingRequest::Reserve {
                fresh_window_start,
                reply,
                ..
            } = pending_rx.recv().await.expect("replay probe")
            else {
                panic!("expected replay probe")
            };
            assert_eq!(fresh_window_start, None);
            reply
                .send(Ok(HostPendingReservationReply::Replayed {
                    window_start: 42,
                }))
                .expect("open awaits replay result");
        };
        let (window, ()) = tokio::join!(open, arbitrate);
        let window = window.expect("existing Pending survives missing log/subscription");
        assert_eq!(window.tracking, DirectedTurnTracking::PendingReplay);
        assert_eq!(window.window_start, 42);
        assert_eq!(
            service
                .subscription_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "replay attempts a wake stream but falls back to durable polling"
        );
        let error = observation
            .cancel_directed_turn_window(&session, window)
            .await
            .expect_err("a retry cannot certify its original Pending as no-effect");
        assert!(error.detail.contains("original delivery had no effect"));
        assert!(matches!(
            pending_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn exact_key_admission_permit_serializes_revalidation_with_cancellation() {
        use meerkat_runtime::member_observation::DirectedTurnAdmissionDecision;

        let session = SessionId::new();
        let expected_member = incarnation(&session, 7, 11);
        let service = Arc::new(BoundarySessionService::default());
        let (_projection_tx, projection_rx) = watch::channel(HostObservationProjection {
            sessions: BTreeMap::from([(
                session.to_string(),
                SessionObservationFacts {
                    incarnation: expected_member.clone(),
                    mob_id: expected_member.mob_id.clone(),
                    agent_identity: expected_member.agent_identity.clone(),
                    generation: 7,
                    fence_token: 11,
                    generation_start_seq: 1,
                    pending_turns: vec![PendingTurnObservation {
                        input_id: "serialized-input".to_string(),
                        generation: 7,
                        fence_token: 11,
                        window_start: 42,
                    }],
                    turn_outcomes: Vec::new(),
                },
            )]),
        });
        let (pending_tx, mut pending_rx) = mpsc::channel(2);
        let (outcome_ack_tx, _outcome_ack_rx) = mpsc::channel(1);
        let observation =
            HostMemberObservation::new(service, None, projection_rx, pending_tx, outcome_ack_tx);
        let window = DirectedTurnWindow {
            expected_member: expected_member.clone(),
            subscription: Box::pin(futures::stream::empty()),
            window_start: 42,
            generation: 7,
            fence_token: 11,
            input_id: "serialized-input".to_string(),
            tracking: DirectedTurnTracking::PendingReplay,
        };

        let first = observation
            .lock_and_revalidate_directed_turn_admission(&session, window.admission_request());
        let first_actor = async {
            let HostTurnOutcomePendingRequest::Reserve {
                fresh_window_start,
                reply,
                ..
            } = pending_rx.recv().await.expect("first revalidation probe")
            else {
                panic!("expected reserve probe")
            };
            assert_eq!(fresh_window_start, None);
            reply
                .send(Ok(HostPendingReservationReply::Replayed {
                    window_start: 42,
                }))
                .expect("first admission awaits actor");
        };
        let (first, ()) = tokio::join!(first, first_actor);
        let permit = match first.expect("first admission revalidates") {
            DirectedTurnAdmissionDecision::Admit(permit) => permit,
            DirectedTurnAdmissionDecision::TerminalReplay => {
                panic!("Pending unexpectedly classified terminal")
            }
        };

        let second = observation
            .lock_and_revalidate_directed_turn_admission(&session, window.admission_request());
        tokio::pin!(second);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut second)
                .await
                .is_err(),
            "same-key revalidation must wait while runtime admission owns the permit"
        );
        assert!(matches!(
            pending_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));

        drop(permit);
        let second_actor = async {
            let HostTurnOutcomePendingRequest::Reserve { reply, .. } =
                pending_rx.recv().await.expect("second revalidation probe")
            else {
                panic!("expected reserve probe")
            };
            reply
                .send(Ok(HostPendingReservationReply::TerminalReplay))
                .expect("second admission awaits actor");
        };
        let (second, ()) = tokio::join!(second, second_actor);
        assert!(matches!(
            second.expect("second revalidation completes"),
            DirectedTurnAdmissionDecision::TerminalReplay
        ));
    }

    #[tokio::test]
    async fn fresh_pending_uses_durable_polling_when_wake_subscription_fails() {
        let session = SessionId::new();
        let expected_member = incarnation(&session, 7, 11);
        let service = Arc::new(BoundarySessionService::default());
        let (_projection_tx, projection_rx) = watch::channel(HostObservationProjection {
            sessions: BTreeMap::from([(
                session.to_string(),
                SessionObservationFacts {
                    incarnation: expected_member.clone(),
                    mob_id: expected_member.mob_id.clone(),
                    agent_identity: expected_member.agent_identity.clone(),
                    generation: 7,
                    fence_token: 11,
                    generation_start_seq: 1,
                    pending_turns: Vec::new(),
                    turn_outcomes: Vec::new(),
                },
            )]),
        });
        let (pending_tx, mut pending_rx) = mpsc::channel(2);
        let (outcome_ack_tx, _outcome_ack_rx) = mpsc::channel(1);
        let observation = HostMemberObservation::new(
            Arc::clone(&service) as Arc<dyn MobSessionService>,
            Some(Arc::new(FixedWatermarkLog(0))),
            projection_rx,
            pending_tx,
            outcome_ack_tx,
        );
        let open =
            observation.open_directed_turn_window(&session, &expected_member, "fresh-pending");
        let arbitrate = async {
            let HostTurnOutcomePendingRequest::Reserve {
                fresh_window_start,
                reply,
                ..
            } = pending_rx.recv().await.expect("fresh probe")
            else {
                panic!("expected fresh probe")
            };
            assert_eq!(fresh_window_start, None);
            reply
                .send(Ok(HostPendingReservationReply::FreshRequired))
                .expect("open awaits fresh probe");
            let HostTurnOutcomePendingRequest::Reserve {
                fresh_window_start,
                reply,
                ..
            } = pending_rx.recv().await.expect("fresh reservation")
            else {
                panic!("expected fresh reservation")
            };
            assert_eq!(fresh_window_start, Some(1));
            reply
                .send(Ok(HostPendingReservationReply::Reserved {
                    window_start: 1,
                }))
                .expect("open awaits fresh reservation");
        };
        let (window, ()) = tokio::join!(open, arbitrate);
        let window = window.expect("durable polling substitutes for wake subscription");
        assert_eq!(window.tracking, DirectedTurnTracking::PendingFresh);
        assert_eq!(window.window_start, 1);
        let cancel = observation.cancel_directed_turn_window(&session, window);
        let certify = async {
            let HostTurnOutcomePendingRequest::Cancel {
                input_id, reply, ..
            } = pending_rx
                .recv()
                .await
                .expect("fresh Pending cancellation request")
            else {
                panic!("fresh Pending must use actor cancellation")
            };
            assert_eq!(input_id, "fresh-pending");
            reply
                .send(Ok(()))
                .expect("fresh cancellation awaits actor certificate");
        };
        let (canceled, ()) = tokio::join!(cancel, certify);
        canceled.expect("fresh request may certify definite no-effect");
    }

    #[test]
    fn ephemeral_ring_exhaustion_is_explicit_and_does_not_wrap_or_append() {
        let mut state = RingState {
            generation: 7,
            capacity: MEMBER_EVENT_RING_CAPACITY,
            next_seq: u64::MAX,
            oldest_retained: u64::MAX,
            exhausted: false,
            rows: VecDeque::new(),
        };
        let envelope = EventEnvelope::new(
            "worker",
            1,
            Some("mob-exhausted".to_string()),
            AgentEvent::TurnStarted { turn_number: 1 },
        );
        let error = state
            .push(envelope)
            .expect_err("ring must fail-stop before assigning an unresumable MAX row");
        assert!(matches!(error, MemberObservationError::Internal { .. }));
        assert!(state.exhausted);
        assert_eq!(state.next_seq, u64::MAX);
        assert!(state.rows.is_empty());
    }

    #[test]
    fn ephemeral_ring_honors_its_composed_capacity() {
        let mut state = RingState {
            generation: 7,
            capacity: 2,
            next_seq: 1,
            oldest_retained: 1,
            exhausted: false,
            rows: VecDeque::new(),
        };
        for turn_number in 1_u32..=3 {
            state
                .push(EventEnvelope::new(
                    "worker",
                    u64::from(turn_number),
                    Some("mob-capacity".to_string()),
                    AgentEvent::TurnStarted { turn_number },
                ))
                .expect("bounded ring push");
        }
        assert_eq!(state.rows.len(), 2);
        assert_eq!(state.rows.front().map(|(seq, _)| *seq), Some(2));
        assert_eq!(state.rows.back().map(|(seq, _)| *seq), Some(3));
        assert_eq!(state.oldest_retained, 2);
        assert_eq!(state.next_seq, 4);
    }

    #[test]
    fn previous_same_kind_terminal_between_window_and_accept_cannot_steal_current_turn() {
        let interaction_id = interaction(1);
        let scan = DurableTerminalScan {
            terminals: vec![
                completed_candidate(41, "previous", false, false),
                completed_candidate(47, "current-result", true, true),
                interaction_completed_candidate(51, "current-result", interaction_id),
            ],
            matching_run_starts: 1,
            watermark: 51,
        };
        let expectation = CompletionTerminalExpectation::Completed {
            output: "current-result".to_string(),
            structured_output: None,
        };
        let selected = select_completion_terminal(
            &scan,
            &expectation,
            &terminal_attribution(0),
            interaction_id,
        );
        assert!(matches!(
            selected,
            TerminalSelection::Selected(candidate) if candidate.terminal_seq == 47
        ));
    }

    #[test]
    fn content_bracketed_run_terminal_precedes_its_exact_interaction_receipt() {
        let interaction_id = interaction(7);
        let scan = DurableTerminalScan {
            terminals: vec![
                completed_candidate(47, "current-result", true, true),
                interaction_completed_candidate(51, "current-result", interaction_id),
            ],
            matching_run_starts: 1,
            watermark: 51,
        };
        let expectation = CompletionTerminalExpectation::Completed {
            output: "current-result".to_string(),
            structured_output: None,
        };

        let selected = select_completion_terminal(
            &scan,
            &expectation,
            &terminal_attribution(0),
            interaction_id,
        );
        assert!(matches!(
            selected,
            TerminalSelection::Selected(candidate) if candidate.terminal_seq == 47
        ));

        let recovered = select_recovered_terminal(&scan, &terminal_attribution(0), interaction_id);
        assert!(matches!(
            recovered,
            TerminalSelection::Selected(candidate) if candidate.terminal_seq == 47
        ));
    }

    #[test]
    fn tracked_peer_always_selects_and_journals_the_exact_interaction_family() {
        use meerkat_core::turn_terminal::{
            ClassifiedTurnTerminal, TurnTerminalKind as K, TurnTerminalOutcome as O,
        };

        let interaction_id = interaction(17);
        let cases = vec![
            (
                "success",
                CompletionTerminalExpectation::Completed {
                    output: "done".to_string(),
                    structured_output: None,
                },
                K::RunCompleted,
                ClassifiedTurnTerminal {
                    kind: K::InteractionComplete,
                    outcome: O::Completed {
                        output: "done".to_string(),
                        structured_output: None,
                    },
                },
                WireFlowTurnOutcome::InteractionComplete,
            ),
            (
                "callback",
                CompletionTerminalExpectation::Failed {
                    kind: Some(K::InteractionCallbackPending),
                    reason: "callback pending for tool 'ask': {\"q\":1}".to_string(),
                },
                K::InteractionCallbackPending,
                ClassifiedTurnTerminal {
                    kind: K::InteractionCallbackPending,
                    outcome: O::Failed {
                        reason: "callback pending for tool 'ask': {\"q\":1}".to_string(),
                    },
                },
                WireFlowTurnOutcome::InteractionCallbackPending,
            ),
            (
                "cancelled",
                CompletionTerminalExpectation::Failed {
                    kind: None,
                    reason: "cancelled".to_string(),
                },
                K::RunFailed,
                ClassifiedTurnTerminal {
                    kind: K::InteractionFailed,
                    outcome: O::Failed {
                        reason: "cancelled".to_string(),
                    },
                },
                WireFlowTurnOutcome::InteractionFailed {
                    detail: WireFlowFailureDetail::complete("cancelled".to_string()),
                },
            ),
            (
                "failed",
                CompletionTerminalExpectation::Failed {
                    kind: Some(K::InteractionFailed),
                    reason: "provider failed".to_string(),
                },
                K::InteractionFailed,
                ClassifiedTurnTerminal {
                    kind: K::InteractionFailed,
                    outcome: O::Failed {
                        reason: "provider failed".to_string(),
                    },
                },
                WireFlowTurnOutcome::InteractionFailed {
                    detail: WireFlowFailureDetail::complete("provider failed".to_string()),
                },
            ),
            (
                "extraction failed",
                CompletionTerminalExpectation::Failed {
                    kind: Some(K::ExtractionFailed),
                    reason: "structured output extraction failed after 2 attempt(s): invalid; last_output=\"raw\"".to_string(),
                },
                K::ExtractionFailed,
                ClassifiedTurnTerminal {
                    // The exact payload is InteractionFailed, but the shared
                    // classifier intentionally preserves this typed kind.
                    kind: K::ExtractionFailed,
                    outcome: O::Failed {
                        reason: "structured output extraction failed after 2 attempt(s): invalid; last_output=\"raw\"".to_string(),
                    },
                },
                WireFlowTurnOutcome::InteractionFailed {
                    detail: WireFlowFailureDetail::complete(
                        "structured output extraction failed after 2 attempt(s): invalid; last_output=\"raw\""
                            .to_string(),
                    ),
                },
            ),
        ];

        for (label, expectation, shadow_kind, exact_terminal, expected_wire) in cases {
            let shadow = ClassifiedTurnTerminal {
                kind: shadow_kind,
                outcome: exact_terminal.outcome.clone(),
            };
            let scan = DurableTerminalScan {
                terminals: vec![
                    terminal_candidate(47, shadow, true, None),
                    terminal_candidate(51, exact_terminal.clone(), false, Some(interaction_id)),
                ],
                matching_run_starts: 1,
                watermark: 51,
            };
            let attribution = peer_terminal_attribution(0);
            for selected in [
                select_completion_terminal(&scan, &expectation, &attribution, interaction_id),
                select_recovered_terminal(&scan, &attribution, interaction_id),
            ] {
                let TerminalSelection::Selected(candidate) = selected else {
                    panic!("{label}: exact Peer interaction was not selected")
                };
                assert_eq!(candidate.terminal_seq, 51, "{label}");
                assert_eq!(
                    wire_outcome_from_terminal(
                        attribution.tracking_kind,
                        candidate.terminal.kind,
                        &candidate.terminal.outcome,
                    )
                    .expect("exact Peer terminal is in the interaction family"),
                    expected_wire,
                    "{label}"
                );
            }
        }
    }

    #[test]
    fn flow_step_retains_run_and_extraction_terminal_selection() {
        use meerkat_core::turn_terminal::{
            ClassifiedTurnTerminal, TurnTerminalKind as K, TurnTerminalOutcome as O,
        };
        let interaction_id = interaction(23);
        let reason =
            "structured output extraction failed after 1 attempt(s): invalid; last_output=\"raw\"";
        let terminal = ClassifiedTurnTerminal {
            kind: K::ExtractionFailed,
            outcome: O::Failed {
                reason: reason.to_string(),
            },
        };
        let scan = DurableTerminalScan {
            terminals: vec![
                terminal_candidate(47, terminal.clone(), true, None),
                terminal_candidate(51, terminal, false, Some(interaction_id)),
            ],
            matching_run_starts: 1,
            watermark: 51,
        };
        let attribution = terminal_attribution(0);
        let expectation = CompletionTerminalExpectation::Failed {
            kind: Some(K::ExtractionFailed),
            reason: reason.to_string(),
        };
        for selected in [
            select_completion_terminal(&scan, &expectation, &attribution, interaction_id),
            select_recovered_terminal(&scan, &attribution, interaction_id),
        ] {
            let TerminalSelection::Selected(candidate) = selected else {
                panic!("FlowStep extraction terminal was not selected")
            };
            assert_eq!(candidate.terminal_seq, 47);
            assert!(matches!(
                wire_outcome_from_terminal(
                    attribution.tracking_kind,
                    candidate.terminal.kind,
                    &candidate.terminal.outcome,
                ),
                Ok(WireFlowTurnOutcome::ExtractionFailed { .. })
            ));
        }
    }

    #[test]
    fn completion_extraction_expectation_uses_typed_last_output() {
        let expectation = CompletionTerminalExpectation::from_completion(
            meerkat_runtime::completion::CompletionOutcome::Completed(Box::new(
                meerkat_core::RunResult {
                    // Deliberately differ: the canonical interaction terminal
                    // is built from ExtractionError::last_output.
                    text: "noncanonical result text".to_string(),
                    session_id: SessionId::new(),
                    usage: meerkat_core::Usage::default(),
                    turns: 1,
                    tool_calls: 0,
                    terminal_cause_kind: None,
                    structured_output: None,
                    extraction_error: Some(meerkat_core::types::ExtractionError {
                        last_output: "committed raw output".to_string(),
                        attempts: 2,
                        reason: "invalid".to_string(),
                    }),
                    schema_warnings: None,
                    skill_diagnostics: None,
                },
            )),
        );
        let CompletionTerminalExpectation::Failed { kind, reason } = expectation else {
            panic!("extraction failure must produce a failed expectation")
        };
        assert_eq!(
            kind,
            Some(meerkat_core::turn_terminal::TurnTerminalKind::ExtractionFailed)
        );
        assert!(reason.contains("last_output=\"committed raw output\""));
        assert!(!reason.contains("noncanonical result text"));
    }

    #[tokio::test]
    async fn exact_receipt_anchor_ignores_later_identical_content_run_start() {
        let session = SessionId::new();
        let interaction_a = interaction(7);
        let log: Arc<dyn DurableEventLogRead> = Arc::new(RowsLog(vec![
            durable_event(
                41,
                EventSourceIdentity::session(session.clone()),
                run_started(&session, "current"),
            ),
            durable_event(
                47,
                EventSourceIdentity::session(session.clone()),
                run_completed(&session, "same-result"),
            ),
            durable_event(
                51,
                EventSourceIdentity::interaction(interaction_a),
                interaction_completed(interaction_a, "same-result"),
            ),
            // A later payload-identical B run has started while A's watcher is
            // still folding its immutable window, but B has not finished.
            durable_event(
                55,
                EventSourceIdentity::session(session.clone()),
                run_started(&session, "current"),
            ),
        ]));
        let scan = scan_durable_terminals(
            &session,
            41,
            &meerkat_core::types::ContentInput::Text("current".to_string()),
            &log,
        )
        .await
        .expect("scan");
        assert_eq!(scan.matching_run_starts, 2);

        let expectation = CompletionTerminalExpectation::Completed {
            output: "same-result".to_string(),
            structured_output: None,
        };
        assert!(matches!(
            select_completion_terminal(
                &scan,
                &expectation,
                &terminal_attribution(0),
                interaction_a,
            ),
            TerminalSelection::Selected(candidate) if candidate.terminal_seq == 47
        ));
        assert!(matches!(
            select_recovered_terminal(&scan, &terminal_attribution(0), interaction_a),
            TerminalSelection::Selected(candidate) if candidate.terminal_seq == 47
        ));
    }

    #[tokio::test]
    async fn exact_receipt_anchor_ignores_later_identical_content_run_finish() {
        let session = SessionId::new();
        let interaction_a = interaction(7);
        let interaction_b = interaction(8);
        let log: Arc<dyn DurableEventLogRead> = Arc::new(RowsLog(vec![
            durable_event(
                41,
                EventSourceIdentity::session(session.clone()),
                run_started(&session, "current"),
            ),
            durable_event(
                47,
                EventSourceIdentity::session(session.clone()),
                run_completed(&session, "same-result"),
            ),
            // B starts before A's exact receipt is projected. The receipt must
            // not reset B's independent run bracket.
            durable_event(
                49,
                EventSourceIdentity::session(session.clone()),
                run_started(&session, "current"),
            ),
            durable_event(
                51,
                EventSourceIdentity::interaction(interaction_a),
                interaction_completed(interaction_a, "same-result"),
            ),
            durable_event(
                58,
                EventSourceIdentity::session(session.clone()),
                run_completed(&session, "same-result"),
            ),
            durable_event(
                62,
                EventSourceIdentity::interaction(interaction_b),
                interaction_completed(interaction_b, "same-result"),
            ),
        ]));
        let scan = scan_durable_terminals(
            &session,
            41,
            &meerkat_core::types::ContentInput::Text("current".to_string()),
            &log,
        )
        .await
        .expect("scan");
        assert_eq!(scan.matching_run_starts, 2);
        assert!(
            scan.terminals
                .iter()
                .any(|candidate| candidate.terminal_seq == 58
                    && candidate.run_matches_expected_content),
            "A's exact receipt must not consume B's active run bracket"
        );

        let expectation = CompletionTerminalExpectation::Completed {
            output: "same-result".to_string(),
            structured_output: None,
        };
        for selected in [
            select_completion_terminal(
                &scan,
                &expectation,
                &terminal_attribution(0),
                interaction_a,
            ),
            select_recovered_terminal(&scan, &terminal_attribution(0), interaction_a),
        ] {
            assert!(matches!(
                selected,
                TerminalSelection::Selected(candidate) if candidate.terminal_seq == 47
            ));
        }

        // The same immutable scan also attributes B to its own nearest
        // preceding run terminal, proving the interleaving did not merely hide
        // B in order to make A pass.
        for selected in [
            select_completion_terminal(
                &scan,
                &expectation,
                &terminal_attribution(0),
                interaction_b,
            ),
            select_recovered_terminal(&scan, &terminal_attribution(0), interaction_b),
        ] {
            assert!(matches!(
                selected,
                TerminalSelection::Selected(candidate) if candidate.terminal_seq == 58
            ));
        }
    }

    #[test]
    fn response_delivered_restart_recovery_ignores_unrelated_intervening_runs() {
        let interaction_id = interaction(1);
        let scan = DurableTerminalScan {
            terminals: vec![
                completed_candidate(42, "previous-tail", false, false),
                completed_candidate(45, "unrelated-before", true, false),
                completed_candidate(51, "current-result", true, true),
                interaction_completed_candidate(54, "current-result", interaction_id),
                completed_candidate(58, "unrelated-after", true, false),
            ],
            matching_run_starts: 1,
            watermark: 58,
        };
        let selected = select_recovered_terminal(&scan, &terminal_attribution(0), interaction_id);
        assert!(matches!(
            selected,
            TerminalSelection::Selected(candidate) if candidate.terminal_seq == 51
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn projection_delay_beyond_two_seconds_never_becomes_channel_closed() {
        let attribution = terminal_attribution(0);
        let expectation = CompletionTerminalExpectation::Completed {
            output: "current-result".to_string(),
            structured_output: None,
        };
        let mut scan = DurableTerminalScan {
            terminals: Vec::new(),
            matching_run_starts: 0,
            watermark: 40,
        };
        assert!(matches!(
            select_completion_terminal(&scan, &expectation, &attribution, interaction(1)),
            TerminalSelection::AwaitMore
        ));
        tokio::time::advance(Duration::from_secs(3)).await;
        assert!(
            POST_COMPLETION_TERMINAL_BIND_TIMEOUT > Duration::from_secs(3),
            "the old two-second grace must not be a terminal classification"
        );
        assert!(matches!(
            select_completion_terminal(&scan, &expectation, &attribution, interaction(1)),
            TerminalSelection::AwaitMore
        ));

        scan.terminals
            .push(completed_candidate(41, "current-result", true, true));
        scan.matching_run_starts = 1;
        scan.watermark = 41;
        assert!(matches!(
            select_completion_terminal(&scan, &expectation, &attribution, interaction(1)),
            TerminalSelection::AwaitMore
        ));
        assert!(matches!(
            select_recovered_terminal(&scan, &attribution, interaction(1)),
            TerminalSelection::AwaitMore
        ));

        scan.terminals.push(interaction_completed_candidate(
            42,
            "current-result",
            interaction(1),
        ));
        scan.watermark = 42;
        assert!(matches!(
            select_completion_terminal(&scan, &expectation, &attribution, interaction(1)),
            TerminalSelection::Selected(candidate) if candidate.terminal_seq == 41
        ));
    }

    #[test]
    fn persisted_input_and_idempotency_mismatches_fail_closed() {
        let valid = stored_directed_turn(PENDING_ONE);
        assert!(DirectedTurnRuntimeAttribution::from_stored(PENDING_ONE, &valid).is_ok());

        let mut shell_mismatch = valid.clone();
        shell_mismatch.state.idempotency_key = Some(
            meerkat_runtime::identifiers::IdempotencyKey::new("different-shell-key"),
        );
        assert!(
            DirectedTurnRuntimeAttribution::from_stored(PENDING_ONE, &shell_mismatch)
                .expect_err("shell mismatch must fail closed")
                .contains("runtime idempotency binding")
        );

        let mut persisted_mismatch = valid.clone();
        if let Some(meerkat_runtime::input::Input::FlowStep(flow_step)) =
            persisted_mismatch.state.persisted_input.as_mut()
        {
            flow_step.header.idempotency_key = Some(
                meerkat_runtime::identifiers::IdempotencyKey::new("different-persisted-key"),
            );
        }
        assert!(
            DirectedTurnRuntimeAttribution::from_stored(PENDING_ONE, &persisted_mismatch)
                .expect_err("persisted-input mismatch must fail closed")
                .contains("persisted input key")
        );

        let mut runtime_id_mismatch = valid;
        runtime_id_mismatch.state.input_id = meerkat_core::lifecycle::InputId::new();
        assert!(
            DirectedTurnRuntimeAttribution::from_stored(PENDING_ONE, &runtime_id_mismatch)
                .expect_err("runtime input-id mismatch must fail closed")
                .contains("persisted runtime input id")
        );
    }

    #[test]
    fn crash_after_accept_before_watcher_retains_replayable_runtime_attribution() {
        let stored = stored_directed_turn(ACCEPTED_BEFORE_WATCHER);
        let attribution =
            DirectedTurnRuntimeAttribution::from_stored(ACCEPTED_BEFORE_WATCHER, &stored)
                .expect("accepted nonterminal input is restart-replayable");
        assert!(!attribution.is_terminal());
        assert_eq!(attribution.admission_sequence, 12);
        assert_eq!(
            attribution.expected_content,
            meerkat_core::types::ContentInput::Text("Flow step step-1\ncurrent".to_string())
        );

        let mut missing_payload = stored;
        missing_payload.state.persisted_input = None;
        assert!(
            DirectedTurnRuntimeAttribution::from_stored(ACCEPTED_BEFORE_WATCHER, &missing_payload,)
                .expect_err("accepted recovery without persisted input must fail closed")
                .contains("persisted replay payload")
        );
    }

    #[test]
    fn tracked_peer_live_attach_and_pending_recovery_share_exact_attribution() {
        let stored = stored_directed_peer_turn(ACCEPTED_BEFORE_WATCHER);
        let persisted = stored
            .state
            .persisted_input
            .as_ref()
            .expect("tracked peer remains replayable");
        let expected = meerkat_runtime::input::directed_input_run_started_content(persisted)
            .expect("tracked peer has a canonical RunStarted projection");

        // `admit_directed_turn` and the cold Pending reconciler both enter
        // through this constructor. A peer accepted before watcher attachment
        // must therefore retain exactly the same multimodal/context identity
        // on live attach and after restart.
        let live = DirectedTurnRuntimeAttribution::from_stored(ACCEPTED_BEFORE_WATCHER, &stored)
            .expect("live tracked-peer watcher attachment is authorized");
        let recovered =
            DirectedTurnRuntimeAttribution::from_stored(ACCEPTED_BEFORE_WATCHER, &stored)
                .expect("Pending tracked-peer recovery is authorized");
        assert!(live.same_binding(&recovered));
        assert_eq!(
            live.tracking_kind,
            DirectedTurnTrackingKind::PeerInteraction
        );
        assert_eq!(live.expected_content, expected);
        assert_eq!(live.admission_sequence, 13);
        assert!(
            live.expected_content
                .text_content()
                .contains("private ambient context")
        );
        assert!(
            live.expected_content
                .text_content()
                .contains("durable placed kickoff")
        );
        assert!(
            matches!(live.expected_content, meerkat_core::types::ContentInput::Blocks(ref blocks) if blocks.iter().any(|block| matches!(block, meerkat_core::types::ContentBlock::Image { .. })))
        );

        let mut untracked = stored;
        if let Some(meerkat_runtime::input::Input::Peer(peer)) =
            untracked.state.persisted_input.as_mut()
        {
            peer.directed_interaction_id = None;
        }
        assert!(
            DirectedTurnRuntimeAttribution::from_stored(ACCEPTED_BEFORE_WATCHER, &untracked)
                .expect_err("ordinary peer input cannot acquire host terminal custody")
                .contains("does not carry directed interaction custody")
        );
    }

    #[test]
    fn active_watcher_dedup_is_exact_to_generation_and_fence() {
        let active = Arc::new(StdMutex::new(HashSet::new()));
        let session = SessionId::new();
        let key = ActiveTurnWatcherKey {
            session_id: session.clone(),
            generation: 7,
            fence_token: 10,
            input_id: "pending-1".to_string(),
        };
        let first = ActiveTurnWatcherClaim::try_acquire(&active, key.clone())
            .expect("first exact watcher claims");
        assert!(ActiveTurnWatcherClaim::try_acquire(&active, key.clone()).is_none());
        let newer_fence = ActiveTurnWatcherClaim::try_acquire(
            &active,
            ActiveTurnWatcherKey {
                fence_token: 11,
                ..key.clone()
            },
        )
        .expect("same generation at a new fence is a distinct watcher key");
        drop(first);
        assert!(ActiveTurnWatcherClaim::try_acquire(&active, key).is_some());
        drop(newer_fence);
    }

    #[tokio::test(start_paused = true)]
    async fn transient_pending_recovery_failure_retries_without_host_restart() {
        let (_projection_tx, projection_rx) = watch::channel(HostObservationProjection::default());
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (reattached_tx, reattached_rx) = oneshot::channel();
        let reattached_tx = Arc::new(StdMutex::new(Some(reattached_tx)));
        let task_attempts = Arc::clone(&attempts);
        let task_tx = Arc::clone(&reattached_tx);
        let task = tokio::spawn(run_pending_recovery_reconciler(projection_rx, move || {
            let attempts = Arc::clone(&task_attempts);
            let reattached_tx = Arc::clone(&task_tx);
            async move {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                if attempt == 1
                    && let Some(tx) = reattached_tx
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                {
                    let _ = tx.send(());
                }
                // Pending remains projected on both the transient failure
                // and successful watcher reattachment passes.
                1
            }
        }));
        tokio::task::yield_now().await;
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        tokio::time::advance(PENDING_RECOVERY_RETRY_MIN).await;
        tokio::task::yield_now().await;
        reattached_rx
            .await
            .expect("second pass reattaches without a host restart");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        task.abort();
    }

    #[test]
    fn outcome_page_is_independently_bounded_and_ordered() {
        let facts = SessionObservationFacts {
            incarnation: incarnation(&SessionId::new(), 3, 5),
            mob_id: "mob-page".to_string(),
            agent_identity: "worker".to_string(),
            generation: 3,
            fence_token: 5,
            generation_start_seq: 11,
            pending_turns: Vec::new(),
            turn_outcomes: vec![outcome("a"), outcome("b"), outcome("c")],
        };
        let (first, complete) = HostMemberObservation::outcome_page(&facts, 2, u64::MAX);
        assert_eq!(
            first
                .iter()
                .map(|record| record.input_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(!complete);

        let remaining = SessionObservationFacts {
            turn_outcomes: vec![facts.turn_outcomes[2].clone()],
            ..facts
        };
        let (second, complete) = HostMemberObservation::outcome_page(&remaining, 2, u64::MAX);
        assert_eq!(second[0].input_id, "c");
        assert!(complete);
    }

    #[test]
    fn resumed_generation_never_reads_before_its_recorded_floor() {
        assert_eq!(
            HostMemberObservation::resolve_cursor_floor(
                MemberObservationCursor::At {
                    generation: 4,
                    seq: 1,
                },
                5,
                101,
                140,
            )
            .unwrap(),
            101,
            "stale generation restarts at the captured Resume boundary"
        );
        assert_eq!(
            HostMemberObservation::resolve_cursor_floor(
                MemberObservationCursor::At {
                    generation: 5,
                    seq: 1,
                },
                5,
                101,
                140,
            )
            .unwrap(),
            101,
            "even a current-generation cursor cannot replay older session rows"
        );
        let error = HostMemberObservation::resolve_cursor_floor(
            MemberObservationCursor::Tail,
            5,
            101,
            u64::MAX,
        )
        .expect_err("Tail at MAX has no representable non-replaying frontier");
        assert!(matches!(error, MemberObservationError::Internal { .. }));
    }

    fn failed_detail(record: &TrackedTurnOutcomeRecord) -> (&'static str, &WireFlowFailureDetail) {
        match &record.outcome {
            WireFlowTurnOutcome::ExtractionFailed { detail } => ("extraction_failed", detail),
            WireFlowTurnOutcome::RunFailed { detail } => ("run_failed", detail),
            WireFlowTurnOutcome::InteractionFailed { detail } => ("interaction_failed", detail),
            outcome => panic!("expected failed outcome, got {outcome:?}"),
        }
    }

    #[test]
    fn failure_detail_compaction_is_utf8_safe_typed_and_hard_bounded() {
        let cases = [
            (
                FailedWireTerminal::Extraction,
                "extraction_failed",
                "a".repeat(MAX_TURN_OUTCOME_RECORD_BYTES * 3),
            ),
            (
                FailedWireTerminal::Run,
                "run_failed",
                "\u{0000}\"\\\n\u{001f}".repeat(MAX_TURN_OUTCOME_RECORD_BYTES * 8),
            ),
            (
                FailedWireTerminal::Interaction,
                "interaction_failed",
                "🦀é中".repeat(MAX_TURN_OUTCOME_RECORD_BYTES / 4),
            ),
        ];

        for (terminal, expected_kind, original) in cases {
            let original_utf8_bytes = original.len();
            let compacted = compact_tracked_turn_outcome_record(failed_record(
                "bounded-input",
                3,
                5,
                42,
                terminal,
                original.clone(),
                u64::try_from(original_utf8_bytes).unwrap_or(u64::MAX),
                false,
            ));
            let encoded = serde_json::to_vec(&BridgeTurnOutcomeRecord {
                input_id: compacted.input_id.clone(),
                generation: compacted.generation,
                fence_token: compacted.fence_token,
                terminal_seq: compacted.terminal_seq,
                outcome: compacted.outcome.clone(),
            })
            .expect("bounded record serializes");
            assert!(
                encoded.len() <= MAX_TURN_OUTCOME_RECORD_BYTES,
                "{expected_kind} row is {} bytes",
                encoded.len()
            );
            let (actual_kind, detail) = failed_detail(&compacted);
            assert_eq!(actual_kind, expected_kind, "terminal kind is unchanged");
            assert!(detail.truncated, "oversized detail is marked truncated");
            assert_eq!(
                detail.original_utf8_bytes,
                u64::try_from(original_utf8_bytes).unwrap_or(u64::MAX),
                "original UTF-8 byte length remains exact"
            );
            assert!(
                original.starts_with(&detail.text),
                "retained text is an exact UTF-8 prefix"
            );
            assert!(detail.text.len() < original_utf8_bytes);
        }
    }

    #[test]
    fn compact_failure_detail_leaves_small_record_complete() {
        let original = "small 🦀 detail".to_string();
        let compacted = compact_tracked_turn_outcome_record(failed_record(
            "small-input",
            3,
            5,
            42,
            FailedWireTerminal::Run,
            original.clone(),
            0,
            true,
        ));
        let (_, detail) = failed_detail(&compacted);
        assert_eq!(detail.text, original);
        assert_eq!(detail.original_utf8_bytes, original.len() as u64);
        assert!(!detail.truncated);
    }

    #[test]
    fn directed_turn_framing_rejects_only_unrepresentable_identity() {
        assert!(directed_turn_record_framing_fits("normal-input", 7, 11));
        assert!(directed_turn_record_framing_fits(
            &"\u{0000}".repeat(MAX_TURN_OUTCOME_RECORD_BYTES / 6 - 512),
            7,
            11,
        ));
        assert!(!directed_turn_record_framing_fits(
            &"\u{0000}".repeat(MAX_TURN_OUTCOME_RECORD_BYTES / 6 + 1),
            7,
            11,
        ));
        assert!(!directed_turn_record_framing_fits(
            &"x".repeat(MAX_TURN_OUTCOME_RECORD_BYTES),
            7,
            11,
        ));
    }

    #[tokio::test]
    async fn journal_defense_rejects_uncompacted_oversized_outcome() {
        let (record_tx, mut record_rx) = mpsc::channel(1);
        let journal = HostTrackedTurnJournal::new(
            meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
                mob_id: "mob-size".to_string(),
                agent_identity: "worker".to_string(),
                host_id: "host".to_string(),
                binding_generation: 1,
                member_session_id: "session".to_string(),
                generation: 1,
                fence_token: 2,
            },
            record_tx,
        );
        let error = journal
            .record_turn_outcome(TrackedTurnOutcomeRecord {
                input_id: "oversized".to_string(),
                generation: 1,
                fence_token: 2,
                terminal_seq: 9,
                outcome: WireFlowTurnOutcome::RunFailed {
                    detail: WireFlowFailureDetail::complete(
                        "x".repeat(MAX_TURN_OUTCOME_RECORD_BYTES),
                    ),
                },
            })
            .await
            .expect_err("one oversized terminal detail must be rejected");
        assert!(matches!(
            error,
            MemberObservationError::OutcomeRecordTooLarge { .. }
        ));
        assert!(
            record_rx.try_recv().is_err(),
            "oversized row must not reach the actor persistence channel"
        );
    }

    #[tokio::test]
    async fn journal_record_carries_exact_host_and_binding_incarnation() {
        let (record_tx, mut record_rx) = mpsc::channel(1);
        let journal = Arc::new(HostTrackedTurnJournal::new(
            meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation {
                mob_id: "mob-record".to_string(),
                agent_identity: "worker".to_string(),
                host_id: "host-g1".to_string(),
                binding_generation: 17,
                member_session_id: "session-g1".to_string(),
                generation: 3,
                fence_token: 5,
            },
            record_tx,
        ));
        let writer = Arc::clone(&journal);
        let write = tokio::spawn(async move {
            writer
                .record_turn_outcome(TrackedTurnOutcomeRecord {
                    input_id: "input-g1".to_string(),
                    generation: 3,
                    fence_token: 5,
                    terminal_seq: 9,
                    outcome: WireFlowTurnOutcome::RunCompleted,
                })
                .await
        });
        let request = record_rx.recv().await.expect("journal forwards record");
        assert_eq!(request.expected_member, *journal.member_incarnation());
        assert_eq!(request.expected_member.host_id, "host-g1");
        assert_eq!(request.expected_member.binding_generation, 17);
        request.reply.send(Ok(())).expect("complete record write");
        write
            .await
            .expect("record task must not panic")
            .expect("actor accepted exact record");
    }
}
