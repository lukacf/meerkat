//! Identity-first runtime: delivery, status and lifecycle over Core's mob.
//!
//! Meta keeps no durable identity state: Core's mob owns each
//! member's identity, generation, fence and session binding, and its journal
//! owns each identity's history. This module provides:
//! - Delivery: `send()` and `dispatch()` with addressability enforcement
//! - Status: `status()`, built from Core's facts on every read
//! - Lifecycle: the wake door, `retire()`, `respawn()`, `reset()` and
//!   `delete_identity()`, each through Core's verbs

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock, Weak};
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt};
use meerkat_core::types::{HandlingMode, SessionId};
use tokio::sync::{Mutex, RwLock, broadcast, oneshot, watch};
use tokio::task::{JoinHandle, JoinSet};

use super::agent_memory::{
    AgentMemoryError, AgentMemoryForgetResult, AgentMemoryRecallRequest, AgentMemoryRecord,
    AgentMemoryRuntimeInjector, NewAgentMemory,
};
use super::bridge::{
    BridgeAdmissionError, BridgeError, BridgeTurnError, ResumeRejectionKind, SessionBridge,
};
use super::contracts::{
    AgentCustomizer, ContinuityStore, LeaseProvider, RosterProvider, TopologyProvider,
};
use super::delivery_scope::{
    DeliveryScope, ScopeMismatch, ScopedDeliveryError, ScopedDispatchReceipt, ScopedRecovery,
    ScopedRetireError,
};
use super::first_boot_manifest::{
    FirstBootLocation, FirstBootManifest, FirstBootManifestRefusal, FirstBootRow, FirstBootRowKind,
    FirstBootRowState,
};
use super::types::registered_spec_digest;
use super::types::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, AgentRuntimeId,
    AgentRuntimeServices, CompletionCursor, CompletionProgress, CompletionWait,
    ContinuityGeneration, CustomizerToolsPending, DeferredIdentityReason, DeliveryErrorClass,
    DeliveryErrorRecord, DispatchAdmission, DispatchInput, DurabilityPolicy, DurableAgentSpec,
    FencingToken, FirstBootPendingRow, FirstBootWait, IdentityBootstrapEntry,
    IdentityBootstrapMode, IdentityBootstrapState, IdentityBootstrapStatus, IdentityLifecycleState,
    IdentityRestoreProgress, IdentityStatus, ManagedPeerEdge, MemberHealthReport,
    MemberReloadDisposition, MemberReloadOutcome, NotAddressable, RegisteredSpec,
    ReloadAttemptOutcome, ReloadAttemptRecord, RosterContext, SendAdmission, SessionRepairRequired,
    SessionRepairScope, Ticketed, TopologyContext, TurnOutcome, TurnOutput, TurnTicket,
    TurnTracking, TurnUntrackable, TurnWait,
};
use crate::actor_loop_health::{ActorLoopHealth, ActorLoopHealthKind, ActorLoopHealthReport};
use crate::memory::records::{
    ManifestTier, MemoryId, MemoryKind, MemoryScope, NewMemoryRecord, RecordMeta, UsageEvent,
};

const MANAGED_PEER_RECONCILE_CONCURRENCY: usize = 64;
const MATERIALIZATION_FAILURE_BACKOFF: Duration = Duration::from_secs(30);
const RAW_MEMBER_ALIAS_LOCK_SWEEP_MIN: usize = 256;
const BACKGROUND_WARM_CANCELLED: &str =
    "identity background warm cancelled before session installation";
fn durable_spec_uses_external_binding(spec: &DurableAgentSpec) -> bool {
    matches!(spec.backend, Some(meerkat_mob::MobBackendKind::External))
        || matches!(
            spec.binding.as_ref(),
            Some(meerkat_contracts::WireRuntimeBinding::External { .. })
        )
}

fn interaction_id_for_delivery<'a>(
    spec: &DurableAgentSpec,
    interaction_id: Option<&'a str>,
) -> Option<&'a str> {
    // Meerkat 0.8.2 deliberately rejects transcript interaction ids on
    // remotely hosted / peer-only member turns: that metadata carrier is not
    // representable on the wire path. Meta still correlates the response
    // through its pending-interaction ledger and the peer terminal event.
    (!durable_spec_uses_external_binding(spec))
        .then_some(interaction_id)
        .flatten()
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Why a member without a session binding cannot take scope-bound delivery.
pub(crate) const NO_NATIVE_SESSION_BINDING: &str =
    "no native session binding; scope-bound delivery is unsupported for this member";

/// A scoped delivery's identity pair, exactly as supplied: both halves are
/// required and nothing is canonicalized, so the key a host persisted is the
/// key the runtime ledger records.
fn scoped_delivery_identity(
    input: &DispatchInput,
) -> Result<meerkat_mob::MobDeliveryIdentity, ScopedDeliveryError> {
    let (Some(idempotency_key), Some(correlation_id)) =
        (&input.idempotency_key, &input.correlation_id)
    else {
        return Err(ScopedDeliveryError::Rejected {
            detail: "scoped delivery requires both an idempotency key and a correlation id"
                .to_string(),
        });
    };
    meerkat_mob::MobDeliveryIdentity::new(idempotency_key.as_str(), correlation_id.as_str())
        .map_err(|error| ScopedDeliveryError::Rejected {
            detail: format!("invalid delivery identity: {error}"),
        })
}

/// The identity embodiment a completion-bearing delivery was admitted onto.
///
/// Compared by value across the unlocked window. Any inequality means the
/// identity was reset, retired or rebound while its turn ran, and the turn's
/// result belongs to an embodiment that no longer exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedIncarnation {
    runtime_id: Option<AgentRuntimeId>,
    generation: Option<u64>,
    fencing_token: Option<FencingToken>,
}

/// Which bridge verb the shared send core delivers through.
///
/// A typed mode rather than a bool, and one shared body rather than two, for
/// the same reason the bridge's own submit path is shaped this way: the parts
/// that differ between the lanes are trivial, and the parts that must NOT
/// differ - lease acquisition, alias pinning, defanging, memory injection,
/// session reconciliation - are the ones a second copy would silently drift on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendCommitMode {
    /// Return once the work is admitted. The historical behaviour of `send`.
    Ingress,
    /// Return only after this exact turn has committed its terminal boundary.
    /// Fails closed when completion cannot be observed.
    AwaitCommit,
    /// Return once the work is admitted, like `Ingress`, with the admitted
    /// turn's own completion handle kept under a [`TurnTicket`] minted for
    /// this admission; the turn's own output is recorded when it completes.
    /// The caller's interaction id rides unchanged. Falls back to `Ingress`
    /// (reported as [`TurnTracking::Unavailable`]) when the turn cannot be
    /// tracked.
    TrackTurn,
}

/// Which bridge verb a send's delivery step calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliveryLane {
    Ingress,
    AwaitCommit,
    Ticketed,
}

/// A send's delivery step once the tracked admission, if the lane called for
/// one, has been attempted.
enum DeliveryStep {
    Ingress,
    AwaitCommit,
    /// Admitted on the tracked lane; the receipt carries the admitted turn.
    Tracked(super::bridge::BridgeTurnReceipt),
}

/// Why a tracked delivery's turn cannot be tracked, or `None` to attempt the
/// tracked lane. Decided from typed state before anything is delivered, so
/// these untrackable turns go to the ingress lane without a tracked attempt.
///
/// The member's runtime mode is deliberately NOT decided here: Meta's spec
/// is the desired mode, but meerkat checks the live roster entry's mode
/// (fixed at spawn), and the two can disagree. The bridge reads the live
/// entry right before admission and refuses an `autonomous_host` member typed
/// ([`BridgeAdmissionError::UnsupportedForMode`]), before anything is
/// submitted; the tracked lanes then fall back to the ingress lane.
fn turn_untrackable(
    delivered: bool,
    external_binding: bool,
    local_human: bool,
    bridge_tracks_turn_output: bool,
) -> Option<TurnUntrackable> {
    if !delivered {
        Some(TurnUntrackable::NotDelivered)
    } else if local_human {
        Some(TurnUntrackable::HostHumanInput)
    } else if external_binding {
        Some(TurnUntrackable::ExternallyBound)
    } else if !bridge_tracks_turn_output {
        Some(TurnUntrackable::BridgeCannotReportOutput)
    } else {
        None
    }
}

/// The tracked admission was refused for the member's LIVE runtime mode
/// (by the bridge's pre-submit check of the live roster entry, or by meerkat
/// itself), before anything was delivered: the delivery falls back to the
/// ingress lane, reported untracked with the typed reason. An
/// `autonomous_host` member is [`TurnUntrackable::AutonomousHost`]; a refusal
/// in any other mode is [`TurnUntrackable::RefusedByRuntime`].
fn runtime_refused_tracking(
    identity: &AgentIdentity,
    mode: meerkat_mob::MobRuntimeMode,
    detail: &str,
) -> TurnTracking {
    tracing::debug!(
        identity = %identity,
        runtime_mode = %mode,
        detail,
        "tracked admission refused for the member's live runtime mode; delivering untracked \
         on the ingress lane"
    );
    TurnTracking::Unavailable(match mode {
        meerkat_mob::MobRuntimeMode::AutonomousHost => TurnUntrackable::AutonomousHost,
        mode => TurnUntrackable::RefusedByRuntime { mode },
    })
}

/// Refuse a send whose input was authored from a row other than `spec`, the
/// identity's registered row (see [`RegisteredSpec`]).
fn refuse_unless_registered_row(
    identity: &AgentIdentity,
    spec: &DurableAgentSpec,
    expected_spec_digest: Option<&str>,
) -> Result<(), IdentityRuntimeError> {
    match expected_spec_digest {
        Some(expected) if registered_spec_digest(spec).ok().as_deref() != Some(expected) => {
            Err(IdentityRuntimeError::RegisteredSpecChanged {
                identity: identity.clone(),
                expected_spec_digest: expected.to_string(),
            })
        }
        _ => Ok(()),
    }
}

/// Tracking reported for a ticketed delivery whose lane recorded none (the
/// identity has no bound runtime, so nothing was delivered to track). A
/// runtime without a session bridge refuses the send instead.
fn untracked_delivery() -> TurnTracking {
    TurnTracking::Unavailable(TurnUntrackable::NotDelivered)
}

/// What [`IdentityRuntime::send_core`] settled on.
/// A governed send the member's native admission accepted or deduplicated.
#[derive(Debug)]
pub struct GovernedSendOutcome {
    /// The identity admission and the admitted turn's own ticket.
    pub ticketed: Ticketed<SendAdmission>,
    /// The native acknowledgement: the canonical admitted input and whether
    /// native admission deduplicated it.
    pub admission: super::bridge::GovernedAdmission,
    /// The host's promotion result for newly accepted work (`Ok` for a
    /// replay, whose custody is withdrawn); a failure is the host's typed
    /// post-admission fault and does not undo the admitted turn.
    pub settlement: Result<(), String>,
}

struct SendCoreOutcome {
    token: FencingToken,
    completion_baseline: CompletionCursor,
    /// `Some` exactly when the send asked for [`SendCommitMode::TrackTurn`].
    turn: Option<TurnTracking>,
}

/// One invocation of the shared identity delivery state machine.
///
/// Keeping the lane-specific carriers in one value makes the common send body
/// explicit without growing a positional parameter list that is easy to wire
/// incorrectly when a new carrier is added.
struct SendRequest<'a> {
    expected_alias: Option<&'a str>,
    content: &'a meerkat_core::ContentInput,
    system_prompt: Option<&'a str>,
    handling_mode: HandlingMode,
    interaction_id: Option<&'a str>,
    commit_mode: SendCommitMode,
    console_human: Option<ConsoleHumanSend<'a>>,
    /// The registered row's digest the input was authored from; a different
    /// registered row refuses the send before delivery.
    expected_spec_digest: Option<&'a str>,
}

/// Budget for ONE managed-topology wiring call to the session bridge (a wire,
/// unwire or wire inspection; each is a mob-actor round trip that also awaits
/// comms trust installs and peer notices). The reconcile or topology mutation
/// settles with [`IdentityRuntimeError::PeerWiringTimedOut`] instead of
/// parking behind a wedged call. No admission waits on these calls at all.
const PEER_WIRING_CALL_BUDGET: Duration = Duration::from_secs(30);

/// End-to-end budget for the wake path's target-only topology reconcile
/// (lock acquisition plus every wiring call), so a woken member's admission
/// is delayed by at most this much. A failure is logged and retried after
/// admission; it never refuses the input.
const WAKE_TOPOLOGY_BUDGET: Duration = Duration::from_secs(30);

/// How many times a managed-topology plan is made before a reconcile or
/// topology mutation gives up with
/// [`IdentityRuntimeError::PeerTopologySuperseded`] because an endpoint's
/// runtime binding kept changing while the plan's wiring ran.
const PEER_TOPOLOGY_MAX_PLAN_ATTEMPTS: u32 = 3;

/// Run one managed-topology bridge wiring call under
/// [`PEER_WIRING_CALL_BUDGET`].
async fn bounded_peer_wiring<T>(
    operation: &'static str,
    call: impl Future<Output = T>,
) -> Result<T, IdentityRuntimeError> {
    bounded_peer_wiring_by(operation, None, call).await
}

/// [`bounded_peer_wiring`], additionally capped by an optional end-to-end
/// `deadline` (the wake path's single budget for its whole target
/// reconcile). Whichever comes first ends the call as `PeerWiringTimedOut`,
/// through the same unsettled-edge bookkeeping as any other timeout.
async fn bounded_peer_wiring_by<T>(
    operation: &'static str,
    deadline: Option<tokio::time::Instant>,
    call: impl Future<Output = T>,
) -> Result<T, IdentityRuntimeError> {
    let start = tokio::time::Instant::now();
    let per_call = start + PEER_WIRING_CALL_BUDGET;
    let until = deadline.map_or(per_call, |deadline| deadline.min(per_call));
    tokio::time::timeout_at(until, call).await.map_err(|_| {
        IdentityRuntimeError::PeerWiringTimedOut {
            operation,
            waited: until.saturating_duration_since(start),
        }
    })
}

/// The runtime bindings a managed-topology plan was made against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PeerBindingEpoch {
    /// Each registered identity Core holds a live member for, to the
    /// member's runtime id, read from Core.
    active: BTreeMap<AgentIdentity, AgentRuntimeId>,
}

impl PeerBindingEpoch {
    /// Whether none of `identities` changed binding (runtime id or
    /// liveness) between `self` and `current`.
    fn unchanged_for(&self, current: &Self, identities: &BTreeSet<AgentIdentity>) -> bool {
        identities
            .iter()
            .all(|identity| self.active.get(identity) == current.active.get(identity))
    }
}

/// What a managed peer-edge reconcile would change, read without applying
/// anything: a candidate launch's census and dry run
/// ([`IdentityRuntime::census_managed_peer_edges`]). Each edge is a pair of
/// identities in canonical order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PeerEdgeCensus {
    /// Desired edges between two live members that are not wired.
    pub(crate) would_wire: BTreeSet<(AgentIdentity, AgentIdentity)>,
    /// Wired edges the reconcile owns that are no longer desired.
    pub(crate) would_unwire: BTreeSet<(AgentIdentity, AgentIdentity)>,
    /// Desired edges already wired.
    pub(crate) retained: BTreeSet<(AgentIdentity, AgentIdentity)>,
    /// Desired edges with an endpoint that is not a live member: a reconcile
    /// leaves them pending.
    pub(crate) pending: BTreeSet<(AgentIdentity, AgentIdentity)>,
}

impl PeerEdgeCensus {
    /// Log the census at WARN when the reconcile would change anything, at
    /// INFO otherwise.
    pub(crate) fn log(&self, origin: &'static str) {
        let joined = |edges: &BTreeSet<(AgentIdentity, AgentIdentity)>| {
            edges
                .iter()
                .map(|(a, b)| format!("{a}<->{b}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        if self.would_wire.is_empty() && self.would_unwire.is_empty() {
            tracing::info!(
                origin,
                retained = self.retained.len(),
                pending = self.pending.len(),
                "candidate launch peer-edge census: the reconcile would wire and unwire no edge"
            );
        } else {
            tracing::warn!(
                origin,
                would_wire = self.would_wire.len(),
                would_unwire = self.would_unwire.len(),
                wire = %joined(&self.would_wire),
                unwire = %joined(&self.would_unwire),
                "candidate launch peer-edge census: the reconcile applies nothing; the \
                 authoritative launch would wire and unwire these edges"
            );
        }
    }
}

/// Outcome of one plan / wire / revalidate pass over the managed topology.
enum PeerTopologyAttempt {
    Committed,
    /// An involved endpoint's binding changed during the wiring; nothing was
    /// committed and the plan must be remade.
    Stale,
}

/// Whether a send or dispatch result means meerkat admitted the input, so the
/// post-admission peer hydration it deferred is still owed. A completion or
/// post-admission failure is a turn that WAS admitted.
///
/// `LeaseLost` and `CompletionIncarnationChanged` are deliberately absent:
/// on the send and dispatch lanes a lease loss is only raised by the lease
/// checks that run BEFORE the bridge admission (the first one, and the
/// re-check after a reload-for-delivery), so nothing was admitted; and
/// `CompletionIncarnationChanged` is raised only by `wait_for_completion`,
/// which is outside these lanes. Neither is a turn that ran.
fn input_was_admitted<T>(result: &Result<T, IdentityRuntimeError>) -> bool {
    matches!(
        result,
        Ok(_)
            | Err(IdentityRuntimeError::CompletionFailed { .. }
                | IdentityRuntimeError::PostAdmissionResolutionFailed { .. }
                | IdentityRuntimeError::PostAdmissionSuperseded { .. })
    )
}

/// One target's in-flight post-admission hydration: the claim that owns it
/// and whether another pass was requested while it ran.
#[derive(Debug, Clone, Copy)]
struct PeerHydrationClaim {
    token: u64,
    rerun: bool,
}

/// In-flight post-admission hydrations, one per target, plus the token
/// counter that makes every claim distinguishable.
#[derive(Debug, Default)]
struct PeerHydrations {
    next_token: u64,
    in_flight: BTreeMap<AgentIdentity, PeerHydrationClaim>,
}

impl PeerHydrations {
    /// Claim `identity`'s slot, returning the claim token. `None` when a
    /// hydration is already in flight; that one is asked to run once more.
    fn claim(&mut self, identity: &AgentIdentity) -> Option<u64> {
        if let Some(claim) = self.in_flight.get_mut(identity) {
            claim.rerun = true;
            return None;
        }
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        self.in_flight.insert(
            identity.clone(),
            PeerHydrationClaim {
                token,
                rerun: false,
            },
        );
        Some(token)
    }

    /// Release `identity`'s slot only if `token` still owns it, so a stale
    /// release (a second removal after a panic, or a refused spawn racing a
    /// newer claim) can never drop someone else's claim.
    fn release(&mut self, identity: &AgentIdentity, token: u64) {
        if self
            .in_flight
            .get(identity)
            .is_some_and(|claim| claim.token == token)
        {
            self.in_flight.remove(identity);
        }
    }
}

/// A claimed post-admission hydration slot; releasing it (also on panic or
/// cancellation) lets the next admission to that target hydrate again.
struct PeerHydrationSlot<'a> {
    hydrations: &'a StdMutex<PeerHydrations>,
    identity: AgentIdentity,
    token: u64,
}

impl PeerHydrationSlot<'_> {
    /// Finish one pass: under one lock, either consume a rerun request (keep
    /// the slot, return true) or release the slot (return false). Atomic, so
    /// a request that arrives as the pass ends is never lost.
    fn finish_pass(&self) -> bool {
        let mut hydrations = self
            .hydrations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match hydrations.in_flight.get_mut(&self.identity) {
            Some(claim) if claim.token == self.token && claim.rerun => {
                claim.rerun = false;
                true
            }
            _ => {
                hydrations.release(&self.identity, self.token);
                false
            }
        }
    }

    /// Run hydration passes until no rerun is pending, then release.
    async fn run(self, runtime: &IdentityRuntime) {
        loop {
            #[cfg(test)]
            runtime
                .peer_hydration_passes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            runtime.hydrate_peer_topology_logged(&self.identity).await;
            if !self.finish_pass() {
                return;
            }
        }
    }
}

impl Drop for PeerHydrationSlot<'_> {
    /// Release this claim (a no-op once `finish_pass` released it, or when a
    /// newer claim owns the slot): covers a pass that panicked or was
    /// cancelled mid-flight, so the next admission can hydrate again.
    fn drop(&mut self) {
        self.hydrations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .release(&self.identity, self.token);
    }
}

/// Whether a foreground materialization reconciles the whole managed peer
/// topology after the embodiment commits, or leaves that to the admission
/// path (which reconciles only the woken target's own edges before admission
/// and hydrates the rest after it; see
/// [`IdentityRuntime::materialize_for_admission`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopologyAfterEmbody {
    Reconcile,
    DeferToPostAdmission,
}

struct ConsoleHumanSend<'a> {
    delivery_identity: &'a meerkat_mob::MobDeliveryIdentity,
    expected_session: Option<&'a str>,
    /// Selected skills for this exact console turn; carried only on the
    /// local host-human lane.
    skill_references: &'a [meerkat_core::skills::SkillKey],
}

enum DeliveryPreparation {
    Work(meerkat_mob::MobRuntimeMode),
    ConsoleHuman,
    /// A keyed send, committed as a member continuation: defanged like every
    /// other delivery, but a continuation's prompt carries no injected
    /// context, so ambient recall is skipped.
    KeyedContinuation,
}

/// Map errors observed before a completion-bearing receipt is available.
/// A deadline without a receipt does not establish whether admission ran.
fn admission_phase_error(
    identity: &AgentIdentity,
    err: BridgeAdmissionError,
) -> IdentityRuntimeError {
    let identity = identity.clone();
    match err {
        BridgeAdmissionError::HostHumanInput(error) => IdentityRuntimeError::HostHumanInput(error),
        BridgeAdmissionError::GovernedInput(error) => IdentityRuntimeError::GovernedInput(error),
        BridgeAdmissionError::CompletionUnsupported(reason) => {
            IdentityRuntimeError::CompletionUnavailable { identity, reason }
        }
        BridgeAdmissionError::Mob(detail)
        | BridgeAdmissionError::UnsupportedForMode { detail, .. }
        | BridgeAdmissionError::InvalidInput(detail)
        | BridgeAdmissionError::InvariantViolation(detail)
        | BridgeAdmissionError::MobAdmissionClosed { detail, .. }
        | BridgeAdmissionError::MobShutdownInProgress { detail, .. }
        | BridgeAdmissionError::IdentityConvergenceAdmissionClosed { detail, .. } => {
            IdentityRuntimeError::AdmissionFailed { identity, detail }
        }
        BridgeAdmissionError::ActorLoopStalled {
            operation,
            stall_id,
            stalled_for,
            observation,
            ..
        } => IdentityRuntimeError::ActorLoopStalled {
            identity,
            operation,
            stall_id,
            stalled_for,
            observation,
        },
        BridgeAdmissionError::ActorTerminated {
            operation,
            detail,
            observation,
            ..
        } => IdentityRuntimeError::ActorTerminated {
            identity,
            operation,
            detail,
            observation,
        },
        BridgeAdmissionError::ReloadRequired { reason, .. } => {
            IdentityRuntimeError::ReloadRequired { identity, reason }
        }
        BridgeAdmissionError::AdmissionBacklogFull { depth, .. } => {
            IdentityRuntimeError::AdmissionBacklogFull { identity, depth }
        }
        BridgeAdmissionError::ReloadRefused {
            session_id, reason, ..
        } => IdentityRuntimeError::ReloadRefused {
            identity,
            session_id,
            reason,
        },
        BridgeAdmissionError::ReloadTimedOut {
            session_id, stage, ..
        } => IdentityRuntimeError::ReloadTimedOut {
            identity,
            session_id,
            stage,
        },
        BridgeAdmissionError::ActorAdmissionTimeout {
            operation,
            waited,
            command,
            ..
        } => IdentityRuntimeError::ActorAdmissionTimeout {
            identity,
            operation,
            waited,
            command,
        },
        BridgeAdmissionError::ResumeRejected {
            kind:
                ResumeRejectionKind::Unavailable {
                    reason: meerkat_mob::error::SessionResumeUnavailableReason::Absent,
                },
            session_id,
            detail,
        } => IdentityRuntimeError::IdentitySessionAbsent {
            identity,
            session_id,
            detail,
        },
        // Core's other resume refusals keep their wake classification:
        // final, held or retryable, by kind and never by text.
        BridgeAdmissionError::ResumeRejected {
            kind,
            session_id,
            detail,
        } => {
            let session = session_id.clone();
            classify_wake_failure(
                &identity,
                Some(&session),
                BridgeError::ResumeRejected {
                    kind,
                    session_id,
                    detail,
                },
            )
        }
        other @ BridgeAdmissionError::ProviderAuthRejected { .. } => {
            IdentityRuntimeError::AdmissionFailed {
                identity,
                detail: other.to_string(),
            }
        }
    }
}

/// Map the ingress-only lane's bridge error. The typed fast-fail refusals
/// (open stall, terminated loop, reload required) keep their type so callers
/// and the automatic reload can act on them; everything else keeps the
/// historical `Internal("bridge deliver: ..")` form.
fn ingress_phase_error(identity: &AgentIdentity, err: BridgeError) -> IdentityRuntimeError {
    let identity = identity.clone();
    match err {
        BridgeError::HostHumanInput(error) => IdentityRuntimeError::HostHumanInput(error),
        BridgeError::ActorAdmissionTimeout {
            operation,
            waited,
            command,
            ..
        } => IdentityRuntimeError::ActorAdmissionTimeout {
            identity,
            operation,
            waited,
            command,
        },
        BridgeError::ActorLoopStalled {
            operation,
            stall_id,
            stalled_for,
            observation,
            ..
        } => IdentityRuntimeError::ActorLoopStalled {
            identity,
            operation,
            stall_id,
            stalled_for,
            observation,
        },
        BridgeError::ActorTerminated {
            operation,
            detail,
            observation,
            ..
        } => IdentityRuntimeError::ActorTerminated {
            identity,
            operation,
            detail,
            observation,
        },
        BridgeError::ReloadRequired { reason, .. } => {
            IdentityRuntimeError::ReloadRequired { identity, reason }
        }
        BridgeError::AdmissionBacklogFull { depth, .. } => {
            IdentityRuntimeError::AdmissionBacklogFull { identity, depth }
        }
        BridgeError::ReloadRefused {
            session_id, reason, ..
        } => IdentityRuntimeError::ReloadRefused {
            identity,
            session_id,
            reason,
        },
        BridgeError::ReloadTimedOut {
            session_id, stage, ..
        } => IdentityRuntimeError::ReloadTimedOut {
            identity,
            session_id,
            stage,
        },
        BridgeError::ResumeRejected {
            kind:
                ResumeRejectionKind::Unavailable {
                    reason: meerkat_mob::error::SessionResumeUnavailableReason::Absent,
                },
            session_id,
            detail,
        } => IdentityRuntimeError::IdentitySessionAbsent {
            identity,
            session_id,
            detail,
        },
        // Core's other resume refusals keep their wake classification.
        BridgeError::ResumeRejected {
            kind,
            session_id,
            detail,
        } => {
            let session = session_id.clone();
            classify_wake_failure(
                &identity,
                Some(&session),
                BridgeError::ResumeRejected {
                    kind,
                    session_id,
                    detail,
                },
            )
        }
        other => IdentityRuntimeError::Internal(format!("bridge deliver: {other}")),
    }
}

/// Map the bridge's reload-primitive error. `Refused` and `TimedOut` keep
/// their class; everything else is an internal bridge failure.
fn reload_phase_error(identity: &AgentIdentity, err: BridgeError) -> IdentityRuntimeError {
    let identity = identity.clone();
    match err {
        typed @ (BridgeError::ActorAdmissionTimeout { .. }
        | BridgeError::ActorLoopStalled { .. }
        | BridgeError::ActorTerminated { .. }) => ingress_phase_error(&identity, typed),
        BridgeError::ReloadRefused {
            session_id, reason, ..
        } => IdentityRuntimeError::ReloadRefused {
            identity,
            session_id,
            reason,
        },
        BridgeError::ReloadTimedOut {
            session_id, stage, ..
        } => IdentityRuntimeError::ReloadTimedOut {
            identity,
            session_id,
            stage,
        },
        other => {
            IdentityRuntimeError::Internal(format!("bridge reload_member_registration: {other}"))
        }
    }
}

/// Why a member is being retired on the mob plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleRetireIntent {
    /// The identity leaves the fleet: memory harvest hooks run (session tail
    /// distillation, exit interview).
    Retire,
    /// Non-destructive reload: the same durable session is re-materialized
    /// immediately under the same lifecycle lock, so nothing rotates and the
    /// memory hooks must NOT run (an exit interview for a member that stays
    /// would be a lie).
    Reload,
}

/// Map the terminal edge of an admitted turn. Pre-admission outcomes are
/// unrepresentable in this input type.
fn turn_phase_error(identity: &AgentIdentity, err: BridgeTurnError) -> IdentityRuntimeError {
    let identity = identity.clone();
    match err {
        BridgeTurnError::CompletionFailed(detail) => {
            IdentityRuntimeError::CompletionFailed { identity, detail }
        }
        BridgeTurnError::PostAdmissionResolutionFailed(detail) => {
            IdentityRuntimeError::PostAdmissionResolutionFailed { identity, detail }
        }
    }
}

/// Why Core cannot resume an identity's last session. Each is
/// Core's durable answer at rest (`SessionResumeUnavailableReason`), matched
/// by kind and never by text; no retry changes it. Core's `Absent` is not
/// here: a session that was never persisted holds nothing to lose, so the
/// wake spawns fresh instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum UnresumableReason {
    /// The document is intact but archived, and its runtime lifecycle refuses
    /// revival.
    ArchivedNotRevivable,
    /// Positive durable authority or lifecycle rows exist without a
    /// materializable session body.
    ContradictoryDurableAuthority,
}

impl UnresumableReason {
    /// The wire token (`data.reason` of `IDENTITY_UNRESUMABLE_CODE`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ArchivedNotRevivable => "archived_not_revivable",
            Self::ContradictoryDurableAuthority => "contradictory_durable_authority",
        }
    }
}

/// Why Core held an identity's resume. The
/// durable session is kept in every case and every later wake tries again;
/// the hold is recorded for the views until one succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WakeHold {
    /// Core withheld the session's durable resume
    /// ([`meerkat_core::service::DurableResumeHold`]). An audited-endpoint
    /// divergence is repaired with `rkat session repair-wholeblob`, then
    /// `meta/reload_member`. Core re-runs its tail recovery on every resume
    /// and has no command for the other holds, so they stay until the
    /// durable evidence changes.
    DurableResume(meerkat_core::service::DurableResumeHold),
    /// Core cannot prove which committed boundary of the session is
    /// authoritative (`SessionResumeUnavailableReason::CommittedBoundaryUnprovable`);
    /// the durable copies are retained for reconciliation.
    CommittedBoundaryUnprovable,
    /// The roster profile moved off the role Core last ran the identity
    /// under, and this activation declared no such migration (Meta never
    /// infers one), or Core refused the declared migration.
    RoleMigrationNotApplied,
}

impl WakeHold {
    /// What the operator does next.
    #[must_use]
    pub const fn remedy(self) -> super::types::WakeRefusalRemedy {
        match self {
            Self::DurableResume(
                meerkat_core::service::DurableResumeHold::AuditedEndpointDivergence,
            ) => super::types::WakeRefusalRemedy::RepairSession,
            Self::DurableResume(_) | Self::CommittedBoundaryUnprovable => {
                super::types::WakeRefusalRemedy::OperatorReview
            }
            Self::RoleMigrationNotApplied => super::types::WakeRefusalRemedy::ResolveRoleMigration,
        }
    }
}

/// Errors from identity-first runtime operations.
#[derive(Debug)]
pub enum IdentityRuntimeError {
    HostHumanInput(super::bridge::HostHumanInputError),
    /// A governed delivery failed. A definite pre-admission refusal means
    /// the input was not admitted; [`GovernedInputError::AdmissionUnresolved`]
    /// means admission may have happened and the custody is with the host.
    GovernedInput(super::bridge::GovernedInputError),
    /// The customizer's callback tools were refused for their
    /// installed-artifact references; the build published nothing.
    ToolArtifactRefused(Box<meerkat_core::ToolArtifactRefusal>),
    /// Target identity is not registered/active.
    UnknownIdentity(AgentIdentity),
    /// send() rejected: target is InternalOnly.
    NotAddressable(NotAddressable),
    /// PHASE 1 of 4. A completion-bearing send could not be honoured AT ALL.
    ///
    /// Nothing was submitted: either no bridge is installed, the identity has
    /// no bound runtime id, or the bridge is ingress-only. The completion lane
    /// fails closed here rather than degrading to the admission-only path -
    /// degrading would report success for a turn that was never awaited.
    ///
    /// Safe to fall back or fail; nothing ran.
    CompletionUnavailable {
        identity: AgentIdentity,
        reason: String,
    },
    /// PHASE 2 of 4. No completion-bearing receipt was obtained. This does
    /// not by itself establish execution fate or authorize a retry.
    AdmissionFailed {
        identity: AgentIdentity,
        detail: String,
    },
    /// An actor round trip was not observed within budget. Neither a missing
    /// reply nor a missing upstream execution flag proves nonexecution.
    ActorAdmissionTimeout {
        identity: AgentIdentity,
        operation: &'static str,
        waited: Duration,
        command: Option<super::bridge::ActorCommandTimeout>,
    },
    /// The actor-loop probe has an open stall. `observation` distinguishes
    /// refusal before this call from an in-flight observation that ended.
    /// A correlated recovery does not settle an in-flight call's fate.
    ActorLoopStalled {
        identity: AgentIdentity,
        operation: &'static str,
        stall_id: u64,
        stalled_for: Duration,
        observation: super::bridge::ActorCallObservation,
    },
    /// The actor-loop probe observed closed channels. An in-flight call may
    /// have progressed; only `BeforeCall` proves this call was not started.
    ActorTerminated {
        identity: AgentIdentity,
        operation: &'static str,
        detail: String,
        observation: super::bridge::ActorCallObservation,
    },
    /// meerkat refused admission because the member's runtime registration is
    /// durability-degraded and needs a registration-authorized cold reload.
    /// The runtime already ran ONE automatic non-destructive reload
    /// (`reload_member`) and retried; this is the typed failure after that
    /// single attempt (or the reload itself failing, with both reasons in
    /// `reason`). Nothing was delivered. Operator path: `meta/member_health`
    /// then `meta/reload_member`; never `respawn_member` (destructive).
    ReloadRequired {
        identity: AgentIdentity,
        reason: String,
    },
    /// Refused BEFORE admission: the member's per-member admission lane is
    /// full (meerkat 0.8.34 `MemberAdmissionBacklogFull`). Per-member
    /// backpressure, not a loop or durability verdict: RETRYABLE once the
    /// lane drains. Nothing was delivered.
    AdmissionBacklogFull {
        identity: AgentIdentity,
        depth: usize,
    },
    /// meerkat refused the member reload because the durable resume authority
    /// is still unreadable: the store is not healthy yet. The degraded
    /// registration is retained and the next send stays reload-required.
    /// RETRYABLE once the store is healthy; nothing was delivered.
    ReloadRefused {
        identity: AgentIdentity,
        session_id: String,
        reason: String,
    },
    /// meerkat's bounded member reload (`MEMBER_RELOAD_TOTAL_TIMEOUT`) timed
    /// out at `stage`. NOT retryable without inspection: the reload may have
    /// partially progressed. Nothing was delivered.
    ReloadTimedOut {
        identity: AgentIdentity,
        session_id: String,
        stage: String,
    },
    /// PHASE 3 of 4. The work WAS admitted and its turn reached a FAILED
    /// terminal.
    ///
    /// The turn RAN. Never retry - a retry would run the identity's turn a
    /// second time.
    CompletionFailed {
        identity: AgentIdentity,
        detail: String,
    },
    /// PHASE 4 of 4. The turn was admitted, reached a SUCCESSFUL terminal, and
    /// only then did projecting its session id fail.
    ///
    /// The member did the work. Non-retryable: repairing cannot undo a turn
    /// that already ran, and resubmitting would run it twice.
    PostAdmissionResolutionFailed {
        identity: AgentIdentity,
        detail: String,
    },
    /// The turn was admitted and ran, and the identity was RESET, RETIRED or
    /// REBOUND while it ran.
    ///
    /// Distinct from [`Self::PostAdmissionResolutionFailed`]: there the
    /// embodiment is intact and only the projection failed; here the embodiment
    /// the turn belonged to no longer exists. Non-retryable, and deliberately
    /// NOT reconciled - binding the live incarnation to a dead turn's session
    /// is worse than losing the attribution.
    PostAdmissionSuperseded {
        identity: AgentIdentity,
        detail: String,
    },
    /// The send named the registered row its input was authored from, and
    /// the identity's registered row is no longer that row. Checked under
    /// the identity's lifecycle lock before delivery: NOTHING was submitted.
    RegisteredSpecChanged {
        identity: AgentIdentity,
        expected_spec_digest: String,
    },
    /// Operation rejected: identity is not in a state that permits this operation.
    InvalidState {
        identity: AgentIdentity,
        state: IdentityLifecycleState,
        operation: &'static str,
    },
    /// Dispatch rejected BEFORE bridge admission: the delivery identity is
    /// half-formed (idempotency key without a correlation id, or vice
    /// versa) or fails upstream canonical validation (non-nil canonical
    /// UUID correlation). Fail-closed on purpose: a degraded delivery under
    /// a broken identity is a silent dedup hole - the caller gets dedup or
    /// this refusal, never at-least-once. NO delivery occurs.
    InvalidDeliveryIdentity {
        identity: AgentIdentity,
        detail: String,
    },
    /// Duplicate identities in roster.
    DuplicateIdentity(AgentIdentity),
    /// A lifecycle request named an old generated runtime alias after the
    /// durable identity had already advanced to another generation.
    StaleRuntimeAlias {
        identity: AgentIdentity,
        requested: String,
        current: Option<AgentRuntimeId>,
    },
    /// A completion wait presented a baseline from a superseded runtime
    /// incarnation. Turn counts do not carry across incarnations, so this is
    /// reported rather than resolved either way — the caller must capture a
    /// fresh baseline instead of inferring that its turn did or did not run.
    CompletionIncarnationChanged {
        identity: AgentIdentity,
        baseline: CompletionCursor,
        observed: CompletionCursor,
    },
    /// A held identity bootstrap refused its roster, or its pass ended
    /// before the host was asked to complete: no member of a refused roster
    /// was constructed, and a refused pass's terminal decision was `Abort`
    /// for every member it held. See
    /// [`super::held_bootstrap::HeldBootstrapRefusal`].
    HeldBootstrapRefused(super::held_bootstrap::HeldBootstrapRefusal),
    /// A held identity bootstrap's pass ended with the terminal decision
    /// `Abort` after its host was asked to complete: `refusal` is why, and
    /// `completion` is how the host's callback actually settled, recorded
    /// apart (a late `Ok` proves callback settlement only, not activation,
    /// and nothing about publication outside this runtime).
    HeldBootstrapAborted {
        refusal: super::held_bootstrap::HeldBootstrapRefusal,
        completion: super::held_bootstrap::HeldCompletionOutcome,
    },
    /// A held bootstrap's member that the pass did not publish owes its host
    /// a fresh held completion: an ordinary (unheld) embodiment would publish
    /// a new instance whose finished surface the host's completion never saw,
    /// so every wake outside a held pass that covers it refuses it typed,
    /// until such a pass completes it.
    HeldCompletionOwed {
        identity: AgentIdentity,
    },
    /// The identity's last session cannot be resumed: Core
    /// answered that it is archived beyond revival, or that its durable
    /// authority contradicts its body. Nothing was spawned and no retry can
    /// change the answer; `delete_identity` starts a fresh life. JSON-RPC
    /// `IDENTITY_UNRESUMABLE_CODE`.
    IdentityUnresumable {
        identity: AgentIdentity,
        session_id: SessionId,
        reason: UnresumableReason,
        detail: String,
    },
    /// The identity's history could not be read from Core, or its resume
    /// failed on a transient fault. Nothing was spawned, and a
    /// read fault is never taken as "no history": the next wake retries.
    /// JSON-RPC `IDENTITY_HISTORY_UNAVAILABLE_CODE`.
    IdentityHistoryUnavailable {
        identity: AgentIdentity,
        detail: String,
    },
    /// Core's member history could not be read for a view over every
    /// identity (`statuses`, `roster_inspect`): no status is reported rather
    /// than an empty or guessed list. JSON-RPC
    /// `IDENTITY_HISTORY_UNAVAILABLE_CODE`.
    MemberHistoryUnavailable {
        detail: String,
    },
    /// The first-boot manifest `storage-migrate-native` or `identity-reset`
    /// left in the state directory cannot be used: the reader refused it.
    /// Which identities it covers is unknown, so the roster compile refuses
    /// every identity with it and no identity wakes; the gateway stays up,
    /// and its bootstrap status names the file and the refusal. JSON-RPC
    /// `STORAGE_RESOLUTION_CODE`.
    FirstBootManifestUnreadable {
        path: PathBuf,
        detail: String,
    },
    /// Core's member history contradicts the identity's first-boot row
    ///: Core's mob store is not the one the migration read.
    /// Nothing was changed, the identity does not wake, and the row stays in
    /// the manifest. JSON-RPC `STORAGE_RESOLUTION_CODE`.
    FirstBootRowRefused {
        identity: AgentIdentity,
        kind: FirstBootRowKind,
        /// The session the row names.
        manifest_session: SessionId,
        /// The session Core's last binding of the identity names, if any.
        core_session: Option<SessionId>,
        detail: String,
    },
    /// The identity still has a first-boot row to carry out, and
    /// `operation` would start a life the row does not know: it is refused
    /// until the row is carried out. `waiting_for` names the way out: a
    /// roster that lists the identity (its compile carries the row out), or
    /// the listed identity's wake. JSON-RPC `STORAGE_RESOLUTION_CODE`.
    FirstBootRowPending {
        identity: AgentIdentity,
        kind: FirstBootRowKind,
        waiting_for: FirstBootWait,
        operation: &'static str,
    },
    /// The wake's resume failed on a transient fault (busy, I/O, a lock, an
    /// unavailable store). Nothing was spawned, the durable session
    /// is kept, and the next wake retries. Never a reason to delete the
    /// identity. JSON-RPC `IDENTITY_HISTORY_UNAVAILABLE_CODE`.
    IdentityWakeRetryable {
        identity: AgentIdentity,
        /// The session the wake was resuming or reviving, when it had one.
        session_id: Option<SessionId>,
        detail: String,
    },
    /// A delivery reached the identity's member, whose session Core holds no
    /// durable copy of: the bridge's delivery repair refused it `Absent`
    /// before retiring anything, so nothing was delivered and nothing was
    /// disposed. Core never persisted that session, so nothing durable is
    /// lost by replacing it: the identity's next delivery starts its fresh
    /// life first, with this session as its replaced session. Deliver again.
    /// JSON-RPC `IDENTITY_HISTORY_UNAVAILABLE_CODE`.
    IdentitySessionAbsent {
        identity: AgentIdentity,
        /// The member's session Core holds no durable copy of.
        session_id: SessionId,
        detail: String,
    },
    /// The build of the identity's member was refused DETERMINISTICALLY: the
    /// host answered the app-side `callback/build_agent` round trip with an
    /// error (the candidate-mode effect gate class), provider authentication
    /// refused it with a credential kind no unattended retry can change,
    /// the placement host's realm backend is not persistent while the
    /// identity needs durable sessions, or its profile declares
    /// `durable = true` on a mob whose member sessions do not outlive the
    /// process. Returned on each attempt; nothing is parked or recorded.
    HostRejectedBuild {
        identity: AgentIdentity,
        reason: String,
    },
    /// Core holds an identity intent row for the identity: a host adopted
    /// it, and Core's reconciler drives it. Meta's lifecycle verbs
    /// refuse it, because a Meta command would make a second driver.
    /// JSON-RPC `IDENTITY_HOST_ADOPTED_CODE`.
    IdentityHostAdopted {
        identity: AgentIdentity,
    },
    /// The identity's member id is held by a member the host made directly
    /// on the mob: Core's live or retiring member under that id carries no
    /// `agent_identity` label naming the identity, and this runtime did not
    /// spawn it. Meta does not adopt such a member: a roster compile that
    /// declares the identity, and the identity's wake, refuse before anything
    /// is registered, woken, respawned or retired. The host labels the member
    /// for the identity (respawns it onto its session with an
    /// `agent_identity` label naming the identity) or retires it, then
    /// compiles or wakes again. JSON-RPC internal-error code with `data.kind`
    /// `identity_name_held_by_raw_member`.
    IdentityNameHeldByRawMember {
        identity: AgentIdentity,
        member: String,
        session_id: Option<SessionId>,
    },
    /// The wake's resume was held: Core withheld the
    /// session's durable resume, or the roster profile moved off the role
    /// Core last ran the identity under without a declared migration. The
    /// durable session is kept, nothing was spawned, and the next wake tries
    /// again; [`WakeHold::remedy`] names what the operator does.
    WakeHeld {
        identity: AgentIdentity,
        session_id: SessionId,
        hold: WakeHold,
        detail: String,
    },
    /// A managed-topology wiring call to the session bridge (a wire, unwire or
    /// wire inspection, each one a mob-actor round trip) was not observed
    /// within its budget. The reconcile or topology mutation that issued it
    /// settles with this instead of parking. The call's own effect is
    /// unknown; nothing was recorded as managed for it, and the next
    /// reconcile observes the live graph and converges.
    PeerWiringTimedOut {
        operation: &'static str,
        waited: Duration,
    },
    /// A managed-topology plan kept going stale: an endpoint's runtime binding
    /// changed (reset, respawn, retire) while its wiring calls ran, on every
    /// one of `attempts` replans. Nothing stale was committed; the next
    /// reconcile replans from the live bindings.
    PeerTopologySuperseded {
        attempts: u32,
    },
    /// Generic I/O or internal error.
    Internal(String),
    /// A keyed send was refused by Core, or its answer could not be made.
    /// `error` is Core's typed refusal, rendered with Core's own code and
    /// data: an invalid key or a conflict (which names the first delivery)
    /// committed nothing new; a host that cannot keep the key refused before
    /// committing; an unreadable continuation status may leave a commit
    /// standing, so retry the same key. Boxed: a mob error is large.
    KeyedSendRefused {
        identity: AgentIdentity,
        error: Box<meerkat_mob::MobError>,
    },
    /// A send status read named a session the identity never had: neither
    /// its current session nor one Core's member history binds to it.
    /// Nothing was read.
    SessionNotHeldByIdentity {
        identity: AgentIdentity,
        session_id: SessionId,
    },
}

impl std::fmt::Display for IdentityRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostHumanInput(error) => write!(f, "{error}"),
            Self::GovernedInput(error) => write!(f, "{error}"),
            Self::ToolArtifactRefused(refusal) => write!(f, "{refusal}"),
            Self::UnknownIdentity(id) => write!(f, "unknown identity: {id}"),
            Self::NotAddressable(err) => write!(f, "{err}"),
            Self::CompletionUnavailable { identity, reason } => write!(
                f,
                "completion-bearing send to {identity} cannot be honoured: {reason}; \
                 NOTHING was submitted"
            ),
            Self::AdmissionFailed { identity, detail } => write!(
                f,
                "completion-bearing send to {identity} did not obtain an admission receipt: {detail}"
            ),
            Self::ActorAdmissionTimeout {
                identity,
                operation,
                waited,
                command,
            } => super::bridge::fmt_actor_timeout(f, operation, identity, *waited, *command),
            Self::ActorLoopStalled {
                identity,
                operation,
                stall_id,
                stalled_for,
                observation,
            } => write!(
                f,
                "send to {identity}: the mob actor loop has an open stall (stall_id \
                 {stall_id}, open for {stalled_for:?}) during `{operation}`; {observation}; \
                 consult runtime fate before retrying an in-flight call"
            ),
            Self::ActorTerminated {
                identity,
                operation,
                detail,
                observation,
            } => write!(
                f,
                "send to {identity}: the mob actor loop TERMINATED ({detail}) so \
                 `{operation}` cannot continue to be observed; {observation}; \
                 this will not recover, the process must restart"
            ),
            Self::ReloadRequired { identity, reason } => write!(
                f,
                "send to {identity} refused: the member's runtime registration requires a \
                 registration-authorized cold reload and one automatic reload attempt did not \
                 clear it; nothing was delivered; run meta/reload_member (non-destructive), \
                 never respawn_member: {reason}"
            ),
            Self::AdmissionBacklogFull { identity, depth } => write!(
                f,
                "send to {identity} refused: the member's admission lane is full ({depth} \
                 deliveries parked); nothing was delivered; retryable once the lane drains"
            ),
            Self::ReloadRefused {
                identity,
                session_id,
                reason,
            } => write!(
                f,
                "reload of {identity} (session {session_id}) refused: store not healthy, reload \
                 refused, retry later; the registration was retained and sends stay \
                 reload-required until the store is healthy: {reason}"
            ),
            Self::ReloadTimedOut {
                identity,
                session_id,
                stage,
            } => write!(
                f,
                "reload of {identity} (session {session_id}) timed out at stage `{stage}` \
                 (MEMBER_RELOAD_TOTAL_TIMEOUT); not retryable without inspection: the reload may \
                 have partially progressed"
            ),
            Self::CompletionFailed { identity, detail } => write!(
                f,
                "completion-bearing send to {identity} was admitted and then failed: {detail}; \
                 the turn RAN - do not retry"
            ),
            Self::PostAdmissionResolutionFailed { identity, detail } => write!(
                f,
                "completion-bearing send to {identity} COMPLETED and then its session \
                 projection failed: {detail}; the turn RAN - do not retry"
            ),
            Self::PostAdmissionSuperseded { identity, detail } => write!(
                f,
                "completion-bearing send to {identity} COMPLETED, but the identity was \
                 superseded while it ran: {detail}; the turn RAN against an embodiment \
                 that no longer exists - do not retry, and its session was NOT rebound"
            ),
            Self::RegisteredSpecChanged { identity, .. } => write!(
                f,
                "the registered row of {identity} is no longer the row the input was \
                 authored from; NOTHING was submitted"
            ),
            Self::InvalidState {
                identity,
                state,
                operation,
            } => write!(
                f,
                "cannot {operation} identity {identity} in state {state:?}"
            ),
            Self::InvalidDeliveryIdentity { identity, detail } => write!(
                f,
                "dispatch to {identity} refused before bridge admission: invalid delivery \
                 identity ({detail}); dedup or typed refusal, never a degraded delivery"
            ),
            Self::DuplicateIdentity(id) => write!(f, "duplicate identity in roster: {id}"),
            Self::StaleRuntimeAlias {
                identity,
                requested,
                current,
            } => write!(
                f,
                "stale runtime alias for {identity}: requested {requested}, current {}",
                current
                    .as_ref()
                    .map(AgentRuntimeId::as_str)
                    .unwrap_or("<none>")
            ),
            Self::CompletionIncarnationChanged {
                identity,
                baseline,
                observed,
            } => write!(
                f,
                "completion baseline {baseline} for {identity} belongs to a superseded runtime \
                 incarnation (now {observed}); capture a fresh baseline"
            ),
            Self::HeldBootstrapRefused(refusal) => {
                write!(f, "held identity bootstrap refused: {refusal}")
            }
            Self::HeldBootstrapAborted {
                refusal,
                completion,
            } => write!(
                f,
                "held identity bootstrap aborted: {refusal}; {completion}"
            ),
            Self::HeldCompletionOwed { identity } => write!(
                f,
                "identity {identity} owes its host a fresh held completion: its held member \
                 was not published, so it is not embodied unheld. Run the host's held \
                 bootstrap again (a pass that covers the identity publishes it); if the host \
                 will never complete it, remove its mark from the state directory's \
                 held-completion-owed directory"
            ),
            Self::IdentityUnresumable {
                identity,
                session_id,
                reason,
                detail,
            } => write!(
                f,
                "identity {identity} cannot be woken: Core cannot resume its last session \
                 {session_id} ({}: {detail}); nothing was spawned, and delete_identity \
                 starts a fresh life",
                reason.as_str()
            ),
            Self::IdentityHistoryUnavailable { identity, detail } => write!(
                f,
                "identity {identity}: {detail}; nothing was changed, retry"
            ),
            Self::MemberHistoryUnavailable { detail } => write!(
                f,
                "Core's member history could not be read: {detail}; no identity status is \
                 reported, retry"
            ),
            Self::FirstBootManifestUnreadable { path, detail } => write!(
                f,
                "the first-boot manifest {} cannot be used ({detail}); no identity was woken. \
                 Move it aside and keep it. If no 0.9.4 runtime has started on the directory \
                 since storage-migrate-native --apply, restore the copy that run kept beside \
                 it (storage-migrate-native.first-boot.json.pre-*.native-migration), then run \
                 meta_gateway identity-reset again for every reset made since that run: \
                 identity-reset keeps no copy, so the restored file lacks them. If a runtime \
                 has started since, restore no older copy (its rows were carried out \
                 already): start with no manifest and run identity-reset again for each reset \
                 made since the last start. Removing the manifest also drops a migration row \
                 still pending for an identity no roster listed since: that identity starts \
                 fresh when it is added back, and nothing reports it, so compare against the \
                 last meta_gateway identity-census output before removing it",
                path.display()
            ),
            Self::FirstBootRowRefused {
                identity,
                kind,
                manifest_session,
                core_session,
                detail,
            } => write!(
                f,
                "identity {identity} was not woken: its first-boot {kind} row names session \
                 {manifest_session}, but Core's member history contradicts it ({detail}; \
                 Core's last session: {}). Core's mob store is not the one \
                 storage-migrate-native migrated; nothing was changed",
                core_session
                    .as_ref()
                    .map_or_else(|| "none".to_string(), ToString::to_string)
            ),
            Self::FirstBootRowPending {
                identity,
                kind,
                waiting_for,
                operation,
            } => write!(
                f,
                "identity {identity} still has a first-boot {kind} row to carry out, so \
                 {operation} is refused; nothing was changed. {}",
                match waiting_for {
                    FirstBootWait::Roster => {
                        "Add the identity to the roster: the roster compile carries the row \
                         out. Then retry"
                    }
                    FirstBootWait::CarryOut => {
                        "Wake the identity: its wake carries the row out, or answers why it \
                         cannot. Then retry"
                    }
                }
            ),
            Self::IdentityWakeRetryable {
                identity,
                session_id: Some(session_id),
                detail,
            } => write!(
                f,
                "identity {identity} was not woken: resuming session {session_id} failed \
                 transiently ({detail}); nothing was spawned, retry the wake"
            ),
            Self::IdentityWakeRetryable {
                identity,
                session_id: None,
                detail,
            } => write!(
                f,
                "identity {identity} was not woken: Core's wake failed transiently \
                 ({detail}); nothing was spawned, retry the wake"
            ),
            Self::IdentitySessionAbsent {
                identity,
                session_id,
                detail,
            } => write!(
                f,
                "identity {identity}'s member is bound to session {session_id}, which Core \
                 holds no durable copy of ({detail}); nothing was delivered. The next \
                 delivery starts the identity's fresh life first, so deliver again"
            ),
            Self::HostRejectedBuild { identity, reason } => write!(
                f,
                "the build of identity {identity} was deterministically rejected ({reason}); \
                 a roster or policy change is needed before it can be woken"
            ),
            Self::IdentityHostAdopted { identity } => write!(
                f,
                "identity {identity} is adopted by its host: Core holds its identity intent \
                 and drives it, so Meta's lifecycle verbs refuse it"
            ),
            Self::IdentityNameHeldByRawMember {
                identity,
                member,
                session_id,
            } => write!(
                f,
                "identity {identity} is declared under the member id of a member the host made \
                 directly on the mob ({member}{}), which Meta does not adopt: label that member \
                 for the identity (an agent_identity label naming {identity}) or retire it, then \
                 try again",
                session_id
                    .as_ref()
                    .map_or_else(String::new, |session| format!(" on session {session}"))
            ),
            Self::WakeHeld {
                identity,
                session_id,
                detail,
                ..
            } => write!(
                f,
                "identity {identity} was not woken: Core held the resume of session \
                 {session_id} ({detail}); the session is kept and nothing was spawned"
            ),
            Self::PeerWiringTimedOut { operation, waited } => write!(
                f,
                "managed topology wiring call {operation} was not observed within {waited:?}; \
                 its effect is unknown and nothing was recorded as managed for it"
            ),
            Self::PeerTopologySuperseded { attempts } => write!(
                f,
                "managed topology plan went stale on all {attempts} attempts: an endpoint's \
                 runtime binding changed while its wiring ran; nothing stale was committed"
            ),
            Self::Internal(msg) => write!(f, "internal: {msg}"),
            Self::KeyedSendRefused { identity, error } => match error.as_ref() {
                meerkat_mob::MobError::MemberSendIdempotency { refusal, .. } => {
                    write!(f, "keyed send to identity '{identity}': {refusal}")
                }
                other => write!(f, "keyed send to identity '{identity}': {other}"),
            },
            Self::SessionNotHeldByIdentity {
                identity,
                session_id,
            } => write!(
                f,
                "session {session_id} is not a session of identity '{identity}'"
            ),
        }
    }
}

/// A build failure whose root cause is the HOST's own answer: the
/// `callback/build_agent` round trip COMPLETED and the app returned an error
/// (rpc_gateway mints `callback/build_agent failed: callback error: <host
/// message>` for exactly this case; see `StdioCallbackBridge::call`).
/// Transport-tier callback failures ("callback transport closed", timeouts,
/// dropped channels) deliberately do NOT match: those are retryable.
pub(crate) fn is_host_rejected_build_error(detail: &str) -> bool {
    detail.contains("callback/build_agent failed: callback error:")
}

/// Whether a typed provider-auth kind is a DETERMINISTIC build wall: a fact
/// about configuration that no unattended retry can change, so the wake
/// refuses with the reason instead of reporting a retryable fault. The
/// remaining kinds describe credential STATE that can change out of band (a
/// lease refreshes, a login completes, an I/O fault clears, the credential
/// store answers again), so the next wake retries them. Exhaustive on
/// purpose: a new kind must be classified here.
pub(crate) const fn is_deterministic_provider_auth_kind(kind: meerkat_core::AuthErrorKind) -> bool {
    use meerkat_core::AuthErrorKind;
    match kind {
        AuthErrorKind::MissingSecret
        | AuthErrorKind::UnsupportedCombination
        | AuthErrorKind::MissingRequiredMetadata
        | AuthErrorKind::WorkspaceMismatch
        | AuthErrorKind::HostOwnedUnavailable => true,
        AuthErrorKind::StaleCredential
        | AuthErrorKind::RefreshRequired
        | AuthErrorKind::LeaseAbsent
        | AuthErrorKind::UserReauthRequired
        | AuthErrorKind::Expired
        | AuthErrorKind::RefreshFailed
        | AuthErrorKind::ResolveRequired
        | AuthErrorKind::InteractiveLoginRequired
        | AuthErrorKind::Io
        | AuthErrorKind::Other
        // Credential-store refusals are the shared process credential
        // owner's state: unattended setup never prompts, and explicit
        // interactive access clears a remembered denial.
        | AuthErrorKind::CredentialAccessDenied
        | AuthErrorKind::CredentialInteractionRequired
        | AuthErrorKind::CredentialAccessPending
        | AuthErrorKind::CredentialStoreUnavailable => false,
    }
}

/// The refusal reason for a build the bridge refused DETERMINISTICALLY, or
/// `None` when the failure may resolve on its own (the next wake retries it).
/// Four walls are recognised: the typed provider-auth refusal
/// ([`BridgeError::ProviderAuthRejected`] with a kind
/// [`is_deterministic_provider_auth_kind`] accepts), the placement host's
/// typed refusal for a realm backend that is not persistent
/// ([`BridgeError::RealmBackendUnavailable`]; the reason names the remedy),
/// meerkat's typed refusal of a durable profile on a mob without persistent
/// sessions ([`BridgeError::NonDurableService`]; the reason names the
/// remedy), and the host's own `callback/build_agent` answer
/// ([`is_host_rejected_build_error`], the one place a string probe remains
/// because `rpc_gateway` mints that text).
pub(crate) fn deterministic_build_rejection_reason(error: &BridgeError) -> Option<String> {
    match error {
        BridgeError::ProviderAuthRejected { failure, detail } => {
            is_deterministic_provider_auth_kind(failure.kind)
                .then(|| provider_auth_refusal_reason(failure, detail))
        }
        BridgeError::RealmBackendUnavailable { detail } => Some(format!(
            "{detail}; a durable identity needs durable sessions on the host it is placed \
             on: run that host with a persistent realm backend, or remove the identity's \
             placement"
        )),
        BridgeError::NonDurableService { profile, detail } => Some(format!(
            "{detail}; profile '{profile}' declares durable = true, which needs a mob whose \
             member sessions outlive the process: run the mob on a persistent realm, or drop \
             durable from the profile"
        )),
        other => {
            let detail = other.to_string();
            is_host_rejected_build_error(&detail).then_some(detail)
        }
    }
}

/// Whether a failed resume is Core's typed answer that no durable session
/// exists for the id (`SessionResumeUnavailableReason::Absent`). Such a
/// session was never persisted and holds nothing to lose, so the wake spawns
/// fresh instead of refusing.
fn resume_refused_absent(error: &BridgeError) -> bool {
    matches!(
        error,
        BridgeError::ResumeRejected {
            kind: ResumeRejectionKind::Unavailable {
                reason: meerkat_mob::error::SessionResumeUnavailableReason::Absent,
            },
            ..
        }
    )
}

/// A resume refusal Core gave no typed reason for (`Other`), which only
/// Core's durable read can tell apart from a transient fault.
fn resume_refused_untyped(error: &BridgeError) -> bool {
    matches!(
        error,
        BridgeError::ResumeRejected {
            kind: ResumeRejectionKind::Other,
            ..
        }
    )
}

/// The runtime-plane reservation on identity-first member creation: an
/// identity the console would attribute to `_system` or `__console__` is
/// refused as a host-rejected build
/// ([`crate::member_comms_id::refuse_runtime_plane_identity`]), typed on
/// every attempt and never recorded, before anything is built.
fn refuse_runtime_plane_creation(identity: &AgentIdentity) -> Result<(), IdentityRuntimeError> {
    crate::member_comms_id::refuse_runtime_plane_identity(identity).map_err(|reason| {
        IdentityRuntimeError::HostRejectedBuild {
            identity: identity.clone(),
            reason,
        }
    })
}

/// Classify a failed resume or revival of `identity`,
/// by kind and never by text:
/// - Core's durable answer that the last session cannot be resumed
///   (archived beyond revival, or contradictory durable authority) is FINAL:
///   `IdentityUnresumable`, remedy `delete_identity`;
/// - a resume Core held (a durable resume hold, an unprovable committed
///   boundary, a role migration it did not apply) is HELD: `WakeHeld`, the
///   session kept;
/// - the host's deterministic build refusal is `HostRejectedBuild`, typed on
///   every attempt and not recorded;
/// - an identity intent row the bridge found (a host adopted the identity
///   after the wake's own check) is `IdentityHostAdopted`;
/// - everything else (Core's refused tail recovery, authority that changed
///   mid-load, a member that moved under the bridge's reads, busy, I/O, a
///   lock, a stringified Core error) is retryable and never suggests
///   deleting anything.
///
/// Core's `Absent` never reaches here as a refusal: the wake spawns fresh
/// first ([`resume_refused_absent`]).
fn classify_wake_failure(
    identity: &AgentIdentity,
    session_id: Option<&SessionId>,
    error: BridgeError,
) -> IdentityRuntimeError {
    use meerkat_core::service::DurableResumeHold;
    use meerkat_mob::error::SessionResumeUnavailableReason as Unavailable;

    if let Some(reason) = deterministic_build_rejection_reason(&error) {
        return IdentityRuntimeError::HostRejectedBuild {
            identity: identity.clone(),
            reason,
        };
    }
    if matches!(error, BridgeError::HostAdopted { .. }) {
        return IdentityRuntimeError::IdentityHostAdopted {
            identity: identity.clone(),
        };
    }
    let detail = error.to_string();
    let retryable = |session_id: Option<&SessionId>| IdentityRuntimeError::IdentityWakeRetryable {
        identity: identity.clone(),
        session_id: session_id.cloned(),
        detail: detail.clone(),
    };
    let held = |session_id: &SessionId, hold: WakeHold| IdentityRuntimeError::WakeHeld {
        identity: identity.clone(),
        session_id: session_id.clone(),
        hold,
        detail: detail.clone(),
    };
    if let BridgeError::SessionRepairRequired { session_id, .. } = &error {
        return held(
            session_id,
            WakeHold::DurableResume(DurableResumeHold::AuditedEndpointDivergence),
        );
    }
    let (BridgeError::ResumeRejected { kind, .. }, Some(session_id)) = (&error, session_id) else {
        return retryable(session_id);
    };
    let unresumable = |reason: UnresumableReason| IdentityRuntimeError::IdentityUnresumable {
        identity: identity.clone(),
        session_id: session_id.clone(),
        reason,
        detail: detail.clone(),
    };
    match kind {
        ResumeRejectionKind::Unavailable { reason } => match reason {
            Unavailable::ArchivedNotRevivable => {
                unresumable(UnresumableReason::ArchivedNotRevivable)
            }
            Unavailable::ContradictoryDurableAuthority => {
                unresumable(UnresumableReason::ContradictoryDurableAuthority)
            }
            Unavailable::CommittedBoundaryUnprovable => {
                held(session_id, WakeHold::CommittedBoundaryUnprovable)
            }
            // The store's authority moved while the body loaded: the next
            // attempt reads it again. `Absent` spawned fresh before this
            // classifier; reaching here it is read as transient.
            Unavailable::AuthorityChangedDuringMaterialization | Unavailable::Absent => {
                retryable(Some(session_id))
            }
        },
        // Core's own doc: retry after the conflicting runtime quiesces.
        ResumeRejectionKind::Held {
            hold: DurableResumeHold::RecoveryRefused,
        } => retryable(Some(session_id)),
        ResumeRejectionKind::Held { hold } => held(session_id, WakeHold::DurableResume(*hold)),
        ResumeRejectionKind::RoleMigrationNotApplied => {
            held(session_id, WakeHold::RoleMigrationNotApplied)
        }
        _ => retryable(Some(session_id)),
    }
}

/// The refusal the views record for a failed wake, or `None`
/// for a refusal the next wake retries. Only the final refusal and Core's
/// holds are recorded; the code is the one the wake answered.
fn wake_refusal_of(error: &IdentityRuntimeError) -> Option<super::types::WakeRefusal> {
    let (session_id, remedy) = match error {
        IdentityRuntimeError::IdentityUnresumable { session_id, .. } => {
            (session_id, super::types::WakeRefusalRemedy::DeleteIdentity)
        }
        IdentityRuntimeError::WakeHeld {
            session_id, hold, ..
        } => (session_id, hold.remedy()),
        _ => return None,
    };
    Some(super::types::WakeRefusal {
        code: crate::rpc::identity_error_code(error),
        session_id: session_id.clone(),
        remedy,
    })
}

/// The operations a candidate launch refuses a wake with, one per change
/// of the stored roster the wake would make
/// ([`DeferredIdentityReason`]).
const CANDIDATE_CREATE_MEMBER: &str = "create a member, on a candidate launch, for";
const CANDIDATE_MOVE_ROLE: &str = "move to another role, on a candidate launch,";
const CANDIDATE_FRESH_SESSION: &str = "start a fresh session, on a candidate launch, for";
const CANDIDATE_RESUME_MEMBER: &str =
    "spawn a member onto the stored session, on a candidate launch, of";
/// The operation a candidate launch refuses a pending fresh first-boot row
/// with.
const CANDIDATE_FIRST_BOOT_FRESH: &str = "carrying it out on a candidate launch";

/// Why a candidate launch deferred an identity to the authoritative launch,
/// when `error` is the candidate's refusal of a roster change; `None` for
/// every other error, which stays the identity's own failure (a store
/// error, a lost comms identity, any refusal Core or the host makes).
pub(super) fn candidate_deferral_of(
    error: &IdentityRuntimeError,
) -> Option<DeferredIdentityReason> {
    match error {
        IdentityRuntimeError::InvalidState { operation, .. } => match *operation {
            CANDIDATE_CREATE_MEMBER => Some(DeferredIdentityReason::CreateMember),
            CANDIDATE_MOVE_ROLE => Some(DeferredIdentityReason::MoveRole),
            CANDIDATE_FRESH_SESSION => Some(DeferredIdentityReason::FreshSession),
            CANDIDATE_RESUME_MEMBER => Some(DeferredIdentityReason::ResumeMember),
            _ => None,
        },
        IdentityRuntimeError::FirstBootRowPending { operation, .. }
            if *operation == CANDIDATE_FIRST_BOOT_FRESH =>
        {
            Some(DeferredIdentityReason::FirstBootFresh)
        }
        _ => None,
    }
}

/// Log the candidate launch roster census of the identities `status`
/// defers to the authoritative launch, when it defers any.
fn log_candidate_deferral_census(status: &IdentityBootstrapStatus) {
    if status.deferred.is_empty() {
        return;
    }
    let deferred = status
        .deferred
        .iter()
        .map(|deferred| format!("{} ({})", deferred.identity, deferred.reason.as_str()))
        .collect::<Vec<_>>();
    tracing::warn!(
        deferred = deferred.len(),
        identities = %deferred.join(", "),
        "candidate launch roster census: these identities are deferred to the authoritative \
         launch, whose wake creates, moves, freshly starts or resumes them; they are not broken"
    );
}

/// Name a stale scoped retire's mismatch as a scoped dispatch names it.
/// meerkat refuses "another incarnation is current" with the current fence
/// only: when the live member read before the retire runs under another
/// runtime id, the mismatch is `RuntimeId`; otherwise only Core's fence
/// moved (`LeaseFencingToken`).
fn name_stale_retire_mismatch(
    error: ScopedRetireError,
    live: Option<&meerkat_mob::MemberDeliveryScope>,
    scope: &meerkat_mob::MemberDeliveryScope,
) -> ScopedRetireError {
    match error {
        ScopedRetireError::StaleScope {
            mismatch: ScopeMismatch::LeaseFencingToken,
            detail,
        } if live.is_some_and(|live| live.runtime_id() != scope.runtime_id()) => {
            ScopedRetireError::StaleScope {
                mismatch: ScopeMismatch::RuntimeId,
                detail,
            }
        }
        error => error,
    }
}

/// Meta's public spelling (`rt:{identity}:{generation}`) of a Core runtime
/// id, the one the session bridge keys its member bookkeeping by.
fn meta_runtime_id(
    runtime_id: &meerkat_mob::AgentRuntimeId,
) -> Result<AgentRuntimeId, IdentityRuntimeError> {
    AgentRuntimeId::parse(&crate::member_comms_id::public_runtime_alias(runtime_id)).map_err(
        |error| {
            IdentityRuntimeError::Internal(format!(
                "Core runtime id {runtime_id} has no public alias: {error}"
            ))
        },
    )
}

/// One producer text for a deterministic provider-auth refusal so operators
/// see a stable, greppable reason with the operator path inline.
fn provider_auth_refusal_reason(
    failure: &meerkat_core::service::SessionProviderAuthFailure,
    detail: &str,
) -> String {
    format!(
        "{} (typed AuthErrorKind `{}`): a credential/binding fact no unattended retry can \
         change, so the wake is refused. Operator path: supply the credential or fix the \
         binding for provider `{}`, then wake the identity again or change the roster spec. \
         Refusal: {detail}",
        failure.public_message(),
        failure.kind.as_str(),
        failure.provider.as_str()
    )
}

impl std::error::Error for IdentityRuntimeError {}

impl IdentityRuntimeError {
    /// Secret-free owner observations for RPC errors and member health.
    /// Omitted execution/retry flags are not a nonexecution verdict.
    pub fn structured_data(&self) -> Option<serde_json::Value> {
        match self {
            Self::HostHumanInput(error) => Some(error.structured_data()),
            Self::ToolArtifactRefused(refusal) => {
                let mut data = serde_json::to_value(refusal.as_ref()).ok()?;
                data["kind"] = serde_json::json!(meerkat_core::ToolArtifactRefusal::KIND);
                Some(data)
            }
            Self::ActorAdmissionTimeout {
                operation,
                waited,
                command,
                ..
            } => Some(super::bridge::actor_timeout_data(
                operation, *waited, *command,
            )),
            Self::ActorLoopStalled {
                operation,
                stall_id,
                observation,
                ..
            } => Some(serde_json::json!({
                "kind": "actor_loop_stalled", "operation": operation,
                "stall_id": stall_id, "observation": observation,
            })),
            Self::ActorTerminated {
                operation,
                observation,
                ..
            } => Some(serde_json::json!({
                "kind": "actor_terminated", "operation": operation,
                "observation": observation,
            })),
            Self::AdmissionBacklogFull { depth, .. } => Some(serde_json::json!({
                "kind": "mob_member_admission_backlog_full", "depth": depth,
            })),
            Self::ReloadRefused { .. } => Some(serde_json::json!({
                "kind": "mob_member_reload_refused",
            })),
            Self::ReloadTimedOut { stage, .. } => Some(serde_json::json!({
                "kind": "mob_member_reload_timed_out", "stage": stage,
            })),
            Self::IdentityUnresumable {
                identity,
                session_id,
                reason,
                ..
            } => Some(serde_json::json!({
                "kind": "identity_unresumable", "identity": identity,
                "session_id": session_id.to_string(), "reason": reason.as_str(),
                "remedy": "delete_identity", "retryable": false,
            })),
            Self::IdentityHistoryUnavailable { identity, .. } => Some(serde_json::json!({
                "kind": "identity_history_unavailable", "identity": identity,
                "retryable": true,
            })),
            Self::MemberHistoryUnavailable { .. } => Some(serde_json::json!({
                "kind": "identity_history_unavailable", "retryable": true,
            })),
            Self::FirstBootManifestUnreadable { path, .. } => Some(serde_json::json!({
                "kind": "first_boot_manifest_unreadable", "path": path.display().to_string(),
                "retryable": false,
            })),
            Self::FirstBootRowRefused {
                identity,
                kind,
                manifest_session,
                core_session,
                ..
            } => Some(serde_json::json!({
                "kind": "first_boot_row_refused", "identity": identity,
                "row_kind": kind, "manifest_session": manifest_session.to_string(),
                "core_session": core_session.as_ref().map(ToString::to_string),
                "retryable": false,
            })),
            Self::FirstBootRowPending {
                identity,
                kind,
                waiting_for,
                operation,
            } => Some(serde_json::json!({
                "kind": "first_boot_row_pending", "identity": identity, "row_kind": kind,
                "waiting_for": waiting_for, "operation": operation,
                "remedy": match waiting_for {
                    FirstBootWait::Roster => "add_to_roster",
                    FirstBootWait::CarryOut => "wake_identity",
                },
                "retryable": true,
            })),
            Self::IdentityWakeRetryable {
                identity,
                session_id,
                ..
            } => Some(serde_json::json!({
                "kind": "identity_wake_retryable", "identity": identity,
                "session_id": session_id.as_ref().map(ToString::to_string), "retryable": true,
            })),
            Self::IdentitySessionAbsent {
                identity,
                session_id,
                ..
            } => Some(serde_json::json!({
                "kind": "identity_session_absent", "identity": identity,
                "session_id": session_id.to_string(), "retryable": true,
            })),
            Self::IdentityHostAdopted { identity } => Some(serde_json::json!({
                "kind": "identity_host_adopted", "identity": identity, "retryable": false,
            })),
            // A member the host made under the identity's member id keeps the
            // internal-error code; the kind names the collision, the member
            // and its session, so the host can label or retire that member.
            Self::IdentityNameHeldByRawMember {
                identity,
                member,
                session_id,
            } => Some(serde_json::json!({
                "kind": "identity_name_held_by_raw_member", "identity": identity,
                "member": member,
                "session_id": session_id.as_ref().map(ToString::to_string),
                "retryable": false,
            })),
            // An identity whose held member was not published keeps the
            // internal-error code; the kind tells its host that the identity
            // waits for a held pass to publish it, so the host can tell this
            // pending state apart from any other refusal.
            Self::HeldCompletionOwed { identity } => Some(serde_json::json!({
                "kind": "held_completion_owed", "identity": identity,
            })),
            // The typed repair hold rides the same `durable_resume_hold` key
            // meerkat uses on every surface, so a caller classifies this
            // exactly as it would classify meerkat's own -32013 refusal.
            Self::WakeHeld {
                session_id,
                hold:
                    WakeHold::DurableResume(
                        meerkat_core::service::DurableResumeHold::AuditedEndpointDivergence,
                    ),
                ..
            } => Some(serde_json::json!({
                "kind": "mob_member_session_repair_required",
                meerkat_core::service::SessionError::DURABLE_RESUME_HOLD_KEY:
                    meerkat_core::service::DurableResumeHold::AuditedEndpointDivergence.as_str(),
                "session_id": session_id.to_string(),
                "content_retained": true,
                "retryable": false,
                "operator_path": [
                    SessionRepairRequired::REPAIR_SUBCOMMAND,
                    "meta/reload_member",
                ],
            })),
            // Core's other durable holds carry Core's own hold projection.
            Self::WakeHeld {
                identity,
                session_id,
                hold: WakeHold::DurableResume(hold),
                ..
            } => Some(serde_json::json!({
                "kind": "mob_member_session_resume_held", "identity": identity,
                meerkat_core::service::SessionError::DURABLE_RESUME_HOLD_KEY: hold.as_str(),
                "session_id": session_id.to_string(), "content_retained": true,
                "retryable": false,
            })),
            Self::WakeHeld {
                identity,
                session_id,
                hold: WakeHold::CommittedBoundaryUnprovable,
                ..
            } => Some(serde_json::json!({
                "kind": "mob_member_session_resume_held", "identity": identity,
                "resume_unavailable_reason": "committed_boundary_unprovable",
                "session_id": session_id.to_string(), "content_retained": true,
                "retryable": false,
            })),
            Self::WakeHeld {
                identity,
                session_id,
                hold: WakeHold::RoleMigrationNotApplied,
                ..
            } => Some(serde_json::json!({
                "kind": "identity_role_migration_refused", "identity": identity,
                "session_id": session_id.to_string(), "content_retained": true,
                "retryable": false,
            })),
            Self::PeerWiringTimedOut { operation, waited } => Some(serde_json::json!({
                "kind": "peer_wiring_timed_out", "operation": operation,
                "waited_ms": u64::try_from(waited.as_millis()).unwrap_or(u64::MAX),
            })),
            Self::PeerTopologySuperseded { attempts } => Some(serde_json::json!({
                "kind": "peer_topology_superseded", "attempts": attempts,
            })),
            Self::RegisteredSpecChanged {
                identity,
                expected_spec_digest,
            } => Some(serde_json::json!({
                "kind": "registered_spec_changed", "identity": identity,
                "expected_spec_digest": expected_spec_digest,
                "submitted": false,
            })),
            Self::KeyedSendRefused { error, .. } => {
                mob_error_wire(error).ok().and_then(|(_, data)| data)
            }
            Self::SessionNotHeldByIdentity {
                identity,
                session_id,
            } => Some(serde_json::json!({
                "kind": "session_not_held_by_identity", "identity": identity,
                "session_id": session_id.to_string(),
            })),
            _ => None,
        }
    }
}

/// The JSON-RPC code and data meerkat's own RPC surface gives a mob error:
/// its wire detail when it has one, else its structured data under its wire
/// code (invalid params when it names none), so a host that moves between
/// meerkat's gateway and Meta's classifies one failure the same way. `Err`
/// when the wire detail does not serialize.
pub(crate) fn mob_error_wire(
    error: &meerkat_mob::MobError,
) -> Result<(i64, Option<serde_json::Value>), serde_json::Error> {
    match error.wire_detail() {
        Some(detail) => detail
            .detail_value()
            .map(|data| (i64::from(detail.code().jsonrpc_code()), Some(data))),
        None => Ok((
            error
                .wire_error_code()
                .map_or(-32602, |code| i64::from(code.jsonrpc_code())),
            error.structured_data(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Per-identity runtime state
// ---------------------------------------------------------------------------

/// Meta's own facts about one identity: its roster policy and this process's
/// diagnostics. No Core fact is kept here: whether the identity is
/// live, and its runtime id, session, generation and fence, are read from
/// Core on every call.
#[derive(Debug, Clone)]
pub(crate) struct IdentityEntry {
    pub spec: DurableAgentSpec,
    /// Bootstrap pass that last accepted `spec` as this entry's desired
    /// roster projection. Foreground materialization binds readiness writes
    /// to this entry-owned generation only after acquiring the lifecycle
    /// lock, so a pass that wins that lock cannot be mistaken for either its
    /// predecessor or successor merely because the global status epoch moved.
    pub bootstrap_generation: u64,
    /// The last wake refused for good or held for the operator,
    /// cleared by the next successful wake or when a lifecycle verb starts on
    /// a member Core holds live. Process-local and never read as truth:
    /// every wake asks Core again.
    pub wake_refusal: Option<super::types::WakeRefusal>,
    /// The session the last wake replaced with a fresh one because Core holds
    /// no durable session for it, until the next wake or the next
    /// lifecycle verb on the live member.
    pub replaced_session: Option<super::types::ReplacedSession>,
    /// The most recent delivery failure for this identity, cleared by the
    /// next successful delivery. Read by `meta/member_health` so an
    /// operator learns WHY sends fail without spending an admission budget.
    pub last_delivery_error: Option<DeliveryErrorRecord>,
    /// The most recent reload attempt and how it ended (see `member_health`).
    pub last_reload: Option<ReloadAttemptRecord>,
    /// A member session a delivery found Core holds no durable copy of
    /// (`IdentitySessionAbsent`): the next delivery to that same session
    /// starts the identity's fresh life first.
    pub absent_delivery_session: Option<SessionId>,
}

// ---------------------------------------------------------------------------
// Identity-scoped events
// ---------------------------------------------------------------------------

/// Events emitted for a specific identity, used by `subscribe()`.
#[derive(Debug, Clone)]
pub enum IdentityEvent {
    /// The identity's view state changed through a verb of this runtime.
    StateChanged {
        identity: AgentIdentity,
        new_state: IdentityLifecycleState,
    },
}

/// Per-identity event channel capacity.
const IDENTITY_EVENT_CHANNEL_CAPACITY: usize = 64;

// ---------------------------------------------------------------------------
// IdentityRuntime
// ---------------------------------------------------------------------------

/// Configuration for the identity-first runtime.
pub struct IdentityRuntimeConfig {
    /// Ignored; removed before 0.9.4. The identity runtime keeps
    /// no durable identity state of its own: Core's mob is the authority.
    pub continuity_store: Arc<dyn ContinuityStore>,
    /// Ignored; removed before 0.9.4. Core's realm lock and
    /// hosting claim are the host exclusivity.
    pub lease_provider: Arc<dyn LeaseProvider>,
    pub runtime_instance_id: String,
    /// Whether the composition has a durable runtime store (dispatch
    /// acknowledgements report durability from it).
    pub has_runtime_store: bool,
    /// Ignored; removed before 0.9.4 (it was the continuity
    /// store's declared durability).
    pub durability_policy: DurabilityPolicy,
    /// Optional session bridge for real session delivery. When `None`,
    /// delivery operations validate invariants but do not forward to
    /// the Meerkat session pipeline (useful for tests).
    pub bridge: Option<Arc<dyn SessionBridge>>,
    /// Default timeout for wait_for_output / wait_for_output_containing.
    /// Defaults to 90 seconds if not set.
    pub default_timeout: Option<Duration>,
}

/// Not `Clone` (D67): a by-value copy reached through an exported view's
/// `Deref` would carry the inner `Arc<IdentityRuntime>` without the mob's
/// public owner. Share it as `Arc<IdentityFirstRuntimeContext>`.
pub struct IdentityFirstRuntimeContext {
    /// Crate-internal: exports reach it through `PublicIdentityFirstContext::runtime()`.
    pub(crate) runtime: Arc<IdentityRuntime>,
    pub roster_provider: Arc<dyn RosterProvider>,
    pub topology_provider: Option<Arc<dyn TopologyProvider>>,
    pub customizer: Option<Arc<dyn AgentCustomizer>>,
    mob_definition: Option<meerkat_mob::MobDefinition>,
    bootstrap_mode: IdentityBootstrapMode,
}

/// Why `spec`'s member cannot be held, if it cannot: only a fresh, locally
/// created member whose first turn waits can be.
fn held_spec_refusal(
    spec: &DurableAgentSpec,
) -> Option<super::held_bootstrap::HeldBootstrapRefusal> {
    use super::held_bootstrap::HeldBootstrapRefusal as Refusal;
    let identity = spec.identity.clone();
    if spec.placement.is_some() {
        return Some(Refusal::Placed { identity });
    }
    if matches!(spec.backend, Some(meerkat_mob::MobBackendKind::External))
        || matches!(
            spec.binding.as_ref(),
            Some(meerkat_contracts::WireRuntimeBinding::External { .. })
        )
    {
        return Some(Refusal::ExternallyBound { identity });
    }
    if spec.initial_message.is_some() {
        return Some(Refusal::InitialMessage { identity });
    }
    None
}

impl IdentityFirstRuntimeContext {
    /// The context's identity runtime, borrowed: callers reach its methods
    /// without ever holding an ownerless `Arc` (D67).
    pub fn runtime(&self) -> &IdentityRuntime {
        &self.runtime
    }

    pub(crate) async fn topology_snapshot_inputs(
        &self,
    ) -> Result<(Vec<DurableAgentSpec>, Vec<ManagedPeerEdge>), IdentityRuntimeError> {
        let previous_identities = self.runtime.registered_identities().await;
        let roster = self
            .roster_provider
            .roster(&RosterContext {
                mob_definition: self.mob_definition.clone(),
                previous_identities,
            })
            .await
            .map_err(|error| IdentityRuntimeError::Internal(format!("roster provider: {error}")))?;
        let identities = roster
            .iter()
            .map(|spec| spec.identity.clone())
            .collect::<Vec<_>>();
        let declared = match self.topology_provider.as_deref() {
            Some(provider) => provider
                .compute_edges(
                    &identities,
                    &TopologyContext {
                        roster: roster.clone(),
                    },
                )
                .await
                .map_err(|error| {
                    IdentityRuntimeError::Internal(format!("topology provider: {error}"))
                })?,
            None => self.runtime.desired_peer_edges.read().await.clone(),
        };
        Ok((roster, declared))
    }

    pub fn new(
        runtime: Arc<IdentityRuntime>,
        roster_provider: Arc<dyn RosterProvider>,
        topology_provider: Option<Arc<dyn TopologyProvider>>,
        customizer: Option<Arc<dyn AgentCustomizer>>,
        mob_definition: Option<meerkat_mob::MobDefinition>,
    ) -> Self {
        Self::new_with_lazy_materialization(
            runtime,
            roster_provider,
            topology_provider,
            customizer,
            mob_definition,
            false,
        )
    }

    pub fn new_with_lazy_materialization(
        runtime: Arc<IdentityRuntime>,
        roster_provider: Arc<dyn RosterProvider>,
        topology_provider: Option<Arc<dyn TopologyProvider>>,
        customizer: Option<Arc<dyn AgentCustomizer>>,
        mob_definition: Option<meerkat_mob::MobDefinition>,
        lazy_materialization: bool,
    ) -> Self {
        Self::new_with_bootstrap_mode(
            runtime,
            roster_provider,
            topology_provider,
            customizer,
            mob_definition,
            if lazy_materialization {
                IdentityBootstrapMode::LazyMaterialize
            } else {
                IdentityBootstrapMode::EagerMaterialize
            },
        )
    }

    /// Construct a context that preserves the complete startup policy across
    /// later roster reconciliation.
    pub fn new_with_bootstrap_mode(
        runtime: Arc<IdentityRuntime>,
        roster_provider: Arc<dyn RosterProvider>,
        topology_provider: Option<Arc<dyn TopologyProvider>>,
        customizer: Option<Arc<dyn AgentCustomizer>>,
        mob_definition: Option<meerkat_mob::MobDefinition>,
        bootstrap_mode: IdentityBootstrapMode,
    ) -> Self {
        runtime.set_reset_roster_provider_context(
            Some(roster_provider.clone()),
            mob_definition.clone(),
        );
        runtime.set_identity_edge_ownership(IdentityEdgeOwnership::for_topology_provider(
            topology_provider.is_some(),
        ));
        Self {
            runtime,
            roster_provider,
            topology_provider,
            customizer,
            mob_definition,
            bootstrap_mode,
        }
    }

    pub fn bootstrap_mode(&self) -> &IdentityBootstrapMode {
        &self.bootstrap_mode
    }

    /// Apply the configured bootstrap policy to an already-resolved roster.
    /// The same helper is used at startup and during reconcile so a lazy
    /// deployment can never accidentally hydrate the full fleet.
    pub async fn bootstrap_roster(
        &self,
        roster: &[DurableAgentSpec],
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError> {
        let mut retired = Vec::new();
        let result = async {
            let _controller = self.runtime.bootstrap_controller.lock().await;
            let generation = self
                .runtime
                .begin_identity_bootstrap_pending(self.bootstrap_mode.clone());
            if let Err(error) = self.prepare_controlled_bootstrap().await {
                self.runtime.fail_identity_bootstrap(generation, &error);
                return Err(error);
            }
            self.apply_roster_controlled(generation, roster, &mut retired)
                .await
        }
        .await;
        // The pass's retirements reach the host's retirement hook after the
        // bootstrap controller is released, whatever the pass returned.
        self.runtime.release_retired_members(retired).await;
        result
    }

    /// [`Self::bootstrap_roster`] with every freshly created roster member
    /// held before its publication (see [`super::held_bootstrap`]). Once each
    /// roster identity is held, `complete` receives the held members and the
    /// pass's [`super::held_bootstrap::HeldCompletion`]. The pass's one
    /// terminal decision comes deadline first, then from the caller's loss,
    /// then from `complete`. An `Ok` recorded before the setup deadline is
    /// the pass's choice `Activate`: Core then accepts or expires each
    /// member's own activation against the same deadline, so each member
    /// keeps its actual outcome (an expired one is `Broken`, and the choice
    /// stays `Activate`). Anything else aborts each member (its embodiment
    /// fails once Core has settled the member, which Core never publishes)
    /// and fails the bootstrap. Only the eager bootstrap mode can hold
    /// its members.
    ///
    /// The call is bounded by the setup deadline: at a terminal `Abort` it
    /// returns [`super::held_bootstrap::HeldBootstrapStart::Unsettled`] at
    /// once, while the identity runtime's tracked foreground operation keeps
    /// owning the members' aborts (each until Core has settled it), the join of
    /// a still running `complete` and the marking of each member the pass did
    /// not publish as owing a fresh held completion. After the pass's choice
    /// `Activate` it waits for the roster pass, through each member's actual
    /// activation or expiry, and returns its per-member result.
    pub async fn bootstrap_roster_held<F, Fut>(
        self: &Arc<Self>,
        roster: &[DurableAgentSpec],
        deadline: meerkat_mob::HeldSpawnDeadline,
        complete: F,
    ) -> Result<super::held_bootstrap::HeldBootstrapStart, IdentityRuntimeError>
    where
        F: FnOnce(
                Vec<super::held_bootstrap::HeldIdentityMember>,
                super::held_bootstrap::HeldCompletion,
            ) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        self.bootstrap_roster_held_after(
            roster,
            Arc::new(super::held_bootstrap::HeldTerminal::new(deadline)),
            None,
            complete,
        )
        .await
    }

    /// [`Self::bootstrap_roster_held`] under `terminal`, the setup's one
    /// terminal decision, which the caller made before anything of the setup
    /// awaited. `prelude` (a prepared mob's activation) runs first in the
    /// same tracked operation, so the deadline bounds it too.
    pub(crate) async fn bootstrap_roster_held_after<F, Fut>(
        self: &Arc<Self>,
        roster: &[DurableAgentSpec],
        terminal: Arc<super::held_bootstrap::HeldTerminal>,
        prelude: Option<super::held_bootstrap::HeldSetupPrelude>,
        complete: F,
    ) -> Result<super::held_bootstrap::HeldBootstrapStart, IdentityRuntimeError>
    where
        F: FnOnce(
                Vec<super::held_bootstrap::HeldIdentityMember>,
                super::held_bootstrap::HeldCompletion,
            ) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let context = Arc::clone(self);
        let roster = roster.to_vec();
        self.start_held_setup(terminal, prelude, move |terminal, abandonment| async move {
            context
                .bootstrap_roster_held_owned(&roster, terminal, complete, abandonment)
                .await
        })
        .await
    }

    /// Add one fresh member to the running roster, held at its producer
    /// hold point until the host has completed its application from the
    /// member's finished surface, while every other member keeps running.
    ///
    /// The one-member counterpart of [`Self::bootstrap_roster_held`]: the
    /// same bootstrap controller, the same one terminal decision (deadline
    /// first, then the caller's loss, then `complete`), the same tracked
    /// settlement and the same [`super::held_bootstrap::HeldBootstrapStart`],
    /// whose activated result carries this member's outcome only. It
    /// registers and embodies `spec`'s identity alone: it never reconciles
    /// the roster, never changes the managed peer edges, and runs in either
    /// bootstrap mode (in lazy mode, too, it embodies the member at once).
    /// The identity must be fresh as a held roster bootstrap reads it: Core
    /// never spawned it since the latest mob reset and no first-boot row
    /// names it
    /// ([`super::held_bootstrap::HeldBootstrapRefusal::ExistingContinuity`]
    /// otherwise; a failed read of Core's member history is never taken as
    /// fresh). An identity Core holds an identity intent row for is the
    /// host's and is refused with
    /// [`IdentityRuntimeError::IdentityHostAdopted`]. A placed, externally
    /// bound or initial-message member is refused, as in a held bootstrap. A member that settles without being
    /// held (embodied outside the add after its freshness read) refuses the
    /// add as `NotHeld`, and the host is never asked.
    ///
    /// The roster provider stays the owner of the desired roster, and the
    /// next roster pass retires a member its result does not list. List the
    /// spec in the provider from inside `complete`: the add holds the
    /// bootstrap controller until it ends, so no pass of the attached
    /// context reads the listing before the member is held and completed. A
    /// spec listed before the add can be read by a pass the host did not
    /// start (a `meta/reconcile_identity`),
    /// which would embody the member unheld. Three paths run outside the
    /// controller, and so outside that guarantee: `meta/reconcile_identity`
    /// on an RPC context that is not attached to the unified runtime reads
    /// the roster and restores it with no controller;
    /// [`IdentityRuntime::restore_flow`] registers and wakes the roster its
    /// caller gives, with no controller (it retires nothing); and a host's
    /// own [`Self::bootstrap_roster`] applies the roster it is given, so a
    /// roster resolved before the add and applied after it retires the
    /// member. When the add ends
    /// [`super::held_bootstrap::HeldBootstrapStart::Unsettled`], unlist the
    /// spec, or add it again: the aborted member owes its host a held
    /// completion, and every pass except a held pass that covers it
    /// refuses it ([`IdentityRuntimeError::HeldCompletionOwed`], whose
    /// structured data is `{"kind": "held_completion_owed", "identity"}`).
    /// A caller that goes away while `complete` runs after listing never
    /// sees `Unsettled`: the add aborts and the member stays listed and
    /// refused, so the host unlists it, or adds it again, on its recovery
    /// path. Refresh the
    /// topology afterwards when the topology provider wires the member.
    ///
    /// The add publishes no bootstrap generation: until the next roster
    /// pass, [`IdentityRuntime::identity_bootstrap_status`] does not list
    /// the added member, and its counts and `ready` do not cover it. The
    /// add's own result is its outcome.
    pub async fn add_member_held<F, Fut>(
        self: &Arc<Self>,
        spec: DurableAgentSpec,
        deadline: meerkat_mob::HeldSpawnDeadline,
        complete: F,
    ) -> Result<super::held_bootstrap::HeldBootstrapStart, IdentityRuntimeError>
    where
        F: FnOnce(
                Vec<super::held_bootstrap::HeldIdentityMember>,
                super::held_bootstrap::HeldCompletion,
            ) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        let context = Arc::clone(self);
        self.start_held_setup(
            Arc::new(super::held_bootstrap::HeldTerminal::new(deadline)),
            None,
            move |terminal, abandonment| async move {
                context
                    .add_member_held_owned(spec, terminal, complete, abandonment)
                    .await
            },
        )
        .await
    }

    /// Start a held setup (`prelude`, then `setup`) in the identity
    /// runtime's tracked foreground owner, bounded by `terminal`, and wait
    /// for its terminal decision.
    async fn start_held_setup<S, SFut>(
        &self,
        terminal: Arc<super::held_bootstrap::HeldTerminal>,
        prelude: Option<super::held_bootstrap::HeldSetupPrelude>,
        setup: S,
    ) -> Result<super::held_bootstrap::HeldBootstrapStart, IdentityRuntimeError>
    where
        S: FnOnce(
                Arc<super::held_bootstrap::HeldTerminal>,
                super::held_bootstrap::HeldAbandonment,
            ) -> SFut
            + Send
            + 'static,
        SFut: std::future::Future<
                Output = Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError>,
            > + Send
            + 'static,
    {
        // The setup runs in the identity runtime's tracked foreground owner,
        // not in the caller's future. A caller that goes away before the
        // pass's terminal decision abandons it: the owned task aborts every
        // held member, joins each identity transaction's own cleanup, clears
        // the pass and settles the bootstrap generation. Once `Activate` has
        // been recorded, abandonment changes nothing and nothing is rolled
        // back retroactively.
        use super::held_bootstrap::{
            AbandonOnDrop, HeldAbandonment, HeldBootstrapSettlement, HeldBootstrapStart,
        };
        let abandonment = HeldAbandonment::new();
        // The caller's loss before the start returns is the setup's terminal
        // `Abort`: whatever setup step is entered then runs to its end, and
        // no later one starts.
        let guard = AbandonOnDrop::new(abandonment.clone(), Arc::clone(&terminal));
        let owned_terminal = Arc::clone(&terminal);
        // Admission into the tracked owner is bounded by the same terminal
        // decision (deadline first) and by the runtime's closure: the start
        // never queues behind a shutdown's join of unrelated operations.
        // Nothing of the setup has run before it is admitted.
        let mut result = self
            .runtime
            .start_tracked_foreground_within(
                ForegroundOwnerSource::Acquire,
                async move {
                    if let Some(prelude) = prelude {
                        prelude.await?;
                    }
                    setup(owned_terminal, abandonment).await
                },
                async {
                    terminal.aborted().await;
                    IdentityRuntimeError::HeldBootstrapRefused(terminal.abort_refusal())
                },
            )
            .await?;
        // The caller waits for the pass's terminal decision (deadline first),
        // never for the settlement of an aborted setup: that stays with the
        // tracked operation, observed through the returned settlement.
        let settled = tokio::select! {
            biased;
            () = terminal.aborted() => None,
            settled = &mut result => Some(settled),
        };
        guard.settled();
        match settled {
            None => Ok(HeldBootstrapStart::Unsettled(HeldBootstrapSettlement::new(
                result,
            ))),
            Some(Ok(Ok(restored))) => Ok(HeldBootstrapStart::Activated(restored)),
            Some(Ok(Err(error))) => Err(error),
            Some(Err(_)) => Err(IdentityRuntimeError::Internal(
                "tracked foreground identity operation terminated without a result".to_string(),
            )),
        }
    }

    async fn bootstrap_roster_held_owned<F, Fut>(
        &self,
        roster: &[DurableAgentSpec],
        terminal: Arc<super::held_bootstrap::HeldTerminal>,
        complete: F,
        abandonment: super::held_bootstrap::HeldAbandonment,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError>
    where
        F: FnOnce(
            Vec<super::held_bootstrap::HeldIdentityMember>,
            super::held_bootstrap::HeldCompletion,
        ) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
    {
        let mut retired = Vec::new();
        let result = self
            .bootstrap_roster_held_controlled(roster, terminal, complete, abandonment, &mut retired)
            .await;
        // The pass's retirements reach the host's retirement hook after the
        // bootstrap controller is released, whatever the pass returned.
        self.runtime.release_retired_members(retired).await;
        result
    }

    async fn bootstrap_roster_held_controlled<F, Fut>(
        &self,
        roster: &[DurableAgentSpec],
        terminal: Arc<super::held_bootstrap::HeldTerminal>,
        complete: F,
        abandonment: super::held_bootstrap::HeldAbandonment,
        retired: &mut Vec<super::types::RetiredIdentityMember>,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError>
    where
        F: FnOnce(
            Vec<super::held_bootstrap::HeldIdentityMember>,
            super::held_bootstrap::HeldCompletion,
        ) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
    {
        use super::held_bootstrap::HeldBootstrapRefusal as Refusal;
        use super::held_bootstrap::{HeldCreatePass, HeldDecision};
        let refused = |refusal: Refusal| IdentityRuntimeError::HeldBootstrapRefused(refusal);
        if self.bootstrap_mode.is_lazy() {
            return Err(refused(Refusal::LazyMode));
        }
        // Only fresh, locally created, deferred members can be held: refuse
        // a roster with any other member rather than publish it unheld.
        if let Some(refusal) = roster.iter().find_map(held_spec_refusal) {
            return Err(refused(refusal));
        }
        // A setup whose terminal decision is already `Abort` (its deadline
        // passed, or its caller went away, while the setup before this pass
        // ran) starts no bootstrap generation and no member.
        if terminal.current() == Some(HeldDecision::Abort) {
            return Err(refused(terminal.abort_refusal()));
        }
        // The freshness decision is made under the runtime's one bootstrap
        // controller, as part of the controlled pass itself, so no other
        // controlled bootstrap can embody a roster member in between. A
        // member embodied outside the controller after this read (a lazy
        // materialization) is caught where the pass accounts for it: it
        // settles without being held, and the pass refuses (`NotHeld`).
        let _controller = self.runtime.bootstrap_controller.lock().await;
        let generation = self
            .runtime
            .begin_identity_bootstrap_pending(self.bootstrap_mode.clone());
        if let Err(error) = self.prepare_controlled_bootstrap().await {
            self.runtime.fail_identity_bootstrap(generation, &error);
            return Err(error);
        }
        // Fresh means Core never spawned the identity (no binding since
        // the latest mob reset) and no first-boot row names a session for
        // it. A read fault is never taken as fresh.
        let identities: Vec<_> = roster.iter().map(|spec| spec.identity.clone()).collect();
        for spec in roster {
            let existing = match self.runtime.core_last_binding(&spec.identity).await {
                Ok(existing) => existing,
                Err(error) => {
                    let error = IdentityRuntimeError::Internal(format!(
                        "held identity bootstrap could not read Core's member history: {error}"
                    ));
                    self.runtime.fail_identity_bootstrap(generation, &error);
                    return Err(error);
                }
            };
            let first_boot_row = match self.runtime.pending_first_boot_row(&spec.identity).await {
                Ok(row) => row,
                Err(error) => {
                    self.runtime.fail_identity_bootstrap(generation, &error);
                    return Err(error);
                }
            };
            if existing.is_some() || first_boot_row.is_some() {
                let error = refused(Refusal::ExistingContinuity {
                    identity: spec.identity.clone(),
                });
                self.runtime.fail_identity_bootstrap(generation, &error);
                return Err(error);
            }
        }
        self.run_held_after_freshness_hook().await;
        // A setup whose terminal decision is already `Abort` (its deadline
        // passed, or its caller went away, while the controller and the
        // roster's continuity were read) starts no member.
        if terminal.current() == Some(HeldDecision::Abort) {
            let error = refused(terminal.abort_refusal());
            self.runtime.fail_identity_bootstrap(generation, &error);
            return Err(error);
        }
        let pass = Arc::new(HeldCreatePass::new(
            terminal,
            identities.clone(),
            super::orchestrator::identity_restore_concurrency(),
        ));
        if let Err(error) = self.install_held_create_pass(&pass) {
            self.runtime.fail_identity_bootstrap(generation, &error);
            return Err(error);
        }
        // Every roster identity owes its host this pass's completion until
        // the pass publishes its member. Core keeps nothing for a member
        // before its activation, so the marks are written durably before any
        // member exists: a process that stops while a member is held keeps
        // them.
        if let Err(error) = self.runtime.mark_held_completion_owed(&identities).await {
            *self
                .runtime
                .held_create_pass
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            self.runtime.fail_identity_bootstrap(generation, &error);
            return Err(error);
        }
        self.join_held_pass(
            &pass,
            abandonment,
            complete,
            self.apply_roster_controlled(generation, roster, retired),
        )
        .await
    }

    async fn add_member_held_owned<F, Fut>(
        &self,
        spec: DurableAgentSpec,
        terminal: Arc<super::held_bootstrap::HeldTerminal>,
        complete: F,
        abandonment: super::held_bootstrap::HeldAbandonment,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError>
    where
        F: FnOnce(
            Vec<super::held_bootstrap::HeldIdentityMember>,
            super::held_bootstrap::HeldCompletion,
        ) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
    {
        use super::held_bootstrap::HeldBootstrapRefusal as Refusal;
        use super::held_bootstrap::{HeldCreatePass, HeldDecision};
        let refused = |refusal: Refusal| IdentityRuntimeError::HeldBootstrapRefused(refusal);
        if let Some(refusal) = held_spec_refusal(&spec) {
            return Err(refused(refusal));
        }
        if terminal.current() == Some(HeldDecision::Abort) {
            return Err(refused(terminal.abort_refusal()));
        }
        // The freshness decision is made under the runtime's one bootstrap
        // controller, so no roster pass and no other held setup runs between
        // it and the member's held creation. A member embodied outside the
        // controller after this read (a lazy materialization) settles
        // without being held, and the add refuses (`NotHeld`).
        let _controller = self.runtime.bootstrap_controller.lock().await;
        if self.runtime.bootstrap_shutdown.load(Ordering::Acquire) {
            return Err(IdentityRuntimeError::Internal(
                "identity bootstrap is shutting down".to_string(),
            ));
        }
        let identity = spec.identity.clone();
        // An identity Core holds an identity intent row for is the host's:
        // no Meta verb adds it.
        self.runtime.refuse_host_adopted(&identity).await?;
        // Fresh means what it means for a held roster bootstrap: Core never
        // spawned the identity (no binding since the latest mob reset) and no
        // first-boot row names a session for it. A read fault is never taken
        // as fresh.
        let existing = self
            .runtime
            .core_last_binding(&identity)
            .await
            .map_err(|error| {
                IdentityRuntimeError::Internal(format!(
                    "held member add could not read Core's member history: {error}"
                ))
            })?;
        let first_boot_row = self.runtime.pending_first_boot_row(&identity).await?;
        if existing.is_some() || first_boot_row.is_some() {
            return Err(refused(Refusal::ExistingContinuity { identity }));
        }
        self.run_held_after_freshness_hook().await;
        // A terminal `Abort` recorded while the controller and the
        // continuity were read starts no member.
        if terminal.current() == Some(HeldDecision::Abort) {
            return Err(refused(terminal.abort_refusal()));
        }
        let pass = Arc::new(HeldCreatePass::new(terminal, [identity.clone()], 1));
        self.install_held_create_pass(&pass)?;
        // The added member owes its host this add's completion until the
        // add publishes it, durably and before the member exists, as for a
        // held roster pass. The add opened no bootstrap generation.
        if let Err(error) = self.runtime.mark_held_completion_owed(&[identity]).await {
            *self
                .runtime
                .held_create_pass
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
            return Err(error);
        }
        self.join_held_pass(
            &pass,
            abandonment,
            complete,
            super::orchestrator::add_member_flow(&self.runtime, &spec, self.customizer.as_deref()),
        )
        .await
    }

    /// Test seam: run the hook a test set to run right after a held
    /// setup's freshness read.
    async fn run_held_after_freshness_hook(&self) {
        #[cfg(test)]
        {
            let hook = self
                .runtime
                .held_after_freshness_hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(hook) = hook {
                hook().await;
            }
        }
    }

    /// Make `pass` the runtime's one held pass, or refuse while another one
    /// is in progress.
    fn install_held_create_pass(
        &self,
        pass: &Arc<super::held_bootstrap::HeldCreatePass>,
    ) -> Result<(), IdentityRuntimeError> {
        let mut slot = self
            .runtime
            .held_create_pass
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_some() {
            return Err(IdentityRuntimeError::Internal(
                "a held identity bootstrap is already in progress".to_string(),
            ));
        }
        *slot = Some(Arc::clone(pass));
        Ok(())
    }

    /// Run an installed held pass to its end: `body` embodies the pass's
    /// members while the producer waits until every one of them is held,
    /// asks `complete` and records the pass's one terminal decision. Both
    /// are joined, never dropped, and the runtime's held pass is cleared
    /// once they have settled.
    async fn join_held_pass<F, Fut, B>(
        &self,
        pass: &super::held_bootstrap::HeldCreatePass,
        abandonment: super::held_bootstrap::HeldAbandonment,
        complete: F,
        body: B,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError>
    where
        F: FnOnce(
            Vec<super::held_bootstrap::HeldIdentityMember>,
            super::held_bootstrap::HeldCompletion,
        ) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
        B: std::future::Future<
                Output = Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError>,
            >,
    {
        use super::held_bootstrap::HeldBootstrapRefusal as Refusal;
        use super::held_bootstrap::{HeldCompletion, HeldCompletionOutcome, deadline_reached};
        let refused = |refusal: Refusal| IdentityRuntimeError::HeldBootstrapRefused(refusal);
        let bootstrap = async {
            let result = body.await;
            pass.end();
            result
        };
        let producer = async {
            // Every member held, deadline first (also when all are already
            // held), or the caller's loss.
            let held = tokio::select! {
                biased;
                held = pass.wait_held() => held,
                () = abandonment.abandoned() => Err(Refusal::CallerAbandoned),
            };
            let members = match held {
                // A caller lost meanwhile already recorded the terminal
                // `Abort`: the host is not asked.
                Ok(_) if pass.terminal().is_aborted() => {
                    return Err((
                        pass.terminal().abort_refusal(),
                        HeldCompletionOutcome::NotAsked,
                    ));
                }
                Ok(members) => members,
                Err(refusal) => {
                    return Err((
                        pass.terminal().abort_with(refusal),
                        HeldCompletionOutcome::NotAsked,
                    ));
                }
            };
            let mut completion =
                std::pin::pin!(complete(members, HeldCompletion::new(pass.terminal())));
            // The host's answer, deadline first by the clock, then the
            // caller's loss. Recording `Activate` before the deadline
            // linearizes the pass's choice, not each member's native
            // activation (Core accepts or expires that per member): an `Ok`
            // recorded at or after it is an `Abort`, and the callback's
            // settlement is kept apart.
            let ended = tokio::select! {
                biased;
                () = deadline_reached(pass.deadline()) => Refusal::DeadlineElapsed,
                () = abandonment.abandoned() => Refusal::CallerAbandoned,
                answered = &mut completion => return pass.terminal().settle_answer(answered),
            };
            // The pass ended before the host answered: its terminal decision
            // is `Abort` (the members abort meanwhile), under the cause that
            // recorded it first, and the host's still running callback is
            // joined here, never dropped, so its actual settlement is what
            // gets recorded.
            let ended = pass.terminal().abort_with(ended);
            let settled = match completion.await {
                Ok(()) => HeldCompletionOutcome::Completed,
                Err(reason) => HeldCompletionOutcome::Refused(reason),
            };
            Err((ended, settled))
        };
        let (result, produced) = futures::join!(bootstrap, producer);
        *self
            .runtime
            .held_create_pass
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        match produced {
            Ok(()) => result,
            Err((refusal, completion)) => {
                if let Err(error) = &result {
                    tracing::warn!(
                        %error,
                        "the held identity bootstrap pass also failed after its refusal"
                    );
                }
                Err(match completion {
                    HeldCompletionOutcome::NotAsked => refused(refusal),
                    completion => IdentityRuntimeError::HeldBootstrapAborted {
                        refusal,
                        completion,
                    },
                })
            }
        }
    }

    /// Apply a roster under the runtime's single bootstrap controller.
    ///
    /// Startup and the public refresh API now share ONE restore path. The
    /// former split (bootstrap skipped snapshot payloads for opted-out
    /// bridges, public refresh always loaded them) meant console add-member
    /// and every reconcile pass paid a full session-blob read per Ready
    /// member for a payload nothing reads. `restore_flow` owns the single
    /// read-on-need rule; see its docs.
    async fn prepare_controlled_bootstrap(&self) -> Result<(), IdentityRuntimeError> {
        if self.runtime.bootstrap_shutdown.load(Ordering::Acquire) {
            return Err(IdentityRuntimeError::Internal(
                "identity bootstrap is shutting down".to_string(),
            ));
        }
        // Stop admitting new warm items and let any materialization already in
        // flight reach a transaction boundary before applying the next roster.
        self.runtime.request_identity_bootstrap_stop();
        self.runtime.join_identity_bootstrap_task().await;
        if self.runtime.bootstrap_shutdown.load(Ordering::Acquire) {
            return Err(IdentityRuntimeError::Internal(
                "identity bootstrap is shutting down".to_string(),
            ));
        }
        Ok(())
    }

    /// One roster compile pass: one attempt per identity, no retry
    /// loop and no timer. A refused identity stays refused, typed, until the
    /// next pass (boot or a roster refresh) or a direct wake asks Core again.
    async fn apply_roster_controlled(
        &self,
        generation: u64,
        roster: &[DurableAgentSpec],
        retired: &mut Vec<super::types::RetiredIdentityMember>,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError> {
        self.apply_roster_pass(generation, roster, retired).await
    }

    async fn apply_roster_pass(
        &self,
        generation: u64,
        roster: &[DurableAgentSpec],
        retired: &mut Vec<super::types::RetiredIdentityMember>,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError> {
        if let Err(message) = self.bootstrap_mode.validate() {
            let error = IdentityRuntimeError::Internal(message);
            self.runtime
                .begin_identity_bootstrap(generation, self.bootstrap_mode.clone(), roster)
                .await;
            self.runtime.fail_identity_bootstrap(generation, &error);
            return Err(error);
        }
        // Publish the new pass before any provider/restore await. RPC dispatch
        // is concurrent, so retaining the previous complete/ready snapshot here
        // would let a readiness waiter falsely pass while reconcile is active.
        self.runtime
            .begin_identity_bootstrap(generation, self.bootstrap_mode.clone(), roster)
            .await;
        // Converge the physical runtime before either restore policy publishes
        // the new roster. The restore flows only iterate desired identities;
        // without this boundary a removed member can remain live (and leased)
        // while the reduced bootstrap snapshot reports ready. Likewise, an
        // already-active member must not keep running an old build after its
        // desired spec changes.
        if let Err(error) = self
            .runtime
            .reconcile_roster_members(roster, generation, retired)
            .await
        {
            self.runtime.fail_identity_bootstrap(generation, &error);
            return Err(error);
        }
        // One eager path for both startup and refresh: the snapshot-policy
        // twin is gone, so there is nothing left for the caller to select.
        let result = match &self.bootstrap_mode {
            IdentityBootstrapMode::EagerMaterialize => {
                super::orchestrator::restore_flow(
                    &self.runtime,
                    roster,
                    self.topology_provider.as_deref(),
                    self.customizer.as_deref(),
                )
                .await
            }
            IdentityBootstrapMode::LazyMaterialize
            | IdentityBootstrapMode::LazyWithBackgroundWarm { .. } => {
                super::orchestrator::lazy_register_flow(
                    &self.runtime,
                    roster,
                    self.topology_provider.as_deref(),
                )
                .await
            }
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.runtime.fail_identity_bootstrap(generation, &error);
                return Err(error);
            }
        };
        if self.runtime.bootstrap_shutdown.load(Ordering::Acquire) {
            let error =
                IdentityRuntimeError::Internal("identity bootstrap is shutting down".to_string());
            self.runtime.fail_identity_bootstrap(generation, &error);
            return Err(error);
        }
        self.runtime
            .install_identity_bootstrap(generation, self.bootstrap_mode.clone(), roster, &result)
            .await;
        Ok(result)
    }

    pub async fn refresh_desired_topology(
        &self,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError> {
        self.refresh_desired_topology_from().await
    }

    async fn refresh_desired_topology_from(
        &self,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError> {
        let mut retired = Vec::new();
        let result = self.refresh_desired_topology_controlled(&mut retired).await;
        // The pass's retirements reach the host's retirement hook after the
        // bootstrap controller is released, whatever the pass returned, so
        // a hook may itself refresh the roster.
        self.runtime.release_retired_members(retired).await;
        result
    }

    async fn refresh_desired_topology_controlled(
        &self,
        retired: &mut Vec<super::types::RetiredIdentityMember>,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError> {
        // One runtime owns one controller from roster discovery through task
        // installation. Provider calls are user code and can be slow; publish
        // the in-flight pass before awaiting them so concurrent readiness RPCs
        // cannot observe the previous terminal snapshot.
        let _controller = self.runtime.bootstrap_controller.lock().await;
        let generation = self
            .runtime
            .begin_identity_bootstrap_pending(self.bootstrap_mode.clone());
        if let Err(error) = self.prepare_controlled_bootstrap().await {
            self.runtime.fail_identity_bootstrap(generation, &error);
            return Err(error);
        }
        let roster = match self
            .roster_provider
            .roster(&RosterContext {
                mob_definition: self.mob_definition.clone(),
                previous_identities: Vec::new(),
            })
            .await
        {
            Ok(roster) => roster,
            Err(err) => {
                let error = IdentityRuntimeError::Internal(format!("roster provider: {err}"));
                self.runtime.fail_identity_bootstrap(generation, &error);
                return Err(error);
            }
        };

        self.apply_roster_controlled(generation, &roster, retired)
            .await
    }

    /// Cancellation-safe reconcile for RPC/host request boundaries.
    /// Dropping the caller only drops its result receiver; the runtime owns
    /// the pass until every acquired lease and bridge mutation reaches an
    /// explicit commit or rollback boundary.
    pub async fn refresh_desired_topology_tracked(
        self: &Arc<Self>,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError> {
        let context = Arc::clone(self);
        let runtime = Arc::clone(&self.runtime);
        runtime
            .run_tracked_foreground(async move { context.refresh_desired_topology().await })
            .await
    }
}

/// Weak keyed locks serialize claims on the shared raw/durable alias
/// namespace without retaining every caller-chosen alias for the lifetime of
/// the runtime. Sweeps grow geometrically while many aliases are live (as in
/// fleet bootstrap), avoiding a full-map scan for every new roster member.
struct RawMemberAliasLockTable {
    entries: BTreeMap<String, Weak<Mutex<()>>>,
    next_sweep_len: usize,
    #[cfg(test)]
    sweep_count: usize,
}

impl Default for RawMemberAliasLockTable {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_sweep_len: RAW_MEMBER_ALIAS_LOCK_SWEEP_MIN,
            #[cfg(test)]
            sweep_count: 0,
        }
    }
}

impl RawMemberAliasLockTable {
    fn sweep_if_needed(&mut self) {
        if self.entries.len() < self.next_sweep_len {
            return;
        }
        self.entries.retain(|_, lock| lock.upgrade().is_some());
        self.next_sweep_len = self
            .entries
            .len()
            .saturating_mul(2)
            .max(RAW_MEMBER_ALIAS_LOCK_SWEEP_MIN);
        #[cfg(test)]
        {
            self.sweep_count += 1;
        }
    }
}

/// The identity-first runtime tracks active identities and enforces delivery,
/// ownership, and lifecycle invariants.
/// Observes a live member moving from one session to another while it stays
/// the same member incarnation: the continuity rebind after a respawn or
/// after a delivery repair that rotated the member's session.
#[async_trait::async_trait]
pub(crate) trait MemberSessionRotationObserver: Send + Sync {
    /// `member` is the member's mob roster identity.
    async fn member_session_rotated(
        &self,
        member: &meerkat_mob::ids::AgentIdentity,
        previous: &SessionId,
        current: &SessionId,
    );
}

/// Run completions of a member's actor that already happened but are not
/// counted yet: the session's events that meerkat retains past the point the
/// identity health monitor credited. [`IdentityRuntime::completion_cursor`]
/// credits them before every read.
#[async_trait::async_trait]
pub(crate) trait PendingCompletionDrain: Send + Sync {
    /// Consume the uncounted events of `session_id` published so far and
    /// return the run terminals among them.
    async fn drain(&self, session_id: &SessionId) -> PendingRunTerminals;
}

/// Run terminals a [`PendingCompletionDrain`] consumed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PendingRunTerminals {
    /// Completed runs.
    pub(crate) completed: u64,
    /// Failed (or cancelled) runs.
    pub(crate) failed: u64,
}

/// The identity table, with a change signal: releasing a write lock, or a
/// verb of this runtime moving an identity's Core member binding
/// ([`Self::notify_binding_changed`]), wakes the completion waiters
/// ([`IdentityRuntime::await_completion`]), which re-read their cursor and
/// Core's fence. The signal carries no state and is never read as an event;
/// a wake only means "re-read now".
struct IdentityEntries {
    table: RwLock<BTreeMap<AgentIdentity, IdentityEntry>>,
    changed: watch::Sender<()>,
    /// Told (an epoch bump) whenever a verb of this runtime woke, retired or
    /// replaced an identity's Core member. The identity health monitor
    /// attaches a member's stream only once Core holds the member, so this is
    /// its typed wake for that moment
    /// ([`IdentityRuntime::install_member_binding_observer`]). A member Core
    /// changes on its own (a host's reconciler) is found on the monitor's
    /// next read, as before.
    binding_observer: StdRwLock<Option<watch::Sender<u64>>>,
    /// Bumped by every [`Self::notify_binding_changed`]: a managed-topology
    /// commit refuses when a verb of this runtime moved a member between its
    /// revalidation and its commit.
    binding_sequence: AtomicU64,
}

impl IdentityEntries {
    fn new() -> Self {
        Self {
            table: RwLock::new(BTreeMap::new()),
            changed: watch::channel(()).0,
            binding_observer: StdRwLock::new(None),
            binding_sequence: AtomicU64::new(0),
        }
    }

    async fn read(
        &self,
    ) -> tokio::sync::RwLockReadGuard<'_, BTreeMap<AgentIdentity, IdentityEntry>> {
        self.table.read().await
    }

    async fn write(&self) -> IdentityEntriesWriteGuard<'_> {
        IdentityEntriesWriteGuard {
            guard: self.table.write().await,
            entries: self,
        }
    }

    /// Wake every waiter subscribed through [`Self::subscribe`].
    fn notify(&self) {
        self.changed.send_replace(());
    }

    fn subscribe(&self) -> watch::Receiver<()> {
        self.changed.subscribe()
    }

    /// A verb of this runtime moved an identity's Core member (woke, retired
    /// or replaced it): wake the completion waiters and the binding observer.
    fn notify_binding_changed(&self) {
        self.binding_sequence.fetch_add(1, Ordering::AcqRel);
        self.notify();
        if let Some(observer) = self
            .binding_observer
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            observer.send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        }
    }

    /// How many member moves of this runtime's verbs were signalled so far.
    fn binding_sequence(&self) -> u64 {
        self.binding_sequence.load(Ordering::Acquire)
    }
}

struct IdentityEntriesWriteGuard<'a> {
    guard: tokio::sync::RwLockWriteGuard<'a, BTreeMap<AgentIdentity, IdentityEntry>>,
    entries: &'a IdentityEntries,
}

impl std::ops::Deref for IdentityEntriesWriteGuard<'_> {
    type Target = BTreeMap<AgentIdentity, IdentityEntry>;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl std::ops::DerefMut for IdentityEntriesWriteGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

impl Drop for IdentityEntriesWriteGuard<'_> {
    fn drop(&mut self) {
        self.entries.changed.send_replace(());
    }
}

/// Who owns the live peer edges between a runtime's identities.
///
/// Its identities' wiring custody is external-managed: meerkat's identity
/// reconciler abstains and the topology layer is the desired-state owner.
/// What that layer can claim depends on whether a topology provider declares
/// the topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdentityEdgeOwnership {
    /// No topology provider: the desired set covers only what was declared
    /// to this runtime, and other edges (for example the mob definition's
    /// spawn-time wiring) are not its to remove. A reconcile unwires only the
    /// edges this process wired.
    WiredByThisProcess,
    /// A topology provider declares the whole desired topology: every live
    /// edge between two identities is the topology's, including one wired
    /// before a restart (the mob's wiring is durable and replays, while the
    /// managed-edge set is this process's memory). A reconcile unwires every
    /// live identity edge that is no longer desired.
    TopologyProvider,
}

impl IdentityEdgeOwnership {
    pub(crate) fn for_topology_provider(present: bool) -> Self {
        if present {
            Self::TopologyProvider
        } else {
            Self::WiredByThisProcess
        }
    }
}

/// Test seam run in a held bootstrap right after its freshness read.
#[cfg(test)]
pub(crate) type HeldFreshnessHook =
    Box<dyn FnOnce() -> futures::future::BoxFuture<'static, ()> + Send>;

/// The durable held-completion marks as this process knows them (see
/// [`super::held_bootstrap::HeldCompletionMarks`]).
enum HeldMarks {
    /// No state directory or host store realm is configured: the marks live
    /// in memory only.
    NoStateDir,
    /// Not read yet: the first wake or held pass reads them into
    /// `held_completion_owed`.
    Unread(super::held_bootstrap::HeldCompletionMarksAt),
    /// Read into `held_completion_owed`; new marks are written here.
    Read(super::held_bootstrap::HeldCompletionMarksAt),
}

/// The first-boot manifest as this process knows it.
enum FirstBootRows {
    /// No state directory or host store realm is configured, so there is no
    /// manifest.
    NoStateDir,
    /// The manifest at `location` is not read yet: the first roster
    /// compile, or the first wake, reads it.
    Unread { location: FirstBootLocation },
    /// The rows still to carry out, by identity, and the digest of the rows
    /// the manifest holds; `file_stale` while the manifest still lists a row
    /// this process carried out (its rewrite failed and is retried).
    Pending {
        location: FirstBootLocation,
        rows: BTreeMap<AgentIdentity, FirstBootRow>,
        file_rows_sha256: String,
        file_stale: bool,
    },
    /// No manifest, or every row carried out and the manifest removed.
    Settled,
}

/// How a host-composed gateway hears that an incarnation of a durable
/// identity's member was retired
/// (`GatewayHostComposition::identity_member_retired`). An `Err` is the
/// host's release failing; the retirement stands.
pub(crate) type RetiredMemberObserver = Arc<
    dyn Fn(
            super::types::RetiredIdentityMember,
        ) -> futures::future::BoxFuture<'static, Result<(), String>>
        + Send
        + Sync,
>;

/// Why this gateway's runtime does not serve a member it has registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotServedHere {
    /// The member is placed on a host.
    PlacedOnHost,
    /// The member is bound to an external runtime.
    ExternalBinding,
}

pub struct IdentityRuntime {
    entries: IdentityEntries,
    event_channels: RwLock<BTreeMap<AgentIdentity, broadcast::Sender<IdentityEvent>>>,
    runtime_instance_id: String,
    has_runtime_store: bool,
    bridge: Option<Arc<dyn SessionBridge>>,
    reset_roster_source: StdRwLock<Option<ResetRosterSource>>,
    runtime_services: AgentRuntimeServices,
    managed_peer_edges: RwLock<BTreeSet<(AgentIdentity, AgentIdentity)>>,
    /// Who owns the live edges between this runtime's identities; see
    /// [`IdentityEdgeOwnership`].
    identity_edge_ownership: StdRwLock<IdentityEdgeOwnership>,
    managed_peer_reconcile_lock: Mutex<()>,
    /// Managed edges whose wire or unwire call timed out: the reply was
    /// dropped, but the mob command may still be queued in the actor, and
    /// inspection (which reads roster and machine state directly) cannot see
    /// a queued command, so the edge's fate is unknown. A reconcile settles
    /// each one only with an ORDERED command that succeeded after it: an
    /// (idempotent) wire if still desired, a tolerant unwire if not. An edge
    /// stays here until such a command succeeded, or until an endpoint has no
    /// member left for the pending command to land on.
    unsettled_peer_wiring: RwLock<BTreeSet<(AgentIdentity, AgentIdentity)>>,
    /// Post-admission peer hydrations in flight, per target, with whether
    /// another admission asked for a rerun meanwhile. Concurrent admissions
    /// to one target merge into the running hydration instead of stacking.
    peer_hydrations_in_flight: StdMutex<PeerHydrations>,
    /// Post-admission hydration passes run (test observability only).
    #[cfg(test)]
    peer_hydration_passes: std::sync::atomic::AtomicUsize,
    /// A weak handle to the owning `Arc`, remembered by the first
    /// `Arc`-receiving entry point (tracked foreground work, the lease
    /// supervisor). Lets `&self` admission paths detach their post-admission
    /// peer hydration instead of running it inline before they return.
    self_handle: std::sync::OnceLock<std::sync::Weak<IdentityRuntime>>,
    desired_peer_edges: RwLock<Vec<ManagedPeerEdge>>,
    topology_controller: StdRwLock<Option<crate::topology_control::TopologyController>>,
    materialization_locks: RwLock<BTreeMap<AgentIdentity, Arc<Mutex<()>>>>,
    best_effort_materialization_locks: RwLock<BTreeMap<AgentIdentity, Arc<Mutex<()>>>>,
    lifecycle_locks: RwLock<BTreeMap<AgentIdentity, Arc<Mutex<()>>>>,
    raw_member_alias_locks: RwLock<RawMemberAliasLockTable>,
    customizer: RwLock<Option<Arc<dyn AgentCustomizer>>>,
    /// Stable per-identity dispatchers for `customize_build` tools (#563).
    /// Set whenever an agent customizer exists; the same registry backs the
    /// mob's `CustomizerToolsSpawnCustomizer`.
    customizer_tools: RwLock<Option<Arc<super::customizer_tools::CustomizerToolRegistry>>>,
    /// Identities whose pre-activation `customize_build` failed: restored
    /// without their customizer tools until materialization publishes.
    customizer_tools_pending: std::sync::RwLock<BTreeMap<AgentIdentity, CustomizerToolsPending>>,
    /// Set once, before the bootstrap, when this runtime serves a candidate
    /// launch (`runtime_options.mob_composition.authority = "candidate"`):
    /// it then releases no member's run-start hold, and its roster pass
    /// changes no stored member (see [`Self::mark_candidate_launch`]).
    candidate_launch: AtomicBool,
    agent_memory: RwLock<Option<AgentMemoryRuntimeInjector>>,
    default_timeout: Duration,
    /// Throttles best-effort warms (peer hydration, background warm) of an
    /// identity whose last best-effort wake failed. A direct wake is never
    /// throttled and always asks Core again.
    materialization_failure_backoff: RwLock<BTreeMap<AgentIdentity, MaterializationFailureBackoff>>,
    /// Host-declared `rkat` scope rendered into [`SessionRepairRequired`]
    /// commands; absent fields render as explicit placeholders.
    session_repair_scope: StdRwLock<Option<SessionRepairScope>>,
    error_hook: StdRwLock<Option<crate::unified_runtime::ErrorHook>>,
    /// The host's retirement hook, installed by a host-composed gateway.
    /// Without one, a retire reads nothing extra and tells no one.
    retired_member_observer: StdRwLock<Option<RetiredMemberObserver>>,
    /// The unified runtime's actor-loop probe verdict, installed late (the
    /// probe starts with the base runtime, identity-first attaches after).
    /// Forwarded to the session bridge so admission fails fast while a stall
    /// is open; read by `member_health`.
    actor_loop_health: StdRwLock<Option<Arc<ActorLoopHealth>>>,
    /// Told when a live member moves to a new session under the same
    /// incarnation (a respawn rebind or a delivery-repair rotation).
    session_rotation_observer: StdRwLock<Option<Arc<dyn MemberSessionRotationObserver>>>,
    bootstrap_status: watch::Sender<IdentityBootstrapStatus>,
    bootstrap_generation: StdMutex<u64>,
    bootstrap_controller: Mutex<()>,
    /// The held bootstrap pass in progress, if any: fresh creations of its
    /// roster identities are held until the pass decides them.
    held_create_pass: StdMutex<Option<Arc<super::held_bootstrap::HeldCreatePass>>>,
    /// Identities whose member a held bootstrap pass did not publish (it
    /// ended Broken in that pass): each owes its host a fresh held
    /// completion, and no embodiment outside a held pass that covers it
    /// creates it meanwhile. A held pass that publishes it discharges it.
    /// With a state directory it also holds the durable marks read back
    /// after a restart (`held_completion_marks`).
    held_completion_owed: StdMutex<BTreeSet<AgentIdentity>>,
    /// Where the held-completion marks are kept durably, so a restart
    /// keeps them.
    held_completion_marks: StdMutex<HeldMarks>,
    /// Serializes the first read of the durable held-completion marks.
    held_completion_marks_reader: Mutex<()>,
    /// The first-boot manifest: the rows of it this process has not
    /// carried out yet. Process-local: each row is read against Core's
    /// member history again before it is carried out.
    first_boot: StdMutex<FirstBootRows>,
    /// Serializes the rewrites of the first-boot manifest file.
    first_boot_writer: Mutex<()>,
    /// The pending first-boot rows as the bootstrap status reports them.
    first_boot_status: StdMutex<Vec<FirstBootPendingRow>>,
    /// Test seam: runs in a held bootstrap right after its freshness read.
    #[cfg(test)]
    held_after_freshness_hook: StdMutex<Option<HeldFreshnessHook>>,
    bootstrap_task: Mutex<Option<JoinHandle<()>>>,
    bootstrap_cancel: StdRwLock<Option<watch::Sender<bool>>>,
    bootstrap_shutdown: AtomicBool,
    foreground_operations: Mutex<JoinSet<()>>,
    foreground_cancel: watch::Sender<bool>,
    foreground_shutdown: AtomicBool,
    /// Wakes the read-only waits (a turn's outcome, a completion, an output
    /// preview) at shutdown. A shutdown's in-flight drain closes foreground
    /// operations but leaves these waiting, so a drained turn's waiter
    /// reports the turn's outcome.
    foreground_waits_closed: watch::Sender<bool>,
    /// Reset memory captures of outgoing sessions not yet run, keyed by the
    /// session. A capture is recorded before its task spawns and leaves the
    /// ledger only after it ran, so shutdown joins or reruns it.
    pending_reset_memory_captures: Arc<RwLock<BTreeMap<String, PendingResetMemoryCapture>>>,
    reset_memory_capture_tasks: Mutex<JoinSet<()>>,
    /// The customizer `after_create` calls told outside any lifecycle lock
    /// ([`Self::spawn_after_create`]). A graceful shutdown abandons and joins
    /// them.
    after_create_tasks: Mutex<JoinSet<()>>,
    /// Per-identity completed-turn counters ([`CompletionCursor`]).
    ///
    /// RETAINED for the process lifetime — never pruned on retire, reset, or
    /// delete. Dropping an entry would let a re-registered identity republish
    /// a cursor it already published, which is the one thing a completion
    /// cursor must never do. The map is bounded by the identities this process
    /// has seen, i.e. by roster size.
    completion_cursors: StdMutex<BTreeMap<AgentIdentity, CompletionCursor>>,
    /// Outcomes of turns admitted under a [`TurnTicket`]
    /// ([`Self::send_with_turn_ticket`], [`Self::dispatch_with_turn_ticket`]).
    /// Shared with the waiter tasks that settle them.
    turn_outcomes: Arc<StdMutex<TurnOutcomes>>,
    /// Failed (or cancelled) runs observed per identity, so a completion
    /// wait can end typed instead of running to its deadline. Retained for
    /// the process lifetime, like the completion cursors.
    run_failures: StdMutex<BTreeMap<AgentIdentity, u64>>,
    /// Completions that happened but are not counted yet
    /// ([`Self::install_pending_completion_drain`]).
    pending_completion_drain: StdRwLock<Option<Arc<dyn PendingCompletionDrain>>>,
    /// Per-identity serialization of completion credit: an observed run
    /// terminal is claimed and credited under it, and a cursor read drains
    /// and reads under it, so a read never sees a terminal claimed but not
    /// yet credited.
    completion_credit_locks:
        StdMutex<BTreeMap<AgentIdentity, std::sync::Weak<tokio::sync::Mutex<()>>>>,
    /// Whether a completion drain was installed when the member-binding
    /// observer was (the first binding-woken attach may replay only with
    /// one).
    #[cfg(test)]
    binding_observer_saw_drain: std::sync::atomic::AtomicBool,
    /// Fired whenever a ticketed turn is admitted (which may evict another)
    /// or settles, so [`Self::wait_for_turn`] waits on the change instead of
    /// polling the registry. Carries no state.
    turn_outcome_changes: Arc<watch::Sender<()>>,
    /// Member inspections shared per runtime incarnation ([`Self::inspect`]).
    inspections: StdMutex<BTreeMap<AgentRuntimeId, SharedInspection>>,
}

/// How many ticketed turns are remembered at once.
const TRACKED_TURNS_CAP: usize = 4096;

/// How long a settled ticketed turn stays readable.
const SETTLED_TURN_RETENTION: Duration = Duration::from_mins(10);

/// Ticketed turns, bounded by [`TRACKED_TURNS_CAP`] and
/// [`SETTLED_TURN_RETENTION`].
///
/// Keyed by a ticket this registry mints once per admission, so two admissions
/// can never share a record: interaction and delivery correlation ids repeat
/// across admissions by design. A ticket read for any identity other than the
/// one it was admitted for is `Unknown`, and only the waiter that owns a
/// pending record settles it.
#[derive(Default)]
struct TurnOutcomes {
    turns: BTreeMap<TurnTicket, TrackedTurn>,
    /// Tracked dispatches by the scope meerkat deduplicates in (the member
    /// incarnation's session runtime ledger) plus the dispatch's idempotency
    /// key, so a re-dispatch of one key names the original admission's ticket.
    by_delivery: HashMap<DeliveryKey, TurnTicket>,
    admitted: u64,
}

/// A dispatch's dedup identity as meerkat sees it: the member incarnation and
/// the session whose runtime ledger admitted it, plus its idempotency key.
/// meerkat deduplicates only within one session's runtime ledger, and a
/// session rotation keeps the incarnation's `AgentRuntimeId`, so the session
/// is part of the key: after a rotation (or a reset, which mints a new
/// incarnation) a re-dispatch is tracked as an admission of its own.
type DeliveryKey = (AgentRuntimeId, SessionId, String);

struct TrackedTurn {
    identity: AgentIdentity,
    delivery_key: Option<DeliveryKey>,
    /// Admission order: evicts the oldest pending turn when the cap is full
    /// of pending turns.
    admitted: u64,
    state: TrackedTurnState,
}

enum TrackedTurnState {
    Pending,
    Settled {
        at: Instant,
        outcome: Result<TurnOutput, String>,
    },
}

impl TurnOutcomes {
    /// Record a newly admitted turn as pending under a freshly minted ticket.
    fn admit(&mut self, identity: &AgentIdentity, delivery_key: Option<DeliveryKey>) -> TurnTicket {
        self.prune();
        let ticket = loop {
            let ticket = TurnTicket::mint();
            if !self.turns.contains_key(&ticket) {
                break ticket;
            }
        };
        self.admitted += 1;
        if let Some(key) = &delivery_key {
            // A later admission under the same key (the earlier one failed)
            // takes over the index; the earlier record stays readable under
            // its own ticket.
            self.by_delivery.insert(key.clone(), ticket);
        }
        self.turns.insert(
            ticket,
            TrackedTurn {
                identity: identity.clone(),
                delivery_key,
                admitted: self.admitted,
                state: TrackedTurnState::Pending,
            },
        );
        ticket
    }

    /// The ticket of the tracked admission a re-dispatch under `key` is
    /// deduplicated onto, while that admission is still pending or completed.
    /// A settled failure is not reused: whether the runtime replays a failed
    /// key is its decision, so the retry is tracked as an admission of its own
    /// (and reads [`TurnOutput::NoOwnResult`] if it is deduplicated after all).
    fn reusable_delivery_ticket(
        &mut self,
        identity: &AgentIdentity,
        key: &DeliveryKey,
    ) -> Option<TurnTicket> {
        self.prune();
        let ticket = *self.by_delivery.get(key)?;
        let turn = self.turns.get(&ticket)?;
        let reusable = &turn.identity == identity
            && matches!(
                turn.state,
                TrackedTurnState::Pending | TrackedTurnState::Settled { outcome: Ok(_), .. }
            );
        reusable.then_some(ticket)
    }

    /// Settle the pending record `ticket` names for `identity`. Called once,
    /// by the waiter that owns the record; a record that is gone, already
    /// settled, or another identity's is left untouched.
    fn settle(
        &mut self,
        identity: &AgentIdentity,
        ticket: TurnTicket,
        outcome: Result<TurnOutput, String>,
    ) {
        if let Some(turn) = self.turns.get_mut(&ticket)
            && &turn.identity == identity
            && matches!(turn.state, TrackedTurnState::Pending)
        {
            turn.state = TrackedTurnState::Settled {
                at: Instant::now(),
                outcome,
            };
        }
    }

    fn outcome(&self, identity: &AgentIdentity, ticket: TurnTicket) -> TurnOutcome {
        match self.turns.get(&ticket) {
            Some(turn) if &turn.identity == identity => match &turn.state {
                TrackedTurnState::Pending => TurnOutcome::Pending,
                TrackedTurnState::Settled {
                    outcome: Ok(output),
                    ..
                } => TurnOutcome::Completed {
                    output: output.clone(),
                },
                TrackedTurnState::Settled {
                    outcome: Err(reason),
                    ..
                } => TurnOutcome::Failed {
                    reason: reason.clone(),
                },
            },
            _ => TurnOutcome::Unknown,
        }
    }

    fn remove(&mut self, ticket: TurnTicket) {
        if let Some(turn) = self.turns.remove(&ticket)
            && let Some(key) = turn.delivery_key
            && self.by_delivery.get(&key) == Some(&ticket)
        {
            self.by_delivery.remove(&key);
        }
    }

    /// Drop settled turns past their retention; then, while at the cap, the
    /// turn that settled LONGEST AGO (settle time, not admission order, so a
    /// long turn that just settled keeps its full retention). Only a cap full
    /// of pending turns evicts a pending one, the oldest admitted; its waiter
    /// then reads `Unknown`, never a wrong output.
    fn prune(&mut self) {
        let turns = &mut self.turns;
        turns.retain(|_, turn| match turn.state {
            TrackedTurnState::Pending => true,
            TrackedTurnState::Settled { at, .. } => at.elapsed() < SETTLED_TURN_RETENTION,
        });
        let turns = &self.turns;
        self.by_delivery
            .retain(|_, ticket| turns.contains_key(ticket));
        while self.turns.len() >= TRACKED_TURNS_CAP {
            let settled_longest_ago = self
                .turns
                .iter()
                .filter_map(|(ticket, turn)| match turn.state {
                    TrackedTurnState::Settled { at, .. } => Some((at, *ticket)),
                    TrackedTurnState::Pending => None,
                })
                .min_by_key(|(at, _)| *at)
                .map(|(_, ticket)| ticket);
            let evicted = settled_longest_ago.or_else(|| {
                self.turns
                    .iter()
                    .min_by_key(|(_, turn)| turn.admitted)
                    .map(|(ticket, _)| *ticket)
            });
            let Some(evicted) = evicted else {
                break;
            };
            if settled_longest_ago.is_none() {
                tracing::warn!(
                    ticket = %evicted,
                    cap = TRACKED_TURNS_CAP,
                    "ticketed turns at capacity with every one still pending; the oldest is \
                     forgotten and reads as unknown"
                );
            }
            self.remove(evicted);
        }
    }
}

/// One bridge inspection of a member incarnation in flight, shared by the
/// callers that join it (see [`IdentityRuntime::inspect`]). A settled read
/// is never reused.
#[derive(Clone)]
struct SharedInspection {
    /// The identity's completion cursor when this read started. The read
    /// reflects at least every turn counted there.
    started_at: CompletionCursor,
    read: Arc<tokio::sync::OnceCell<Result<super::bridge::MemberInspection, String>>>,
}

impl SharedInspection {
    fn start(started_at: CompletionCursor) -> Self {
        Self {
            started_at,
            read: Arc::default(),
        }
    }

    fn in_flight(&self) -> bool {
        self.read.get().is_none()
    }

    /// A caller that has seen completion cursor `seen` may join only a read
    /// still in flight that started at or after it. A read started before a
    /// turn the caller already counts could report the previous turn's
    /// output next to the new cursor.
    fn joinable_at(&self, seen: CompletionCursor) -> bool {
        self.in_flight() && self.started_at >= seen
    }
}

/// One generated member alias plus the lifecycle lock owned by its durable
/// identity runtime. Cross-runtime topology code resolves all endpoints first,
/// then acquires these targets in one global order before inspecting or
/// mutating either side.
pub(crate) struct MemberAliasLifecycleTarget {
    runtime: Arc<IdentityRuntime>,
    identity: AgentIdentity,
    alias: String,
    lock: Arc<Mutex<()>>,
}

impl MemberAliasLifecycleTarget {
    /// Canonical durable identity whose lifecycle lock owns this generated
    /// member alias. Surfaces may project this label, but the target itself
    /// remains the authority carried into the tracked operation.
    pub(crate) fn durable_identity(&self) -> &AgentIdentity {
        &self.identity
    }
}

struct MultiRuntimeForegroundCompletion(watch::Sender<bool>);

impl Drop for MultiRuntimeForegroundCompletion {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

/// Internal result of one dispatch attempt: the caller-facing admission plus
/// the concrete bridge session that accepted the work (scheduler delivery
/// needs the latter; RPC callers do not).
pub(crate) struct DispatchOutcome {
    admission: DispatchAdmission,
    pub(crate) session_id: Option<SessionId>,
    pub(crate) incarnation: CapturedIncarnation,
    /// `Some` exactly when the dispatch asked to track its turn.
    turn: Option<TurnTracking>,
}

/// Exact result of the one wake door shared by eager boot, lazy foreground
/// wakes, and background warms.
pub(crate) struct EmbodimentOutcome {
    /// The binding Core committed for the member: its runtime id, and its
    /// session (`None` for a peer-only member).
    pub(crate) binding: super::bridge::MemberRuntimeBinding,
    /// Core resumed the identity's committed session (`false`: a fresh
    /// spawn).
    pub(crate) resumed: bool,
    pub(crate) draft: AgentBuildDraft,
    /// Core already held the member when the wake door ran (a revival or a
    /// converged re-validation, not a wake).
    pub(crate) already_active: bool,
    /// The last session this wake replaced with a fresh one because Core
    /// holds no durable session for it.
    pub(crate) replaced_session: Option<SessionId>,
    /// The `after_create` owed to the customizer the embodiment's caller
    /// passed ([`EmbodimentOverrides::customizer`]), which
    /// [`IdentityRuntime::embody_identity`] tells once it has released the
    /// identity's lifecycle lock.
    pub(crate) override_after_create: Option<AfterCreateNotice>,
}

/// An `after_create` a build owes its customizer: the session the build
/// created or resumed, and what the build made of it.
pub(crate) struct AfterCreateNotice {
    session_id: SessionId,
    context: crate::mob_handle_runtime::SessionCreatedContext,
}

/// Eager-only inputs to the shared embodiment transaction. Lazy foreground
/// and background callers use the registered entry's spec and customizer.
#[derive(Default)]
pub(crate) struct EmbodimentOverrides<'a> {
    pub(crate) spec: Option<&'a DurableAgentSpec>,
    pub(crate) customizer: Option<&'a dyn AgentCustomizer>,
    /// The registered row a waking send's input was authored from: a
    /// different registered row refuses the embodiment before any effect.
    pub(crate) expected_spec_digest: Option<&'a str>,
}

#[derive(Clone)]
struct ResetRosterSource {
    provider: Arc<dyn RosterProvider>,
    mob_definition: Option<meerkat_mob::MobDefinition>,
}

#[derive(Debug, Clone)]
struct MaterializationFailureBackoff {
    suppress_until: Instant,
    error: String,
}

/// What Core's member history says of one identity, for the views.
#[derive(Debug, Clone, Default)]
struct CoreViewFacts {
    /// Core holds the identity's member.
    live: bool,
    /// The live member's runtime id (Meta's public alias of Core's).
    runtime_id: Option<AgentRuntimeId>,
    /// The session of the identity's last binding, live or retired.
    session_id: Option<SessionId>,
    /// Core's mob member id of that binding.
    member_id: Option<String>,
    /// Core's generation of that binding.
    generation: Option<meerkat_mob::Generation>,
    /// Core holds an identity intent row for the identity.
    intent_managed: bool,
}

impl CoreViewFacts {
    /// Read `identity` out of `history` (`None` without a bridge: nothing
    /// is live).
    fn of(history: Option<&meerkat_mob::MemberBindingHistory>, identity: &AgentIdentity) -> Self {
        let Some(history) = history else {
            return Self::default();
        };
        let core_identity =
            crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let intent_managed = !matches!(
            history.identity_management(&core_identity),
            meerkat_mob::MemberIdentityManagement::CommandDriven
        );
        let Some(binding) = history.last_binding(&core_identity) else {
            return Self {
                intent_managed,
                ..Self::default()
            };
        };
        Self {
            live: binding.is_live(),
            runtime_id: live_binding_runtime_id(binding),
            session_id: binding.session_id().cloned(),
            member_id: Some(binding.identity().as_str().to_string()),
            generation: Some(binding.generation()),
            intent_managed,
        }
    }
}

/// Meta's public runtime id of a Core binding while Core holds it live
/// (`rt:{identity}:{generation}`); `None` once the binding is retired.
pub fn live_binding_runtime_id(binding: &meerkat_mob::MemberBinding) -> Option<AgentRuntimeId> {
    if !binding.is_live() {
        return None;
    }
    AgentRuntimeId::parse(&crate::member_comms_id::public_runtime_alias(
        &meerkat_mob::ids::AgentRuntimeId::new(binding.identity().clone(), binding.generation()),
    ))
    .ok()
}

/// Whether `binding` carries Core's durable-identity marker for
/// `identity`: keyed by the identity (every identity-first spawn and
/// resume) or by its member id (Core's identity reconciler keys by the
/// member it spawns).
pub(crate) fn binding_carries_identity_marker(
    binding: &meerkat_mob::MemberBinding,
    identity: &AgentIdentity,
) -> bool {
    matches!(
        binding.continuity_intent(),
        meerkat_mob::SpawnContinuityIntent::DurableIdentity { continuity_key }
            if continuity_key == identity.as_str()
                || continuity_key == binding.identity().as_str()
    )
}

/// Whether an identity-first spawn made `binding`'s member for `identity`:
/// it carries the identity's durable-identity marker, or its spawn labels
/// carry the `agent_identity` label naming the identity, which every
/// identity-first spawn writes (Meta 0.9.3's included) and the raw member
/// creation surfaces refuse. A member the host made directly on the mob
/// under the identity's member id has neither.
pub(crate) fn binding_spawned_for_identity(
    binding: &meerkat_mob::MemberBinding,
    identity: &AgentIdentity,
) -> bool {
    binding_carries_identity_marker(binding, identity)
        || crate::member_comms_id::durable_identity_label(binding.labels())
            == Some(identity.as_str())
}

#[derive(Clone)]
struct PendingResetMemoryCapture {
    injector: AgentMemoryRuntimeInjector,
    identity: AgentIdentity,
    session_key: String,
    generation: u64,
}

impl PendingResetMemoryCapture {
    async fn run(&self) {
        // Repeat the synchronous marks in the owned task so a raw reset
        // future dropped after debt publication cannot let the distiller read
        // evidence before the reset quarantine boundary exists.
        self.injector.note_reset_boundary(&self.session_key);
        self.injector
            .note_session_generation(&self.identity, &self.session_key, self.generation);
        self.injector
            .distill_before_rotation(
                &self.identity,
                &self.session_key,
                crate::memory::distiller::DistillCause::Reset,
            )
            .await;
    }
}

impl IdentityRuntime {
    /// Test seam: run `hook` in the next held bootstrap right after its
    /// freshness read.
    #[cfg(test)]
    pub(crate) fn set_held_after_freshness_hook(&self, hook: HeldFreshnessHook) {
        *self
            .held_after_freshness_hook
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }

    /// The held bootstrap pass in progress, if any.
    pub(crate) fn held_create_pass(&self) -> Option<Arc<super::held_bootstrap::HeldCreatePass>> {
        self.held_create_pass
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Whether `identity` owes its host a fresh held completion (see
    /// [`IdentityRuntimeError::HeldCompletionOwed`]).
    pub(crate) fn held_completion_owed(&self, identity: &AgentIdentity) -> bool {
        self.held_completion_owed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(identity)
    }

    /// Record how a held pass settled `identity`'s member: published, it
    /// discharges any held completion the identity owed; Broken, the
    /// identity owes its host a fresh held completion.
    pub(crate) fn record_held_completion(&self, identity: &AgentIdentity, published: bool) {
        let mut owed = self
            .held_completion_owed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if published {
            owed.remove(identity);
        } else {
            owed.insert(identity.clone());
        }
    }

    /// Point the runtime at its state directory, where the held-completion
    /// marks are kept durably, one file per identity under
    /// [`super::HELD_COMPLETION_OWED_DIR`]; the first wake or
    /// held pass reads them. `None` keeps the marks in memory only, so a
    /// restart forgets them.
    pub fn set_held_completion_state_dir(&self, state_dir: Option<PathBuf>) {
        *self
            .held_completion_marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = match state_dir {
            Some(state_dir) => {
                HeldMarks::Unread(super::held_bootstrap::HeldCompletionMarksAt::StateDir(
                    super::held_bootstrap::HeldCompletionMarks::in_state_dir(&state_dir),
                ))
            }
            None => HeldMarks::NoStateDir,
        };
    }

    /// Point the runtime at the host store realm that keeps its
    /// held-completion marks, one document per identity; the first wake or
    /// held pass reads them, on whichever host the runtime starts.
    pub(crate) fn set_held_completion_host_store(
        &self,
        documents: crate::host_store_documents::MetaDocuments,
    ) {
        *self
            .held_completion_marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = HeldMarks::Unread(
            super::held_bootstrap::HeldCompletionMarksAt::HostStore(documents),
        );
    }

    /// The durable held-completion marks, when a state directory or a host
    /// store realm keeps them.
    fn held_completion_marks(&self) -> Option<super::held_bootstrap::HeldCompletionMarksAt> {
        match &*self
            .held_completion_marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            HeldMarks::NoStateDir => None,
            HeldMarks::Unread(marks) | HeldMarks::Read(marks) => Some(marks.clone()),
        }
    }

    /// Read the durable held-completion marks into `held_completion_owed`,
    /// once. A marked identity that Core's member history binds was
    /// published by the held pass that marked it (removing its mark did not
    /// land), so it owes nothing and its mark is removed now. A read that
    /// fails refuses the caller, and the next call reads again: which
    /// identities owe a completion is unknown until then.
    async fn load_held_completion_marks(&self) -> Result<(), IdentityRuntimeError> {
        let unread = matches!(
            &*self
                .held_completion_marks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            HeldMarks::Unread(_)
        );
        if !unread {
            return Ok(());
        }
        let _reader = self.held_completion_marks_reader.lock().await;
        let marks = match &*self
            .held_completion_marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            HeldMarks::Unread(marks) => marks.clone(),
            HeldMarks::NoStateDir | HeldMarks::Read(_) => return Ok(()),
        };
        let marked = match &marks {
            super::held_bootstrap::HeldCompletionMarksAt::StateDir(files) => {
                let reader = files.clone();
                tokio::task::spawn_blocking(move || reader.read())
                    .await
                    .map_err(|error| {
                        IdentityRuntimeError::Internal(format!(
                            "reading the held-completion marks did not finish: {error}"
                        ))
                    })?
                    .map_err(|error| error.to_string())
            }
            super::held_bootstrap::HeldCompletionMarksAt::HostStore(documents) => {
                super::held_bootstrap::read_host_store_marks(documents).await
            }
        }
        .map_err(|error| {
            IdentityRuntimeError::Internal(format!(
                "reading the held-completion marks in {}: {error}",
                marks.describe()
            ))
        })?;
        let mut owed = BTreeSet::new();
        for identity in marked {
            if self.bridge.is_some() && self.core_last_binding(&identity).await?.is_some() {
                self.clear_held_completion_mark(&identity).await;
                continue;
            }
            owed.insert(identity);
        }
        self.held_completion_owed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(owed);
        *self
            .held_completion_marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = HeldMarks::Read(marks);
        Ok(())
    }

    /// Mark `identities` durably as owing their host a held completion,
    /// before a held pass creates any of their members.
    pub(crate) async fn mark_held_completion_owed(
        &self,
        identities: &[AgentIdentity],
    ) -> Result<(), IdentityRuntimeError> {
        self.load_held_completion_marks().await?;
        let Some(marks) = self.held_completion_marks() else {
            return Ok(());
        };
        let dir = marks.describe();
        match marks {
            super::held_bootstrap::HeldCompletionMarksAt::StateDir(files) => {
                let marked = identities.to_vec();
                tokio::task::spawn_blocking(move || files.mark(&marked))
                    .await
                    .map_err(|error| {
                        IdentityRuntimeError::Internal(format!(
                            "marking the held members did not finish: {error}"
                        ))
                    })?
                    .map_err(|error| error.to_string())
            }
            super::held_bootstrap::HeldCompletionMarksAt::HostStore(documents) => {
                super::held_bootstrap::mark_host_store(&documents, identities).await
            }
        }
        .map_err(|error| {
            IdentityRuntimeError::Internal(format!("marking the held members in {dir}: {error}"))
        })
    }

    /// Remove `identity`'s durable held-completion mark once a held pass
    /// published its member. A removal that fails leaves a stale mark, which
    /// the next read drops, since Core's member history then binds the
    /// identity.
    pub(crate) async fn clear_held_completion_mark(&self, identity: &AgentIdentity) {
        let Some(marks) = self.held_completion_marks() else {
            return;
        };
        let failure = match marks {
            super::held_bootstrap::HeldCompletionMarksAt::StateDir(files) => {
                let cleared = identity.clone();
                match tokio::task::spawn_blocking(move || files.clear(&cleared)).await {
                    Ok(Ok(())) => return,
                    Ok(Err(error)) => error.to_string(),
                    Err(error) => error.to_string(),
                }
            }
            super::held_bootstrap::HeldCompletionMarksAt::HostStore(documents) => {
                match super::held_bootstrap::clear_host_store_mark(&documents, identity).await {
                    Ok(()) => return,
                    Err(error) => error,
                }
            }
        };
        tracing::warn!(
            %identity,
            error = %failure,
            "the held-completion mark of a published member was not removed; the next read \
             of the marks drops it"
        );
    }

    /// Create a new identity runtime with the given configuration.
    ///
    /// The runtime serves its mob fully once it is installed in the mob's
    /// [`crate::UnifiedRuntime`], as the builder's identity-first composition
    /// does with the runtime it makes. Install one made here with
    /// [`crate::UnifiedRuntime::install_identity_first_context`], in an
    /// [`IdentityFirstRuntimeContext`]. A runtime that is never installed is
    /// credited no turn completions by the mob, so a turn ending never wakes
    /// its output and completion waits (its owner drives them with
    /// [`Self::record_turn_completed`]), and it is no authority for its
    /// identities' generated aliases in
    /// [`crate::UnifiedRuntime::wire_cross_mob`].
    pub fn new(config: IdentityRuntimeConfig) -> Self {
        let (bootstrap_status, _) = watch::channel(IdentityBootstrapStatus::empty(
            IdentityBootstrapMode::EagerMaterialize,
        ));
        let (foreground_cancel, _) = watch::channel(false);
        let (foreground_waits_closed, _) = watch::channel(false);
        Self {
            entries: IdentityEntries::new(),
            event_channels: RwLock::new(BTreeMap::new()),
            runtime_instance_id: config.runtime_instance_id,
            has_runtime_store: config.has_runtime_store,
            bridge: config.bridge,
            reset_roster_source: StdRwLock::new(None),
            runtime_services: AgentRuntimeServices::empty(),
            managed_peer_edges: RwLock::new(BTreeSet::new()),
            identity_edge_ownership: StdRwLock::new(IdentityEdgeOwnership::WiredByThisProcess),
            managed_peer_reconcile_lock: Mutex::new(()),
            unsettled_peer_wiring: RwLock::new(BTreeSet::new()),
            peer_hydrations_in_flight: StdMutex::new(PeerHydrations::default()),
            #[cfg(test)]
            peer_hydration_passes: std::sync::atomic::AtomicUsize::new(0),
            self_handle: std::sync::OnceLock::new(),
            desired_peer_edges: RwLock::new(Vec::new()),
            topology_controller: StdRwLock::new(None),
            materialization_locks: RwLock::new(BTreeMap::new()),
            best_effort_materialization_locks: RwLock::new(BTreeMap::new()),
            lifecycle_locks: RwLock::new(BTreeMap::new()),
            raw_member_alias_locks: RwLock::new(RawMemberAliasLockTable::default()),
            customizer: RwLock::new(None),
            customizer_tools: RwLock::new(None),
            customizer_tools_pending: std::sync::RwLock::new(BTreeMap::new()),
            candidate_launch: AtomicBool::new(false),
            agent_memory: RwLock::new(None),
            default_timeout: config.default_timeout.unwrap_or(Duration::from_secs(90)),
            materialization_failure_backoff: RwLock::new(BTreeMap::new()),
            session_repair_scope: StdRwLock::new(None),
            error_hook: StdRwLock::new(None),
            retired_member_observer: StdRwLock::new(None),
            actor_loop_health: StdRwLock::new(None),
            session_rotation_observer: StdRwLock::new(None),
            bootstrap_status,
            bootstrap_generation: StdMutex::new(0),
            bootstrap_controller: Mutex::new(()),
            held_create_pass: StdMutex::new(None),
            held_completion_owed: StdMutex::new(BTreeSet::new()),
            held_completion_marks: StdMutex::new(HeldMarks::NoStateDir),
            held_completion_marks_reader: Mutex::new(()),
            first_boot: StdMutex::new(FirstBootRows::NoStateDir),
            first_boot_writer: Mutex::new(()),
            first_boot_status: StdMutex::new(Vec::new()),
            #[cfg(test)]
            held_after_freshness_hook: StdMutex::new(None),
            bootstrap_task: Mutex::new(None),
            bootstrap_cancel: StdRwLock::new(None),
            bootstrap_shutdown: AtomicBool::new(false),
            foreground_operations: Mutex::new(JoinSet::new()),
            foreground_cancel,
            foreground_shutdown: AtomicBool::new(false),
            foreground_waits_closed,
            pending_reset_memory_captures: Arc::new(RwLock::new(BTreeMap::new())),
            reset_memory_capture_tasks: Mutex::new(JoinSet::new()),
            after_create_tasks: Mutex::new(JoinSet::new()),
            completion_cursors: StdMutex::new(BTreeMap::new()),
            turn_outcomes: Arc::default(),
            turn_outcome_changes: Arc::new(watch::channel(()).0),
            run_failures: StdMutex::new(BTreeMap::new()),
            pending_completion_drain: StdRwLock::new(None),
            completion_credit_locks: StdMutex::new(BTreeMap::new()),
            #[cfg(test)]
            binding_observer_saw_drain: std::sync::atomic::AtomicBool::new(false),
            inspections: StdMutex::new(BTreeMap::new()),
        }
    }

    pub fn with_runtime_services(mut self, runtime_services: AgentRuntimeServices) -> Self {
        self.runtime_services = runtime_services;
        self
    }

    pub fn with_reset_roster_provider(self, provider: Arc<dyn RosterProvider>) -> Self {
        self.set_reset_roster_provider(Some(provider));
        self
    }

    pub fn with_reset_roster_provider_context(
        self,
        provider: Arc<dyn RosterProvider>,
        mob_definition: Option<meerkat_mob::MobDefinition>,
    ) -> Self {
        self.set_reset_roster_provider_context(Some(provider), mob_definition);
        self
    }

    pub(crate) fn runtime_services(&self) -> AgentRuntimeServices {
        self.runtime_services.clone()
    }

    /// Current typed bootstrap snapshot. Reading it never waits on an
    /// in-flight materialization.
    pub fn identity_bootstrap_status(&self) -> IdentityBootstrapStatus {
        self.identity_bootstrap_status_with_generation().1
    }

    pub(crate) fn identity_bootstrap_status_with_generation(
        &self,
    ) -> (u64, IdentityBootstrapStatus) {
        let generation = self
            .bootstrap_generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (*generation, self.bootstrap_status.borrow().clone())
    }

    pub(crate) fn subscribe_identity_bootstrap_status(
        &self,
    ) -> watch::Receiver<IdentityBootstrapStatus> {
        self.bootstrap_status.subscribe()
    }

    /// Change stream of the typed bootstrap snapshot, including each
    /// identity's eager-restore progress, so an in-process host can wait on
    /// typed per-member progress instead of a timeout.
    pub fn watch_identity_bootstrap_status(&self) -> watch::Receiver<IdentityBootstrapStatus> {
        self.bootstrap_status.subscribe()
    }

    /// Wait until every tracked identity has reached Active or Broken.
    /// Broken is terminal (and `ready == false`), so callers receive a useful
    /// failure snapshot instead of hanging forever.
    pub async fn wait_identity_bootstrap_terminal(
        &self,
        timeout: Duration,
    ) -> (IdentityBootstrapStatus, bool) {
        let (status, timed_out, _) = self
            .wait_identity_bootstrap_terminal_with_generation(timeout)
            .await;
        (status, timed_out)
    }

    pub(crate) async fn wait_identity_bootstrap_terminal_with_generation(
        &self,
        timeout: Duration,
    ) -> (IdentityBootstrapStatus, bool, u64) {
        let mut receiver = self.subscribe_identity_bootstrap_status();
        let wait = async {
            loop {
                let (generation, snapshot) = self.identity_bootstrap_status_with_generation();
                if snapshot.complete && snapshot.materialization_terminal() {
                    return (snapshot, generation);
                }
                if receiver.changed().await.is_err() {
                    let (generation, snapshot) = self.identity_bootstrap_status_with_generation();
                    return (snapshot, generation);
                }
            }
        };
        match tokio::time::timeout(timeout, wait).await {
            Ok((snapshot, generation)) => (snapshot, false, generation),
            Err(_) => {
                let (generation, snapshot) = self.identity_bootstrap_status_with_generation();
                (snapshot, true, generation)
            }
        }
    }

    fn request_identity_bootstrap_stop(&self) {
        if let Some(cancel) = self
            .bootstrap_cancel
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            let _ = cancel.send(true);
        }
    }

    async fn join_identity_bootstrap_task(&self) {
        if let Some(task) = self.bootstrap_task.lock().await.take() {
            let _ = task.await;
        }
        *self
            .bootstrap_cancel
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Serialize status mutation with pass supersession. The generation lock
    /// closes the check-then-write race that an atomic epoch alone would leave
    /// between a retiring warm task and a newly-published reconcile barrier.
    fn modify_bootstrap_status<F>(&self, expected_generation: Option<u64>, modify: F) -> bool
    where
        F: FnOnce(&mut IdentityBootstrapStatus),
    {
        let current_generation = self
            .bootstrap_generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if expected_generation.is_some_and(|expected| expected != *current_generation) {
            return false;
        }
        self.bootstrap_status.send_modify(modify);
        true
    }

    fn replace_bootstrap_status(
        &self,
        expected_generation: u64,
        mut status: IdentityBootstrapStatus,
    ) -> bool {
        let current_generation = self
            .bootstrap_generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if expected_generation != *current_generation {
            return false;
        }
        // A new pass's snapshot keeps the first-boot rows still pending.
        status.first_boot_pending = self
            .first_boot_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        self.bootstrap_status.send_replace(status);
        true
    }

    /// Close the controller, cooperatively cancel warm operations before
    /// bridge/member installation, and join the tracked task. An acquired but
    /// uninstalled lease is released explicitly; operations past that boundary
    /// reach their explicit commit/rollback path so no external lease or
    /// bridge session can be leaked by a raw task abort.
    pub(crate) async fn cancel_identity_bootstrap(&self) {
        self.bootstrap_shutdown.store(true, Ordering::Release);
        self.request_identity_bootstrap_stop();
        let _controller = self.bootstrap_controller.lock().await;
        self.request_identity_bootstrap_stop();
        self.join_identity_bootstrap_task().await;
        self.modify_bootstrap_status(None, |snapshot| {
            for entry in snapshot.identities.values_mut() {
                if entry.state == IdentityBootstrapState::Warming {
                    entry.state = IdentityBootstrapState::Dormant;
                }
            }
            snapshot.complete = true;
            snapshot.refresh_aggregates();
        });
    }

    async fn begin_identity_bootstrap(
        &self,
        generation: u64,
        mode: IdentityBootstrapMode,
        roster: &[DurableAgentSpec],
    ) {
        let mut identities = BTreeMap::new();
        for spec in roster {
            // Core holds a live member for the identity: it starts Active.
            let live = matches!(self.core_live_member(&spec.identity).await, Ok(Some(_)));
            let state = if live {
                IdentityBootstrapState::Active
            } else if matches!(&mode, IdentityBootstrapMode::EagerMaterialize) {
                IdentityBootstrapState::Warming
            } else {
                IdentityBootstrapState::Dormant
            };
            identities.insert(
                spec.identity.clone(),
                IdentityBootstrapEntry {
                    state,
                    error: None,
                    restore: None,
                },
            );
        }
        let mut status = IdentityBootstrapStatus {
            mode,
            complete: false,
            ready: false,
            error: None,
            counts: Default::default(),
            identities,
            first_boot_pending: Vec::new(),
            deferred: Vec::new(),
        };
        status.refresh_aggregates();
        // A roster consisting only of already-active identities still has a
        // reconcile pass in flight. `complete` is the barrier guard even when
        // the aggregate states themselves happen to look ready.
        status.complete = false;
        status.ready = false;
        self.replace_bootstrap_status(generation, status);
    }

    fn begin_identity_bootstrap_pending(&self, mode: IdentityBootstrapMode) -> u64 {
        let mut generation = self
            .bootstrap_generation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *generation = generation.wrapping_add(1);
        if *generation == 0 {
            *generation = 1;
        }
        let current_generation = *generation;
        self.bootstrap_status.send_modify(|snapshot| {
            snapshot.mode = mode;
            snapshot.complete = false;
            snapshot.ready = false;
            snapshot.error = None;
            // Per-identity causes are pass-scoped. `begin_identity_bootstrap`
            // already rebuilds every entry with `error: None`, but it runs only
            // AFTER the prepare/provider await, and `fail_identity_bootstrap`
            // is reachable before that (prepare failure in `bootstrap_roster`,
            // roster-provider failure in `refresh_desired_topology`). Without
            // this reset, its `entry.error.is_none()` guard cannot tell "this
            // identity produced its own cause during THIS pass" from "left
            // over from the last pass", and an early failure would report a
            // previous pass's cause beside the current pass's `error`.
            // Only causes are cleared: `refresh_aggregates` reads `entry.state`
            // exclusively, so no count, `ready`, or `materialization_terminal`
            // value moves here.
            for entry in snapshot.identities.values_mut() {
                entry.error = None;
            }
        });
        current_generation
    }

    /// Record a PASS-level bootstrap failure.
    ///
    /// Terminality is preserved deliberately: the barrier rule is
    /// `complete && dormant == 0 && warming == 0`
    /// ([`IdentityBootstrapStatus::materialization_terminal`]), and an eager
    /// pass seeds the whole roster `Warming`, so leaving those entries alone
    /// would hang `wait_identity_bootstrap_terminal` until its timeout
    /// instead of returning a truthful failure snapshot.
    ///
    /// What must NOT happen is attributing the pass cause to each identity as
    /// if it were that identity's own. Before embodiment became per identity,
    /// one member's `bridge create_session:` failure was copied onto 16 peers
    /// (downstream app activation-33 shape). Member failures now bypass this method,
    /// while a genuine pass failure stamps only what is known: the pass died
    /// before this identity's own outcome was recorded. A cause the identity actually produced
    /// during the pass (`mark_bootstrap_from_lifecycle`,
    /// `mark_bootstrap_materialization_finished`) is authoritative and is
    /// never overwritten. That equivalence between "the entry already has a
    /// cause" and "this pass produced it" is not free: it holds because
    /// `begin_identity_bootstrap_pending` resets per-entry causes when the
    /// pass opens, which is the only reason this `is_none()` guard cannot
    /// resurrect a previous pass's cause. Member embodiment failures no longer
    /// enter this path; they park only their own identity. The pass cause still rides
    /// `snapshot.error`, which the type documents as the pass-level slot and
    /// which every reader (RPC status, `wait_identity_bootstrap`, the Python
    /// SDK model) already parses.
    fn fail_identity_bootstrap(&self, generation: u64, error: &IdentityRuntimeError) {
        self.modify_bootstrap_status(Some(generation), |snapshot| {
            let detail = error.to_string();
            let unattributed = format!(
                "identity bootstrap pass failed before this identity reached its own \
                 outcome: {detail}"
            );
            snapshot.complete = true;
            snapshot.error = Some(detail);
            for entry in snapshot.identities.values_mut() {
                if entry.state != IdentityBootstrapState::Active {
                    entry.state = IdentityBootstrapState::Broken;
                    if entry.error.is_none() {
                        entry.error = Some(unattributed.clone());
                    }
                }
            }
            snapshot.refresh_aggregates();
        });
    }

    #[cfg(test)]
    pub(crate) fn test_supersede_identity_bootstrap_ready(&self) {
        let generation =
            self.begin_identity_bootstrap_pending(IdentityBootstrapMode::EagerMaterialize);
        self.modify_bootstrap_status(Some(generation), |snapshot| {
            snapshot.identities.clear();
            snapshot.complete = true;
            snapshot.error = None;
            snapshot.refresh_aggregates();
        });
    }

    #[cfg(test)]
    pub(crate) fn test_fail_identity_bootstrap(&self, detail: &str) {
        let generation =
            self.begin_identity_bootstrap_pending(IdentityBootstrapMode::EagerMaterialize);
        self.modify_bootstrap_status(Some(generation), |snapshot| {
            snapshot.identities.clear();
            if let Ok(identity) = AgentIdentity::parse("agent:test-bootstrap-failure") {
                snapshot.identities.insert(
                    identity,
                    IdentityBootstrapEntry {
                        state: IdentityBootstrapState::Dormant,
                        error: None,
                        restore: None,
                    },
                );
            }
            snapshot.refresh_aggregates();
        });
        self.fail_identity_bootstrap(
            generation,
            &IdentityRuntimeError::Internal(detail.to_string()),
        );
    }

    async fn install_identity_bootstrap(
        self: &Arc<Self>,
        generation: u64,
        mode: IdentityBootstrapMode,
        roster: &[DurableAgentSpec],
        result: &super::orchestrator::RestoreFlowResult,
    ) {
        let background_concurrency = match mode {
            IdentityBootstrapMode::LazyWithBackgroundWarm { concurrency } => Some(concurrency),
            _ => None,
        };
        // An unreadable first-boot manifest refuses every identity of the
        // pass; the pass-level slot names the file and the refusal once.
        let manifest_refusal = result.outcomes.values().find_map(|outcome| match outcome {
            super::orchestrator::RestoreOutcome::Refused(error)
                if matches!(
                    error.as_ref(),
                    IdentityRuntimeError::FirstBootManifestUnreadable { .. }
                ) =>
            {
                Some(error.to_string())
            }
            _ => None,
        });
        let mut status = IdentityBootstrapStatus {
            mode: mode.clone(),
            complete: background_concurrency.is_none(),
            ready: false,
            error: manifest_refusal,
            counts: Default::default(),
            identities: BTreeMap::new(),
            first_boot_pending: Vec::new(),
            deferred: Vec::new(),
        };
        // One read of Core's member history and of its member list serves
        // every identity of the pass. A failed history read leaves every
        // view Dormant, as a failed status read did.
        let history = self.core_view_history().await.ok();
        let broken = self.core_broken_members().await;
        for spec in roster {
            let refusal = result.outcomes.get(&spec.identity).and_then(|outcome| {
                if let super::orchestrator::RestoreOutcome::Refused(error) = outcome {
                    Some(error)
                } else {
                    None
                }
            });
            // A candidate's refusal of a roster change defers the identity to
            // the authoritative launch: it is not tracked, so it counts
            // neither as broken nor against readiness.
            if let Some(reason) = refusal.and_then(|error| candidate_deferral_of(error.as_ref())) {
                status.deferred.push(super::types::DeferredIdentity {
                    identity: spec.identity.clone(),
                    reason,
                });
                continue;
            }
            let broken_outcome = refusal.map(ToString::to_string);
            let lifecycle = match history.as_ref() {
                Some(history) => self
                    .status_from(&spec.identity, history.as_ref(), &broken)
                    .await
                    .ok()
                    .map(|item| item.state),
                None => None,
            };
            let (state, error) = match (broken_outcome, lifecycle) {
                // A refused wake settles the identity for this pass. The
                // view reads Dormant after a refusal that is not final, so
                // consulting it alone would make a terminal wait barrier
                // time out.
                (Some(detail), _) => (IdentityBootstrapState::Broken, Some(detail)),
                (None, Some(IdentityLifecycleState::Active)) => {
                    (IdentityBootstrapState::Active, None)
                }
                // Broken with no refused wake in this pass: Core holds the
                // identity's live member Broken, and its reason says why.
                (None, Some(IdentityLifecycleState::Broken)) => (
                    IdentityBootstrapState::Broken,
                    broken
                        .get(&crate::member_comms_id::roster_member_id_for_identity(
                            spec.identity.as_str(),
                        ))
                        .cloned(),
                ),
                (None, _) => (IdentityBootstrapState::Dormant, None),
            };
            // The eager pass's typed per-member progress survives into the
            // settled snapshot.
            let restore = match mode {
                IdentityBootstrapMode::EagerMaterialize => result
                    .outcomes
                    .get(&spec.identity)
                    .map(super::orchestrator::RestoreOutcome::restore_progress),
                _ => None,
            };
            status.identities.insert(
                spec.identity.clone(),
                IdentityBootstrapEntry {
                    state,
                    error,
                    restore,
                },
            );
        }
        status
            .deferred
            .sort_by(|left, right| left.identity.cmp(&right.identity));
        status.refresh_aggregates();
        log_candidate_deferral_census(&status);
        if !self.replace_bootstrap_status(generation, status.clone()) {
            return;
        }

        let Some(concurrency) = background_concurrency else {
            return;
        };
        let identities = status
            .identities
            .iter()
            .filter(|(_, entry)| entry.state == IdentityBootstrapState::Dormant)
            .map(|(identity, _)| identity.clone())
            .collect::<Vec<_>>();
        if identities.is_empty() {
            self.modify_bootstrap_status(Some(generation), |snapshot| {
                snapshot.complete = true;
                snapshot.refresh_aggregates();
            });
            return;
        }

        let runtime = Arc::clone(self);
        let deferred_before_warm = status.deferred.len();
        let (cancel, task_cancel) = watch::channel(false);
        let task = tokio::spawn(async move {
            stream::iter(identities.into_iter().map(|identity| {
                let runtime = Arc::clone(&runtime);
                let mut cancel = task_cancel.clone();
                async move {
                    if *cancel.borrow() {
                        return;
                    }
                    let Some(result) = runtime
                        .materialize_for_background(&identity, &mut cancel, generation)
                        .await
                    else {
                        return;
                    };
                    // A candidate's refusal of a roster change deferred the
                    // identity (`mark_bootstrap_materialization_finished`);
                    // it is no materialization failure.
                    if let Err(error) = result
                        && candidate_deferral_of(&error).is_none()
                    {
                        tracing::warn!(
                            %identity,
                            error = %error,
                            "identity background warm failed"
                        );
                        runtime
                            .record_best_effort_materialization_failure(
                                &identity,
                                None,
                                "background_warm",
                                &error,
                            )
                            .await;
                    }
                }
            }))
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;
            if !*task_cancel.borrow() {
                let mut warm_deferred = None;
                runtime.modify_bootstrap_status(Some(generation), |snapshot| {
                    snapshot.complete = true;
                    snapshot.refresh_aggregates();
                    if snapshot.deferred.len() > deferred_before_warm {
                        warm_deferred = Some(snapshot.clone());
                    }
                });
                // The warm wakes the dormant identities after the pass's
                // census: the ones a candidate deferred are named here.
                if let Some(status) = warm_deferred {
                    log_candidate_deferral_census(&status);
                }
            }
        });
        *self
            .bootstrap_cancel
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(cancel);
        *self.bootstrap_task.lock().await = Some(task);
    }

    fn mark_bootstrap_materialization_started(
        &self,
        identity: &AgentIdentity,
        generation: Option<u64>,
    ) {
        self.modify_bootstrap_status(generation, |snapshot| {
            let Some(entry) = snapshot.identities.get_mut(identity) else {
                return;
            };
            entry.state = IdentityBootstrapState::Warming;
            entry.error = None;
            snapshot.refresh_aggregates();
        });
    }

    fn mark_bootstrap_from_lifecycle(
        &self,
        identity: &AgentIdentity,
        lifecycle: IdentityLifecycleState,
        error: Option<String>,
    ) {
        self.modify_bootstrap_status(None, |snapshot| {
            let Some(entry) = snapshot.identities.get_mut(identity) else {
                return;
            };
            entry.state = match lifecycle {
                IdentityLifecycleState::Active => IdentityBootstrapState::Active,
                IdentityLifecycleState::Broken => IdentityBootstrapState::Broken,
                IdentityLifecycleState::Dormant => IdentityBootstrapState::Dormant,
            };
            entry.error = if entry.state == IdentityBootstrapState::Broken {
                error
            } else {
                None
            };
            snapshot.refresh_aggregates();
        });
    }

    fn mark_bootstrap_materialization_finished<T>(
        &self,
        identity: &AgentIdentity,
        result: &Result<T, IdentityRuntimeError>,
        generation: Option<u64>,
    ) {
        let deferral = result.as_ref().err().and_then(candidate_deferral_of);
        self.modify_bootstrap_status(generation, |snapshot| {
            if !snapshot.identities.contains_key(identity) {
                return;
            }
            if let Some(reason) = deferral {
                snapshot.defer(identity, reason);
                return;
            }
            let Some(entry) = snapshot.identities.get_mut(identity) else {
                return;
            };
            match result {
                Ok(_) => {
                    entry.state = IdentityBootstrapState::Active;
                    entry.error = None;
                }
                Err(error) => {
                    entry.state = IdentityBootstrapState::Broken;
                    entry.error = Some(error.to_string());
                }
            }
            snapshot.refresh_aggregates();
        });
    }

    /// Report `identity` as deferred to the authoritative launch when
    /// `error` is a candidate launch's refusal of a roster change
    /// ([`candidate_deferral_of`]): it leaves the tracked identities, so it
    /// counts neither as broken nor against the bootstrap's readiness.
    /// Returns whether it did; any other error is the caller's to record.
    pub(crate) fn defer_bootstrap_identity_on_candidate_refusal(
        &self,
        identity: &AgentIdentity,
        error: &IdentityRuntimeError,
    ) -> bool {
        let Some(reason) = candidate_deferral_of(error) else {
            return false;
        };
        self.modify_bootstrap_status(None, |snapshot| {
            if snapshot.identities.contains_key(identity) {
                snapshot.defer(identity, reason);
            }
        });
        true
    }

    fn mark_bootstrap_materialization_cancelled(
        &self,
        identity: &AgentIdentity,
        generation: Option<u64>,
    ) {
        self.modify_bootstrap_status(generation, |snapshot| {
            if let Some(entry) = snapshot.identities.get_mut(identity) {
                entry.state = IdentityBootstrapState::Dormant;
                entry.error = None;
                snapshot.refresh_aggregates();
            }
        });
    }

    /// Publish one identity's typed eager-restore progress on the bootstrap
    /// status as the restore pass reaches it. A settled stage also settles
    /// the identity's bootstrap state, so per-member barriers can pass before
    /// the whole pass ends.
    pub(crate) fn publish_restore_progress(
        &self,
        identity: &AgentIdentity,
        progress: IdentityRestoreProgress,
        error: Option<String>,
    ) {
        self.modify_bootstrap_status(None, |snapshot| {
            let Some(entry) = snapshot.identities.get_mut(identity) else {
                return;
            };
            match &progress {
                IdentityRestoreProgress::Minted | IdentityRestoreProgress::Resumed => {
                    entry.state = IdentityBootstrapState::Active;
                    entry.error = None;
                }
                IdentityRestoreProgress::Refused { .. } => {
                    entry.state = IdentityBootstrapState::Broken;
                    entry.error = error;
                }
                IdentityRestoreProgress::Registered | IdentityRestoreProgress::Resuming => {}
            }
            entry.restore = Some(progress);
            snapshot.refresh_aggregates();
        });
    }

    /// Exact concrete member ids represented by the tracked bootstrap roster.
    /// Used only after `ready == true`, preventing a false-ready snapshot of a
    /// partially warmed mob.
    pub async fn identity_bootstrap_member_ids(&self) -> Vec<meerkat_mob::ids::AgentIdentity> {
        let tracked = self.identity_bootstrap_status();
        self.identity_bootstrap_member_ids_for_status(&tracked)
            .await
    }

    pub(crate) async fn identity_bootstrap_member_ids_for_status(
        &self,
        tracked: &IdentityBootstrapStatus,
    ) -> Vec<meerkat_mob::ids::AgentIdentity> {
        // The roster is keyed by the encoded DURABLE identity for every
        // binding. This used to mirror the old spawn-side lowering (identity
        // for external, runtime id for local); keeping that split would report
        // member ids no roster row answers to, and the bootstrap status would
        // look healthy while naming nothing.
        let registered: Vec<AgentIdentity> = {
            let entries = self.entries.read().await;
            tracked
                .identities
                .keys()
                .filter(|identity| entries.contains_key(*identity))
                .cloned()
                .collect()
        };
        let mut members = Vec::new();
        for identity in registered {
            // Core's history gates inclusion: an identity Core never spawned
            // (since the latest mob reset) is not a bootstrapped member.
            if matches!(self.core_last_binding(&identity).await, Ok(Some(_))) {
                members.push(crate::member_comms_id::roster_member_id_for_identity(
                    identity.as_str(),
                ));
            }
        }
        members
    }

    pub async fn set_agent_customizer(&self, customizer: Option<Arc<dyn AgentCustomizer>>) {
        *self.customizer.write().await = customizer;
    }

    /// Install the registry whose per-identity dispatchers carry
    /// `customize_build` tools across meerkat-side rebuilds (#563). The same
    /// registry must back the mob's `CustomizerToolsSpawnCustomizer`.
    pub async fn set_customizer_tool_registry(
        &self,
        registry: Option<Arc<super::customizer_tools::CustomizerToolRegistry>>,
    ) {
        *self.customizer_tools.write().await = registry;
    }

    /// Publish one successful `customize_build` result for `identity` and
    /// point the draft at the identity's stable dispatcher, so the spawn (or
    /// the adopted occupant, which already holds it) serves the new tools.
    async fn publish_customizer_tools(
        &self,
        identity: &AgentIdentity,
        draft: &mut AgentBuildDraft,
    ) {
        let Some(registry) = self.customizer_tools.read().await.clone() else {
            return;
        };
        let entry = registry.publish(identity, draft.local_external_tools.dispatcher());
        draft.local_external_tools = super::types::LocalExternalToolOverlay::new(entry);
        let was_pending = self
            .customizer_tools_pending
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(identity)
            .is_some();
        // The mob build held this restored member's run starts while its
        // tools were unpublished; they are published now, so release it. A
        // candidate launch releases nothing: its members' runs belong to the
        // authoritative launch.
        if was_pending
            && !self.is_candidate_launch()
            && let Some(bridge) = self.bridge.as_ref()
        {
            let member_id = crate::member_comms_id::mob_member_id(identity.as_str());
            if let Err(error) = bridge
                .release_member_run_starts_for_published_tools(&member_id)
                .await
            {
                let reason = format!("customizer tools published, but {error}");
                tracing::error!(
                    %identity,
                    %reason,
                    "restored member stays held: it starts no run until the hold is released"
                );
                self.customizer_tools_pending
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(
                        identity.clone(),
                        CustomizerToolsPending {
                            reason,
                            artifact_refusal: None,
                        },
                    );
            }
        }
    }

    pub(crate) fn customizer_tools_pending_for(
        &self,
        identity: &AgentIdentity,
    ) -> Option<CustomizerToolsPending> {
        self.customizer_tools_pending
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(identity)
            .cloned()
    }

    /// Register every roster identity's customizer-tool dispatcher before the
    /// mob is lifted, so Core's restart restore attaches it.
    pub(crate) async fn ensure_customizer_tool_dispatchers(&self, roster: &[DurableAgentSpec]) {
        if let Some(registry) = self.customizer_tools.read().await.clone() {
            for spec in roster {
                registry.ensure(&spec.identity);
            }
        }
    }

    /// Publish each roster identity's customizer tools BEFORE the mob is
    /// lifted (#563). meerkat's restore at activation then builds every
    /// restored member with its tools already present: its tool scope is
    /// right from the first build, and a run that starts at activation (a
    /// queued input, a kickoff, a peer message) sees them. The materialization
    /// that follows runs `customize_build` again and republishes through the
    /// same atomic swap. A failure here is logged per identity and recorded as
    /// `IdentityStatus::customizer_tools_pending` until the materialization
    /// publishes.
    ///
    /// An identity already pending (the mob build's early publication
    /// failed, so its restore holds the member's run starts) that publishes
    /// here stays pending: the hold is still staged, and only
    /// [`Self::release_customizer_tool_holds`] after the lift, or a later
    /// publication, lets it go. Returns those identities.
    pub async fn prepublish_customizer_tools(
        &self,
        roster: &[DurableAgentSpec],
        customizer: Option<&dyn AgentCustomizer>,
    ) -> Vec<AgentIdentity> {
        let Some(registry) = self.customizer_tools.read().await.clone() else {
            return Vec::new();
        };
        let installed = self.customizer.read().await.clone();
        let Some(customizer) = customizer.or(installed.as_deref()) else {
            return Vec::new();
        };
        let active_peers: Vec<AgentIdentity> = self.entries.read().await.keys().cloned().collect();
        let managed_edges = self.desired_peer_edges.read().await.clone();
        let failed = super::customizer_tools::prepublish(
            &registry,
            roster,
            customizer,
            self.runtime_services(),
            &active_peers,
            &managed_edges,
        )
        .await;
        let mut pending = self
            .customizer_tools_pending
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut release_owed = Vec::new();
        for spec in roster {
            if failed.contains_key(&spec.identity) {
                continue;
            }
            if pending.contains_key(&spec.identity) {
                release_owed.push(spec.identity.clone());
            }
        }
        pending.extend(failed);
        release_owed
    }

    /// The roster entries whose customizer tools a bootstrap with no lift
    /// staged still owes: the mob resumed Running (an unclean stop left its
    /// log Running), so meerkat's own resume revived its persisted members
    /// while the mob was built, before any activation step. Each entry names
    /// a member Core already holds live, or one Core holds a binding for
    /// that is still pending (the mob build's early publication failed, so
    /// meerkat holds its member's run starts).
    ///
    /// A first boot creates the mob, so no lift is staged there either, but
    /// Core holds no binding yet: it owes nothing, even when the mob build's
    /// early publication failed. Its members are created by the roster pass,
    /// which publishes their tools itself, and the early failure stays
    /// pending until that publication. Without a readable binding history
    /// every pending entry stays owed.
    pub(crate) async fn roster_owing_customizer_tools_after_resume(
        &self,
        roster: &[DurableAgentSpec],
    ) -> Vec<DurableAgentSpec> {
        let history = match self.bridge.as_ref() {
            Some(bridge) => bridge.member_binding_history().await.ok(),
            None => None,
        };
        let pending = self
            .customizer_tools_pending
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        roster
            .iter()
            .filter(|spec| {
                let facts = CoreViewFacts::of(history.as_ref(), &spec.identity);
                facts.live
                    || pending.contains(&spec.identity)
                        && (history.is_none() || facts.member_id.is_some())
            })
            .cloned()
            .collect()
    }

    /// Release the run-start hold of each identity whose customizer tools
    /// were published before the lift although the mob build's earlier
    /// publication had failed (see [`Self::prepublish_customizer_tools`]).
    /// Called once the mob is live, since meerkat staged the hold at the
    /// build and a release must reach it. A failed release keeps the
    /// identity pending with the reason. A candidate launch releases
    /// nothing: the tools are published, and the member stays held.
    pub(crate) async fn release_customizer_tool_holds(&self, identities: &[AgentIdentity]) {
        let Some(bridge) = self.bridge.as_ref() else {
            return;
        };
        if self.is_candidate_launch() {
            let mut pending = self
                .customizer_tools_pending
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for identity in identities {
                pending.remove(identity);
            }
            return;
        }
        for identity in identities {
            let member_id = crate::member_comms_id::mob_member_id(identity.as_str());
            match bridge
                .release_member_run_starts_for_published_tools(&member_id)
                .await
            {
                Ok(()) => {
                    self.customizer_tools_pending
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(identity);
                }
                Err(error) => {
                    let reason = format!("customizer tools published, but {error}");
                    tracing::error!(
                        %identity,
                        %reason,
                        "restored member stays held: it starts no run until the hold is released"
                    );
                    self.customizer_tools_pending
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(
                            identity.clone(),
                            CustomizerToolsPending {
                                reason,
                                artifact_refusal: None,
                            },
                        );
                }
            }
        }
    }

    /// Publish the customizer tools of a member Core holds live while they
    /// are still pending (every publication before the lift failed): meerkat
    /// holds its run starts until they are published, and nothing rebuilds a
    /// live member, so its wake or its delivery publishes them here, as a
    /// materialization did. The publication releases the hold.
    ///
    /// While they cannot be published the wake or delivery is refused, so
    /// nothing is admitted behind the hold: a refused build answers
    /// `ToolArtifactRefused` or `IdentityWakeRetryable` and keeps the
    /// identity pending with the new reason. The host's build is bounded by
    /// the runtime's shutdown, like the other wake builds.
    async fn publish_pending_customizer_tools(
        &self,
        identity: &AgentIdentity,
        session_id: Option<&SessionId>,
        operation: &'static str,
    ) -> Result<(), IdentityRuntimeError> {
        if self.customizer_tools_pending_for(identity).is_none() {
            return Ok(());
        }
        let Some(customizer) = self.customizer.read().await.clone() else {
            return Ok(());
        };
        let Some(spec) = self
            .entries
            .read()
            .await
            .get(identity)
            .map(|entry| entry.spec.clone())
        else {
            return Ok(());
        };
        let refused = |detail: String| IdentityRuntimeError::IdentityWakeRetryable {
            identity: identity.clone(),
            session_id: session_id.cloned(),
            detail,
        };
        let mut shutdown = self.foreground_cancel.subscribe();
        if *shutdown.borrow() {
            return Err(IdentityRuntimeError::Internal(format!(
                "identity {operation} cancelled during shutdown before its customizer tools \
                 were published"
            )));
        }
        let build_context = AgentBuildContext {
            identity: identity.clone(),
            active_peers: self.entries.read().await.keys().cloned().collect(),
            managed_edges: self.desired_peer_edges.read().await.clone(),
            runtime_services: self.runtime_services(),
        };
        let mut draft = Self::base_build_draft(&spec);
        let customize_result = {
            let customize = customizer.customize_build(&build_context, &spec, &mut draft);
            tokio::pin!(customize);
            tokio::select! {
                result = &mut customize => result,
                changed = shutdown.changed() => {
                    if changed.is_ok() && *shutdown.borrow() {
                        return Err(IdentityRuntimeError::Internal(format!(
                            "identity {operation} cancelled during shutdown before its \
                             customizer tools were published"
                        )));
                    }
                    customize.await
                }
            }
        };
        match customize_result {
            Ok(()) => {
                self.publish_customizer_tools(identity, &mut draft).await;
                // A release that failed keeps the identity pending (and the
                // member held): nothing is admitted behind it.
                match self.customizer_tools_pending_for(identity) {
                    None => Ok(()),
                    Some(pending) => Err(refused(pending.reason)),
                }
            }
            Err(error) => {
                let reason = format!("customize_build failed: {error}");
                let artifact_refusal = match &error {
                    super::types::CustomizerError::ToolArtifactRefused(refusal) => {
                        Some(refusal.as_ref().clone())
                    }
                    _ => None,
                };
                tracing::warn!(
                    %identity,
                    %reason,
                    "live member's customizer tools are still unpublished; its {operation} is \
                     refused until they are"
                );
                self.customizer_tools_pending
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(
                        identity.clone(),
                        CustomizerToolsPending {
                            reason: reason.clone(),
                            artifact_refusal,
                        },
                    );
                if let super::types::CustomizerError::ToolArtifactRefused(refusal) = error {
                    return Err(IdentityRuntimeError::ToolArtifactRefused(refusal));
                }
                Err(refused(format!(
                    "the member's customizer tools are not published ({reason}); it starts no \
                     run until they are, retry"
                )))
            }
        }
    }

    /// Record identities whose customizer tools a pre-build publication could
    /// not publish (see [`super::customizer_tools::prepublish`]).
    pub fn record_customizer_tools_pending(
        &self,
        failed: BTreeMap<AgentIdentity, CustomizerToolsPending>,
    ) {
        self.customizer_tools_pending
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(failed);
    }

    /// Mark this runtime as serving a candidate launch
    /// (`runtime_options.mob_composition.authority = "candidate"`), before
    /// its bootstrap. The mob build holds every stored member's run starts on
    /// a candidate, and this runtime never releases them: a publication of a
    /// member's customizer tools still publishes, and leaves the member held.
    /// Its roster pass changes no stored member either: it retires none,
    /// respawns none, creates none and moves none to another role, and
    /// logs what it would have changed
    /// ([`Self::census_roster_changes_for_candidate`]).
    pub(crate) fn mark_candidate_launch(&self) {
        self.candidate_launch.store(true, Ordering::Release);
    }

    /// Whether this runtime serves a candidate launch
    /// ([`Self::mark_candidate_launch`]).
    pub(crate) fn is_candidate_launch(&self) -> bool {
        self.candidate_launch.load(Ordering::Acquire)
    }

    pub async fn set_agent_memory(&self, injector: Option<AgentMemoryRuntimeInjector>) {
        *self.agent_memory.write().await = injector;
    }

    pub async fn agent_memory_supports_recall(&self) -> bool {
        self.agent_memory.read().await.is_some()
    }

    pub async fn agent_memory_supports_remember(&self) -> bool {
        self.agent_memory
            .read()
            .await
            .as_ref()
            .is_some_and(|injector| injector.provider().supports_remember())
    }

    pub async fn agent_memory_supports_forget(&self) -> bool {
        self.agent_memory
            .read()
            .await
            .as_ref()
            .is_some_and(|injector| injector.provider().supports_forget())
    }

    pub async fn agent_memory_supports_update(&self) -> bool {
        self.agent_memory
            .read()
            .await
            .as_ref()
            .is_some_and(|injector| injector.provider().supports_supersede())
    }

    pub async fn agent_memory_supports_manifest(&self) -> bool {
        self.agent_memory
            .read()
            .await
            .as_ref()
            .is_some_and(|injector| injector.provider().supports_manifest())
    }

    pub async fn remember_agent_memory(
        &self,
        realm: &str,
        identity: &AgentIdentity,
        memory: NewAgentMemory,
    ) -> Result<AgentMemoryRecord, AgentMemoryError> {
        self.status(identity)
            .await
            .map_err(|err| AgentMemoryError::InvalidConfig(err.to_string()))?;
        let provider = self
            .agent_memory
            .read()
            .await
            .as_ref()
            .map(AgentMemoryRuntimeInjector::provider)
            .ok_or_else(|| {
                AgentMemoryError::InvalidConfig("agent memory is not configured".to_string())
            })?;
        provider.remember(realm, identity, memory).await
    }

    pub async fn forget_agent_memory(
        &self,
        realm: &str,
        identity: &AgentIdentity,
        memory_id: &str,
    ) -> Result<AgentMemoryForgetResult, AgentMemoryError> {
        self.status(identity)
            .await
            .map_err(|err| AgentMemoryError::InvalidConfig(err.to_string()))?;
        let provider = self
            .agent_memory
            .read()
            .await
            .as_ref()
            .map(AgentMemoryRuntimeInjector::provider)
            .ok_or_else(|| {
                AgentMemoryError::InvalidConfig("agent memory is not configured".to_string())
            })?;
        provider.forget(realm, identity, memory_id).await
    }

    /// Supersede `memory_id` within its lineage (the D4 fix): the new
    /// title/body/tags become the active record; the prior stays
    /// retrievable with provenance.
    pub async fn update_agent_memory(
        &self,
        realm: &str,
        identity: &AgentIdentity,
        memory_id: &str,
        memory: NewAgentMemory,
    ) -> Result<MemoryId, AgentMemoryError> {
        self.status(identity)
            .await
            .map_err(|err| AgentMemoryError::InvalidConfig(err.to_string()))?;
        let provider = self
            .agent_memory
            .read()
            .await
            .as_ref()
            .map(AgentMemoryRuntimeInjector::provider)
            .ok_or_else(|| {
                AgentMemoryError::InvalidConfig("agent memory is not configured".to_string())
            })?;
        let scope = MemoryScope::Identity {
            realm: realm.to_string(),
            identity: identity.as_str().to_string(),
        };
        let record = NewMemoryRecord {
            kind: MemoryKind::Fact,
            title: memory.title,
            description: String::new(),
            body: memory.body,
            tags: memory.tags,
            evidence: Vec::new(),
            verification: None,
        };
        provider.supersede(&scope, memory_id, record).await
    }

    /// Tiered metadata manifest for the identity's own scope (§8.3).
    pub async fn manifest_agent_memory(
        &self,
        realm: &str,
        identity: &AgentIdentity,
        tier: ManifestTier,
    ) -> Result<Vec<RecordMeta>, AgentMemoryError> {
        self.status(identity)
            .await
            .map_err(|err| AgentMemoryError::InvalidConfig(err.to_string()))?;
        let provider = self
            .agent_memory
            .read()
            .await
            .as_ref()
            .map(AgentMemoryRuntimeInjector::provider)
            .ok_or_else(|| {
                AgentMemoryError::InvalidConfig("agent memory is not configured".to_string())
            })?;
        let scope = MemoryScope::Identity {
            realm: realm.to_string(),
            identity: identity.as_str().to_string(),
        };
        provider.manifest(&[scope], tier).await
    }

    pub async fn recall_agent_memory(
        &self,
        request: AgentMemoryRecallRequest,
    ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryError> {
        self.status(&request.identity)
            .await
            .map_err(|err| AgentMemoryError::InvalidConfig(err.to_string()))?;
        let provider = self
            .agent_memory
            .read()
            .await
            .as_ref()
            .map(AgentMemoryRuntimeInjector::provider)
            .ok_or_else(|| {
                AgentMemoryError::InvalidConfig("agent memory is not configured".to_string())
            })?;
        let records = provider.recall(request).await?;
        // §9.2: explicit recall reads mark usage mechanically. Telemetry
        // never fails the read — providers without usage support
        // (markdown) return Unsupported, which is downgraded here.
        if !records.is_empty() {
            let ids: Vec<MemoryId> = records
                .iter()
                .map(|record| record.memory_id.clone())
                .collect();
            if let Err(err) = provider.mark_usage(&ids, UsageEvent::ExplicitRecall).await {
                tracing::debug!(error = %err, "agent memory explicit-recall usage marking skipped");
            }
        }
        Ok(records)
    }

    /// Attach the roster provider reset should consult for current specs.
    pub fn set_reset_roster_provider(&self, provider: Option<Arc<dyn RosterProvider>>) {
        self.set_reset_roster_provider_context(provider, None);
    }

    /// Attach the roster provider and context reset should consult for current specs.
    pub fn set_reset_roster_provider_context(
        &self,
        provider: Option<Arc<dyn RosterProvider>>,
        mob_definition: Option<meerkat_mob::MobDefinition>,
    ) {
        let source = provider.map(|provider| ResetRosterSource {
            provider,
            mob_definition,
        });
        match self.reset_roster_source.write() {
            Ok(mut stored_source) => *stored_source = source,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "identity runtime reset roster source lock poisoned; dropping provider update"
                );
            }
        }
    }

    async fn adopt_current_roster_spec_for_reset(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), IdentityRuntimeError> {
        let source = match self.reset_roster_source.read() {
            Ok(stored_source) => stored_source.clone(),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "reset: roster source lock poisoned; rebuilding on stored spec"
                );
                None
            }
        };
        if let Some(source) = source {
            self.adopt_roster_spec_with_context(
                &source.provider,
                identity,
                source.mob_definition.clone(),
            )
            .await?;
        }
        Ok(())
    }

    /// Attach a best-effort operational error hook used for alerting.
    /// Declare where the operator's `rkat --state-root .. --realm ..` scope
    /// for [`SessionRepairRequired`] commands points. Hosts that embed Meta
    /// in a meerkat realm know both; a field left `None` renders as an
    /// explicit placeholder rather than a guessed path.
    pub fn set_session_repair_scope(&self, scope: Option<SessionRepairScope>) {
        *self
            .session_repair_scope
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = scope;
    }

    pub fn session_repair_scope(&self) -> Option<SessionRepairScope> {
        self.session_repair_scope
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn set_error_hook(&self, hook: Option<crate::unified_runtime::ErrorHook>) {
        match self.error_hook.write() {
            Ok(mut stored_hook) => *stored_hook = hook,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "identity runtime error hook lock poisoned; dropping hook update"
                );
            }
        }
    }

    /// Install (or clear) the host's retirement hook. A host-composed
    /// gateway installs it right after the host binds the identity runtime,
    /// before the identity bootstrap, so a boot pass's retirements reach it.
    pub(crate) fn set_retired_member_observer(&self, observer: Option<RetiredMemberObserver>) {
        *self
            .retired_member_observer
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = observer;
    }

    /// Test seam: whether no roster pass holds the bootstrap controller now.
    #[cfg(test)]
    pub(crate) fn bootstrap_controller_is_free(&self) -> bool {
        self.bootstrap_controller.try_lock().is_ok()
    }

    fn retired_member_observer(&self) -> Option<RetiredMemberObserver> {
        self.retired_member_observer
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The incarnation a retire of `identity` is about to retire, for the
    /// host's retirement hook: the live binding in Core's member history
    /// and the exact registration its session holds, read under the
    /// identity's lifecycle lock before Core retires it (Core hands out
    /// only a current registration). `None` when no hook is installed
    /// (nothing extra is read) or Core holds no live member.
    async fn retiring_incarnation(
        &self,
        identity: &AgentIdentity,
        cause: super::types::RetiredIdentityCause,
    ) -> Result<Option<super::types::RetiredIdentityMember>, IdentityRuntimeError> {
        if self.retired_member_observer().is_none() {
            return Ok(None);
        }
        let Some(binding) = self.core_last_binding(identity).await? else {
            return Ok(None);
        };
        let Some(agent_runtime_id) = live_binding_runtime_id(&binding) else {
            return Ok(None);
        };
        let registration = match (binding.session_id(), self.bridge.as_ref()) {
            (Some(session_id), Some(bridge)) => bridge.member_registration(session_id).await,
            _ => None,
        };
        Ok(Some(super::types::RetiredIdentityMember {
            identity: identity.clone(),
            member: binding.identity().as_str().to_string(),
            role: binding.role().clone(),
            agent_runtime_id,
            generation: binding.generation().get(),
            session_id: binding.session_id().cloned(),
            registration,
            cause,
        }))
    }

    /// Right after Core's retire answered, still under the identity's
    /// lifecycle lock: Core's retire answers only once the retired
    /// registration's teardown removed it, so a registration still current
    /// here was not ended by this retire. The host is then told of no
    /// registration and keeps what it bound to it.
    async fn settle_retired_registration(
        &self,
        retiring: &mut Option<super::types::RetiredIdentityMember>,
    ) {
        let Some(member) = retiring.as_mut() else {
            return;
        };
        let still_current = match (member.registration.as_ref(), self.bridge.as_ref()) {
            (Some(registration), Some(bridge)) => {
                bridge
                    .member_registration(registration.session_id())
                    .await
                    .as_ref()
                    == Some(registration)
            }
            _ => false,
        };
        if still_current {
            tracing::warn!(
                identity = %member.identity,
                agent_runtime_id = %member.agent_runtime_id,
                "Core's retire answered while the retired incarnation's registration is \
                 still current; the host's retirement hook is told of no registration"
            );
            member.registration = None;
        }
    }

    /// Hand each retired incarnation to the host's retirement hook, in
    /// order. Callers hold no Meta lock here (no identity lifecycle lock, no
    /// bootstrap controller), so the hook may call back into this runtime.
    /// A hook error leaves the retirement standing: it is reported once to
    /// the error hook and not retried.
    pub(crate) async fn release_retired_members(
        &self,
        retired: Vec<super::types::RetiredIdentityMember>,
    ) {
        if retired.is_empty() {
            return;
        }
        let Some(observer) = self.retired_member_observer() else {
            return;
        };
        for member in retired {
            let identity = member.identity.to_string();
            let agent_runtime_id = member.agent_runtime_id.to_string();
            let session_id = member.session_id.as_ref().map(ToString::to_string);
            let cause = member.cause.as_str().to_string();
            if let Err(error) = observer(member).await {
                self.emit_error(
                    crate::unified_runtime::types::ErrorEvent::RetiredMemberReleaseFailed {
                        identity,
                        agent_runtime_id,
                        session_id,
                        cause,
                        error,
                    },
                );
            }
        }
    }

    /// Remember the owning `Arc` (weakly) for detached post-admission work.
    fn remember_self_handle(self: &Arc<Self>) {
        let _ = self.self_handle.set(Arc::downgrade(self));
    }

    /// The owning `Arc`, when an `Arc`-receiving entry point has run.
    fn shared_handle(&self) -> Option<Arc<Self>> {
        self.self_handle.get().and_then(std::sync::Weak::upgrade)
    }

    pub async fn set_desired_peer_edges(&self, edges: Vec<ManagedPeerEdge>) {
        *self.desired_peer_edges.write().await = edges;
    }

    pub(crate) fn set_topology_controller(
        &self,
        controller: crate::topology_control::TopologyController,
    ) {
        *self
            .topology_controller
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(controller);
    }

    fn topology_controller(&self) -> Option<crate::topology_control::TopologyController> {
        self.topology_controller
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub async fn desired_peer_edges(&self) -> Vec<ManagedPeerEdge> {
        let declared = self.desired_peer_edges.read().await.clone();
        match self.topology_controller() {
            Some(controller) => {
                // Materialization/console reads must not observe the target
                // intent while a topology transaction is between WAL and
                // terminal commit/rollback.
                let _admission = controller.mutation_guard().await;
                match controller.compose_managed_peer_edges(&declared).await {
                    Ok(edges) => edges,
                    Err(error) => {
                        tracing::error!(error = %error, "failed to compose identity topology overlay");
                        Vec::new()
                    }
                }
            }
            None => declared,
        }
    }

    async fn registered_identities(&self) -> Vec<AgentIdentity> {
        self.entries.read().await.keys().cloned().collect()
    }

    async fn reachable_peer_identities(&self, identity: &AgentIdentity) -> Vec<AgentIdentity> {
        self.desired_peer_edges()
            .await
            .iter()
            .filter_map(|edge| {
                if edge.a() == identity {
                    Some(edge.b().clone())
                } else if edge.b() == identity {
                    Some(edge.a().clone())
                } else {
                    None
                }
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    #[must_use]
    pub fn has_session_bridge(&self) -> bool {
        self.bridge.is_some()
    }

    pub(crate) async fn logical_peer_edges(
        &self,
    ) -> Result<Vec<ManagedPeerEdge>, IdentityRuntimeError> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(Vec::new());
        };
        let runtime_identities: BTreeMap<AgentRuntimeId, AgentIdentity> = self
            .core_live_runtime_ids()
            .await
            .into_iter()
            .map(|(identity, runtime_id)| (runtime_id, identity))
            .collect();
        let runtime_edges = bridge.current_member_wires().await.map_err(|error| {
            IdentityRuntimeError::Internal(format!("bridge current_member_wires: {error}"))
        })?;
        Ok(runtime_edges
            .into_iter()
            .filter_map(|(runtime_a, runtime_b)| {
                let a = runtime_identities.get(&runtime_a)?.clone();
                let b = runtime_identities.get(&runtime_b)?.clone();
                ManagedPeerEdge::new(a, b).ok()
            })
            .collect())
    }

    pub(crate) async fn logical_peer_edges_any_half(
        &self,
    ) -> Result<Vec<ManagedPeerEdge>, IdentityRuntimeError> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(Vec::new());
        };
        let runtime_identities: BTreeMap<AgentRuntimeId, AgentIdentity> = self
            .core_live_runtime_ids()
            .await
            .into_iter()
            .map(|(identity, runtime_id)| (runtime_id, identity))
            .collect();
        let runtime_edges = bridge
            .current_member_wires_any_half()
            .await
            .map_err(|error| {
                IdentityRuntimeError::Internal(format!(
                    "bridge current_member_wires_any_half: {error}"
                ))
            })?;
        Ok(runtime_edges
            .into_iter()
            .filter_map(|(runtime_a, runtime_b)| {
                let a = runtime_identities.get(&runtime_a)?.clone();
                let b = runtime_identities.get(&runtime_b)?.clone();
                ManagedPeerEdge::new(a, b).ok()
            })
            .collect())
    }

    pub(crate) fn identity_edge_ownership(&self) -> IdentityEdgeOwnership {
        *self
            .identity_edge_ownership
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Identity edges the mob definition's wiring declares over this
    /// runtime's identities (`auto_wire_orchestrator` / `role_wiring`, an
    /// identity's profile as its role). They are the definition's: its
    /// spawn-time wiring and edge reconciler own them, so the topology never
    /// prunes them.
    pub(crate) async fn definition_declared_identity_edges(
        &self,
    ) -> BTreeSet<(AgentIdentity, AgentIdentity)> {
        let definition = self
            .reset_roster_source
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|source| source.mob_definition.clone());
        let Some(policy) = definition.as_ref().and_then(
            crate::unified_runtime::edge_reconcile::DefinitionWiringEdgeDiscovery::from_definition,
        ) else {
            return BTreeSet::new();
        };
        let members: Vec<crate::unified_runtime::edge_types::EdgeMemberView> = self
            .entries
            .read()
            .await
            .iter()
            .map(
                |(identity, entry)| crate::unified_runtime::edge_types::EdgeMemberView {
                    agent_identity: identity.as_str().to_string(),
                    role: entry.spec.profile.to_string(),
                    wired_to: BTreeSet::new(),
                    labels: BTreeMap::new(),
                },
            )
            .collect();
        policy
            .desired_edges(&members)
            .into_iter()
            .filter_map(|edge| {
                let (a, b) = edge.endpoints();
                let a = AgentIdentity::parse(a).ok()?;
                let b = AgentIdentity::parse(b).ok()?;
                Some(if a <= b { (a, b) } else { (b, a) })
            })
            .collect()
    }

    pub(crate) fn set_identity_edge_ownership(&self, ownership: IdentityEdgeOwnership) {
        *self
            .identity_edge_ownership
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = ownership;
    }

    pub(crate) async fn managed_peer_edges_snapshot(
        &self,
    ) -> BTreeSet<(AgentIdentity, AgentIdentity)> {
        self.managed_peer_edges.read().await.clone()
    }

    pub(crate) async fn retain_managed_peer_edges(
        &self,
        edges: &BTreeSet<(AgentIdentity, AgentIdentity)>,
    ) {
        self.managed_peer_edges
            .write()
            .await
            .extend(edges.iter().cloned());
    }

    /// Logical identity actuator used only while the shared topology
    /// controller's admission lock is already held by TopologyRuntimeHandle.
    ///
    /// Same plan / wire / revalidate contract as
    /// [`Self::reconcile_managed_peer_edges_admitted`]: the endpoints'
    /// runtime bindings are resolved from one snapshot, the bounded bridge
    /// calls run with no lifecycle lock held, and the bookkeeping commits
    /// (under the entries read lock, never a lifecycle lock) only if neither
    /// binding moved; otherwise the mutation is replanned against the live
    /// bindings. A wiring error while an endpoint's binding moved (e.g. it
    /// was just retired) is a replan too, not a failure. A wiring call that
    /// timed out leaves the edge in the unsettled set for the next reconcile.
    pub(crate) async fn mutate_managed_peer_edge_admitted(
        &self,
        action: crate::topology_control::TopologyAction,
        edge: &ManagedPeerEdge,
    ) -> Result<(), IdentityRuntimeError> {
        let _guard = self.managed_peer_reconcile_lock.lock().await;
        let Some(bridge) = self.bridge.clone() else {
            return Err(IdentityRuntimeError::Internal(
                "topology mutation requires a session bridge".to_string(),
            ));
        };
        let endpoints: BTreeSet<AgentIdentity> =
            [edge.a().clone(), edge.b().clone()].into_iter().collect();
        let key = (edge.a().clone(), edge.b().clone());
        for _attempt in 0..PEER_TOPOLOGY_MAX_PLAN_ATTEMPTS {
            let epoch = self.peer_binding_epoch().await;
            match self
                .mutate_managed_peer_edge_attempt(&bridge, action, edge, &key, &endpoints, &epoch)
                .await
            {
                Ok(PeerTopologyAttempt::Committed) => return Ok(()),
                Ok(PeerTopologyAttempt::Stale) => {}
                Err(error) => {
                    if matches!(error, IdentityRuntimeError::PeerWiringTimedOut { .. }) {
                        self.record_unsettled_peer_wiring([key.clone()]).await;
                        return Err(error);
                    }
                    if self.peer_bindings_moved(&epoch, &endpoints).await {
                        tracing::debug!(
                            edge = ?key,
                            %error,
                            "identity-first topology mutation failed while an endpoint's binding \
                             moved; replanning"
                        );
                    } else {
                        return Err(error);
                    }
                }
            }
            tracing::debug!(
                edge = ?key,
                "identity-first topology mutation went stale during wiring; replanning"
            );
        }
        Err(IdentityRuntimeError::PeerTopologySuperseded {
            attempts: PEER_TOPOLOGY_MAX_PLAN_ATTEMPTS,
        })
    }

    /// One plan / wire / revalidate pass of
    /// [`Self::mutate_managed_peer_edge_admitted`] against `epoch`.
    async fn mutate_managed_peer_edge_attempt(
        &self,
        bridge: &Arc<dyn SessionBridge>,
        action: crate::topology_control::TopologyAction,
        edge: &ManagedPeerEdge,
        key: &(AgentIdentity, AgentIdentity),
        endpoints: &BTreeSet<AgentIdentity>,
        epoch: &PeerBindingEpoch,
    ) -> Result<PeerTopologyAttempt, IdentityRuntimeError> {
        let resolve = |identity: &AgentIdentity| {
            epoch.active.get(identity).cloned().ok_or_else(|| {
                IdentityRuntimeError::Internal(format!(
                    "topology endpoint is not active: {identity}"
                ))
            })
        };
        let (runtime_a, runtime_b) = (resolve(edge.a())?, resolve(edge.b())?);
        let current = bounded_peer_wiring("current_member_wires", bridge.current_member_wires())
            .await?
            .map_err(|error| {
                IdentityRuntimeError::Internal(format!("bridge current_member_wires: {error}"))
            })?;
        let actual = current.iter().any(|(a, b)| {
            (a == &runtime_a && b == &runtime_b) || (a == &runtime_b && b == &runtime_a)
        });
        let any_half = bounded_peer_wiring(
            "current_member_wires_any_half",
            bridge.current_member_wires_any_half(),
        )
        .await?
        .map_err(|error| {
            IdentityRuntimeError::Internal(format!("bridge current_member_wires_any_half: {error}"))
        })?
        .iter()
        .any(|(a, b)| (a == &runtime_a && b == &runtime_b) || (a == &runtime_b && b == &runtime_a));
        let wire = || async {
            bounded_peer_wiring(
                "wire_peers_batch",
                bridge.wire_peers_batch(&[(runtime_a.clone(), runtime_b.clone())]),
            )
            .await?
            .map_err(|error| {
                IdentityRuntimeError::Internal(format!("bridge wire_peers_batch: {error}"))
            })
        };
        let unwire = || async {
            bounded_peer_wiring("unwire_peer", bridge.unwire_peer(&runtime_a, &runtime_b))
                .await?
                .map_err(|error| {
                    IdentityRuntimeError::Internal(format!("bridge unwire_peer: {error}"))
                })
        };
        match action {
            crate::topology_control::TopologyAction::Connect if !actual => {
                wire().await?;
            }
            crate::topology_control::TopologyAction::Reconnect => {
                if any_half {
                    unwire().await?;
                }
                wire().await?;
            }
            crate::topology_control::TopologyAction::Disconnect if any_half => {
                unwire().await?;
            }
            _ => {}
        }
        let committed = self
            .commit_managed_edges_if_current(epoch, endpoints, |managed| {
                if matches!(action, crate::topology_control::TopologyAction::Disconnect) {
                    managed.remove(key);
                } else {
                    managed.insert(key.clone());
                }
            })
            .await;
        Ok(if committed.is_some() {
            PeerTopologyAttempt::Committed
        } else {
            PeerTopologyAttempt::Stale
        })
    }

    /// What a managed peer-edge reconcile over `desired` (within `scope`)
    /// would change, read from Core and this runtime's managed set without
    /// wiring, unwiring or recording anything. A failed read of the live
    /// edges counts none as wired.
    pub(crate) async fn census_managed_peer_edges(
        &self,
        desired: &BTreeSet<(AgentIdentity, AgentIdentity)>,
        scope: Option<&AgentIdentity>,
    ) -> PeerEdgeCensus {
        let in_scope = |edge: &(AgentIdentity, AgentIdentity)| {
            scope.is_none_or(|target| &edge.0 == target || &edge.1 == target)
        };
        let desired = desired
            .iter()
            .filter(|edge| in_scope(edge))
            .cloned()
            .collect::<BTreeSet<_>>();
        let managed = self
            .managed_peer_edges
            .read()
            .await
            .iter()
            .filter(|edge| in_scope(edge))
            .cloned()
            .collect::<BTreeSet<_>>();
        let live = self.core_live_runtime_ids().await;
        let wired = match self.logical_peer_edges().await {
            Ok(edges) => edges
                .iter()
                .map(|edge| (edge.a().clone(), edge.b().clone()))
                .filter(|edge| in_scope(edge))
                .collect::<BTreeSet<_>>(),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "candidate launch: the live peer edges could not be read for the census"
                );
                BTreeSet::new()
            }
        };
        let mut census = PeerEdgeCensus::default();
        for edge in &desired {
            if wired.contains(edge) {
                census.retained.insert(edge.clone());
            } else if live.contains_key(&edge.0) && live.contains_key(&edge.1) {
                census.would_wire.insert(edge.clone());
            } else {
                census.pending.insert(edge.clone());
            }
        }
        let owned = match self.identity_edge_ownership() {
            IdentityEdgeOwnership::TopologyProvider => {
                let declared = self.definition_declared_identity_edges().await;
                managed
                    .iter()
                    .chain(wired.difference(&declared))
                    .cloned()
                    .collect::<BTreeSet<_>>()
            }
            IdentityEdgeOwnership::WiredByThisProcess => managed,
        };
        census.would_unwire = owned
            .difference(&desired)
            .filter(|edge| wired.contains(*edge))
            .cloned()
            .collect();
        census
    }

    /// Apply identity-first managed topology to the concrete mob graph.
    ///
    /// Topology providers return stable logical identities. The mob comms graph
    /// is keyed by active runtime member IDs, so this resolves each endpoint
    /// through continuity records before calling the same-mob bridge wire APIs.
    ///
    /// A candidate launch rewires no member: it applies nothing, touches no
    /// topology journal, and logs the census of what the reconcile would
    /// change.
    pub async fn reconcile_managed_peer_edges(
        &self,
        desired_edges: &[ManagedPeerEdge],
    ) -> Result<(), IdentityRuntimeError> {
        if self.is_candidate_launch() {
            let desired = desired_edges
                .iter()
                .map(|edge| (edge.a().clone(), edge.b().clone()))
                .collect::<BTreeSet<_>>();
            self.census_managed_peer_edges(&desired, None)
                .await
                .log("managed peer-edge reconcile");
            return Ok(());
        }
        let topology_controller = self.topology_controller();
        let _topology_guard = match topology_controller.as_ref() {
            Some(controller) => Some(controller.mutation_guard().await),
            None => None,
        };
        if let Some(controller) = topology_controller.as_ref() {
            controller
                .prepare_pending_recovery()
                .await
                .map_err(|error| {
                    IdentityRuntimeError::Internal(format!("topology recovery journal: {error}"))
                })?;
        }
        let pending_recovery_edges = if let Some(controller) = topology_controller.as_ref() {
            controller
                .pending_local_recovery_edges()
                .await
                .map_err(|error| {
                    IdentityRuntimeError::Internal(format!("topology recovery ownership: {error}"))
                })?
        } else {
            BTreeSet::new()
        };
        if !pending_recovery_edges.is_empty() {
            self.retain_managed_peer_edges(&pending_recovery_edges)
                .await;
        }
        let composed_edges;
        let desired_edges = if let Some(controller) = topology_controller.as_ref() {
            composed_edges = controller
                .compose_managed_peer_edges(desired_edges)
                .await
                .map_err(|error| {
                    IdentityRuntimeError::Internal(format!("topology overlay: {error}"))
                })?;
            composed_edges.as_slice()
        } else {
            desired_edges
        };
        let result = self
            .reconcile_managed_peer_edges_admitted(desired_edges)
            .await;
        let mut recovery_inspection_error = None;
        if let Some(controller) = topology_controller.as_ref() {
            let recovery_complete = if result.is_ok() && !pending_recovery_edges.is_empty() {
                match self
                    .pending_recovery_is_physically_complete(desired_edges, &pending_recovery_edges)
                    .await
                {
                    Ok(complete) => complete,
                    Err(error) => {
                        recovery_inspection_error = Some(error);
                        false
                    }
                }
            } else {
                false
            };
            controller
                .finalize_recovered_pending(result.is_ok() && recovery_complete)
                .await
                .map_err(|error| {
                    IdentityRuntimeError::Internal(format!("topology recovery receipt: {error}"))
                })?;
        }
        result?;
        if let Some(error) = recovery_inspection_error {
            return Err(error);
        }
        Ok(())
    }

    pub(crate) async fn pending_recovery_is_physically_complete(
        &self,
        desired_edges: &[ManagedPeerEdge],
        pending_recovery_edges: &BTreeSet<(AgentIdentity, AgentIdentity)>,
    ) -> Result<bool, IdentityRuntimeError> {
        if self.bridge.is_none() {
            return Ok(false);
        }
        let active_identities = self
            .core_live_runtime_ids()
            .await
            .into_keys()
            .collect::<BTreeSet<_>>();
        if pending_recovery_edges
            .iter()
            .any(|(a, b)| !active_identities.contains(a) || !active_identities.contains(b))
        {
            return Ok(false);
        }
        let desired = desired_edges
            .iter()
            .map(|edge| (edge.a().clone(), edge.b().clone()))
            .collect::<BTreeSet<_>>();
        let actual = self
            .logical_peer_edges()
            .await?
            .into_iter()
            .map(|edge| (edge.a().clone(), edge.b().clone()))
            .collect::<BTreeSet<_>>();
        let actual_any_half = self
            .logical_peer_edges_any_half()
            .await?
            .into_iter()
            .map(|edge| (edge.a().clone(), edge.b().clone()))
            .collect::<BTreeSet<_>>();

        Ok(desired.is_subset(&actual)
            && pending_recovery_edges
                .difference(&desired)
                .all(|edge| !actual_any_half.contains(edge)))
    }

    /// Reconcile an already-composed desired topology while the caller holds
    /// the shared topology-controller admission guard.
    ///
    /// Plan / wire / revalidate. The plan is computed from one snapshot of the
    /// endpoints' runtime bindings (its epoch) with no lifecycle lock held;
    /// the bridge wiring calls (mob-actor round trips, each bounded by
    /// [`PEER_WIRING_CALL_BUDGET`]) run with no lifecycle lock held either.
    /// The commit takes no lifecycle lock at all: it revalidates the involved
    /// endpoints' bindings and writes the managed-edge bookkeeping while
    /// holding the entries read lock (see
    /// [`Self::commit_managed_edges_if_current`]), so no admission path ever
    /// waits behind a topology commit. A binding that changed meanwhile
    /// (reset, respawn, retire) makes the plan Stale: nothing is committed
    /// and the reconcile replans from the live bindings, at most
    /// [`PEER_TOPOLOGY_MAX_PLAN_ATTEMPTS`] times.
    pub(crate) async fn reconcile_managed_peer_edges_admitted(
        &self,
        desired_edges: &[ManagedPeerEdge],
    ) -> Result<(), IdentityRuntimeError> {
        let _guard = self.managed_peer_reconcile_lock.lock().await;
        let Some(bridge) = self.bridge.clone() else {
            return Ok(());
        };
        let desired: BTreeSet<(AgentIdentity, AgentIdentity)> = desired_edges
            .iter()
            .map(|edge| (edge.a().clone(), edge.b().clone()))
            .collect();
        self.reconcile_managed_peer_edges_planned(&bridge, &desired, None, None)
            .await
    }

    /// The replan loop shared by the full reconcile and the wake path's
    /// target-scoped reconcile. The caller holds the managed-peer reconcile
    /// lock (and the topology admission guard when a controller exists).
    async fn reconcile_managed_peer_edges_planned(
        &self,
        bridge: &Arc<dyn SessionBridge>,
        desired: &BTreeSet<(AgentIdentity, AgentIdentity)>,
        scope: Option<&AgentIdentity>,
        deadline: Option<tokio::time::Instant>,
    ) -> Result<(), IdentityRuntimeError> {
        // A candidate launch rewires no member, here either (the topology
        // controller's reconcile and the wake path's scoped reconcile).
        if self.is_candidate_launch() {
            self.census_managed_peer_edges(desired, scope)
                .await
                .log("managed peer-edge reconcile");
            return Ok(());
        }
        for _attempt in 0..PEER_TOPOLOGY_MAX_PLAN_ATTEMPTS {
            match self
                .reconcile_managed_peer_edges_attempt(bridge, desired, scope, deadline)
                .await?
            {
                PeerTopologyAttempt::Committed => return Ok(()),
                PeerTopologyAttempt::Stale => {
                    tracing::debug!(
                        "identity-first topology plan went stale during wiring; replanning"
                    );
                }
            }
        }
        Err(IdentityRuntimeError::PeerTopologySuperseded {
            attempts: PEER_TOPOLOGY_MAX_PLAN_ATTEMPTS,
        })
    }

    /// One plan / wire / revalidate pass of
    /// [`Self::reconcile_managed_peer_edges_planned`]. With a `scope`, only
    /// edges touching that identity are planned, wired or cleaned up (the
    /// wake path's target-only reconcile); every other edge is left alone.
    async fn reconcile_managed_peer_edges_attempt(
        &self,
        bridge: &Arc<dyn SessionBridge>,
        desired: &BTreeSet<(AgentIdentity, AgentIdentity)>,
        scope: Option<&AgentIdentity>,
        deadline: Option<tokio::time::Instant>,
    ) -> Result<PeerTopologyAttempt, IdentityRuntimeError> {
        let in_scope = |edge: &(AgentIdentity, AgentIdentity)| {
            scope.is_none_or(|target| &edge.0 == target || &edge.1 == target)
        };
        // Edges whose earlier wiring call timed out. Their command may still
        // be queued in the mob actor, and inspection reads roster and machine
        // state directly (not through the actor queue), so what inspection
        // says about them is not their fate. Each is settled only by an
        // ORDERED command that succeeded after it: a wire if it is still
        // desired, a tolerant unwire (not-wired is Ok) if not. Until then it
        // stays unsettled and inspection is never trusted for it.
        let unsettled: BTreeSet<(AgentIdentity, AgentIdentity)> = self
            .unsettled_peer_wiring
            .read()
            .await
            .iter()
            .filter(|edge| in_scope(edge))
            .cloned()
            .collect();
        let managed_snapshot: BTreeSet<(AgentIdentity, AgentIdentity)> = self
            .managed_peer_edges
            .read()
            .await
            .iter()
            .filter(|edge| in_scope(edge))
            .cloned()
            .collect();
        let desired: BTreeSet<(AgentIdentity, AgentIdentity)> = desired
            .iter()
            .filter(|edge| in_scope(edge))
            .cloned()
            .collect();
        let epoch = self.peer_binding_epoch().await;
        let runtime_identities: BTreeMap<AgentRuntimeId, AgentIdentity> = epoch
            .active
            .iter()
            .map(|(identity, runtime_id)| (runtime_id.clone(), identity.clone()))
            .collect();
        let logical = |runtime_edges: &[(AgentRuntimeId, AgentRuntimeId)]| {
            runtime_edges
                .iter()
                .filter_map(|(runtime_a, runtime_b)| {
                    let a = runtime_identities.get(runtime_a)?;
                    let b = runtime_identities.get(runtime_b)?;
                    if a <= b {
                        Some((a.clone(), b.clone()))
                    } else {
                        Some((b.clone(), a.clone()))
                    }
                })
                .collect::<BTreeSet<_>>()
        };
        let current_logical_edges: Option<BTreeSet<(AgentIdentity, AgentIdentity)>> =
            match bounded_peer_wiring_by(
                "current_member_wires",
                deadline,
                bridge.current_member_wires(),
            )
            .await?
            {
                Ok(current_runtime_edges) => Some(logical(&current_runtime_edges)),
                Err(err) => {
                    tracing::debug!(
                        error = %err,
                        "identity-first topology reconcile could not inspect current member wires"
                    );
                    None
                }
            };
        let current_any_half_edges: Option<BTreeSet<(AgentIdentity, AgentIdentity)>> =
            match bounded_peer_wiring_by(
                "current_member_wires_any_half",
                deadline,
                bridge.current_member_wires_any_half(),
            )
            .await?
            {
                Ok(current_runtime_edges) => Some(logical(&current_runtime_edges)),
                Err(error) => {
                    tracing::debug!(
                        %error,
                        "identity-first topology reconcile could not inspect orphan wire halves"
                    );
                    None
                }
            };
        let active_runtimes = &epoch.active;

        let edge_is_managed_and_live = |edge: &(AgentIdentity, AgentIdentity)| {
            // Managed-but-missing live edges are retried deliberately so tolerant topology restores self-heal.
            managed_snapshot.contains(edge)
                && current_logical_edges
                    .as_ref()
                    .is_none_or(|edges| edges.contains(edge))
        };
        let retained_logical_edges: Vec<(AgentIdentity, AgentIdentity)> = desired
            .iter()
            .filter(|edge| !unsettled.contains(*edge))
            .filter(|edge| !edge_is_managed_and_live(edge))
            .filter(|edge| {
                current_logical_edges
                    .as_ref()
                    .is_some_and(|edges| edges.contains(*edge))
            })
            .filter(|(a, b)| active_runtimes.contains_key(a) && active_runtimes.contains_key(b))
            .cloned()
            .collect();
        let to_wire: Vec<(AgentIdentity, AgentIdentity, AgentRuntimeId, AgentRuntimeId)> = desired
            .iter()
            .filter(|edge| {
                // A still-desired unsettled edge gets an ordered (idempotent)
                // wire whatever inspection reports.
                unsettled.contains(*edge)
                    || (!edge_is_managed_and_live(edge)
                        && current_logical_edges
                            .as_ref()
                            .is_none_or(|edges| !edges.contains(*edge)))
            })
            .filter_map(|(a, b)| {
                let runtime_a = active_runtimes.get(a)?;
                let runtime_b = active_runtimes.get(b)?;
                Some((a.clone(), b.clone(), runtime_a.clone(), runtime_b.clone()))
            })
            .collect();

        // Stale: a managed edge that is no longer desired and, when a
        // topology provider owns the topology, every live edge between two of
        // this runtime's identities that no declared owner wants (neither the
        // desired set nor the mob definition's wiring). The managed
        // set is this process's memory and starts empty, while the mob's
        // wiring is durable and replays on recovery, so an edge wired before
        // a restart is live but unmanaged (see `IdentityEdgeOwnership`). An
        // edge to a member that is not one of this runtime's identities never
        // appears here.
        // A live edge the mob definition's wiring declares has its own
        // owner; it is never the topology's to prune.
        let live_owned_edges: BTreeSet<(AgentIdentity, AgentIdentity)> = match (
            self.identity_edge_ownership(),
            current_logical_edges.as_ref(),
        ) {
            (IdentityEdgeOwnership::TopologyProvider, Some(live)) => {
                let definition_declared = self.definition_declared_identity_edges().await;
                live.iter()
                    .filter(|edge| in_scope(edge))
                    .filter(|edge| !definition_declared.contains(*edge))
                    .cloned()
                    .collect()
            }
            (IdentityEdgeOwnership::TopologyProvider, None)
            | (IdentityEdgeOwnership::WiredByThisProcess, _) => BTreeSet::new(),
        };
        let stale: Vec<(AgentIdentity, AgentIdentity)> = managed_snapshot
            .iter()
            .chain(live_owned_edges.iter())
            .filter(|edge| !desired.contains(*edge))
            .filter(|edge| !unsettled.contains(*edge))
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        // No longer desired and unsettled: an ordered, tolerant unwire,
        // issued whatever inspection says (it queues behind a pending wire).
        // One whose endpoint is no longer Active has no member left for a
        // late wire to land on (retire removes the roster entry and its
        // wiring), so it settles without a command.
        let (forced_unwire, unsettled_without_members): (Vec<_>, Vec<_>) = unsettled
            .iter()
            .filter(|edge| !desired.contains(*edge))
            .map(
                |(a, b)| match (active_runtimes.get(a), active_runtimes.get(b)) {
                    (Some(runtime_a), Some(runtime_b)) => {
                        Ok((a.clone(), b.clone(), runtime_a.clone(), runtime_b.clone()))
                    }
                    _ => Err((a.clone(), b.clone())),
                },
            )
            .partition(Result::is_ok);
        let forced_unwire: Vec<(AgentIdentity, AgentIdentity, AgentRuntimeId, AgentRuntimeId)> =
            forced_unwire.into_iter().filter_map(Result::ok).collect();
        let unsettled_without_members: Vec<(AgentIdentity, AgentIdentity)> =
            unsettled_without_members
                .into_iter()
                .filter_map(Result::err)
                .collect();
        let to_unwire: Vec<(AgentIdentity, AgentIdentity, AgentRuntimeId, AgentRuntimeId)> = stale
            .iter()
            .filter_map(|(a, b)| {
                let runtime_a = active_runtimes.get(a)?;
                let runtime_b = active_runtimes.get(b)?;
                if current_any_half_edges
                    .as_ref()
                    .is_some_and(|edges| !edges.contains(&(a.clone(), b.clone())))
                {
                    return None;
                }
                Some((a.clone(), b.clone(), runtime_a.clone(), runtime_b.clone()))
            })
            .collect();

        // Every endpoint whose binding this plan relied on.
        let involved: BTreeSet<AgentIdentity> = retained_logical_edges
            .iter()
            .chain(stale.iter())
            .flat_map(|(a, b)| [a.clone(), b.clone()])
            .chain(
                to_wire
                    .iter()
                    .chain(forced_unwire.iter())
                    .flat_map(|(a, b, _, _)| [a.clone(), b.clone()]),
            )
            .collect();

        let wire_logical_edges = to_wire
            .iter()
            .map(|(a, b, _, _)| (a.clone(), b.clone()))
            .collect::<Vec<_>>();
        let wire_runtime_edges = to_wire
            .iter()
            .map(|(_, _, runtime_a, runtime_b)| (runtime_a.clone(), runtime_b.clone()))
            .collect::<Vec<_>>();
        if !wire_runtime_edges.is_empty() {
            match bounded_peer_wiring_by(
                "wire_peers_batch",
                deadline,
                bridge.wire_peers_batch(&wire_runtime_edges),
            )
            .await
            {
                Ok(Ok(())) => {
                    // An ordered wire succeeded after any pending command for
                    // these edges: their fate is known again.
                    self.settle_unsettled_peer_wiring(wire_logical_edges.iter().cloned())
                        .await;
                }
                Ok(Err(error)) => {
                    // A refusal from an endpoint that was retired or respawned
                    // under the plan is the plan going stale, not a failure.
                    if self.peer_bindings_moved(&epoch, &involved).await {
                        return Ok(PeerTopologyAttempt::Stale);
                    }
                    return Err(IdentityRuntimeError::Internal(format!(
                        "bridge wire_peers_batch: {error}"
                    )));
                }
                Err(timed_out) => {
                    // The batch may still land in the actor after its reply
                    // was dropped: its edges' fate is unknown.
                    self.record_unsettled_peer_wiring(wire_logical_edges.iter().cloned())
                        .await;
                    return Err(timed_out);
                }
            }
        }

        let unwire_results = stream::iter(to_unwire.into_iter().chain(forced_unwire).map(
            |(a, b, runtime_a, runtime_b)| {
                let bridge = bridge.clone();
                async move {
                    // The bridge's unwire is tolerant: an edge that is not
                    // wired (or a peer that is gone) is Ok.
                    let result = bounded_peer_wiring_by(
                        "unwire_peer",
                        deadline,
                        bridge.unwire_peer(&runtime_a, &runtime_b),
                    )
                    .await
                    .map(|outcome| outcome.map_err(|e| format!("{e}")));
                    (a, b, result)
                }
            },
        ))
        .buffer_unordered(MANAGED_PEER_RECONCILE_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        let timed_out_unwires: Vec<(AgentIdentity, AgentIdentity)> = unwire_results
            .iter()
            .filter(|(_, _, result)| result.is_err())
            .map(|(a, b, _)| (a.clone(), b.clone()))
            .collect();
        if !timed_out_unwires.is_empty() {
            self.record_unsettled_peer_wiring(timed_out_unwires).await;
        }
        // Ordered unwires that succeeded, and unsettled edges with no member
        // left, are settled; a refused or timed-out one stays unsettled.
        self.settle_unsettled_peer_wiring(
            unwire_results
                .iter()
                .filter(|(_, _, result)| matches!(result, Ok(Ok(()))))
                .map(|(a, b, _)| (a.clone(), b.clone()))
                .filter(|edge| unsettled.contains(edge))
                .chain(unsettled_without_members.iter().cloned()),
        )
        .await;

        // Commit only if none of the involved bindings moved since the plan
        // was made; no lifecycle lock is taken.
        let committed = self
            .commit_managed_edges_if_current(&epoch, &involved, |managed| {
                for edge in retained_logical_edges {
                    managed.insert(edge);
                }
                for edge in wire_logical_edges {
                    managed.insert(edge);
                }
                for (a, b) in stale {
                    let key = (a.clone(), b.clone());
                    if current_any_half_edges
                        .as_ref()
                        .is_some_and(|edges| !edges.contains(&key))
                    {
                        managed.remove(&key);
                    }
                }
                for edge in unsettled_without_members {
                    managed.remove(&edge);
                }
                for (a, b, result) in unwire_results {
                    result?.map_err(|e| {
                        IdentityRuntimeError::Internal(format!("bridge unwire_peer: {e}"))
                    })?;
                    managed.remove(&(a, b));
                }
                Ok::<(), IdentityRuntimeError>(())
            })
            .await;
        match committed {
            Some(result) => result.map(|()| PeerTopologyAttempt::Committed),
            None => Ok(PeerTopologyAttempt::Stale),
        }
    }

    /// Commit managed-edge bookkeeping if none of `involved` changed binding
    /// since `epoch`: Core's live members are read again, and the commit
    /// refuses when a verb of this runtime signalled a member move while
    /// that read ran, so no move of ours interleaves with the commit. A
    /// member Core moves on its own is found by the next reconcile, which
    /// observes the live graph. No admission waits on a topology commit.
    /// Returns `None` (nothing committed) when the plan is stale.
    async fn commit_managed_edges_if_current<R>(
        &self,
        epoch: &PeerBindingEpoch,
        involved: &BTreeSet<AgentIdentity>,
        commit: impl FnOnce(&mut BTreeSet<(AgentIdentity, AgentIdentity)>) -> R,
    ) -> Option<R> {
        let sequence = self.entries.binding_sequence();
        if !epoch.unchanged_for(&self.peer_binding_epoch().await, involved) {
            return None;
        }
        let mut managed = self.managed_peer_edges.write().await;
        if self.entries.binding_sequence() != sequence {
            return None;
        }
        Some(commit(&mut managed))
    }

    /// Whether any of `involved` changed binding since `epoch`.
    async fn peer_bindings_moved(
        &self,
        epoch: &PeerBindingEpoch,
        involved: &BTreeSet<AgentIdentity>,
    ) -> bool {
        !epoch.unchanged_for(&self.peer_binding_epoch().await, involved)
    }

    /// Record edges whose wiring call timed out (see
    /// [`IdentityRuntime::unsettled_peer_wiring`]).
    async fn record_unsettled_peer_wiring(
        &self,
        edges: impl IntoIterator<Item = (AgentIdentity, AgentIdentity)>,
    ) {
        self.unsettled_peer_wiring.write().await.extend(edges);
    }

    /// Settle unsettled edges: an ordered command for each succeeded after
    /// its timed-out one (or no member is left for it to land on).
    async fn settle_unsettled_peer_wiring(
        &self,
        edges: impl IntoIterator<Item = (AgentIdentity, AgentIdentity)>,
    ) {
        let mut unsettled = self.unsettled_peer_wiring.write().await;
        for edge in edges {
            unsettled.remove(&edge);
        }
    }

    /// Edges whose last wiring call timed out and that no ordered command has
    /// settled yet.
    #[cfg(test)]
    pub(crate) async fn unsettled_peer_wiring_snapshot(
        &self,
    ) -> BTreeSet<(AgentIdentity, AgentIdentity)> {
        self.unsettled_peer_wiring.read().await.clone()
    }

    /// Every registered identity's live Core member: the epoch a
    /// managed-topology plan is made against and revalidated with.
    async fn peer_binding_epoch(&self) -> PeerBindingEpoch {
        PeerBindingEpoch {
            active: self.core_live_runtime_ids().await,
        }
    }

    /// Core's live runtime id of each registered identity (read per
    /// call, never cached). An identity whose live member cannot be read is
    /// left out, as if it were not live: a plan made on it replans, and the
    /// next reconcile converges on the live graph.
    async fn core_live_runtime_ids(&self) -> BTreeMap<AgentIdentity, AgentRuntimeId> {
        let identities: Vec<AgentIdentity> = self.entries.read().await.keys().cloned().collect();
        let mut live = BTreeMap::new();
        for identity in identities {
            match self.core_live_member(&identity).await {
                Ok(Some(binding)) => {
                    live.insert(identity, binding.agent_runtime_id);
                }
                Ok(None) => {}
                Err(error) => tracing::warn!(
                    %identity,
                    %error,
                    "managed topology: Core's live member could not be read; the identity is \
                     treated as not live for this pass"
                ),
            }
        }
        live
    }

    /// Emit an event for the given identity. Best-effort — no error if no subscribers.
    async fn emit_event(&self, identity: &AgentIdentity, event: IdentityEvent) {
        let channels = self.event_channels.read().await;
        if let Some(tx) = channels.get(identity) {
            let _ = tx.send(event);
        }
    }

    fn emit_error(&self, event: crate::unified_runtime::types::ErrorEvent) {
        let hook = match self.error_hook.read() {
            Ok(stored_hook) => stored_hook.clone(),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "identity runtime error hook lock poisoned; dropping error event"
                );
                None
            }
        };
        // The identity runtime owns a hook slot of its own, so it is a second
        // place an ErrorEvent can vanish when no host wired one. Same default
        // sink as the unified-runtime fire point.
        crate::unified_runtime::log_error_event(&event, hook.is_some());
        if let Some(hook) = hook {
            tokio::spawn(async move {
                let () = hook(event).await;
            });
        }
    }

    async fn materialization_backoff_error(&self, identity: &AgentIdentity) -> Option<String> {
        let backoffs = self.materialization_failure_backoff.read().await;
        let backoff = backoffs.get(identity)?;
        if Instant::now() < backoff.suppress_until {
            Some(backoff.error.clone())
        } else {
            None
        }
    }

    async fn clear_materialization_backoff(&self, identity: &AgentIdentity) {
        self.materialization_failure_backoff
            .write()
            .await
            .remove(identity);
    }

    async fn record_best_effort_materialization_failure(
        &self,
        identity: &AgentIdentity,
        initiator: Option<&AgentIdentity>,
        operation: &'static str,
        err: &IdentityRuntimeError,
    ) {
        let error = err.to_string();
        let suppress_until = Instant::now() + MATERIALIZATION_FAILURE_BACKOFF;
        self.materialization_failure_backoff.write().await.insert(
            identity.clone(),
            MaterializationFailureBackoff {
                suppress_until,
                error: error.clone(),
            },
        );
        self.emit_error(
            crate::unified_runtime::types::ErrorEvent::IdentityMaterializationFailure {
                identity: identity.to_string(),
                initiator: initiator.map(ToString::to_string),
                operation: operation.to_string(),
                error,
            },
        );
    }

    // -----------------------------------------------------------------------
    // Registration / activation
    // -----------------------------------------------------------------------

    /// Register (or re-register) an identity's roster spec, under its
    /// lifecycle lock: replacing an entry replaces its registered row, which
    /// a governed or row-checked send holding that lock delivers under. Meta
    /// keeps no Core fact here. A recorded wake refusal survives a
    /// re-registration, because it describes Core's session rather than the
    /// spec, until the next successful wake.
    pub async fn register(&self, spec: DurableAgentSpec) {
        let lifecycle_lock = self.lifecycle_lock_for(&spec.identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        self.register_locked(spec).await;
    }

    /// [`Self::register`]'s body for callers that already hold the
    /// identity's lifecycle lock (roster registration).
    pub(crate) async fn register_locked(&self, spec: DurableAgentSpec) {
        let identity = spec.identity.clone();
        let bootstrap_generation = self.identity_bootstrap_status_with_generation().0;
        {
            let mut entries = self.entries.write().await;
            let (wake_refusal, replaced_session, absent_delivery_session) = entries
                .get(&identity)
                .map(|existing| {
                    (
                        existing.wake_refusal.clone(),
                        existing.replaced_session.clone(),
                        existing.absent_delivery_session.clone(),
                    )
                })
                .unwrap_or_default();
            entries.insert(
                identity.clone(),
                IdentityEntry {
                    spec,
                    bootstrap_generation,
                    wake_refusal,
                    replaced_session,
                    last_delivery_error: None,
                    last_reload: None,
                    absent_delivery_session,
                },
            );
        }
        // The event channel outlives a re-registration: its subscribers
        // belong to the identity's life, not to the roster row.
        self.event_channels
            .write()
            .await
            .entry(identity)
            .or_insert_with(|| broadcast::channel(IDENTITY_EVENT_CHANNEL_CAPACITY).0);
    }

    async fn materialization_lock_for(&self, identity: &AgentIdentity) -> Arc<Mutex<()>> {
        if let Some(lock) = self.materialization_locks.read().await.get(identity) {
            return lock.clone();
        }
        let mut locks = self.materialization_locks.write().await;
        locks
            .entry(identity.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    async fn best_effort_materialization_lock_for(
        &self,
        identity: &AgentIdentity,
    ) -> Arc<Mutex<()>> {
        if let Some(lock) = self
            .best_effort_materialization_locks
            .read()
            .await
            .get(identity)
        {
            return lock.clone();
        }
        let mut locks = self.best_effort_materialization_locks.write().await;
        locks
            .entry(identity.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Whether a lifecycle verb holds `identity`'s lifecycle lock right now.
    async fn lifecycle_verb_in_flight(&self, identity: &AgentIdentity) -> bool {
        self.lifecycle_lock_for(identity).await.try_lock().is_err()
    }

    pub(crate) async fn lifecycle_lock_for(&self, identity: &AgentIdentity) -> Arc<Mutex<()>> {
        if let Some(lock) = self.lifecycle_locks.read().await.get(identity) {
            return lock.clone();
        }
        let mut locks = self.lifecycle_locks.write().await;
        locks
            .entry(identity.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub(crate) async fn raw_member_alias_lock(&self, alias: &str) -> Arc<Mutex<()>> {
        let alias = crate::member_comms_id::runtime_alias_str(alias.trim()).into_owned();
        if let Some(lock) = self
            .raw_member_alias_locks
            .read()
            .await
            .entries
            .get(&alias)
            .and_then(Weak::upgrade)
        {
            return lock;
        }
        let mut table = self.raw_member_alias_locks.write().await;
        table.sweep_if_needed();
        // Another caller may have installed the key after our read miss. The
        // write-side upgrade is the authority boundary that prevents two live
        // mutexes from serializing the same alias independently.
        if let Some(lock) = table.entries.get(&alias).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        table.entries.insert(alias, Arc::downgrade(&lock));
        lock
    }

    #[cfg(test)]
    pub(crate) async fn raw_member_alias_lock_metrics(&self) -> (usize, usize, usize) {
        let table = self.raw_member_alias_locks.read().await;
        (table.entries.len(), table.next_sweep_len, table.sweep_count)
    }

    pub(crate) async fn ensure_raw_member_alias_available(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), IdentityRuntimeError> {
        if let Some(bridge) = self.bridge.as_ref()
            && bridge
                .raw_member_alias_exists(identity.as_str())
                .await
                .map_err(|error| {
                    IdentityRuntimeError::Internal(format!(
                        "raw member namespace inspection for {identity}: {error}"
                    ))
                })?
        {
            // The member's session, for the refusal: a read fault leaves it
            // unnamed, and the refusal stands either way.
            let session_id = self
                .core_live_member(identity)
                .await
                .ok()
                .flatten()
                .and_then(|live| live.session_id);
            return Err(IdentityRuntimeError::IdentityNameHeldByRawMember {
                identity: identity.clone(),
                member: crate::member_comms_id::mob_member_id_str(identity.as_str()).into_owned(),
                session_id,
            });
        }
        Ok(())
    }

    /// Resolve an alias into a lifecycle-lock target. Classic members return
    /// `None`; generated aliases always resolve to an authority target even
    /// after deletion so validation under the lock fails closed.
    pub(crate) async fn member_alias_lifecycle_target(
        self: &Arc<Self>,
        alias: &str,
    ) -> Result<Option<MemberAliasLifecycleTarget>, IdentityRuntimeError> {
        let alias = crate::member_comms_id::runtime_alias_str(alias).into_owned();
        match self.identity_for_member_mutation(&alias).await {
            Some(identity) => {
                // Fail stale/deleted generated aliases before unrelated
                // operation prerequisites (for example cross-mob directory
                // lookup). The tracked operation validates again after taking
                // the lifecycle lock, so this preflight does not become the
                // authority boundary or introduce a TOCTOU gap.
                self.ensure_expected_member_alias_current(&identity, &alias)
                    .await?;
                Ok(Some(MemberAliasLifecycleTarget {
                    runtime: Arc::clone(self),
                    lock: self.lifecycle_lock_for(&identity).await,
                    identity,
                    alias,
                }))
            }
            None if crate::member_comms_id::is_reserved_generated_alias(&alias) => {
                Err(IdentityRuntimeError::Internal(format!(
                    "generated member alias requires identity authority: {alias}"
                )))
            }
            None => Ok(None),
        }
    }

    /// Acquire member-alias lifecycle targets across one or more identity
    /// runtimes in a deterministic process-global order, then validate every
    /// alias while its owning lock is held. Duplicate identities acquire one
    /// lock but still validate every spelling/generation.
    pub(crate) async fn acquire_member_alias_lifecycle_targets(
        mut targets: Vec<MemberAliasLifecycleTarget>,
    ) -> Result<Vec<tokio::sync::OwnedMutexGuard<()>>, IdentityRuntimeError> {
        targets.sort_by(|a, b| {
            a.runtime
                .runtime_instance_id
                .cmp(&b.runtime.runtime_instance_id)
                .then_with(|| {
                    (Arc::as_ptr(&a.runtime) as usize).cmp(&(Arc::as_ptr(&b.runtime) as usize))
                })
                .then_with(|| a.identity.cmp(&b.identity))
                .then_with(|| a.alias.cmp(&b.alias))
        });

        let mut guards = Vec::with_capacity(targets.len());
        let mut held: Option<(usize, AgentIdentity)> = None;
        for target in targets {
            let key = (
                Arc::as_ptr(&target.runtime) as usize,
                target.identity.clone(),
            );
            if held.as_ref() != Some(&key) {
                guards.push(target.lock.lock_owned().await);
                held = Some(key);
            }
            target
                .runtime
                .ensure_expected_member_alias_current(&target.identity, &target.alias)
                .await?;
            let wake_refusal = target
                .runtime
                .entries
                .read()
                .await
                .get(&target.identity)
                .map(|entry| entry.wake_refusal.clone())
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(target.identity.clone()))?;
            // A member alias names a live Core member.
            if target
                .runtime
                .core_live_member(&target.identity)
                .await?
                .is_none()
            {
                return Err(IdentityRuntimeError::InvalidState {
                    identity: target.identity,
                    state: Self::view_state(false, wake_refusal.as_ref()),
                    operation: "mutate member alias",
                });
            }
        }
        Ok(guards)
    }

    /// Cancellation-safe operation spanning one or more alias targets. The
    /// first target in global order supervises the transaction to its explicit
    /// commit/rollback boundary if the request future is dropped.
    pub(crate) async fn run_member_alias_targets_operation_tracked<T, F, Fut>(
        targets: Vec<MemberAliasLifecycleTarget>,
        operation: F,
    ) -> Result<T, IdentityRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
    {
        let mut runtimes = targets
            .iter()
            .map(|target| Arc::clone(&target.runtime))
            .collect::<Vec<_>>();
        runtimes.sort_by(|a, b| {
            a.runtime_instance_id
                .cmp(&b.runtime_instance_id)
                .then_with(|| (Arc::as_ptr(a) as usize).cmp(&(Arc::as_ptr(b) as usize)))
        });
        runtimes.dedup_by(|a, b| Arc::ptr_eq(a, b));
        let transaction = async move {
            let _guards = Self::acquire_member_alias_lifecycle_targets(targets).await?;
            operation().await.map_err(IdentityRuntimeError::Internal)
        };
        if !runtimes.is_empty() {
            return Self::run_tracked_foreground_multi(runtimes, transaction).await;
        }
        transaction.await
    }

    /// Register one compound transaction with every participating runtime.
    /// Non-owner shutdowns wait on a completion task in their own JoinSet,
    /// while the globally first runtime owns the actual operation and result.
    /// The public owner for one tracked foreground operation (D67): acquired
    /// for an operation started through an owner-bearing view, or carried
    /// from the work that spawned it.
    fn foreground_owner(
        &self,
        source: ForegroundOwnerSource,
    ) -> Result<Option<meerkat_mob::MobHandle>, IdentityRuntimeError> {
        match source {
            ForegroundOwnerSource::Acquire => self.acquire_foreground_owner(),
            ForegroundOwnerSource::Carried(owner) => Ok(owner),
        }
    }

    /// A public owner of this runtime's mob for one tracked foreground
    /// operation (D67). The runtime's services hold the handle its
    /// composition supplied; the builder and both gateways supply a
    /// non-owning one, so the runtime does not keep the mob owned itself.
    /// The native lifecycle owner grants an owner only while the mob's
    /// admission is open, decided against the same close, so the operation
    /// is either owned before its first native call or refused.
    fn acquire_foreground_owner(
        &self,
    ) -> Result<Option<meerkat_mob::MobHandle>, IdentityRuntimeError> {
        match self.runtime_services.mob_handle() {
            Some(handle) => handle.acquire_owner().map(Some).map_err(|error| {
                IdentityRuntimeError::Internal(format!(
                    "the mob no longer admits identity operations: {error}"
                ))
            }),
            None => Ok(None),
        }
    }

    async fn run_tracked_foreground_multi<T, F>(
        runtimes: Vec<Arc<Self>>,
        operation: F,
    ) -> Result<T, IdentityRuntimeError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, IdentityRuntimeError>> + Send + 'static,
    {
        let Some((supervisor, participants)) = runtimes.split_first() else {
            return operation.await;
        };
        if runtimes
            .iter()
            .any(|runtime| runtime.foreground_shutdown.load(Ordering::Acquire))
        {
            return Err(IdentityRuntimeError::Internal(
                "identity runtime is shutting down".to_string(),
            ));
        }

        // D67: see `run_tracked_foreground`; every participating mob is owned
        // for the operation's whole life.
        let owners = runtimes
            .iter()
            .map(|runtime| runtime.acquire_foreground_owner())
            .collect::<Result<Vec<_>, _>>()?;
        let (completion, _) = watch::channel(false);
        for runtime in participants {
            let mut operations = runtime.foreground_operations.lock().await;
            if runtime.foreground_shutdown.load(Ordering::Acquire) {
                completion.send_replace(true);
                return Err(IdentityRuntimeError::Internal(
                    "identity runtime is shutting down".to_string(),
                ));
            }
            while let Some(result) = operations.try_join_next() {
                if let Err(error) = result {
                    tracing::error!(
                        error = %error,
                        "tracked foreground identity operation panicked"
                    );
                }
            }
            let mut completed = completion.subscribe();
            operations.spawn(async move {
                while !*completed.borrow() && completed.changed().await.is_ok() {}
            });
        }

        let (sender, receiver) = oneshot::channel();
        {
            let mut operations = supervisor.foreground_operations.lock().await;
            if supervisor.foreground_shutdown.load(Ordering::Acquire) {
                completion.send_replace(true);
                return Err(IdentityRuntimeError::Internal(
                    "identity runtime is shutting down".to_string(),
                ));
            }
            while let Some(result) = operations.try_join_next() {
                if let Err(error) = result {
                    tracing::error!(
                        error = %error,
                        "tracked foreground identity operation panicked"
                    );
                }
            }
            operations.spawn(async move {
                let _completion = MultiRuntimeForegroundCompletion(completion);
                let carried = owners.first().cloned().flatten();
                let result = FOREGROUND_OWNER.scope(carried, operation).await;
                drop(owners);
                let _ = sender.send(result);
            });
        }
        receiver.await.map_err(|_| {
            IdentityRuntimeError::Internal(
                "tracked foreground identity operation terminated without a result".to_string(),
            )
        })?
    }

    /// Run an externally-cancellable operation under runtime ownership.
    ///
    /// Dropping the caller only drops the result receiver; the transaction
    /// remains in the runtime's join set and reaches its explicit
    /// commit/rollback boundary. Graceful shutdown closes admission and joins
    /// every such task before lease renewal or the mob actor is stopped.
    pub(crate) async fn run_tracked_foreground<T, F>(
        self: &Arc<Self>,
        operation: F,
    ) -> Result<T, IdentityRuntimeError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, IdentityRuntimeError>> + Send + 'static,
    {
        self.run_tracked_foreground_with(ForegroundOwnerSource::Acquire, operation)
            .await
    }

    /// [`Self::run_tracked_foreground`] with an explicit owner source: work
    /// spawned on behalf of an owned operation carries that operation's owner
    /// rather than acquiring one (internal code never revives a mob).
    pub(crate) async fn run_tracked_foreground_with<T, F>(
        self: &Arc<Self>,
        source: ForegroundOwnerSource,
        operation: F,
    ) -> Result<T, IdentityRuntimeError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, IdentityRuntimeError>> + Send + 'static,
    {
        let receiver = self
            .start_tracked_foreground_with(source, operation)
            .await?;
        receiver.await.map_err(|_| {
            IdentityRuntimeError::Internal(
                "tracked foreground identity operation terminated without a result".to_string(),
            )
        })?
    }

    /// Admit `operation` into the runtime's tracked foreground owner, as
    /// [`Self::run_tracked_foreground_with`] does, and return the receiver of
    /// its result instead of awaiting it: the owner keeps the operation
    /// whether or not the receiver is awaited or dropped.
    pub(crate) async fn start_tracked_foreground_with<T, F>(
        self: &Arc<Self>,
        source: ForegroundOwnerSource,
        operation: F,
    ) -> Result<oneshot::Receiver<Result<T, IdentityRuntimeError>>, IdentityRuntimeError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, IdentityRuntimeError>> + Send + 'static,
    {
        self.start_tracked_foreground_within(source, operation, std::future::pending())
            .await
    }

    /// [`Self::start_tracked_foreground_with`] with its wait for admission
    /// bounded. The wait for the owner's lock ends unadmitted, refused, as
    /// soon as this runtime's foreground operations close (a shutdown's join
    /// holds that lock while it waits for every earlier operation, which is
    /// not bounded), and with `bound`'s error once `bound` resolves first.
    /// Nothing of `operation` has run before it is admitted; once admitted,
    /// under the lock, it is never dropped.
    pub(crate) async fn start_tracked_foreground_within<T, F, B>(
        self: &Arc<Self>,
        source: ForegroundOwnerSource,
        operation: F,
        bound: B,
    ) -> Result<oneshot::Receiver<Result<T, IdentityRuntimeError>>, IdentityRuntimeError>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, IdentityRuntimeError>> + Send + 'static,
        B: Future<Output = IdentityRuntimeError>,
    {
        let shutting_down =
            || IdentityRuntimeError::Internal("identity runtime is shutting down".to_string());
        self.remember_self_handle();
        if self.foreground_shutdown.load(Ordering::Acquire) {
            return Err(shutting_down());
        }
        // D67: the operation owns the mob publicly from before its first
        // await until it settles, so an idle close cannot win against it.
        let owner = self.foreground_owner(source)?;
        let (sender, receiver) = oneshot::channel();
        {
            let mut closed = self.foreground_cancel.subscribe();
            let mut operations = tokio::select! {
                biased;
                _ = closed.wait_for(|closed| *closed) => return Err(shutting_down()),
                error = bound => return Err(error),
                operations = self.foreground_operations.lock() => operations,
            };
            if self.foreground_shutdown.load(Ordering::Acquire) {
                return Err(shutting_down());
            }
            while let Some(result) = operations.try_join_next() {
                if let Err(error) = result {
                    tracing::error!(
                        error = %error,
                        "tracked foreground identity operation panicked"
                    );
                }
            }
            operations.spawn(async move {
                // Work this operation spawns on its behalf carries its owner.
                let result = FOREGROUND_OWNER.scope(owner.clone(), operation).await;
                drop(owner);
                let _ = sender.send(result);
            });
        }
        Ok(receiver)
    }

    /// Install the observer told (an epoch bump) whenever a verb of this
    /// runtime woke, retired or replaced an identity's Core member, and bump
    /// it once so the observer re-reads members that predate the
    /// installation. The unified runtime installs its identity health
    /// monitor's wake here.
    pub(crate) fn install_member_binding_observer(&self, observer: watch::Sender<u64>) {
        #[cfg(test)]
        self.binding_observer_saw_drain
            .store(self.has_pending_completion_drain(), Ordering::Release);
        *self
            .entries
            .binding_observer
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(observer);
        self.entries.notify_binding_changed();
    }

    /// Whether a completion drain was installed when the member-binding
    /// observer was.
    #[cfg(test)]
    pub(crate) fn binding_observer_installed_after_drain(&self) -> bool {
        self.binding_observer_saw_drain.load(Ordering::Acquire)
    }

    pub(crate) fn close_foreground_operations(&self) {
        self.close_foreground_operations_for_drain();
        self.foreground_waits_closed.send_replace(true);
    }

    /// Close foreground operations as [`Self::close_foreground_operations`]
    /// does, but leave the read-only waits (a turn's outcome, a completion,
    /// an output preview) waiting: a shutdown's in-flight drain lets admitted
    /// turns finish, and their waiters report how they ended. The shutdown
    /// that follows wakes them.
    pub(crate) fn close_foreground_operations_for_drain(&self) {
        self.foreground_shutdown.store(true, Ordering::Release);
        // `watch::Sender::send` discards the value when no receiver exists.
        // A just-admitted JoinSet task may not have subscribed yet, so retain
        // shutdown truth for future receivers explicitly.
        self.foreground_cancel.send_replace(true);
    }

    pub(crate) async fn join_foreground_operations(&self) {
        let mut operations = self.foreground_operations.lock().await;
        while let Some(result) = operations.join_next().await {
            if let Err(error) = result {
                tracing::error!(
                    error = %error,
                    "tracked foreground identity operation panicked during shutdown"
                );
            }
        }
    }

    /// Wake an identity: resume its committed session, or start it
    /// fresh when Core holds no history for it.
    ///
    /// The lazy counterpart to eager `restore_flow`: the build and Core's
    /// spawn or resume happen only when an identity is touched. Parallel calls
    /// for one identity coalesce on a per-identity lock. Returns Core's
    /// delivery scope of the live member (runtime id, generation, fence,
    /// session), read after the wake; `None` for a peer-only member, which
    /// Core binds to no session.
    pub async fn materialize(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        self.materialize_with_expected_member_alias(identity, None)
            .await
    }

    async fn materialize_with_expected_member_alias(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        self.materialize_with_expected_member_alias_after_inner(
            identity,
            expected_alias,
            None,
            std::future::ready(()),
            TopologyAfterEmbody::Reconcile,
            &mut false,
        )
        .await?;
        self.core_live_scope(identity).await
    }

    /// Materialize a Dormant/retired send or dispatch target on the admission
    /// path (the wake path).
    ///
    /// The target's own embodiment is required for admission, and so are its
    /// own managed edges: the input wakes the member, and its first turn must
    /// be able to reach the peers it is wired to (a peer message sent before
    /// the edge exists fails as "peer not found" with no wait or retry). So
    /// after the embodiment commits, the TARGET's edges (only edges touching
    /// it) are reconciled before admission, bounded (see
    /// [`Self::reconcile_target_topology_for_wake`]). The fleet-wide
    /// reconcile and the peer builds an ordinary materialization would run
    /// are not needed for admission and would serialize it behind every
    /// topology identity; the sender hydrates those after admission (see
    /// [`Self::hydrate_peer_topology_after_admission`]). Already-Active
    /// targets never come here: they are admitted first (the mid-turn case).
    ///
    /// A failed target reconcile (typed: `PeerWiringTimedOut`,
    /// `PeerTopologySuperseded`, a bridge refusal) is logged and does not
    /// refuse the input. That is the contract topology reconcile has always
    /// had on this path (a reconcile warning never failed a send), and the
    /// post-admission hydration retries it; refusing the operator's message
    /// because one edge could not be wired would trade a degraded first turn
    /// for a lost one.
    async fn materialize_for_admission(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        expected_spec_digest: Option<&str>,
    ) -> Result<(), IdentityRuntimeError> {
        let mut already_active = false;
        if let Err(error) = self
            .materialize_with_expected_member_alias_after_inner(
                identity,
                expected_alias,
                expected_spec_digest,
                std::future::ready(()),
                TopologyAfterEmbody::DeferToPostAdmission,
                &mut already_active,
            )
            .await
        {
            return Err(self.admission_wake_refusal(identity, error).await);
        }
        // A concurrent wake won: Core already held the member when the wake
        // door ran, so this send woke nothing and is admitted first like any
        // send to a live target.
        if already_active {
            return Ok(());
        }
        if let Err(error) = self.reconcile_target_topology_for_wake(identity).await {
            tracing::warn!(
                identity = %identity,
                %error,
                "woken target's managed edges were not reconciled before admission; the \
                 post-admission hydration retries them"
            );
        }
        Ok(())
    }

    /// The refusal an admission's wake answers. A host-adopted identity is
    /// never woken by Meta, but while Core still converges its present
    /// intent (after a restart Core waits for the previous process's
    /// identity lease) the refusal is retryable: nothing is queued, and the
    /// same request, sent again, is delivered once Core holds its live
    /// member. An absent intent, a
    /// blocked or final convergence, and a read Meta cannot make keep the
    /// final `IdentityHostAdopted`.
    async fn admission_wake_refusal(
        &self,
        identity: &AgentIdentity,
        error: IdentityRuntimeError,
    ) -> IdentityRuntimeError {
        if !matches!(error, IdentityRuntimeError::IdentityHostAdopted { .. }) {
            return error;
        }
        match self.host_convergence_pending(identity).await {
            Some(decision) => IdentityRuntimeError::IdentityWakeRetryable {
                identity: identity.clone(),
                session_id: None,
                detail: format!(
                    "a host adopted the identity and Core holds its present intent but no \
                     live member yet (Core's decision: {decision}); Meta never wakes it and \
                     queued nothing: send the request again once Core holds the member"
                ),
            },
            None => error,
        }
    }

    /// Core's decision for a host-adopted `identity` while its intent row
    /// declares a live member and its convergence is neither blocked nor
    /// final (`pending` before Core's first status row). `None` otherwise,
    /// and whenever either read fails or does not read.
    async fn host_convergence_pending(&self, identity: &AgentIdentity) -> Option<String> {
        use meerkat_mob::identity::{
            IdentityConvergenceCondition as Condition, IdentityStoredObservation as Observation,
        };
        let bridge = self.bridge.as_ref()?;
        if bridge.identity_intent_present(identity).await.ok()? != Some(true) {
            return None;
        }
        let status = match bridge.identity_convergence_status(identity).await.ok()? {
            Observation::Missing => return Some("pending".to_string()),
            Observation::Valid(status) => status,
            Observation::Unsupported { .. } | Observation::Malformed { .. } => return None,
        };
        match status.condition() {
            Condition::Pending
            | Condition::Reconciling
            | Condition::Suspended
            | Condition::Backoff
            | Condition::Converged => Some(
                status
                    .decision
                    .and_then(|decision| serde_json::to_value(decision).ok())
                    .and_then(|decision| decision.as_str().map(str::to_string))
                    .unwrap_or_else(|| "pending".to_string()),
            ),
            Condition::RepairBlocked
            | Condition::Quarantined
            | Condition::Tombstoned
            | Condition::DrainBlocked => None,
        }
    }

    /// Reconcile only `target`'s own managed edges, for the wake path.
    ///
    /// Bounded end to end by ONE deadline ([`WAKE_TOPOLOGY_BUDGET`] from the
    /// start): acquiring the topology admission guard and the managed-peer
    /// reconcile lock settles as
    /// `PeerWiringTimedOut { operation: "wake_topology_locks" }` when the
    /// deadline passes first, and every wiring call inside is capped by the
    /// remaining time (a timed-out wire is recorded unsettled like any
    /// other); a plan that keeps going stale settles as
    /// `PeerTopologySuperseded`. Only edges whose
    /// other endpoint is already Active can be wired here; a Dormant peer is
    /// built and wired by the post-admission hydration.
    async fn reconcile_target_topology_for_wake(
        &self,
        target: &AgentIdentity,
    ) -> Result<(), IdentityRuntimeError> {
        let Some(bridge) = self.bridge.clone() else {
            return Ok(());
        };
        let touches = |a: &AgentIdentity, b: &AgentIdentity| a == target || b == target;
        let declared = self.desired_peer_edges.read().await.clone();
        let controller = self.topology_controller();
        if controller.is_none()
            && !declared.iter().any(|edge| touches(edge.a(), edge.b()))
            && !self
                .managed_peer_edges
                .read()
                .await
                .iter()
                .any(|(a, b)| touches(a, b))
        {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + WAKE_TOPOLOGY_BUDGET;
        let (_topology_guard, _reconcile_guard) =
            bounded_peer_wiring_by("wake_topology_locks", Some(deadline), async {
                let topology_guard = match controller.as_ref() {
                    Some(controller) => Some(controller.mutation_guard().await),
                    None => None,
                };
                (
                    topology_guard,
                    self.managed_peer_reconcile_lock.lock().await,
                )
            })
            .await?;
        let desired_edges = match controller.as_ref() {
            Some(controller) => controller
                .compose_managed_peer_edges(&declared)
                .await
                .map_err(|error| {
                    IdentityRuntimeError::Internal(format!("topology overlay: {error}"))
                })?,
            None => declared,
        };
        let desired: BTreeSet<(AgentIdentity, AgentIdentity)> = desired_edges
            .iter()
            .filter(|edge| touches(edge.a(), edge.b()))
            .map(|edge| (edge.a().clone(), edge.b().clone()))
            .collect();
        self.reconcile_managed_peer_edges_planned(&bridge, &desired, Some(target), Some(deadline))
            .await
    }

    /// Complete a foreground materialization with an internal seam after the
    /// lifecycle transaction commits. Production callers use an immediately
    /// ready future; deterministic concurrency tests pause here to exercise
    /// superseding roster passes without adding runtime-visible hooks.
    async fn materialize_with_expected_member_alias_after_inner<F>(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        expected_spec_digest: Option<&str>,
        after_inner: F,
        topology: TopologyAfterEmbody,
        already_active: &mut bool,
    ) -> Result<(), IdentityRuntimeError>
    where
        F: Future<Output = ()>,
    {
        // `embody_identity` binds this attempt to the generation stored on
        // the IdentityEntry after it acquires the lifecycle lock. Sampling the
        // global generation here is insufficient: a newer reconcile can
        // publish G+1 and win that lock before this future reaches the entry,
        // in which case the materialized spec belongs to G+1, not G.
        let mut bootstrap_generation = None;
        let mut shutdown = self.foreground_cancel.subscribe();
        let result = self
            .embody_identity(
                identity,
                expected_alias,
                Some(&mut shutdown),
                None,
                &mut bootstrap_generation,
                EmbodimentOverrides {
                    expected_spec_digest,
                    ..EmbodimentOverrides::default()
                },
            )
            .await
            .map(|outcome| {
                *already_active = outcome.already_active;
            });
        after_inner.await;
        if matches!(
            &result,
            Err(IdentityRuntimeError::Internal(message)) if message == BACKGROUND_WARM_CANCELLED
        ) {
            if let Some(generation) = bootstrap_generation {
                self.mark_bootstrap_materialization_cancelled(identity, Some(generation));
            }
            return result;
        }
        // Alias validation happens under the lifecycle lock before
        // `embody_identity` marks bootstrap work as started. A stale alias
        // therefore must leave the replacement generation's exact readiness
        // state untouched, and so must a send refused for a superseded
        // registered row, which is refused before that mark.
        if matches!(
            &result,
            Err(IdentityRuntimeError::StaleRuntimeAlias { .. }
                | IdentityRuntimeError::RegisteredSpecChanged { .. })
        ) {
            return result;
        }
        if result.is_ok() && topology == TopologyAfterEmbody::Reconcile {
            let desired_edges = self.desired_peer_edges.read().await.clone();
            let has_pending_topology_recovery = match self.topology_controller() {
                Some(controller) => controller.has_pending().await,
                None => false,
            };
            if (!desired_edges.is_empty() || has_pending_topology_recovery)
                && let Err(error) = self.reconcile_managed_peer_edges(&desired_edges).await
            {
                tracing::warn!(
                    identity = %identity,
                    %error,
                    "identity materialized with topology reconcile warning"
                );
            }
        }
        if let Some(generation) = bootstrap_generation {
            self.mark_bootstrap_materialization_finished(identity, &result, Some(generation));
        }
        result
    }

    /// Cancellation-safe materialization for request/host boundaries.
    pub async fn materialize_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        self.run_tracked_foreground(async move { runtime.materialize(&identity).await })
            .await
    }

    /// Cancellation-safe compatibility restore used by embedders that pass
    /// an RPC identity context without attaching an
    /// [`IdentityFirstRuntimeContext`] to the unified runtime.
    pub(crate) async fn restore_flow_tracked(
        self: &Arc<Self>,
        roster: Vec<DurableAgentSpec>,
        topology_provider: Option<Arc<dyn TopologyProvider>>,
        customizer: Option<Arc<dyn AgentCustomizer>>,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        self.run_tracked_foreground(async move {
            super::orchestrator::restore_flow(
                runtime.as_ref(),
                &roster,
                topology_provider.as_deref(),
                customizer.as_deref(),
            )
            .await
        })
        .await
    }

    /// Cancellation-safe retirement for RPC/host request boundaries.
    pub async fn retire_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
    ) -> Result<(), IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        self.run_tracked_foreground(async move { runtime.retire(&identity).await })
            .await
    }

    /// Cancellation-safe retirement that atomically rejects an old generated
    /// runtime alias under the same lifecycle lock as the mutation.
    pub async fn retire_member_alias_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: &str,
    ) -> Result<(), IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.to_string();
        self.run_tracked_foreground(async move {
            runtime
                .retire_with_expected_member_alias(&identity, Some(&expected_alias))
                .await
        })
        .await
    }

    /// Retire identity authority and its captured lower-plane generations
    /// under one lifecycle lock. Enumeration performed by `cleanup` therefore
    /// observes the generation actually retired, even when this request had
    /// waited behind a concurrent reset.
    pub(crate) async fn retire_and_cleanup_live_members_tracked<T, F, Fut>(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        cleanup: F,
    ) -> Result<T, IdentityRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(Option<AgentRuntimeId>) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.map(str::to_owned);
        self.run_tracked_foreground(async move {
            let (retired, cleaned) = {
                let lifecycle_lock = runtime.lifecycle_lock_for(&identity).await;
                let _lifecycle_guard = lifecycle_lock.lock().await;
                if let Some(expected_alias) = expected_alias.as_deref() {
                    runtime
                        .ensure_expected_member_alias_current(&identity, expected_alias)
                        .await?;
                }
                let retired_alias = runtime
                    .core_live_member(&identity)
                    .await?
                    .map(|live| live.agent_runtime_id);
                let mut retiring = runtime
                    .retiring_incarnation(&identity, super::types::RetiredIdentityCause::Retire)
                    .await?;
                runtime.retire_locked(&identity).await?;
                runtime.settle_retired_registration(&mut retiring).await;
                (retiring, cleanup(retired_alias).await)
            };
            // After the cleanup, with the lifecycle lock released.
            runtime
                .release_retired_members(retired.into_iter().collect())
                .await;
            Ok(cleaned)
        })
        .await
    }

    /// Cancellation-safe respawn for RPC/host request boundaries.
    pub async fn respawn_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        self.run_tracked_foreground(async move { runtime.respawn(&identity).await })
            .await
    }

    /// Cancellation-safe respawn that atomically rejects an old generated
    /// runtime alias under the same lifecycle lock as the mutation.
    pub async fn respawn_member_alias_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: &str,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.to_string();
        self.run_tracked_foreground(async move {
            runtime
                .respawn_with_expected_member_alias(&identity, Some(&expected_alias))
                .await
        })
        .await
    }

    /// Execute a lower member-plane mutation while pinning a generated alias
    /// to the durable identity generation that owns it. This is the common
    /// authority boundary for legacy member RPCs (for example force-cancel)
    /// that cannot otherwise express identity continuity.
    pub(crate) async fn run_member_alias_operation_tracked<T, F, Fut>(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: &str,
        operation: F,
    ) -> Result<T, IdentityRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, String>> + Send + 'static,
    {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.to_string();
        self.run_tracked_foreground(async move {
            let lifecycle_lock = runtime.lifecycle_lock_for(&identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            runtime
                .ensure_expected_member_alias_current(&identity, &expected_alias)
                .await?;
            operation().await.map_err(IdentityRuntimeError::Internal)
        })
        .await
    }

    /// Cancellation-safe identity-first respawn for RPC and console boundaries.
    ///
    /// A durable identity already owns an authoritative Meerkat session. Its
    /// respawn retires the member and wakes it onto that same session (Core
    /// resumes it under a new generation). It must not call the raw
    /// member-plane respawn convenience: that operation creates a new session
    /// and would silently turn a non-destructive identity recovery into a
    /// destructive reset.
    pub(crate) async fn respawn_identity_in_place_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        match expected_alias {
            Some(expected_alias) => {
                self.respawn_member_alias_tracked(identity, expected_alias)
                    .await
            }
            None => self.respawn_tracked(identity).await,
        }
    }

    /// Cancellation-safe destructive reset for RPC/host boundaries.
    pub async fn reset_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberBinding, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        self.run_tracked_foreground(async move { runtime.reset(&identity).await })
            .await
    }

    /// Cancellation-safe reset that atomically rejects an old generated
    /// runtime alias under the same lifecycle lock as the mutation.
    pub async fn reset_member_alias_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: &str,
    ) -> Result<meerkat_mob::MemberBinding, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.to_string();
        self.run_tracked_foreground(async move {
            runtime
                .reset_with_expected_member_alias(&identity, Some(&expected_alias))
                .await
        })
        .await
    }

    /// Cancellation-safe identity deletion for RPC/host boundaries.
    pub async fn delete_identity_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        self.run_tracked_foreground(async move { runtime.delete_identity(&identity).await })
            .await
    }

    /// Cancellation-safe deletion that atomically rejects an old generated
    /// runtime alias under the same lifecycle lock as the mutation.
    pub async fn delete_identity_member_alias_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: &str,
    ) -> Result<Option<meerkat_mob::MemberBinding>, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.to_string();
        self.run_tracked_foreground(async move {
            runtime
                .delete_identity_with_expected_member_alias(&identity, Some(&expected_alias))
                .await
        })
        .await
    }

    /// Delete identity authority and clean its captured concrete generations
    /// in the same lifecycle transaction.
    pub(crate) async fn delete_identity_and_cleanup_live_members_tracked<T, F, Fut>(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        cleanup: F,
    ) -> Result<T, IdentityRuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(Option<AgentRuntimeId>) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.map(str::to_owned);
        self.run_tracked_foreground(async move {
            let lifecycle_lock = runtime.lifecycle_lock_for(&identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            if let Some(expected_alias) = expected_alias.as_deref() {
                runtime
                    .ensure_expected_member_alias_current(&identity, expected_alias)
                    .await?;
            }
            // The live member the delete replaces (a dormant identity has
            // none): its captured concrete generation is cleaned up.
            let deleted_alias = runtime
                .core_live_member(&identity)
                .await?
                .map(|binding| binding.agent_runtime_id);
            runtime.delete_identity_locked(&identity).await?;
            Ok(cleanup(deleted_alias).await)
        })
        .await
    }

    /// Background warming observes controller cancellation after lease
    /// acquisition bookkeeping but before bridge/member installation. A
    /// cancelled grant is explicitly released; after customization completes,
    /// the operation is joined to its explicit commit/rollback boundary.
    async fn materialize_for_background(
        &self,
        identity: &AgentIdentity,
        cancellation: &mut watch::Receiver<bool>,
        generation: u64,
    ) -> Option<Result<(), IdentityRuntimeError>> {
        // Background warm is indirect hydration: a retired identity stays
        // retired until something addressed to it revives it.
        if let Some(err) = self.retired_terminal_refusal(identity, "materialize").await {
            return Some(Err(err));
        }
        let mut bound_generation = None;
        let result = self
            .embody_identity(
                identity,
                None,
                Some(cancellation),
                Some(generation),
                &mut bound_generation,
                EmbodimentOverrides::default(),
            )
            .await
            .map(|_| ());
        if matches!(
            &result,
            Err(IdentityRuntimeError::Internal(message)) if message == BACKGROUND_WARM_CANCELLED
        ) {
            self.mark_bootstrap_materialization_cancelled(identity, Some(generation));
            return None;
        }
        if result.is_ok() {
            let desired_edges = self.desired_peer_edges.read().await.clone();
            if !desired_edges.is_empty()
                && let Err(error) = self.reconcile_managed_peer_edges(&desired_edges).await
            {
                tracing::warn!(
                    identity = %identity,
                    %error,
                    "background identity materialized with topology reconcile warning"
                );
            }
        }
        self.mark_bootstrap_materialization_finished(identity, &result, Some(generation));
        Some(result)
    }

    pub(crate) async fn embody_identity(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        cancellation: Option<&mut watch::Receiver<bool>>,
        expected_bootstrap_generation: Option<u64>,
        bound_bootstrap_generation: &mut Option<u64>,
        overrides: EmbodimentOverrides<'_>,
    ) -> Result<EmbodimentOutcome, IdentityRuntimeError> {
        let override_customizer = overrides.customizer;
        let mut outcome = {
            let lifecycle_lock = self.lifecycle_lock_for(identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            self.embody_identity_locked(
                identity,
                expected_alias,
                cancellation,
                expected_bootstrap_generation,
                bound_bootstrap_generation,
                overrides,
            )
            .await?
        };
        // The caller's customizer is told with the lifecycle lock released,
        // so its `after_create` may call back into this identity.
        if let (Some(customizer), Some(notice)) =
            (override_customizer, outcome.override_after_create.take())
        {
            Self::tell_after_create(customizer, identity, &notice).await;
        }
        Ok(outcome)
    }

    // -----------------------------------------------------------------------
    // Core facts: read per call, never cached
    // -----------------------------------------------------------------------

    /// The session bridge every Core read and lifecycle verb goes through.
    /// Identity lifecycle needs Core's mob; a runtime without a bridge has
    /// no member to wake.
    fn core_bridge(
        &self,
        identity: &AgentIdentity,
    ) -> Result<&Arc<dyn SessionBridge>, IdentityRuntimeError> {
        self.bridge.as_ref().ok_or_else(|| {
            IdentityRuntimeError::Internal(format!(
                "identity '{identity}': no session bridge is installed, so Core's mob cannot \
                 wake, retire or read it"
            ))
        })
    }

    /// The identity Core's member history binds `session_id` to, live or
    /// retired (Core's journal is the only record of who owns a
    /// session). `Ok(None)` when no binding names the session, or without a
    /// session bridge; a failed read is typed history-unavailable.
    pub async fn core_session_owner(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<AgentIdentity>, IdentityRuntimeError> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(None);
        };
        let history = bridge.member_binding_history().await.map_err(|error| {
            IdentityRuntimeError::MemberHistoryUnavailable {
                detail: format!("reading the owner of session {session_id}: {error}"),
            }
        })?;
        let Some(binding) = history.binding_for_session(session_id) else {
            return Ok(None);
        };
        let durable =
            crate::member_comms_id::durable_identity_for_roster_member(binding.identity().as_str());
        AgentIdentity::parse(&durable).map(Some).map_err(|error| {
            IdentityRuntimeError::Internal(format!(
                "Core binds session {session_id} to '{durable}', which is not a valid identity: \
                 {error}"
            ))
        })
    }

    /// Core's live member for `identity`: the runtime binding Core committed
    /// (runtime id and session, `None` for a peer-only member), or `None`
    /// while Core holds no member. Read-only: it revives nothing. A failed
    /// read is typed history-unavailable, never "not live".
    pub(crate) async fn core_live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<super::bridge::MemberRuntimeBinding>, IdentityRuntimeError> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(None);
        };
        bridge.live_member(identity).await.map_err(|error| {
            IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!("reading Core's live member: {error}"),
            }
        })
    }

    /// Core's delivery scope of `identity`'s live member (runtime id,
    /// generation, fence, session), or `None` while Core holds no
    /// session-backed member. Core refuses the scope read of a member it
    /// holds no session binding for, such as a retired one: when Core then
    /// holds no live member either, there is no live scope.
    pub(crate) async fn core_live_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(None);
        };
        match bridge.live_member_scope(identity).await {
            Ok(scope) => Ok(scope),
            Err(error) => match bridge.live_member(identity).await {
                Ok(None) => Ok(None),
                _ => Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                    identity: identity.clone(),
                    detail: format!("reading Core's live member scope: {error}"),
                }),
            },
        }
    }

    /// `identity`'s latest binding since the latest mob reset, live or
    /// retired; `None` when Core never spawned it since then (a wake
    /// after a mob reset is fresh). A failed read is never "no history".
    async fn core_last_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, IdentityRuntimeError> {
        self.core_bridge(identity)?
            .last_member_binding(identity)
            .await
            .map_err(|error| IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!("reading Core's member history: {error}"),
            })
    }

    /// Whether Core holds an identity intent row for `identity` (a host
    /// adopted it, Present or Absent). Such an identity is read-only to every
    /// Meta verb.
    async fn core_identity_management(
        &self,
        identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, IdentityRuntimeError> {
        self.core_bridge(identity)?
            .identity_management(identity)
            .await
            .map_err(|error| IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!("reading Core's identity management: {error}"),
            })
    }

    /// On every lifecycle verb: refuse an identity a host adopted. A read
    /// fault refuses too (history unavailable): Meta never acts on an
    /// identity it could not prove is its own.
    pub(crate) async fn refuse_host_adopted(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), IdentityRuntimeError> {
        if self.bridge.is_none() {
            return Ok(());
        }
        match self.core_identity_management(identity).await? {
            meerkat_mob::MemberIdentityManagement::CommandDriven => Ok(()),
            meerkat_mob::MemberIdentityManagement::IntentManaged => {
                Err(IdentityRuntimeError::IdentityHostAdopted {
                    identity: identity.clone(),
                })
            }
            // An intent row this build cannot read may declare anything:
            // never act on it until it reads.
            meerkat_mob::MemberIdentityManagement::IntentUnreadable => {
                Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                    identity: identity.clone(),
                    detail: "Core holds an identity intent row for the identity that this \
                             build cannot read"
                        .to_string(),
                })
            }
            // A management kind this build does not know: never act on it.
            _ => Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: "Core reports an identity management kind this build does not know"
                    .to_string(),
            }),
        }
    }

    // -----------------------------------------------------------------------
    // The first-boot manifest
    // -----------------------------------------------------------------------

    /// Point the runtime at its state directory, where
    /// `storage-migrate-native` leaves the first-boot manifest of a migrated
    /// state directory. The first roster compile (or wake) reads it; `None`
    /// means there is none.
    pub fn set_first_boot_state_dir(&self, state_dir: Option<PathBuf>) {
        self.set_first_boot_location(state_dir.map(FirstBootLocation::StateDir));
    }

    /// Point the runtime at where its first-boot manifest lives: a state
    /// directory, or the `first-boot` document of a host store realm, which
    /// an import of a continuity export leaves. `None` means there is none.
    pub(crate) fn set_first_boot_location(&self, location: Option<FirstBootLocation>) {
        *self
            .first_boot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = match location {
            Some(location) => FirstBootRows::Unread { location },
            None => FirstBootRows::NoStateDir,
        };
    }

    /// Read the first-boot manifest, once. A manifest the reader refuses
    /// fails typed (`FirstBootManifestUnreadable`) on every call until it
    /// reads: which identities it covers is unknown, so nothing may wake.
    pub(crate) async fn load_first_boot_manifest(&self) -> Result<(), IdentityRuntimeError> {
        if !self.first_boot_unread() {
            return Ok(());
        }
        let writer = self.first_boot_writer.lock().await;
        let location = match &*self
            .first_boot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            FirstBootRows::Unread { location } => location.clone(),
            _ => return Ok(()),
        };
        let manifest = match &location {
            FirstBootLocation::StateDir(state_dir) => {
                let read_dir = state_dir.clone();
                tokio::task::spawn_blocking(move || FirstBootManifest::read(&read_dir))
                    .await
                    .map_err(|error| {
                        IdentityRuntimeError::Internal(format!(
                            "reading the first-boot manifest did not finish: {error}"
                        ))
                    })?
            }
            FirstBootLocation::HostStore(documents) => {
                FirstBootManifest::read_host_store(documents).await
            }
        }
        .map_err(|error| IdentityRuntimeError::FirstBootManifestUnreadable {
            path: error.path().to_path_buf(),
            detail: error.refusal().to_string(),
        })?;
        *self
            .first_boot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = match manifest {
            None => FirstBootRows::Settled,
            Some(manifest) => {
                let file_rows_sha256 = manifest.rows_sha256().to_string();
                let rows = manifest
                    .into_rows()
                    .into_iter()
                    .map(|row| (row.identity().clone(), row))
                    .collect::<BTreeMap<_, _>>();
                FirstBootRows::Pending {
                    location,
                    // A manifest without rows is removed by the next
                    // rewrite.
                    file_stale: rows.is_empty(),
                    rows,
                    file_rows_sha256,
                }
            }
        };
        drop(writer);
        self.publish_first_boot_pending().await;
        Ok(())
    }

    fn first_boot_unread(&self) -> bool {
        matches!(
            &*self
                .first_boot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            FirstBootRows::Unread { .. }
        )
    }

    /// `identity`'s first-boot row still to carry out, if any (the manifest
    /// is read first when nothing read it yet).
    async fn pending_first_boot_row(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<FirstBootRow>, IdentityRuntimeError> {
        self.load_first_boot_manifest().await?;
        Ok(
            match &*self
                .first_boot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                FirstBootRows::Pending { rows, .. } => rows.get(identity).cloned(),
                _ => None,
            },
        )
    }

    /// Whether `identity` has a first-boot row still to carry out, as the
    /// manifest last read says.
    pub(crate) fn has_pending_first_boot_row(&self, identity: &AgentIdentity) -> bool {
        matches!(
            &*self
                .first_boot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            FirstBootRows::Pending { rows, .. } if rows.contains_key(identity)
        )
    }

    /// Refuse `operation` while `identity` has a first-boot row to carry
    /// out: it would start a life the row does not know.
    async fn refuse_pending_first_boot_row(
        &self,
        identity: &AgentIdentity,
        operation: &'static str,
    ) -> Result<(), IdentityRuntimeError> {
        match self.pending_first_boot_row(identity).await? {
            Some(row) => Err(IdentityRuntimeError::FirstBootRowPending {
                identity: identity.clone(),
                kind: row.kind(),
                waiting_for: self.first_boot_wait(identity).await,
                operation,
            }),
            None => Ok(()),
        }
    }

    /// What `identity`'s pending first-boot row waits for: the roster, while
    /// the roster does not list the identity; otherwise its carry-out.
    async fn first_boot_wait(&self, identity: &AgentIdentity) -> FirstBootWait {
        if self.entries.read().await.contains_key(identity) {
            FirstBootWait::CarryOut
        } else {
            FirstBootWait::Roster
        }
    }

    /// Carry out `identity`'s first-boot row, if it has one, before anything
    /// wakes it. The caller holds the identity's lifecycle lock.
    /// Core's member history decides ([`FirstBootRow::state_in`]):
    /// - done: the row is settled and nothing changes;
    /// - a `Fresh` row to do: the identity's fresh life starts as a reset
    ///   starts it, keeping its liveness (Core replaces a live member by its
    ///   successor, and gives a dormant identity a fresh session it retires
    ///   at once), and the row is settled;
    /// - a `Resume` row to do: returns the row's session, for the caller's
    ///   wake to resume; the row is settled once that resume succeeds;
    /// - contradicted: refused typed (`FirstBootRowRefused`); nothing
    ///   changes and the row stays.
    async fn first_boot_step_locked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<SessionId>, IdentityRuntimeError> {
        let Some(row) = self.pending_first_boot_row(identity).await? else {
            return Ok(None);
        };
        let history = self
            .core_bridge(identity)?
            .member_binding_history()
            .await
            .map_err(|error| IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!(
                    "reading Core's member history for the identity's first-boot row: {error}"
                ),
            })?;
        match row.state_in(&history) {
            FirstBootRowState::Done => {
                self.settle_first_boot_row(identity).await;
                Ok(None)
            }
            FirstBootRowState::Contradicted {
                core_session,
                detail,
            } => Err(IdentityRuntimeError::FirstBootRowRefused {
                identity: identity.clone(),
                kind: row.kind(),
                manifest_session: row.session().clone(),
                core_session,
                detail,
            }),
            FirstBootRowState::ToDo => match row {
                FirstBootRow::Resume { session, .. } => Ok(Some(session)),
                // A fresh life replaces a live member by its successor: a
                // candidate launch leaves the row to the authoritative one.
                FirstBootRow::Fresh { .. } if self.is_candidate_launch() => {
                    Err(IdentityRuntimeError::FirstBootRowPending {
                        identity: identity.clone(),
                        kind: FirstBootRowKind::Fresh,
                        waiting_for: FirstBootWait::CarryOut,
                        operation: CANDIDATE_FIRST_BOOT_FRESH,
                    })
                }
                FirstBootRow::Fresh { .. } => {
                    let mut foreground_shutdown = self.foreground_cancel.subscribe();
                    let live = self
                        .start_fresh_life_locked(
                            identity,
                            &mut foreground_shutdown,
                            "first-boot fresh start",
                            None,
                        )
                        .await?;
                    // A live member was replaced by its successor: the
                    // views settle as after a reset. A dormant identity's
                    // fresh life leaves it as it was registered.
                    if live {
                        self.settle_fresh_life(identity, true, None).await;
                    }
                    self.settle_first_boot_row(identity).await;
                    Ok(None)
                }
            },
        }
    }

    /// A delivery to a live member whose first-boot row is still pending
    /// reads the row against Core's history again first: a done row
    /// settles, a contradicted one refuses the delivery typed, and a `Fresh`
    /// row still to do is carried out, replacing the member by its
    /// successor. (A `Resume` row still to do has no live member; it refuses
    /// as pending.) While the manifest is unread, any identity may have a
    /// row in it: the delivery reads it first, and refuses typed while it
    /// stays unreadable. Returns whether a row was read: the caller reads
    /// Core's live member again, since it may be the successor now.
    async fn first_boot_guard_live_locked(
        &self,
        identity: &AgentIdentity,
        operation: &'static str,
    ) -> Result<bool, IdentityRuntimeError> {
        if !self.has_pending_first_boot_row(identity) && !self.first_boot_unread() {
            return Ok(false);
        }
        match self.first_boot_step_locked(identity).await? {
            None => Ok(true),
            Some(_) => Err(IdentityRuntimeError::FirstBootRowPending {
                identity: identity.clone(),
                kind: FirstBootRowKind::Resume,
                waiting_for: FirstBootWait::CarryOut,
                operation,
            }),
        }
    }

    /// Carry out the pending first-boot rows of the identities a roster
    /// compile just registered, before anything wakes them. Each
    /// entry carries its identity's lifecycle reservation, held until its
    /// row is carried out. A `Resume` row is carried out by the identity's
    /// wake onto the row's session; a `Fresh` row by a fresh life that keeps
    /// the identity's liveness. Returns each identity whose row was refused
    /// or failed, with the typed error: its row stays pending, and the
    /// identity's next wake or roster compile tries it again.
    pub(crate) async fn carry_out_first_boot_rows(
        &self,
        registered: Vec<(DurableAgentSpec, tokio::sync::OwnedMutexGuard<()>)>,
    ) -> Vec<(AgentIdentity, IdentityRuntimeError)> {
        let failed = stream::iter(registered)
            .map(|(spec, lifecycle_guard)| async move {
                let result = self.carry_out_first_boot_row_locked(&spec).await;
                drop(lifecycle_guard);
                result.err().map(|error| (spec.identity, error))
            })
            .buffer_unordered(super::orchestrator::identity_restore_concurrency())
            .filter_map(std::future::ready)
            .collect::<Vec<_>>()
            .await;
        // A rewrite that failed before is retried once per compile.
        self.rewrite_first_boot_manifest().await;
        self.publish_first_boot_pending().await;
        failed
    }

    async fn carry_out_first_boot_row_locked(
        &self,
        spec: &DurableAgentSpec,
    ) -> Result<(), IdentityRuntimeError> {
        let identity = &spec.identity;
        let Some(row) = self.pending_first_boot_row(identity).await? else {
            return Ok(());
        };
        match row.kind() {
            // The wake door carries a `Resume` row out: it resumes the
            // row's session and settles the row.
            FirstBootRowKind::Resume => {
                let mut bound_bootstrap_generation = None;
                let result = self
                    .embody_identity_locked(
                        identity,
                        None,
                        None,
                        None,
                        &mut bound_bootstrap_generation,
                        EmbodimentOverrides {
                            spec: Some(spec),
                            ..EmbodimentOverrides::default()
                        },
                    )
                    .await;
                self.mark_bootstrap_materialization_finished(
                    identity,
                    &result,
                    bound_bootstrap_generation,
                );
                result.map(|_| ())
            }
            FirstBootRowKind::Fresh => self.first_boot_step_locked(identity).await.map(|_| ()),
        }
    }

    /// Respawn, once and through the bridge, each identity of `listed`
    /// whose live Core member lacks the durable-identity marker: a member
    /// Meta 0.9.3 spawned (it never set one) that Core kept live across the
    /// upgrade. Extension documents grant a member's sessions the identity
    /// only on that marker, and the bridge's resume carries it.
    ///
    /// The roster compile runs this after its first-boot rows, on every
    /// compile: a marked member is skipped, so a later compile does nothing.
    /// One read of Core's member history picks the members
    /// ([`Self::unmarked_live_session`]); a failed read respawns nothing and
    /// the next compile reads again. An identity whose first-boot row is
    /// still pending is left to its row. Each respawn takes the identity's
    /// lifecycle lock and is the reload pair: the member retires (no memory
    /// harvest: the session does not rotate) and the wake door resumes the
    /// same session at a new generation, so the transcript is kept; the
    /// member's queued inputs are captured before the retire and admitted to
    /// the resumed session. A
    /// not-live member is left alone: its next wake resumes it through the
    /// bridge, which marks it. Returns each identity whose respawn failed,
    /// with the typed error; the others go on. A candidate launch respawns
    /// nothing: the respawn retires the stored member and spawns one onto
    /// its session, which Core journals in the shared mob store. Each
    /// identity an authoritative launch would respawn is returned with the
    /// candidate's refusal instead, so it is deferred to that launch as
    /// `resume_member`, not broken, and its member stays as Core restored
    /// it.
    pub(crate) async fn respawn_unmarked_identities(
        &self,
        listed: &[DurableAgentSpec],
    ) -> Vec<(AgentIdentity, IdentityRuntimeError)> {
        if listed.is_empty() {
            return Vec::new();
        }
        let history = match self.core_view_history().await {
            Ok(Some(history)) => history,
            // Without a bridge Core holds no member of this runtime.
            Ok(None) => return Vec::new(),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "reading Core's member history for members without the durable-identity \
                     marker failed; nothing is respawned, and the next roster compile reads it \
                     again"
                );
                return Vec::new();
            }
        };
        let unmarked = listed
            .iter()
            .filter(|spec| !self.has_pending_first_boot_row(&spec.identity))
            .filter_map(|spec| {
                let session = Self::unmarked_live_session(&history, spec)?;
                Some((spec.clone(), session))
            })
            .collect::<Vec<_>>();
        stream::iter(unmarked)
            .map(|(spec, session)| async move {
                let lifecycle_lock = self.lifecycle_lock_for(&spec.identity).await;
                let _lifecycle_guard = lifecycle_lock.lock().await;
                let result = self.respawn_unmarked_locked(&spec, &session).await;
                result.err().map(|error| (spec.identity, error))
            })
            .buffer_unordered(super::orchestrator::identity_restore_concurrency())
            .filter_map(std::future::ready)
            .collect::<Vec<_>>()
            .await
    }

    /// The session of `spec`'s live Core member when that member lacks the
    /// identity's durable-identity marker and a respawn through the bridge
    /// can keep the session. `None` for a peer-only identity (no session to
    /// keep), one an identity intent row manages (a host adopted it, or the
    /// row does not read: no Meta verb acts on it), a member Core does not
    /// hold live, a marked one, one without a session, and one whose role
    /// is not the roster profile (the wake would hold it, retired). A member
    /// skipped for its role stays unmarked, so extension documents refuse its
    /// sessions until a declared migration respawns it.
    fn unmarked_live_session(
        history: &meerkat_mob::MemberBindingHistory,
        spec: &DurableAgentSpec,
    ) -> Option<SessionId> {
        if durable_spec_uses_external_binding(spec) {
            return None;
        }
        let member = crate::member_comms_id::roster_member_id_for_identity(spec.identity.as_str());
        if history.identity_management(&member)
            != meerkat_mob::MemberIdentityManagement::CommandDriven
        {
            return None;
        }
        let binding = history.last_binding(&member)?;
        if !binding.is_live()
            || binding_carries_identity_marker(binding, &spec.identity)
            || *binding.role() != spec.profile
        {
            return None;
        }
        binding.session_id().cloned()
    }

    /// Respawn `spec`'s identity onto `session` under the caller's lifecycle
    /// lock ([`Self::respawn_unmarked_identities`]). Nothing happens when
    /// Core's live member no longer runs `session` (a verb moved it since
    /// the history read), or when Core holds no durable copy of `session` or
    /// the bridge cannot say (the wake would start a fresh session in its
    /// place and lose the live member's transcript).
    async fn respawn_unmarked_locked(
        &self,
        spec: &DurableAgentSpec,
        session: &SessionId,
    ) -> Result<(), IdentityRuntimeError> {
        let identity = &spec.identity;
        let still_live = self
            .core_live_member(identity)
            .await?
            .is_some_and(|live| live.session_id.as_ref() == Some(session));
        if !still_live {
            return Ok(());
        }
        match self
            .core_bridge(identity)?
            .durable_session_held(session)
            .await
        {
            Ok(true) => {}
            Ok(false) | Err(BridgeError::Unsupported { .. }) => {
                tracing::warn!(
                    %identity,
                    %session,
                    "the identity's live member lacks the durable-identity marker, but nothing \
                     vouches for a durable copy of its session; it is not respawned"
                );
                return Ok(());
            }
            Err(error) => {
                return Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                    identity: identity.clone(),
                    detail: format!(
                        "reading Core's session state before respawning a member without the \
                         durable-identity marker: {error}"
                    ),
                });
            }
        }
        // A candidate launch defers the respawn to the authoritative launch
        // (`candidate_deferral_of`): the member stays as Core restored it.
        if self.is_candidate_launch() {
            tracing::info!(
                %identity,
                %session,
                "candidate launch: a live member without the durable-identity marker is not \
                 respawned; the identity is deferred to the authoritative launch, which \
                 respawns it onto the same session"
            );
            return Err(IdentityRuntimeError::InvalidState {
                identity: identity.clone(),
                state: IdentityLifecycleState::Active,
                operation: CANDIDATE_RESUME_MEMBER,
            });
        }
        // The retire takes the member's queued inputs with it: they are
        // captured first (the delivery repair's own capture) and admitted to
        // the resumed session. A queue that cannot be observed leaves the
        // member as it is, so no input is lost unseen.
        let bridge = Arc::clone(self.core_bridge(identity)?);
        let queued = match bridge.capture_queued_member_inputs(identity, session).await {
            Ok(queued) => queued,
            Err(error) => {
                tracing::warn!(
                    %identity,
                    %session,
                    %error,
                    "the queue of a live member without the durable-identity marker could not \
                     be observed; it is not respawned"
                );
                return Ok(());
            }
        };
        tracing::info!(
            %identity,
            %session,
            queued = queued.carryable(),
            "respawning a live member without the durable-identity marker onto the same session"
        );
        if let Err(error) = self
            .retire_locked_with_intent(identity, LifecycleRetireIntent::Reload)
            .await
        {
            if queued.carryable() > 0 {
                tracing::error!(
                    %identity,
                    lost = queued.carryable(),
                    %error,
                    "the retire before the respawn failed; the queued inputs captured before it \
                     are lost if Core retired the member"
                );
            }
            return Err(error);
        }
        let mut bound_bootstrap_generation = None;
        let result = self
            .embody_identity_locked(
                identity,
                None,
                None,
                None,
                &mut bound_bootstrap_generation,
                EmbodimentOverrides::default(),
            )
            .await;
        self.mark_bootstrap_materialization_finished(identity, &result, bound_bootstrap_generation);
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                if queued.carryable() > 0 {
                    tracing::error!(
                        %identity,
                        lost = queued.carryable(),
                        %error,
                        "the wake after the retire failed; the queued inputs captured before the \
                         retire are lost"
                    );
                }
                return Err(error);
            }
        };
        match outcome.binding.session_id.as_ref() {
            Some(resumed) => {
                bridge
                    .readmit_queued_member_inputs(identity, resumed, queued)
                    .await;
            }
            None if queued.carryable() > 0 => tracing::error!(
                %identity,
                lost = queued.carryable(),
                "the resumed member has no session to admit the captured queued inputs to; they \
                 are lost"
            ),
            None => {}
        }
        Ok(())
    }

    /// Settle the pending first-boot rows of identities the roster does not
    /// list that Core's member history shows done
    /// ([`FirstBootRow::state_in`]). Such a row would otherwise wait for a
    /// roster compile that lists its identity, which may never come: notably
    /// a `Fresh` row `storage-migrate-native` before 0.9.5 wrote for a helper
    /// or a fork child the host made on the mob and retired, whose
    /// identity's wake starts fresh anyway. A row still to do, or one Core
    /// contradicts, keeps waiting for the roster. One read of Core's member
    /// history; a failed read settles nothing, and the next roster compile
    /// reads it again.
    pub(crate) async fn settle_done_unlisted_first_boot_rows(&self) {
        let unlisted = {
            let entries = self.entries.read().await;
            match &*self
                .first_boot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                FirstBootRows::Pending { rows, .. } => rows
                    .values()
                    .filter(|row| !entries.contains_key(row.identity()))
                    .cloned()
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            }
        };
        if unlisted.is_empty() {
            return;
        }
        let history = match self.core_view_history().await {
            Ok(Some(history)) => history,
            // Without a bridge Core holds no member of this runtime.
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "reading Core's member history for the first-boot rows no roster lists \
                     failed; they stay pending, and the next roster compile reads it again"
                );
                return;
            }
        };
        let done = unlisted
            .iter()
            .filter(|row| matches!(row.state_in(&history), FirstBootRowState::Done))
            .map(|row| row.identity().clone())
            .collect::<Vec<_>>();
        if done.is_empty() {
            return;
        }
        if let FirstBootRows::Pending {
            rows, file_stale, ..
        } = &mut *self
            .first_boot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            for identity in &done {
                if rows.remove(identity).is_some() {
                    *file_stale = true;
                }
            }
        }
        tracing::info!(
            settled = done.len(),
            "first-boot rows no roster lists, which Core's member history shows done, are settled"
        );
        self.rewrite_first_boot_manifest().await;
        self.publish_first_boot_pending().await;
    }

    /// A first-boot row is carried out: it leaves the pending rows, and the
    /// manifest file is rewritten without it (removed once none remains).
    async fn settle_first_boot_row(&self, identity: &AgentIdentity) {
        if let FirstBootRows::Pending {
            rows, file_stale, ..
        } = &mut *self
            .first_boot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            && rows.remove(identity).is_some()
        {
            *file_stale = true;
        }
        self.rewrite_first_boot_manifest().await;
        self.publish_first_boot_pending().await;
    }

    /// Bring the manifest file in line with the rows still pending: rewrite
    /// it without the carried-out ones, or remove it once none remains. A
    /// failed rewrite is logged and retried by the next roster compile; the
    /// carried-out rows stay done, since Core's history shows them.
    async fn rewrite_first_boot_manifest(&self) {
        // A candidate launch writes no shared state: it carries out no row
        // (a `Resume` row's wake and a `Fresh` row's fresh life are both
        // deferred), the rows it found done leave only its own pending rows,
        // and the file stays as it found it for the authoritative launch,
        // which settles them again.
        if self.is_candidate_launch() {
            let pending = match &*self
                .first_boot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
            {
                FirstBootRows::Pending { rows, .. } => rows.len(),
                _ => 0,
            };
            tracing::warn!(
                pending,
                "candidate launch census: the first-boot manifest is left as this launch found \
                 it; the authoritative launch rewrites it once it settles the rows"
            );
            return;
        }
        let _writer = self.first_boot_writer.lock().await;
        let (location, expected, remaining) = match &*self
            .first_boot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            FirstBootRows::Pending {
                location,
                rows,
                file_rows_sha256,
                file_stale: true,
            } => (
                location.clone(),
                file_rows_sha256.clone(),
                rows.values().cloned().collect::<Vec<_>>(),
            ),
            _ => return,
        };
        let rewritten = match &location {
            FirstBootLocation::StateDir(state_dir) => {
                let rewrite_dir = state_dir.clone();
                tokio::task::spawn_blocking(move || {
                    FirstBootManifest::replace_rows(&rewrite_dir, &expected, remaining)
                })
                .await
            }
            FirstBootLocation::HostStore(documents) => Ok(
                FirstBootManifest::replace_host_store_rows(documents, &expected, remaining).await,
            ),
        };
        let mut first_boot = self
            .first_boot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match rewritten {
            Ok(Ok(Some(manifest))) => {
                if let FirstBootRows::Pending {
                    rows,
                    file_rows_sha256,
                    file_stale,
                    ..
                } = &mut *first_boot
                {
                    *file_rows_sha256 = manifest.rows_sha256().to_string();
                    // A row settled while the file was written is still in it.
                    *file_stale = manifest.rows().len() != rows.len();
                }
            }
            Ok(Ok(None)) => *first_boot = FirstBootRows::Settled,
            Ok(Err(error))
                if matches!(error.refusal(), FirstBootManifestRefusal::Changed { .. }) =>
            {
                tracing::warn!(
                    %error,
                    "the first-boot manifest changed under this process; it is read again before \
                     the next wake or roster compile"
                );
                *first_boot = FirstBootRows::Unread { location };
            }
            Ok(Err(error)) => tracing::warn!(
                %error,
                "a carried-out first-boot row is still in the manifest; the next roster compile \
                 rewrites it"
            ),
            Err(error) => tracing::warn!(
                %error,
                "the first-boot manifest rewrite did not finish; the next roster compile retries it"
            ),
        }
    }

    /// Publish the pending first-boot rows on the bootstrap status: each
    /// waits for the roster, or (listed) for its carry-out to succeed.
    async fn publish_first_boot_pending(&self) {
        let pending = match &*self
            .first_boot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            FirstBootRows::Pending { rows, .. } => rows.values().cloned().collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let published = {
            let entries = self.entries.read().await;
            pending
                .iter()
                .map(|row| FirstBootPendingRow {
                    identity: row.identity().clone(),
                    kind: row.kind(),
                    waiting_for: if entries.contains_key(row.identity()) {
                        FirstBootWait::CarryOut
                    } else {
                        FirstBootWait::Roster
                    },
                })
                .collect::<Vec<_>>()
        };
        self.first_boot_status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone_from(&published);
        self.modify_bootstrap_status(None, |status| status.first_boot_pending = published);
    }

    /// Record a FINAL or HELD wake refusal for the views,
    /// until the next successful wake. Process-local; never read as truth.
    async fn record_wake_refusal(&self, identity: &AgentIdentity, error: &IdentityRuntimeError) {
        let Some(refusal) = wake_refusal_of(error) else {
            return;
        };
        let is_final = refusal.is_final();
        if let Some(entry) = self.entries.write().await.get_mut(identity) {
            entry.wake_refusal = Some(refusal);
        }
        if is_final {
            self.emit_event(
                identity,
                IdentityEvent::StateChanged {
                    identity: identity.clone(),
                    new_state: IdentityLifecycleState::Broken,
                },
            )
            .await;
        }
    }

    /// A lifecycle verb is about to act on a member Core holds live (the
    /// caller holds the identity's lifecycle lock and just read it live).
    /// Neither a recorded wake refusal nor the last wake's replaced-session
    /// note describes that member, so both go before Core is asked: no read
    /// between the verb's Core steps (retire, then wake or spawn) shows the
    /// identity Broken or names a replaced session.
    async fn clear_wake_notes_of_live_member(&self, identity: &AgentIdentity) {
        if let Some(entry) = self.entries.write().await.get_mut(identity) {
            entry.wake_refusal = None;
            entry.replaced_session = None;
        }
    }

    /// A wake (or a fresh life) succeeded: drop any recorded refusal, wake
    /// the binding observer and the completion waiters, and announce the
    /// view's Active state.
    async fn settle_successful_wake(
        &self,
        identity: &AgentIdentity,
        replaced_session: Option<super::types::ReplacedSession>,
    ) {
        // The replaced-session marker is written with the settle, before the
        // binding notify and the Active event, so a reader woken by either
        // already sees it. A new member ends any absent-session note.
        if let Some(entry) = self.entries.write().await.get_mut(identity) {
            entry.wake_refusal = None;
            entry.replaced_session = replaced_session;
            entry.absent_delivery_session = None;
        }
        self.clear_materialization_backoff(identity).await;
        self.entries.notify_binding_changed();
        self.emit_event(
            identity,
            IdentityEvent::StateChanged {
                identity: identity.clone(),
                new_state: IdentityLifecycleState::Active,
            },
        )
        .await;
    }

    /// Whether this runtime runs the live member Core's binding names: not
    /// once it began shutting down, though Core keeps the binding live for
    /// the next start.
    fn runs_live_member(&self, core_live: bool) -> bool {
        core_live && !self.foreground_shutdown.load(Ordering::Acquire)
    }

    /// The view state Core's facts give an identity right now.
    fn view_state(
        live: bool,
        wake_refusal: Option<&super::types::WakeRefusal>,
    ) -> IdentityLifecycleState {
        if live {
            IdentityLifecycleState::Active
        } else if wake_refusal.is_some_and(super::types::WakeRefusal::is_final) {
            IdentityLifecycleState::Broken
        } else {
            IdentityLifecycleState::Dormant
        }
    }

    /// [`Self::embody_identity`] for a caller that ALREADY holds this
    /// identity's lifecycle lock (the reload verb runs retire and wake under
    /// one lock so no send can interleave). The per-identity tokio mutex is
    /// not reentrant, so the body is split rather than re-acquired.
    async fn embody_identity_locked(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        cancellation: Option<&mut watch::Receiver<bool>>,
        expected_bootstrap_generation: Option<u64>,
        bound_bootstrap_generation: &mut Option<u64>,
        overrides: EmbodimentOverrides<'_>,
    ) -> Result<EmbodimentOutcome, IdentityRuntimeError> {
        let result = self
            .embody_identity_transaction(
                identity,
                expected_alias,
                cancellation,
                expected_bootstrap_generation,
                bound_bootstrap_generation,
                overrides,
            )
            .await;
        match &result {
            Ok(outcome) if !outcome.already_active => {
                let replaced = outcome.replaced_session.clone().map(|session_id| {
                    super::types::ReplacedSession {
                        session_id,
                        reason: super::types::ReplacedSessionReason::Absent,
                    }
                });
                self.settle_successful_wake(identity, replaced).await;
            }
            Ok(_) => {
                if let Some(entry) = self.entries.write().await.get_mut(identity) {
                    entry.wake_refusal = None;
                }
            }
            Err(error) => self.record_wake_refusal(identity, error).await,
        }
        result
    }

    /// Whether Core runs `identity`'s live member under a role other than
    /// the roster profile `spec` names. Only an identity the history path
    /// would resume is compared: a first-boot `Resume` row pins its own
    /// session, and a peer-only identity always starts fresh.
    async fn live_role_moved_off_profile(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        first_boot_row: bool,
    ) -> Result<bool, IdentityRuntimeError> {
        if first_boot_row || durable_spec_uses_external_binding(spec) {
            return Ok(false);
        }
        Ok(self
            .core_last_binding(identity)
            .await?
            .is_some_and(|binding| binding.role() != &spec.profile))
    }

    /// The one wake door: eager boot, lazy foreground wakes, background
    /// warms and reloads all come here. Meta decides nothing Core owns:
    /// - Core holds the member: make sure its live materialization exists
    ///   (Core's own restore at boot, or an idle executor teardown); nothing
    ///   is built or spawned.
    /// - Otherwise Core's member history decides. The identity's last
    ///   binding (since the latest mob reset) is resumed onto its
    ///   committed session; no history, or a last session Core never
    ///   persisted, is a fresh spawn. Core's durable refusal of the last
    ///   session is typed `IdentityUnresumable`, never a silent fresh
    ///   spawn; a held resume, or a profile change the host did not declare
    ///   as a migration, is typed `WakeHeld`; a transient fault is
    ///   retryable. One attempt, no retry loop.
    async fn embody_identity_transaction(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        mut cancellation: Option<&mut watch::Receiver<bool>>,
        expected_bootstrap_generation: Option<u64>,
        bound_bootstrap_generation: &mut Option<u64>,
        overrides: EmbodimentOverrides<'_>,
    ) -> Result<EmbodimentOutcome, IdentityRuntimeError> {
        let bootstrap_generation = {
            let entries = self.entries.read().await;
            entries
                .get(identity)
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?
                .bootstrap_generation
        };
        *bound_bootstrap_generation = Some(bootstrap_generation);
        // A background item belongs to one controller generation. If a newer
        // roster pass converged the entry before this item acquired lifecycle
        // authority, treat the old work as cancelled instead of waking the
        // newer spec on behalf of a superseded pass.
        if expected_bootstrap_generation.is_some_and(|expected| expected != bootstrap_generation) {
            return Err(IdentityRuntimeError::Internal(
                BACKGROUND_WARM_CANCELLED.to_string(),
            ));
        }
        let raw_alias_lock = self.raw_member_alias_lock(identity.as_str()).await;
        let _raw_alias_guard = raw_alias_lock.lock().await;
        self.ensure_raw_member_alias_available(identity).await?;
        if let Some(expected_alias) = expected_alias {
            self.ensure_expected_member_alias_current(identity, expected_alias)
                .await?;
        }
        let lock = self.materialization_lock_for(identity).await;
        let _guard = lock.lock().await;

        let spec = {
            let entries = self.entries.read().await;
            let entry = entries
                .get(identity)
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
            overrides
                .spec
                .cloned()
                .unwrap_or_else(|| entry.spec.clone())
        };
        // A send authored from another registered row wakes nothing: this is
        // the lifecycle-owned wake decision, under the identity's lifecycle
        // lock, before readiness bookkeeping or build.
        refuse_unless_registered_row(identity, &spec, overrides.expected_spec_digest)?;
        // Before anything acts: a host-adopted identity is read-only.
        self.refuse_host_adopted(identity).await?;
        let bridge = Arc::clone(self.core_bridge(identity)?);
        // The first-boot manifest: the identity's pending row is
        // carried out before anything wakes it. A `Fresh` row starts its
        // fresh life here, and the wake resumes that; a `Resume` row pins
        // the wake to the row's session.
        let first_boot_resume = self.first_boot_step_locked(identity).await?;
        // A `Resume` row names a session the migration carried into Core's
        // session store. Core's session state is read before any resume, so
        // a session Core holds no durable copy of is refused typed and never
        // reaches a resume that could start a fresh life in its place.
        if let Some(session_id) = first_boot_resume.as_ref() {
            match bridge.durable_session_held(session_id).await {
                Ok(true) => {}
                Ok(false) => {
                    return Err(IdentityRuntimeError::FirstBootRowRefused {
                        identity: identity.clone(),
                        kind: FirstBootRowKind::Resume,
                        manifest_session: session_id.clone(),
                        core_session: None,
                        detail: format!("Core holds no durable copy of session {session_id}"),
                    });
                }
                Err(error) => {
                    return Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                        identity: identity.clone(),
                        detail: format!(
                            "reading Core's session state for the identity's first-boot row: \
                             {error}"
                        ),
                    });
                }
            }
        }

        // Core holds the member: revive its live materialization if needed,
        // while Core runs it under the roster's profile. The mob's own
        // restore at boot keeps a member's durable role, so reviving a member
        // that runs another role would make the roster's profile, and any
        // migration this activation declared, a silent no-op. Such a member
        // goes to the history path below, which applies a declared migration
        // (the bridge restamps the live member) or refuses the change typed
        // before anything is retired.
        if let Some(live) = self.core_live_member(identity).await?
            && !self
                .live_role_moved_off_profile(identity, &spec, first_boot_resume.is_some())
                .await?
        {
            match bridge.ensure_member_live(identity).await {
                Ok(Some(binding)) => {
                    self.refuse_unprojected_session(identity, &binding).await?;
                    self.publish_pending_customizer_tools(
                        identity,
                        binding.session_id.as_ref(),
                        "wake",
                    )
                    .await?;
                    return Ok(EmbodimentOutcome {
                        override_after_create: None,
                        already_active: true,
                        binding,
                        resumed: true,
                        draft: Self::base_build_draft(&spec),
                        replaced_session: None,
                    });
                }
                // The member left between the read and the revive: wake it
                // from its history below.
                Ok(None) => {}
                Err(error) => {
                    return Err(classify_wake_failure(
                        identity,
                        live.session_id.as_ref(),
                        error,
                    ));
                }
            }
        }

        // A member a held pass could not publish owes its host a fresh held
        // completion. Any wake outside a held pass that covers it would
        // publish a new instance that completion never saw, so it is refused
        // typed. The durable marks are read first, so a restart keeps them.
        self.load_held_completion_marks().await?;
        if self.held_completion_owed(identity)
            && self
                .held_create_pass()
                .is_none_or(|pass| !pass.covers(identity))
        {
            return Err(IdentityRuntimeError::HeldCompletionOwed {
                identity: identity.clone(),
            });
        }

        // What Core remembers of the identity. A peer-only (externally bound)
        // identity has no session to resume and always starts fresh. A
        // first-boot `Resume` row's identity has no binding yet.
        let last = if first_boot_resume.is_some() || durable_spec_uses_external_binding(&spec) {
            None
        } else {
            // A last binding no identity-first spawn made (a member the host
            // made directly on the mob under the identity's member id, since
            // retired) is not the identity's history: the wake starts fresh
            // rather than hand that member's transcript to the identity.
            self.core_last_binding(identity)
                .await?
                .filter(|binding| binding_spawned_for_identity(binding, identity))
                .and_then(|binding| {
                    binding
                        .session_id()
                        .cloned()
                        .map(|session_id| (session_id, binding.role().clone()))
                })
        };
        // Meta never infers a role migration. A resume
        // declares one only when the roster profile moved off the role Core
        // last ran the identity under AND this activation's host declared
        // exactly that migration; any other profile change is refused typed
        // before Core is asked.
        let resume = match last {
            Some((session_id, last_role)) if last_role != spec.profile => {
                let declared = bridge.declared_role_migration(identity);
                if declared.as_ref() != Some(&last_role) {
                    let detail = match declared {
                        // A declaration from another predecessor does not
                        // describe this member.
                        Some(from_role) => format!(
                            "declared role migration of {identity} from '{from_role}' to '{}' \
                             was not applied: the role '{last_role}' Core last ran session \
                             {session_id} under is not the declared predecessor role",
                            spec.profile
                        ),
                        None => format!(
                            "the roster profile '{}' differs from the role '{last_role}' Core \
                             last ran session {session_id} under, and this activation declared \
                             no migration from it",
                            spec.profile
                        ),
                    };
                    return Err(IdentityRuntimeError::WakeHeld {
                        identity: identity.clone(),
                        hold: WakeHold::RoleMigrationNotApplied,
                        detail,
                        session_id,
                    });
                }
                Some((session_id, Some(last_role)))
            }
            Some((session_id, _)) => Some((session_id, None)),
            None => first_boot_resume
                .clone()
                .map(|session_id| (session_id, None)),
        };
        // With no binding to resume this embodiment creates the identity's
        // member. One the console would attribute to a runtime-plane
        // identity is refused like every other creation path, before the
        // readiness bookkeeping, the build customizer or Core sees it. Like
        // the host's own build refusal it is answered typed on every attempt
        // and never recorded, so a later wake refuses again and builds
        // nothing. An identity Core holds a binding for resumes.
        if resume.is_none() {
            refuse_runtime_plane_creation(identity)?;
        }
        // A candidate launch wakes no identity here: a wake that reaches this
        // point spawns a member, which Core journals in the shared mob store.
        // It creates no member, moves none to another role, starts no fresh
        // session in place of a lost one and spawns none onto the stored
        // session Core still holds: the roster is the authoritative launch's
        // to change. The members Core restored live at start took the live
        // path above and stay as they are, their runs held.
        // Each refusal defers the identity to the authoritative launch
        // (`candidate_deferral_of`); a session read that fails is the
        // identity's own failure.
        if self.is_candidate_launch() {
            let change = match &resume {
                None => Some(("create the member", CANDIDATE_CREATE_MEMBER)),
                Some((_, Some(_))) => {
                    Some(("move the member to another role", CANDIDATE_MOVE_ROLE))
                }
                Some((session_id, None)) => match bridge.durable_session_held(session_id).await {
                    Ok(true) => Some((
                        "spawn a member onto its stored session",
                        CANDIDATE_RESUME_MEMBER,
                    )),
                    Ok(false) => Some((
                        "start a fresh session in place of the stored one",
                        CANDIDATE_FRESH_SESSION,
                    )),
                    Err(error) => {
                        return Err(IdentityRuntimeError::Internal(format!(
                            "candidate launch: whether Core holds the stored session \
                             {session_id} of {identity} could not be read: {error}"
                        )));
                    }
                },
            };
            if let Some((change, operation)) = change {
                tracing::warn!(
                    %identity,
                    change,
                    "candidate launch: the identity's wake would change the stored roster, so \
                     it is refused and the identity is deferred to the authoritative launch, \
                     which makes the change"
                );
                return Err(IdentityRuntimeError::InvalidState {
                    identity: identity.clone(),
                    state: IdentityLifecycleState::Dormant,
                    operation,
                });
            }
        }

        // Begin readiness bookkeeping only after ownership validation and
        // only for an identity that will actually wake.
        self.mark_bootstrap_materialization_started(identity, Some(bootstrap_generation));
        if cancellation
            .as_ref()
            .is_some_and(|cancellation| *cancellation.borrow())
        {
            return Err(IdentityRuntimeError::Internal(
                BACKGROUND_WARM_CANCELLED.to_string(),
            ));
        }

        let active_peers = self.entries.read().await.keys().cloned().collect();
        let managed_edges = self.desired_peer_edges.read().await.clone();
        let build_context = AgentBuildContext {
            identity: identity.clone(),
            active_peers,
            managed_edges,
            runtime_services: self.runtime_services(),
        };
        let mut draft = Self::base_build_draft(&spec);
        let installed_customizer = self.customizer.read().await.clone();
        let mut customized = false;
        if let Some(customizer) = overrides.customizer.or(installed_customizer.as_deref()) {
            let customize = customizer.customize_build(&build_context, &spec, &mut draft);
            tokio::pin!(customize);
            let customize_result = if let Some(cancellation) = cancellation.as_mut() {
                let cancellation = &mut **cancellation;
                tokio::select! {
                    result = &mut customize => result,
                    changed = cancellation.changed() => {
                        if changed.is_ok() && *cancellation.borrow() {
                            return Err(IdentityRuntimeError::Internal(
                                BACKGROUND_WARM_CANCELLED.to_string(),
                            ));
                        }
                        customize.await
                    }
                }
            } else {
                customize.await
            };
            if let Err(err) = customize_result {
                // A refused artifact keeps its type through the wake.
                if let super::types::CustomizerError::ToolArtifactRefused(refusal) = err {
                    return Err(IdentityRuntimeError::ToolArtifactRefused(refusal));
                }
                return Err(IdentityRuntimeError::Internal(format!("customizer: {err}")));
            }
            customized = true;
        }
        if customized {
            self.publish_customizer_tools(identity, &mut draft).await;
        }

        let mut resumed = None;
        let mut replaced_session = None;
        if let Some((session_id, resume_from_role)) = resume {
            match bridge
                .resume_session(
                    identity,
                    &spec,
                    &draft,
                    &session_id,
                    resume_from_role.as_ref(),
                )
                .await
            {
                Ok(outcome) => {
                    if let Some(reason) = outcome.fallback_reason() {
                        // Not for a first-boot row: Core's session state was
                        // read first. Should Core's resume still have started
                        // a fresh session, the row is refused, never settled.
                        if first_boot_resume.is_some() {
                            return Err(IdentityRuntimeError::FirstBootRowRefused {
                                identity: identity.clone(),
                                kind: FirstBootRowKind::Resume,
                                manifest_session: session_id.clone(),
                                core_session: Some(outcome.session_id().clone()),
                                detail: format!(
                                    "Core's resume started session {} in place of the row's \
                                     session ({reason:?})",
                                    outcome.session_id()
                                ),
                            });
                        }
                        // Only Core's own answer that it holds no durable
                        // copy (never persisted) may replace a session. Any
                        // other fallback would turn a session that exists
                        // into a silent fresh spawn: refused typed, nothing
                        // recorded.
                        if !matches!(
                            reason,
                            super::bridge::ResumeFallbackReason::NeverPersisted { .. }
                        ) {
                            return Err(IdentityRuntimeError::Internal(format!(
                                "identity {identity}: the bridge started session {} in place \
                                 of session {session_id} for a reason other than Core's \
                                 absent answer ({reason:?}); a session that exists is never \
                                 replaced",
                                outcome.session_id()
                            )));
                        }
                        tracing::warn!(
                            %identity,
                            %session_id,
                            reason = ?reason,
                            "identity wake fresh-spawned after a typed resume fallback"
                        );
                        replaced_session = Some(session_id.clone());
                    }
                    resumed = Some((
                        super::bridge::MemberRuntimeBinding {
                            agent_runtime_id: outcome.agent_runtime_id().clone(),
                            session_id: Some(outcome.session_id().clone()),
                        },
                        outcome.fallback_reason().is_none(),
                    ));
                }
                // A first-boot `Resume` row names a session the migration
                // carried into Core. Core holding none for it means Core's
                // session store is not the migrated one: refused typed,
                // never a fresh spawn.
                Err(error) if first_boot_resume.is_some() && resume_refused_absent(&error) => {
                    return Err(IdentityRuntimeError::FirstBootRowRefused {
                        identity: identity.clone(),
                        kind: FirstBootRowKind::Resume,
                        manifest_session: session_id.clone(),
                        core_session: None,
                        detail: format!("Core holds no durable session {session_id} ({error})"),
                    });
                }
                // Core holds no durable session for that id, so it was
                // never persisted and there is nothing to lose: spawn fresh.
                // The replaced session rides the outcome (A3), not just a log.
                Err(error) if resume_refused_absent(&error) => {
                    tracing::warn!(
                        %identity,
                        %session_id,
                        "Core holds no durable session for the identity's last session; the \
                         wake spawns a fresh one"
                    );
                    replaced_session = Some(session_id.clone());
                }
                // An untyped refusal on a runtime whose sessions live in
                // memory only: the session was never persisted and went with
                // its retired member, so the Absent rule above applies.
                Err(error)
                    if resume_refused_untyped(&error)
                        && matches!(
                            bridge.session_outlives_member(&session_id).await,
                            Ok(false)
                        ) =>
                {
                    tracing::warn!(
                        %identity,
                        %session_id,
                        %error,
                        "the runtime keeps sessions in memory only, so the identity's last \
                         session went with its retired member; the wake spawns a fresh one"
                    );
                    replaced_session = Some(session_id.clone());
                }
                Err(error) => {
                    return Err(classify_wake_failure(identity, Some(&session_id), error));
                }
            }
        }
        let (binding, resumed) = match resumed {
            Some(resumed) => resumed,
            None => {
                // Nothing was resumed (no binding to resume, or Core holds
                // no durable session for it), so this creates the member:
                // the same reservation applies.
                refuse_runtime_plane_creation(identity)?;
                let held_pass = self.held_create_pass().filter(|pass| pass.covers(identity));
                let created = match held_pass {
                    Some(pass) => {
                        bridge
                            .create_session_held(identity, &spec, &draft, &pass)
                            .await
                    }
                    None => bridge.create_session(identity, &spec, &draft).await,
                };
                match created {
                    Ok(binding) => (binding, false),
                    // Typed on every attempt, never recorded: the
                    // host's build gate answers each wake again.
                    Err(error) => {
                        return Err(match deterministic_build_rejection_reason(&error) {
                            Some(reason) => IdentityRuntimeError::HostRejectedBuild {
                                identity: identity.clone(),
                                reason,
                            },
                            None => IdentityRuntimeError::Internal(format!(
                                "bridge create_session: {error}"
                            )),
                        });
                    }
                }
            }
        };
        if first_boot_resume.is_some() {
            self.settle_first_boot_row(identity).await;
        }
        // The customizer that ran the build is told of its session with the
        // lifecycle lock released: the installed one in a runtime-owned
        // task, the caller's own once the caller has released the lock.
        let mut override_after_create = None;
        if customized && let Some(session_id) = binding.session_id.as_ref() {
            let notice = AfterCreateNotice {
                session_id: session_id.clone(),
                context: self.session_created_context(&spec, &draft),
            };
            match (overrides.customizer, installed_customizer) {
                (Some(_), _) => override_after_create = Some(notice),
                (None, Some(installed)) => {
                    self.spawn_after_create(installed, identity.clone(), notice)
                        .await;
                }
                (None, None) => {}
            }
        }
        Ok(EmbodimentOutcome {
            override_after_create,
            already_active: false,
            binding,
            resumed,
            draft,
            replaced_session,
        })
    }

    /// The build draft a spec starts from before any customizer runs.
    pub(crate) fn base_build_draft(spec: &DurableAgentSpec) -> AgentBuildDraft {
        AgentBuildDraft {
            model: None,
            system_prompt: None,
            additional_instructions: spec.additional_instructions.clone(),
            labels: spec.labels.clone(),
            app_context: spec.context.clone(),
            external_tools: Vec::new(),
            local_external_tools: Default::default(),
            provider_params: None,
            compaction_curator: Default::default(),
        }
    }

    /// What a build of `spec` with `draft` reports to `after_create`.
    fn session_created_context(
        &self,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
    ) -> crate::mob_handle_runtime::SessionCreatedContext {
        crate::mob_handle_runtime::SessionCreatedContext {
            model: self.built_model(spec, draft),
            labels: draft.labels.clone(),
            system_prompt: draft.system_prompt.clone(),
        }
    }

    /// CONTRACT-04's `after_create`: the customizer that ran a build is told
    /// of the session the build created or resumed. Best-effort: an error is
    /// logged at WARN and never fails or rolls back the build. Callers tell
    /// it with no lifecycle lock held, so it may call back into the identity.
    async fn tell_after_create(
        customizer: &dyn AgentCustomizer,
        identity: &AgentIdentity,
        notice: &AfterCreateNotice,
    ) {
        if let Err(error) = customizer
            .after_create(identity, &notice.session_id, &notice.context)
            .await
        {
            tracing::warn!(
                %identity,
                session_id = %notice.session_id,
                %error,
                "the agent customizer's after_create failed; the build stands"
            );
        }
    }

    /// Tell the installed `customizer` of a build's session in a
    /// runtime-owned task, so no lifecycle lock is held while it answers. A
    /// graceful shutdown abandons the call ([`Self::close_foreground_operations`])
    /// and joins the task ([`Self::join_after_create_notices`]).
    async fn spawn_after_create(
        &self,
        customizer: Arc<dyn AgentCustomizer>,
        identity: AgentIdentity,
        notice: AfterCreateNotice,
    ) {
        let mut shutdown = self.foreground_cancel.subscribe();
        let mut tasks = self.after_create_tasks.lock().await;
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result {
                tracing::error!(%error, "an after_create task panicked");
            }
        }
        tasks.spawn(async move {
            tokio::select! {
                () = Self::tell_after_create(customizer.as_ref(), &identity, &notice) => {}
                _ = shutdown.wait_for(|shutting_down| *shutting_down) => {
                    tracing::debug!(
                        %identity,
                        session_id = %notice.session_id,
                        "the runtime shut down before the agent customizer's after_create \
                         answered"
                    );
                }
            }
        });
    }

    /// Join every `after_create` task once foreground lifecycle admission is
    /// closed, which abandons the calls still waiting for their customizer.
    pub(crate) async fn join_after_create_notices(&self) {
        let mut tasks = self.after_create_tasks.lock().await;
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                tracing::error!(%error, "an after_create task panicked during shutdown");
            }
        }
    }

    /// The model a build of `spec` with `draft` runs: the draft's model, else
    /// the spec's inline profile model, else empty when neither is known.
    fn built_model(&self, spec: &DurableAgentSpec, draft: &AgentBuildDraft) -> String {
        if let Some(model) = draft.model.as_ref() {
            return model.clone();
        }
        self.reset_roster_source
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|source| source.mob_definition.as_ref())
            .and_then(|definition| definition.profiles.get(&spec.profile))
            .and_then(|binding| binding.as_inline())
            .map(|profile| profile.model.clone())
            .unwrap_or_default()
    }

    /// A retired identity (Core's last binding is Retired and Core holds no
    /// member) is revived ONLY by an operation addressed to it (`send`,
    /// `dispatch`, `materialize`, `reload_member`) or by the eager boot
    /// compile. Indirect hydration (a peer hand-off's
    /// `materialize_reachable_peers`, fleet `materialize_all`, the background
    /// warm) must not silently bring a retired member back; it is refused
    /// typed so the initiator's cycle completes without the retired peer. A
    /// failed history read refuses too, typed: indirect hydration never
    /// guesses.
    async fn retired_terminal_refusal(
        &self,
        identity: &AgentIdentity,
        operation: &'static str,
    ) -> Option<IdentityRuntimeError> {
        self.bridge.as_ref()?;
        match self.core_last_binding(identity).await {
            Ok(Some(binding)) if binding.state() == meerkat_mob::MemberBindingState::Retired => {
                Some(IdentityRuntimeError::InvalidState {
                    identity: identity.clone(),
                    state: IdentityLifecycleState::Dormant,
                    operation,
                })
            }
            Ok(_) => None,
            Err(error) => Some(error),
        }
    }

    async fn best_effort_materialize_identity(
        &self,
        identity: AgentIdentity,
        initiator: Option<&AgentIdentity>,
        operation: &'static str,
    ) -> Option<meerkat_mob::MemberDeliveryScope> {
        let attempt_lock = self.best_effort_materialization_lock_for(&identity).await;
        let _attempt_guard = attempt_lock.lock().await;

        if let Some(err) = self
            .retired_terminal_refusal(&identity, "materialize")
            .await
        {
            tracing::warn!(
                identity = %identity,
                initiator = initiator.map(ToString::to_string).as_deref(),
                error = %err,
                "identity best-effort materialization refused: the identity is retired; only a \
                 send addressed to it re-materializes it"
            );
            self.record_best_effort_materialization_failure(&identity, initiator, operation, &err)
                .await;
            return None;
        }

        if let Some(error) = self.materialization_backoff_error(&identity).await {
            tracing::debug!(
                identity = %identity,
                initiator = initiator.map(ToString::to_string).as_deref(),
                error = %error,
                "identity best-effort materialization skipped due to materialization backoff"
            );
            return None;
        }

        match self.materialize(&identity).await {
            Ok(scope) => {
                self.clear_materialization_backoff(&identity).await;
                scope
            }
            // A candidate launch defers a roster change to the authoritative
            // launch: no materialization failure to record.
            Err(err) if candidate_deferral_of(&err).is_some() => {
                tracing::info!(
                    identity = %identity,
                    initiator = initiator.map(ToString::to_string).as_deref(),
                    error = %err,
                    "identity best-effort materialization skipped: the candidate launch defers \
                     the identity to the authoritative launch"
                );
                None
            }
            Err(err) => {
                tracing::warn!(
                    identity = %identity,
                    initiator = initiator.map(ToString::to_string).as_deref(),
                    error = %err,
                    "identity best-effort materialization skipped identity after materialization failure"
                );
                self.record_best_effort_materialization_failure(
                    &identity, initiator, operation, &err,
                )
                .await;
                None
            }
        }
    }

    async fn materialize_all_records(
        &self,
    ) -> Vec<(
        AgentIdentity,
        Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError>,
    )> {
        let identities = self.registered_identities().await;
        stream::iter(identities.into_iter().map(|identity| async move {
            let result = self.materialize(&identity).await;
            (identity, result)
        }))
        .buffer_unordered(MANAGED_PEER_RECONCILE_CONCURRENCY)
        .collect::<Vec<_>>()
        .await
    }

    /// Wake all identities currently registered with the runtime, returning
    /// Core's delivery scope of each session-backed member woken.
    ///
    /// Fleet hydration is best-effort: one member that cannot build is skipped
    /// and surfaced through logs/error hooks rather than aborting unrelated
    /// members.
    pub async fn materialize_all(
        &self,
    ) -> Result<Vec<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        let identities = self.registered_identities().await;
        let records = stream::iter(identities.into_iter().map(|identity| async move {
            self.best_effort_materialize_identity(identity, None, "materialize_all")
                .await
        }))
        .buffer_unordered(MANAGED_PEER_RECONCILE_CONCURRENCY)
        .filter_map(async move |record| record)
        .collect::<Vec<_>>()
        .await;

        let desired_edges = self.desired_peer_edges.read().await.clone();
        if !desired_edges.is_empty()
            && let Err(err) = self.reconcile_managed_peer_edges(&desired_edges).await
        {
            tracing::warn!(
                error = %err,
                "identity materialize_all completed with topology reconcile warning"
            );
        }

        Ok(records)
    }

    /// Materialize all identities and fail if any registered identity cannot
    /// hydrate. Flow admission uses this strict variant so a run is not accepted
    /// with a partially materialized identity-first fleet.
    pub async fn materialize_all_required(
        &self,
    ) -> Result<Vec<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        let results = self.materialize_all_records().await;
        let mut records = Vec::with_capacity(results.len());
        let mut failures = Vec::new();

        for (identity, result) in results {
            match result {
                Ok(scope) => records.extend(scope),
                Err(err) => failures.push(format!("{identity}: {err}")),
            }
        }

        if !failures.is_empty() {
            return Err(IdentityRuntimeError::Internal(format!(
                "identity-first required materialization failed for {} identities: {}",
                failures.len(),
                failures.join("; ")
            )));
        }

        let desired_edges = self.desired_peer_edges.read().await.clone();
        if !desired_edges.is_empty() {
            self.reconcile_managed_peer_edges(&desired_edges).await?;
        }

        Ok(records)
    }

    /// [`Self::materialize_all_required_tracked`] for the flow-start
    /// provisioning callback: it runs inside an admitted flow run, which is
    /// entered native work, so it carries no owner of its own and acquires
    /// none.
    pub(crate) async fn materialize_all_required_for_entered_flow(
        self: &Arc<Self>,
    ) -> Result<Vec<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        self.run_tracked_foreground_with(ForegroundOwnerSource::Carried(None), async move {
            runtime.materialize_all_required().await
        })
        .await
    }

    /// Cancellation-safe strict fleet hydration for flow/request boundaries.
    pub async fn materialize_all_required_tracked(
        self: &Arc<Self>,
    ) -> Result<Vec<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        self.run_tracked_foreground(async move { runtime.materialize_all_required().await })
            .await
    }

    /// Ensure an active identity's desired peer neighborhood exists in the
    /// concrete mob graph before ordinary communication starts.
    pub async fn materialize_reachable_peers(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Vec<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        let peers = self.reachable_peer_identities(identity).await;
        let records = stream::iter(peers.into_iter().map(|peer| async move {
            self.best_effort_materialize_identity(
                peer,
                Some(identity),
                "materialize_reachable_peers",
            )
            .await
        }))
        .buffer_unordered(MANAGED_PEER_RECONCILE_CONCURRENCY)
        .filter_map(async move |record| record)
        .collect::<Vec<_>>()
        .await;

        let desired_edges = self.desired_peer_edges.read().await.clone();
        if !desired_edges.is_empty()
            && let Err(err) = self.reconcile_managed_peer_edges(&desired_edges).await
        {
            tracing::warn!(
                identity = %identity,
                error = %err,
                "identity peer materialization completed with topology reconcile warning"
            );
        }

        Ok(records)
    }

    // -----------------------------------------------------------------------
    // Subscribe - REQ-06
    // -----------------------------------------------------------------------

    /// Subscribe to identity-scoped events.
    ///
    /// Returns a broadcast receiver that yields `IdentityEvent` items for
    /// state changes, lease updates, lease loss, and checkpoint completions.
    pub async fn subscribe(
        &self,
        identity: &AgentIdentity,
    ) -> Result<broadcast::Receiver<IdentityEvent>, IdentityRuntimeError> {
        let channels = self.event_channels.read().await;
        let tx = channels
            .get(identity)
            .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
        Ok(tx.subscribe())
    }

    /// Update the registered row of an existing identity, under its
    /// lifecycle lock: a governed or row-checked send holding that lock
    /// delivers under the row it checked, and this update commits after it.
    pub async fn update_spec(&self, spec: DurableAgentSpec) -> Result<(), IdentityRuntimeError> {
        let lifecycle_lock = self.lifecycle_lock_for(&spec.identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        self.update_spec_locked(spec).await
    }

    /// [`Self::update_spec`]'s body for callers that already hold the
    /// identity's lifecycle lock (roster registration, reset adoption).
    pub(crate) async fn update_spec_locked(
        &self,
        spec: DurableAgentSpec,
    ) -> Result<(), IdentityRuntimeError> {
        let mut entries = self.entries.write().await;
        let entry = entries
            .get_mut(&spec.identity)
            .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(spec.identity.clone()))?;
        entry.spec = spec;
        Ok(())
    }

    async fn accept_bootstrap_spec_locked(
        &self,
        spec: DurableAgentSpec,
        generation: u64,
    ) -> Result<(), IdentityRuntimeError> {
        let mut entries = self.entries.write().await;
        let entry = entries
            .get_mut(&spec.identity)
            .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(spec.identity.clone()))?;
        entry.spec = spec;
        entry.bootstrap_generation = generation;
        Ok(())
    }

    /// Converge currently registered embodiments with a newly desired roster.
    ///
    /// Restore flows only walk the desired roster, so removal and replacement
    /// must happen first. Each transition shares the same per-identity lock as
    /// foreground materialization and lifecycle RPCs. Same-profile changes are
    /// REQ-33 hot reloads: publish the new registry metadata while preserving
    /// the exact live session and grant. Profile changes retire to Dormant
    /// before the new spec is installed; the selected eager/background/lazy
    /// policy then decides when to rebuild the physical member.
    ///
    /// Each member it retires because the roster no longer lists it is
    /// pushed to `retired` as soon as Core retired it, so the caller hands
    /// it to the host's retirement hook even when a later step of the pass
    /// fails.
    ///
    /// A candidate launch converges nothing: see
    /// [`Self::census_roster_changes_for_candidate`].
    async fn reconcile_roster_members(
        &self,
        roster: &[DurableAgentSpec],
        generation: u64,
        retired: &mut Vec<super::types::RetiredIdentityMember>,
    ) -> Result<(), IdentityRuntimeError> {
        if self.is_candidate_launch() {
            return self.census_roster_changes_for_candidate(roster).await;
        }
        self.reconcile_roster_members_after_lifecycle_lock(roster, generation, retired, |_| {
            std::future::ready(())
        })
        .await
    }

    /// A candidate launch's roster pass: it boots the members Core holds as
    /// the store has them and applies no roster change. It retires no member
    /// the roster no longer lists and no member whose profile the roster
    /// moved; it reads which ones a pass would change and logs them at WARN,
    /// with their count. The wake door refuses, typed, the other changes a
    /// pass would make: a member created fresh, or moved to another role.
    async fn census_roster_changes_for_candidate(
        &self,
        roster: &[DurableAgentSpec],
    ) -> Result<(), IdentityRuntimeError> {
        Self::validate_roster_uniqueness(roster)?;
        let desired = roster
            .iter()
            .map(|spec| (spec.identity.clone(), spec.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut unlisted = self
            .registered_identities()
            .await
            .into_iter()
            .filter(|identity| !desired.contains_key(identity))
            .collect::<BTreeSet<_>>();
        match self.core_unlisted_durable_members(&desired).await {
            Ok(members) => unlisted.extend(members),
            Err(error) => tracing::warn!(
                %error,
                "candidate launch: Core's durable identity members could not be read for the \
                 roster census"
            ),
        }
        unlisted.extend(self.core_unlisted_labelled_members(&desired).await);
        let mut would_retire = Vec::new();
        for identity in unlisted {
            if self.compile_may_act_on(&identity).await.unwrap_or(false)
                && matches!(self.core_live_member(&identity).await, Ok(Some(_)))
            {
                would_retire.push(identity.to_string());
            }
        }
        let mut would_move = Vec::new();
        for (identity, spec) in &desired {
            let registered_profile = self
                .entries
                .read()
                .await
                .get(identity)
                .map(|entry| entry.spec.profile.clone());
            if registered_profile.is_some_and(|profile| profile != spec.profile) {
                would_move.push(identity.to_string());
            }
        }
        if would_retire.is_empty() && would_move.is_empty() {
            tracing::info!(
                listed = desired.len(),
                "candidate launch roster census: the roster pass would retire and move no member"
            );
        } else {
            tracing::warn!(
                listed = desired.len(),
                would_retire = would_retire.len(),
                would_move = would_move.len(),
                retire = %would_retire.join(", "),
                moved = %would_move.join(", "),
                "candidate launch roster census: the roster pass applies nothing; the \
                 authoritative launch would retire and move these members"
            );
        }
        Ok(())
    }

    /// Internal deterministic seam used by concurrency regressions. The hook
    /// runs with each identity's lifecycle lock held and before the entry is
    /// inspected or mutated.
    ///
    /// The roster compile's member half: an identity the roster no
    /// longer lists is retired on Core (Core keeps its history) and dropped
    /// here; a profile change retires the live member so the next wake
    /// resumes it under the new profile, declaring the role migration only
    /// when the host did; any other spec change is a metadata hot
    /// reload. Host-adopted identities are read-only: the compile never
    /// retires one.
    async fn reconcile_roster_members_after_lifecycle_lock<H, F>(
        &self,
        roster: &[DurableAgentSpec],
        generation: u64,
        retired: &mut Vec<super::types::RetiredIdentityMember>,
        mut after_lifecycle_lock: H,
    ) -> Result<(), IdentityRuntimeError>
    where
        H: FnMut(&AgentIdentity) -> F,
        F: Future<Output = ()>,
    {
        Self::validate_roster_uniqueness(roster)?;
        let desired = roster
            .iter()
            .map(|spec| (spec.identity.clone(), spec.clone()))
            .collect::<BTreeMap<_, _>>();
        let registered = self.registered_identities().await;

        for identity in registered
            .iter()
            .filter(|identity| !desired.contains_key(*identity))
        {
            let lifecycle_lock = self.lifecycle_lock_for(identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            after_lifecycle_lock(identity).await;
            if !self.entries.read().await.contains_key(identity) {
                continue;
            }
            retired.extend(self.retire_unlisted_locked(identity).await?);
            self.event_channels.write().await.remove(identity);
            self.entries.write().await.remove(identity);
        }

        // A live Core member the roster materialized (marked
        // `SpawnContinuityIntent::DurableIdentity`) that the roster no longer
        // lists is retired too, even when this process never registered it:
        // the roster dropped it while Meta was down. A durable member spawned
        // at runtime (mob tools, delegate, a helper) is never the roster's to
        // retire.
        for identity in self.core_unlisted_durable_members(&desired).await? {
            let lifecycle_lock = self.lifecycle_lock_for(&identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            after_lifecycle_lock(&identity).await;
            if self.entries.read().await.contains_key(&identity) {
                continue;
            }
            retired.extend(self.retire_unlisted_locked(&identity).await?);
        }

        // A live member Meta 0.9.3 spawned carries no marker, only the
        // identity-first label; one the roster no longer lists is retired
        // the same way. The label only ever retires, never grants.
        for identity in self.core_unlisted_labelled_members(&desired).await {
            let lifecycle_lock = self.lifecycle_lock_for(&identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            after_lifecycle_lock(&identity).await;
            if self.entries.read().await.contains_key(&identity) {
                continue;
            }
            match self.retire_unlisted_locked(&identity).await {
                Ok(retiring) => retired.extend(retiring),
                Err(error) => {
                    tracing::warn!(
                        %identity,
                        %error,
                        "retiring a labelled member the roster no longer lists failed; the \
                         next roster pass tries it again"
                    );
                }
            }
        }

        for (identity, desired_spec) in desired {
            let lifecycle_lock = self.lifecycle_lock_for(&identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            after_lifecycle_lock(&identity).await;
            // Initial bootstrap reaches this loop before the restore pass has
            // registered the desired identities.
            let current_spec = self
                .entries
                .read()
                .await
                .get(&identity)
                .map(|entry| entry.spec.clone());
            let Some(current_spec) = current_spec else {
                continue;
            };
            // REQ-33 and `compute_reconcile_actions` classify every
            // same-profile change (addressability, labels, display name,
            // context, and instructions) as a metadata hot reload: the live
            // member stays.
            if current_spec == desired_spec || current_spec.profile == desired_spec.profile {
                self.accept_bootstrap_spec_locked(desired_spec, generation)
                    .await?;
                continue;
            }
            // A reprofile retires the live member; the next wake resumes it
            // under the new profile (with the role migration declared
            // only when the host declared it, otherwise refused typed).
            if self.compile_may_act_on(&identity).await?
                && self.core_live_member(&identity).await?.is_some()
            {
                self.retire_locked(&identity).await?;
            }
            self.accept_bootstrap_spec_locked(desired_spec, generation)
                .await?;
        }

        Ok(())
    }

    /// Whether the roster compile may change `identity`'s Core member: only
    /// a command-driven identity. A host-adopted identity, or one whose
    /// intent row Core cannot read, is read-only and never retired by
    /// the compile.
    async fn compile_may_act_on(
        &self,
        identity: &AgentIdentity,
    ) -> Result<bool, IdentityRuntimeError> {
        if self.bridge.is_none() {
            return Ok(false);
        }
        Ok(matches!(
            self.core_identity_management(identity).await?,
            meerkat_mob::MemberIdentityManagement::CommandDriven
        ))
    }

    /// Retire an identity the roster no longer lists, under its lifecycle
    /// lock: Core's live member retires and Core keeps its history. Nothing
    /// to do when Core holds no live member or the identity is read-only to
    /// the compile. Returns the incarnation it retired when the host's
    /// retirement hook is installed.
    async fn retire_unlisted_locked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<super::types::RetiredIdentityMember>, IdentityRuntimeError> {
        if !self.compile_may_act_on(identity).await? {
            return Ok(None);
        }
        if self.core_live_member(identity).await?.is_none() {
            return Ok(None);
        }
        let mut retiring = self
            .retiring_incarnation(identity, super::types::RetiredIdentityCause::Unlisted)
            .await?;
        self.retire_locked(identity).await?;
        self.settle_retired_registration(&mut retiring).await;
        Ok(retiring)
    }

    /// Identities of Core's live Meta-marked members that the roster
    /// materialized and does not list. A member spawned at runtime with a
    /// durable profile carries the marker too, but not the roster's
    /// `agent_identity` label, so it is never listed here.
    async fn core_unlisted_durable_members(
        &self,
        desired: &BTreeMap<AgentIdentity, DurableAgentSpec>,
    ) -> Result<Vec<AgentIdentity>, IdentityRuntimeError> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(Vec::new());
        };
        let members = bridge.durable_identity_members().await.map_err(|error| {
            IdentityRuntimeError::Internal(format!(
                "the roster compile could not read Core's durable identity members: {error}"
            ))
        })?;
        let mut unlisted = Vec::new();
        for binding in members.iter().filter(|binding| {
            binding.is_live() && crate::member_comms_id::binding_materialized_by_roster(binding)
        }) {
            let durable = crate::member_comms_id::durable_identity_for_roster_member(
                binding.identity().as_str(),
            );
            let identity = AgentIdentity::parse(&durable).map_err(|error| {
                IdentityRuntimeError::Internal(format!(
                    "Core's durable identity member '{durable}' is not a valid identity: {error}"
                ))
            })?;
            if !desired.contains_key(&identity) {
                unlisted.push(identity);
            }
        }
        Ok(unlisted)
    }

    /// Identities of Core's members that carry the identity-first label
    /// naming the identity their member id encodes and that the roster does
    /// not list. A failed read lists none: the next roster pass reads again.
    async fn core_unlisted_labelled_members(
        &self,
        desired: &BTreeMap<AgentIdentity, DurableAgentSpec>,
    ) -> Vec<AgentIdentity> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Vec::new();
        };
        let members = match bridge.identity_labelled_members().await {
            Ok(members) => members,
            // A bridge without the read shows no such members.
            Err(BridgeError::Unsupported { .. }) => return Vec::new(),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "the roster pass could not read Core's identity-labelled members; none is \
                     retired, and the next pass reads them again"
                );
                return Vec::new();
            }
        };
        members
            .iter()
            .filter_map(|member| {
                AgentIdentity::parse(&crate::member_comms_id::durable_identity_for_roster_member(
                    member.as_str(),
                ))
                .ok()
            })
            .filter(|identity| !desired.contains_key(identity))
            .collect()
    }

    /// Adopt the roster's CURRENT spec for `identity` into the in-memory entry,
    /// so a subsequent [`reset`](Self::reset) rebuilds the regenerated session
    /// on the current profile instead of carrying the stored one forward.
    ///
    /// Best-effort: logs and leaves the stored spec in place if the roster
    /// can't be resolved or no longer lists the identity (reset's primary job —
    /// the destructive continuity reset — must not fail because the roster
    /// provider hiccuped). The runtime stays roster-agnostic; the provider is
    /// supplied by the caller (the reset RPC handler), which owns it.
    pub async fn adopt_roster_spec(
        &self,
        roster_provider: &Arc<dyn RosterProvider>,
        identity: &AgentIdentity,
    ) {
        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        if let Err(error) = self
            .adopt_roster_spec_with_context(roster_provider, identity, None)
            .await
        {
            tracing::warn!(
                identity = %identity,
                %error,
                "roster provider refused the current spec; keeping the stored spec unchanged",
            );
        }
    }

    async fn adopt_roster_spec_with_context(
        &self,
        roster_provider: &Arc<dyn RosterProvider>,
        identity: &AgentIdentity,
        mob_definition: Option<meerkat_mob::MobDefinition>,
    ) -> Result<(), IdentityRuntimeError> {
        match roster_provider
            .roster(&RosterContext {
                mob_definition,
                previous_identities: Vec::new(),
            })
            .await
        {
            Ok(specs) => {
                if let Some(spec) = specs.into_iter().find(|s| &s.identity == identity)
                    && let Err(err) = self.update_spec_locked(spec).await
                {
                    tracing::warn!(
                        identity = %identity,
                        error = %err,
                        "reset: failed to adopt current roster spec; rebuilding on stored spec",
                    );
                }
            }
            // An authoritative refusal (a prepared application's projection
            // failed) must not rebuild on the stored spec: that could
            // revive a member the application removed.
            Err(super::types::RosterError::Refused(detail)) => {
                return Err(IdentityRuntimeError::Internal(format!(
                    "roster provider refused the reset of {identity}: {detail}"
                )));
            }
            Err(err) => {
                tracing::warn!(
                    identity = %identity,
                    error = %err,
                    "reset: roster provider failed; rebuilding on stored spec",
                );
            }
        }
        Ok(())
    }

    /// Test only: drops an identity's entry and event channel without
    /// recording anything, the way a concurrent durable delete leaves the
    /// table. Outside tests an identity leaves only through its recorded
    /// retire and delete paths.
    #[cfg(test)]
    pub(crate) async fn remove_unrecorded_for_test(
        &self,
        identity: &AgentIdentity,
    ) -> Option<IdentityEntry> {
        self.event_channels.write().await.remove(identity);
        self.entries.write().await.remove(identity)
    }

    /// A member's agent event stream closed for good (its executor is gone).
    /// Meta repairs nothing: Core still holds the member, and the next
    /// wake or delivery revives it through Core (`ensure_member_live`). The
    /// completion waiters are woken so they re-read Core's fence now rather
    /// than at their deadline.
    pub(crate) fn note_member_stream_closed(&self, identity: &AgentIdentity, detail: &str) {
        tracing::info!(
            %identity,
            detail,
            "member event stream closed; Core revives the member on its next wake or delivery"
        );
        self.entries.notify();
    }

    // -----------------------------------------------------------------------
    // Lease checking (INV-01, INV-02)
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Delivery: send() - REQ-01, REQ-03
    // -----------------------------------------------------------------------

    /// Send conversational content and return only after the runtime has
    /// COMMITTED this exact turn's terminal boundary.
    ///
    /// Same body, same enforcement, same memory/defang/alias handling as
    /// [`Self::send`]; the single difference is the bridge verb. Use this when
    /// the caller needs PROOF the turn finished rather than proof it was
    /// admitted.
    ///
    /// Why this exists rather than "send, then watch the event stream": a
    /// session-wide `RunCompleted`/`RunFailed` cannot authorize a specific
    /// turn, because queued turns share a `session_id`. Waiting on one means
    /// some OTHER turn's terminal can satisfy the wait, so a test built that
    /// way can pass while the behaviour it claims to prove is broken. A timer
    /// is worse still: it elapses whether the turn succeeded, failed closed, or
    /// never ran at all.
    ///
    /// FAILS CLOSED. If no bridge is installed, or the identity has no bound
    /// runtime id, or the bridge is ingress-only, this returns a typed error.
    /// It never silently degrades to the admission-only path - that would
    /// report success for a turn whose outcome is unknown.
    pub async fn send_awaiting_commit(
        &self,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.send_awaiting_commit_with_mode(identity, content, HandlingMode::Queue)
            .await
    }

    /// [`Self::send_awaiting_commit`] with an explicit turn handling mode.
    pub async fn send_awaiting_commit_with_mode(
        &self,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.send_awaiting_commit_with_mode_and_interaction(identity, content, handling_mode, None)
            .await
    }

    /// [`Self::send_awaiting_commit`] carrying a host-minted interaction id, so
    /// the completed turn can be looked up EXACTLY afterwards by the identity
    /// the caller stamped on it - rather than by "the last request", by a
    /// request count, or by polling content, none of which name one turn.
    pub async fn send_awaiting_commit_with_mode_and_interaction(
        &self,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.send_core(
            identity,
            SendRequest {
                expected_alias: None,
                content,
                system_prompt: None,
                handling_mode,
                interaction_id,
                commit_mode: SendCommitMode::AwaitCommit,
                console_human: None,
                expected_spec_digest: None,
            },
        )
        .await
        .map(|outcome| outcome.token)
    }

    /// [`Self::send_awaiting_commit`] carrying one ordinary System message
    /// authored for THIS exact turn (meerkat 0.8.11 `WorkSpec::system_prompt`,
    /// appended at the turn's admitted transcript boundary).
    ///
    /// Per-turn content, never member or session configuration - the same
    /// carrier the ingress authored path uses, on the lane that can prove the
    /// turn finished.
    pub async fn send_awaiting_commit_with_system_prompt(
        &self,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
        system_prompt: Option<&str>,
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.send_core(
            identity,
            SendRequest {
                expected_alias: None,
                content,
                system_prompt,
                handling_mode,
                interaction_id,
                commit_mode: SendCommitMode::AwaitCommit,
                console_human: None,
                expected_spec_digest: None,
            },
        )
        .await
        .map(|outcome| outcome.token)
    }

    /// Send conversational content to an addressable identity.
    ///
    /// Enforces:
    /// - Identity must be registered and active
    /// - Identity must be Addressable (REQ-03)
    /// - Lease must be held (INV-01)
    /// - Lease must not be lost (INV-02)
    ///
    /// Returns the fencing token for the delivery (caller uses it for checkpoint).
    pub async fn send(
        &self,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.send_with_mode(identity, content, HandlingMode::Queue)
            .await
    }

    /// Cancellation-safe queue send for RPC/host request boundaries.
    pub async fn send_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.send_with_mode_tracked(identity, content, HandlingMode::Queue)
            .await
    }

    /// Send conversational content using an explicit turn handling mode.
    ///
    /// This is the identity-first counterpart to the mob member send path used
    /// by the console. Ordinary API callers can keep using [`Self::send`],
    /// which preserves queue semantics.
    pub async fn send_with_mode(
        &self,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.send_with_mode_and_interaction(identity, content, handling_mode, None)
            .await
    }

    /// Cancellation-safe send that also returns the completion baseline to
    /// wait past.
    ///
    /// This is the delivery entry point every surface should use when the
    /// caller intends to wait for the answer: it is the only one that hands
    /// back a [`CompletionCursor`] captured before delivery, which is what
    /// makes "wait for MY turn" expressible without comparing output text.
    /// `expected_alias` pins the delivery to a generated runtime alias the
    /// caller already resolved; pass `None` for the durable identity.
    pub async fn send_admission_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<SendAdmission, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.map(ToString::to_string);
        let content = content.clone();
        let interaction_id = interaction_id.map(ToString::to_string);
        self.run_tracked_foreground(async move {
            runtime
                .send_with_mode_and_interaction_with_expected_member_alias(
                    &identity,
                    expected_alias.as_deref(),
                    &content,
                    handling_mode,
                    interaction_id.as_deref(),
                )
                .await
                .map(|(fencing_token, completion_baseline)| SendAdmission {
                    fencing_token,
                    completion_baseline,
                })
        })
        .await
    }

    /// REQ-03: the send doors refuse an unknown or `InternalOnly` identity.
    async fn refuse_unless_addressable(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), IdentityRuntimeError> {
        let entries = self.entries.read().await;
        let entry = entries
            .get(identity)
            .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
        if entry.spec.addressability == AgentAddressability::InternalOnly {
            return Err(IdentityRuntimeError::NotAddressable(NotAddressable {
                identity: identity.clone(),
                addressability: entry.spec.addressability,
            }));
        }
        Ok(())
    }

    /// Cancellation-safe keyed send: `content` to `identity` as a member
    /// continuation of `mob` (the mob this runtime's identities live in),
    /// committed under the caller's `idempotency_key` in Core's continuation
    /// key ledger for the identity's roster member.
    ///
    /// The key belongs to the identity, not to a session or an incarnation:
    /// a replay answers the first commit's receipt (its delivery id and
    /// commit time) with the delivery's current status, after a respawn,
    /// reset, wake or gateway restart too, and other content under the key
    /// is refused. Keys never expire. The send runs the unkeyed send's own
    /// steps (the wake of a dormant identity, its lifecycle lock, the alias
    /// and registered-row checks, the live delivery target, and the delivery
    /// preparation, where ambient recall is skipped because a continuation
    /// carries no injected context), then commits under the lock instead of
    /// submitting. The drain admits the content into the member's session
    /// with the system origin.
    ///
    /// `handling_mode` is the continuation's handling, committed with it:
    /// `Queue` runs the content after the member's current turn, `Steer`
    /// delivers it into the running turn at its next boundary, and either
    /// starts a turn on an idle member. The mode is part of what the key
    /// commits, so a replay under the key in another mode is refused as
    /// other content is.
    ///
    /// # Errors
    /// [`IdentityRuntimeError::KeyedSendRefused`] with Core's typed refusal,
    /// and the unkeyed send's refusals (an unknown or `InternalOnly`
    /// identity, a failed wake, a stale alias, an inactive member).
    pub(crate) async fn send_keyed_tracked(
        self: &Arc<Self>,
        mob: &crate::mob_handle_runtime::MobRuntime,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        idempotency_key: &str,
    ) -> Result<meerkat_contracts::WireContinuationReceipt, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let mob = mob.clone();
        let identity = identity.clone();
        let expected_alias = expected_alias.map(ToString::to_string);
        let content = content.clone();
        let idempotency_key = idempotency_key.to_string();
        self.run_tracked_foreground(async move {
            let receipt = runtime
                .send_keyed_admit(
                    &mob,
                    &identity,
                    expected_alias.as_deref(),
                    &content,
                    handling_mode,
                    &idempotency_key,
                )
                .await?;
            // As after any admission: the member's peers converge, best
            // effort, with no admission lock held.
            runtime
                .hydrate_peer_topology_after_admission(&identity)
                .await;
            Ok(receipt)
        })
        .await
    }

    /// The commit of [`Self::send_keyed_tracked`].
    async fn send_keyed_admit(
        &self,
        mob: &crate::mob_handle_runtime::MobRuntime,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        idempotency_key: &str,
    ) -> Result<meerkat_contracts::WireContinuationReceipt, IdentityRuntimeError> {
        self.refuse_unless_addressable(identity).await?;
        let member = crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let refused = |refusal: meerkat_mob::MemberSendIdempotencyRefusal| {
            IdentityRuntimeError::KeyedSendRefused {
                identity: identity.clone(),
                error: Box::new(meerkat_mob::MobError::MemberSendIdempotency {
                    identity: member.clone(),
                    refusal,
                }),
            }
        };
        // Refused before anything is woken: a key that is not canonical key
        // text (Core's own rule), or a host whose mob tools bind no
        // continuation service to keep it.
        meerkat_runtime::validate_canonical_key_text(
            "idempotency_key",
            idempotency_key,
            meerkat_runtime::CALLER_IDEMPOTENCY_KEY_MAX_BYTES,
        )
        .map_err(|error| {
            refused(meerkat_mob::MemberSendIdempotencyRefusal::InvalidKey {
                detail: error.to_string(),
            })
        })?;
        let Some(mob_tools) = mob.agent_mob_mcp_state() else {
            return Err(refused(
                meerkat_mob::MemberSendIdempotencyRefusal::NoContinuationService,
            ));
        };
        // Core holds no live member (dormant, or retired): wake it onto its
        // committed session through the one wake door, so the commit binds
        // to the incarnation that serves it.
        if self.core_live_member(identity).await?.is_none() {
            self.materialize_for_admission(identity, expected_alias, None)
                .await?;
        }
        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        if let Some(expected_alias) = expected_alias {
            self.ensure_expected_member_alias_current(identity, expected_alias)
                .await?;
        }
        {
            let entries = self.entries.read().await;
            let entry = entries
                .get(identity)
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
            refuse_unless_registered_row(identity, &entry.spec, None)?;
        }
        let (live, scope) = self.live_delivery_target(identity, "send").await?;
        let memory_session_key = live.session_id.as_ref().map(ToString::to_string);
        let (content_to_deliver, _) = self
            .prepare_member_delivery(
                identity,
                content,
                memory_session_key.as_deref(),
                scope.as_ref().map(|scope| scope.generation().get()),
                false,
                DeliveryPreparation::KeyedContinuation,
            )
            .await?;
        // Committed under the lifecycle lock: a retire, reset or row change
        // is ordered around a keyed send as around an unkeyed one. The
        // caller's handling is committed with the content, as Core keeps it.
        let mob_handle = mob.handle();
        mob_tools
            .mob_member_send_keyed(
                mob_handle.mob_id(),
                member.clone(),
                content_to_deliver,
                handling_mode,
                idempotency_key,
            )
            .await
            .map_err(|error| IdentityRuntimeError::KeyedSendRefused {
                identity: identity.clone(),
                error: Box::new(error),
            })
    }

    /// The state of an input in a session `identity` has had (its current
    /// session, or one Core's member history binds to it), from the
    /// runtime's own input ledger, projected as Core's `session/input_status`
    /// projects it: the read a keyed send's admitted pair is followed with.
    /// `state` is `None` when the session holds no such input.
    ///
    /// # Errors
    /// The send doors' identity refusals (an unknown or `InternalOnly`
    /// identity); [`IdentityRuntimeError::SessionNotHeldByIdentity`] for any
    /// other session; a failed history or ledger read.
    pub(crate) async fn send_status(
        &self,
        mob: &crate::mob_handle_runtime::MobRuntime,
        identity: &AgentIdentity,
        session_id: &SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<meerkat_contracts::SessionInputStateResult, IdentityRuntimeError> {
        use meerkat_runtime::SessionServiceRuntimeExt as _;

        self.refuse_unless_addressable(identity).await?;
        let current = self
            .core_live_member(identity)
            .await?
            .and_then(|live| live.session_id);
        if current.as_ref() != Some(session_id)
            && self.core_session_owner(session_id).await?.as_ref() != Some(identity)
        {
            return Err(IdentityRuntimeError::SessionNotHeldByIdentity {
                identity: identity.clone(),
                session_id: session_id.clone(),
            });
        }
        let machine = mob
            .session_service()
            .and_then(|sessions| {
                crate::mob_handle_runtime::observed_runtime_adapter(sessions.as_ref())
            })
            .ok_or_else(|| {
                IdentityRuntimeError::Internal(format!(
                    "identity '{identity}': this runtime has no input ledger to read"
                ))
            })?;
        let stored = machine
            .input_state(session_id, input_id)
            .await
            .map_err(|error| {
                IdentityRuntimeError::Internal(format!("input state read failed: {error}"))
            })?;
        let state = stored
            .map(meerkat::surface::wire_input_state)
            .transpose()
            .map_err(|error| {
                IdentityRuntimeError::Internal(format!("input state projection failed: {error}"))
            })?;
        Ok(meerkat_contracts::SessionInputStateResult { state })
    }

    /// Cancellation-safe send whose admitted turn is tracked by ticket.
    ///
    /// The same delivery as [`Self::send_admission_tracked`] (same admission,
    /// same interaction id, same session reconciliation, same completion
    /// baseline), returning once the work is admitted. The runtime keeps the
    /// admitted turn's own completion handle under a [`TurnTicket`] minted for
    /// this admission alone; [`Self::turn_outcome`] and
    /// [`Self::wait_for_turn`] then report THAT turn and its own output. No
    /// other delivery to the identity (a peer message, a scheduled turn, a
    /// fork completion wake, another send reusing the interaction id) can
    /// satisfy them.
    ///
    /// When the turn cannot be tracked ([`TurnUntrackable`]: a member whose
    /// LIVE runtime mode is `autonomous_host`, an externally bound member, a
    /// bridge that cannot report per-turn output), it is delivered exactly
    /// once, exactly like [`Self::send_admission_tracked`], and
    /// [`Ticketed::turn`] says why; only [`TurnUntrackable::NotDelivered`]
    /// (no bridge or no bound runtime) delivers nothing.
    pub async fn send_with_turn_ticket(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<Ticketed<SendAdmission>, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.map(ToString::to_string);
        let content = content.clone();
        let interaction_id = interaction_id.map(ToString::to_string);
        self.run_tracked_foreground(async move {
            let outcome = runtime
                .send_core(
                    &identity,
                    SendRequest {
                        expected_alias: expected_alias.as_deref(),
                        content: &content,
                        system_prompt: None,
                        handling_mode,
                        interaction_id: interaction_id.as_deref(),
                        commit_mode: SendCommitMode::TrackTurn,
                        console_human: None,
                        expected_spec_digest: None,
                    },
                )
                .await?;
            Ok(Ticketed {
                admission: SendAdmission {
                    fencing_token: outcome.token,
                    completion_baseline: outcome.completion_baseline,
                },
                turn: outcome.turn.unwrap_or_else(untracked_delivery),
            })
        })
        .await
    }

    /// [`Self::send_with_turn_ticket`] (`track_turn`) or
    /// [`Self::send_admission_tracked`] for an input the host authored from a
    /// registered row it read through [`Self::registered_spec`].
    ///
    /// The send compares the identity's registered row with
    /// `expected_spec_digest` in two separate acquisitions of the identity's
    /// lifecycle lock: in the embodiment decision of a non-Active identity,
    /// and again before the lease check, memory and delivery. A row that is
    /// stale at the embodiment decision does not wake the identity or take
    /// its lease. A row that was current there can still wake it, and a row
    /// change committing between the two acquisitions then refuses the
    /// submission at the final check. Either refusal is
    /// [`IdentityRuntimeError::RegisteredSpecChanged`] with nothing
    /// submitted; it does not prove that no earlier wake happened. A row
    /// change that commits after the final check waits behind the delivery,
    /// so a queued change alone never refuses it.
    pub async fn send_expecting_registered_spec(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        track_turn: bool,
        expected_spec_digest: &str,
    ) -> Result<(SendAdmission, Option<TurnTracking>), IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.map(ToString::to_string);
        let content = content.clone();
        let expected_spec_digest = expected_spec_digest.to_string();
        self.run_tracked_foreground(async move {
            let outcome = runtime
                .send_core(
                    &identity,
                    SendRequest {
                        expected_alias: expected_alias.as_deref(),
                        content: &content,
                        system_prompt: None,
                        handling_mode,
                        interaction_id: None,
                        commit_mode: if track_turn {
                            SendCommitMode::TrackTurn
                        } else {
                            SendCommitMode::Ingress
                        },
                        console_human: None,
                        expected_spec_digest: Some(&expected_spec_digest),
                    },
                )
                .await?;
            Ok((
                SendAdmission {
                    fencing_token: outcome.token,
                    completion_baseline: outcome.completion_baseline,
                },
                track_turn.then(|| outcome.turn.unwrap_or_else(untracked_delivery)),
            ))
        })
        .await
    }

    /// The identity's REGISTERED row and its digest (see [`RegisteredSpec`]).
    /// Informational: a send that names the digest re-checks it under the
    /// identity's lifecycle lock.
    pub async fn registered_spec(
        &self,
        identity: &AgentIdentity,
    ) -> Result<RegisteredSpec, IdentityRuntimeError> {
        let spec = self
            .entries
            .read()
            .await
            .get(identity)
            .map(|entry| entry.spec.clone())
            .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
        let spec_digest = registered_spec_digest(&spec).map_err(|error| {
            IdentityRuntimeError::Internal(format!(
                "registered row of {identity} has no canonical digest: {error}"
            ))
        })?;
        Ok(RegisteredSpec { spec, spec_digest })
    }

    /// The identities whose REGISTERED row names the profile `profile`, in
    /// identity order.
    pub async fn registered_with_profile(&self, profile: &str) -> Vec<AgentIdentity> {
        self.entries
            .read()
            .await
            .iter()
            .filter(|(_, entry)| entry.spec.profile.as_str() == profile)
            .map(|(identity, _)| identity.clone())
            .collect()
    }

    /// Governed counterpart of [`Self::send_with_turn_ticket`]: deliver one
    /// trusted governed input to `identity`'s member under the same identity
    /// lease, lifecycle lock, delivery recording and [`TurnTicket`].
    ///
    /// Under the lease Meta resolves the member's exact current local session
    /// and its session actor's actual pinned controller, and hands both to the
    /// trusted host's `binder` with the raw `input`. The binder returns the
    /// bound input with its host settlement custody. The bound input goes to
    /// the member's native admission unchanged; Meta never mints, alters,
    /// drops or retargets its association. Once submitted, the native answer
    /// is awaited to its definitive acknowledgement or refusal, inside the
    /// runtime-owned foreground operation, so neither an observation deadline
    /// nor the caller going away loses an admitted input.
    ///
    /// The custody is promoted ([`GovernedSettlement::accepted`]) only when
    /// the native acknowledgement says the input was ACCEPTED as new work; a
    /// native replay (deduplicated onto the canonical existing input)
    /// withdraws the retry's custody. It is dropped (withdrawn) when nothing
    /// was submitted or the member session's native admission settled its
    /// custody `NotAdmitted` (see [`super::bridge::GovernedAdmissionRecord`]),
    /// whatever the error. An `Uncertain` settlement, no observed
    /// settlement, or an `Admitted` one without an accepted acknowledgement
    /// for the bound session leaves admission unresolved: the custody goes to
    /// the binder's
    /// [`super::bridge::GovernedInputBinder::retain_unresolved`] from this
    /// runtime-owned operation, whether or not the caller is still waiting,
    /// and the error is
    /// [`super::bridge::GovernedInputError::AdmissionUnresolved`]. The
    /// admitted turn is tracked under its ticket before this returns; a
    /// promotion failure after admission is reported in the outcome, never
    /// as a refusal. Only a local session-backed member takes governed input,
    /// with no repair, retry or ungoverned fallback. Replay is the native
    /// replay of the bound input's event key.
    pub async fn send_governed_with_turn_ticket(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        input: meerkat_runtime::Input,
        binder: Arc<dyn super::bridge::GovernedInputBinder>,
    ) -> Result<GovernedSendOutcome, IdentityRuntimeError> {
        self.governed_with_turn_ticket(
            identity,
            expected_alias,
            input,
            binder,
            super::bridge::GovernedDoor::External,
        )
        .await
    }

    /// [`Self::send_governed_with_turn_ticket`] through the trusted host's
    /// internal door, for the host's own retained, already-bound input to a
    /// member it composed. Only the door differs: the member need not be
    /// externally addressable, and an `InternalOnly` identity is not
    /// refused. Everything else is the same: the lease, the session actor's
    /// pinned controller, the host binder, the bound-input check, the entry
    /// fence, the exact-session check, the definitive wait for native
    /// admission and the custody settlement. No wire method selects this
    /// door (the ordinary dispatch method is unchanged) and no caller
    /// chooses a door; external callers use
    /// [`Self::send_governed_with_turn_ticket`].
    pub async fn dispatch_governed_with_turn_ticket(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        input: meerkat_runtime::Input,
        binder: Arc<dyn super::bridge::GovernedInputBinder>,
    ) -> Result<GovernedSendOutcome, IdentityRuntimeError> {
        self.governed_with_turn_ticket(
            identity,
            expected_alias,
            input,
            binder,
            super::bridge::GovernedDoor::TrustedHostInternal,
        )
        .await
    }

    async fn governed_with_turn_ticket(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        input: meerkat_runtime::Input,
        binder: Arc<dyn super::bridge::GovernedInputBinder>,
        door: super::bridge::GovernedDoor,
    ) -> Result<GovernedSendOutcome, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.map(ToString::to_string);
        self.run_tracked_foreground(async move {
            let steer = input.handling_mode() == Some(HandlingMode::Steer);
            let result = runtime
                .send_governed_core(&identity, expected_alias.as_deref(), input, binder, door)
                .await;
            if !steer && input_was_admitted(&result) {
                runtime
                    .hydrate_peer_topology_after_admission(&identity)
                    .await;
            }
            let (outcome, admission, settlement) = result?;
            Ok(GovernedSendOutcome {
                ticketed: Ticketed {
                    admission: SendAdmission {
                        fencing_token: outcome.token,
                        completion_baseline: outcome.completion_baseline,
                    },
                    turn: outcome.turn.unwrap_or_else(untracked_delivery),
                },
                admission,
                settlement,
            })
        })
        .await
    }

    async fn send_governed_core(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        input: meerkat_runtime::Input,
        binder: Arc<dyn super::bridge::GovernedInputBinder>,
        door: super::bridge::GovernedDoor,
    ) -> Result<
        (
            SendCoreOutcome,
            super::bridge::GovernedAdmission,
            Result<(), String>,
        ),
        IdentityRuntimeError,
    > {
        use super::bridge::{GovernedDoor, GovernedInputError};
        {
            let entries = self.entries.read().await;
            let entry = entries
                .get(identity)
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
            // Only the external door refuses an internal-only identity; the
            // trusted host's internal door reaches any identity.
            if door == GovernedDoor::External
                && entry.spec.addressability == AgentAddressability::InternalOnly
            {
                return Err(IdentityRuntimeError::NotAddressable(NotAddressable {
                    identity: identity.clone(),
                    addressability: entry.spec.addressability,
                }));
            }
        }
        // Core holds no live member: wake it through the one wake door.
        if self.core_live_member(identity).await?.is_none() {
            self.materialize_for_admission(identity, expected_alias, None)
                .await?;
        }
        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        if let Some(expected_alias) = expected_alias {
            self.ensure_expected_member_alias_current(identity, expected_alias)
                .await?;
        }
        // The registered row is read with the Active/session check, under
        // the lifecycle lock this operation holds through native submission
        // and which every row writer takes: the binder checks the row the
        // input is admitted under.
        let spec = self
            .entries
            .read()
            .await
            .get(identity)
            .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?
            .spec
            .clone();
        let operation = match door {
            GovernedDoor::External => "send",
            GovernedDoor::TrustedHostInternal => "dispatch",
        };
        let (live, scope) = self.live_delivery_target(identity, operation).await?;
        let runtime_id = Some(live.agent_runtime_id);
        let session_id = live.session_id;
        let token = Self::scope_fence(scope.as_ref());
        let completion_baseline = self.credited_completion_cursor(identity, token).await;
        if durable_spec_uses_external_binding(&spec) {
            return Err(IdentityRuntimeError::GovernedInput(
                GovernedInputError::Unsupported(
                    "governed input requires a local session-backed member".to_string(),
                ),
            ));
        }
        let (Some(bridge), Some(runtime_id), Some(session_id)) =
            (self.bridge.as_ref(), runtime_id, session_id)
        else {
            return Err(IdentityRuntimeError::GovernedInput(
                GovernedInputError::Unsupported(
                    "no session bridge, bound runtime or session for the identity".to_string(),
                ),
            ));
        };
        // The session actor's actual controller, pinned by its owner.
        let controller = bridge
            .governed_controller(&runtime_id, &session_id)
            .await
            .map_err(|error| admission_phase_error(identity, error))?;
        let context = super::bridge::GovernedBindContext {
            session_id: session_id.clone(),
            controller,
            spec,
        };
        // The trusted host binds for exactly this session, under the lease.
        let super::bridge::BoundGovernedInput {
            input: bound,
            settlement,
            entry_fence,
        } = binder.bind(&context, input).await.map_err(|reason| {
            IdentityRuntimeError::GovernedInput(GovernedInputError::Bind(reason))
        })?;
        let header = bound.header();
        if header.authority_association.is_none() || header.ingress_context.is_none() {
            // Nothing was submitted: the custody is withdrawn.
            drop(settlement);
            return Err(IdentityRuntimeError::GovernedInput(
                GovernedInputError::Unbound,
            ));
        }
        // Custody whose submission's native admission is unresolved goes to
        // the host binder, the custody's owner, from this runtime-owned
        // operation: never left to a caller that may be gone, never dropped.
        let unresolved = |settlement: Box<dyn super::bridge::GovernedSettlement>,
                          detail: String| {
            binder.retain_unresolved(settlement, &detail);
            IdentityRuntimeError::GovernedInput(GovernedInputError::AdmissionUnresolved { detail })
        };
        // The member session's native admission settles this custody; the
        // host custody follows that settlement, never an error's kind.
        let native = super::bridge::GovernedAdmissionRecord::default();
        // The host's entry fence is entered at the final protected entry,
        // inside the bridge after its member lookup, immediately before
        // native admission; its use's outcome is reported below at the
        // settlement decision.
        let entry = super::bridge::GovernedEntryRecord::default();
        let custody = native.custody().with_entry_fence(entry_fence, &entry);
        let attempt = match door {
            GovernedDoor::External => {
                bridge
                    .begin_governed_delivery(&runtime_id, &session_id, bound, custody)
                    .await
            }
            GovernedDoor::TrustedHostInternal => {
                bridge
                    .begin_internal_governed_delivery(&runtime_id, &session_id, bound, custody)
                    .await
            }
        };
        let delivery = match attempt {
            Ok(delivery) => {
                self.record_delivery_success(identity).await;
                delivery
            }
            Err(error) => {
                let error = match (entry.refusal(), native.settlement()) {
                    // The fence refused the final entry: nothing was
                    // submitted, the custody is withdrawn.
                    (
                        Some(refusal),
                        Some(super::bridge::NativeInputAdmissionSettlement::NotAdmitted),
                    ) => {
                        drop(settlement);
                        IdentityRuntimeError::GovernedInput(GovernedInputError::EntryRefused(
                            refusal,
                        ))
                    }
                    // Native admission did not admit it: withdrawn.
                    (_, Some(super::bridge::NativeInputAdmissionSettlement::NotAdmitted)) => {
                        entry.finish(super::bridge::GovernedEntryOutcome::Unentered);
                        drop(settlement);
                        admission_phase_error(identity, error)
                    }
                    // Admitted, uncertain, or no settlement observed: the
                    // input may have run, and the custody stays with its
                    // owner.
                    (_, settled) => {
                        entry.finish(super::bridge::GovernedEntryOutcome::Uncertain);
                        unresolved(
                            settlement,
                            format!("native admission settled {settled:?}: {error}"),
                        )
                    }
                };
                self.record_delivery_error(identity, &error).await;
                return Err(error);
            }
        };
        let super::bridge::GovernedDelivery { receipt, admission } = delivery;
        let resolved_session = receipt.resolved_session().cloned();
        let resolution_error = receipt.session_resolution_error().map(ToString::to_string);
        // Admitted: the turn is tracked under its own ticket first, whatever
        // happens to the settlement.
        let ticket = self.track_turn(identity, None, receipt);
        let native_settlement = native.settlement();
        let admission = match admission {
            Some(admission)
                if native_settlement
                    == Some(super::bridge::NativeInputAdmissionSettlement::Admitted)
                    && admission.session_id == session_id =>
            {
                entry.finish(super::bridge::GovernedEntryOutcome::Entered);
                admission
            }
            admission => {
                entry.finish(super::bridge::GovernedEntryOutcome::Uncertain);
                let error = unresolved(
                    settlement,
                    format!(
                        "native admission answered ticket {ticket} with settlement \
                         {native_settlement:?} and acknowledgement {admission:?}, not an \
                         admission onto session {session_id}"
                    ),
                );
                self.record_delivery_error(identity, &error).await;
                return Err(error);
            }
        };
        // Promoted only for new work native admission accepted, here after
        // native admission returned and outside its guards; a native replay
        // reuses the canonical existing input and withdraws the retry's
        // custody. A promotion failure is the host's, after admission, and is
        // reported separately.
        let settled = if admission.deduplicated {
            drop(settlement);
            Ok(())
        } else {
            settlement.accepted(&admission)
        };
        // The turn was admitted onto the bound session; it is never
        // reconciled onto another one.
        match resolved_session {
            Some(resolved) if resolved == session_id => {}
            Some(resolved) => {
                self.report_ticketed_resolution_failure(
                    identity,
                    ticket,
                    &format!(
                        "the governed turn resolved session {resolved}, not its bound \
                         session {session_id}"
                    ),
                )
                .await;
            }
            None => {
                self.report_ticketed_resolution_failure(
                    identity,
                    ticket,
                    resolution_error
                        .as_deref()
                        .unwrap_or("no session was resolved"),
                )
                .await;
            }
        }
        Ok((
            SendCoreOutcome {
                token,
                completion_baseline,
                turn: Some(TurnTracking::Tracked(ticket)),
            },
            admission,
            settled,
        ))
    }

    /// Where the turn admitted under `ticket` for `identity` stands. A ticket
    /// of another identity, or one this runtime never admitted (or has
    /// forgotten), is [`TurnOutcome::Unknown`].
    pub fn turn_outcome(&self, identity: &AgentIdentity, ticket: TurnTicket) -> TurnOutcome {
        self.turn_outcomes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .outcome(identity, ticket)
    }

    /// Wait until the turn admitted under `ticket` settles, then return its
    /// outcome ([`TurnOutcome::Completed`] carries the turn's own output).
    /// Returns [`TurnOutcome::Unknown`] at once for an unknown ticket, and
    /// [`TurnOutcome::Pending`] if `timeout` elapses first or the runtime
    /// shuts down. See [`Self::wait_for_turn_outcome`] for why it ended.
    pub async fn wait_for_turn(
        &self,
        identity: &AgentIdentity,
        ticket: TurnTicket,
        timeout: Duration,
    ) -> TurnOutcome {
        match self.wait_for_turn_outcome(identity, ticket, timeout).await {
            TurnWait::Settled(outcome)
            | TurnWait::TimedOut(outcome)
            | TurnWait::ShuttingDown(outcome) => outcome,
        }
    }

    /// [`Self::wait_for_turn`], saying how the wait ended.
    ///
    /// Event-driven: the wait sleeps until the ticket registry changes (an
    /// admission or a settlement), or the runtime starts shutting down, and
    /// re-reads it then. `timeout` is only the caller's overall deadline.
    pub async fn wait_for_turn_outcome(
        &self,
        identity: &AgentIdentity,
        ticket: TurnTicket,
        timeout: Duration,
    ) -> TurnWait {
        let deadline = tokio::time::Instant::now() + timeout;
        // Subscribe before the first read so a settlement landing between the
        // read and the wait still wakes it.
        let mut changes = self.turn_outcome_changes.subscribe();
        let mut shutdown = self.foreground_waits_closed.subscribe();
        loop {
            let outcome = self.turn_outcome(identity, ticket);
            if outcome != TurnOutcome::Pending {
                return TurnWait::Settled(outcome);
            }
            if *shutdown.borrow_and_update() {
                return TurnWait::ShuttingDown(outcome);
            }
            tokio::select! {
                changed = changes.changed() => {
                    if changed.is_err() {
                        return TurnWait::ShuttingDown(self.turn_outcome(identity, ticket));
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() {
                        return TurnWait::ShuttingDown(self.turn_outcome(identity, ticket));
                    }
                }
                () = tokio::time::sleep_until(deadline) => {
                    return TurnWait::TimedOut(self.turn_outcome(identity, ticket));
                }
            }
        }
    }

    /// Record the admitted turn as pending under a freshly minted ticket and
    /// hand `receipt` to a waiter that settles THAT record with the turn's own
    /// outcome. The waiter holds only the outcome registry, never a lock or
    /// the runtime, and ends when the turn reaches its terminal (or its
    /// completion channel closes).
    ///
    /// The outcome is the turn's own, whatever post-admission session
    /// resolution did: a resolution failure is reported on the delivery-error
    /// channel by the caller ([`Self::report_ticketed_resolution_failure`]),
    /// never as a failure of a turn that ran and succeeded.
    fn track_turn(
        &self,
        identity: &AgentIdentity,
        delivery_key: Option<DeliveryKey>,
        receipt: super::bridge::BridgeTurnReceipt,
    ) -> TurnTicket {
        let ticket = self
            .turn_outcomes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .admit(identity, delivery_key);
        // An admission can evict the oldest pending turn at the cap.
        self.turn_outcome_changes.send_replace(());
        let outcomes = Arc::clone(&self.turn_outcomes);
        let changes = Arc::clone(&self.turn_outcome_changes);
        let owner = identity.clone();
        tokio::spawn(async move {
            let outcome = receipt.wait_turn_output().await;
            outcomes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .settle(&owner, ticket, outcome);
            changes.send_replace(());
        });
        ticket
    }

    /// The tracked admission a re-dispatch under `key` is deduplicated onto,
    /// if it is still pending or completed; see
    /// [`TurnOutcomes::reusable_delivery_ticket`].
    fn reusable_delivery_ticket(
        &self,
        identity: &AgentIdentity,
        key: &DeliveryKey,
    ) -> Option<TurnTicket> {
        self.turn_outcomes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reusable_delivery_ticket(identity, key)
    }

    /// A ticketed turn was admitted and is tracked, but projecting its session
    /// failed, so its session is not reconciled. The turn's own outcome is
    /// unaffected (see [`Self::track_turn`]); the resolution failure is
    /// recorded typed on the identity's delivery-error channel, where an
    /// operator reads it, instead of failing the admission or the turn.
    async fn report_ticketed_resolution_failure(
        &self,
        identity: &AgentIdentity,
        ticket: TurnTicket,
        detail: &str,
    ) {
        tracing::warn!(
            identity = %identity,
            ticket = %ticket,
            detail,
            "ticketed turn admitted but its session could not be resolved; the turn is \
             tracked, its session is not reconciled"
        );
        self.record_delivery_error(
            identity,
            &IdentityRuntimeError::PostAdmissionResolutionFailed {
                identity: identity.clone(),
                detail: format!(
                    "ticketed turn {ticket} was admitted and is tracked, but its session could \
                     not be resolved, so it was not reconciled: {detail}"
                ),
            },
        )
        .await;
    }

    /// Cancellation-safe explicit-mode send for RPC/host request boundaries.
    pub async fn send_with_mode_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.send_with_mode_and_interaction_tracked(identity, content, handling_mode, None)
            .await
    }

    /// Cancellation-safe interaction send for RPC/host request boundaries.
    pub async fn send_with_mode_and_interaction_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.send_admission_tracked(identity, None, content, handling_mode, interaction_id)
            .await
            .map(|admission| admission.fencing_token)
    }

    /// Cancellation-safe interaction send pinned to the generated runtime
    /// alias that the caller resolved. Validation and delivery share the
    /// identity lifecycle lock, so a concurrent reset cannot retarget the
    /// request onto the replacement generation.
    pub async fn send_with_mode_and_interaction_member_alias_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: &str,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.send_admission_tracked(
            identity,
            Some(expected_alias),
            content,
            handling_mode,
            interaction_id,
        )
        .await
        .map(|admission| admission.fencing_token)
    }

    /// Console-only dispatch, called ONCE by the owner of a fresh persisted
    /// console reservation. HTTP replays return that original frame/status or
    /// typed unknown fate after failure; they never re-prepare changed memory.
    /// No replay cache (or eviction which could make an old key executable).
    /// Why this gateway's runtime does not serve `identity`'s member, read
    /// from its registered spec: `None` for a local session-backed member,
    /// otherwise its remote placement or its external binding. A runtime
    /// inbox row for such a member is never applied here.
    pub(crate) async fn not_served_here(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<NotServedHere>, IdentityRuntimeError> {
        let entries = self.entries.read().await;
        let entry = entries
            .get(identity)
            .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
        Ok(if entry.spec.placement.is_some() {
            Some(NotServedHere::PlacedOnHost)
        } else if durable_spec_uses_external_binding(&entry.spec) {
            Some(NotServedHere::ExternalBinding)
        } else {
            None
        })
    }

    /// Whether a console send to `identity` can carry a native skill
    /// selection: only a local session-backed member (no external binding,
    /// no remote placement) has meerkat's host-human skill carrier. Read
    /// from the durable spec, so a console send can refuse a selection
    /// before it reserves an interaction.
    pub(crate) async fn carries_console_skill_selection(&self, identity: &AgentIdentity) -> bool {
        self.entries
            .read()
            .await
            .get(identity)
            .is_some_and(|entry| {
                !durable_spec_uses_external_binding(&entry.spec) && entry.spec.placement.is_none()
            })
    }

    /// Whether a Console human send to `identity` is admitted under its
    /// delivery identity, pinned to the session the send recorded: a local
    /// session-backed identity. An externally bound identity takes the
    /// external delivery, which carries no delivery identity, so a send to it
    /// must never be dispatched a second time.
    pub(crate) async fn console_human_delivery_is_keyed(&self, identity: &AgentIdentity) -> bool {
        self.entries
            .read()
            .await
            .get(identity)
            .is_some_and(|entry| !durable_spec_uses_external_binding(&entry.spec))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn send_console_human_input_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        skill_references: &[meerkat_core::skills::SkillKey],
        accepted: &crate::console_aggregator::ConsoleInteractionAccepted,
        governed: Option<Arc<dyn super::bridge::GovernedInputBinder>>,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        if let Some(binder) = governed {
            return self
                .send_console_governed_input(
                    identity,
                    expected_alias,
                    content,
                    handling_mode,
                    skill_references,
                    accepted,
                    binder,
                )
                .await;
        }
        let delivery_identity = meerkat_mob::MobDeliveryIdentity::new(
            accepted.input_frame_id.clone(),
            accepted.interaction_id.clone(),
        )
        .map_err(|error| {
            IdentityRuntimeError::HostHumanInput(super::bridge::HostHumanInputError::Mob(Box::new(
                error.into(),
            )))
        })?;
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.map(ToString::to_string);
        let content = content.clone();
        let skill_references = skill_references.to_vec();
        let interaction_id = accepted.interaction_id.clone();
        let expected_session = accepted.session_id.clone();
        self.run_tracked_foreground(async move {
            runtime
                .send_core(
                    &identity,
                    SendRequest {
                        expected_alias: expected_alias.as_deref(),
                        content: &content,
                        system_prompt: None,
                        handling_mode,
                        interaction_id: Some(&interaction_id),
                        commit_mode: SendCommitMode::Ingress,
                        console_human: Some(ConsoleHumanSend {
                            delivery_identity: &delivery_identity,
                            expected_session: expected_session.as_deref(),
                            skill_references: &skill_references,
                        }),
                        expected_spec_digest: None,
                    },
                )
                .await
                .map(|outcome| outcome.token)
        })
        .await
    }

    /// One Console send to an identity whose host governs its Console input:
    /// the raw prompt goes to native admission only through the host's
    /// `binder` ([`Self::send_governed_with_turn_ticket`]), keyed by the
    /// Console interaction so a replay is the native replay of the same
    /// input. It carries no skill selection.
    #[allow(clippy::too_many_arguments)]
    async fn send_console_governed_input(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        skill_references: &[meerkat_core::skills::SkillKey],
        accepted: &crate::console_aggregator::ConsoleInteractionAccepted,
        binder: Arc<dyn super::bridge::GovernedInputBinder>,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        if !skill_references.is_empty() {
            return Err(IdentityRuntimeError::GovernedInput(
                super::bridge::GovernedInputError::Unsupported(
                    "a governed Console send carries no skill selection".to_string(),
                ),
            ));
        }
        let mut prompt = meerkat_runtime::PromptInput::from_content_input(
            content.clone(),
            Some(
                meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                    handling_mode: Some(handling_mode),
                    ..Default::default()
                },
            ),
        );
        prompt.header.idempotency_key = Some(meerkat_runtime::IdempotencyKey::new(format!(
            "console:{}:{}",
            accepted.interaction_id, accepted.input_frame_id
        )));
        let outcome = self
            .send_governed_with_turn_ticket(
                identity,
                expected_alias,
                meerkat_runtime::Input::Prompt(prompt),
                binder,
            )
            .await?;
        if let Err(detail) = &outcome.settlement {
            // Admitted: the host's custody promotion failed after the fact,
            // which is the host's to reconcile, never a refusal of the send.
            tracing::warn!(
                identity = %identity,
                detail = %detail,
                "governed Console send was admitted but the host custody did not settle"
            );
        }
        Ok(outcome.ticketed.admission.fencing_token)
    }

    /// Converge an identity's reachable peer neighbourhood AFTER a queue-mode
    /// send or dispatch to it was admitted.
    ///
    /// Admission never waits for this: hydration materializes peers (member
    /// builds, host callbacks) and reconciles the managed topology (the
    /// topology guard, the reconcile lock and mob wiring calls), none of which
    /// the target's admission needs. meerkat already holds the admitted input
    /// durably and queues it behind any running turn. When the runtime's
    /// owning `Arc` is known the hydration is detached and runtime-tracked
    /// (shutdown joins it at its own commit/rollback boundary); otherwise it
    /// runs inline here, after admission and with no admission lock held.
    /// Best effort either way: a failure is logged, like the materialization
    /// warnings it wraps.
    ///
    /// Concurrent admissions to one target merge: while a hydration for it is
    /// in flight, a new admission only asks it to run once more when it
    /// finishes (so the latest topology is still converged), instead of
    /// stacking another hydration behind the same locks.
    async fn hydrate_peer_topology_after_admission(&self, identity: &AgentIdentity) {
        let Some(token) = self.claim_peer_hydration(identity) else {
            return;
        };
        if let Some(runtime) = self.shared_handle() {
            runtime.spawn_post_admission_peer_hydration(identity.clone(), token);
            return;
        }
        PeerHydrationSlot {
            hydrations: &self.peer_hydrations_in_flight,
            identity: identity.clone(),
            token,
        }
        .run(self)
        .await;
    }

    fn spawn_post_admission_peer_hydration(self: &Arc<Self>, identity: AgentIdentity, token: u64) {
        let runtime = Arc::clone(self);
        // D67: the admitted send owns this follow-up; it carries the send's
        // owner instead of acquiring one, so it never races the close.
        let carried = FOREGROUND_OWNER.try_with(Clone::clone).ok().flatten();
        tokio::spawn(async move {
            let hydrator = Arc::clone(&runtime);
            let owner = identity.clone();
            let _ = runtime
                .run_tracked_foreground_with(ForegroundOwnerSource::Carried(carried), async move {
                    PeerHydrationSlot {
                        hydrations: &hydrator.peer_hydrations_in_flight,
                        identity: owner,
                        token,
                    }
                    .run(&hydrator)
                    .await;
                    Ok::<(), IdentityRuntimeError>(())
                })
                .await
                .inspect_err(|error| {
                    // Refused at shutdown: release this claim (only this
                    // one) so nothing stays marked in flight.
                    runtime
                        .peer_hydrations_in_flight
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .release(&identity, token);
                    tracing::warn!(
                        identity = %identity,
                        %error,
                        "input was admitted; post-admission peer hydration was not run"
                    );
                });
        });
    }

    async fn hydrate_peer_topology_logged(&self, identity: &AgentIdentity) {
        if let Err(error) = self.hydrate_peer_topology(identity).await {
            tracing::warn!(
                identity = %identity,
                %error,
                "input was admitted; post-admission peer hydration failed"
            );
        }
    }

    /// Claim `identity`'s hydration slot (see [`PeerHydrations::claim`]).
    fn claim_peer_hydration(&self, identity: &AgentIdentity) -> Option<u64> {
        self.peer_hydrations_in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .claim(identity)
    }

    /// The topology work an ordinary send used to run before admission:
    /// materialize the reachable peers and reconcile the managed edges, plus
    /// the pending-recovery reconcile that a wake-path materialization
    /// ([`Self::materialize_for_admission`], which reconciles only the
    /// target's own edges) left to it.
    async fn hydrate_peer_topology(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), IdentityRuntimeError> {
        self.materialize_reachable_peers(identity).await?;
        let desired_edges = self.desired_peer_edges.read().await.clone();
        if desired_edges.is_empty()
            && let Some(controller) = self.topology_controller()
            && controller.has_pending().await
        {
            self.reconcile_managed_peer_edges(&desired_edges).await?;
        }
        Ok(())
    }

    /// [`Self::send_with_mode`] with a host-minted interaction id (meerkat
    /// 0.7.25 ask 15 addendum). The id rides `WorkSpec` into runtime
    /// admission, so the turn's live events and its committed transcript
    /// messages carry the same identity the console stamped on its frames -
    /// the exact live<->history join the console dedup needs. Only UUID-form
    /// ids thread; others are delivered without one.
    pub async fn send_with_mode_and_interaction(
        &self,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.send_with_mode_and_interaction_with_expected_member_alias(
            identity,
            None,
            content,
            handling_mode,
            interaction_id,
        )
        .await
        .map(|(token, _)| token)
    }

    async fn send_with_mode_and_interaction_with_expected_member_alias(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<(FencingToken, CompletionCursor), IdentityRuntimeError> {
        self.send_core(
            identity,
            SendRequest {
                expected_alias,
                content,
                system_prompt: None,
                handling_mode,
                interaction_id,
                commit_mode: SendCommitMode::Ingress,
                console_human: None,
                expected_spec_digest: None,
            },
        )
        .await
        .map(|outcome| (outcome.token, outcome.completion_baseline))
    }

    /// The ONE send body. Every lane (ingress, await-commit, ticketed) runs
    /// it; the only difference is which bridge verb the delivery step calls,
    /// which is exactly the part that must not drift. A second copy would let
    /// the completion lanes diverge from the ingress lane on lease
    /// acquisition, alias pinning, memory injection, defanging or session
    /// reconciliation - every one of which is load-bearing and none of which
    /// is visible from a test.
    async fn send_core(
        &self,
        identity: &AgentIdentity,
        request: SendRequest<'_>,
    ) -> Result<SendCoreOutcome, IdentityRuntimeError> {
        // Steers keep their established contract: no topology work at all
        // (background and full-fleet materialization own the peers).
        let hydrate = request.handling_mode != HandlingMode::Steer;
        let result = self.send_core_admit(identity, request).await;
        if hydrate && input_was_admitted(&result) {
            self.hydrate_peer_topology_after_admission(identity).await;
        }
        result
    }

    /// Admission for [`Self::send_core`]. For an already-Active target it
    /// waits only on the target's lifecycle lock, alias validation, its
    /// lease, the bounded memory recall and the bounded bridge admission: it
    /// never hydrates peers, reconciles topology or takes another identity's
    /// lifecycle lock, so a queued input gets meerkat's durable admission
    /// receipt while the member is mid-turn (meerkat queues it behind the
    /// running turn). A Dormant or retired target is woken first, and the
    /// wake reconciles that target's own managed edges, bounded, so its first
    /// turn can reach its peers (see [`Self::materialize_for_admission`]).
    async fn send_core_admit(
        &self,
        identity: &AgentIdentity,
        request: SendRequest<'_>,
    ) -> Result<SendCoreOutcome, IdentityRuntimeError> {
        let SendRequest {
            expected_alias,
            content,
            system_prompt,
            handling_mode,
            interaction_id,
            commit_mode,
            console_human,
            expected_spec_digest,
        } = request;
        self.refuse_unless_addressable(identity).await?;
        // Core holds no live member (dormant, or retired: the operator's
        // "retire then send" recovery path): wake it onto its committed
        // session through the one wake door.
        if self.core_live_member(identity).await?.is_none() {
            self.materialize_for_admission(identity, expected_alias, expected_spec_digest)
                .await?;
        }

        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        if let Some(expected_alias) = expected_alias {
            self.ensure_expected_member_alias_current(identity, expected_alias)
                .await?;
        }
        {
            let entries = self.entries.read().await;
            let entry = entries
                .get(identity)
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
            // Every row writer takes this lifecycle lock, which the send
            // holds through delivery: the row checked here is the row the
            // input is delivered under.
            refuse_unless_registered_row(identity, &entry.spec, expected_spec_digest)?;
        }
        let (live, scope) = self.live_delivery_target(identity, "send").await?;
        let mut token = Self::scope_fence(scope.as_ref());
        // Read the completion baseline BEFORE delivery is attempted. The turn
        // this send starts can only complete after this point, so a caller
        // waiting past the baseline cannot miss it. The converse ambiguity is
        // deliberate and documented: on an identity receiving concurrent
        // traffic, another delivery's completion can also satisfy the wait.
        // Waiting too little beats the failure this replaces (waiting forever).
        let completion_baseline = self.credited_completion_cursor(identity, token).await;
        let (
            runtime_id,
            memory_session_key,
            memory_generation,
            bridge_interaction_id,
            memory_runtime_mode,
            local_human,
            human_session,
            external_binding,
        ) = {
            let entries = self.entries.read().await;
            let entry = entries
                .get(identity)
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
            (
                Some(live.agent_runtime_id.clone()),
                // Scopes the injector's cross-turn dedup + cumulative budget.
                live.session_id.as_ref().map(ToString::to_string),
                scope.as_ref().map(|scope| scope.generation().get()),
                interaction_id_for_delivery(&entry.spec, interaction_id),
                // Resolved as meerkat resolves it at spawn (override, profile,
                // default). Generic autonomous work has no user context slot;
                // the explicit human boundary does.
                self.effective_runtime_mode(&entry.spec),
                console_human.is_some() && !durable_spec_uses_external_binding(&entry.spec),
                live.session_id.clone(),
                durable_spec_uses_external_binding(&entry.spec),
            )
        };
        if !local_human
            && console_human
                .as_ref()
                .is_some_and(|human| !human.skill_references.is_empty())
        {
            // An externally bound identity has no native member-turn skill
            // carrier; refuse before delivery instead of running the turn
            // without the selected skills.
            return Err(IdentityRuntimeError::HostHumanInput(
                super::bridge::HostHumanInputError::Unsupported(
                    "selected skills require a local session-backed member".to_string(),
                ),
            ));
        }
        if local_human
            && let Some(expected_session) = console_human
                .as_ref()
                .and_then(|human| human.expected_session)
            && human_session.as_ref().map(ToString::to_string).as_deref() != Some(expected_session)
        {
            return Err(IdentityRuntimeError::HostHumanInput(
                super::bridge::HostHumanInputError::BindingChanged,
            ));
        }
        let (content_to_deliver, injected_context) = self
            .prepare_member_delivery(
                identity,
                content,
                memory_session_key.as_deref(),
                memory_generation,
                handling_mode == HandlingMode::Steer,
                if local_human {
                    DeliveryPreparation::ConsoleHuman
                } else {
                    DeliveryPreparation::Work(memory_runtime_mode)
                },
            )
            .await?;

        // Deliver through the session bridge when available.
        //
        // AwaitCommit FAILS CLOSED here and nowhere else: if there is no bridge
        // or no runtime id, the ingress lane legitimately no-ops, but the
        // completion lane must NOT - silently taking the ingress path would
        // return success for a turn that was never awaited, which is the exact
        // false-success this API exists to remove.
        if (commit_mode == SendCommitMode::AwaitCommit || local_human)
            && (self.bridge.is_none() || runtime_id.is_none())
        {
            if local_human {
                return Err(IdentityRuntimeError::HostHumanInput(
                    super::bridge::HostHumanInputError::BindingChanged,
                ));
            }
            return Err(IdentityRuntimeError::CompletionUnavailable {
                identity: identity.clone(),
                reason: if self.bridge.is_none() {
                    "no session bridge is installed on this runtime".to_string()
                } else {
                    "the identity has no bound agent runtime id".to_string()
                },
            });
        }
        // Whether a tracked send can be tracked is decided here, from typed
        // state, before anything is delivered: a turn that cannot be tracked
        // is delivered on the ingress lane and reported as such, never
        // attempted on one lane and retried on the other.
        let mut turn_tracking = None;
        let lane = match commit_mode {
            SendCommitMode::Ingress => DeliveryLane::Ingress,
            SendCommitMode::AwaitCommit => DeliveryLane::AwaitCommit,
            SendCommitMode::TrackTurn => match turn_untrackable(
                self.bridge.is_some() && runtime_id.is_some(),
                external_binding,
                local_human,
                self.bridge
                    .as_ref()
                    .is_some_and(|bridge| bridge.tracks_turn_output()),
            ) {
                None => DeliveryLane::Ticketed,
                Some(reason) => {
                    turn_tracking = Some(TurnTracking::Unavailable(reason));
                    DeliveryLane::Ingress
                }
            },
        };
        if let (Some(bridge), Some(rid)) = (&self.bridge, &runtime_id) {
            // Validated -> Admitted(receipt). A tracked send is admitted
            // before the lane dispatch below, under the lock like every lane,
            // so meerkat's typed pre-admission refusal (the member's ACTUAL
            // runtime mode refuses per-turn completion: a profile whose
            // runtime_mode changed without a respawn) can still take the
            // ingress lane. Nothing was delivered by the refused attempt, so
            // the send is still delivered exactly once.
            let step = match lane {
                DeliveryLane::Ingress => DeliveryStep::Ingress,
                DeliveryLane::AwaitCommit => DeliveryStep::AwaitCommit,
                DeliveryLane::Ticketed => {
                    let mut delivery = super::bridge::BridgeDelivery::new(
                        content_to_deliver.clone(),
                        handling_mode,
                    );
                    delivery.system_prompt = system_prompt.map(ToString::to_string);
                    delivery.injected_context = injected_context.clone();
                    delivery.interaction_id = bridge_interaction_id.map(ToString::to_string);
                    // One automatic non-destructive reload on a typed
                    // reload-required refusal, then the typed failure (same
                    // rule as the other lanes).
                    let mut reload_attempted = false;
                    loop {
                        let attempt = match bridge
                            .begin_delivery_with_output(rid, delivery.clone())
                            .await
                        {
                            Err(BridgeAdmissionError::UnsupportedForMode {
                                mode, detail, ..
                            }) => {
                                turn_tracking =
                                    Some(runtime_refused_tracking(identity, mode, &detail));
                                break DeliveryStep::Ingress;
                            }
                            attempt => attempt.map_err(|err| admission_phase_error(identity, err)),
                        };
                        match attempt {
                            Ok(receipt) => {
                                self.record_delivery_success(identity).await;
                                break DeliveryStep::Tracked(receipt);
                            }
                            Err(err) => {
                                self.record_delivery_error(identity, &err).await;
                                if !reload_attempted
                                    && matches!(err, IdentityRuntimeError::ReloadRequired { .. })
                                {
                                    reload_attempted = true;
                                    if let Err(reload_error) =
                                        self.reload_for_delivery_locked(identity, &err).await
                                    {
                                        self.record_delivery_error(identity, &reload_error).await;
                                        return Err(reload_error);
                                    }
                                    token = self
                                        .fence_after_delivery_reload(identity, rid, "send")
                                        .await?;
                                    continue;
                                }
                                return Err(err);
                            }
                        }
                    }
                }
            };
            let delivered_session_id = match step {
                // Validated -> Delivered. One await, all of it bounded, all of
                // it under the lock. Unchanged.
                DeliveryStep::Ingress => {
                    // One automatic non-destructive reload on a typed
                    // reload-required refusal, then the typed failure. Never a
                    // loop: a second refusal after a reload is an operator
                    // matter.
                    let mut reload_attempted = false;
                    loop {
                        let attempt = if local_human {
                            let session = human_session.as_ref().ok_or(
                                IdentityRuntimeError::HostHumanInput(
                                    super::bridge::HostHumanInputError::BindingChanged,
                                ),
                            )?;
                            let mut delivery = super::bridge::BridgeDelivery::new(
                                content_to_deliver.clone(),
                                handling_mode,
                            );
                            delivery.system_prompt = system_prompt.map(ToString::to_string);
                            delivery.injected_context = injected_context.clone();
                            delivery.interaction_id =
                                bridge_interaction_id.map(ToString::to_string);
                            delivery.delivery_identity = console_human
                                .as_ref()
                                .map(|human| human.delivery_identity.clone());
                            delivery.skill_references = console_human
                                .as_ref()
                                .map(|human| human.skill_references.to_vec())
                                .unwrap_or_default();
                            bridge
                                .deliver_host_human_input(rid, session, delivery)
                                .await
                        } else {
                            // External delivery keeps its established wire semantics.
                            // It is not a claim of local canonical human authorship.
                            bridge
                                .deliver_with_mode_context_and_system_prompt(
                                    rid,
                                    &content_to_deliver,
                                    system_prompt,
                                    &injected_context,
                                    handling_mode,
                                    bridge_interaction_id,
                                )
                                .await
                        }
                        .map_err(|e| ingress_phase_error(identity, e));
                        match attempt {
                            Ok(delivered) => {
                                self.record_delivery_success(identity).await;
                                break delivered;
                            }
                            Err(err) => {
                                self.record_delivery_error(identity, &err).await;
                                if !reload_attempted
                                    && matches!(err, IdentityRuntimeError::ReloadRequired { .. })
                                {
                                    reload_attempted = true;
                                    if let Err(reload_error) =
                                        self.reload_for_delivery_locked(identity, &err).await
                                    {
                                        // The reload's own typed verdict (refused,
                                        // timed out, failed) is what the operator
                                        // sees, so it is the recorded class too.
                                        self.record_delivery_error(identity, &reload_error).await;
                                        return Err(reload_error);
                                    }
                                    token = self
                                        .fence_after_delivery_reload(identity, rid, "send")
                                        .await?;
                                    continue;
                                }
                                return Err(err);
                            }
                        }
                    }
                }

                // Admitted(receipt) -> Tracked -> Delivered. The admission and
                // its bounded session resolution ran under the lock exactly
                // like the ingress lane, and the send returns at the same
                // point. The turn is NOT awaited here: its receipt moves to a
                // waiter that records the turn's own output under the ticket.
                DeliveryStep::Tracked(receipt) => {
                    let resolved_session = receipt.resolved_session().cloned();
                    let resolution_error =
                        receipt.session_resolution_error().map(ToString::to_string);
                    // Recorded before the send returns, so the caller's first
                    // look at the ticket never reads `Unknown`.
                    let ticket = self.track_turn(identity, None, receipt);
                    turn_tracking = Some(TurnTracking::Tracked(ticket));
                    match resolved_session {
                        Some(session_id) => session_id,
                        // The turn is admitted and tracked, and its outcome is
                        // its own; the resolution failure is reported on its
                        // own channel. There is no session to reconcile onto.
                        None => {
                            self.report_ticketed_resolution_failure(
                                identity,
                                ticket,
                                resolution_error
                                    .as_deref()
                                    .unwrap_or("no session was resolved"),
                            )
                            .await;
                            return Ok(SendCoreOutcome {
                                token,
                                completion_baseline,
                                turn: turn_tracking,
                            });
                        }
                    }
                }

                // Validated -> Admitted(receipt) -> [UNLOCK] -> Terminal ->
                // [RELOCK] -> Revalidated -> Reconciled | Superseded.
                //
                // The lock covers admission and bounded session resolution and
                // NOTHING else. Awaiting an LLM turn under it would serialise
                // same-identity sends behind a model call and block every
                // lifecycle operation - reset, retire, alias rebind - for the
                // turn's whole duration.
                DeliveryStep::AwaitCommit => {
                    // ADMITTED. Bounded; still under the lock. One automatic
                    // non-destructive reload on a typed reload-required
                    // refusal, then the typed failure (same rule as the
                    // ingress lane).
                    let mut reload_attempted = false;
                    let receipt = loop {
                        let attempt = bridge
                            .begin_awaiting_commit(
                                rid,
                                &content_to_deliver,
                                system_prompt,
                                &injected_context,
                                handling_mode,
                                bridge_interaction_id,
                            )
                            .await
                            .map_err(|err| admission_phase_error(identity, err));
                        match attempt {
                            Ok(receipt) => {
                                self.record_delivery_success(identity).await;
                                break receipt;
                            }
                            Err(err) => {
                                self.record_delivery_error(identity, &err).await;
                                if !reload_attempted
                                    && matches!(err, IdentityRuntimeError::ReloadRequired { .. })
                                {
                                    reload_attempted = true;
                                    if let Err(reload_error) =
                                        self.reload_for_delivery_locked(identity, &err).await
                                    {
                                        // The reload's own typed verdict (refused,
                                        // timed out, failed) is what the operator
                                        // sees, so it is the recorded class too.
                                        self.record_delivery_error(identity, &reload_error).await;
                                        return Err(reload_error);
                                    }
                                    token = self
                                        .fence_after_delivery_reload(identity, rid, "send")
                                        .await?;
                                    continue;
                                }
                                return Err(err);
                            }
                        }
                    };

                    // CAPTURE the incarnation we admitted onto, so a reset,
                    // retire or alias rebind during the unlocked window is
                    // detectable rather than silently reconciled onto.
                    let captured = self.capture_incarnation(identity).await?;

                    // UNLOCK. Nothing is held from here.
                    drop(_lifecycle_guard);

                    // TERMINAL. The LLM turn. The only long step.
                    let terminal = receipt.wait().await;

                    // RELOCK.
                    let _relock_guard = lifecycle_lock.lock().await;

                    // DETECT supersede WITHOUT returning on it. What the turn
                    // did and whether its embodiment still exists are
                    // INDEPENDENT axes, so neither may swallow the other. A
                    // post-lock read failure IS a supersede - the identity was
                    // retired or deleted while the turn ran - and must never
                    // surface as a pre-admission-shaped UnknownIdentity, which
                    // would say nothing was delivered.
                    let supersede: Option<String> = match self.capture_incarnation(identity).await {
                        Err(IdentityRuntimeError::UnknownIdentity(_)) => {
                            Some("the identity no longer exists".to_string())
                        }
                        Err(other) => Some(format!("the identity could not be re-read: {other}")),
                        Ok(current) if current != captured => {
                            Some(format!("admitted onto {captured:?}, now {current:?}"))
                        }
                        // Re-run the ORIGINAL expected alias through the
                        // authoritative check rather than comparing a copy
                        // of it. Any failure is a POST-admission supersede.
                        Ok(_) => match expected_alias {
                            Some(expected) => match self
                                .ensure_expected_member_alias_current(identity, expected)
                                .await
                            {
                                Ok(()) => None,
                                Err(err) => Some(format!(
                                    "the expected member alias {expected} is no longer \
                                         current: {err}"
                                )),
                            },
                            None => None,
                        },
                    };

                    // REVALIDATE BEFORE RECONCILE is load-bearing rather than
                    // defensive: `reconcile_delivered_session_locked` treats a
                    // session mismatch as "the bridge rotated the session" and
                    // calls `rebind_session_after_live_respawn_locked`, which
                    // SUSPENDS the identity and RE-ACQUIRES its leases. After a
                    // reset during the unlocked window, current belongs to the
                    // NEW incarnation and delivered to the dead OLD turn, so
                    // reconciling first would bind the live incarnation to a
                    // dead turn's session - an active corruption path with a
                    // lifecycle mutation in it.
                    return match (terminal, supersede) {
                        // The turn did not reach a clean success. Nothing was
                        // superseded, so this is the ordinary phase mapping.
                        (Err(err), None) => Err(turn_phase_error(identity, err)),
                        // Both axes fired. Which one leads depends on what the
                        // turn actually did, and the two phases mean opposite
                        // things about that.
                        (
                            Err(BridgeTurnError::PostAdmissionResolutionFailed(detail)),
                            Some(sup),
                        ) => Err(IdentityRuntimeError::PostAdmissionSuperseded {
                            identity: identity.clone(),
                            detail: format!(
                                "{sup}; the turn also failed to project its session: \
                                     {detail}"
                            ),
                        }),
                        // The turn RAN AND FAILED. That is what an operator
                        // acts on, so it leads; the supersede is secondary.
                        (Err(BridgeTurnError::CompletionFailed(detail)), Some(sup)) => {
                            Err(IdentityRuntimeError::CompletionFailed {
                                identity: identity.clone(),
                                detail: format!(
                                    "{detail}; additionally, the identity was superseded \
                                     while the turn ran: {sup}"
                                ),
                            })
                        }
                        // The turn SUCCEEDED against an embodiment that is
                        // gone. Do NOT reconcile: losing the attribution beats
                        // binding the live incarnation to a dead turn's session.
                        (Ok(_), Some(sup)) => Err(IdentityRuntimeError::PostAdmissionSuperseded {
                            identity: identity.clone(),
                            detail: sup,
                        }),
                        // RECONCILED. Same relocked guard the ingress lane
                        // would have held throughout.
                        (Ok(delivered), None) => {
                            if let Some(rebound_token) = self
                                .reconcile_delivered_session_locked(
                                    identity,
                                    live.session_id.as_ref(),
                                    delivered,
                                )
                                .await?
                            {
                                token = rebound_token;
                            }
                            Ok(SendCoreOutcome {
                                token,
                                completion_baseline,
                                turn: None,
                            })
                        }
                    };
                }
            };
            if let Some(rebound_token) = self
                .reconcile_delivered_session_locked(
                    identity,
                    live.session_id.as_ref(),
                    delivered_session_id,
                )
                .await?
            {
                token = rebound_token;
            }
        }

        Ok(SendCoreOutcome {
            token,
            completion_baseline,
            turn: turn_tracking,
        })
    }

    /// Capture `identity`'s current Core incarnation (runtime id, generation,
    /// fence) for a later "did the owner change?" check. Every atom is Core's
    /// live member scope at the read; an identity Core holds no
    /// session-backed member for captures all-`None`.
    pub(crate) async fn capture_incarnation(
        &self,
        identity: &AgentIdentity,
    ) -> Result<CapturedIncarnation, IdentityRuntimeError> {
        if !self.entries.read().await.contains_key(identity) {
            return Err(IdentityRuntimeError::UnknownIdentity(identity.clone()));
        }
        Ok(match self.core_live_scope(identity).await? {
            Some(scope) => CapturedIncarnation {
                runtime_id: Some(meta_runtime_id(scope.runtime_id())?),
                generation: Some(scope.generation().get()),
                fencing_token: Some(FencingToken::new(scope.fence_token().get())),
            },
            None => CapturedIncarnation {
                runtime_id: None,
                generation: None,
                fencing_token: None,
            },
        })
    }

    // -----------------------------------------------------------------------
    // Scope-bound delivery
    // -----------------------------------------------------------------------

    /// Capture `identity`'s exact delivery scope: Core's live member scope
    /// (runtime incarnation, generation, fence, session), with its runtime
    /// id, generation and fence also in the scope's v1 atom slots.
    ///
    /// Strictly observational: it materializes nothing, renews nothing and
    /// submits nothing. A host persists the scope before dispatching against
    /// it with [`Self::dispatch_at_scope`].
    ///
    /// # Errors
    ///
    /// [`ScopedDeliveryError::Unsupported`] without a scoped bridge seam or
    /// for a member that cannot be scope-bound; `Rejected` for an unknown
    /// identity, or one Core holds no live session-backed member for.
    pub async fn capture_delivery_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<DeliveryScope, ScopedDeliveryError> {
        self.scoped_bridge()?;
        self.refuse_unscopable_identity(identity).await?;
        // No lifecycle lock: every atom comes from one Core read, and a scope
        // Core moved past fails the dispatch's own re-validation typed. A
        // status read never waits behind a delivery.
        let (runtime_id, generation, fence, member) =
            self.active_scope_continuity(identity).await?;
        Ok(DeliveryScope::new(
            identity.clone(),
            runtime_id,
            generation,
            fence,
            member,
        ))
    }

    /// Dispatch exact content to the member session a host captured with
    /// [`Self::capture_delivery_scope`] and persisted beforehand.
    ///
    /// Requires a full delivery identity pair, used exactly as supplied (no
    /// canonicalization). Never materializes, repairs or retargets the
    /// identity and applies no delivery preparation (no defang, no ambient
    /// memory, no injected context). Every scope atom is re-validated
    /// against Core's live scope here (a persisted 0.9.3 scope, whose atoms
    /// were Meta's, is refused), and the native session, incarnation and
    /// fence again inside meerkat's admission. A moved scope is refused as
    /// [`ScopedDeliveryError::StaleScope`] with nothing submitted.
    ///
    /// `input.origin` is not consulted: the scoped path has no origin-specific
    /// handling.
    ///
    /// # Errors
    ///
    /// See [`ScopedDeliveryError`]. Only `Uncertain` leaves the admission
    /// fate open; recover it with [`Self::recover_at_scope`] and the same
    /// scope.
    pub async fn dispatch_at_scope(
        &self,
        identity: &AgentIdentity,
        scope: &DeliveryScope,
        input: &DispatchInput,
    ) -> Result<ScopedDispatchReceipt, ScopedDeliveryError> {
        Self::ensure_scope_identity(identity, scope)?;
        let delivery_identity = scoped_delivery_identity(input)?;
        let bridge = self.scoped_bridge()?;
        self.refuse_unscopable_identity(identity).await?;
        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        let (runtime_id, generation, fence, _) = self.active_scope_continuity(identity).await?;
        if &runtime_id != scope.agent_runtime_id() {
            return Err(ScopedDeliveryError::StaleScope {
                mismatch: ScopeMismatch::RuntimeId,
                detail: format!(
                    "identity '{identity}' runs as '{runtime_id}', the scope names '{}'",
                    scope.agent_runtime_id()
                ),
            });
        }
        if generation != scope.generation() {
            return Err(ScopedDeliveryError::StaleScope {
                mismatch: ScopeMismatch::Generation,
                detail: format!(
                    "identity '{identity}' is at generation {generation}, the scope names {}",
                    scope.generation()
                ),
            });
        }
        // Core's fence in the v1 token slot: any other value is another
        // incarnation, or a 0.9.3 scope whose slot held Meta's lease token.
        if fence != scope.lease_fencing_token() {
            return Err(ScopedDeliveryError::StaleScope {
                mismatch: ScopeMismatch::LeaseFencingToken,
                detail: format!(
                    "identity '{identity}' runs under Core fence {fence}, the scope names {}",
                    scope.lease_fencing_token()
                ),
            });
        }
        let receipt = bridge
            .submit_at_scope(
                &runtime_id,
                scope.member(),
                &input.content,
                HandlingMode::Queue,
                &delivery_identity,
            )
            .await?;
        ScopedDispatchReceipt::admitted(
            scope.clone(),
            receipt.work_ref.to_string(),
            receipt.stage,
            receipt.session_id,
        )
    }

    /// Read one delivery's state from the ORIGINAL session `scope` pins,
    /// never from the identity's current session. Works for an identity in
    /// any lifecycle state; takes no lock and changes nothing.
    ///
    /// An authoritative miss is [`ScopedRecovery::Absent`], which is never
    /// permission to retry; an unreachable original owner is
    /// [`ScopedRecovery::Unresolved`].
    ///
    /// # Errors
    ///
    /// `Unsupported` without a scoped bridge seam; `Rejected` for a scope of
    /// another identity or an invalid delivery identity.
    pub async fn recover_at_scope(
        &self,
        identity: &AgentIdentity,
        scope: &DeliveryScope,
        input: &DispatchInput,
        timeout: Duration,
    ) -> Result<ScopedRecovery, ScopedDeliveryError> {
        Self::ensure_scope_identity(identity, scope)?;
        let delivery_identity = scoped_delivery_identity(input)?;
        let bridge = self.scoped_bridge()?;
        self.refuse_unscopable_identity(identity).await?;
        Self::ensure_scope_member(scope)?;
        let recovery = bridge
            .recover_at_scope(scope.member(), &delivery_identity, Instant::now() + timeout)
            .await?;
        Ok(ScopedRecovery::from_native(recovery.into_parts().1))
    }

    /// Retire `identity` only while it still runs at `scope`, a delivery
    /// scope the host captured with [`Self::capture_delivery_scope`].
    ///
    /// Cancellation-safe, like [`Self::retire_tracked`]. Core decides: the
    /// scope's native member scope goes to meerkat's scoped retire
    /// (`MobHandle::retire_at_scope`). It retires exactly the member
    /// incarnation the scope names, with [`Self::retire_tracked`]'s
    /// process-tree semantics: the member's `spawned_by` descendants, the
    /// member, then the descendants again. Each step checks the scope in the
    /// same actor command that retires, so a successor minted while the call
    /// runs (a respawn, a reset, a wake after a retire) is never retired
    /// through an older scope. Meta holds no lease and writes no continuity
    /// record for it. Under the identity's lifecycle lock, an ordinary
    /// retire's memory hooks run first while the scope still names the live
    /// member.
    ///
    /// Returns the Core fence of the incarnation the scope names. `Ok` is
    /// meerkat's terminal barrier: that incarnation is retired, or no
    /// incarnation of the identity is current and none is still retiring,
    /// so a retry after a lost reply converges to `Ok`, as does a first call
    /// for an identity that is already retired.
    ///
    /// # Errors
    ///
    /// [`ScopedRetireError`]. The scope's member is not retired when the call
    /// answers:
    /// - `Unsupported`: the bridge has no scoped seam, or the identity is
    ///   externally bound;
    /// - `Refused`: a host-adopted identity (Core holds its identity intent
    ///   row) or one whose identity history Core could not read;
    /// - `Rejected`: a scope of another identity or member, a runtime that no
    ///   longer admits identity operations, or a mob that is not running;
    /// - `StaleScope`: another incarnation of the identity is current
    ///   (`RuntimeId` when it runs under another runtime id,
    ///   `LeaseFencingToken` when only Core's fence moved), or the scope's
    ///   incarnation is current under another session (`MemberBinding`).
    ///
    /// Meta answers `Unsupported`, `Refused` and the scope check's
    /// `Rejected` before Core is asked, so nothing is retired. Core retires
    /// the member's `spawned_by` descendants before the member and checks the
    /// scope in every step: a `StaleScope` or `Rejected` it reaches after its
    /// first step (a successor minted, or the mob stopping, while the
    /// cascade runs) leaves the descendants it already retired retired.
    ///
    /// `Retire` is meerkat's retirement failing or stuck, or the tracked
    /// operation ending without a result.
    pub async fn retire_at_scope_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        scope: &DeliveryScope,
    ) -> Result<FencingToken, ScopedRetireError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let scope = scope.clone();
        // A runtime that admits no operation (it is shutting down, or its
        // mob is closed) refuses before any of the retirement runs.
        let receiver = self
            .start_tracked_foreground_with(ForegroundOwnerSource::Acquire, async move {
                Ok(runtime.retire_at_scope(&identity, &scope).await)
            })
            .await
            .map_err(|error| ScopedRetireError::Rejected {
                detail: error.to_string(),
            })?;
        match receiver.await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => Err(ScopedRetireError::Retire(error)),
            // The operation ended without its result (it panicked): whether
            // the identity was retired is unknown.
            Err(_) => Err(ScopedRetireError::Retire(IdentityRuntimeError::Internal(
                "tracked foreground identity operation terminated without a result".to_string(),
            ))),
        }
    }

    /// [`Self::retire_at_scope_tracked`]'s body, in its tracked operation.
    async fn retire_at_scope(
        &self,
        identity: &AgentIdentity,
        scope: &DeliveryScope,
    ) -> Result<FencingToken, ScopedRetireError> {
        let refused = ScopedRetireError::from_scope_refusal;
        Self::ensure_scope_identity(identity, scope).map_err(refused)?;
        Self::ensure_scope_member(scope).map_err(refused)?;
        let bridge = self.scoped_bridge().map_err(refused)?;
        // A host-adopted identity is the host's: no Meta verb retires it.
        self.refuse_host_adopted(identity)
            .await
            .map_err(ScopedRetireError::Refused)?;
        self.refuse_unscopable_identity(identity)
            .await
            .map_err(refused)?;
        // Core's native scope is the authority: the retired incarnation's
        // public runtime id and fence are its, whatever Meta atoms the
        // persisted scope carries.
        let member = scope.member();
        let runtime_id =
            meta_runtime_id(member.runtime_id()).map_err(|error| ScopedRetireError::Rejected {
                detail: error.to_string(),
            })?;
        let retired = {
            let lifecycle_lock = self.lifecycle_lock_for(identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            // Core decides, step by step, which incarnation the scope names.
            // Meta runs an ordinary retire's memory hooks only while the scope
            // names the live member, before Core retires it.
            let live = match self.core_live_scope(identity).await {
                Ok(scope) => scope,
                // Core refuses the scope read of a member it holds no session
                // binding for, such as a retired one. With no live member
                // there is no live scope: the call goes on to Core, whose
                // answer for a scope no incarnation is current at is `Ok`.
                // Any other failure to read refuses here, before Core is asked.
                Err(error) => match self.core_live_member(identity).await {
                    Ok(None) => None,
                    Ok(Some(_)) | Err(_) => {
                        return Err(ScopedRetireError::Rejected {
                            detail: error.to_string(),
                        });
                    }
                },
            };
            let retires_live = live.as_ref() == Some(member);
            // Only a retire of the live member tells the host's retirement
            // hook: Core answers `Ok` for a scope no incarnation is current
            // at without retiring anything.
            let mut retiring = if retires_live {
                self.retiring_incarnation(identity, super::types::RetiredIdentityCause::Scoped)
                    .await
                    .map_err(|error| ScopedRetireError::Rejected {
                        detail: error.to_string(),
                    })?
            } else {
                None
            };
            if retires_live {
                self.clear_wake_notes_of_live_member(identity).await;
                self.distill_before_retire(identity, Some(member.session_id()))
                    .await;
            }
            bridge
                .retire_at_scope(&runtime_id, member)
                .await
                .map_err(|error| name_stale_retire_mismatch(error, live.as_ref(), member))?;
            self.settle_retired_registration(&mut retiring).await;
            if retires_live {
                self.entries.notify_binding_changed();
                self.mark_bootstrap_from_lifecycle(identity, IdentityLifecycleState::Dormant, None);
                self.emit_event(
                    identity,
                    IdentityEvent::StateChanged {
                        identity: identity.clone(),
                        new_state: IdentityLifecycleState::Dormant,
                    },
                )
                .await;
            }
            retiring
        };
        // The host hears of the retirement with the lifecycle lock released.
        self.release_retired_members(retired.into_iter().collect())
            .await;
        Ok(FencingToken::new(member.fence_token().get()))
    }

    fn scoped_bridge(&self) -> Result<&Arc<dyn SessionBridge>, ScopedDeliveryError> {
        self.bridge
            .as_ref()
            .ok_or_else(|| ScopedDeliveryError::Unsupported {
                detail: "no session bridge is installed".to_string(),
            })
    }

    /// An externally bound (remotely hosted or peer-only) identity has no
    /// native session binding, so it cannot be scope-bound. The scoped
    /// capability refuses it typed and never falls back to the unscoped
    /// submit: a caller that asked for scope correctness must not silently
    /// get a weaker guarantee. An identity this runtime no longer knows is
    /// left to the scope itself (recovery reads the scope's own session).
    async fn refuse_unscopable_identity(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), ScopedDeliveryError> {
        let entries = self.entries.read().await;
        match entries.get(identity) {
            Some(entry) if durable_spec_uses_external_binding(&entry.spec) => {
                Err(ScopedDeliveryError::Unsupported {
                    detail: format!(
                        "identity '{identity}' is externally bound: {NO_NATIVE_SESSION_BINDING}"
                    ),
                })
            }
            _ => Ok(()),
        }
    }

    /// The scope's nested member must be the roster member this identity
    /// composes as. The roster id is the comms-safe encoding of the durable
    /// identity for every binding, so it is the same after rotation,
    /// unregistration and restart, and no live registration is consulted. A
    /// scope whose member belongs to another identity is refused before
    /// anything is read; meerkat proves the session against that member's
    /// own history.
    fn ensure_scope_member(scope: &DeliveryScope) -> Result<(), ScopedDeliveryError> {
        let member =
            crate::member_comms_id::roster_member_id_for_identity(scope.identity().as_str());
        if scope.member().agent_identity() == &member {
            Ok(())
        } else {
            Err(ScopedDeliveryError::Rejected {
                detail: format!(
                    "the delivery scope names member '{}', identity '{}' is member '{member}'",
                    scope.member().agent_identity(),
                    scope.identity()
                ),
            })
        }
    }

    fn ensure_scope_identity(
        identity: &AgentIdentity,
        scope: &DeliveryScope,
    ) -> Result<(), ScopedDeliveryError> {
        if scope.identity() == identity {
            Ok(())
        } else {
            Err(ScopedDeliveryError::Rejected {
                detail: format!(
                    "the delivery scope belongs to identity '{}', not '{identity}'",
                    scope.identity()
                ),
            })
        }
    }

    /// The scope atoms of `identity`'s live Core member, read without
    /// changing anything (the scoped path never wakes an identity): Core's
    /// runtime id (as Meta's public alias), Core's generation and Core's
    /// fence in the v1 slots, plus
    /// Core's native member scope.
    async fn active_scope_continuity(
        &self,
        identity: &AgentIdentity,
    ) -> Result<
        (
            AgentRuntimeId,
            ContinuityGeneration,
            FencingToken,
            meerkat_mob::MemberDeliveryScope,
        ),
        ScopedDeliveryError,
    > {
        if !self.entries.read().await.contains_key(identity) {
            return Err(ScopedDeliveryError::Rejected {
                detail: format!("unknown identity: {identity}"),
            });
        }
        let scope = self
            .core_live_scope(identity)
            .await
            .map_err(|error| ScopedDeliveryError::Rejected {
                detail: error.to_string(),
            })?
            .ok_or_else(|| ScopedDeliveryError::Rejected {
                detail: format!(
                    "Core holds no live session-backed member for identity '{identity}'; scoped \
                     delivery never wakes an identity"
                ),
            })?;
        let runtime_id =
            meta_runtime_id(scope.runtime_id()).map_err(|error| ScopedDeliveryError::Rejected {
                detail: error.to_string(),
            })?;
        Ok((
            runtime_id,
            ContinuityGeneration::new(scope.generation().get()),
            FencingToken::new(scope.fence_token().get()),
            scope,
        ))
    }

    // -----------------------------------------------------------------------
    // Delivery: dispatch() - REQ-02
    // -----------------------------------------------------------------------

    /// Dispatch internal content to any identity (Addressable or InternalOnly).
    ///
    /// Enforces:
    /// - Identity must be registered and active
    /// - Lease must be held (INV-01)
    /// - Lease must not be lost (INV-02)
    ///
    /// Returns (fencing_token, is_durable) where is_durable indicates whether
    /// the dispatch is backed by a runtime_store (REQ-04).
    pub async fn dispatch(
        &self,
        identity: &AgentIdentity,
        input: &DispatchInput,
    ) -> Result<(FencingToken, bool), IdentityRuntimeError> {
        self.dispatch_with_expected_member_alias(identity, None, input)
            .await
            .map(|outcome| (outcome.admission.fencing_token, outcome.admission.durable))
    }

    /// The runtime mode a member actually runs in, resolved exactly as
    /// meerkat-mob resolves it at spawn: the roster's explicit override wins,
    /// else the profile's `runtime_mode`, else meerkat's default
    /// (`AutonomousHost`). Reading only the override - as the first cut of the
    /// autonomous-host skip did - treated every member whose roster provider
    /// set nothing as autonomous, and skipped ambient memory on `turn_driven`
    /// profiles fleet-wide. Two sources of one fact; this is the one meerkat
    /// uses.
    ///
    /// A profile bound by realm reference (not inline) has no mode Meta can
    /// read here and resolves to the default; that is a documented limitation,
    /// not a guess dressed as knowledge.
    fn effective_runtime_mode(&self, spec: &DurableAgentSpec) -> meerkat_mob::MobRuntimeMode {
        // The mob definition reaches this runtime through the reset-roster
        // context the gateway installs at boot; a host that installs none has
        // given Meta no profile table to read, and resolves to the default.
        let source = self
            .reset_roster_source
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        effective_runtime_mode(
            source
                .as_ref()
                .and_then(|source| source.mob_definition.as_ref()),
            spec,
        )
    }

    /// The ONE delivery preparation every member door runs (send, keyed
    /// send, dispatch): the send door always had it; the dispatch door
    /// previously delivered raw,
    /// so internal dispatches (schedules foremost) skipped defanging, taint
    /// session attribution, and ambient memory injection for the member's
    /// whole lifetime (a downstream app: zero surface=Turn injection-ledger rows).
    ///
    /// Steer is latency-sensitive live operator input: it bypasses both
    /// memory injection and inbound defanging by design. Every other
    /// delivery is defanged first (section 9.1 anti-spoofing - even with injection
    /// off, forged memory envelopes are an inbound threat) and only then
    /// considered for ambient injection. Ask 1: the user content and the
    /// ambient recall travel as SEPARATE bodies - the (defanged) message and
    /// the recall as its own typed injected-context body, never fused.
    /// section 10.1 taint hook: `note_current_session` keeps session attribution
    /// authoritative ahead of the async observe stream; the generation bind
    /// feeds the Distiller's EvidenceRefs (section 8.4).
    async fn prepare_member_delivery(
        &self,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
        memory_session_key: Option<&str>,
        memory_generation: Option<u64>,
        steer: bool,
        preparation: DeliveryPreparation,
    ) -> Result<(meerkat_core::ContentInput, Vec<meerkat_core::ContentInput>), IdentityRuntimeError>
    {
        if steer {
            return Ok((content.clone(), Vec::new()));
        }
        match self.agent_memory.read().await.clone() {
            Some(injector) => {
                if let Some(session_key) = memory_session_key {
                    injector.note_current_session(identity, session_key);
                    if let Some(generation) = memory_generation {
                        injector.note_session_generation(identity, session_key, generation);
                    }
                }
                let defanged = injector.defang_inbound(identity, content);
                // meerkat refuses injected context on generic autonomous work
                // ("autonomous inbox delivery carries no user-channel work
                // boundary"), and a refusal there is the WHOLE turn, not just the
                // memory. Defang still runs - it is an inbound threat regardless of
                // mode - but recall is skipped, typed, so the zero names itself.
                if matches!(
                    preparation,
                    DeliveryPreparation::Work(meerkat_mob::MobRuntimeMode::AutonomousHost)
                ) {
                    injector.note_turn_skip(
                        identity,
                        crate::memory::coordinator::TurnInjectionSkip::RuntimeModeAutonomousHost,
                    );
                    return Ok((defanged, Vec::new()));
                }
                // A keyed send is committed as a member continuation, whose
                // prompt has no injected-context body: recall is skipped,
                // typed, after the defang.
                if matches!(preparation, DeliveryPreparation::KeyedContinuation) {
                    injector.note_turn_skip(
                        identity,
                        crate::memory::coordinator::TurnInjectionSkip::KeyedContinuation,
                    );
                    return Ok((defanged, Vec::new()));
                }
                let injected_context = injector
                    .inject_for_turn(identity, memory_session_key, &defanged)
                    .await
                    .map_err(|err| {
                        IdentityRuntimeError::Internal(format!("agent memory recall: {err}"))
                    })?;
                Ok((defanged, injected_context))
            }
            None => Ok((content.clone(), Vec::new())),
        }
    }

    async fn dispatch_with_expected_member_alias(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        input: &DispatchInput,
    ) -> Result<DispatchOutcome, IdentityRuntimeError> {
        self.dispatch_with_expected_incarnation(identity, expected_alias, None, input)
            .await
    }

    pub(crate) async fn dispatch_with_expected_incarnation(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        expected: Option<&CapturedIncarnation>,
        input: &DispatchInput,
    ) -> Result<DispatchOutcome, IdentityRuntimeError> {
        self.dispatch_core(identity, expected_alias, expected, input, false)
            .await
    }

    /// The ONE dispatch body. `track_turn` admits the work under a
    /// [`TurnTicket`] (see [`Self::dispatch_with_turn_ticket`]); everything
    /// else is identical.
    async fn dispatch_core(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        expected: Option<&CapturedIncarnation>,
        input: &DispatchInput,
        track_turn: bool,
    ) -> Result<DispatchOutcome, IdentityRuntimeError> {
        let result = self
            .dispatch_core_admit(identity, expected_alias, expected, input, track_turn)
            .await;
        // A maintenance dispatch (`expected` incarnation) never touched the
        // topology; an ordinary one hydrates it, after admission.
        if expected.is_none() && input_was_admitted(&result) {
            self.hydrate_peer_topology_after_admission(identity).await;
        }
        result
    }

    /// Admission for [`Self::dispatch_core`]; the same admission-first
    /// contract as [`Self::send_core_admit`].
    async fn dispatch_core_admit(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        expected: Option<&CapturedIncarnation>,
        input: &DispatchInput,
        track_turn: bool,
    ) -> Result<DispatchOutcome, IdentityRuntimeError> {
        if !self.entries.read().await.contains_key(identity) {
            return Err(IdentityRuntimeError::UnknownIdentity(identity.clone()));
        }
        // Core holds no live member (dormant, or retired: the operator's
        // "retire then send" recovery path): wake it onto its committed
        // session through the one wake door. A maintenance dispatch bound to
        // a captured incarnation never wakes anything.
        if expected.is_none() && self.core_live_member(identity).await?.is_none() {
            self.materialize_for_admission(identity, expected_alias, None)
                .await?;
        }

        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        if let Some(expected) = expected
            && self.capture_incarnation(identity).await? != *expected
        {
            return Err(IdentityRuntimeError::PostAdmissionSuperseded {
                identity: identity.clone(),
                detail: "maintenance incarnation changed before dispatch".to_string(),
            });
        }
        if let Some(expected_alias) = expected_alias {
            self.ensure_expected_member_alias_current(identity, expected_alias)
                .await?;
        }
        let (live, scope) = self.live_delivery_target(identity, "dispatch").await?;
        let mut token = Self::scope_fence(scope.as_ref());
        // Same pre-delivery baseline contract as the send path - see
        // `send_with_mode_and_interaction_with_expected_member_alias`.
        let completion_baseline = self.credited_completion_cursor(identity, token).await;
        let (
            is_durable,
            runtime_id,
            current_session,
            memory_session_key,
            memory_generation,
            memory_runtime_mode,
            external_binding,
        ) = {
            let entries = self.entries.read().await;
            let entry = entries
                .get(identity)
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
            // REQ-04: durability depends on the runtime store composition.
            let is_durable = self.has_runtime_store;

            (
                is_durable,
                Some(live.agent_runtime_id.clone()),
                live.session_id.clone(),
                live.session_id.as_ref().map(ToString::to_string),
                scope.as_ref().map(|scope| scope.generation().get()),
                self.effective_runtime_mode(&entry.spec),
                durable_spec_uses_external_binding(&entry.spec),
            )
        };

        // The dispatch door runs the SAME delivery preparation as
        // the send door (defang, taint session attribution, ambient memory
        // injection). Dispatch has no Steer mode; `DispatchOrigin` rides the
        // input untouched.
        let (content_to_deliver, injected_context) = self
            .prepare_member_delivery(
                identity,
                &input.content,
                memory_session_key.as_deref(),
                memory_generation,
                false,
                DeliveryPreparation::Work(memory_runtime_mode),
            )
            .await?;
        // The fail-closed matrix, validated BEFORE bridge admission:
        // (None, None) is an ordinary delivery; a full pair validates
        // upstream (canonical non-nil UUID correlation) and carries; EVERY
        // other combination - half-pair, invalid UUID - is a typed refusal
        // and NO delivery occurs. Never warn-and-degrade: a degraded
        // delivery under a broken identity is a silent dedup hole.
        let delivery_identity = match (&input.idempotency_key, &input.correlation_id) {
            (None, None) => None,
            (Some(idempotency_key), Some(correlation_id)) => {
                // App-supplied correlations canonicalize deterministically
                // instead of refusing on value shape (downstream app admission
                // break: source-string correlations were tolerated through
                // the 0.8.15 pair and refused by 0.8.16 identity threading).
                // Half-pairs below stay typed refusals - structure is still
                // fail closed; only the VALUE domain is widened, and ONLY
                // for the app-reachable dispatch origins (Connector, plus
                // the RPC surface's System default). Code-owned lanes -
                // Scheduler, Policy, Flow - mint their own canonical UUIDs,
                // so a non-canonical value there is a defect to refuse
                // typed (the fail-closed matrix), never to mask.
                let app_lane = matches!(
                    input.origin,
                    super::types::DispatchOrigin::Connector | super::types::DispatchOrigin::System
                );
                let canonical = if app_lane {
                    crate::member_comms_id::canonical_correlation_id(correlation_id.as_str())
                } else {
                    std::borrow::Cow::Borrowed(correlation_id.as_str())
                };
                if canonical.as_ref() != correlation_id.as_str() {
                    tracing::info!(
                        identity = %identity,
                        canonical_correlation = %canonical,
                        source_len = correlation_id.as_str().len(),
                        "app-supplied delivery correlation canonicalized to UUIDv5 \
                         for bridge admission"
                    );
                }
                Some(
                    meerkat_mob::MobDeliveryIdentity::new(
                        idempotency_key.as_str(),
                        canonical.as_ref(),
                    )
                    .map_err(|error| {
                        IdentityRuntimeError::InvalidDeliveryIdentity {
                            identity: identity.clone(),
                            detail: error.to_string(),
                        }
                    })?,
                )
            }
            (Some(_), None) => {
                return Err(IdentityRuntimeError::InvalidDeliveryIdentity {
                    identity: identity.clone(),
                    detail: "idempotency key without a correlation id (half-pair)".to_string(),
                });
            }
            (None, Some(_)) => {
                return Err(IdentityRuntimeError::InvalidDeliveryIdentity {
                    identity: identity.clone(),
                    detail: "correlation id without an idempotency key (half-pair)".to_string(),
                });
            }
        };

        // Deliver through the session bridge when available. When the dedup
        // carrier is present its correlation id also rides as the
        // interaction id (meerkat requires the two to agree), tracked or not.
        // A tracked dispatch's ticket is minted per admission and never
        // derived from the correlation, which several admissions may share
        // (a fan-out to several identities, several keys under one
        // correlation).
        let mut dispatched_session_id = None;
        let mut turn = track_turn.then(untracked_delivery);
        if let (Some(bridge), Some(rid)) = (&self.bridge, &runtime_id) {
            // The dedup scope a re-dispatch would land in: this incarnation's
            // current session ledger.
            let current_delivery_key = delivery_identity
                .as_ref()
                .zip(current_session.as_ref())
                .map(|(carrier, session)| {
                    (
                        rid.clone(),
                        session.clone(),
                        carrier.idempotency_key.clone(),
                    )
                });
            let ticketed = track_turn
                && match turn_untrackable(
                    true,
                    external_binding,
                    false,
                    bridge.tracks_turn_output(),
                ) {
                    Some(reason) => {
                        turn = Some(TurnTracking::Unavailable(reason));
                        false
                    }
                    // A re-dispatch of a key whose tracked admission is still
                    // pending or completed is deduplicated by the runtime onto
                    // that admission: it names the original's ticket and is
                    // delivered on the ingress lane, so no second waiter can
                    // ever report the runtime's "deduplicated, no result of
                    // its own" terminal over the original's output.
                    None => match current_delivery_key
                        .as_ref()
                        .and_then(|key| self.reusable_delivery_ticket(identity, key))
                    {
                        Some(original) => {
                            turn = Some(TurnTracking::Tracked(original));
                            false
                        }
                        None => true,
                    },
                };
            let mut delivery =
                super::bridge::BridgeDelivery::new(content_to_deliver.clone(), HandlingMode::Queue);
            delivery.injected_context = injected_context.clone();
            delivery.interaction_id = delivery_identity
                .as_ref()
                .map(|carrier| carrier.correlation_id.clone());
            delivery.delivery_identity = delivery_identity.clone();
            // meerkat's typed pre-admission refusal of the tracked admission
            // (the member's ACTUAL runtime mode) sends the dispatch down the
            // ingress lane below: nothing was delivered by the refused attempt.
            let tracked_attempt = if ticketed {
                match bridge
                    .begin_delivery_with_output(rid, delivery.clone())
                    .await
                {
                    Err(BridgeAdmissionError::UnsupportedForMode { mode, detail, .. }) => {
                        turn = Some(runtime_refused_tracking(identity, mode, &detail));
                        None
                    }
                    attempt => Some(attempt),
                }
            } else {
                None
            };
            let delivered_session_id = if let Some(attempt) = tracked_attempt {
                // Same admission and reconciliation as the ingress arm; the
                // receipt moves to a waiter that records the turn's own output.
                let attempt = attempt.map_err(|error| admission_phase_error(identity, error));
                let receipt = match attempt {
                    Ok(receipt) => {
                        self.record_delivery_success(identity).await;
                        receipt
                    }
                    Err(error) => {
                        self.record_delivery_error(identity, &error).await;
                        return Err(error);
                    }
                };
                let resolved_session = receipt.resolved_session().cloned();
                let resolution_error = receipt.session_resolution_error().map(ToString::to_string);
                // Indexed under the session the turn was actually admitted onto;
                // an unresolved session cannot name a dedup scope, so it is not
                // indexed.
                let admitted_delivery_key = delivery_identity
                    .as_ref()
                    .zip(resolved_session.as_ref())
                    .map(|(carrier, session)| {
                        (
                            rid.clone(),
                            session.clone(),
                            carrier.idempotency_key.clone(),
                        )
                    });
                let ticket = self.track_turn(identity, admitted_delivery_key, receipt);
                turn = Some(TurnTracking::Tracked(ticket));
                match resolved_session {
                    Some(session_id) => session_id,
                    None => {
                        self.report_ticketed_resolution_failure(
                            identity,
                            ticket,
                            resolution_error
                                .as_deref()
                                .unwrap_or("no session was resolved"),
                        )
                        .await;
                        return Ok(DispatchOutcome {
                            admission: DispatchAdmission {
                                fencing_token: token,
                                durable: is_durable,
                                completion_baseline,
                            },
                            session_id: None,
                            incarnation: self.capture_incarnation(identity).await?,
                            turn,
                        });
                    }
                }
            } else {
                let attempt = bridge
                    .deliver_admitted(rid, delivery)
                    .await
                    .map_err(|error| ingress_phase_error(identity, error));
                match attempt {
                    Ok(delivered) => {
                        self.record_delivery_success(identity).await;
                        delivered
                    }
                    Err(error) => {
                        self.record_delivery_error(identity, &error).await;
                        return Err(error);
                    }
                }
            };
            // A reused ticket claims the runtime deduplicated this delivery
            // onto the original. That holds only if it landed on the session
            // whose ledger admitted the original: delivery repair or a respawn
            // can move the member onto a new session under the same runtime
            // id, and that ledger has never seen the key, so the delivery ran
            // as a turn of its own. Then the original's ticket would name the
            // wrong turn: report it untracked instead.
            if let (Some(TurnTracking::Tracked(_)), Some((_, reused_session, _)), false) =
                (&turn, current_delivery_key.as_ref(), ticketed)
                && reused_session != &delivered_session_id
            {
                tracing::warn!(
                    identity = %identity,
                    reused_session = %reused_session,
                    delivered_session = %delivered_session_id,
                    "re-dispatch landed on a different session than the admission it was to be \
                     deduplicated onto; it ran as its own turn and is reported untracked"
                );
                turn = Some(TurnTracking::Unavailable(TurnUntrackable::SessionRotated));
            }
            dispatched_session_id = Some(delivered_session_id.clone());
            if let Some(rebound_token) = self
                .reconcile_delivered_session_locked(
                    identity,
                    live.session_id.as_ref(),
                    delivered_session_id,
                )
                .await?
            {
                token = rebound_token;
            }
        }

        Ok(DispatchOutcome {
            admission: DispatchAdmission {
                fencing_token: token,
                durable: is_durable,
                completion_baseline,
            },
            session_id: dispatched_session_id,
            incarnation: self.capture_incarnation(identity).await?,
            turn,
        })
    }

    /// Cancellation-safe dispatch for RPC/host request boundaries.
    pub async fn dispatch_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        input: &DispatchInput,
    ) -> Result<(FencingToken, bool), IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let input = input.clone();
        self.run_tracked_foreground(async move { runtime.dispatch(&identity, &input).await })
            .await
    }

    /// Cancellation-safe dispatch pinned to the generated runtime alias that
    /// the caller resolved.
    pub async fn dispatch_member_alias_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: &str,
        input: &DispatchInput,
    ) -> Result<(FencingToken, bool), IdentityRuntimeError> {
        self.dispatch_admission_tracked(identity, Some(expected_alias), input)
            .await
            .map(|admission| (admission.fencing_token, admission.durable))
    }

    /// Cancellation-safe dispatch that also returns the completion baseline to
    /// wait past. Dispatch counterpart of [`Self::send_admission_tracked`];
    /// pass `expected_alias = None` for the durable identity.
    pub async fn dispatch_admission_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        input: &DispatchInput,
    ) -> Result<DispatchAdmission, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.map(ToString::to_string);
        let input = input.clone();
        self.run_tracked_foreground(async move {
            runtime
                .dispatch_with_expected_member_alias(&identity, expected_alias.as_deref(), &input)
                .await
                .map(|outcome| outcome.admission)
        })
        .await
    }

    /// Cancellation-safe dispatch whose admitted turn is tracked by ticket.
    /// Dispatch counterpart of [`Self::send_with_turn_ticket`]: the same
    /// delivery as [`Self::dispatch_admission_tracked`] (the correlation id
    /// still rides as the interaction id), with the admitted turn tracked
    /// under a [`TurnTicket`] minted for this admission, never derived from
    /// the correlation, which a fan-out or several keys may share.
    ///
    /// A re-dispatch of an idempotency key whose tracked admission to this
    /// incarnation's current session is still pending or completed is
    /// deduplicated by the runtime onto that admission, so it names the
    /// original's ticket (and the original's output). Any other re-dispatch
    /// the runtime deduplicates (the original was untracked, failed, aged out
    /// of the registry, or ran before a session rotation this key does not
    /// span) is tracked on its own and settles with what the runtime reports
    /// for the deduplicated input: the ORIGINAL input's own result while that
    /// input is still running (the runtime shares the one input's
    /// completion), and [`TurnOutput::NoOwnResult`] once it has ended.
    pub async fn dispatch_with_turn_ticket(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        input: &DispatchInput,
    ) -> Result<Ticketed<DispatchAdmission>, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.map(ToString::to_string);
        let input = input.clone();
        self.run_tracked_foreground(async move {
            runtime
                .dispatch_core(&identity, expected_alias.as_deref(), None, &input, true)
                .await
                .map(|outcome| Ticketed {
                    admission: outcome.admission,
                    turn: outcome.turn.unwrap_or_else(untracked_delivery),
                })
        })
        .await
    }

    /// Scheduler delivery pinned to a generated alias, returning the exact
    /// bridge session that accepted the work while the lifecycle lock held.
    pub(crate) async fn dispatch_member_alias_with_session_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: &str,
        input: &DispatchInput,
    ) -> Result<Option<SessionId>, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.to_string();
        let input = input.clone();
        self.run_tracked_foreground(async move {
            runtime
                .dispatch_with_expected_member_alias(&identity, Some(&expected_alias), &input)
                .await
                .map(|outcome| outcome.session_id)
        })
        .await
    }

    // -----------------------------------------------------------------------
    // Status: status() - REQ-07
    // -----------------------------------------------------------------------

    /// Return the full identity status for the given identity.
    /// Hold the identity table's write lock until `release`, telling
    /// `acquired` once it is held: a contended table for timing tests.
    #[cfg(test)]
    pub(crate) async fn hold_identity_table_until(
        &self,
        acquired: tokio::sync::oneshot::Sender<()>,
        release: tokio::sync::oneshot::Receiver<()>,
    ) {
        let _held = self.entries.table.write().await;
        let _ = acquired.send(());
        let _ = release.await;
    }

    /// Whether a roster compile registered `identity` and no later compile
    /// dropped it (extension documents' principal rule reads it).
    #[cfg(feature = "extension-state")]
    pub(crate) async fn is_registered(&self, identity: &AgentIdentity) -> bool {
        self.entries.read().await.contains_key(identity)
    }

    /// The identity's status, read from Core on every call: Active
    /// while Core holds its member, Broken while Core holds that member
    /// Broken (it could not restore it) or after a FINAL refused wake,
    /// Dormant otherwise. The Core facts come from one read of Core's member
    /// history, a store read that never waits on the mob actor, and one read
    /// of Core's member list; a history read fault is typed history
    /// unavailable, never a guess.
    pub async fn status(
        &self,
        identity: &AgentIdentity,
    ) -> Result<IdentityStatus, IdentityRuntimeError> {
        if !self.entries.read().await.contains_key(identity) {
            return Err(IdentityRuntimeError::UnknownIdentity(identity.clone()));
        }
        let history = self.core_view_history().await.map_err(|error| {
            IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!("reading Core's member history: {error}"),
            }
        })?;
        let broken = self.core_broken_members().await;
        self.status_from(identity, history.as_ref(), &broken).await
    }

    /// [`Self::status`] over a history and Core's broken members already
    /// read.
    async fn status_from(
        &self,
        identity: &AgentIdentity,
        history: Option<&meerkat_mob::MemberBindingHistory>,
        broken: &BTreeMap<meerkat_mob::AgentIdentity, String>,
    ) -> Result<IdentityStatus, IdentityRuntimeError> {
        let (spec, wake_refusal, replaced_session) = {
            let entries = self.entries.read().await;
            let entry = entries
                .get(identity)
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
            (
                entry.spec.clone(),
                entry.wake_refusal.clone(),
                entry.replaced_session.clone(),
            )
        };
        let facts = CoreViewFacts::of(history, identity);
        let convergence = if facts.intent_managed {
            self.core_convergence(identity).await
        } else {
            None
        };
        let session_repair_required = self.session_repair_hold(wake_refusal.as_ref());
        Ok(IdentityStatus {
            identity: identity.clone(),
            state: self.core_view_state(identity, &facts, broken, wake_refusal.as_ref()),
            agent_runtime_id: facts.runtime_id,
            session_id: facts.session_id,
            member_id: facts.member_id,
            profile: Some(spec.profile.clone()),
            runtime_mode: spec.runtime_mode_override,
            addressability: spec.addressability,
            display_name: spec.display_name.clone(),
            labels: spec.labels.clone(),
            generation: facts.generation,
            intent_managed: facts.intent_managed,
            convergence,
            wake_refusal,
            replaced_session,
            session_repair_required,
            customizer_tools_pending: self.customizer_tools_pending_for(identity),
        })
    }

    /// The typed repair hold of a wake Core held for audited-endpoint
    /// repair (the `repair_session` remedy): the exact repair commands for
    /// the held session, scoped to this runtime's store. `None` for every
    /// other refusal.
    fn session_repair_hold(
        &self,
        wake_refusal: Option<&super::types::WakeRefusal>,
    ) -> Option<SessionRepairRequired> {
        let refusal = wake_refusal?;
        (refusal.remedy == super::types::WakeRefusalRemedy::RepairSession).then(|| {
            SessionRepairRequired::audited_endpoint_divergence(
                refusal.session_id.clone(),
                self.session_repair_scope().as_ref(),
                "Core held the session's resume: its committed document fails the \
                 audited-endpoint guard",
            )
        })
    }

    /// Core's members it holds Broken, with Core's reasons. None without a
    /// bridge, or when the read fails: no status read fails over this
    /// advisory read.
    async fn core_broken_members(&self) -> BTreeMap<meerkat_mob::AgentIdentity, String> {
        let Some(bridge) = self.bridge.as_ref() else {
            return BTreeMap::new();
        };
        bridge.broken_members().await.unwrap_or_else(|error| {
            tracing::debug!(%error, "Core's broken members could not be read");
            BTreeMap::new()
        })
    }

    /// The view state of `identity` from Core's facts: Broken while Core
    /// holds its live member Broken (it could not restore it), otherwise as
    /// [`Self::view_state`] answers.
    fn core_view_state(
        &self,
        identity: &AgentIdentity,
        facts: &CoreViewFacts,
        broken: &BTreeMap<meerkat_mob::AgentIdentity, String>,
        wake_refusal: Option<&super::types::WakeRefusal>,
    ) -> IdentityLifecycleState {
        let core_broken = facts.live
            && broken.contains_key(&crate::member_comms_id::roster_member_id_for_identity(
                identity.as_str(),
            ));
        if core_broken {
            IdentityLifecycleState::Broken
        } else {
            Self::view_state(self.runs_live_member(facts.live), wake_refusal)
        }
    }

    /// Core's member history: one fold of its journal and identity
    /// intent rows. `None` without a bridge (no Core mob, nothing live).
    async fn core_view_history(
        &self,
    ) -> Result<Option<meerkat_mob::MemberBindingHistory>, BridgeError> {
        match self.bridge.as_ref() {
            Some(bridge) => bridge.member_binding_history().await.map(Some),
            None => Ok(None),
        }
    }

    /// Core's convergence status of an intent-managed identity, in wire
    /// form naming the Meta identity. `None` while Core holds no readable
    /// status row: an unreadable row is diagnostic noise here, never a
    /// guessed condition.
    async fn core_convergence(
        &self,
        identity: &AgentIdentity,
    ) -> Option<meerkat_contracts::wire::WireIdentityConvergenceStatus> {
        let bridge = self.bridge.as_ref()?;
        match bridge.identity_convergence_status(identity).await {
            Ok(meerkat_mob::identity::IdentityStoredObservation::Valid(status)) => {
                let mut wire = status.to_wire();
                wire.agent_identity = identity.as_str().to_string();
                Some(wire)
            }
            Ok(_) => None,
            Err(error) => {
                tracing::debug!(%identity, %error, "Core's convergence status could not be read");
                None
            }
        }
    }

    /// Return statuses for every registered identity without waking any.
    /// One read of Core's member history serves every identity; when that
    /// read fails the call fails typed (`MemberHistoryUnavailable`): no
    /// status is guessed and no empty list stands in for "none".
    pub async fn statuses(&self) -> Result<Vec<IdentityStatus>, IdentityRuntimeError> {
        let identities = self
            .entries
            .read()
            .await
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let history = self.core_view_history().await.map_err(|error| {
            IdentityRuntimeError::MemberHistoryUnavailable {
                detail: format!("reading Core's member history: {error}"),
            }
        })?;
        let broken = self.core_broken_members().await;
        let mut statuses = Vec::with_capacity(identities.len());
        for identity in identities {
            // An identity unregistered between the two reads has no status.
            if let Ok(status) = self.status_from(&identity, history.as_ref(), &broken).await {
                statuses.push(status);
            }
        }
        Ok(statuses)
    }

    // -----------------------------------------------------------------------
    // Lifecycle: retire() - REQ-08
    // -----------------------------------------------------------------------

    /// Retire an identity: Core retires its member and keeps the member's
    /// history, so the next wake resumes the same session.
    pub async fn retire(&self, identity: &AgentIdentity) -> Result<(), IdentityRuntimeError> {
        self.retire_with_expected_member_alias(identity, None).await
    }

    async fn retire_with_expected_member_alias(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
    ) -> Result<(), IdentityRuntimeError> {
        let retired = {
            let lifecycle_lock = self.lifecycle_lock_for(identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            if let Some(expected_alias) = expected_alias {
                self.ensure_expected_member_alias_current(identity, expected_alias)
                    .await?;
            }
            let mut retiring = self
                .retiring_incarnation(identity, super::types::RetiredIdentityCause::Retire)
                .await?;
            self.retire_locked(identity).await?;
            self.settle_retired_registration(&mut retiring).await;
            retiring
        };
        // The host hears of the retirement with the lifecycle lock released.
        self.release_retired_members(retired.into_iter().collect())
            .await;
        Ok(())
    }

    async fn retire_locked(&self, identity: &AgentIdentity) -> Result<(), IdentityRuntimeError> {
        self.retire_locked_with_intent(identity, LifecycleRetireIntent::Retire)
            .await
    }

    /// Change a maintenance build input and rebuild only the captured owner.
    /// The existing retire and embodiment doors share one lifecycle lock.
    pub(crate) async fn rebuild_compaction_owner(
        &self,
        identity: &AgentIdentity,
        expected: &CapturedIncarnation,
        expected_alias: Option<&str>,
        change: super::bridge::CompactionFloorChange<'_>,
    ) -> Result<(SessionId, CapturedIncarnation), IdentityRuntimeError> {
        let lock = self.lifecycle_lock_for(identity).await;
        let _guard = lock.lock().await;
        if self.capture_incarnation(identity).await? != *expected {
            return Err(IdentityRuntimeError::PostAdmissionSuperseded {
                identity: identity.clone(),
                detail: "maintenance owner changed before profile rebuild".to_string(),
            });
        }
        if let Some(alias) = expected_alias {
            self.ensure_expected_member_alias_current(identity, alias)
                .await?;
        }
        let (before, _) = self
            .live_delivery_target(identity, "compact_member")
            .await?;
        self.retire_locked(identity).await?;
        match change.floor {
            Some(floor) => {
                change
                    .registry
                    .set_owned(identity, change.operation_id, floor);
            }
            None => {
                change.registry.clear_owned(identity, change.operation_id);
            }
        }
        let mut generation = None;
        let result = self
            .embody_identity_locked(
                identity,
                None,
                None,
                None,
                &mut generation,
                EmbodimentOverrides::default(),
            )
            .await
            .map(|outcome| outcome.binding);
        self.mark_bootstrap_materialization_finished(identity, &result, generation);
        change
            .registry
            .finish_owned_build(identity, change.operation_id);
        if result.is_err() && change.floor.is_some() {
            change.registry.clear_owned(identity, change.operation_id);
        }
        let binding = result?;
        let Some(session_id) = binding
            .session_id
            .filter(|session_id| before.session_id.as_ref() == Some(session_id))
        else {
            if change.floor.is_some() {
                change.registry.clear_owned(identity, change.operation_id);
            }
            return Err(IdentityRuntimeError::PostAdmissionSuperseded {
                identity: identity.clone(),
                detail: "maintenance rebuild did not retain the captured durable session"
                    .to_string(),
            });
        };
        Ok((session_id, self.capture_incarnation(identity).await?))
    }

    pub(crate) async fn invalidate_superseded_compaction_floor(
        &self,
        identity: &AgentIdentity,
        expected: &CapturedIncarnation,
        floors: &super::bridge::CompactionFloorRegistry,
        operation_id: &str,
    ) {
        let lock = self.lifecycle_lock_for(identity).await;
        let _guard = lock.lock().await;
        if !matches!(self.capture_incarnation(identity).await, Ok(current) if current == *expected)
        {
            floors.clear_owned(identity, operation_id);
        }
    }

    /// Section 8.4 trigger (b): distill the outgoing session's tail BEFORE
    /// the member retires, then queue the retired identity's store for the
    /// next dream's harvest (section 8.5 exit interview). Best-effort and
    /// bounded: retirement proceeds at the distiller's pre-rotation timeout.
    async fn distill_before_retire(
        &self,
        identity: &AgentIdentity,
        session_id: Option<&SessionId>,
    ) {
        let Some(injector) = self.agent_memory.read().await.clone() else {
            return;
        };
        if let Some(session_id) = session_id {
            injector
                .distill_before_rotation(
                    identity,
                    &session_id.to_string(),
                    crate::memory::distiller::DistillCause::Retire,
                )
                .await;
        }
        // Section 8.5 exit interview: queue the retired identity's store for
        // the next dream's harvest sub-phase.
        injector
            .note_identity_retired(
                identity,
                session_id.map(std::string::ToString::to_string).as_deref(),
                "retire",
            )
            .await;
    }

    /// Retire the identity's Core member while the caller holds the lifecycle
    /// lock. Core keeps the member's history, so a later wake resumes
    /// the same session at a new generation. `Retire` is the ordinary path;
    /// `Reload` is the first half of a non-destructive reload and skips the
    /// memory harvest hooks because the same session is woken right after
    /// (nothing rotates, no exit).
    async fn retire_locked_with_intent(
        &self,
        identity: &AgentIdentity,
        intent: LifecycleRetireIntent,
    ) -> Result<(), IdentityRuntimeError> {
        // A host-adopted identity is read-only to Meta.
        self.refuse_host_adopted(identity).await?;
        let Some(live) = self.core_live_member(identity).await? else {
            let wake_refusal = self
                .entries
                .read()
                .await
                .get(identity)
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?
                .wake_refusal
                .clone();
            return Err(IdentityRuntimeError::InvalidState {
                identity: identity.clone(),
                state: Self::view_state(false, wake_refusal.as_ref()),
                operation: "retire",
            });
        };
        self.clear_wake_notes_of_live_member(identity).await;

        // Section 8.4 trigger (b), and the exit interview. A reload keeps
        // the session, so neither applies.
        if intent == LifecycleRetireIntent::Retire {
            self.distill_before_retire(identity, live.session_id.as_ref())
                .await;
        }

        self.core_bridge(identity)?
            .retire_member(&live.agent_runtime_id)
            .await
            .map_err(|err| IdentityRuntimeError::Internal(format!("bridge retire: {err}")))?;
        self.entries.notify_binding_changed();
        self.mark_bootstrap_from_lifecycle(identity, IdentityLifecycleState::Dormant, None);
        self.emit_event(
            identity,
            IdentityEvent::StateChanged {
                identity: identity.clone(),
                new_state: IdentityLifecycleState::Dormant,
            },
        )
        .await;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Lifecycle: reload_member (non-destructive cold reload, a 2026-09-04 production incident)
    // -----------------------------------------------------------------------

    /// Record every completed reload attempt, including early no-ops/refusals.
    async fn reload_locked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberReloadOutcome, IdentityRuntimeError> {
        let result = self.reload_locked_inner(identity).await;
        match &result {
            Ok(outcome) => self.record_reload_outcome(identity, outcome).await,
            Err(error) => self.record_reload_failure(identity, error).await,
        }
        result
    }

    /// Non-destructive cold reload of the identity's live member: same
    /// session, same Core incarnation.
    ///
    /// Core's member registration reload (`reload_member_registration`)
    /// quiesces, discards and re-registers the executor for the SAME session
    /// and can tell a healthy registration apart (`not_degraded`). A bridge
    /// without that primitive falls back to retire then wake under the
    /// identity's lifecycle lock: the same session is resumed and Core's
    /// generation advances.
    ///
    /// Contrast reset and `delete_identity`: a fresh session. This verb never
    /// starts one. An identity Core holds no member for returns `not_current`
    /// (the next send wakes it), except one whose last wake Core held for an
    /// audited-endpoint repair: reload is the operator's path after that
    /// repair, and wakes it onto the same session. A host-adopted identity
    /// refuses typed.
    async fn reload_locked_inner(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberReloadOutcome, IdentityRuntimeError> {
        let wake_refusal = self
            .entries
            .read()
            .await
            .get(identity)
            .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?
            .wake_refusal
            .clone();
        self.refuse_host_adopted(identity).await?;
        let Some(live) = self.core_live_member(identity).await? else {
            if wake_refusal.as_ref().is_some_and(|refusal| {
                refusal.remedy == super::types::WakeRefusalRemedy::RepairSession
            }) {
                return self.reload_repaired_session_locked(identity).await;
            }
            let last = self.core_last_binding(identity).await?;
            return Ok(MemberReloadOutcome {
                reloaded: false,
                disposition: MemberReloadDisposition::NotCurrent,
                session_id: last
                    .as_ref()
                    .and_then(|binding| binding.session_id().cloned()),
                generation: last
                    .as_ref()
                    .map(|binding| ContinuityGeneration::new(binding.generation().get())),
            });
        };
        let Some(session_id) = live.session_id.clone() else {
            // A peer-only member has no session registration to reload.
            return Ok(MemberReloadOutcome {
                reloaded: false,
                disposition: MemberReloadDisposition::NotCurrent,
                session_id: None,
                generation: None,
            });
        };
        let generation = self
            .core_live_scope(identity)
            .await?
            .map(|scope| ContinuityGeneration::new(scope.generation().get()));

        let bridge = Arc::clone(self.core_bridge(identity)?);
        // A `not_current` verdict is a no-op, not authority to retire or
        // replace the member.
        let registration_reload = match bridge
            .reload_member_registration(&live.agent_runtime_id)
            .await
        {
            Ok(reload) => reload,
            // Typed audited-endpoint divergence: the registration is retained,
            // but no reload can read the document. Refuse with the exact
            // repair commands instead of a store-not-healthy refusal that
            // invites the retry storm.
            Err(error @ BridgeError::SessionRepairRequired { .. }) => {
                let refusal = classify_wake_failure(identity, Some(&session_id), error);
                self.record_wake_refusal(identity, &refusal).await;
                return Err(refusal);
            }
            Err(error) => return Err(reload_phase_error(identity, error)),
        };
        if let Some(reload) = registration_reload {
            if reload.session_id != session_id {
                return Err(IdentityRuntimeError::Internal(format!(
                    "reload_member registration session {} differs from Core's live session {}",
                    reload.session_id, session_id,
                )));
            }
            tracing::info!(
                %identity,
                %session_id,
                disposition = ?reload.disposition,
                registration_generation = reload.registration_generation,
                "reload_member: meerkat member registration reload completed \
                 (non-destructive; the incarnation does not change)"
            );
            return Ok(MemberReloadOutcome {
                reloaded: matches!(
                    reload.disposition,
                    MemberReloadDisposition::Discarded | MemberReloadDisposition::Reattached
                ),
                disposition: reload.disposition,
                session_id: Some(session_id),
                generation,
            });
        }

        tracing::info!(
            %identity,
            %session_id,
            "reload_member: retiring the live member and waking it onto the same session \
             (non-destructive; Core's generation advances)"
        );
        self.retire_locked_with_intent(identity, LifecycleRetireIntent::Reload)
            .await?;
        let outcome = self.wake_onto_last_session_locked(identity).await?;
        Ok(outcome)
    }

    /// `reload_member` for an identity whose last wake Core held for the
    /// sanctioned audited-endpoint repair (`WakeHeld`, remedy
    /// `repair_session`): the operator has (presumably) run the repair and
    /// asks for the same durable session back. The wake door resumes it; a
    /// document still refused records the hold again, and that typed
    /// refusal is what the operator sees.
    async fn reload_repaired_session_locked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberReloadOutcome, IdentityRuntimeError> {
        tracing::info!(
            %identity,
            "reload_member: re-attempting the resume of a session held for the sanctioned \
             audited-endpoint repair"
        );
        self.clear_materialization_backoff(identity).await;
        self.wake_onto_last_session_locked(identity).await
    }

    /// Wake `identity` through the one wake door under the caller's
    /// lifecycle lock and report it as a completed reload.
    async fn wake_onto_last_session_locked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberReloadOutcome, IdentityRuntimeError> {
        let before = self.core_last_binding(identity).await?;
        let mut bound_generation = None;
        let result = self
            .embody_identity_locked(
                identity,
                None,
                None,
                None,
                &mut bound_generation,
                EmbodimentOverrides::default(),
            )
            .await;
        self.mark_bootstrap_materialization_finished(identity, &result, bound_generation);
        let outcome = result?;
        let scope = self.core_live_scope(identity).await?;
        let session_id = outcome.binding.session_id.clone();
        if let (Some(before), Some(after)) = (
            before.as_ref().and_then(|binding| binding.session_id()),
            session_id.as_ref(),
        ) && before != after
        {
            tracing::warn!(
                %identity,
                before = %before,
                after = %after,
                "reload_member: Core holds no durable session for the last session, so the wake \
                 spawned a fresh one; the identity runs on the new session"
            );
        }
        Ok(MemberReloadOutcome {
            reloaded: true,
            disposition: MemberReloadDisposition::Discarded,
            session_id,
            generation: scope.map(|scope| ContinuityGeneration::new(scope.generation().get())),
        })
    }

    fn unix_ms_now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    async fn record_reload_outcome(&self, identity: &AgentIdentity, outcome: &MemberReloadOutcome) {
        let record = ReloadAttemptRecord {
            outcome: match outcome.disposition {
                MemberReloadDisposition::Discarded => ReloadAttemptOutcome::Discarded,
                MemberReloadDisposition::NotDegraded => ReloadAttemptOutcome::NotDegraded,
                MemberReloadDisposition::NotCurrent => ReloadAttemptOutcome::NotCurrent,
                MemberReloadDisposition::Reattached => ReloadAttemptOutcome::Reattached,
            },
            detail: None,
            data: None,
            at_unix_ms: Self::unix_ms_now(),
        };
        if let Some(entry) = self.entries.write().await.get_mut(identity) {
            entry.last_reload = Some(record);
        }
    }

    async fn record_reload_failure(&self, identity: &AgentIdentity, error: &IdentityRuntimeError) {
        let (outcome, detail) = match error {
            IdentityRuntimeError::ReloadRefused { reason, .. } => {
                (ReloadAttemptOutcome::Refused, reason.clone())
            }
            IdentityRuntimeError::ReloadTimedOut { stage, .. } => {
                (ReloadAttemptOutcome::TimedOut, stage.clone())
            }
            IdentityRuntimeError::ActorAdmissionTimeout { .. } => {
                (ReloadAttemptOutcome::TimedOut, error.to_string())
            }
            IdentityRuntimeError::WakeHeld {
                hold:
                    WakeHold::DurableResume(
                        meerkat_core::service::DurableResumeHold::AuditedEndpointDivergence,
                    ),
                detail,
                ..
            } => (ReloadAttemptOutcome::RepairRequired, detail.clone()),
            other => (ReloadAttemptOutcome::Failed, other.to_string()),
        };
        let record = ReloadAttemptRecord {
            outcome,
            detail: Some(detail),
            data: error.structured_data(),
            at_unix_ms: Self::unix_ms_now(),
        };
        if let Some(entry) = self.entries.write().await.get_mut(identity) {
            entry.last_reload = Some(record);
        }
    }

    /// The delivery path's single automatic reload after a typed
    /// reload-required refusal. Runs under the caller's lifecycle lock. Any
    /// failure becomes the typed `ReloadRequired` carrying both reasons.
    async fn reload_for_delivery_locked(
        &self,
        identity: &AgentIdentity,
        refusal: &IdentityRuntimeError,
    ) -> Result<(), IdentityRuntimeError> {
        tracing::warn!(
            %identity,
            refusal = %refusal,
            "delivery refused: member requires a cold reload; running ONE automatic \
             non-destructive reload_member before retrying"
        );
        match self.reload_locked(identity).await {
            Ok(outcome) => {
                tracing::info!(
                    %identity,
                    disposition = ?outcome.disposition,
                    "automatic reload_member completed; retrying the delivery once"
                );
                Ok(())
            }
            // The store is not healthy yet, or the bounded reload timed out:
            // keep the class (retry-later vs inspect) instead of folding it
            // into a generic reload-required failure.
            Err(
                typed @ (IdentityRuntimeError::ReloadRefused { .. }
                | IdentityRuntimeError::ReloadTimedOut { .. }
                | IdentityRuntimeError::ActorAdmissionTimeout { .. }
                | IdentityRuntimeError::ActorLoopStalled { .. }
                | IdentityRuntimeError::ActorTerminated { .. }),
            ) => Err(typed),
            Err(reload_error) => Err(IdentityRuntimeError::ReloadRequired {
                identity: identity.clone(),
                reason: format!("{refusal}; automatic reload_member failed: {reload_error}"),
            }),
        }
    }

    /// Public entry for the reload verb (in-process callers and tests).
    pub async fn reload_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberReloadOutcome, IdentityRuntimeError> {
        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        self.reload_locked(identity).await
    }

    /// Cancellation-safe reload for RPC/console boundaries that atomically
    /// rejects a stale generated runtime alias under the same lifecycle lock
    /// as the mutation.
    pub async fn reload_member_alias_tracked(
        self: &Arc<Self>,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
    ) -> Result<MemberReloadOutcome, IdentityRuntimeError> {
        let runtime = Arc::clone(self);
        let identity = identity.clone();
        let expected_alias = expected_alias.map(str::to_owned);
        self.run_tracked_foreground(async move {
            let lifecycle_lock = runtime.lifecycle_lock_for(&identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            if let Some(expected_alias) = expected_alias.as_deref()
                && let Err(error) = runtime
                    .ensure_expected_member_alias_current(&identity, expected_alias)
                    .await
            {
                runtime.record_reload_failure(&identity, &error).await;
                return Err(error);
            }
            runtime.reload_locked(&identity).await
        })
        .await
    }

    // -----------------------------------------------------------------------
    // Health: member_health (in-process reads only; answers during a stall)
    // -----------------------------------------------------------------------

    /// Install the unified runtime's actor-loop probe verdict and forward it
    /// to the session bridge, whose admission path fails fast while a stall
    /// is open.
    /// Install the observer told when a live member's session is rebound
    /// to a new session id (see [`MemberSessionRotationObserver`]).
    pub(crate) fn install_session_rotation_observer(
        &self,
        observer: Arc<dyn MemberSessionRotationObserver>,
    ) {
        *self
            .session_rotation_observer
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(observer);
    }

    pub fn install_actor_loop_health(&self, health: Arc<ActorLoopHealth>) {
        if let Some(bridge) = self.bridge.as_ref() {
            bridge.observe_actor_loop_health(Arc::clone(&health));
        }
        *self
            .actor_loop_health
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(health);
    }

    /// The installed probe verdict, if any.
    pub fn actor_loop_health(&self) -> Option<Arc<ActorLoopHealth>> {
        self.actor_loop_health
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// The identity's lifecycle and delivery health. Its Core facts come from
    /// Core's member history (a store read) and the rest is in-process: this
    /// never touches the mob actor, so it answers while the loop is stalled,
    /// which is exactly when an operator needs it.
    pub async fn member_health(
        &self,
        identity: &AgentIdentity,
    ) -> Result<MemberHealthReport, IdentityRuntimeError> {
        let (last_delivery_error, last_reload, wake_refusal) = {
            let entries = self.entries.read().await;
            let entry = entries
                .get(identity)
                .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?;
            (
                entry.last_delivery_error.clone(),
                entry.last_reload.clone(),
                entry.wake_refusal.clone(),
            )
        };
        let history = self.core_view_history().await.map_err(|error| {
            IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!("reading Core's member history: {error}"),
            }
        })?;
        let facts = CoreViewFacts::of(history.as_ref(), identity);
        let broken = self.core_broken_members().await;
        let bootstrap_state = self
            .identity_bootstrap_status()
            .identities
            .get(identity)
            .map(|entry| entry.state);
        let actor_loop = self
            .actor_loop_health()
            .map(|health| health.report())
            .unwrap_or_else(ActorLoopHealthReport::unobserved);
        let open_stall_id = (actor_loop.state == ActorLoopHealthKind::Stalled)
            .then_some(actor_loop.stall_id)
            .flatten();
        let mut report = MemberHealthReport {
            identity: identity.clone(),
            state: self.core_view_state(identity, &facts, &broken, wake_refusal.as_ref()),
            member_id: facts.runtime_id,
            bootstrap_state,
            materialization_in_flight: bootstrap_state == Some(IdentityBootstrapState::Warming),
            session_id: facts.session_id,
            generation: facts.generation,
            last_delivery_error,
            last_reload,
            actor_loop,
            open_stall_id,
            durability: None,
            session_repair_required: self.session_repair_hold(wake_refusal.as_ref()),
            wake_refusal,
        };
        // Custom bridges may await I/O; no lock is held across it.
        if facts.live
            && let (Some(bridge), Some(session_id)) =
                (self.bridge.as_ref(), report.session_id.as_ref())
        {
            report.durability = bridge.member_durability(session_id).await;
        }
        Ok(report)
    }

    fn delivery_error_class(err: &IdentityRuntimeError) -> DeliveryErrorClass {
        match err {
            IdentityRuntimeError::ReloadRequired { .. } => DeliveryErrorClass::ReloadRequired,
            IdentityRuntimeError::ActorLoopStalled { .. } => DeliveryErrorClass::ActorLoopStalled,
            IdentityRuntimeError::ActorTerminated { .. } => DeliveryErrorClass::ActorTerminated,
            IdentityRuntimeError::AdmissionBacklogFull { .. } => {
                DeliveryErrorClass::AdmissionBacklogFull
            }
            IdentityRuntimeError::ReloadRefused { .. } => DeliveryErrorClass::ReloadRefused,
            IdentityRuntimeError::ReloadTimedOut { .. } => DeliveryErrorClass::ReloadTimedOut,
            IdentityRuntimeError::ActorAdmissionTimeout { .. } => {
                DeliveryErrorClass::AdmissionTimeout
            }
            IdentityRuntimeError::CompletionFailed { .. }
            | IdentityRuntimeError::PostAdmissionResolutionFailed { .. }
            | IdentityRuntimeError::PostAdmissionSuperseded { .. } => {
                DeliveryErrorClass::Completion
            }
            _ => DeliveryErrorClass::AdmissionFailed,
        }
    }

    async fn record_delivery_error(&self, identity: &AgentIdentity, err: &IdentityRuntimeError) {
        let record = DeliveryErrorRecord {
            class: Self::delivery_error_class(err),
            detail: err.to_string(),
            data: err.structured_data(),
            at_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
                .unwrap_or(0),
        };
        if let Some(entry) = self.entries.write().await.get_mut(identity) {
            entry.last_delivery_error = Some(record);
            if let IdentityRuntimeError::IdentitySessionAbsent { session_id, .. } = err {
                entry.absent_delivery_session = Some(session_id.clone());
            }
        }
    }

    async fn record_delivery_success(&self, identity: &AgentIdentity) {
        if let Some(entry) = self.entries.write().await.get_mut(identity)
            && entry.last_delivery_error.is_some()
        {
            entry.last_delivery_error = None;
        }
    }

    // -----------------------------------------------------------------------
    // Lifecycle: respawn() - REQ-09
    // -----------------------------------------------------------------------

    /// Respawn: non-destructive recovery onto the same session at a new Core
    /// generation (see [`Self::respawn_locked`]).
    pub async fn respawn(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        self.respawn_with_expected_member_alias(identity, None)
            .await
    }

    async fn respawn_with_expected_member_alias(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        if let Some(expected_alias) = expected_alias {
            self.ensure_expected_member_alias_current(identity, expected_alias)
                .await?;
        }
        self.respawn_locked(identity).await
    }

    /// Respawn: retire the identity's Core member, then wake it
    /// onto the same session (Core's `Resume` from the member history) in one
    /// verb under the lifecycle lock. The transcript is kept and Core's
    /// generation ADVANCES, so a delivery scope captured before the respawn
    /// refuses as stale. A crash between the two halves leaves the identity
    /// retired with its history; the next wake resumes it. An identity Core
    /// holds no member for is simply woken.
    async fn respawn_locked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        // An identity this runtime does not hold is refused before Core's
        // member is touched: the wake below would refuse it only after the
        // retire had already happened.
        if !self.entries.read().await.contains_key(identity) {
            return Err(IdentityRuntimeError::UnknownIdentity(identity.clone()));
        }
        self.refuse_host_adopted(identity).await?;
        if let Some(live) = self.core_live_member(identity).await? {
            // The retire below ends the member, and the wake after it resumes
            // the session from what Core keeps of it. When Core keeps nothing
            // beyond the member (sessions in memory only), nothing could
            // resume it again: refuse before anything is retired, as the
            // bridge's own delivery repair does.
            if let Some(session_id) = live.session_id.as_ref() {
                self.refuse_respawn_without_durable_copy(identity, session_id)
                    .await?;
            }
            self.clear_wake_notes_of_live_member(identity).await;
            // Section 8.4 trigger (b): respawn is a recovery boundary; harvest the
            // session's window before the member is replaced (the session
            // does not rotate, so the cursor stays valid). Bounded.
            let injector = self.agent_memory.read().await.clone();
            if let Some(injector) = injector
                && let Some(scope) = self.core_live_scope(identity).await?
            {
                let session_key = scope.session_id().to_string();
                injector.note_session_generation(identity, &session_key, scope.generation().get());
                injector
                    .distill_before_rotation(
                        identity,
                        &session_key,
                        crate::memory::distiller::DistillCause::Respawn,
                    )
                    .await;
            }
            self.core_bridge(identity)?
                .retire_member(&live.agent_runtime_id)
                .await
                .map_err(|err| {
                    IdentityRuntimeError::Internal(format!("bridge retire before respawn: {err}"))
                })?;
            self.entries.notify_binding_changed();
        }
        let mut generation = None;
        let result = self
            .embody_identity_locked(
                identity,
                None,
                None,
                None,
                &mut generation,
                EmbodimentOverrides::default(),
            )
            .await;
        self.mark_bootstrap_materialization_finished(identity, &result, generation);
        result?;
        self.core_live_scope(identity).await
    }

    /// Refuse a respawn whose retire would end the only copy of the live
    /// member's session: Core keeps it nowhere beyond the member (sessions
    /// in memory only). A bridge that cannot say keeps the retire-then-wake
    /// path; one that fails to answer refuses retryable. Nothing is retired
    /// either way.
    async fn refuse_respawn_without_durable_copy(
        &self,
        identity: &AgentIdentity,
        session_id: &SessionId,
    ) -> Result<(), IdentityRuntimeError> {
        match self
            .core_bridge(identity)?
            .session_outlives_member(session_id)
            .await
        {
            Ok(true) | Err(BridgeError::Unsupported { .. }) => Ok(()),
            Ok(false) => Err(IdentityRuntimeError::InvalidState {
                identity: identity.clone(),
                state: IdentityLifecycleState::Active,
                operation: "respawn a member whose session lives only in memory",
            }),
            Err(error) => Err(IdentityRuntimeError::IdentityWakeRetryable {
                identity: identity.clone(),
                session_id: Some(session_id.clone()),
                detail: format!(
                    "the respawn could not tell whether session {session_id} outlives its \
                     member ({error}); nothing was retired"
                ),
            }),
        }
    }

    /// A delivery landed on `delivered_session_id`. When that is not the
    /// session the delivery was admitted under (`admitted_session_id`, from
    /// the Core scope read before admission), Core rotated the member's
    /// session (a delivery repair): tell the session rotation observer and
    /// return Core's fence for the rotated binding. No Meta record is
    /// rebound: every view reads Core.
    async fn reconcile_delivered_session_locked(
        &self,
        identity: &AgentIdentity,
        admitted_session_id: Option<&SessionId>,
        delivered_session_id: SessionId,
    ) -> Result<Option<FencingToken>, IdentityRuntimeError> {
        let Some(previous_session_id) = admitted_session_id else {
            return Ok(None);
        };
        if *previous_session_id == delivered_session_id {
            return Ok(None);
        }
        tracing::warn!(
            %identity,
            old_session_id = %previous_session_id,
            new_session_id = %delivered_session_id,
            "identity bridge delivery returned a rotated session"
        );
        let observer = self
            .session_rotation_observer
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(observer) = observer {
            // The observer keys the member by its roster row, the encoded
            // DURABLE identity; the runtime alias names only one incarnation.
            observer
                .member_session_rotated(
                    &crate::member_comms_id::roster_member_id_for_identity(identity.as_str()),
                    previous_session_id,
                    &delivered_session_id,
                )
                .await;
        }
        self.entries.notify_binding_changed();
        Ok(self
            .core_live_scope(identity)
            .await?
            .map(|scope| FencingToken::new(scope.fence_token().get())))
    }

    // -----------------------------------------------------------------------
    // Lifecycle: reset() - REQ-10
    // -----------------------------------------------------------------------

    /// Reset: a destructive fresh start. Core mints a fresh session at
    /// a higher generation under the CURRENT roster spec; the old session
    /// stays in Core as history and the identity never resumes it again.
    /// Liveness is kept: a live identity gets its successor, a dormant one
    /// gets a fresh spawn that is retired at once. Returns Core's binding of
    /// the fresh life. A host-adopted identity refuses typed.
    pub async fn reset(
        &self,
        identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberBinding, IdentityRuntimeError> {
        self.reset_with_expected_member_alias(identity, None).await
    }

    async fn reset_with_expected_member_alias(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
    ) -> Result<meerkat_mob::MemberBinding, IdentityRuntimeError> {
        let mut foreground_shutdown = self.foreground_cancel.subscribe();
        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        if let Some(expected_alias) = expected_alias {
            self.ensure_expected_member_alias_current(identity, expected_alias)
                .await?;
        }
        // Before anything acts: a host-adopted identity is read-only.
        self.refuse_host_adopted(identity).await?;
        self.refuse_pending_first_boot_row(identity, "reset")
            .await?;
        // Re-profile first so the fresh session uses the current roster spec.
        self.adopt_current_roster_spec_for_reset(identity).await?;
        // The identity's previous life is Core's last binding only when an
        // identity-first spawn made it, as for the wake: a member the host
        // made directly on the mob under the identity's member id, since
        // retired, is not the identity's, so its session is never captured
        // into the identity's memory.
        let previous = self
            .core_last_binding(identity)
            .await?
            .filter(|binding| binding_spawned_for_identity(binding, identity));
        let live = self
            .start_fresh_life_locked(identity, &mut foreground_shutdown, "reset", None)
            .await?;

        // The fresh life is committed in Core from here. The bookkeeping that
        // needs only the old binding runs first, so a failed read of the
        // fresh binding below never leaves it undone or invites a caller to
        // rotate a second time.
        //
        // Section 10.1: reset is the deliberate clean-slate boundary. Clear session
        // taint explicitly (rotation clears implicitly; this also drops
        // pending pre-attribution taint) and mark the outgoing boundary
        // synchronously; a runtime-owned task distills the outgoing session,
        // which Core keeps, so the reset stays latency-neutral. The fresh
        // life always runs on a fresh session, so the old one is outgoing.
        let injector = self.agent_memory.read().await.clone();
        if let Some(injector) = injector.as_ref() {
            injector.clear_taint_for_identity(identity);
            if let Some(previous) = previous.as_ref()
                && let Some(old_session_id) = previous.session_id()
            {
                let capture = PendingResetMemoryCapture {
                    injector: injector.clone(),
                    identity: identity.clone(),
                    session_key: old_session_id.to_string(),
                    generation: previous.generation().get(),
                };
                injector.note_reset_boundary(&capture.session_key);
                injector.note_session_generation(
                    identity,
                    &capture.session_key,
                    capture.generation,
                );
                self.spawn_reset_memory_capture(capture).await;
            }
        }
        self.settle_fresh_life(identity, live, None).await;
        let fresh = self.committed_fresh_binding(identity, "reset").await?;
        if let (Some(injector), Some(session_id)) = (injector.as_ref(), fresh.session_id()) {
            injector.note_session_generation(
                identity,
                &session_id.to_string(),
                fresh.generation().get(),
            );
        }
        tracing::debug!(
            %identity,
            generation = fresh.generation().get(),
            session_id = ?fresh.session_id(),
            live,
            "reset completed",
        );
        Ok(fresh)
    }

    /// Core's binding of the fresh life `operation` just committed, read
    /// once more on a failed read. If Core still cannot be read, the answer
    /// says the fresh life IS committed and is not retryable: repeating the
    /// verb would start another fresh life.
    async fn committed_fresh_binding(
        &self,
        identity: &AgentIdentity,
        operation: &'static str,
    ) -> Result<meerkat_mob::MemberBinding, IdentityRuntimeError> {
        let mut failure = String::new();
        for _ in 0..2 {
            match self.core_last_binding(identity).await {
                Ok(Some(binding)) => return Ok(binding),
                Ok(None) => {
                    failure = "Core holds no binding of it".to_string();
                    break;
                }
                Err(error) => failure = error.to_string(),
            }
        }
        Err(IdentityRuntimeError::Internal(format!(
            "identity {identity}: the {operation}'s fresh life is committed in Core, but its \
             binding could not be read ({failure}); read the identity's status, do not repeat \
             the {operation}"
        )))
    }

    /// Start `identity`'s fresh life on Core while the caller holds the
    /// lifecycle lock (reset, `delete_identity`): Core mints a fresh session
    /// at a higher generation, and the old one stays in Core's store as
    /// history. Liveness is kept. A live member is replaced by its
    /// successor in ONE Core transition (`reset_member_to_successor`, never
    /// composed from retire plus create); an identity Core holds no member
    /// for gets a fresh spawn that is retired in the same verb, so it never
    /// becomes live as a side effect. `queued` is the old member's captured
    /// queue, admitted to the successor as soon as Core commits it. Returns
    /// whether the identity is live afterwards.
    async fn start_fresh_life_locked(
        &self,
        identity: &AgentIdentity,
        foreground_shutdown: &mut watch::Receiver<bool>,
        operation: &'static str,
        queued: Option<super::bridge::QueuedMemberInputs>,
    ) -> Result<bool, IdentityRuntimeError> {
        // An identity a held pass did not publish owes its host a fresh held
        // completion. A fresh life outside a held pass that covers it would
        // have Core bind a member that completion never saw, and after a
        // restart that binding would read as the pass's publication and drop
        // the mark; so it is refused typed before Core is asked anything.
        self.load_held_completion_marks().await?;
        if self.held_completion_owed(identity)
            && self
                .held_create_pass()
                .is_none_or(|pass| !pass.covers(identity))
        {
            return Err(IdentityRuntimeError::HeldCompletionOwed {
                identity: identity.clone(),
            });
        }
        let bridge = Arc::clone(self.core_bridge(identity)?);
        let spec = self
            .entries
            .read()
            .await
            .get(identity)
            .ok_or_else(|| IdentityRuntimeError::UnknownIdentity(identity.clone()))?
            .spec
            .clone();
        let live = self.core_live_member(identity).await?.is_some();
        if live {
            self.clear_wake_notes_of_live_member(identity).await;
        }
        let (draft, customizer) = self
            .fresh_life_draft(identity, &spec, foreground_shutdown, operation)
            .await?;

        if live {
            let successor = bridge
                .reset_member_to_successor(identity, &spec, &draft)
                .await
                .map_err(|error| {
                    if let Some(queued) = queued.as_ref().filter(|queued| queued.carryable() > 0) {
                        tracing::error!(
                            %identity,
                            lost = queued.carryable(),
                            %error,
                            "Core's successor respawn failed; the queued inputs captured before \
                             it are lost if Core retired the member"
                        );
                    }
                    IdentityRuntimeError::Internal(format!(
                        "{operation} of identity {identity}: Core's successor respawn failed: \
                         {error}"
                    ))
                })?;
            // The customizer that built the successor is told of its session
            // with the lifecycle lock released, in a runtime-owned task.
            if let (Some(customizer), Some(session_id)) =
                (customizer, successor.session_id.as_ref())
            {
                let notice = AfterCreateNotice {
                    session_id: session_id.clone(),
                    context: self.session_created_context(&spec, &draft),
                };
                self.spawn_after_create(customizer, identity.clone(), notice)
                    .await;
            }
            // The captured queue goes to the fresh member before anything
            // reads the binding back.
            if let Some(queued) = queued {
                match successor.session_id.as_ref() {
                    Some(session_id) => {
                        bridge
                            .readmit_queued_member_inputs(identity, session_id, queued)
                            .await;
                    }
                    None if queued.carryable() > 0 => tracing::error!(
                        %identity,
                        lost = queued.carryable(),
                        "the fresh member has no session to admit the captured queued inputs \
                         to; they are lost"
                    ),
                    None => {}
                }
            }
        } else {
            if let Some(queued) = queued.as_ref().filter(|queued| queued.carryable() > 0) {
                tracing::error!(
                    %identity,
                    lost = queued.carryable(),
                    "the member left Core before its fresh life; the queued inputs captured from \
                     it have no member to go to and are lost"
                );
            }
            let created = bridge
                .create_session(identity, &spec, &draft)
                .await
                .map_err(|error| match deterministic_build_rejection_reason(&error) {
                    Some(reason) => IdentityRuntimeError::HostRejectedBuild {
                        identity: identity.clone(),
                        reason,
                    },
                    None => IdentityRuntimeError::Internal(format!(
                        "{operation} of identity {identity}: Core's fresh spawn failed: {error}"
                    )),
                })?;
            if let Err(error) = bridge.retire_member(&created.agent_runtime_id).await {
                self.entries.notify_binding_changed();
                return Err(IdentityRuntimeError::Internal(format!(
                    "{operation} of identity {identity}: Core committed the fresh session, but \
                     retiring it to keep the identity dormant failed ({error}); the identity is \
                     live until it is retired"
                )));
            }
        }
        self.entries.notify_binding_changed();
        Ok(live)
    }

    /// The build draft of a fresh life (reset, `delete_identity`): the base
    /// draft plus the installed customizer, with the customizer that ran, if
    /// any. A foreground shutdown cancels it before Core is asked for
    /// anything.
    async fn fresh_life_draft(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        foreground_shutdown: &mut watch::Receiver<bool>,
        operation: &'static str,
    ) -> Result<(AgentBuildDraft, Option<Arc<dyn AgentCustomizer>>), IdentityRuntimeError> {
        let mut draft = Self::base_build_draft(spec);
        let Some(customizer) = self.customizer.read().await.clone() else {
            return Ok((draft, None));
        };
        let cancelled = || {
            IdentityRuntimeError::Internal(format!(
                "identity {operation} cancelled during shutdown before Core was asked for a \
                 fresh session"
            ))
        };
        if *foreground_shutdown.borrow() {
            return Err(cancelled());
        }
        let active_peers = self.entries.read().await.keys().cloned().collect();
        let managed_edges = self.desired_peer_edges.read().await.clone();
        let build_context = AgentBuildContext {
            identity: identity.clone(),
            active_peers,
            managed_edges,
            runtime_services: self.runtime_services(),
        };
        let customize_result = {
            let customize = customizer.customize_build(&build_context, spec, &mut draft);
            tokio::pin!(customize);
            tokio::select! {
                result = &mut customize => result,
                changed = foreground_shutdown.changed() => {
                    if changed.is_ok() && *foreground_shutdown.borrow() {
                        return Err(cancelled());
                    }
                    customize.await
                }
            }
        };
        if let Err(err) = customize_result {
            if let super::types::CustomizerError::ToolArtifactRefused(refusal) = err {
                return Err(IdentityRuntimeError::ToolArtifactRefused(refusal));
            }
            return Err(IdentityRuntimeError::Internal(format!(
                "customizer for {operation}: {err}"
            )));
        }
        self.publish_customizer_tools(identity, &mut draft).await;
        Ok((draft, Some(customizer)))
    }

    /// Settle the views after a fresh life (reset, `delete_identity`): a
    /// recorded refusal of the old session no longer describes the identity.
    async fn settle_fresh_life(
        &self,
        identity: &AgentIdentity,
        live: bool,
        replaced_session: Option<super::types::ReplacedSession>,
    ) {
        if live {
            self.settle_successful_wake(identity, replaced_session)
                .await;
            self.mark_bootstrap_from_lifecycle(identity, IdentityLifecycleState::Active, None);
            return;
        }
        if let Some(entry) = self.entries.write().await.get_mut(identity) {
            entry.wake_refusal = None;
            entry.replaced_session = replaced_session;
            entry.absent_delivery_session = None;
        }
        self.clear_materialization_backoff(identity).await;
        self.mark_bootstrap_from_lifecycle(identity, IdentityLifecycleState::Dormant, None);
        self.emit_event(
            identity,
            IdentityEvent::StateChanged {
                identity: identity.clone(),
                new_state: IdentityLifecycleState::Dormant,
            },
        )
        .await;
    }

    // -----------------------------------------------------------------------
    // Lifecycle: delete_identity() - REQ-11
    // -----------------------------------------------------------------------

    /// Delete an identity's continuity: its next life starts fresh
    /// at once, and no "deleted" mark is stored (Core's last binding is the
    /// fresh session). Liveness is kept, as for [`Self::reset`]. The old
    /// session stays in Core's store as history; the identity never resumes
    /// it again. `None` when Core never spawned the identity: there is
    /// nothing to delete. A host-adopted identity refuses typed.
    pub async fn delete_identity(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, IdentityRuntimeError> {
        self.delete_identity_with_expected_member_alias(identity, None)
            .await
    }

    async fn delete_identity_with_expected_member_alias(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
    ) -> Result<Option<meerkat_mob::MemberBinding>, IdentityRuntimeError> {
        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        if let Some(expected_alias) = expected_alias {
            self.ensure_expected_member_alias_current(identity, expected_alias)
                .await?;
        }
        self.delete_identity_locked(identity).await
    }

    async fn delete_identity_locked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, IdentityRuntimeError> {
        // Before anything acts: a host-adopted identity is read-only.
        self.refuse_host_adopted(identity).await?;
        if !self.entries.read().await.contains_key(identity) {
            return Err(IdentityRuntimeError::UnknownIdentity(identity.clone()));
        }
        self.refuse_pending_first_boot_row(identity, "delete_identity")
            .await?;
        // Only a binding an identity-first spawn made is the identity's last
        // life, as for the wake. A member the host made directly on the mob
        // under the identity's member id, since retired, answers as an
        // identity Core never spawned: nothing of its session is distilled,
        // dropped or interviewed.
        let Some(previous) = self
            .core_last_binding(identity)
            .await?
            .filter(|binding| binding_spawned_for_identity(binding, identity))
        else {
            return Ok(None);
        };
        let previous_session_key = previous.session_id().map(ToString::to_string);

        // Section 8.4 trigger (b): delete is the identity's LAST boundary - harvest
        // the outgoing session before its fresh life starts. Bounded; the
        // delete proceeds at the pre-rotation timeout.
        let injector = self.agent_memory.read().await.clone();
        if let (Some(injector), Some(session_key)) =
            (injector.as_ref(), previous_session_key.as_ref())
        {
            injector
                .distill_before_rotation(
                    identity,
                    session_key,
                    crate::memory::distiller::DistillCause::Delete,
                )
                .await;
        }

        let mut foreground_shutdown = self.foreground_cancel.subscribe();
        let live = self
            .start_fresh_life_locked(identity, &mut foreground_shutdown, "delete", None)
            .await?;

        // The fresh life is committed in Core from here: the bookkeeping runs
        // before the fresh binding is read, so a failed read never leaves it
        // undone or invites a second rotation.
        if let Some(injector) = injector {
            if let Some(session_key) = previous_session_key.as_ref() {
                // Ask 2 GC: the identity never resumes the old session again,
                // and its knowledge was just distilled into the identity-keyed
                // Meta store (which the exit-interview harvest below reads, a
                // different store from the meerkat session-memory scope), so
                // reclaiming that scope frees dead re-embed weight without
                // starving any downstream read.
                injector
                    .drop_orphaned_session_scope(
                        session_key,
                        crate::memory::distiller::DistillCause::Delete,
                    )
                    .await;
            }
            // Section 8.5 exit interview (delete is the identity's LAST boundary).
            injector
                .note_identity_retired(identity, previous_session_key.as_deref(), "delete")
                .await;
        }

        // Subscriptions belong to the old life and end with it.
        self.event_channels.write().await.remove(identity);
        self.settle_fresh_life(identity, live, None).await;
        let fresh = self.committed_fresh_binding(identity, "delete").await?;
        Ok(Some(fresh))
    }

    // -----------------------------------------------------------------------
    // Checkpoint - REQ-14, REQ-15, REQ-16, REQ-17
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // Roster inspection - REQ-32
    // -----------------------------------------------------------------------

    /// Return every registered identity with its spec and status (one read
    /// of Core's member history serves them all; a failed read fails typed,
    /// as [`Self::statuses`]).
    pub async fn roster_inspect(
        &self,
    ) -> Result<BTreeMap<AgentIdentity, (DurableAgentSpec, IdentityStatus)>, IdentityRuntimeError>
    {
        let specs = self
            .entries
            .read()
            .await
            .iter()
            .map(|(identity, entry)| (identity.clone(), entry.spec.clone()))
            .collect::<Vec<_>>();
        let history = self.core_view_history().await.map_err(|error| {
            IdentityRuntimeError::MemberHistoryUnavailable {
                detail: format!("reading Core's member history: {error}"),
            }
        })?;
        let broken = self.core_broken_members().await;
        let mut result = BTreeMap::new();
        for (identity, spec) in specs {
            if let Ok(status) = self.status_from(&identity, history.as_ref(), &broken).await {
                result.insert(identity, (spec, status));
            }
        }
        Ok(result)
    }

    // -----------------------------------------------------------------------
    // Roster uniqueness validation - INV-06
    // -----------------------------------------------------------------------

    /// Validate that a roster contains no duplicate identities.
    pub fn validate_roster_uniqueness(
        specs: &[DurableAgentSpec],
    ) -> Result<(), IdentityRuntimeError> {
        let mut seen = std::collections::BTreeSet::new();
        for spec in specs {
            if !seen.insert(&spec.identity) {
                return Err(IdentityRuntimeError::DuplicateIdentity(
                    spec.identity.clone(),
                ));
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Accessors for internal state
    // -----------------------------------------------------------------------

    /// Get the current entries (read-only snapshot).
    #[allow(dead_code)]
    pub(crate) async fn entries(&self) -> BTreeMap<AgentIdentity, IdentityEntry> {
        self.entries.read().await.clone()
    }

    /// Check if an identity is registered.
    pub async fn contains(&self, identity: &AgentIdentity) -> bool {
        self.entries.read().await.contains_key(identity)
    }

    /// Whether an identity is registered AND Core holds its member (read from
    /// Core's member history; a failed read answers `false`).
    pub async fn is_active(&self, identity: &AgentIdentity) -> bool {
        if !self.entries.read().await.contains_key(identity) {
            return false;
        }
        match self.core_view_history().await {
            Ok(history) => CoreViewFacts::of(history.as_ref(), identity).live,
            Err(error) => {
                tracing::debug!(%identity, %error, "is_active: Core's member history read failed");
                false
            }
        }
    }

    fn identity_from_member_alias(alias: &str) -> Option<(AgentIdentity, bool)> {
        let alias = crate::member_comms_id::runtime_alias_str(alias);
        let alias = alias.as_ref();
        alias
            .strip_prefix("rt:")
            .and_then(|rest| rest.rsplit_once(':'))
            .filter(|(identity, generation)| {
                !identity.is_empty()
                    && !generation.is_empty()
                    && generation.chars().all(|ch| ch.is_ascii_digit())
            })
            .and_then(|(identity, _)| AgentIdentity::parse(identity).ok())
            .map(|identity| (identity, true))
            .or_else(|| {
                AgentIdentity::parse(alias)
                    .ok()
                    .map(|identity| (identity, false))
            })
    }

    /// Parse the durable owner encoded in the reserved generated-alias
    /// namespace, whether or not that identity is still registered. Mutating
    /// member surfaces use this to fail closed after a concurrent delete
    /// instead of treating an orphaned `rt:*` alias as an ordinary mob member.
    pub(crate) fn identity_for_generated_member_alias(alias: &str) -> Option<AgentIdentity> {
        Self::identity_from_member_alias(alias).and_then(|(identity, generated)| {
            if generated { Some(identity) } else { None }
        })
    }

    /// Resolve a mutating member request without allowing a generated alias
    /// to fall back to the raw mob plane after concurrent deletion. Plain
    /// durable identities retain their historical registered-only behavior.
    pub(crate) async fn identity_for_member_mutation(&self, alias: &str) -> Option<AgentIdentity> {
        match Self::identity_from_member_alias(alias) {
            Some((identity, true)) => Some(identity),
            Some((identity, false)) => self.contains(&identity).await.then_some(identity),
            None => None,
        }
    }

    async fn ensure_expected_member_alias_current(
        &self,
        identity: &AgentIdentity,
        expected_alias: &str,
    ) -> Result<(), IdentityRuntimeError> {
        let canonical_alias = crate::member_comms_id::runtime_alias_str(expected_alias);
        let Some((alias_identity, generated_runtime_alias)) =
            Self::identity_from_member_alias(canonical_alias.as_ref())
        else {
            return Err(IdentityRuntimeError::UnknownIdentity(identity.clone()));
        };
        if alias_identity != *identity {
            return Err(IdentityRuntimeError::UnknownIdentity(alias_identity));
        }
        if !self.entries.read().await.contains_key(identity) {
            return Err(IdentityRuntimeError::UnknownIdentity(identity.clone()));
        }
        if generated_runtime_alias {
            // A generated alias names one incarnation: it must be the live
            // member Core holds now.
            let current = self
                .core_live_member(identity)
                .await?
                .map(|live| live.agent_runtime_id);
            if current.as_ref().map(AgentRuntimeId::as_str) != Some(canonical_alias.as_ref()) {
                return Err(IdentityRuntimeError::StaleRuntimeAlias {
                    identity: identity.clone(),
                    requested: canonical_alias.into_owned(),
                    current,
                });
            }
        }
        Ok(())
    }

    /// Resolve a member alias to the durable identity that OWNS it, if any.
    ///
    /// Accepts both a generated runtime alias (`rt:<identity>:<generation>`)
    /// and a plain identity string; returns the identity only when it is
    /// actually registered in this runtime. Member-scoped RPCs use this to
    /// route lifecycle mutations of identity-owned members through the
    /// identity authority - a classic `handle.retire()`/`respawn()` on such
    /// a member would mutate it behind the IdentityRuntime's back (stale
    /// continuity binding, generation drift), which is the doctrine's
    /// "mob plane must not mangle durables" rule.
    pub async fn owned_identity_for_member_alias(&self, alias: &str) -> Option<AgentIdentity> {
        let (identity, _) = Self::identity_from_member_alias(alias)?;
        self.contains(&identity).await.then_some(identity)
    }

    /// Get the runtime instance ID.
    pub fn runtime_instance_id(&self) -> &str {
        &self.runtime_instance_id
    }

    /// Get whether a runtime store is configured.
    pub fn has_runtime_store(&self) -> bool {
        self.has_runtime_store
    }

    /// The session bridge, if configured. Crate-internal (D67): callers
    /// outside the composition reach it through
    /// [`super::PublicIdentityRuntime::bridge`], which carries the mob's
    /// public owner.
    pub(crate) fn bridge(&self) -> Option<&Arc<dyn SessionBridge>> {
        self.bridge.as_ref()
    }

    // -----------------------------------------------------------------------
    // Convenience methods
    // -----------------------------------------------------------------------

    /// Send plain text to an addressable identity.
    pub async fn send_text(
        &self,
        identity: &AgentIdentity,
        text: impl Into<String>,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.send(identity, &meerkat_core::ContentInput::Text(text.into()))
            .await
    }

    /// Dispatch plain text with system origin.
    pub async fn dispatch_text(
        &self,
        identity: &AgentIdentity,
        text: impl Into<String>,
    ) -> Result<(FencingToken, bool), IdentityRuntimeError> {
        self.dispatch(identity, &DispatchInput::system(text)).await
    }

    /// Execute the restore flow for the given roster.
    pub async fn restore_flow(
        &self,
        roster: &[DurableAgentSpec],
        topology_provider: Option<&dyn super::contracts::TopologyProvider>,
        customizer: Option<&dyn super::contracts::AgentCustomizer>,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError> {
        super::orchestrator::restore_flow(self, roster, topology_provider, customizer).await
    }

    /// Resolve the AgentRuntimeId of a registered identity's live Core member.
    /// # NOT a roster key
    ///
    /// This returns the identity's CURRENT INCARNATION - `rt:{identity}:{gen}` -
    /// which is binding detail. Since the stable-identity lowering the mob roster
    /// is keyed by the bare DURABLE identity, so encoding this result into a
    /// member id yields a valid-looking id for a member that does not exist, and
    /// operations on it fail with "mob member not found" about a healthy member.
    ///
    /// For "which roster row is this identity", use
    /// [`crate::member_comms_id::roster_member_id_for_identity`].
    ///
    /// This distinction cost a downstream fleet a zeroed board: our resolver
    /// handed out this spelling, the consumer encoded it as documented, and wire
    /// rejected it - two halves of one public API disagreeing about what a roster
    /// key is.
    pub async fn runtime_id_for(
        &self,
        identity: &AgentIdentity,
    ) -> Result<AgentRuntimeId, IdentityRuntimeError> {
        if !self.entries.read().await.contains_key(identity) {
            return Err(IdentityRuntimeError::UnknownIdentity(identity.clone()));
        }
        self.core_live_member(identity)
            .await?
            .map(|live| live.agent_runtime_id)
            .ok_or_else(|| {
                IdentityRuntimeError::Internal(format!(
                    "Core holds no live member for identity {identity}"
                ))
            })
    }

    /// Inspect the current execution state of an identity via the bridge.
    ///
    /// The bridge's inspection is a full meerkat member-status read, which
    /// the mob admits one at a time. So concurrent callers asking about the
    /// same member incarnation join one read in flight instead of each
    /// starting their own. A settled read is never reused: the next caller
    /// reads afresh. A caller joins only a read that started at or after the
    /// identity's completion cursor as the caller sees it, so an inspection
    /// never reports an older turn's output than the cursor read before it
    /// (`meta/inspect_identity` returns both, and SDK completion waits take
    /// the output once the cursor moves). A new incarnation (respawn, reset)
    /// has a new runtime id and is always read afresh.
    pub async fn inspect(
        &self,
        identity: &AgentIdentity,
    ) -> Result<super::bridge::MemberInspection, IdentityRuntimeError> {
        let runtime_id = self.runtime_id_for(identity).await?;
        let bridge = self
            .bridge
            .as_ref()
            .ok_or_else(|| IdentityRuntimeError::Internal("no bridge configured".to_string()))?;
        let seen = self.completion_cursor(identity).await;
        let shared = {
            let mut inspections = self
                .inspections
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            inspections.retain(|_, shared| shared.in_flight());
            match inspections.get(&runtime_id) {
                Some(shared) if shared.joinable_at(seen) => shared.clone(),
                _ => {
                    let shared = SharedInspection::start(seen);
                    inspections.insert(runtime_id.clone(), shared.clone());
                    shared
                }
            }
        };
        shared
            .read
            .get_or_init(|| async {
                bridge
                    .inspect_member(&runtime_id)
                    .await
                    .map_err(|e| format!("inspect: {e}"))
            })
            .await
            .clone()
            .map_err(IdentityRuntimeError::Internal)
    }

    // -----------------------------------------------------------------------
    // Continuity repair triggers
    // -----------------------------------------------------------------------

    /// The refusal of a delivery to an identity Core holds no live member
    /// for. Nothing is triggered: the next wake asks Core again.
    fn refuse_inactive_delivery(
        &self,
        identity: &AgentIdentity,
        state: IdentityLifecycleState,
        operation: &'static str,
    ) -> IdentityRuntimeError {
        IdentityRuntimeError::InvalidState {
            identity: identity.clone(),
            state,
            operation,
        }
    }

    /// [`Self::refuse_inactive_delivery`] with the identity's view state.
    async fn inactive_delivery_refusal(
        &self,
        identity: &AgentIdentity,
        operation: &'static str,
    ) -> IdentityRuntimeError {
        let wake_refusal = match self.entries.read().await.get(identity) {
            Some(entry) => entry.wake_refusal.clone(),
            None => return IdentityRuntimeError::UnknownIdentity(identity.clone()),
        };
        self.refuse_inactive_delivery(
            identity,
            Self::view_state(false, wake_refusal.as_ref()),
            operation,
        )
    }

    /// A member session a delivery found Core holds no durable copy of is
    /// never delivered to again: the identity's fresh life starts here, under
    /// the caller's lifecycle lock, with that session as its replaced session
    /// (the wake's own `Absent` rule), and the delivery goes to the fresh
    /// member. Core never persisted the absent session, so nothing durable is
    /// lost.
    async fn start_fresh_life_after_absent_session(
        &self,
        identity: &AgentIdentity,
        absent: SessionId,
    ) -> Result<super::bridge::MemberRuntimeBinding, IdentityRuntimeError> {
        tracing::warn!(
            %identity,
            session_id = %absent,
            "a delivery found the member's session absent from Core's durable store; the \
             identity starts its fresh life before delivering"
        );
        // The wedged member still holds the inputs other senders queued
        // behind it: capture them before the fresh life replaces it, for the
        // fresh member, as the bridge's delivery repair carries its member's
        // queue. A queue that cannot be observed defers the fresh life:
        // replacing the member would destroy inputs nobody has seen.
        let queued = self
            .core_bridge(identity)?
            .capture_queued_member_inputs(identity, &absent)
            .await
            .map_err(|error| IdentityRuntimeError::IdentityWakeRetryable {
                identity: identity.clone(),
                session_id: Some(absent.clone()),
                detail: format!(
                    "the fresh life after the absent session is deferred: the member's queued \
                     inputs could not be captured ({error})"
                ),
            })?;
        let mut foreground_shutdown = self.foreground_cancel.subscribe();
        let live = self
            .start_fresh_life_locked(
                identity,
                &mut foreground_shutdown,
                "fresh life after an absent session",
                Some(queued),
            )
            .await?;
        self.settle_fresh_life(
            identity,
            live,
            Some(super::types::ReplacedSession {
                session_id: absent,
                reason: super::types::ReplacedSessionReason::Absent,
            }),
        )
        .await;
        self.core_live_member(identity).await?.ok_or_else(|| {
            IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: "Core holds no live member after the fresh life an absent session \
                         started; nothing was delivered"
                    .to_string(),
            }
        })
    }

    /// Core answers a live member with no session both for a peer-only
    /// member and for a session-backed member whose session binding it does
    /// not project at that moment (a retiring member, or the projection
    /// behind the actor). Whether an identity is peer-only is read from its
    /// spec, never from that answer: a session-backed identity whose member
    /// reads without a session refuses typed (history unavailable, so the
    /// next call reads Core again), and nothing skips a revive or delivers
    /// without Core's fence.
    async fn refuse_unprojected_session(
        &self,
        identity: &AgentIdentity,
        binding: &super::bridge::MemberRuntimeBinding,
    ) -> Result<(), IdentityRuntimeError> {
        if binding.session_id.is_some() {
            return Ok(());
        }
        let session_backed = self
            .entries
            .read()
            .await
            .get(identity)
            .is_some_and(|entry| !durable_spec_uses_external_binding(&entry.spec));
        if session_backed {
            return Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: format!(
                    "Core holds member {} for the identity but projects no session binding \
                     for it; a session-backed identity is never treated as peer-only, so \
                     nothing was done",
                    binding.agent_runtime_id
                ),
            });
        }
        Ok(())
    }

    /// Core's live member of `identity` for a delivery, read under the
    /// caller's lifecycle lock: its binding and, for a session-backed
    /// member, its delivery scope. An identity Core holds no member for
    /// refuses typed with its view state (the caller woke it first, so Core
    /// let it go meanwhile).
    async fn live_delivery_target(
        &self,
        identity: &AgentIdentity,
        operation: &'static str,
    ) -> Result<
        (
            super::bridge::MemberRuntimeBinding,
            Option<meerkat_mob::MemberDeliveryScope>,
        ),
        IdentityRuntimeError,
    > {
        let Some(live) = self.core_live_member(identity).await? else {
            return Err(self.inactive_delivery_refusal(identity, operation).await);
        };
        // A pending first-boot `Fresh` row carried out here replaced the
        // member by its successor: the delivery goes to the member Core
        // holds now.
        let live = if self
            .first_boot_guard_live_locked(identity, operation)
            .await?
        {
            match self.core_live_member(identity).await? {
                Some(live) => live,
                None => return Err(self.inactive_delivery_refusal(identity, operation).await),
            }
        } else {
            live
        };
        self.refuse_unprojected_session(identity, &live).await?;
        self.publish_pending_customizer_tools(identity, live.session_id.as_ref(), operation)
            .await?;
        let absent_session = self
            .entries
            .read()
            .await
            .get(identity)
            .and_then(|entry| entry.absent_delivery_session.clone());
        let live = match absent_session {
            Some(absent) if live.session_id.as_ref() == Some(&absent) => {
                self.start_fresh_life_after_absent_session(identity, absent)
                    .await?
            }
            _ => live,
        };
        let scope = if live.session_id.is_some() {
            self.core_live_scope(identity).await?
        } else {
            None
        };
        Ok((live, scope))
    }

    /// The fencing token a delivery carries and its completion cursor's
    /// epoch: Core's fence of the scope. A peer-only member has no
    /// Core scope and carries `0`.
    fn scope_fence(scope: Option<&meerkat_mob::MemberDeliveryScope>) -> FencingToken {
        scope.map_or_else(
            || FencingToken::new(0),
            |scope| FencingToken::new(scope.fence_token().get()),
        )
    }

    /// After the delivery path's one automatic reload: Core's member must
    /// still be the one the delivery is addressed to (Core's reload
    /// primitive keeps it). Returns its fence. A reload that re-spawned the
    /// member refuses typed: the refused attempt delivered nothing, and the
    /// caller delivers again to the new member.
    async fn fence_after_delivery_reload(
        &self,
        identity: &AgentIdentity,
        addressed: &AgentRuntimeId,
        operation: &'static str,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        let (live, scope) = self.live_delivery_target(identity, operation).await?;
        if live.agent_runtime_id != *addressed {
            return Err(IdentityRuntimeError::ReloadRequired {
                identity: identity.clone(),
                reason: format!(
                    "the automatic reload re-spawned the member as {}; nothing was delivered, \
                     deliver again",
                    live.agent_runtime_id
                ),
            });
        }
        Ok(Self::scope_fence(scope.as_ref()))
    }

    // -----------------------------------------------------------------------
    // Turn-completion cursor
    // -----------------------------------------------------------------------

    /// Completion epoch of a REGISTERED identity: Core's fence of its live
    /// session-backed member, or 0 while Core holds none (dormant,
    /// peer-only) or the read fails (a 0 rebase never rewinds the cursor).
    /// `None` when the identity is not registered at all.
    async fn registered_completion_epoch(&self, identity: &AgentIdentity) -> Option<FencingToken> {
        if !self.entries.read().await.contains_key(identity) {
            return None;
        }
        Some(
            self.core_live_scope(identity)
                .await
                .ok()
                .flatten()
                .map_or_else(
                    || FencingToken::new(0),
                    |scope| Self::scope_fence(Some(&scope)),
                ),
        )
    }

    /// Move the stored cursor onto `epoch` if the lease incarnation advanced,
    /// and return it. Never rewinds: a stale epoch leaves the cursor alone.
    fn rebase_completion_cursor(
        &self,
        identity: &AgentIdentity,
        epoch: FencingToken,
    ) -> CompletionCursor {
        let mut cursors = self
            .completion_cursors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cursor = cursors
            .entry(identity.clone())
            .or_insert_with(|| CompletionCursor::start(epoch));
        *cursor = cursor.rebased(epoch);
        *cursor
    }

    /// Whatever was last published for `identity`, without minting a ledger
    /// entry. Reads for unregistered names go through here so probing
    /// arbitrary strings cannot grow the map.
    fn retained_completion_cursor(&self, identity: &AgentIdentity) -> CompletionCursor {
        self.completion_cursors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(identity)
            .copied()
            .unwrap_or_default()
    }

    /// Current [`CompletionCursor`] for `identity`.
    ///
    /// A registered identity's cursor is rebased onto its live lease
    /// incarnation first, so a poller observes an incarnation change
    /// immediately rather than comparing against a turn count that no longer
    /// means anything. An identity that is gone (retired, deleted, or never
    /// registered) reports its last published cursor - retained precisely so
    /// this read cannot rewind.
    ///
    /// Credit before read: completions that already happened but are still
    /// uncounted in the session's retained events are credited first, so the
    /// read never precedes the credit of a completion that happened before it
    /// (a baseline taken now can then never be satisfied by an older turn).
    pub async fn completion_cursor(&self, identity: &AgentIdentity) -> CompletionCursor {
        match self.registered_completion_epoch(identity).await {
            Some(epoch) => {
                if epoch.get() != 0 {
                    self.credited_completion_cursor(identity, epoch).await
                } else {
                    self.rebase_completion_cursor(identity, epoch)
                }
            }
            None => self.retained_completion_cursor(identity),
        }
    }

    /// Credit the run completions of `identity`'s live member that happened
    /// but are not counted yet ([`PendingCompletionDrain`]). Only under a
    /// live Core fence: a completion credited before it would be discarded
    /// by the rebase onto the fence, while one credited under it is part of
    /// every baseline taken from then on.
    async fn credit_pending_completions(&self, identity: &AgentIdentity) {
        let drain = self
            .pending_completion_drain
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let Some(drain) = drain else {
            return;
        };
        let Some(session_id) = self
            .core_live_member(identity)
            .await
            .ok()
            .flatten()
            .and_then(|live| live.session_id)
        else {
            return;
        };
        let terminals = drain.drain(&session_id).await;
        for _ in 0..terminals.completed {
            self.record_turn_completed(identity).await;
        }
        for _ in 0..terminals.failed {
            self.record_turn_failed(identity).await;
        }
    }

    fn completion_credit_lock(&self, identity: &AgentIdentity) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = self
            .completion_credit_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(identity).and_then(std::sync::Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(identity.clone(), Arc::downgrade(&lock));
        lock
    }

    /// Credit what already happened, then read the cursor under `epoch`, both
    /// under the identity's completion-credit lock: no terminal is claimed
    /// but not yet credited while the read happens.
    async fn credited_completion_cursor(
        &self,
        identity: &AgentIdentity,
        epoch: FencingToken,
    ) -> CompletionCursor {
        let lock = self.completion_credit_lock(identity);
        let _credit = lock.lock().await;
        self.credit_pending_completions(identity).await;
        self.rebase_completion_cursor(identity, epoch)
    }

    /// Credit one observed run terminal of `identity` if `claim` admits it,
    /// claiming and crediting under the identity's completion-credit lock so
    /// a concurrent cursor read sees either neither or both. Returns whether
    /// a completion was credited.
    pub(crate) async fn credit_observed_terminal(
        &self,
        identity: &AgentIdentity,
        failed: bool,
        claim: impl FnOnce() -> bool,
    ) -> bool {
        let lock = self.completion_credit_lock(identity);
        let _credit = lock.lock().await;
        if !claim() {
            return false;
        }
        if failed {
            self.record_turn_failed(identity).await;
            false
        } else {
            self.record_turn_completed(identity).await;
            true
        }
    }

    /// Whether a completion-cursor drain is installed
    /// ([`Self::install_pending_completion_drain`]).
    pub(crate) fn has_pending_completion_drain(&self) -> bool {
        self.pending_completion_drain
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }

    /// Install the source of completions that happened but are not counted
    /// yet: meerkat's retained session events past the health monitor's
    /// credited position.
    pub(crate) fn install_pending_completion_drain(&self, drain: Arc<dyn PendingCompletionDrain>) {
        *self
            .pending_completion_drain
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(drain);
    }

    /// Record that a turn completed for `identity`, advancing its cursor by
    /// one within the current lease incarnation.
    ///
    /// Production drives this from `AgentEvent::RunCompleted` on the always-on
    /// identity agent-event monitor. It is deliberately event-driven rather
    /// than derived from a polled projection: a poll cannot distinguish "new
    /// turn, identical text" from "no new turn", which is the entire defect
    /// this cursor exists to close.
    ///
    /// Not idempotent by design - one observed completion advances the cursor
    /// once, which is exactly what a poller comparing against a pre-delivery
    /// baseline needs.
    pub async fn record_turn_completed(&self, identity: &AgentIdentity) -> CompletionCursor {
        // A completion racing deregistration still counts under the last
        // known epoch rather than being dropped.
        let epoch = self
            .registered_completion_epoch(identity)
            .await
            .unwrap_or_else(|| self.retained_completion_cursor(identity).epoch);
        let mut cursors = self
            .completion_cursors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cursor = cursors
            .entry(identity.clone())
            .or_insert_with(|| CompletionCursor::start(epoch));
        *cursor = cursor.rebased(epoch).advanced();
        let cursor = *cursor;
        drop(cursors);
        self.entries.notify();
        cursor
    }

    /// Wait until a turn completes past `baseline`, or the timeout expires.
    ///
    /// This is the correct completion barrier: it compares cursors, never
    /// output text, so two consecutive turns emitting byte-identical text are
    /// still two distinct completions. `baseline` is the
    /// `completion_baseline` returned by [`Self::dispatch_admission_tracked`]
    /// or [`Self::send_admission_tracked`].
    ///
    /// A genuinely stalled turn still times out rather than hanging forever,
    /// and an incarnation change is reported as its own error rather than
    /// being read as either completion or continued waiting. Event-driven, as
    /// [`Self::await_completion`].
    pub async fn wait_for_completion(
        &self,
        identity: &AgentIdentity,
        baseline: CompletionCursor,
        timeout: Duration,
    ) -> Result<CompletionCursor, IdentityRuntimeError> {
        match self
            .await_completion(identity, Some(baseline), timeout)
            .await
        {
            CompletionWait::Completed(cursor) => Ok(cursor),
            CompletionWait::IncarnationChanged(observed) => {
                Err(IdentityRuntimeError::CompletionIncarnationChanged {
                    identity: identity.clone(),
                    baseline,
                    observed,
                })
            }
            CompletionWait::TimedOut(_) => Err(IdentityRuntimeError::Internal(format!(
                "timed out after {}s waiting for a turn past {baseline} on {identity}",
                timeout.as_secs_f64()
            ))),
            other => Err(IdentityRuntimeError::Internal(format!(
                "waiting for a turn past {baseline} on {identity} ended: {}",
                other.wire_str()
            ))),
        }
    }

    /// Wait until the identity's completion cursor moves past `after`, or,
    /// with no `after`, until it counts at least one completed turn (a
    /// startup readiness barrier).
    ///
    /// Event-driven: the wait sleeps until a completion or a failed run is
    /// recorded, the identity table changes, a verb of this runtime moves the
    /// identity's Core member (wake, retire, respawn, reset, delete), or the
    /// runtime starts shutting down, and re-derives the outcome then. Every
    /// wake re-reads the entry and Core's fence, so a wait never runs to its
    /// deadline on an identity whose last wake was refused for good, that
    /// Core no longer holds (retired, unless a verb of this runtime is still
    /// replacing the member) or that was unregistered. `timeout` is only the
    /// caller's overall deadline; reaching it is
    /// [`CompletionWait::TimedOut`], a typed outcome rather than an error.
    ///
    /// An identity that is not registered yet reads its retained cursor, so a
    /// readiness wait started before the identity materializes simply waits
    /// for its first completion.
    ///
    /// The wake signal is shared by every identity (one watch over the
    /// identity table), so a write to one identity wakes every waiter, which
    /// re-reads its own entry and cursor and sleeps again. That costs one
    /// cheap in-memory read per waiter per write; waiters are few (one per
    /// parked SDK wait), so a per-identity signal is not worth the bookkeeping.
    pub async fn await_completion(
        &self,
        identity: &AgentIdentity,
        after: Option<CompletionCursor>,
        timeout: Duration,
    ) -> CompletionWait {
        let deadline = tokio::time::Instant::now() + timeout;
        // Subscribe before the first read so a completion landing between the
        // read and the wait still wakes it.
        let mut changes = self.entries.subscribe();
        let mut shutdown = self.foreground_waits_closed.subscribe();
        // Credit what already happened before taking the failure baseline: a
        // failure that preceded this wait but is credited by its first read
        // must not end the wait as its own failure.
        let _ = self.completion_cursor(identity).await;
        let failures_at_start = self.run_failure_count(identity);
        let registered_at_start = self.entries.read().await.contains_key(identity);
        // A readiness wait on a dormant identity waits for its first wake;
        // only a member Core held when the wait began can retire under it.
        let live_at_start = matches!(self.core_live_member(identity).await, Ok(Some(_)));
        loop {
            let cursor = self.completion_cursor(identity).await;
            match after {
                Some(baseline) => match cursor.progress_since(baseline) {
                    CompletionProgress::Completed => return CompletionWait::Completed(cursor),
                    CompletionProgress::IncarnationChanged => {
                        return CompletionWait::IncarnationChanged(cursor);
                    }
                    CompletionProgress::Pending => {}
                },
                None if cursor.turns > 0 => return CompletionWait::Completed(cursor),
                None => {}
            }
            if self.run_failure_count(identity) > failures_at_start {
                return CompletionWait::RunFailed(cursor);
            }
            let wake_refusal = self
                .entries
                .read()
                .await
                .get(identity)
                .map(|entry| entry.wake_refusal.clone());
            match wake_refusal {
                None if registered_at_start => return CompletionWait::IdentityGone(cursor),
                Some(Some(refusal)) if refusal.is_final() => {
                    return CompletionWait::Broken(cursor);
                }
                _ => {}
            }
            // Core let the member go and no verb of this runtime is replacing
            // it (a respawn retires, then wakes, under the lifecycle lock).
            if live_at_start && matches!(self.core_live_member(identity).await, Ok(None)) {
                if !self.lifecycle_verb_in_flight(identity).await {
                    return CompletionWait::Retiring(cursor);
                }
                // The verb signalled its move while it still held the lock,
                // and nothing signals when it lets go: read again once it
                // has, so a retire never leaves this wait to its deadline.
                let lifecycle_lock = self.lifecycle_lock_for(identity).await;
                tokio::select! {
                    _released = lifecycle_lock.lock() => continue,
                    changed = shutdown.changed() => {
                        if changed.is_err() {
                            return CompletionWait::ShuttingDown(cursor);
                        }
                        continue;
                    }
                    () = tokio::time::sleep_until(deadline) => {
                        return CompletionWait::TimedOut(cursor);
                    }
                }
            }
            if *shutdown.borrow_and_update() {
                return CompletionWait::ShuttingDown(cursor);
            }
            tokio::select! {
                changed = changes.changed() => {
                    if changed.is_err() {
                        return CompletionWait::ShuttingDown(cursor);
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() {
                        return CompletionWait::ShuttingDown(cursor);
                    }
                }
                () = tokio::time::sleep_until(deadline) => {
                    return CompletionWait::TimedOut(cursor);
                }
            }
        }
    }

    /// Record that a run on `identity` failed (or was cancelled), waking the
    /// completion waiters. Driven from `AgentEvent::RunFailed` on the
    /// identity agent-event monitor, beside [`Self::record_turn_completed`].
    pub async fn record_turn_failed(&self, identity: &AgentIdentity) {
        *self
            .run_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(identity.clone())
            .or_default() += 1;
        self.entries.notify();
    }

    fn run_failure_count(&self, identity: &AgentIdentity) -> u64 {
        self.run_failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(identity)
            .copied()
            .unwrap_or(0)
    }

    /// The configured default timeout for wait operations.
    pub fn default_timeout(&self) -> Duration {
        self.default_timeout
    }

    /// Run a reset's memory capture of the outgoing session in a
    /// runtime-owned task, so the reset stays latency-neutral. The capture
    /// stays in the ledger until it ran: a graceful shutdown joins the task,
    /// then reruns whatever a cancelled task left behind.
    async fn spawn_reset_memory_capture(&self, capture: PendingResetMemoryCapture) {
        let key = capture.session_key.clone();
        self.pending_reset_memory_captures
            .write()
            .await
            .insert(key.clone(), capture.clone());
        let pending = Arc::clone(&self.pending_reset_memory_captures);
        let mut tasks = self.reset_memory_capture_tasks.lock().await;
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result {
                tracing::error!(
                    runtime_instance_id = %self.runtime_instance_id,
                    %error,
                    "reset memory capture task panicked"
                );
            }
        }
        tasks.spawn(async move {
            capture.run().await;
            pending.write().await.remove(&key);
        });
    }

    /// Join every reset memory capture task after foreground lifecycle
    /// admission is closed.
    pub(crate) async fn join_reset_memory_captures(&self) {
        let mut tasks = self.reset_memory_capture_tasks.lock().await;
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                tracing::error!(
                    runtime_instance_id = %self.runtime_instance_id,
                    %error,
                    "reset memory capture task panicked"
                );
            }
        }
    }

    /// Run every reset memory capture a cancelled task left in the ledger.
    /// Returns how many ran.
    pub(crate) async fn drain_pending_reset_memory_captures(&self) -> usize {
        let pending = std::mem::take(&mut *self.pending_reset_memory_captures.write().await);
        for capture in pending.values() {
            capture.run().await;
        }
        pending.len()
    }

    /// Wait until the identity produces an output_preview, or timeout.
    ///
    /// **Unsound as a completion barrier.** `output_preview` is the last
    /// committed assistant text, so this returns immediately when a PREVIOUS
    /// turn already left a preview, and it cannot tell "new turn, identical
    /// text" from "no new turn" at all. Use
    /// [`Self::wait_for_completion`] with the `completion_baseline` from
    /// [`Self::send_admission_tracked`] / [`Self::dispatch_admission_tracked`]
    /// when you need to wait for a specific turn. Retained for callers that
    /// only need "has this identity ever spoken".
    ///
    /// Event-driven, as [`Self::wait_for_output_where`].
    pub async fn wait_for_output(
        &self,
        identity: &AgentIdentity,
        timeout: Duration,
    ) -> Result<String, IdentityRuntimeError> {
        self.wait_for_output_where(identity, timeout, |_| true)
            .await
            .map_err(|ended| ended.into_error(identity, "output"))
    }

    /// Wait until the identity's output_preview contains `needle`, or
    /// timeout. Event-driven, as [`Self::wait_for_output_where`].
    pub async fn wait_for_output_containing(
        &self,
        identity: &AgentIdentity,
        needle: &str,
        timeout: Duration,
    ) -> Result<String, IdentityRuntimeError> {
        self.wait_for_output_where(identity, timeout, |preview| preview.contains(needle))
            .await
            .map_err(|ended| ended.into_error(identity, &format!("output containing '{needle}'")))
    }

    /// Read the identity's output_preview whenever it can have changed, until
    /// `accept` takes it or `timeout` elapses.
    ///
    /// No sleep or poll: the preview is the last committed assistant text,
    /// and a commit is recorded as a completion, which wakes the identity
    /// table's change signal ([`Self::await_completion`] waits on the same
    /// one). Registration, lease changes, Broken, retire and delete are table
    /// writes too, so a wait started before the identity materializes reads
    /// again once it does. The subscription is taken before each read, so a
    /// completion landing during the read still wakes the next one. A read
    /// that fails (the identity is not active yet, say) is kept as the
    /// timeout's detail and retried on the next wake, never on a timer.
    async fn wait_for_output_where(
        &self,
        identity: &AgentIdentity,
        timeout: Duration,
        accept: impl Fn(&str) -> bool,
    ) -> Result<String, OutputWaitEnded> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut changes = self.entries.subscribe();
        let mut shutdown = self.foreground_waits_closed.subscribe();
        let mut last_error = None;
        loop {
            changes.borrow_and_update();
            match self.inspect(identity).await {
                Ok(inspection) => {
                    if let Some(preview) = inspection.output_preview
                        && accept(&preview)
                    {
                        return Ok(preview);
                    }
                }
                Err(error) => last_error = Some(error.to_string()),
            }
            if *shutdown.borrow_and_update() {
                return Err(OutputWaitEnded::ShuttingDown);
            }
            tokio::select! {
                changed = changes.changed() => {
                    if changed.is_err() {
                        return Err(OutputWaitEnded::ShuttingDown);
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() {
                        return Err(OutputWaitEnded::ShuttingDown);
                    }
                }
                () = tokio::time::sleep_until(deadline) => {
                    return Err(OutputWaitEnded::TimedOut { last_error });
                }
            }
        }
    }
}

/// Why [`IdentityRuntime::wait_for_output_where`] ended without an accepted
/// preview.
enum OutputWaitEnded {
    TimedOut { last_error: Option<String> },
    ShuttingDown,
}

impl OutputWaitEnded {
    fn into_error(self, identity: &AgentIdentity, what: &str) -> IdentityRuntimeError {
        match self {
            Self::TimedOut { last_error: None } => IdentityRuntimeError::Internal(format!(
                "timed out waiting for {what} from {identity}"
            )),
            Self::TimedOut {
                last_error: Some(error),
            } => IdentityRuntimeError::Internal(format!(
                "timed out waiting for {what} from {identity} (last read failed: {error})"
            )),
            Self::ShuttingDown => IdentityRuntimeError::Internal(format!(
                "runtime shutting down while waiting for {what} from {identity}"
            )),
        }
    }
}

/// Wire two identities across mobs, resolving runtime IDs from both IdentityRuntimes.
///
/// This is a convenience function for same-process cross-mob scenarios where both
/// IdentityRuntimes are available. It resolves the AgentRuntimeId for each identity
/// and delegates to `UnifiedRuntime::wire_cross_mob()`.
///
/// The wire checks each generated alias (`rt:{identity}:{generation}`) under
/// the identity runtime that owns it: the one installed in `local_unified`
/// for the local identity, and the one the peer mob was registered with
/// ([`crate::UnifiedRuntime::register_peer_runtime`] or
/// [`crate::UnifiedRuntime::register_peer_mob_with_identity_runtime`]) for the
/// remote one. Pass those installed runtimes, for example the views
/// [`crate::UnifiedRuntime::identity_runtime`] returns. A runtime made with
/// [`IdentityRuntime::new`] and never installed is no such authority, and the
/// wire refuses its identities ("generated aliases require the owning
/// IdentityRuntime").
pub async fn wire_cross_mob_by_identity(
    local_irt: &IdentityRuntime,
    local_identity: &AgentIdentity,
    remote_irt: &IdentityRuntime,
    remote_identity: &AgentIdentity,
    local_unified: &crate::UnifiedRuntime,
    remote_mob_id: &str,
) -> Result<(), IdentityRuntimeError> {
    let local_rt = local_irt.runtime_id_for(local_identity).await?;
    let remote_rt = remote_irt.runtime_id_for(remote_identity).await?;
    Box::pin(local_unified.wire_cross_mob(local_rt.as_str(), remote_rt.as_str(), remote_mob_id))
        .await
        .map_err(|e| IdentityRuntimeError::Internal(format!("wire_cross_mob: {e}")))
}

/// Core's member facts for this file's `SessionBridge` test doubles. A double
/// records each member it creates, replaces or retires here and answers the
/// runtime's Core reads (live member, delivery scope, member history,
/// identity management) from it, so a unit test drives the identity runtime
/// against the facts Core would hold without standing up a mob.
#[cfg(test)]
mod reset_reprofile_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::sync::{Mutex as AsyncMutex, RwLock as AsyncRwLock};

    use super::super::bridge::{
        BridgeDelivery, BridgeError, MemberInspection, MemberRuntimeBinding, ResumeSessionOutcome,
    };
    use super::super::contracts::RosterProvider;
    use super::super::local_lease::LocalLeaseProvider;
    use super::super::local_store::LocalContinuityStore;
    use super::super::types::{AgentBuildDraft, CheckpointVersion, RosterError, SessionSnapshot};
    use tokio::sync::Notify;

    struct MutableRoster {
        specs: AsyncRwLock<Vec<DurableAgentSpec>>,
    }

    impl MutableRoster {
        fn new(specs: Vec<DurableAgentSpec>) -> Self {
            Self {
                specs: AsyncRwLock::new(specs),
            }
        }

        async fn set(&self, specs: Vec<DurableAgentSpec>) {
            *self.specs.write().await = specs;
        }
    }

    #[async_trait::async_trait]
    impl RosterProvider for MutableRoster {
        async fn roster(
            &self,
            _context: &RosterContext,
        ) -> Result<Vec<DurableAgentSpec>, RosterError> {
            Ok(self.specs.read().await.clone())
        }
    }

    #[derive(Default)]
    struct RecordingBridge {
        create_profiles: AsyncMutex<Vec<String>>,
        create_delay: Duration,
        creates_in_flight: AtomicUsize,
        max_creates_in_flight: AtomicUsize,
        retired_runtime_ids: AsyncMutex<Vec<String>>,
        /// Successor aliases minted by `reset_member_to_successor`; their
        /// count numbers the next successor.
        successor_generations: AsyncMutex<Vec<String>>,
        /// Identities created through the held arm (`create_session_held`).
        held_creates: AsyncMutex<Vec<String>>,
        /// The members Core holds, as this double's creates and retires
        /// leave them.
        core: crate::identity_first::core_test_members::CoreTestMembers,
    }

    impl RecordingBridge {
        async fn create_profiles(&self) -> Vec<String> {
            self.create_profiles.lock().await.clone()
        }

        fn max_creates_in_flight(&self) -> usize {
            self.max_creates_in_flight.load(Ordering::SeqCst)
        }

        async fn retired_runtime_ids(&self) -> Vec<String> {
            self.retired_runtime_ids.lock().await.clone()
        }
    }

    #[async_trait::async_trait]
    impl SessionBridge for RecordingBridge {
        /// Mirrors respawn, including the profile the successor is built on.
        ///
        /// This used to deliberately DROP `spec.profile`, because respawn
        /// preserved the predecessor's profile and a double that applied the
        /// requested one could reprofile where Meerkat could not - the capability
        /// tests would then have gone green against a capability that did not
        /// exist.
        ///
        /// It exists now. `respawn_with_successor_spec` applies the successor
        /// spec atomically, proven end to end on the real mob handle by
        /// identity_first_builder_reset_reprofiles_{from_current_roster_provider,
        /// with_roster_context}, which assert status().profile through no double
        /// at all. So recording the profile now mirrors production rather than
        /// overstating it - and the ORDER mattered: the production path was
        /// proven first, and only then was this unlocked.
        async fn reset_member_to_successor(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<crate::identity_first::bridge::MemberRuntimeBinding, BridgeError> {
            // The successor IS built on the requested spec now, so the double
            // publishes the profile the same way create does.
            self.create_profiles
                .lock()
                .await
                .push(spec.profile.as_str().to_string());
            let generation = self.successor_generations.lock().await.len() as u64 + 1;
            let alias = format!("rt:{}:{generation}", identity.as_str());
            self.successor_generations.lock().await.push(alias.clone());
            let agent_runtime_id = AgentRuntimeId::parse(&alias).map_err(|error| {
                BridgeError::Mob(format!("test double minted an unusable successor: {error}"))
            })?;
            let binding = crate::identity_first::bridge::MemberRuntimeBinding {
                agent_runtime_id,
                session_id: Some(SessionId::new()),
            };
            self.core.bind(identity, &binding, &spec.profile);
            Ok(binding)
        }

        async fn create_session(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<crate::identity_first::MemberRuntimeBinding, BridgeError> {
            let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            let session_id = &meerkat_core::types::SessionId::new();
            let in_flight = self.creates_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_creates_in_flight
                .fetch_max(in_flight, Ordering::SeqCst);
            tokio::time::sleep(self.create_delay).await;
            self.creates_in_flight.fetch_sub(1, Ordering::SeqCst);
            self.create_profiles
                .lock()
                .await
                .push(spec.profile.to_string());
            let binding = crate::identity_first::MemberRuntimeBinding {
                agent_runtime_id,
                session_id: Some(session_id.clone()),
            };
            self.core.bind(identity, &binding, &spec.profile);
            Ok(binding)
        }

        /// Core mints a held member's incarnation too. Generation 7 is one
        /// the runtime never mints, so a status that names it was adopted.
        async fn create_session_held(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            _pass: &super::super::held_bootstrap::HeldCreatePass,
        ) -> Result<crate::identity_first::MemberRuntimeBinding, BridgeError> {
            self.held_creates.lock().await.push(identity.to_string());
            let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:7"))
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            let binding = crate::identity_first::MemberRuntimeBinding {
                agent_runtime_id,
                session_id: Some(SessionId::new()),
            };
            self.core.bind(identity, &binding, &spec.profile);
            Ok(binding)
        }

        async fn resume_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            _session_id: &SessionId,
            _resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            Err(BridgeError::Mob(
                "resume not used in reset test".to_string(),
            ))
        }

        async fn deliver_admitted(
            &self,
            _runtime_id: &AgentRuntimeId,
            _delivery: BridgeDelivery,
        ) -> Result<SessionId, BridgeError> {
            Err(BridgeError::Mob(
                "deliver not used in reset test".to_string(),
            ))
        }

        async fn checkpoint_session(
            &self,
            _runtime_id: &AgentRuntimeId,
            _session_id: &SessionId,
        ) -> Result<SessionSnapshot, BridgeError> {
            Err(BridgeError::Mob(
                "checkpoint not used in reset test".to_string(),
            ))
        }

        async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            self.core.retire(runtime_id);
            self.retired_runtime_ids
                .lock()
                .await
                .push(runtime_id.to_string());
            Ok(())
        }

        async fn inspect_member(
            &self,
            _runtime_id: &AgentRuntimeId,
        ) -> Result<MemberInspection, BridgeError> {
            Err(BridgeError::Mob(
                "inspect not used in reset test".to_string(),
            ))
        }

        async fn live_member(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn live_member_scope(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
            self.core.live_member_scope(identity)
        }

        async fn ensure_member_live(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn last_member_binding(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
            self.core.last_member_binding(identity)
        }

        async fn member_binding_history(
            &self,
        ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
            self.core.history()
        }

        async fn durable_identity_members(
            &self,
        ) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
            self.core.durable_identity_members()
        }

        async fn identity_management(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
            Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
        }

        async fn register_session_runtime_state(
            &self,
            _session_id: &SessionId,
            _identity: &AgentIdentity,
            _generation: ContinuityGeneration,
            checkpoint_version: CheckpointVersion,
            _fencing_token: FencingToken,
        ) -> Result<CheckpointVersion, BridgeError> {
            Ok(checkpoint_version)
        }

        async fn suspend_session_runtime_state(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn unregister_session_runtime_state(
            &self,
            _session_id: &SessionId,
        ) -> Result<(), BridgeError> {
            Ok(())
        }
    }

    /// Bridge used to prove REQ-33 metadata hot reloads never cross the
    /// retire/resume boundary. Initial creation and ordinary dispatch work;
    /// any attempted resume fails loudly, reproducing the real gateway's
    /// still-draining-member collision without an LLM or network dependency.
    #[derive(Default)]
    struct HotReloadBridge {
        sessions: AsyncMutex<BTreeMap<String, SessionId>>,
        retire_calls: AtomicUsize,
        resume_calls: AtomicUsize,
        core: crate::identity_first::core_test_members::CoreTestMembers,
    }

    #[async_trait::async_trait]
    impl SessionBridge for HotReloadBridge {
        async fn create_session(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<crate::identity_first::MemberRuntimeBinding, BridgeError> {
            let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            let runtime_id = &agent_runtime_id;
            let session_id = &meerkat_core::types::SessionId::new();
            self.sessions
                .lock()
                .await
                .insert(runtime_id.to_string(), session_id.clone());
            let binding = crate::identity_first::MemberRuntimeBinding {
                agent_runtime_id,
                session_id: Some(session_id.clone()),
            };
            self.core.bind(identity, &binding, &spec.profile);
            Ok(binding)
        }

        async fn resume_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            _session_id: &SessionId,
            _resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            self.resume_calls.fetch_add(1, Ordering::SeqCst);
            Err(BridgeError::Mob(
                "same-profile metadata hot reload attempted session resume".to_string(),
            ))
        }

        async fn deliver_admitted(
            &self,
            runtime_id: &AgentRuntimeId,
            _delivery: BridgeDelivery,
        ) -> Result<SessionId, BridgeError> {
            self.sessions
                .lock()
                .await
                .get(runtime_id.as_str())
                .cloned()
                .ok_or_else(|| BridgeError::Mob(format!("missing session for {runtime_id}")))
        }

        async fn checkpoint_session(
            &self,
            _runtime_id: &AgentRuntimeId,
            _session_id: &SessionId,
        ) -> Result<SessionSnapshot, BridgeError> {
            Err(BridgeError::Mob(
                "checkpoint not used in hot-reload test".to_string(),
            ))
        }

        async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            self.retire_calls.fetch_add(1, Ordering::SeqCst);
            self.sessions.lock().await.remove(runtime_id.as_str());
            self.core.retire(runtime_id);
            Ok(())
        }

        async fn inspect_member(
            &self,
            _runtime_id: &AgentRuntimeId,
        ) -> Result<MemberInspection, BridgeError> {
            Err(BridgeError::Mob(
                "inspect not used in hot-reload test".to_string(),
            ))
        }

        async fn live_member(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn live_member_scope(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
            self.core.live_member_scope(identity)
        }

        async fn ensure_member_live(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn last_member_binding(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
            self.core.last_member_binding(identity)
        }

        async fn member_binding_history(
            &self,
        ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
            self.core.history()
        }

        async fn durable_identity_members(
            &self,
        ) -> Result<Vec<meerkat_mob::MemberBinding>, BridgeError> {
            self.core.durable_identity_members()
        }

        async fn identity_management(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
            Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
        }
    }

    fn durable_spec(identity: AgentIdentity, profile: &str) -> DurableAgentSpec {
        DurableAgentSpec {
            identity,
            profile: meerkat_mob::ProfileName::from(profile),
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
        }
    }

    /// Records the run-start hold releases the identity runtime asks for;
    /// `fail` makes each release fail like a refused mob command.
    #[derive(Default)]
    struct RunStartReleaseBridge {
        fail: bool,
        released: std::sync::Mutex<Vec<String>>,
    }

    impl RunStartReleaseBridge {
        fn released(&self) -> Vec<String> {
            self.released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl SessionBridge for RunStartReleaseBridge {
        async fn create_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<crate::identity_first::MemberRuntimeBinding, BridgeError> {
            Err(BridgeError::Mob(
                "create not used in release test".to_string(),
            ))
        }

        async fn resume_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            _session_id: &SessionId,
            _resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            Err(BridgeError::Mob(
                "resume not used in release test".to_string(),
            ))
        }

        async fn deliver_admitted(
            &self,
            _runtime_id: &AgentRuntimeId,
            _delivery: BridgeDelivery,
        ) -> Result<SessionId, BridgeError> {
            Err(BridgeError::Mob(
                "deliver not used in release test".to_string(),
            ))
        }

        async fn checkpoint_session(
            &self,
            _runtime_id: &AgentRuntimeId,
            _session_id: &SessionId,
        ) -> Result<SessionSnapshot, BridgeError> {
            Err(BridgeError::Mob(
                "checkpoint not used in release test".to_string(),
            ))
        }

        async fn retire_member(&self, _runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn release_member_run_starts_for_published_tools(
            &self,
            member_id: &meerkat_mob::AgentIdentity,
        ) -> Result<(), BridgeError> {
            self.released
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(member_id.to_string());
            if self.fail {
                return Err(BridgeError::Mob("mob refused the release".to_string()));
            }
            Ok(())
        }
    }

    /// A restored member whose early `customize_build` failed
    /// is held by the mob build (`ToolsNotPublished`). The publication that
    /// clears its pending record releases that hold exactly once; publishing
    /// for an identity that was not pending releases nothing; a failed
    /// release keeps the identity pending with the error, so a member left
    /// held is visible in its status.
    #[tokio::test]
    async fn publishing_a_pending_identitys_tools_releases_its_run_start_hold()
    -> Result<(), Box<dyn std::error::Error>> {
        for fail in [false, true] {
            let identity = AgentIdentity::parse("personal:alice")?;
            let spec = durable_spec(identity.clone(), "personal");
            let bridge = Arc::new(RunStartReleaseBridge {
                fail,
                ..Default::default()
            });
            let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: "run-start-release-test".to_string(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(bridge.clone()),
                default_timeout: None,
            });
            runtime
                .set_customizer_tool_registry(Some(
                    super::super::customizer_tools::CustomizerToolRegistry::new(),
                ))
                .await;
            runtime.record_customizer_tools_pending(BTreeMap::from([(
                identity.clone(),
                CustomizerToolsPending {
                    reason: "pre-activation customize_build failed: host down".to_string(),
                    artifact_refusal: None,
                },
            )]));
            let draft = || AgentBuildDraft {
                model: None,
                system_prompt: None,
                additional_instructions: spec.additional_instructions.clone(),
                labels: spec.labels.clone(),
                app_context: spec.context.clone(),
                external_tools: Vec::new(),
                local_external_tools: Default::default(),
                provider_params: None,
                compaction_curator: Default::default(),
            };
            runtime
                .publish_customizer_tools(&identity, &mut draft())
                .await;
            let member = crate::member_comms_id::mob_member_id(identity.as_str()).to_string();
            assert_eq!(bridge.released(), vec![member.clone()]);
            let pending = runtime.customizer_tools_pending_for(&identity);
            if fail {
                let pending = pending.ok_or("a failed release keeps the identity pending")?;
                assert!(
                    pending.reason.contains("customizer tools published, but")
                        && pending.reason.contains("mob refused the release"),
                    "{pending:?}"
                );
            } else {
                assert!(pending.is_none(), "{pending:?}");
                // Not pending any more: a later publication releases nothing.
                runtime
                    .publish_customizer_tools(&identity, &mut draft())
                    .await;
                assert_eq!(bridge.released(), vec![member]);
            }
        }
        Ok(())
    }

    /// A candidate launch releases no member's run-start hold. Its mob build
    /// held every stored member, and the customizer tools of a restored
    /// identity still publish, through either publication path: the
    /// identity is no longer pending, and the bridge was asked to release
    /// nothing, so the member stays held.
    #[tokio::test]
    async fn a_candidate_launch_publishes_customizer_tools_and_releases_no_hold()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("personal:alice")?;
        let spec = durable_spec(identity.clone(), "personal");
        let bridge = Arc::new(RunStartReleaseBridge::default());
        let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "candidate-run-start-release-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        });
        runtime.mark_candidate_launch();
        runtime
            .set_customizer_tool_registry(Some(
                super::super::customizer_tools::CustomizerToolRegistry::new(),
            ))
            .await;
        let pending = || {
            BTreeMap::from([(
                identity.clone(),
                CustomizerToolsPending {
                    reason: "pre-activation customize_build failed: host down".to_string(),
                    artifact_refusal: None,
                },
            )])
        };
        let mut draft = AgentBuildDraft {
            model: None,
            system_prompt: None,
            additional_instructions: spec.additional_instructions.clone(),
            labels: spec.labels.clone(),
            app_context: spec.context.clone(),
            external_tools: Vec::new(),
            local_external_tools: Default::default(),
            provider_params: None,
            compaction_curator: Default::default(),
        };

        // A wake's or delivery's publication.
        runtime.record_customizer_tools_pending(pending());
        runtime
            .publish_customizer_tools(&identity, &mut draft)
            .await;
        assert!(runtime.customizer_tools_pending_for(&identity).is_none());
        // The publication before the lift, released once the mob is live.
        runtime.record_customizer_tools_pending(pending());
        runtime
            .release_customizer_tool_holds(std::slice::from_ref(&identity))
            .await;
        assert!(runtime.customizer_tools_pending_for(&identity).is_none());

        assert_eq!(
            bridge.released(),
            Vec::<String>::new(),
            "a candidate launch asks for no release"
        );
        Ok(())
    }

    /// The classifier reads the typed refusal (its kind) and the host gate's
    /// minted text; everything else is not a deterministic wall.
    #[test]
    fn deterministic_build_rejection_reason_reads_the_typed_kind()
    -> Result<(), Box<dyn std::error::Error>> {
        let refusal = |kind| BridgeError::ProviderAuthRejected {
            failure: meerkat_core::service::SessionProviderAuthFailure {
                kind,
                provider: meerkat_core::Provider::OpenAI,
                realm_id: None,
                binding_id: None,
            },
            detail: "create_session: refused".to_string(),
        };
        let reason = deterministic_build_rejection_reason(&refusal(
            meerkat_core::AuthErrorKind::MissingSecret,
        ))
        .ok_or("a missing secret is a deterministic wall")?;
        assert!(
            reason.contains("provider=openai kind=missing_secret"),
            "{reason}"
        );
        assert!(
            reason.contains("Refusal: create_session: refused"),
            "{reason}"
        );
        for transient in [
            meerkat_core::AuthErrorKind::Expired,
            meerkat_core::AuthErrorKind::RefreshFailed,
            meerkat_core::AuthErrorKind::InteractiveLoginRequired,
            meerkat_core::AuthErrorKind::Io,
            meerkat_core::AuthErrorKind::CredentialAccessDenied,
            meerkat_core::AuthErrorKind::CredentialInteractionRequired,
            meerkat_core::AuthErrorKind::CredentialAccessPending,
            meerkat_core::AuthErrorKind::CredentialStoreUnavailable,
        ] {
            assert!(
                deterministic_build_rejection_reason(&refusal(transient)).is_none(),
                "{transient:?} can change out of band and must not park"
            );
        }
        let realm = deterministic_build_rejection_reason(&BridgeError::RealmBackendUnavailable {
            detail: "create_session: durable sessions required".to_string(),
        })
        .ok_or("a host without a persistent realm backend is a deterministic wall")?;
        assert!(
            realm.starts_with("create_session: durable sessions required; "),
            "{realm}"
        );
        assert!(
            realm.contains(
                "run that host with a persistent realm backend, or remove the identity's \
                 placement"
            ),
            "the reason names the remedy: {realm}"
        );
        let durable = deterministic_build_rejection_reason(&BridgeError::NonDurableService {
            profile: meerkat_mob::ProfileName::from("durable-worker"),
            detail: "create_session: no persistent sessions".to_string(),
        })
        .ok_or("a durable profile on a mob without persistent sessions is a deterministic wall")?;
        assert!(
            durable.starts_with("create_session: no persistent sessions; "),
            "{durable}"
        );
        assert!(
            durable.contains("run the mob on a persistent realm, or drop durable from the profile"),
            "the reason names the remedy: {durable}"
        );
        assert!(durable.contains("'durable-worker'"), "{durable}");
        let host_gate = BridgeError::Mob(
            "spawn_member: callback/build_agent failed: callback error: gate refused".to_string(),
        );
        assert!(deterministic_build_rejection_reason(&host_gate).is_some());
        let transport = BridgeError::Mob("callback transport closed".to_string());
        assert!(deterministic_build_rejection_reason(&transport).is_none());
        Ok(())
    }

    #[test]
    fn external_delivery_omits_unrepresentable_interaction_id_carrier()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("target-smoke")?;
        let mut external = durable_spec(identity.clone(), "target");
        external.backend = Some(meerkat_mob::MobBackendKind::External);
        assert_eq!(
            interaction_id_for_delivery(&external, Some("interaction-1")),
            None
        );

        let local = durable_spec(identity, "target");
        assert_eq!(
            interaction_id_for_delivery(&local, Some("interaction-1")),
            Some("interaction-1")
        );
        Ok(())
    }

    async fn active_alias_runtime(
        runtime_instance_id: &str,
        identity: &str,
    ) -> Result<(Arc<IdentityRuntime>, String), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse(identity)?;
        let alias = format!("rt:{identity}:0");
        // Core holds the identity's live member at generation 0, so the
        // generated alias names its current incarnation.
        let bridge = Arc::new(RecordingBridge::default());
        bridge.core.bind(
            &identity,
            &MemberRuntimeBinding {
                agent_runtime_id: AgentRuntimeId::parse(&alias)?,
                session_id: Some(SessionId::new()),
            },
            &meerkat_mob::ProfileName::from("domain"),
        );
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: runtime_instance_id.to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        }));
        runtime.register(durable_spec(identity, "domain")).await;
        Ok((runtime, alias))
    }

    async fn alias_target(
        runtime: &Arc<IdentityRuntime>,
        alias: &str,
    ) -> Result<MemberAliasLifecycleTarget, Box<dyn std::error::Error>> {
        runtime
            .member_alias_lifecycle_target(alias)
            .await?
            .ok_or_else(|| {
                format!("generated alias did not resolve to lifecycle target: {alias}").into()
            })
    }

    #[tokio::test]
    async fn compound_alias_targets_sort_opposite_orders_without_deadlock()
    -> Result<(), Box<dyn std::error::Error>> {
        let (runtime_a, alias_a) = active_alias_runtime("00-alias-runtime", "domain:alpha").await?;
        let (runtime_b, alias_b) = active_alias_runtime("01-alias-runtime", "domain:beta").await?;

        // Hold the globally first lock while both transactions are admitted.
        // Correct ordering makes both wait on A without touching B. An input-
        // ordered implementation lets the reverse request take B first and
        // then deadlocks once the forward request receives A.
        let held_a = alias_target(&runtime_a, &alias_a)
            .await?
            .lock
            .clone()
            .lock_owned()
            .await;
        let completed = Arc::new(AtomicUsize::new(0));

        let forward = tokio::spawn({
            let targets = vec![
                alias_target(&runtime_a, &alias_a).await?,
                alias_target(&runtime_b, &alias_b).await?,
            ];
            let completed = Arc::clone(&completed);
            async move {
                IdentityRuntime::run_member_alias_targets_operation_tracked(targets, move || {
                    let completed = Arc::clone(&completed);
                    async move {
                        completed.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                })
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if !runtime_a.foreground_operations.lock().await.is_empty() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;

        let reverse = tokio::spawn({
            let targets = vec![
                alias_target(&runtime_b, &alias_b).await?,
                alias_target(&runtime_a, &alias_a).await?,
            ];
            let completed = Arc::clone(&completed);
            async move {
                IdentityRuntime::run_member_alias_targets_operation_tracked(targets, move || {
                    let completed = Arc::clone(&completed);
                    async move {
                        completed.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                })
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if runtime_a.foreground_operations.lock().await.len() >= 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await?;
        tokio::task::yield_now().await;

        let b_probe = alias_target(&runtime_b, &alias_b)
            .await?
            .lock
            .try_lock_owned()
            .map_err(|_| "reverse-order request acquired B before globally-first A")?;
        drop(b_probe);
        drop(held_a);

        tokio::time::timeout(Duration::from_secs(2), forward).await???;
        tokio::time::timeout(Duration::from_secs(2), reverse).await???;
        assert_eq!(completed.load(Ordering::SeqCst), 2);

        runtime_a.close_foreground_operations();
        runtime_b.close_foreground_operations();
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(
                runtime_a.join_foreground_operations(),
                runtime_b.join_foreground_operations()
            );
        })
        .await?;
        Ok(())
    }

    #[tokio::test]
    async fn dropped_compound_alias_caller_still_completes_operation_boundary()
    -> Result<(), Box<dyn std::error::Error>> {
        let (runtime_a, alias_a) = active_alias_runtime("00-drop-runtime", "domain:alpha").await?;
        let (runtime_b, alias_b) = active_alias_runtime("01-drop-runtime", "domain:beta").await?;
        let targets = vec![
            alias_target(&runtime_a, &alias_a).await?,
            alias_target(&runtime_b, &alias_b).await?,
        ];
        let entered = Arc::new(tokio::sync::Semaphore::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let completed = Arc::new(AtomicUsize::new(0));
        let caller = tokio::spawn({
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            let completed = Arc::clone(&completed);
            async move {
                IdentityRuntime::run_member_alias_targets_operation_tracked(targets, move || {
                    let entered = Arc::clone(&entered);
                    let release = Arc::clone(&release);
                    let completed = Arc::clone(&completed);
                    async move {
                        entered.add_permits(1);
                        release
                            .acquire()
                            .await
                            .map_err(|error| error.to_string())?
                            .forget();
                        completed.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                })
                .await
            }
        });
        tokio::time::timeout(Duration::from_secs(2), entered.acquire())
            .await??
            .forget();

        caller.abort();
        match caller.await {
            Err(error) => assert!(error.is_cancelled()),
            Ok(result) => {
                return Err(format!("aborted caller unexpectedly returned: {result:?}").into());
            }
        }
        release.add_permits(1);

        runtime_a.close_foreground_operations();
        runtime_b.close_foreground_operations();
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(
                runtime_a.join_foreground_operations(),
                runtime_b.join_foreground_operations()
            );
        })
        .await?;
        assert_eq!(
            completed.load(Ordering::SeqCst),
            1,
            "runtime-owned compound transaction must reach its boundary"
        );
        Ok(())
    }

    #[tokio::test]
    async fn either_compound_alias_runtime_shutdown_waits_for_operation_boundary()
    -> Result<(), Box<dyn std::error::Error>> {
        for shutdown_first_runtime in [true, false] {
            let suffix = if shutdown_first_runtime {
                "first"
            } else {
                "second"
            };
            let (runtime_a, alias_a) =
                active_alias_runtime(&format!("00-shutdown-{suffix}"), "domain:alpha").await?;
            let (runtime_b, alias_b) =
                active_alias_runtime(&format!("01-shutdown-{suffix}"), "domain:beta").await?;
            let targets = vec![
                alias_target(&runtime_a, &alias_a).await?,
                alias_target(&runtime_b, &alias_b).await?,
            ];
            let entered = Arc::new(tokio::sync::Semaphore::new(0));
            let release = Arc::new(tokio::sync::Semaphore::new(0));
            let operation = tokio::spawn({
                let entered = Arc::clone(&entered);
                let release = Arc::clone(&release);
                async move {
                    IdentityRuntime::run_member_alias_targets_operation_tracked(
                        targets,
                        move || {
                            let entered = Arc::clone(&entered);
                            let release = Arc::clone(&release);
                            async move {
                                entered.add_permits(1);
                                release
                                    .acquire()
                                    .await
                                    .map_err(|error| error.to_string())?
                                    .forget();
                                Ok(())
                            }
                        },
                    )
                    .await
                }
            });
            tokio::time::timeout(Duration::from_secs(2), entered.acquire())
                .await??
                .forget();

            let shutting_down = if shutdown_first_runtime {
                Arc::clone(&runtime_a)
            } else {
                Arc::clone(&runtime_b)
            };
            shutting_down.close_foreground_operations();
            let mut shutdown = tokio::spawn(async move {
                shutting_down.join_foreground_operations().await;
            });
            assert!(
                tokio::time::timeout(Duration::from_millis(50), &mut shutdown)
                    .await
                    .is_err(),
                "shutdown of {suffix} participating runtime returned before compound boundary"
            );

            release.add_permits(1);
            tokio::time::timeout(Duration::from_secs(2), &mut shutdown).await??;
            tokio::time::timeout(Duration::from_secs(2), operation).await???;

            runtime_a.close_foreground_operations();
            runtime_b.close_foreground_operations();
            tokio::time::timeout(Duration::from_secs(2), async {
                tokio::join!(
                    runtime_a.join_foreground_operations(),
                    runtime_b.join_foreground_operations()
                );
            })
            .await?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn restore_flow_bounds_parallel_member_creation() -> Result<(), Box<dyn std::error::Error>>
    {
        let specs = (0..8)
            .map(|index| {
                Ok(durable_spec(
                    AgentIdentity::parse(&format!("domain:restore-{index}"))?,
                    "domain",
                ))
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        let bridge = Arc::new(RecordingBridge {
            create_delay: Duration::from_millis(25),
            ..RecordingBridge::default()
        });
        let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "parallel-restore-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        });

        let result = super::super::orchestrator::restore_flow(&runtime, &specs, None, None).await?;

        assert_eq!(result.outcomes.len(), specs.len());
        assert!(
            bridge.max_creates_in_flight() > 1,
            "member creation should no longer be serial"
        );
        assert!(
            bridge.max_creates_in_flight()
                <= super::super::orchestrator::IDENTITY_RESTORE_CONCURRENCY,
            "restore concurrency must remain bounded"
        );
        Ok(())
    }

    #[tokio::test]
    async fn reset_reprofiles_session_from_runtime_configured_roster_provider()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("domain:security")?;
        let roster = Arc::new(MutableRoster::new(vec![durable_spec(
            identity.clone(),
            "domain",
        )]));
        let bridge = Arc::new(RecordingBridge::default());
        let runtime = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: "reset-reprofile-test".to_string(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(bridge.clone()),
                default_timeout: None,
            })
            .with_reset_roster_provider(roster.clone()),
        );

        super::super::orchestrator::restore_flow(
            &runtime,
            &roster
                .roster(&RosterContext {
                    mob_definition: None,
                    previous_identities: Vec::new(),
                })
                .await?,
            None,
            None,
        )
        .await?;
        roster
            .set(vec![durable_spec(identity.clone(), "security")])
            .await;

        let binding = runtime.reset(&identity).await?;

        assert_eq!(binding.generation().get(), 1);
        assert_eq!(
            bridge.create_profiles().await,
            vec!["domain".to_string(), "security".to_string()]
        );
        let status = runtime.status(&identity).await?;
        assert_eq!(
            status.profile.map(|profile| profile.to_string()).as_deref(),
            Some("security")
        );
        Ok(())
    }

    /// The held create arm adopts the incarnation Core committed, like the
    /// plain arm: the materialized scope and the status name the runtime id
    /// Core minted for the held member.
    #[tokio::test]
    async fn a_held_create_adopts_the_runtime_id_core_committed()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("domain:held")?;
        let bridge = Arc::new(RecordingBridge::default());
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "held-create-adoption-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        }));
        super::super::orchestrator::lazy_register_flow(
            &runtime,
            &[durable_spec(identity.clone(), "domain")],
            None,
        )
        .await?;
        *runtime
            .held_create_pass
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::new(super::super::held_bootstrap::HeldCreatePass::new(
                Arc::new(super::super::held_bootstrap::HeldTerminal::new(
                    meerkat_mob::HeldSpawnDeadline::now() + Duration::from_mins(1),
                )),
                [identity.clone()],
                1,
            )));

        let scope = runtime
            .materialize(&identity)
            .await?
            .ok_or("the held create left no live member")?;

        assert_eq!(
            bridge.held_creates.lock().await.clone(),
            vec![identity.to_string()],
            "the held arm created the member"
        );
        assert!(
            bridge.create_profiles().await.is_empty(),
            "the plain create arm never ran"
        );
        let committed = AgentRuntimeId::parse("rt:domain:held:7")?;
        assert_eq!(
            crate::member_comms_id::public_runtime_alias(scope.runtime_id()),
            committed.as_str()
        );
        let status = runtime.status(&identity).await?;
        assert_eq!(
            status.agent_runtime_id.as_ref(),
            Some(&committed),
            "the status must name the incarnation Core committed on the held create"
        );
        Ok(())
    }

    /// A reset roster provider that refuses authoritatively once told to:
    /// the shape a prepared application's failed projection takes.
    struct RefusingRoster {
        inner: Arc<MutableRoster>,
        refuse: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl RosterProvider for RefusingRoster {
        async fn roster(
            &self,
            context: &RosterContext,
        ) -> Result<Vec<DurableAgentSpec>, RosterError> {
            if self.refuse.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(RosterError::Refused("selected helper missing".into()));
            }
            self.inner.roster(context).await
        }
    }

    /// An authoritative roster refusal refuses the reset before it fences or
    /// advances anything, instead of rebuilding on the stored spec.
    #[tokio::test]
    async fn reset_is_refused_when_the_roster_refuses_authoritatively()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("domain:security")?;
        let roster = Arc::new(MutableRoster::new(vec![durable_spec(
            identity.clone(),
            "domain",
        )]));
        let refusing = Arc::new(RefusingRoster {
            inner: roster.clone(),
            refuse: std::sync::atomic::AtomicBool::new(false),
        });
        let bridge = Arc::new(RecordingBridge::default());
        let runtime = Arc::new(
            IdentityRuntime::new(IdentityRuntimeConfig {
                continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
                lease_provider: Arc::new(LocalLeaseProvider::new()),
                runtime_instance_id: "reset-refused-test".to_string(),
                has_runtime_store: true,
                durability_policy: DurabilityPolicy::SyncWriteThrough,
                bridge: Some(bridge.clone()),
                default_timeout: None,
            })
            .with_reset_roster_provider(refusing.clone()),
        );
        super::super::orchestrator::restore_flow(
            &runtime,
            &roster
                .roster(&RosterContext {
                    mob_definition: None,
                    previous_identities: Vec::new(),
                })
                .await?,
            None,
            None,
        )
        .await?;
        let before = runtime.status(&identity).await?;
        refusing
            .refuse
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let Err(error) = runtime.reset(&identity).await else {
            return Err("the refusal must refuse the reset".into());
        };
        assert!(error.to_string().contains("refused"), "{error}");
        assert_eq!(bridge.create_profiles().await, vec!["domain".to_string()]);
        let after = runtime.status(&identity).await?;
        assert_eq!(after.generation, before.generation, "nothing advanced");
        assert_eq!(
            after.profile.map(|profile| profile.to_string()).as_deref(),
            Some("domain")
        );
        Ok(())
    }

    #[tokio::test]
    async fn stale_alias_materialization_preserves_active_and_dormant_bootstrap_status()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("domain:bootstrap-alias")?;
        let spec = durable_spec(identity.clone(), "domain");
        let stale_alias = "rt:domain:bootstrap-alias:0";
        // Core holds the identity's live member at generation 1.
        let bridge = Arc::new(RecordingBridge::default());
        bridge.core.bind(
            &identity,
            &MemberRuntimeBinding {
                agent_runtime_id: AgentRuntimeId::parse("rt:domain:bootstrap-alias:1")?,
                session_id: Some(SessionId::new()),
            },
            &spec.profile,
        );
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "bootstrap-alias-status".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        }));
        runtime.register(spec.clone()).await;

        let active_generation =
            runtime.begin_identity_bootstrap_pending(IdentityBootstrapMode::LazyMaterialize);
        runtime
            .begin_identity_bootstrap(
                active_generation,
                IdentityBootstrapMode::LazyMaterialize,
                std::slice::from_ref(&spec),
            )
            .await;
        runtime.modify_bootstrap_status(Some(active_generation), |status| {
            status.complete = true;
            status.refresh_aggregates();
        });
        let active_before = runtime.identity_bootstrap_status();
        assert_eq!(
            active_before
                .identities
                .get(&identity)
                .map(|entry| entry.state),
            Some(IdentityBootstrapState::Active)
        );
        assert!(matches!(
            runtime
                .materialize_with_expected_member_alias(&identity, Some(stale_alias))
                .await,
            Err(IdentityRuntimeError::StaleRuntimeAlias { .. })
        ));
        assert_eq!(runtime.identity_bootstrap_status(), active_before);

        runtime.retire(&identity).await?;
        let dormant_generation =
            runtime.begin_identity_bootstrap_pending(IdentityBootstrapMode::LazyMaterialize);
        runtime
            .begin_identity_bootstrap(
                dormant_generation,
                IdentityBootstrapMode::LazyMaterialize,
                std::slice::from_ref(&spec),
            )
            .await;
        runtime.modify_bootstrap_status(Some(dormant_generation), |status| {
            status.complete = true;
            status.refresh_aggregates();
        });
        let dormant_before = runtime.identity_bootstrap_status();
        assert_eq!(
            dormant_before
                .identities
                .get(&identity)
                .map(|entry| entry.state),
            Some(IdentityBootstrapState::Dormant)
        );
        assert!(matches!(
            runtime
                .materialize_with_expected_member_alias(&identity, Some(stale_alias))
                .await,
            Err(IdentityRuntimeError::StaleRuntimeAlias { .. })
        ));
        assert_eq!(runtime.identity_bootstrap_status(), dormant_before);
        Ok(())
    }

    #[tokio::test]
    async fn eager_reconcile_hot_reloads_active_metadata_without_retire_or_resume()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("identity:luka")?;
        let mut initial_spec = durable_spec(identity.clone(), "personal");
        initial_spec
            .labels
            .insert("revision".to_string(), "v1".to_string());
        let roster = Arc::new(MutableRoster::new(vec![initial_spec.clone()]));
        let bridge = Arc::new(HotReloadBridge::default());
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "eager-hot-reload-contract".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        }));
        let context = IdentityFirstRuntimeContext::new_with_bootstrap_mode(
            runtime.clone(),
            roster.clone(),
            None,
            None,
            None,
            IdentityBootstrapMode::EagerMaterialize,
        );
        context
            .bootstrap_roster(std::slice::from_ref(&initial_spec))
            .await?;

        let before = runtime.status(&identity).await?;
        assert_eq!(before.state, IdentityLifecycleState::Active);
        let before_session = before.session_id.clone();
        let before_runtime_id = before.agent_runtime_id.clone();
        let before_generation = before.generation;
        let before_fence = bridge
            .core
            .live_member_scope(&identity)?
            .ok_or("the eager bootstrap left no live member")?
            .fence_token()
            .get();

        let mut updated_spec = initial_spec;
        updated_spec.addressability = AgentAddressability::InternalOnly;
        updated_spec
            .labels
            .insert("timezone".to_string(), "Europe/Stockholm".to_string());
        updated_spec
            .labels
            .insert("revision".to_string(), "v2".to_string());
        roster.set(vec![updated_spec]).await;

        let result = context.refresh_desired_topology().await?;
        assert!(matches!(
            result.outcomes.get(&identity),
            Some(super::super::orchestrator::RestoreOutcome::Resumed { .. })
        ));
        let after = runtime.status(&identity).await?;
        assert_eq!(after.state, IdentityLifecycleState::Active);
        assert_eq!(after.addressability, AgentAddressability::InternalOnly);
        assert_eq!(
            after.labels.get("timezone").map(String::as_str),
            Some("Europe/Stockholm")
        );
        assert_eq!(after.session_id, before_session);
        assert_eq!(after.agent_runtime_id, before_runtime_id);
        assert_eq!(after.generation, before_generation);
        assert_eq!(bridge.retire_calls.load(Ordering::SeqCst), 0);
        assert_eq!(bridge.resume_calls.load(Ordering::SeqCst), 0);

        assert!(matches!(
            runtime
                .send(
                    &identity,
                    &meerkat_core::ContentInput::Text("external".to_string()),
                )
                .await,
            Err(IdentityRuntimeError::NotAddressable(_))
        ));
        let (dispatch_token, durable) = runtime
            .dispatch(&identity, &DispatchInput::system("system notice"))
            .await?;
        assert_eq!(
            dispatch_token.get(),
            before_fence,
            "same-profile reconcile must keep Core's live incarnation"
        );
        assert!(durable);
        Ok(())
    }

    #[tokio::test]
    async fn foreground_materialization_completion_cannot_overwrite_newer_lazy_reconcile_status()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("domain:foreground-generation")?;
        let mut v1 = durable_spec(identity.clone(), "domain");
        v1.labels
            .insert("roster_revision".to_string(), "v1".to_string());
        let mut v2 = v1.clone();
        v2.profile = meerkat_mob::ProfileName::from("replacement");
        v2.labels
            .insert("roster_revision".to_string(), "v2".to_string());

        let roster = Arc::new(MutableRoster::new(vec![v1.clone()]));
        let bridge = Arc::new(RecordingBridge::default());
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "foreground-generation-fence".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        }));
        let context = IdentityFirstRuntimeContext::new_with_bootstrap_mode(
            runtime.clone(),
            roster.clone(),
            None,
            None,
            None,
            IdentityBootstrapMode::LazyMaterialize,
        );
        context.bootstrap_roster(std::slice::from_ref(&v1)).await?;
        let (v1_generation, initial) = runtime.identity_bootstrap_status_with_generation();
        assert!(initial.complete);
        assert!(!initial.ready);
        assert_eq!(
            initial.identities.get(&identity).map(|entry| entry.state),
            Some(IdentityBootstrapState::Dormant)
        );

        // Pause the v1 foreground call after its lifecycle transaction has
        // committed Active but before its outer readiness bookkeeping runs.
        // The lifecycle lock is free at this seam, so a v2 roster pass can
        // retire the v1 member and install a new lazy Dormant snapshot.
        let completion_entered = Arc::new(Notify::new());
        let release_completion = Arc::new(Notify::new());
        let materialize = tokio::spawn({
            let runtime = runtime.clone();
            let identity = identity.clone();
            let completion_entered = completion_entered.clone();
            let release_completion = release_completion.clone();
            async move {
                runtime
                    .materialize_with_expected_member_alias_after_inner(
                        &identity,
                        None,
                        None,
                        async move {
                            completion_entered.notify_one();
                            release_completion.notified().await;
                        },
                        TopologyAfterEmbody::Reconcile,
                        &mut false,
                    )
                    .await
            }
        });
        completion_entered.notified().await;
        assert_eq!(
            runtime.status(&identity).await?.state,
            IdentityLifecycleState::Active
        );

        roster.set(vec![v2]).await;
        context.refresh_desired_topology().await?;
        let after_v2 = runtime.identity_bootstrap_status();
        assert!(after_v2.complete);
        assert!(!after_v2.ready);
        assert_eq!(
            after_v2.identities.get(&identity).map(|entry| entry.state),
            Some(IdentityBootstrapState::Dormant)
        );
        let lifecycle = runtime.status(&identity).await?;
        assert_eq!(lifecycle.state, IdentityLifecycleState::Dormant);
        assert_eq!(
            lifecycle.labels.get("roster_revision").map(String::as_str),
            Some("v2")
        );
        assert_eq!(bridge.retired_runtime_ids().await.len(), 1);

        // Let the older v1 outer future run its terminal status update. Its
        // captured generation must make the update a no-op against v2.
        release_completion.notify_one();
        materialize.await??;
        assert_eq!(runtime.identity_bootstrap_status(), after_v2);
        assert_eq!(
            runtime.status(&identity).await?.state,
            IdentityLifecycleState::Dormant
        );
        assert_ne!(
            runtime.identity_bootstrap_status_with_generation().0,
            v1_generation
        );
        Ok(())
    }

    #[tokio::test]
    async fn foreground_materialization_uses_generation_of_reconcile_that_wins_lifecycle_lock()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("domain:foreground-reconcile-first")?;
        let spec = durable_spec(identity.clone(), "domain");
        let roster = Arc::new(MutableRoster::new(vec![spec.clone()]));
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "foreground-reconcile-first".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            // Identity lifecycle runs on Core's mob: the materialization
            // wakes the identity through a session bridge.
            bridge: Some(Arc::new(RecordingBridge::default())),
            default_timeout: None,
        }));
        let context = IdentityFirstRuntimeContext::new_with_bootstrap_mode(
            runtime.clone(),
            roster,
            None,
            None,
            None,
            IdentityBootstrapMode::LazyMaterialize,
        );
        context
            .bootstrap_roster(std::slice::from_ref(&spec))
            .await?;
        let first_generation = runtime.identity_bootstrap_status_with_generation().0;

        // Publish G+1 with an unchanged desired spec, then pause its reconcile
        // while it owns the lifecycle lock. This is the ordering that a
        // pre-lock global generation sample gets wrong: materialization starts
        // during G+1 and must operate on the G+1-stamped entry, even though the
        // entry itself was originally registered by G.
        let next_generation =
            runtime.begin_identity_bootstrap_pending(IdentityBootstrapMode::LazyMaterialize);
        assert_ne!(next_generation, first_generation);
        runtime
            .begin_identity_bootstrap(
                next_generation,
                IdentityBootstrapMode::LazyMaterialize,
                std::slice::from_ref(&spec),
            )
            .await;

        let reconcile_holds_lock = Arc::new(Notify::new());
        let release_reconcile = Arc::new(Notify::new());
        let paused = Arc::new(AtomicBool::new(false));
        let reconcile = tokio::spawn({
            let runtime = runtime.clone();
            let spec = spec.clone();
            let reconcile_holds_lock = reconcile_holds_lock.clone();
            let release_reconcile = release_reconcile.clone();
            let paused = paused.clone();
            async move {
                runtime
                    .reconcile_roster_members_after_lifecycle_lock(
                        std::slice::from_ref(&spec),
                        next_generation,
                        &mut Vec::new(),
                        move |_| {
                            let should_pause = !paused.swap(true, Ordering::SeqCst);
                            let reconcile_holds_lock = reconcile_holds_lock.clone();
                            let release_reconcile = release_reconcile.clone();
                            async move {
                                if should_pause {
                                    reconcile_holds_lock.notify_one();
                                    release_reconcile.notified().await;
                                }
                            }
                        },
                    )
                    .await
            }
        });
        reconcile_holds_lock.notified().await;

        let materialize = tokio::spawn({
            let runtime = runtime.clone();
            let identity = identity.clone();
            async move { runtime.materialize(&identity).await }
        });
        // Poll the spawned future until it queues behind the lifecycle owner.
        tokio::task::yield_now().await;
        release_reconcile.notify_one();
        reconcile.await??;
        materialize.await??;

        let (published_generation, status) = runtime.identity_bootstrap_status_with_generation();
        assert_eq!(published_generation, next_generation);
        assert_eq!(
            status.identities.get(&identity).map(|entry| entry.state),
            Some(IdentityBootstrapState::Active),
            "G+1 materialization completion was misbound to the older pass"
        );
        let entries = runtime.entries.read().await;
        assert_eq!(
            entries
                .get(&identity)
                .map(|entry| entry.bootstrap_generation),
            Some(next_generation),
            "unchanged roster acceptance must still stamp the new generation"
        );
        Ok(())
    }

    #[tokio::test]
    async fn reset_reprofiles_session_from_identity_first_context_roster_provider()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("domain:security")?;
        let roster = Arc::new(MutableRoster::new(vec![durable_spec(
            identity.clone(),
            "domain",
        )]));
        let bridge = Arc::new(RecordingBridge::default());
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "reset-reprofile-context-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        }));
        let context =
            IdentityFirstRuntimeContext::new(runtime.clone(), roster.clone(), None, None, None);

        context.refresh_desired_topology().await?;
        roster
            .set(vec![durable_spec(identity.clone(), "security")])
            .await;

        let binding = runtime.reset(&identity).await?;

        assert_eq!(binding.generation().get(), 1);
        assert_eq!(
            bridge.create_profiles().await,
            vec!["domain".to_string(), "security".to_string()]
        );
        let status = runtime.status(&identity).await?;
        assert_eq!(
            status.profile.map(|profile| profile.to_string()).as_deref(),
            Some("security")
        );
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod turn_ticket_tests {
    use super::*;

    fn keeper() -> AgentIdentity {
        AgentIdentity::parse("keeper").expect("identity")
    }

    fn text(text: &str) -> Result<TurnOutput, String> {
        Ok(TurnOutput::Text {
            text: text.to_string(),
            truncated: false,
        })
    }

    fn delivery_key(key: &str) -> DeliveryKey {
        delivery_key_in(&SESSION, key)
    }

    static SESSION: std::sync::LazyLock<SessionId> = std::sync::LazyLock::new(SessionId::new);

    fn delivery_key_in(session: &SessionId, key: &str) -> DeliveryKey {
        (
            AgentRuntimeId::parse("keeper-g1").expect("runtime id"),
            session.clone(),
            key.to_string(),
        )
    }

    #[test]
    fn trackability_is_typed_and_leaves_the_runtime_mode_to_the_live_entry() {
        assert_eq!(turn_untrackable(true, false, false, true), None);
        assert_eq!(
            turn_untrackable(false, false, false, true),
            Some(TurnUntrackable::NotDelivered)
        );
        assert_eq!(
            turn_untrackable(true, false, true, true),
            Some(TurnUntrackable::HostHumanInput)
        );
        assert_eq!(
            turn_untrackable(true, true, false, true),
            Some(TurnUntrackable::ExternallyBound)
        );
        assert_eq!(
            turn_untrackable(true, false, false, false),
            Some(TurnUntrackable::BridgeCannotReportOutput)
        );
        let keeper = keeper();
        assert_eq!(
            runtime_refused_tracking(&keeper, meerkat_mob::MobRuntimeMode::AutonomousHost, "x"),
            TurnTracking::Unavailable(TurnUntrackable::AutonomousHost)
        );
        assert_eq!(
            runtime_refused_tracking(&keeper, meerkat_mob::MobRuntimeMode::TurnDriven, "x"),
            TurnTracking::Unavailable(TurnUntrackable::RefusedByRuntime {
                mode: meerkat_mob::MobRuntimeMode::TurnDriven
            })
        );
    }

    #[test]
    fn a_redispatch_after_a_session_rotation_is_its_own_admission() {
        let mut outcomes = TurnOutcomes::default();
        let original = outcomes.admit(&keeper(), Some(delivery_key("evt-1")));
        outcomes.settle(&keeper(), original, text("answer"));
        assert_eq!(
            outcomes.reusable_delivery_ticket(&keeper(), &delivery_key("evt-1")),
            Some(original)
        );
        // Same incarnation and key, rotated session: its runtime ledger has
        // never seen the key, so nothing is reused.
        let rotated = SessionId::new();
        assert_eq!(
            outcomes.reusable_delivery_ticket(&keeper(), &delivery_key_in(&rotated, "evt-1")),
            None
        );
    }

    #[test]
    fn every_admission_gets_its_own_ticket() {
        let mut outcomes = TurnOutcomes::default();
        let first = outcomes.admit(&keeper(), Some(delivery_key("k")));
        let second = outcomes.admit(&keeper(), Some(delivery_key("k")));
        assert_ne!(first, second);
        outcomes.settle(&keeper(), first, text("first"));
        assert_eq!(outcomes.outcome(&keeper(), second), TurnOutcome::Pending);
    }

    #[test]
    fn only_the_owning_pending_record_is_settled() {
        let erin = AgentIdentity::parse("erin").expect("identity");
        let mut outcomes = TurnOutcomes::default();
        let ticket = outcomes.admit(&keeper(), None);
        // Another identity's waiter can never settle this record.
        outcomes.settle(&erin, ticket, text("erin's reply"));
        assert_eq!(outcomes.outcome(&keeper(), ticket), TurnOutcome::Pending);
        outcomes.settle(&keeper(), ticket, text("keeper's reply"));
        // A settled record is final: nothing overwrites it.
        outcomes.settle(&keeper(), ticket, Ok(TurnOutput::NoOwnResult));
        assert_eq!(
            outcomes.outcome(&keeper(), ticket),
            TurnOutcome::Completed {
                output: TurnOutput::Text {
                    text: "keeper's reply".to_string(),
                    truncated: false,
                }
            }
        );
        assert_eq!(outcomes.outcome(&erin, ticket), TurnOutcome::Unknown);
    }

    #[test]
    fn a_redispatch_reuses_a_pending_or_completed_admission_but_not_a_failed_one() {
        let erin = AgentIdentity::parse("erin").expect("identity");
        let mut outcomes = TurnOutcomes::default();
        let key = delivery_key("evt-1");
        let original = outcomes.admit(&keeper(), Some(key.clone()));
        assert_eq!(
            outcomes.reusable_delivery_ticket(&keeper(), &key),
            Some(original)
        );
        assert_eq!(outcomes.reusable_delivery_ticket(&erin, &key), None);
        outcomes.settle(&keeper(), original, text("answer"));
        assert_eq!(
            outcomes.reusable_delivery_ticket(&keeper(), &key),
            Some(original)
        );

        let failed_key = delivery_key("evt-2");
        let failed = outcomes.admit(&keeper(), Some(failed_key.clone()));
        outcomes.settle(&keeper(), failed, Err("boom".to_string()));
        assert_eq!(
            outcomes.reusable_delivery_ticket(&keeper(), &failed_key),
            None
        );
    }

    #[test]
    fn a_full_registry_forgets_settled_turns_before_pending_ones() {
        let mut outcomes = TurnOutcomes::default();
        let pending = outcomes.admit(&keeper(), None);
        let mut settled = Vec::new();
        for _ in 1..TRACKED_TURNS_CAP {
            let ticket = outcomes.admit(&keeper(), None);
            outcomes.settle(&keeper(), ticket, Ok(TurnOutput::Empty));
            settled.push(ticket);
        }
        // At the cap: the next admission forgets a SETTLED turn, never the
        // older pending one.
        let newest = outcomes.admit(&keeper(), None);
        assert_eq!(outcomes.outcome(&keeper(), pending), TurnOutcome::Pending);
        assert_eq!(outcomes.outcome(&keeper(), newest), TurnOutcome::Pending);
        let forgotten = settled
            .iter()
            .filter(|ticket| outcomes.outcome(&keeper(), **ticket) == TurnOutcome::Unknown)
            .count();
        assert_eq!(forgotten, 1);
    }

    #[test]
    fn a_long_turn_that_just_settled_is_not_the_next_eviction() {
        let mut outcomes = TurnOutcomes::default();
        // Admitted first, settles last.
        let long = outcomes.admit(&keeper(), Some(delivery_key("long")));
        let mut short = Vec::new();
        for _ in 2..TRACKED_TURNS_CAP {
            let ticket = outcomes.admit(&keeper(), None);
            outcomes.settle(&keeper(), ticket, Ok(TurnOutput::Empty));
            short.push(ticket);
        }
        std::thread::sleep(Duration::from_millis(2));
        outcomes.settle(&keeper(), long, text("long answer"));
        // Fill to the cap, then one more admission evicts one record: the one
        // settled longest ago, not the one admitted first.
        outcomes.admit(&keeper(), None);
        outcomes.admit(&keeper(), None);
        assert_eq!(
            outcomes.outcome(&keeper(), long),
            TurnOutcome::Completed {
                output: TurnOutput::Text {
                    text: "long answer".to_string(),
                    truncated: false,
                }
            }
        );
        let forgotten = short
            .iter()
            .filter(|ticket| outcomes.outcome(&keeper(), **ticket) == TurnOutcome::Unknown)
            .count();
        assert_eq!(forgotten, 1);
        // The index follows the records it names.
        assert_eq!(
            outcomes.reusable_delivery_ticket(&keeper(), &delivery_key("long")),
            Some(long)
        );
    }
}

#[cfg(test)]
mod completion_wait_tests {
    use super::*;
    use crate::identity_first::{ContinuityStoreError, LocalContinuityStore, LocalLeaseProvider};

    fn runtime() -> Result<Arc<IdentityRuntime>, ContinuityStoreError> {
        Ok(Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "completion-wait-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        })))
    }

    fn spec(identity: &AgentIdentity) -> DurableAgentSpec {
        DurableAgentSpec {
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
        }
    }

    /// A registered identity whose entry leaves the table (without passing
    /// through Retiring) ends a baseline wait as `IdentityGone`.
    #[tokio::test]
    async fn a_wait_ends_identity_gone_when_the_entry_leaves_the_table()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = runtime()?;
        let identity = AgentIdentity::parse("agent:gone")?;
        runtime.register(spec(&identity)).await;
        let baseline = runtime.completion_cursor(&identity).await;
        let wait = runtime.await_completion(&identity, Some(baseline), Duration::from_mins(10));
        let remove = async {
            tokio::task::yield_now().await;
            runtime.entries.write().await.remove(&identity);
        };
        let (outcome, ()) = tokio::join!(wait, remove);
        assert_eq!(outcome, CompletionWait::IdentityGone(baseline));
        Ok(())
    }

    /// A readiness wait (no baseline) on an identity that is not registered
    /// yet keeps waiting; it is not "gone".
    #[tokio::test(start_paused = true)]
    async fn a_readiness_wait_on_an_unregistered_identity_keeps_waiting()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = runtime()?;
        let identity = AgentIdentity::parse("agent:later")?;
        let outcome = runtime
            .await_completion(&identity, None, Duration::from_secs(5))
            .await;
        assert!(
            matches!(outcome, CompletionWait::TimedOut(_)),
            "{outcome:?}"
        );
        Ok(())
    }

    /// Shutdown ends both waits, typed.
    #[tokio::test]
    async fn waits_end_shutting_down_when_the_runtime_closes()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = runtime()?;
        let identity = AgentIdentity::parse("agent:closing")?;
        runtime.register(spec(&identity)).await;
        let baseline = runtime.completion_cursor(&identity).await;
        let completion =
            runtime.await_completion(&identity, Some(baseline), Duration::from_mins(10));
        let ticket = TurnTicket::parse(&uuid::Uuid::new_v4().to_string())?;
        let turn = runtime.wait_for_turn_outcome(&identity, ticket, Duration::from_mins(10));
        let close = async {
            tokio::task::yield_now().await;
            runtime.close_foreground_operations();
        };
        let (completion, turn, ()) = tokio::join!(completion, turn, close);
        assert!(matches!(completion, CompletionWait::ShuttingDown(_)));
        // An unknown ticket settles as Unknown at once, shutdown or not.
        assert!(matches!(turn, TurnWait::Settled(TurnOutcome::Unknown)));
        Ok(())
    }

    /// A retire signals the waiters while its verb still holds the
    /// identity's lifecycle lock, and nothing signals when the verb lets go.
    /// A wait that reads in that window waits for the release, then answers
    /// Retiring, well inside its deadline.
    #[tokio::test]
    async fn a_wait_that_reads_inside_a_retire_answers_retiring_once_the_verb_lets_go()
    -> Result<(), Box<dyn std::error::Error>> {
        let bridge = Arc::new(super::super::core_test_bridge::CoreReadsBridge::default());
        let session_bridge: Arc<dyn SessionBridge> = bridge.clone();
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "completion-wait-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(session_bridge),
            default_timeout: None,
        }));
        let identity = AgentIdentity::parse("agent:retired-under-a-verb")?;
        let runtime_id = super::super::core_test_bridge::register_live(
            &runtime,
            &bridge.core,
            spec(&identity),
            1,
            SessionId::new(),
        )
        .await?;
        let baseline = runtime.completion_cursor(&identity).await;

        // A lifecycle verb holds the identity, as a retire does.
        let lifecycle_lock = runtime.lifecycle_lock_for(&identity).await;
        let held = lifecycle_lock.lock().await;
        let wait = tokio::spawn({
            let runtime = Arc::clone(&runtime);
            let identity = identity.clone();
            async move {
                runtime
                    .await_completion(&identity, Some(baseline), Duration::from_mins(10))
                    .await
            }
        });
        // The waiter reads the member live and parks.
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        // The retire: Core lets the member go, and the waiters are signalled
        // while the verb still holds the lock.
        bridge.core.retire(&runtime_id);
        runtime.entries.notify_binding_changed();
        for _ in 0..16 {
            tokio::task::yield_now().await;
        }
        assert!(!wait.is_finished(), "the wait holds while the verb runs");
        // The verb lets go and signals nothing.
        drop(held);
        let outcome = tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .map_err(|_| "the wait must answer once the verb lets go, not at its deadline")??;
        assert!(
            matches!(outcome, CompletionWait::Retiring(_)),
            "{outcome:?}"
        );
        Ok(())
    }
}

#[cfg(test)]
mod foreground_shutdown_tests {
    use super::*;
    use crate::identity_first::{ContinuityStoreError, LocalContinuityStore, LocalLeaseProvider};

    #[test]
    fn foreground_shutdown_value_is_retained_for_late_subscribers()
    -> Result<(), ContinuityStoreError> {
        let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "late-foreground-cancel-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });

        assert_eq!(runtime.foreground_cancel.receiver_count(), 0);
        runtime.close_foreground_operations();
        let receiver = runtime.foreground_cancel.subscribe();
        assert!(
            *receiver.borrow(),
            "a task subscribed after close must still observe shutdown"
        );
        Ok(())
    }

    /// A shutdown's in-flight drain closes foreground operations but leaves
    /// a turn's waiter waiting, so the drained turn's caller hears how it
    /// ended. The shutdown's own close then wakes the waiter.
    #[tokio::test]
    #[allow(clippy::expect_used)]
    async fn the_drain_close_leaves_a_turn_waiter_to_the_shutdown_close()
    -> Result<(), ContinuityStoreError> {
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "drain-turn-waiter-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        }));
        let identity = AgentIdentity::parse("agent:waiting").expect("identity");
        let ticket = runtime
            .turn_outcomes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .admit(&identity, None);
        let waiting = {
            let runtime = Arc::clone(&runtime);
            let identity = identity.clone();
            tokio::spawn(async move {
                runtime
                    .wait_for_turn_outcome(&identity, ticket, Duration::from_mins(1))
                    .await
            })
        };
        tokio::task::yield_now().await;

        runtime.close_foreground_operations_for_drain();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !waiting.is_finished(),
            "the drain's close leaves the turn's waiter waiting"
        );

        runtime.close_foreground_operations();
        let wait = tokio::time::timeout(Duration::from_secs(10), waiting)
            .await
            .expect("the shutdown's close wakes the waiter")
            .expect("waiter task");
        assert!(
            matches!(wait, TurnWait::ShuttingDown(TurnOutcome::Pending)),
            "{wait:?}"
        );
        Ok(())
    }
}

#[cfg(test)]
mod bootstrap_failure_attribution_tests {
    use super::*;
    use crate::identity_first::{ContinuityStoreError, LocalContinuityStore, LocalLeaseProvider};

    fn make_runtime() -> Result<IdentityRuntime, ContinuityStoreError> {
        Ok(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "bootstrap-failure-attribution-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        }))
    }

    /// Before embodiment became per identity, `restore_flow` failed the whole
    /// pass on one member's error and copied that cause onto every peer. The
    /// pass-failure mechanism remains for fleet-level roster, topology, and
    /// batch-store failures. This pins the three things that must hold there:
    /// terminality (or the wait barrier hangs), the pass cause in the
    /// pass-level slot, and no borrowed cause on a bystander.
    #[test]
    fn pass_failure_keeps_terminality_without_borrowing_one_members_cause()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = make_runtime()?;
        let culprit = AgentIdentity::parse("agent:culprit")?;
        let bystander = AgentIdentity::parse("agent:bystander")?;
        let survivor = AgentIdentity::parse("agent:survivor")?;
        let culprit_cause = "bridge create_session: culprit exploded";

        let generation =
            runtime.begin_identity_bootstrap_pending(IdentityBootstrapMode::EagerMaterialize);
        assert!(
            runtime.modify_bootstrap_status(Some(generation), |snapshot| {
                snapshot.identities.insert(
                    culprit.clone(),
                    IdentityBootstrapEntry {
                        state: IdentityBootstrapState::Broken,
                        error: Some(culprit_cause.to_string()),
                        restore: None,
                    },
                );
                snapshot.identities.insert(
                    bystander.clone(),
                    IdentityBootstrapEntry {
                        state: IdentityBootstrapState::Warming,
                        error: None,
                        restore: None,
                    },
                );
                snapshot.identities.insert(
                    survivor.clone(),
                    IdentityBootstrapEntry {
                        state: IdentityBootstrapState::Active,
                        error: None,
                        restore: None,
                    },
                );
                snapshot.refresh_aggregates();
            }),
            "seeding must land on the generation under test"
        );

        // Positive control: prove the pre-failure snapshot is NOT already in
        // the state the post-failure assertions claim. Without this, a seeding
        // bug that produced a Broken, cause-less bystander would let every
        // assertion below pass vacuously.
        let before = runtime.identity_bootstrap_status();
        assert!(before.error.is_none());
        assert!(!before.materialization_terminal());
        let before_bystander = before
            .identities
            .get(&bystander)
            .ok_or_else(|| "seeded bystander entry".to_string())?;
        assert_eq!(before_bystander.state, IdentityBootstrapState::Warming);
        assert!(before_bystander.error.is_none());

        // `IdentityRuntimeError`'s `Display` prefixes `Internal` ("internal:
        // {msg}"), and `fail_identity_bootstrap` stamps `error.to_string()`,
        // not the inner message. Pin the rendered form so the test tracks the
        // mechanism instead of one variant's wording.
        let pass_error = IdentityRuntimeError::Internal(culprit_cause.to_string());
        let pass_detail = pass_error.to_string();
        runtime.fail_identity_bootstrap(generation, &pass_error);

        let after = runtime.identity_bootstrap_status();
        // The pass cause rides the slot the type documents for it.
        assert_eq!(after.error.as_deref(), Some(pass_detail.as_str()));
        // Terminality survives: `wait_identity_bootstrap_terminal` returns a
        // truthful failure snapshot instead of waiting out its timeout.
        assert!(after.complete);
        assert!(after.materialization_terminal());
        assert!(!after.ready);

        // An identity that already reached Active is not retro-broken.
        let survivor_entry = after
            .identities
            .get(&survivor)
            .ok_or_else(|| "survivor entry".to_string())?;
        assert_eq!(survivor_entry.state, IdentityBootstrapState::Active);
        assert!(survivor_entry.error.is_none());

        // The identity that actually failed keeps ITS own cause verbatim.
        let culprit_entry = after
            .identities
            .get(&culprit)
            .ok_or_else(|| "culprit entry".to_string())?;
        assert_eq!(culprit_entry.state, IdentityBootstrapState::Broken);
        assert_eq!(culprit_entry.error.as_deref(), Some(culprit_cause));

        // The bystander transitions Warming -> Broken for terminality, but
        // must not report the culprit's cause as its own, and must not be
        // left cause-less either (17 Broken with no reason on record was the
        // original operator complaint).
        let bystander_entry = after
            .identities
            .get(&bystander)
            .ok_or_else(|| "bystander entry".to_string())?;
        assert_eq!(bystander_entry.state, IdentityBootstrapState::Broken);
        let bystander_error = bystander_entry
            .error
            .as_deref()
            .ok_or_else(|| "a Broken entry must carry a reason".to_string())?;
        assert_ne!(
            bystander_error, culprit_cause,
            "the culprit's raw cause must never be reported as the bystander's own"
        );
        // The discriminating assertion: the rendered pass detail is EXACTLY
        // what the previous implementation stamped on every non-Active entry,
        // so this is the one that fails if the borrowed-cause behaviour comes
        // back. (`assert_ne!` against `culprit_cause` alone would pass under
        // the old code too, because `Display` prefixes it.)
        assert_ne!(
            bystander_error, pass_detail,
            "a bystander must never carry the raw pass detail as its own cause"
        );
        assert!(
            bystander_error.starts_with("identity bootstrap pass failed before this identity"),
            "an unattributed entry must be labelled pass-level: {bystander_error}"
        );
        Ok(())
    }

    /// The `entry.error.is_none()` guard above is only truthful because a
    /// pass RESETS per-entry causes when it opens. Two failure paths stamp
    /// entries that `begin_identity_bootstrap` has not yet republished -
    /// `bootstrap_roster`'s prepare failure and `refresh_desired_topology`'s
    /// roster-provider failure - so without the reset a pass would report the
    /// PREVIOUS pass's cause per identity beside its own `status.error`.
    #[test]
    fn a_new_pass_does_not_report_the_previous_passs_cause()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = make_runtime()?;
        let identity = AgentIdentity::parse("agent:stale")?;
        let first_cause = "pass one: resume rejected";

        let first =
            runtime.begin_identity_bootstrap_pending(IdentityBootstrapMode::EagerMaterialize);
        assert!(
            runtime.modify_bootstrap_status(Some(first), |snapshot| {
                snapshot.identities.insert(
                    identity.clone(),
                    IdentityBootstrapEntry {
                        state: IdentityBootstrapState::Broken,
                        error: Some(first_cause.to_string()),
                        restore: None,
                    },
                );
                snapshot.refresh_aggregates();
            }),
            "seeding must land on the first generation"
        );

        // Positive control: the stale cause really IS on the entry before the
        // next pass opens, so the assertions below observe a transition rather
        // than a state that was never there.
        let stale = runtime.identity_bootstrap_status();
        let stale_entry = stale
            .identities
            .get(&identity)
            .ok_or_else(|| "seeded pass-one entry".to_string())?;
        assert_eq!(stale_entry.error.as_deref(), Some(first_cause));

        // Pass two fails before `begin_identity_bootstrap` republishes entries.
        let second =
            runtime.begin_identity_bootstrap_pending(IdentityBootstrapMode::EagerMaterialize);
        let cleared = runtime.identity_bootstrap_status();
        let cleared_entry = cleared
            .identities
            .get(&identity)
            .ok_or_else(|| "entry survives the pass boundary".to_string())?;
        assert!(
            cleared_entry.error.is_none(),
            "opening a pass must drop the previous pass's cause"
        );
        assert_eq!(
            cleared_entry.state,
            IdentityBootstrapState::Broken,
            "only causes are pass-scoped; states are not touched here"
        );

        let pass_error =
            IdentityRuntimeError::Internal("pass two: roster provider down".to_string());
        let pass_detail = pass_error.to_string();
        runtime.fail_identity_bootstrap(second, &pass_error);

        let after = runtime.identity_bootstrap_status();
        assert_eq!(after.error.as_deref(), Some(pass_detail.as_str()));
        let entry_error = after
            .identities
            .get(&identity)
            .and_then(|entry| entry.error.as_deref())
            .ok_or_else(|| "a Broken entry must carry a reason".to_string())?;
        assert!(
            entry_error.contains("pass two: roster provider down"),
            "an entry stamped by pass two must name pass two: {entry_error}"
        );
        assert!(
            !entry_error.contains("pass one"),
            "pass one's cause must not survive into pass two: {entry_error}"
        );
        Ok(())
    }
}

/// See [`IdentityRuntime::effective_runtime_mode`]. Free so it can be pinned
/// without a runtime: the resolution order is the contract.
pub(crate) fn effective_runtime_mode(
    definition: Option<&meerkat_mob::MobDefinition>,
    spec: &DurableAgentSpec,
) -> meerkat_mob::MobRuntimeMode {
    if let Some(mode) = spec.runtime_mode_override {
        return mode;
    }
    definition
        .and_then(|definition| definition.profiles.get(&spec.profile))
        .and_then(|binding| binding.as_inline())
        .map(|profile| profile.runtime_mode)
        .unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod effective_runtime_mode_tests {
    use super::*;

    fn spec(profile: &str, override_mode: Option<meerkat_mob::MobRuntimeMode>) -> DurableAgentSpec {
        DurableAgentSpec {
            identity: AgentIdentity::parse("identity:probe").expect("valid identity"),
            profile: meerkat_mob::ProfileName::from(profile),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: Default::default(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: override_mode,
            backend: None,
            binding: None,
            placement: None,
        }
    }

    fn definition() -> meerkat_mob::MobDefinition {
        meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "mode-probe"

[profiles.driven]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.omitted]
model = "gpt-5.5"
"#,
        )
        .expect("definition parses")
    }

    /// The contract, in meerkat's order: override, else profile, else default.
    /// The first cut of the autonomous-host skip read only the override and
    /// skipped memory on every turn_driven profile whose roster set nothing.
    #[test]
    fn profile_runtime_mode_is_honoured_when_the_roster_sets_no_override() {
        let definition = definition();
        assert_eq!(
            effective_runtime_mode(Some(&definition), &spec("driven", None)),
            meerkat_mob::MobRuntimeMode::TurnDriven,
            "a turn_driven profile with no roster override must resolve turn_driven"
        );
        assert_eq!(
            effective_runtime_mode(Some(&definition), &spec("omitted", None)),
            meerkat_mob::MobRuntimeMode::AutonomousHost,
            "a profile that omits runtime_mode resolves to meerkat's default"
        );
    }

    #[test]
    fn roster_override_wins_over_the_profile() {
        let definition = definition();
        assert_eq!(
            effective_runtime_mode(
                Some(&definition),
                &spec("driven", Some(meerkat_mob::MobRuntimeMode::AutonomousHost)),
            ),
            meerkat_mob::MobRuntimeMode::AutonomousHost
        );
        assert_eq!(
            effective_runtime_mode(
                Some(&definition),
                &spec("omitted", Some(meerkat_mob::MobRuntimeMode::TurnDriven)),
            ),
            meerkat_mob::MobRuntimeMode::TurnDriven
        );
    }

    #[test]
    fn unknown_profile_or_missing_definition_resolves_to_the_default() {
        let definition = definition();
        assert_eq!(
            effective_runtime_mode(Some(&definition), &spec("nope", None)),
            meerkat_mob::MobRuntimeMode::default()
        );
        assert_eq!(
            effective_runtime_mode(None, &spec("driven", None)),
            meerkat_mob::MobRuntimeMode::default(),
            "with no definition installed Meta has no profile table; default, documented"
        );
    }
}

/// Admission-first delivery and plan / wire / revalidate topology reconcile:
/// no send, steer or dispatch waits behind managed-topology wiring or a peer
/// build, and a binding change during wiring replans instead of committing a
/// stale plan.
#[cfg(test)]
mod admission_first_topology_tests {
    use super::*;
    use std::sync::Mutex as StdTestMutex;
    use tokio::sync::Notify;

    use super::super::bridge::{BridgeDelivery, BridgeError, ResumeSessionOutcome};
    use super::super::local_lease::LocalLeaseProvider;
    use super::super::local_store::LocalContinuityStore;
    use super::super::types::{AgentBuildDraft, SessionSnapshot};

    const ADMISSION_WAIT: Duration = Duration::from_secs(5);

    /// A session bridge whose managed-topology calls and member builds can
    /// be wedged behind `gate` (closed while false), and which records every
    /// admission and wire call it sees.
    struct WedgeableTopologyBridge {
        gate: watch::Receiver<bool>,
        wedge_inspection: bool,
        /// Park only the FIRST wire call (0 = never park).
        wedge_first_wire: bool,
        wedge_builds: bool,
        /// Park EVERY wire call until the test releases it (one
        /// `wire_release` permit per call).
        wedge_every_wire: bool,
        sessions: StdTestMutex<HashMap<String, SessionId>>,
        admissions: StdTestMutex<Vec<(String, HandlingMode)>>,
        wire_calls: StdTestMutex<Vec<Vec<(AgentRuntimeId, AgentRuntimeId)>>>,
        unwire_calls: StdTestMutex<Vec<(AgentRuntimeId, AgentRuntimeId)>>,
        /// The physical wires the inspection reports (a late-landing wire is
        /// simulated by adding to it).
        live_wires: StdTestMutex<Vec<(AgentRuntimeId, AgentRuntimeId)>>,
        /// Wire and admission calls in the order the bridge saw them.
        events: StdTestMutex<Vec<String>>,
        inspection_entered: Notify,
        wire_entered: Notify,
        wire_release: Notify,
        build_entered: Notify,
        /// The members Core holds.
        core: crate::identity_first::core_test_members::CoreTestMembers,
    }

    impl WedgeableTopologyBridge {
        fn new(gate: watch::Receiver<bool>) -> Self {
            Self {
                gate,
                wedge_inspection: false,
                wedge_first_wire: false,
                wedge_builds: false,
                wedge_every_wire: false,
                sessions: StdTestMutex::new(HashMap::new()),
                admissions: StdTestMutex::new(Vec::new()),
                wire_calls: StdTestMutex::new(Vec::new()),
                unwire_calls: StdTestMutex::new(Vec::new()),
                live_wires: StdTestMutex::new(Vec::new()),
                events: StdTestMutex::new(Vec::new()),
                inspection_entered: Notify::new(),
                wire_entered: Notify::new(),
                wire_release: Notify::new(),
                build_entered: Notify::new(),
                core: crate::identity_first::core_test_members::CoreTestMembers::default(),
            }
        }

        fn log(&self, event: String) {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event);
        }

        fn events(&self) -> Vec<String> {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn unwire_calls(&self) -> Vec<(AgentRuntimeId, AgentRuntimeId)> {
            self.unwire_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn land_wire(&self, a: &AgentRuntimeId, b: &AgentRuntimeId) {
            let mut live = self
                .live_wires
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            live.push((a.clone(), b.clone()));
            live.push((b.clone(), a.clone()));
        }

        fn bind_session(&self, runtime_id: &AgentRuntimeId, session_id: &SessionId) {
            self.sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(runtime_id.as_str().to_string(), session_id.clone());
        }

        fn admissions(&self) -> Vec<(String, HandlingMode)> {
            self.admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn wire_calls(&self) -> Vec<Vec<(AgentRuntimeId, AgentRuntimeId)>> {
            self.wire_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        async fn park_until_open(&self) {
            let mut gate = self.gate.clone();
            while !*gate.borrow_and_update() {
                if gate.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        }
    }

    #[async_trait::async_trait]
    impl SessionBridge for WedgeableTopologyBridge {
        async fn create_session(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<crate::identity_first::MemberRuntimeBinding, BridgeError> {
            let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            let runtime_id = &agent_runtime_id;
            let session_id = &meerkat_core::types::SessionId::new();
            self.build_entered.notify_one();
            if self.wedge_builds {
                // A member build parked in the host's build callback.
                self.park_until_open().await;
            }
            self.bind_session(runtime_id, session_id);
            let binding = crate::identity_first::MemberRuntimeBinding {
                agent_runtime_id,
                session_id: Some(session_id.clone()),
            };
            self.core.bind(identity, &binding, &spec.profile);
            Ok(binding)
        }

        async fn resume_session(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            session_id: &SessionId,
            _resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            self.build_entered.notify_one();
            if self.wedge_builds {
                self.park_until_open().await;
            }
            self.bind_session(&agent_runtime_id, session_id);
            self.core.bind(
                identity,
                &crate::identity_first::MemberRuntimeBinding {
                    agent_runtime_id: agent_runtime_id.clone(),
                    session_id: Some(session_id.clone()),
                },
                &spec.profile,
            );
            Ok(ResumeSessionOutcome::Resumed {
                session_id: session_id.clone(),
                agent_runtime_id,
            })
        }

        async fn deliver_admitted(
            &self,
            runtime_id: &AgentRuntimeId,
            delivery: BridgeDelivery,
        ) -> Result<SessionId, BridgeError> {
            self.admissions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((runtime_id.as_str().to_string(), delivery.handling_mode));
            self.log(format!("admit:{runtime_id}"));
            self.sessions
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(runtime_id.as_str())
                .cloned()
                .ok_or_else(|| BridgeError::Mob(format!("no session bound to {runtime_id}")))
        }

        async fn wire_peer(
            &self,
            a: &AgentRuntimeId,
            b: &AgentRuntimeId,
        ) -> Result<(), BridgeError> {
            let first = {
                let mut calls = self
                    .wire_calls
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                calls.push(vec![(a.clone(), b.clone())]);
                calls.len() == 1
            };
            self.log(format!("wire:{a}:{b}"));
            self.wire_entered.notify_one();
            if first && self.wedge_first_wire {
                self.park_until_open().await;
            }
            if self.wedge_every_wire {
                self.wire_release.notified().await;
            }
            Ok(())
        }

        async fn current_member_wires(
            &self,
        ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
            self.inspection_entered.notify_one();
            if self.wedge_inspection {
                self.park_until_open().await;
            }
            Ok(self
                .live_wires
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone())
        }

        async fn unwire_peer(
            &self,
            a: &AgentRuntimeId,
            b: &AgentRuntimeId,
        ) -> Result<(), BridgeError> {
            self.unwire_calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((a.clone(), b.clone()));
            self.live_wires
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(x, y)| !((x == a && y == b) || (x == b && y == a)));
            Ok(())
        }

        async fn checkpoint_session(
            &self,
            _runtime_id: &AgentRuntimeId,
            _session_id: &SessionId,
        ) -> Result<SessionSnapshot, BridgeError> {
            Err(BridgeError::Mob(
                "checkpoint not used in this test".to_string(),
            ))
        }

        async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            self.core.retire(runtime_id);
            Ok(())
        }

        async fn live_member(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn live_member_scope(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
            self.core.live_member_scope(identity)
        }

        async fn ensure_member_live(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn last_member_binding(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
            self.core.last_member_binding(identity)
        }

        async fn member_binding_history(
            &self,
        ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
            self.core.history()
        }

        async fn identity_management(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
            Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
        }
    }

    fn spec(identity: &AgentIdentity) -> DurableAgentSpec {
        DurableAgentSpec {
            identity: identity.clone(),
            profile: meerkat_mob::ProfileName::from("default"),
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
        }
    }

    fn runtime_with(
        bridge: Arc<WedgeableTopologyBridge>,
        name: &str,
    ) -> Result<Arc<IdentityRuntime>, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: name.to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        })))
    }

    /// Register `identity` Active on runtime generation `generation`, bound
    /// to a fresh session the bridge delivers to.
    async fn register_active(
        runtime: &IdentityRuntime,
        bridge: &WedgeableTopologyBridge,
        identity: &AgentIdentity,
        generation: u64,
    ) -> Result<AgentRuntimeId, Box<dyn std::error::Error + Send + Sync>> {
        register_active_as(runtime, bridge, spec(identity), generation).await
    }

    /// [`register_active`] with an explicit spec (for example a profile):
    /// Core holds the identity's live member at `generation`.
    async fn register_active_as(
        runtime: &IdentityRuntime,
        bridge: &WedgeableTopologyBridge,
        spec: DurableAgentSpec,
        generation: u64,
    ) -> Result<AgentRuntimeId, Box<dyn std::error::Error + Send + Sync>> {
        let identity = spec.identity.clone();
        let runtime_id = AgentRuntimeId::parse(&format!("rt:{}:{generation}", identity.as_str()))?;
        let session_id = SessionId::new();
        bridge.bind_session(&runtime_id, &session_id);
        bridge.core.bind(
            &identity,
            &crate::identity_first::MemberRuntimeBinding {
                agent_runtime_id: runtime_id.clone(),
                session_id: Some(session_id),
            },
            &spec.profile,
        );
        runtime.register(spec).await;
        Ok(runtime_id)
    }

    /// A candidate launch rewires no member: its managed peer-edge reconcile
    /// asks the bridge to wire nothing and records no managed edge, and its
    /// census names the edge an authoritative reconcile would wire. The same
    /// reconcile on an authoritative runtime wires it.
    #[tokio::test]
    async fn a_candidate_launch_reconciles_no_managed_peer_edge()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for candidate in [true, false] {
            let alpha = AgentIdentity::parse("agent:census-alpha")?;
            let beta = AgentIdentity::parse("agent:census-beta")?;
            let (_open_gate, gate) = watch::channel(true);
            let bridge = Arc::new(WedgeableTopologyBridge::new(gate));
            let runtime = runtime_with(bridge.clone(), "candidate-peer-edges")?;
            if candidate {
                runtime.mark_candidate_launch();
            }
            register_active(&runtime, &bridge, &alpha, 0).await?;
            register_active(&runtime, &bridge, &beta, 0).await?;
            let edge = ManagedPeerEdge::new(alpha.clone(), beta.clone())?;
            let key = (edge.a().clone(), edge.b().clone());
            let census = runtime
                .census_managed_peer_edges(&BTreeSet::from([key.clone()]), None)
                .await;
            assert_eq!(
                census.would_wire,
                BTreeSet::from([key.clone()]),
                "{census:?}"
            );

            runtime
                .reconcile_managed_peer_edges(std::slice::from_ref(&edge))
                .await?;
            if candidate {
                assert!(
                    bridge.wire_calls().is_empty(),
                    "a candidate wires nothing: {:?}",
                    bridge.wire_calls()
                );
                assert!(runtime.managed_peer_edges_snapshot().await.is_empty());
            } else {
                assert!(
                    !bridge.wire_calls().is_empty(),
                    "an authoritative reconcile wires the edge"
                );
                assert!(runtime.managed_peer_edges_snapshot().await.contains(&key));
            }
        }
        Ok(())
    }

    /// A topology reconcile wedged inside a wiring call (a mob-actor round
    /// trip that cannot settle, e.g. behind a member mid-turn) holds no
    /// lifecycle lock, so a queue send, a dispatch and a steer to either
    /// endpoint are all admitted while it is parked. Before the fix the
    /// queue send and dispatch ran that same reconcile before admission (and
    /// waited on its lock), and the steer waited on its endpoint lifecycle
    /// lock, which the reconcile held for the whole wiring.
    #[tokio::test]
    async fn queue_dispatch_and_steer_are_admitted_while_topology_wiring_is_wedged()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let alpha = AgentIdentity::parse("agent:wedge-alpha")?;
        let beta = AgentIdentity::parse("agent:wedge-beta")?;
        let (open_gate, gate) = watch::channel(false);
        let mut bridge = WedgeableTopologyBridge::new(gate);
        bridge.wedge_inspection = true;
        let bridge = Arc::new(bridge);
        let runtime = runtime_with(bridge.clone(), "admission-first-wedged-wiring")?;
        let alpha_rt = register_active(&runtime, &bridge, &alpha, 0).await?;
        let beta_rt = register_active(&runtime, &bridge, &beta, 0).await?;
        let edges = vec![ManagedPeerEdge::new(alpha.clone(), beta.clone())?];
        runtime.set_desired_peer_edges(edges.clone()).await;

        let reconcile = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.reconcile_managed_peer_edges(&edges).await }
        });
        tokio::time::timeout(ADMISSION_WAIT, bridge.inspection_entered.notified())
            .await
            .map_err(|_| "the reconcile should reach its wiring inspection")?;

        tokio::time::timeout(
            ADMISSION_WAIT,
            runtime.send_with_mode_tracked(
                &alpha,
                &meerkat_core::ContentInput::Text("queued".to_string()),
                HandlingMode::Queue,
            ),
        )
        .await
        .map_err(|_| "a queue send must not wait behind wedged topology wiring")??;
        tokio::time::timeout(
            ADMISSION_WAIT,
            runtime.dispatch_tracked(&beta, &DispatchInput::system("dispatched")),
        )
        .await
        .map_err(|_| "a dispatch must not wait behind wedged topology wiring")??;
        tokio::time::timeout(
            ADMISSION_WAIT,
            runtime.send_with_mode_tracked(
                &beta,
                &meerkat_core::ContentInput::Text("steered".to_string()),
                HandlingMode::Steer,
            ),
        )
        .await
        .map_err(|_| "a steer must not wait behind wedged topology wiring")??;

        let admissions = bridge.admissions();
        assert!(
            admissions.contains(&(alpha_rt.as_str().to_string(), HandlingMode::Queue)),
            "queue send admitted: {admissions:?}"
        );
        assert!(
            admissions.contains(&(beta_rt.as_str().to_string(), HandlingMode::Steer)),
            "steer admitted: {admissions:?}"
        );
        assert_eq!(
            admissions
                .iter()
                .filter(|(runtime_id, _)| runtime_id == beta_rt.as_str())
                .count(),
            2,
            "dispatch and steer each admitted exactly once: {admissions:?}"
        );
        assert!(
            !reconcile.is_finished(),
            "the reconcile is still parked inside its wiring call"
        );

        open_gate.send_replace(true);
        tokio::time::timeout(ADMISSION_WAIT, reconcile)
            .await
            .map_err(|_| "the reconcile settles once its wiring does")???;
        assert!(
            runtime
                .managed_peer_edges_snapshot()
                .await
                .contains(&(alpha.clone(), beta.clone())),
            "the released reconcile commits the edge"
        );
        Ok(())
    }

    /// A peer whose member build is wedged (the host's build callback never
    /// answers) does not hold up a queue send or a steer to its neighbour:
    /// the neighbour is admitted first and the peer hydration that reaches
    /// the wedged build runs after admission.
    #[tokio::test]
    async fn admission_does_not_wait_behind_a_wedged_peer_build()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let target = AgentIdentity::parse("agent:build-target")?;
        let peer = AgentIdentity::parse("agent:build-peer")?;
        let (_open_gate, gate) = watch::channel(false);
        let mut bridge = WedgeableTopologyBridge::new(gate);
        bridge.wedge_builds = true;
        let bridge = Arc::new(bridge);
        let runtime = runtime_with(bridge.clone(), "admission-first-wedged-build")?;
        let target_rt = register_active(&runtime, &bridge, &target, 0).await?;
        runtime.register(spec(&peer)).await;
        runtime
            .set_desired_peer_edges(vec![ManagedPeerEdge::new(target.clone(), peer.clone())?])
            .await;

        tokio::time::timeout(
            ADMISSION_WAIT,
            runtime.send_with_mode_tracked(
                &target,
                &meerkat_core::ContentInput::Text("queued".to_string()),
                HandlingMode::Queue,
            ),
        )
        .await
        .map_err(|_| "a queue send must not wait behind a wedged peer build")??;
        tokio::time::timeout(ADMISSION_WAIT, bridge.build_entered.notified())
            .await
            .map_err(|_| "the post-admission hydration should reach the peer build")?;
        tokio::time::timeout(
            ADMISSION_WAIT,
            runtime.send_with_mode_tracked(
                &target,
                &meerkat_core::ContentInput::Text("steered".to_string()),
                HandlingMode::Steer,
            ),
        )
        .await
        .map_err(|_| "a steer must not wait behind a wedged peer build")??;

        assert_eq!(
            bridge.admissions(),
            vec![
                (target_rt.as_str().to_string(), HandlingMode::Queue),
                (target_rt.as_str().to_string(), HandlingMode::Steer),
            ]
        );
        Ok(())
    }

    /// An endpoint respawned (new runtime binding) while the reconcile's wire
    /// call is parked makes the plan stale: nothing is committed for the old
    /// incarnation, the reconcile replans from the live bindings and wires
    /// the new incarnation, then commits.
    #[tokio::test]
    async fn topology_reconcile_replans_when_an_endpoint_binding_changes_during_wiring()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let alpha = AgentIdentity::parse("agent:replan-alpha")?;
        let beta = AgentIdentity::parse("agent:replan-beta")?;
        let (open_gate, gate) = watch::channel(false);
        let mut bridge = WedgeableTopologyBridge::new(gate);
        bridge.wedge_first_wire = true;
        let bridge = Arc::new(bridge);
        let runtime = runtime_with(bridge.clone(), "topology-replan")?;
        let alpha_rt = register_active(&runtime, &bridge, &alpha, 0).await?;
        let beta_rt_old = register_active(&runtime, &bridge, &beta, 0).await?;
        let edges = vec![ManagedPeerEdge::new(alpha.clone(), beta.clone())?];
        runtime.set_desired_peer_edges(edges.clone()).await;

        let reconcile = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.reconcile_managed_peer_edges(&edges).await }
        });
        tokio::time::timeout(ADMISSION_WAIT, bridge.wire_entered.notified())
            .await
            .map_err(|_| "the reconcile should reach its first wire call")?;

        // beta is respawned onto a new incarnation while the wire is parked.
        let beta_rt_new = register_active(&runtime, &bridge, &beta, 1).await?;
        assert!(
            runtime.managed_peer_edges_snapshot().await.is_empty(),
            "nothing is committed while the wiring is in flight"
        );
        open_gate.send_replace(true);
        tokio::time::timeout(ADMISSION_WAIT, reconcile)
            .await
            .map_err(|_| "the reconcile settles after replanning")???;

        let calls = bridge.wire_calls();
        let wired_pair = |call: &Vec<(AgentRuntimeId, AgentRuntimeId)>,
                          beta_rt: &AgentRuntimeId| {
            call.iter()
                .any(|(a, b)| (a == &alpha_rt && b == beta_rt) || (a == beta_rt && b == &alpha_rt))
        };
        assert_eq!(
            calls.len(),
            2,
            "one stale wire, one replanned wire: {calls:?}"
        );
        assert!(
            wired_pair(&calls[0], &beta_rt_old),
            "first plan wired the old incarnation"
        );
        assert!(
            wired_pair(&calls[1], &beta_rt_new),
            "the replan wired the live incarnation: {calls:?}"
        );
        assert!(
            runtime
                .managed_peer_edges_snapshot()
                .await
                .contains(&(alpha.clone(), beta.clone())),
            "the replanned edge is committed"
        );
        Ok(())
    }

    /// Waking a Dormant member reconciles ITS managed edges before the input
    /// is admitted, so the woken member's first turn can message its peer:
    /// the bridge sees the wire before the admission.
    #[tokio::test]
    async fn waking_a_dormant_member_wires_its_peers_before_admitting_its_first_turn()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let sleeper = AgentIdentity::parse("agent:wake-sleeper")?;
        let peer = AgentIdentity::parse("agent:wake-peer")?;
        let (_open_gate, gate) = watch::channel(false);
        let bridge = Arc::new(WedgeableTopologyBridge::new(gate));
        let runtime = runtime_with(bridge.clone(), "wake-wires-before-admission")?;
        let peer_rt = register_active(&runtime, &bridge, &peer, 0).await?;
        runtime.register(spec(&sleeper)).await;
        runtime
            .set_desired_peer_edges(vec![ManagedPeerEdge::new(sleeper.clone(), peer.clone())?])
            .await;

        tokio::time::timeout(
            ADMISSION_WAIT,
            runtime.send_with_mode_tracked(
                &sleeper,
                &meerkat_core::ContentInput::Text("ask agent:wake-peer for the plan".to_string()),
                HandlingMode::Queue,
            ),
        )
        .await
        .map_err(|_| "the wake send should be admitted")??;

        let sleeper_rt = runtime
            .status(&sleeper)
            .await?
            .agent_runtime_id
            .ok_or("the woken member has a runtime binding")?;
        let events = bridge.events();
        let wired = events.iter().position(|event| {
            event == &format!("wire:{sleeper_rt}:{peer_rt}")
                || event == &format!("wire:{peer_rt}:{sleeper_rt}")
        });
        let admitted = events
            .iter()
            .position(|event| event == &format!("admit:{sleeper_rt}"));
        match (wired, admitted) {
            (Some(wired), Some(admitted)) => assert!(
                wired < admitted,
                "the woken member's edge must exist before its first turn is admitted: {events:?}"
            ),
            _ => return Err(format!("expected a wire and an admission: {events:?}").into()),
        }
        let edge = ManagedPeerEdge::new(sleeper.clone(), peer.clone())?;
        assert!(
            runtime
                .managed_peer_edges_snapshot()
                .await
                .contains(&(edge.a().clone(), edge.b().clone())),
            "the woken member's edge is committed"
        );
        Ok(())
    }

    /// A wiring call that never answers settles as a typed
    /// `PeerWiringTimedOut` after its budget instead of parking the
    /// reconcile.
    #[tokio::test(start_paused = true)]
    async fn a_wedged_wiring_inspection_settles_as_peer_wiring_timed_out()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let alpha = AgentIdentity::parse("agent:timeout-alpha")?;
        let beta = AgentIdentity::parse("agent:timeout-beta")?;
        let (_open_gate, gate) = watch::channel(false);
        let mut bridge = WedgeableTopologyBridge::new(gate);
        bridge.wedge_inspection = true;
        let bridge = Arc::new(bridge);
        let runtime = runtime_with(bridge.clone(), "wiring-timeout")?;
        register_active(&runtime, &bridge, &alpha, 0).await?;
        register_active(&runtime, &bridge, &beta, 0).await?;
        let edges = vec![ManagedPeerEdge::new(alpha, beta)?];

        match runtime.reconcile_managed_peer_edges(&edges).await {
            Err(IdentityRuntimeError::PeerWiringTimedOut { operation, waited }) => {
                assert_eq!(operation, "current_member_wires");
                assert_eq!(waited, PEER_WIRING_CALL_BUDGET);
            }
            other => return Err(format!("expected PeerWiringTimedOut, got {other:?}").into()),
        }
        assert!(runtime.managed_peer_edges_snapshot().await.is_empty());
        Ok(())
    }

    /// A wire whose reply timed out may still land in the actor. Its edge is
    /// recorded as unsettled; once the desired topology no longer wants it,
    /// the next reconcile sends an ordered, tolerant unwire for it (whatever
    /// inspection says) and settles it, instead of leaking an unmanaged wire.
    #[tokio::test(start_paused = true)]
    async fn a_timed_out_wire_that_lands_late_is_unwired_once_no_longer_desired()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let alpha = AgentIdentity::parse("agent:late-alpha")?;
        let beta = AgentIdentity::parse("agent:late-beta")?;
        let (_open_gate, gate) = watch::channel(false);
        let mut bridge = WedgeableTopologyBridge::new(gate);
        bridge.wedge_first_wire = true;
        let bridge = Arc::new(bridge);
        let runtime = runtime_with(bridge.clone(), "late-landing-wire")?;
        let alpha_rt = register_active(&runtime, &bridge, &alpha, 0).await?;
        let beta_rt = register_active(&runtime, &bridge, &beta, 0).await?;
        let edge = ManagedPeerEdge::new(alpha.clone(), beta.clone())?;
        let key = (edge.a().clone(), edge.b().clone());

        match runtime
            .reconcile_managed_peer_edges(std::slice::from_ref(&edge))
            .await
        {
            Err(IdentityRuntimeError::PeerWiringTimedOut { operation, .. }) => {
                assert_eq!(operation, "wire_peers_batch");
            }
            other => return Err(format!("expected a wire timeout, got {other:?}").into()),
        }
        assert!(
            runtime.managed_peer_edges_snapshot().await.is_empty(),
            "a timed-out wire is not committed as managed"
        );
        assert!(
            runtime
                .unsettled_peer_wiring_snapshot()
                .await
                .contains(&key),
            "the timed-out edge's fate is recorded as unknown"
        );

        // The dropped wire command lands in the actor after all.
        bridge.land_wire(&alpha_rt, &beta_rt);
        // The desired topology no longer wants the edge.
        runtime.set_desired_peer_edges(Vec::new()).await;
        runtime.reconcile_managed_peer_edges(&[]).await?;

        let unwired = bridge.unwire_calls();
        assert!(
            unwired.iter().any(
                |(a, b)| (a == &alpha_rt && b == &beta_rt) || (a == &beta_rt && b == &alpha_rt)
            ),
            "the late-landed wire is unwired: {unwired:?}"
        );
        assert!(runtime.unsettled_peer_wiring_snapshot().await.is_empty());
        assert!(runtime.managed_peer_edges_snapshot().await.is_empty());
        Ok(())
    }

    /// Topology rule: member agents must not keep reaching the lead's agent
    /// once the topology separates them. `managed_peer_edges` is this
    /// process's memory and starts empty, while the mob's wiring is durable
    /// and replays on recovery. An edge between two of this runtime's
    /// identities that the topology no longer wants must be unwired after a
    /// restart, even though this process never wired it. An edge to a member
    /// that is not one of this runtime's identities is left alone.
    #[tokio::test]
    async fn an_edge_no_longer_desired_is_unwired_after_a_restart()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let lead = AgentIdentity::parse("identity:lead-1")?;
        let member = AgentIdentity::parse("identity:member-1")?;
        let peer_member = AgentIdentity::parse("identity:member-2")?;
        let (_open_gate, gate) = watch::channel(false);
        let bridge = Arc::new(WedgeableTopologyBridge::new(gate));
        // The process after the restart: nothing in its managed set. A
        // topology provider owns the topology.
        let runtime = runtime_with(bridge.clone(), "edge-prune-after-restart")?;
        runtime.set_identity_edge_ownership(IdentityEdgeOwnership::TopologyProvider);
        let lead_rt = register_active(&runtime, &bridge, &lead, 0).await?;
        let member_rt = register_active(&runtime, &bridge, &member, 0).await?;
        let peer_member_rt = register_active(&runtime, &bridge, &peer_member, 0).await?;
        let delegate_rt = AgentRuntimeId::parse("rt:delegate:0")?;
        // The previous process wired these; the mob replayed them.
        bridge.land_wire(&lead_rt, &member_rt);
        bridge.land_wire(&member_rt, &peer_member_rt);
        bridge.land_wire(&member_rt, &delegate_rt);
        assert!(runtime.managed_peer_edges_snapshot().await.is_empty());

        // The topology now keeps the members together and away from the
        // lead.
        let kept = ManagedPeerEdge::new(member.clone(), peer_member.clone())?;
        runtime
            .reconcile_managed_peer_edges(std::slice::from_ref(&kept))
            .await?;

        let unwired = bridge.unwire_calls();
        let unwired_pair = |x: &AgentRuntimeId, y: &AgentRuntimeId| {
            unwired
                .iter()
                .any(|(a, b)| (a == x && b == y) || (a == y && b == x))
        };
        assert!(
            unwired_pair(&lead_rt, &member_rt),
            "the edge the topology no longer wants is unwired: {unwired:?}"
        );
        assert!(
            !unwired_pair(&member_rt, &peer_member_rt),
            "the desired edge is kept: {unwired:?}"
        );
        assert!(
            !unwired_pair(&member_rt, &delegate_rt),
            "an edge to a member that is not one of this runtime's identities is left alone: \
             {unwired:?}"
        );
        let live = runtime.logical_peer_edges().await?;
        let stale = ManagedPeerEdge::new(lead.clone(), member.clone())?;
        assert!(
            live.contains(&kept),
            "the desired edge stays wired: {live:?}"
        );
        assert!(!live.contains(&stale), "the stale edge is gone: {live:?}");
        Ok(())
    }

    /// With a topology provider, a live identity edge the mob definition's
    /// wiring declares (here `role_wiring` worker-helper) is the definition's,
    /// so the topology never prunes it, even though its desired set does not
    /// contain it. An edge that no declared owner wants is pruned.
    #[tokio::test]
    async fn a_definition_declared_edge_is_not_the_topologys_to_prune()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let lead = AgentIdentity::parse("identity:lead")?;
        let worker = AgentIdentity::parse("identity:worker")?;
        let helper = AgentIdentity::parse("identity:helper")?;
        let as_profile = |identity: &AgentIdentity, profile: &str| {
            let mut spec = spec(identity);
            spec.profile = meerkat_mob::ProfileName::from(profile);
            spec
        };
        let (_open_gate, gate) = watch::channel(false);
        let bridge = Arc::new(WedgeableTopologyBridge::new(gate));
        let runtime = runtime_with(bridge.clone(), "edge-prune-definition-owned")?;
        runtime.set_identity_edge_ownership(IdentityEdgeOwnership::TopologyProvider);
        let definition = meerkat_mob::MobDefinition::from_toml(
            r#"
[mob]
id = "edge-prune-definition-owned"

[[wiring.role_wiring]]
a = "worker"
b = "helper"

[profiles.lead]
model = "gpt-5.5"

[profiles.worker]
model = "gpt-5.5"

[profiles.helper]
model = "gpt-5.5"
"#,
        )?;
        runtime.set_reset_roster_provider_context(
            Some(Arc::new(super::super::MutableRosterProvider::new(
                Vec::new(),
            ))),
            Some(definition),
        );
        let lead_rt = register_active_as(&runtime, &bridge, as_profile(&lead, "lead"), 0).await?;
        let worker_rt =
            register_active_as(&runtime, &bridge, as_profile(&worker, "worker"), 0).await?;
        let helper_rt =
            register_active_as(&runtime, &bridge, as_profile(&helper, "helper"), 0).await?;
        bridge.land_wire(&lead_rt, &worker_rt);
        bridge.land_wire(&worker_rt, &helper_rt);

        runtime.reconcile_managed_peer_edges(&[]).await?;

        let unwired = bridge.unwire_calls();
        let unwired_pair = |x: &AgentRuntimeId, y: &AgentRuntimeId| {
            unwired
                .iter()
                .any(|(a, b)| (a == x && b == y) || (a == y && b == x))
        };
        assert!(
            unwired_pair(&lead_rt, &worker_rt),
            "the edge no declared owner wants is pruned: {unwired:?}"
        );
        assert!(
            !unwired_pair(&worker_rt, &helper_rt),
            "the definition-declared edge is kept: {unwired:?}"
        );
        Ok(())
    }

    /// Without a topology provider the desired set covers only what was
    /// declared to this runtime: a live edge it did not wire (for example the
    /// mob definition's spawn-time wiring) is not its to remove, after a
    /// restart as before one.
    #[tokio::test]
    async fn without_a_topology_provider_an_unmanaged_edge_is_left_alone()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let lead = AgentIdentity::parse("identity:lead")?;
        let member = AgentIdentity::parse("identity:member")?;
        let (_open_gate, gate) = watch::channel(false);
        let bridge = Arc::new(WedgeableTopologyBridge::new(gate));
        let runtime = runtime_with(bridge.clone(), "edge-prune-no-provider")?;
        assert_eq!(
            runtime.identity_edge_ownership(),
            IdentityEdgeOwnership::WiredByThisProcess
        );
        let lead_rt = register_active(&runtime, &bridge, &lead, 0).await?;
        let member_rt = register_active(&runtime, &bridge, &member, 0).await?;
        bridge.land_wire(&lead_rt, &member_rt);

        runtime.reconcile_managed_peer_edges(&[]).await?;

        assert!(
            bridge.unwire_calls().is_empty(),
            "an edge this runtime did not wire is left alone: {:?}",
            bridge.unwire_calls()
        );
        let edge = ManagedPeerEdge::new(lead.clone(), member.clone())?;
        assert!(runtime.logical_peer_edges().await?.contains(&edge));
        Ok(())
    }

    /// An endpoint whose binding moves during every plan's wiring exhausts
    /// the bounded replans and settles as `PeerTopologySuperseded`, having
    /// committed nothing.
    #[tokio::test]
    async fn a_binding_that_keeps_moving_settles_as_peer_topology_superseded()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let alpha = AgentIdentity::parse("agent:superseded-alpha")?;
        let beta = AgentIdentity::parse("agent:superseded-beta")?;
        let (_open_gate, gate) = watch::channel(false);
        let mut bridge = WedgeableTopologyBridge::new(gate);
        bridge.wedge_every_wire = true;
        let bridge = Arc::new(bridge);
        let runtime = runtime_with(bridge.clone(), "topology-superseded")?;
        register_active(&runtime, &bridge, &alpha, 0).await?;
        register_active(&runtime, &bridge, &beta, 0).await?;
        let edges = vec![ManagedPeerEdge::new(alpha.clone(), beta.clone())?];

        let reconcile = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.reconcile_managed_peer_edges(&edges).await }
        });
        for generation in 1..=u64::from(PEER_TOPOLOGY_MAX_PLAN_ATTEMPTS) {
            tokio::time::timeout(ADMISSION_WAIT, bridge.wire_entered.notified())
                .await
                .map_err(|_| "each plan reaches its wire call")?;
            // beta is respawned while this plan's wire is in flight.
            register_active(&runtime, &bridge, &beta, generation).await?;
            bridge.wire_release.notify_one();
        }
        match tokio::time::timeout(ADMISSION_WAIT, reconcile).await?? {
            Err(IdentityRuntimeError::PeerTopologySuperseded { attempts }) => {
                assert_eq!(attempts, PEER_TOPOLOGY_MAX_PLAN_ATTEMPTS);
            }
            other => return Err(format!("expected PeerTopologySuperseded, got {other:?}").into()),
        }
        assert_eq!(
            bridge.wire_calls().len(),
            PEER_TOPOLOGY_MAX_PLAN_ATTEMPTS as usize,
            "one wire per plan"
        );
        assert!(
            runtime.managed_peer_edges_snapshot().await.is_empty(),
            "a stale plan commits nothing"
        );
        Ok(())
    }

    /// A topology mutation whose endpoint is respawned while its wire is in
    /// flight replans and wires the live incarnation, committing only that.
    #[tokio::test]
    async fn a_topology_mutation_replans_when_an_endpoint_binding_changes_during_wiring()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let alpha = AgentIdentity::parse("agent:mutate-alpha")?;
        let beta = AgentIdentity::parse("agent:mutate-beta")?;
        let (open_gate, gate) = watch::channel(false);
        let mut bridge = WedgeableTopologyBridge::new(gate);
        bridge.wedge_first_wire = true;
        let bridge = Arc::new(bridge);
        let runtime = runtime_with(bridge.clone(), "topology-mutation-replan")?;
        let alpha_rt = register_active(&runtime, &bridge, &alpha, 0).await?;
        register_active(&runtime, &bridge, &beta, 0).await?;
        let edge = ManagedPeerEdge::new(alpha.clone(), beta.clone())?;

        let mutation = tokio::spawn({
            let runtime = runtime.clone();
            let edge = edge.clone();
            async move {
                runtime
                    .mutate_managed_peer_edge_admitted(
                        crate::topology_control::TopologyAction::Connect,
                        &edge,
                    )
                    .await
            }
        });
        tokio::time::timeout(ADMISSION_WAIT, bridge.wire_entered.notified())
            .await
            .map_err(|_| "the mutation reaches its wire call")?;
        let beta_rt_new = register_active(&runtime, &bridge, &beta, 1).await?;
        open_gate.send_replace(true);
        tokio::time::timeout(ADMISSION_WAIT, mutation).await???;

        let calls = bridge.wire_calls();
        assert_eq!(
            calls.len(),
            2,
            "one stale wire, one replanned wire: {calls:?}"
        );
        assert!(
            calls[1]
                .iter()
                .any(|(a, b)| (a == &alpha_rt && b == &beta_rt_new)
                    || (a == &beta_rt_new && b == &alpha_rt)),
            "the replan wired the live incarnation: {calls:?}"
        );
        assert!(
            runtime
                .managed_peer_edges_snapshot()
                .await
                .contains(&(edge.a().clone(), edge.b().clone()))
        );
        Ok(())
    }

    /// A session bridge whose wiring commands run in order on one simulated
    /// mob actor: a command enqueued later runs after an earlier one, even
    /// when the earlier command's caller stopped waiting for its reply. The
    /// inspection, like the real one, reads live state directly (NOT through
    /// the actor queue), so a command still queued is invisible to it.
    struct OrderedActorBridge {
        actor: Arc<tokio::sync::Mutex<()>>,
        gate: watch::Receiver<bool>,
        park_first_wire: std::sync::atomic::AtomicBool,
        live_wires: Arc<StdTestMutex<Vec<(AgentRuntimeId, AgentRuntimeId)>>>,
        events: Arc<StdTestMutex<Vec<String>>>,
        wire_entered: Notify,
        unwire_entered: Notify,
        /// The members Core holds.
        core: crate::identity_first::core_test_members::CoreTestMembers,
    }

    impl OrderedActorBridge {
        fn new(gate: watch::Receiver<bool>) -> Self {
            Self {
                actor: Arc::new(tokio::sync::Mutex::new(())),
                gate,
                park_first_wire: std::sync::atomic::AtomicBool::new(true),
                live_wires: Arc::new(StdTestMutex::new(Vec::new())),
                events: Arc::new(StdTestMutex::new(Vec::new())),
                wire_entered: Notify::new(),
                unwire_entered: Notify::new(),
                core: crate::identity_first::core_test_members::CoreTestMembers::default(),
            }
        }

        fn events(&self) -> Vec<String> {
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn live_wires(&self) -> Vec<(AgentRuntimeId, AgentRuntimeId)> {
            self.live_wires
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl SessionBridge for OrderedActorBridge {
        async fn create_session(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<crate::identity_first::MemberRuntimeBinding, BridgeError> {
            let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            let binding = crate::identity_first::MemberRuntimeBinding {
                agent_runtime_id,
                session_id: Some(meerkat_core::types::SessionId::new()),
            };
            self.core.bind(identity, &binding, &spec.profile);
            Ok(binding)
        }

        async fn resume_session(
            &self,
            identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            session_id: &SessionId,
            _resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            Ok(ResumeSessionOutcome::Resumed {
                session_id: session_id.clone(),
                agent_runtime_id,
            })
        }

        async fn deliver_admitted(
            &self,
            _runtime_id: &AgentRuntimeId,
            _delivery: BridgeDelivery,
        ) -> Result<SessionId, BridgeError> {
            Err(BridgeError::Mob(
                "delivery not used in this test".to_string(),
            ))
        }

        async fn wire_peer(
            &self,
            a: &AgentRuntimeId,
            b: &AgentRuntimeId,
        ) -> Result<(), BridgeError> {
            // Enqueue on the actor: once holding its turn, the command runs
            // to completion in the actor even if this caller is dropped.
            let turn = self.actor.clone().lock_owned().await;
            let park = self
                .park_first_wire
                .swap(false, std::sync::atomic::Ordering::SeqCst);
            let (done, reply) = tokio::sync::oneshot::channel();
            let mut gate = self.gate.clone();
            let live = self.live_wires.clone();
            let events = self.events.clone();
            let (a, b) = (a.clone(), b.clone());
            self.wire_entered.notify_one();
            tokio::spawn(async move {
                let _turn = turn;
                if park {
                    while !*gate.borrow_and_update() {
                        if gate.changed().await.is_err() {
                            return;
                        }
                    }
                }
                {
                    let mut live = live
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    live.push((a.clone(), b.clone()));
                    live.push((b.clone(), a.clone()));
                }
                events
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(format!("wire-landed:{a}:{b}"));
                let _ = done.send(());
            });
            reply
                .await
                .map_err(|_| BridgeError::Mob("actor dropped the wire".to_string()))
        }

        async fn current_member_wires(
            &self,
        ) -> Result<Vec<(AgentRuntimeId, AgentRuntimeId)>, BridgeError> {
            Ok(self.live_wires())
        }

        async fn unwire_peer(
            &self,
            a: &AgentRuntimeId,
            b: &AgentRuntimeId,
        ) -> Result<(), BridgeError> {
            self.unwire_entered.notify_one();
            // Ordered behind every earlier command, and tolerant: removing a
            // wire that is not there is Ok.
            let _turn = self.actor.clone().lock_owned().await;
            self.live_wires
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(x, y)| !((x == a && y == b) || (x == b && y == a)));
            self.events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("unwire-applied:{a}:{b}"));
            Ok(())
        }

        async fn checkpoint_session(
            &self,
            _runtime_id: &AgentRuntimeId,
            _session_id: &SessionId,
        ) -> Result<SessionSnapshot, BridgeError> {
            Err(BridgeError::Mob(
                "checkpoint not used in this test".to_string(),
            ))
        }

        async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            self.core.retire(runtime_id);
            Ok(())
        }

        async fn live_member(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn live_member_scope(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
            self.core.live_member_scope(identity)
        }

        async fn ensure_member_live(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn last_member_binding(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
            self.core.last_member_binding(identity)
        }

        async fn member_binding_history(
            &self,
        ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
            self.core.history()
        }

        async fn identity_management(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
            Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
        }
    }

    /// A wire whose reply timed out is still queued in a wedged actor when
    /// the next reconcile runs, so inspection cannot see it. That reconcile
    /// must not trust inspection for the unsettled edge: it issues an
    /// ordered, tolerant unwire, which runs after the wire lands, and the
    /// edge stays unsettled until that unwire succeeded. Before the fix the
    /// edge was folded into the managed set, found absent, dropped at commit,
    /// and then landed unmanaged (a leaked wire).
    #[tokio::test(start_paused = true)]
    async fn a_wire_that_lands_after_the_next_reconcile_is_still_unwired()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let alpha = AgentIdentity::parse("agent:ordered-alpha")?;
        let beta = AgentIdentity::parse("agent:ordered-beta")?;
        let (open_gate, gate) = watch::channel(false);
        let bridge = Arc::new(OrderedActorBridge::new(gate));
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "ordered-late-wire".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        }));
        let mut runtime_ids = Vec::new();
        for identity in [&alpha, &beta] {
            // Core holds both members live.
            let runtime_id = AgentRuntimeId::parse(&format!("rt:{}:0", identity.as_str()))?;
            let spec = spec(identity);
            bridge.core.bind(
                identity,
                &crate::identity_first::MemberRuntimeBinding {
                    agent_runtime_id: runtime_id.clone(),
                    session_id: Some(SessionId::new()),
                },
                &spec.profile,
            );
            runtime.register(spec).await;
            runtime_ids.push(runtime_id);
        }
        let edge = ManagedPeerEdge::new(alpha.clone(), beta.clone())?;
        let key = (edge.a().clone(), edge.b().clone());

        // 1. The wire is enqueued, parks in the actor, and its reply times out.
        match runtime
            .reconcile_managed_peer_edges(std::slice::from_ref(&edge))
            .await
        {
            Err(IdentityRuntimeError::PeerWiringTimedOut { operation, .. }) => {
                assert_eq!(operation, "wire_peers_batch");
            }
            other => return Err(format!("expected a wire timeout, got {other:?}").into()),
        }
        assert!(
            runtime
                .unsettled_peer_wiring_snapshot()
                .await
                .contains(&key)
        );
        assert!(bridge.live_wires().is_empty(), "the wire is still queued");

        // 2. The desired set drops the edge and the next reconcile runs while
        //    the wire is STILL queued (inspection sees nothing).
        runtime.set_desired_peer_edges(Vec::new()).await;
        let reconcile = tokio::spawn({
            let runtime = runtime.clone();
            async move { runtime.reconcile_managed_peer_edges(&[]).await }
        });
        tokio::time::timeout(ADMISSION_WAIT, bridge.unwire_entered.notified())
            .await
            .map_err(|_| "the reconcile issues an ordered unwire for the unsettled edge")?;
        assert!(
            runtime
                .unsettled_peer_wiring_snapshot()
                .await
                .contains(&key),
            "the edge stays unsettled until an ordered command succeeded"
        );

        // 3. Only now does the queued wire land; the unwire runs after it.
        open_gate.send_replace(true);
        tokio::time::timeout(ADMISSION_WAIT, reconcile)
            .await
            .map_err(|_| "the reconcile settles once the actor drains")???;

        let events = bridge.events();
        let landed = events
            .iter()
            .position(|event| event.starts_with("wire-landed:"));
        let unwired = events
            .iter()
            .position(|event| event.starts_with("unwire-applied:"));
        match (landed, unwired) {
            (Some(landed), Some(unwired)) => assert!(
                landed < unwired,
                "the ordered unwire runs after the late wire: {events:?}"
            ),
            _ => return Err(format!("expected a late wire then an unwire: {events:?}").into()),
        }
        assert!(bridge.live_wires().is_empty(), "no wire leaked");
        assert!(runtime.unsettled_peer_wiring_snapshot().await.is_empty());
        assert!(runtime.managed_peer_edges_snapshot().await.is_empty());
        Ok(())
    }

    /// The topology commit takes no lifecycle lock: a reconcile completes and
    /// commits its edge while the test holds an endpoint's lifecycle lock (as
    /// a lifecycle operation or an admission would).
    #[tokio::test]
    async fn a_topology_commit_goes_through_while_an_endpoint_lifecycle_lock_is_held()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let alpha = AgentIdentity::parse("agent:locked-alpha")?;
        let beta = AgentIdentity::parse("agent:locked-beta")?;
        let (_open_gate, gate) = watch::channel(false);
        let bridge = Arc::new(WedgeableTopologyBridge::new(gate));
        let runtime = runtime_with(bridge.clone(), "commit-without-lifecycle-locks")?;
        register_active(&runtime, &bridge, &alpha, 0).await?;
        register_active(&runtime, &bridge, &beta, 0).await?;
        let edge = ManagedPeerEdge::new(alpha.clone(), beta.clone())?;

        let alpha_lock = runtime.lifecycle_lock_for(&alpha).await;
        let beta_lock = runtime.lifecycle_lock_for(&beta).await;
        let _alpha_held = alpha_lock.lock().await;
        let _beta_held = beta_lock.lock().await;
        tokio::time::timeout(
            ADMISSION_WAIT,
            runtime.reconcile_managed_peer_edges(std::slice::from_ref(&edge)),
        )
        .await
        .map_err(|_| "the topology commit must not wait on an endpoint lifecycle lock")??;
        assert!(
            runtime
                .managed_peer_edges_snapshot()
                .await
                .contains(&(edge.a().clone(), edge.b().clone())),
            "the edge is committed while both lifecycle locks are held"
        );
        Ok(())
    }

    /// The wake path's target reconcile is bounded end to end: when its
    /// topology locks stay held past the budget, it settles as
    /// `PeerWiringTimedOut { operation: "wake_topology_locks" }`, which is
    /// logged, and the woken member's input is still admitted.
    #[tokio::test(start_paused = true)]
    async fn a_wake_topology_lock_timeout_does_not_refuse_admission()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let sleeper = AgentIdentity::parse("agent:wake-lock-sleeper")?;
        let peer = AgentIdentity::parse("agent:wake-lock-peer")?;
        let (_open_gate, gate) = watch::channel(false);
        let bridge = Arc::new(WedgeableTopologyBridge::new(gate));
        let runtime = runtime_with(bridge.clone(), "wake-lock-timeout")?;
        register_active(&runtime, &bridge, &peer, 0).await?;
        runtime.register(spec(&sleeper)).await;
        runtime
            .set_desired_peer_edges(vec![ManagedPeerEdge::new(sleeper.clone(), peer.clone())?])
            .await;

        // Another topology operation holds the reconcile lock throughout.
        let _held = runtime.managed_peer_reconcile_lock.lock().await;
        let started = tokio::time::Instant::now();
        runtime
            .send_with_mode_tracked(
                &sleeper,
                &meerkat_core::ContentInput::Text("wake up".to_string()),
                HandlingMode::Queue,
            )
            .await?;
        assert!(
            started.elapsed() >= WAKE_TOPOLOGY_BUDGET,
            "the wake reconcile waited out its single budget"
        );
        let sleeper_rt = runtime
            .status(&sleeper)
            .await?
            .agent_runtime_id
            .ok_or("the woken member has a runtime binding")?;
        assert!(
            bridge
                .admissions()
                .iter()
                .any(|(runtime_id, mode)| runtime_id == sleeper_rt.as_str()
                    && *mode == HandlingMode::Queue),
            "the woken member's input is admitted despite the lock timeout"
        );
        assert!(
            bridge.wire_calls().is_empty(),
            "nothing was wired while the reconcile lock was held"
        );
        Ok(())
    }

    /// Concurrent admissions to one target merge their post-admission
    /// hydrations: while the first is parked (building a Dormant peer), two
    /// more admissions only request ONE more pass, and no second hydration
    /// runs in parallel.
    #[tokio::test]
    async fn concurrent_hydrations_to_one_target_merge_into_one_rerun()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let target = AgentIdentity::parse("agent:merge-target")?;
        let peer = AgentIdentity::parse("agent:merge-peer")?;
        let (open_gate, gate) = watch::channel(false);
        let mut bridge = WedgeableTopologyBridge::new(gate);
        bridge.wedge_builds = true;
        let bridge = Arc::new(bridge);
        let runtime = runtime_with(bridge.clone(), "merged-hydrations")?;
        register_active(&runtime, &bridge, &target, 0).await?;
        runtime.register(spec(&peer)).await;
        runtime
            .set_desired_peer_edges(vec![ManagedPeerEdge::new(target.clone(), peer.clone())?])
            .await;
        let passes = || {
            runtime
                .peer_hydration_passes
                .load(std::sync::atomic::Ordering::SeqCst)
        };
        let send = |text: &'static str| {
            let runtime = runtime.clone();
            let target = target.clone();
            async move {
                runtime
                    .send_with_mode_tracked(
                        &target,
                        &meerkat_core::ContentInput::Text(text.to_string()),
                        HandlingMode::Queue,
                    )
                    .await
            }
        };

        tokio::time::timeout(ADMISSION_WAIT, send("first"))
            .await
            .map_err(|_| "the first send is admitted")??;
        // Its hydration parks building the Dormant peer.
        tokio::time::timeout(ADMISSION_WAIT, bridge.build_entered.notified())
            .await
            .map_err(|_| "the first hydration reaches the peer build")?;
        for text in ["second", "third"] {
            tokio::time::timeout(ADMISSION_WAIT, send(text))
                .await
                .map_err(|_| "a later send is admitted while hydration is parked")??;
        }
        assert_eq!(passes(), 1, "no second hydration started in parallel");
        {
            let hydrations = runtime
                .peer_hydrations_in_flight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(hydrations.in_flight.len(), 1);
            assert!(
                hydrations
                    .in_flight
                    .get(&target)
                    .is_some_and(|claim| claim.rerun),
                "the later admissions requested one more pass"
            );
        }

        open_gate.send_replace(true);
        tokio::time::timeout(ADMISSION_WAIT, async {
            loop {
                let idle = runtime
                    .peer_hydrations_in_flight
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .in_flight
                    .is_empty();
                if idle {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .map_err(|_| "the merged hydration finishes")?;
        assert_eq!(passes(), 2, "three admissions ran exactly one extra pass");
        assert_eq!(bridge.admissions().len(), 3);
        Ok(())
    }

    /// A stale release (a second removal after a panic, or a refused spawn)
    /// only releases its own claim, never a newer one.
    #[test]
    fn a_stale_hydration_release_never_drops_a_newer_claim()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let target = AgentIdentity::parse("agent:claim-target")?;
        let hydrations = StdMutex::new(PeerHydrations::default());
        let lock = || {
            hydrations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        let first = lock().claim(&target).ok_or("first claim")?;
        {
            let slot = PeerHydrationSlot {
                hydrations: &hydrations,
                identity: target.clone(),
                token: first,
            };
            assert!(!slot.finish_pass(), "no rerun pending: released");
            // Dropping the slot releases again; it must be a no-op.
        }
        let second = lock().claim(&target).ok_or("the slot is free again")?;
        assert_ne!(first, second);
        // The first claim's stale release does not drop the second.
        lock().release(&target, first);
        assert!(
            lock()
                .in_flight
                .get(&target)
                .is_some_and(|claim| claim.token == second)
        );
        assert!(
            lock().claim(&target).is_none(),
            "the second claim still owns it"
        );
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod wake_refusal_classifier_tests {
    //! Per reason: each of Core's answers to
    //! a failed resume maps to one typed outcome, one JSON-RPC code, one wire
    //! kind and one recorded remedy, matched by kind and never by text.
    use super::*;
    use meerkat_core::service::{DurableResumeHold, SessionError};
    use meerkat_mob::error::SessionResumeUnavailableReason as Unavailable;

    use super::super::types::{WakeRefusal, WakeRefusalRemedy};

    fn keeper() -> Result<AgentIdentity, Box<dyn std::error::Error>> {
        Ok(AgentIdentity::parse("keeper")?)
    }

    fn rejected(kind: ResumeRejectionKind) -> BridgeError {
        BridgeError::ResumeRejected {
            kind,
            session_id: SessionId::new(),
            detail: "resume refused".to_string(),
        }
    }

    /// A classified wake failure with what each surface shows of it.
    struct Classified {
        error: IdentityRuntimeError,
        code: i64,
        data: Option<serde_json::Value>,
        recorded: Option<WakeRefusal>,
    }

    fn classify(
        session_id: Option<&SessionId>,
        error: BridgeError,
    ) -> Result<Classified, Box<dyn std::error::Error>> {
        let error = classify_wake_failure(&keeper()?, session_id, error);
        Ok(Classified {
            code: crate::rpc::identity_error_code(&error),
            data: error.structured_data(),
            recorded: wake_refusal_of(&error),
            error,
        })
    }

    /// Archived beyond revival and contradictory durable authority are
    /// Core's final answers: `IdentityUnresumable` (-32051) naming the
    /// session, remedy `delete_identity`, recorded as final.
    #[test]
    fn archived_and_contradictory_sessions_are_final_unresumable()
    -> Result<(), Box<dyn std::error::Error>> {
        let session_id = SessionId::new();
        for (reason, expected, token) in [
            (
                Unavailable::ArchivedNotRevivable,
                UnresumableReason::ArchivedNotRevivable,
                "archived_not_revivable",
            ),
            (
                Unavailable::ContradictoryDurableAuthority,
                UnresumableReason::ContradictoryDurableAuthority,
                "contradictory_durable_authority",
            ),
        ] {
            let classified = classify(
                Some(&session_id),
                rejected(ResumeRejectionKind::Unavailable { reason }),
            )?;
            assert!(
                matches!(
                    &classified.error,
                    IdentityRuntimeError::IdentityUnresumable {
                        session_id: refused,
                        reason: got,
                        ..
                    } if *refused == session_id && *got == expected
                ),
                "{reason:?}: {:?}",
                classified.error
            );
            assert_eq!(
                classified.code,
                crate::rpc::IDENTITY_UNRESUMABLE_CODE,
                "{reason:?}"
            );
            let data = classified
                .data
                .ok_or("an unresumable refusal carries data")?;
            assert_eq!(data["kind"], "identity_unresumable", "{reason:?}");
            assert_eq!(data["reason"], token, "{reason:?}");
            assert_eq!(data["session_id"], session_id.to_string(), "{reason:?}");
            assert_eq!(data["remedy"], "delete_identity", "{reason:?}");
            assert_eq!(data["retryable"], false, "{reason:?}");
            let recorded = classified
                .recorded
                .ok_or("a final refusal is recorded for the views")?;
            assert_eq!(recorded.code, crate::rpc::IDENTITY_UNRESUMABLE_CODE);
            assert_eq!(recorded.session_id, session_id);
            assert_eq!(recorded.remedy, WakeRefusalRemedy::DeleteIdentity);
            assert!(recorded.is_final(), "{reason:?}");
        }
        Ok(())
    }

    /// An unprovable committed boundary is held, not final: the durable
    /// copies stay for reconciliation, the wake answers -32603 with Core's
    /// reason in its data, and the operator reviews it.
    #[test]
    fn an_unprovable_committed_boundary_is_held_for_operator_review()
    -> Result<(), Box<dyn std::error::Error>> {
        let session_id = SessionId::new();
        let classified = classify(
            Some(&session_id),
            rejected(ResumeRejectionKind::Unavailable {
                reason: Unavailable::CommittedBoundaryUnprovable,
            }),
        )?;
        assert!(
            matches!(
                &classified.error,
                IdentityRuntimeError::WakeHeld {
                    session_id: held,
                    hold: WakeHold::CommittedBoundaryUnprovable,
                    ..
                } if *held == session_id
            ),
            "{:?}",
            classified.error
        );
        assert_eq!(classified.code, -32603);
        let data = classified.data.ok_or("a held refusal carries data")?;
        assert_eq!(data["kind"], "mob_member_session_resume_held");
        assert_eq!(
            data["resume_unavailable_reason"],
            "committed_boundary_unprovable"
        );
        assert_eq!(data["session_id"], session_id.to_string());
        assert_eq!(data["content_retained"], true);
        assert_eq!(data["retryable"], false);
        assert!(
            data.get("remedy").is_none(),
            "held error data carries no remedy key: {data}"
        );
        let recorded = classified.recorded.ok_or("a hold is recorded")?;
        assert_eq!(recorded.code, -32603);
        assert_eq!(recorded.remedy, WakeRefusalRemedy::OperatorReview);
        assert!(!recorded.is_final());
        Ok(())
    }

    /// Authority that moved while the body loaded is transient: the wake
    /// answers -32052 retryable and records nothing; the next wake reads
    /// the session again.
    #[test]
    fn authority_changed_during_materialization_is_retryable()
    -> Result<(), Box<dyn std::error::Error>> {
        let session_id = SessionId::new();
        let classified = classify(
            Some(&session_id),
            rejected(ResumeRejectionKind::Unavailable {
                reason: Unavailable::AuthorityChangedDuringMaterialization,
            }),
        )?;
        assert!(
            matches!(
                &classified.error,
                IdentityRuntimeError::IdentityWakeRetryable {
                    session_id: Some(retried),
                    ..
                } if *retried == session_id
            ),
            "{:?}",
            classified.error
        );
        assert_eq!(
            classified.code,
            crate::rpc::IDENTITY_HISTORY_UNAVAILABLE_CODE
        );
        let data = classified.data.ok_or("a retryable refusal carries data")?;
        assert_eq!(data["kind"], "identity_wake_retryable");
        assert_eq!(data["session_id"], session_id.to_string());
        assert_eq!(data["retryable"], true);
        assert!(
            classified.recorded.is_none(),
            "a transient refusal is never recorded"
        );
        Ok(())
    }

    /// Core's `Absent` is the one answer the wake replaces with a fresh
    /// spawn: the predicate matches it and nothing else, never by
    /// text. Were it to reach the classifier anyway, it stays transient.
    #[test]
    fn only_an_absent_session_is_replaced_by_a_fresh_spawn()
    -> Result<(), Box<dyn std::error::Error>> {
        for reason in [
            Unavailable::ArchivedNotRevivable,
            Unavailable::CommittedBoundaryUnprovable,
            Unavailable::AuthorityChangedDuringMaterialization,
            Unavailable::ContradictoryDurableAuthority,
            Unavailable::Absent,
        ] {
            assert_eq!(
                resume_refused_absent(&rejected(ResumeRejectionKind::Unavailable { reason })),
                reason == Unavailable::Absent,
                "{reason:?}"
            );
        }
        for hold in [
            DurableResumeHold::TailHeldForRecovery,
            DurableResumeHold::RecoveryRefused,
            DurableResumeHold::EvidenceQuarantined,
            DurableResumeHold::AuditedEndpointDivergence,
        ] {
            assert!(
                !resume_refused_absent(&rejected(ResumeRejectionKind::Held { hold })),
                "{hold:?}"
            );
        }
        assert!(!resume_refused_absent(&rejected(
            ResumeRejectionKind::RoleMigrationNotApplied
        )));
        assert!(
            !resume_refused_absent(&BridgeError::Mob(Unavailable::Absent.to_string())),
            "Core's Absent prose is never read as Absent"
        );
        let session_id = SessionId::new();
        let classified = classify(
            Some(&session_id),
            rejected(ResumeRejectionKind::Unavailable {
                reason: Unavailable::Absent,
            }),
        )?;
        assert!(
            matches!(
                classified.error,
                IdentityRuntimeError::IdentityWakeRetryable { .. }
            ),
            "{:?}",
            classified.error
        );
        assert!(classified.recorded.is_none());
        Ok(())
    }

    /// Core's durable holds keep the session: the audited-endpoint
    /// divergence is repaired (`repair_session`), the tail and evidence
    /// holds wait for operator review, and a refused tail recovery is
    /// retried (Core's own advice), unrecorded.
    #[test]
    fn each_durable_resume_hold_maps_to_its_remedy() -> Result<(), Box<dyn std::error::Error>> {
        let session_id = SessionId::new();
        let audited = classify(
            Some(&session_id),
            rejected(ResumeRejectionKind::Held {
                hold: DurableResumeHold::AuditedEndpointDivergence,
            }),
        )?;
        assert!(
            matches!(
                &audited.error,
                IdentityRuntimeError::WakeHeld {
                    hold: WakeHold::DurableResume(DurableResumeHold::AuditedEndpointDivergence),
                    ..
                }
            ),
            "{:?}",
            audited.error
        );
        assert_eq!(audited.code, -32603);
        let data = audited.data.ok_or("a held refusal carries data")?;
        assert_eq!(data["kind"], "mob_member_session_repair_required");
        assert_eq!(
            data[SessionError::DURABLE_RESUME_HOLD_KEY],
            "audited_endpoint_divergence"
        );
        assert_eq!(data["session_id"], session_id.to_string());
        assert!(data.get("remedy").is_none(), "{data}");
        assert_eq!(
            audited.recorded.ok_or("a hold is recorded")?.remedy,
            WakeRefusalRemedy::RepairSession
        );

        for hold in [
            DurableResumeHold::TailHeldForRecovery,
            DurableResumeHold::EvidenceQuarantined,
        ] {
            let held = classify(
                Some(&session_id),
                rejected(ResumeRejectionKind::Held { hold }),
            )?;
            assert!(
                matches!(
                    &held.error,
                    IdentityRuntimeError::WakeHeld {
                        hold: WakeHold::DurableResume(got),
                        ..
                    } if *got == hold
                ),
                "{hold:?}: {:?}",
                held.error
            );
            assert_eq!(held.code, -32603, "{hold:?}");
            let data = held.data.ok_or("a held refusal carries data")?;
            assert_eq!(data["kind"], "mob_member_session_resume_held", "{hold:?}");
            assert_eq!(
                data[SessionError::DURABLE_RESUME_HOLD_KEY],
                hold.as_str(),
                "{hold:?}"
            );
            assert_eq!(data["session_id"], session_id.to_string(), "{hold:?}");
            assert_eq!(data["content_retained"], true, "{hold:?}");
            assert_eq!(data["retryable"], false, "{hold:?}");
            assert!(data.get("remedy").is_none(), "{hold:?}: {data}");
            let recorded = held.recorded.ok_or("a hold is recorded")?;
            assert_eq!(
                recorded.remedy,
                WakeRefusalRemedy::OperatorReview,
                "{hold:?}"
            );
            assert!(!recorded.is_final(), "{hold:?}");
        }

        let refused = classify(
            Some(&session_id),
            rejected(ResumeRejectionKind::Held {
                hold: DurableResumeHold::RecoveryRefused,
            }),
        )?;
        assert!(
            matches!(
                refused.error,
                IdentityRuntimeError::IdentityWakeRetryable { .. }
            ),
            "{:?}",
            refused.error
        );
        assert_eq!(refused.code, crate::rpc::IDENTITY_HISTORY_UNAVAILABLE_CODE);
        assert!(refused.recorded.is_none());
        Ok(())
    }

    /// The registration reload path raises the audited-endpoint hold as its
    /// own bridge error; it classifies like the resume path's, keyed on the
    /// session that error names.
    #[test]
    fn a_session_repair_requirement_is_the_audited_endpoint_hold()
    -> Result<(), Box<dyn std::error::Error>> {
        let session_id = SessionId::new();
        let classified = classify(
            None,
            BridgeError::SessionRepairRequired {
                identity: crate::member_comms_id::roster_member_id_for_identity("keeper"),
                session_id: session_id.clone(),
                detail: "the audited endpoint diverged".to_string(),
            },
        )?;
        assert!(
            matches!(
                &classified.error,
                IdentityRuntimeError::WakeHeld {
                    session_id: held,
                    hold: WakeHold::DurableResume(DurableResumeHold::AuditedEndpointDivergence),
                    ..
                } if *held == session_id
            ),
            "{:?}",
            classified.error
        );
        assert_eq!(
            classified.recorded.ok_or("a hold is recorded")?.remedy,
            WakeRefusalRemedy::RepairSession
        );
        Ok(())
    }

    /// A role migration Core did not apply is held until the host declares
    /// it or the profile is restored.
    #[test]
    fn a_role_migration_core_did_not_apply_is_held_for_resolution()
    -> Result<(), Box<dyn std::error::Error>> {
        let session_id = SessionId::new();
        let classified = classify(
            Some(&session_id),
            rejected(ResumeRejectionKind::RoleMigrationNotApplied),
        )?;
        assert!(
            matches!(
                &classified.error,
                IdentityRuntimeError::WakeHeld {
                    hold: WakeHold::RoleMigrationNotApplied,
                    ..
                }
            ),
            "{:?}",
            classified.error
        );
        assert_eq!(classified.code, -32603);
        let data = classified.data.ok_or("a held refusal carries data")?;
        assert_eq!(data["kind"], "identity_role_migration_refused");
        assert!(data.get("remedy").is_none(), "{data}");
        assert_eq!(
            classified.recorded.ok_or("a hold is recorded")?.remedy,
            WakeRefusalRemedy::ResolveRoleMigration
        );
        Ok(())
    }

    /// Text never classifies: Core's prose for every final and held reason,
    /// a rejection kind with no durable meaning, and a typed refusal with no
    /// session to name are all transient, and none is recorded.
    #[test]
    fn text_and_untyped_rejections_never_classify_as_final_or_held()
    -> Result<(), Box<dyn std::error::Error>> {
        let session_id = SessionId::new();
        let prose = BridgeError::Mob(format!(
            "resume failed: {} / {} / {} / {}",
            Unavailable::ArchivedNotRevivable,
            Unavailable::ContradictoryDurableAuthority,
            Unavailable::CommittedBoundaryUnprovable,
            DurableResumeHold::AuditedEndpointDivergence,
        ));
        for (label, session, error) in [
            ("prose", Some(&session_id), prose),
            (
                "untyped kind",
                Some(&session_id),
                rejected(ResumeRejectionKind::Other),
            ),
            (
                "no session",
                None,
                rejected(ResumeRejectionKind::Unavailable {
                    reason: Unavailable::ArchivedNotRevivable,
                }),
            ),
        ] {
            let classified = classify(session, error)?;
            assert!(
                matches!(
                    classified.error,
                    IdentityRuntimeError::IdentityWakeRetryable { .. }
                ),
                "{label}: {:?}",
                classified.error
            );
            assert_eq!(
                classified.code,
                crate::rpc::IDENTITY_HISTORY_UNAVAILABLE_CODE,
                "{label}"
            );
            assert!(classified.recorded.is_none(), "{label}");
        }
        Ok(())
    }

    /// The host's deterministic build refusal on a resume is the typed
    /// build refusal on every attempt, and it is never recorded.
    #[test]
    fn a_deterministic_build_refusal_on_resume_is_host_rejected_build()
    -> Result<(), Box<dyn std::error::Error>> {
        let classified = classify(
            Some(&SessionId::new()),
            BridgeError::Mob(
                "spawn_member: callback/build_agent failed: callback error: gate refused"
                    .to_string(),
            ),
        )?;
        assert!(
            matches!(
                classified.error,
                IdentityRuntimeError::HostRejectedBuild { .. }
            ),
            "{:?}",
            classified.error
        );
        assert!(classified.recorded.is_none());
        Ok(())
    }

    /// A host adoption and an unreadable history answer their own codes, and neither is
    /// a recorded wake refusal.
    #[test]
    fn host_adopted_and_unreadable_history_answer_their_own_codes()
    -> Result<(), Box<dyn std::error::Error>> {
        let adopted = IdentityRuntimeError::IdentityHostAdopted {
            identity: keeper()?,
        };
        assert_eq!(
            crate::rpc::identity_error_code(&adopted),
            crate::rpc::IDENTITY_HOST_ADOPTED_CODE
        );
        let data = adopted
            .structured_data()
            .ok_or("the host-adoption refusal carries data")?;
        assert_eq!(data["kind"], "identity_host_adopted");
        assert_eq!(data["identity"], "keeper");
        assert!(wake_refusal_of(&adopted).is_none());

        let unreadable = IdentityRuntimeError::IdentityHistoryUnavailable {
            identity: keeper()?,
            detail: "the store is busy".to_string(),
        };
        assert_eq!(
            crate::rpc::identity_error_code(&unreadable),
            crate::rpc::IDENTITY_HISTORY_UNAVAILABLE_CODE
        );
        let data = unreadable
            .structured_data()
            .ok_or("an unreadable history carries data")?;
        assert_eq!(data["kind"], "identity_history_unavailable");
        assert_eq!(data["retryable"], true);
        assert!(wake_refusal_of(&unreadable).is_none());
        Ok(())
    }

    /// An identity that owes its host a held completion keeps the internal
    /// error code, and its data names the pending held completion, so a
    /// host reads the refusal without parsing its message.
    #[test]
    fn held_completion_owed_keeps_the_internal_code_with_typed_data()
    -> Result<(), Box<dyn std::error::Error>> {
        let owed = IdentityRuntimeError::HeldCompletionOwed {
            identity: keeper()?,
        };
        assert_eq!(crate::rpc::identity_error_code(&owed), -32603);
        assert_eq!(
            owed.structured_data(),
            Some(serde_json::json!({
                "kind": "held_completion_owed", "identity": "keeper",
            }))
        );
        Ok(())
    }

    /// The view reads the recorded refusal: only a final one shows
    /// `broken`; a hold leaves the identity `dormant`, and a live member is
    /// `active` whatever was recorded.
    #[test]
    fn only_a_final_refusal_shows_broken() {
        let refusal = |remedy| WakeRefusal {
            code: -32603,
            session_id: SessionId::new(),
            remedy,
        };
        assert_eq!(
            IdentityRuntime::view_state(false, None),
            IdentityLifecycleState::Dormant
        );
        assert_eq!(
            IdentityRuntime::view_state(false, Some(&refusal(WakeRefusalRemedy::DeleteIdentity))),
            IdentityLifecycleState::Broken
        );
        assert_eq!(
            IdentityRuntime::view_state(true, Some(&refusal(WakeRefusalRemedy::DeleteIdentity))),
            IdentityLifecycleState::Active
        );
        for remedy in [
            WakeRefusalRemedy::RepairSession,
            WakeRefusalRemedy::ResolveRoleMigration,
            WakeRefusalRemedy::OperatorReview,
        ] {
            assert_eq!(
                IdentityRuntime::view_state(false, Some(&refusal(remedy))),
                IdentityLifecycleState::Dormant,
                "{remedy:?}"
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod wake_per_reason_tests {
    //! A placement host that refuses the spawn itself (its realm backend is
    //! not persistent) answers the typed build refusal with its remedy, on
    //! every attempt and unrecorded.
    //!
    //! On the wake door: each answer Core gives for not resuming an
    //! identity's last session reaches `materialize` as its own typed
    //! outcome after exactly one resume attempt, and only `Absent` spawns. A
    //! refused wake creates nothing in Core, and the views record only the
    //! final and held refusals.
    use super::*;
    use meerkat_core::service::DurableResumeHold;
    use meerkat_mob::error::SessionResumeUnavailableReason as Unavailable;
    use std::sync::atomic::AtomicUsize;

    use super::super::bridge::{MemberRuntimeBinding, ResumeSessionOutcome};
    use super::super::local_lease::LocalLeaseProvider;
    use super::super::local_store::LocalContinuityStore;
    use super::super::types::{ReplacedSession, ReplacedSessionReason, WakeRefusalRemedy};

    const PROFILE: &str = "worker";

    fn spec(identity: &AgentIdentity) -> DurableAgentSpec {
        DurableAgentSpec {
            identity: identity.clone(),
            profile: meerkat_mob::ProfileName::from(PROFILE),
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
        }
    }

    /// Core's member history for one retired identity: its last binding is
    /// `session` at generation 3, under the roster profile.
    fn retired_binding(
        identity: &AgentIdentity,
        session: &SessionId,
    ) -> meerkat_mob::MemberBinding {
        meerkat_mob::MemberBinding::for_test(
            crate::member_comms_id::roster_member_id_for_identity(identity.as_str()),
            meerkat_mob::Generation::new(3),
            Some(session.clone()),
            meerkat_mob::MemberBindingState::Retired,
            meerkat_mob::ProfileName::from(PROFILE),
            meerkat_mob::SpawnContinuityIntent::DurableIdentity {
                continuity_key: identity.as_str().to_string(),
            },
            meerkat_mob::MemberIdentityManagement::CommandDriven,
        )
    }

    /// A bridge over a Core that holds no live member for the identity and
    /// answers its resume with one fixed refusal, which names the session
    /// the wake asked for, as Core names the session it refused. It counts
    /// resume and create calls; a create commits a fresh binding.
    struct RefusingResumeBridge {
        last: meerkat_mob::MemberBinding,
        refusal: Box<dyn Fn(&SessionId) -> BridgeError + Send + Sync>,
        resumes: AtomicUsize,
        creates: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl SessionBridge for RefusingResumeBridge {
        async fn deliver_admitted(
            &self,
            _runtime_id: &AgentRuntimeId,
            _delivery: super::super::bridge::BridgeDelivery,
        ) -> Result<SessionId, BridgeError> {
            Err(BridgeError::Mob(
                "deliver is not used in this test".to_string(),
            ))
        }

        async fn checkpoint_session(
            &self,
            _runtime_id: &AgentRuntimeId,
            _session_id: &SessionId,
        ) -> Result<super::super::types::SessionSnapshot, BridgeError> {
            Err(BridgeError::Mob(
                "checkpoint is not used in this test".to_string(),
            ))
        }

        async fn create_session(
            &self,
            identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<MemberRuntimeBinding, BridgeError> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            Ok(MemberRuntimeBinding {
                agent_runtime_id: AgentRuntimeId::parse(&format!("rt:{}:4", identity.as_str()))
                    .map_err(|error| BridgeError::InvalidInput(error.to_string()))?,
                session_id: Some(SessionId::new()),
            })
        }

        async fn resume_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            session_id: &SessionId,
            _resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            self.resumes.fetch_add(1, Ordering::SeqCst);
            Err((self.refusal)(session_id))
        }

        async fn retire_member(&self, _runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn live_member(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
            Ok(None)
        }

        async fn live_member_scope(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
            Ok(None)
        }

        async fn last_member_binding(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
            Ok(Some(self.last.clone()))
        }

        async fn identity_management(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
            Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
        }

        async fn member_binding_history(
            &self,
        ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
            Ok(meerkat_mob::MemberBindingHistory::for_test(
                [self.last.clone()],
                [self.last.clone()],
                [],
                1,
            ))
        }
    }

    async fn runtime_refusing_with(
        identity: &AgentIdentity,
        session: &SessionId,
        refusal: impl Fn(&SessionId) -> BridgeError + Send + Sync + 'static,
    ) -> Result<(Arc<RefusingResumeBridge>, IdentityRuntime), Box<dyn std::error::Error>> {
        let bridge = Arc::new(RefusingResumeBridge {
            last: retired_binding(identity, session),
            refusal: Box::new(refusal),
            resumes: AtomicUsize::new(0),
            creates: AtomicUsize::new(0),
        });
        let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "wake-per-reason".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone() as Arc<dyn SessionBridge>),
            default_timeout: None,
        });
        runtime.register(spec(identity)).await;
        Ok((bridge, runtime))
    }

    /// What one refused wake must look like on every surface.
    struct Expected {
        code: i64,
        recorded: Option<WakeRefusalRemedy>,
        state: IdentityLifecycleState,
    }

    fn unavailable(
        reason: Unavailable,
    ) -> impl Fn(&SessionId) -> BridgeError + Send + Sync + 'static {
        move |session_id: &SessionId| BridgeError::ResumeRejected {
            kind: ResumeRejectionKind::Unavailable { reason },
            session_id: session_id.clone(),
            detail: format!("Core refused the resume: {reason:?}"),
        }
    }

    fn held(hold: DurableResumeHold) -> impl Fn(&SessionId) -> BridgeError + Send + Sync + 'static {
        move |session_id: &SessionId| BridgeError::ResumeRejected {
            kind: ResumeRejectionKind::Held { hold },
            session_id: session_id.clone(),
            detail: format!("Core held the resume: {hold:?}"),
        }
    }

    async fn assert_refused_without_spawning(
        case: &str,
        refusal: impl Fn(&SessionId) -> BridgeError + Send + Sync + 'static,
        expected: Expected,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("keeper")?;
        let session = SessionId::new();
        let (bridge, runtime) = runtime_refusing_with(&identity, &session, refusal).await?;

        let error = match runtime.materialize(&identity).await {
            Err(error) => error,
            Ok(scope) => panic!("{case}: the wake must refuse, got {scope:?}"),
        };
        assert_eq!(
            crate::rpc::identity_error_code(&error),
            expected.code,
            "{case}: {error:?}"
        );
        assert_eq!(
            bridge.resumes.load(Ordering::SeqCst),
            1,
            "{case}: one resume attempt, no retry loop"
        );
        assert_eq!(
            bridge.creates.load(Ordering::SeqCst),
            0,
            "{case}: a refused wake must spawn nothing in Core"
        );

        let status = runtime.status(&identity).await?;
        assert_eq!(
            status.wake_refusal.as_ref().map(|refusal| refusal.remedy),
            expected.recorded,
            "{case}: the recorded refusal"
        );
        if let Some(recorded) = status.wake_refusal.as_ref() {
            assert_eq!(recorded.session_id, session, "{case}: it names the session");
            assert_eq!(
                recorded.code, expected.code,
                "{case}: it keeps the wake's code"
            );
        }
        assert_eq!(status.state, expected.state, "{case}: the view state");
        assert_eq!(
            status.replaced_session, None,
            "{case}: nothing was replaced"
        );
        Ok(())
    }

    #[tokio::test]
    async fn every_refused_resume_reason_spawns_nothing_and_answers_its_own_code()
    -> Result<(), Box<dyn std::error::Error>> {
        let final_refusal = || Expected {
            code: crate::rpc::IDENTITY_UNRESUMABLE_CODE,
            recorded: Some(WakeRefusalRemedy::DeleteIdentity),
            state: IdentityLifecycleState::Broken,
        };
        let held_refusal = |remedy| Expected {
            code: -32603,
            recorded: Some(remedy),
            state: IdentityLifecycleState::Dormant,
        };
        let retryable = || Expected {
            code: crate::rpc::IDENTITY_HISTORY_UNAVAILABLE_CODE,
            recorded: None,
            state: IdentityLifecycleState::Dormant,
        };

        assert_refused_without_spawning(
            "archived not revivable",
            unavailable(Unavailable::ArchivedNotRevivable),
            final_refusal(),
        )
        .await?;
        assert_refused_without_spawning(
            "contradictory durable authority",
            unavailable(Unavailable::ContradictoryDurableAuthority),
            final_refusal(),
        )
        .await?;
        assert_refused_without_spawning(
            "committed boundary unprovable",
            unavailable(Unavailable::CommittedBoundaryUnprovable),
            held_refusal(WakeRefusalRemedy::OperatorReview),
        )
        .await?;
        assert_refused_without_spawning(
            "authority changed during materialization",
            unavailable(Unavailable::AuthorityChangedDuringMaterialization),
            retryable(),
        )
        .await?;
        assert_refused_without_spawning(
            "tail held for recovery",
            held(DurableResumeHold::TailHeldForRecovery),
            held_refusal(WakeRefusalRemedy::OperatorReview),
        )
        .await?;
        assert_refused_without_spawning(
            "recovery refused",
            held(DurableResumeHold::RecoveryRefused),
            retryable(),
        )
        .await?;
        assert_refused_without_spawning(
            "evidence quarantined",
            held(DurableResumeHold::EvidenceQuarantined),
            held_refusal(WakeRefusalRemedy::OperatorReview),
        )
        .await?;
        assert_refused_without_spawning(
            "audited endpoint divergence",
            held(DurableResumeHold::AuditedEndpointDivergence),
            held_refusal(WakeRefusalRemedy::RepairSession),
        )
        .await?;
        assert_refused_without_spawning(
            "role migration not applied",
            |session_id: &SessionId| BridgeError::ResumeRejected {
                kind: ResumeRejectionKind::RoleMigrationNotApplied,
                session_id: session_id.clone(),
                detail: "Core did not apply the role migration".to_string(),
            },
            held_refusal(WakeRefusalRemedy::ResolveRoleMigration),
        )
        .await?;
        assert_refused_without_spawning(
            "session repair required",
            |session_id: &SessionId| BridgeError::SessionRepairRequired {
                identity: crate::member_comms_id::roster_member_id_for_identity("keeper"),
                session_id: session_id.clone(),
                detail: "the audited endpoint diverged".to_string(),
            },
            held_refusal(WakeRefusalRemedy::RepairSession),
        )
        .await
    }

    /// The one reason that spawns: Core holds nothing durable for the last
    /// session, so the wake creates a fresh one, once, and says so (A3).
    #[tokio::test]
    async fn only_an_absent_last_session_spawns_fresh_with_its_marker()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("keeper")?;
        let session = SessionId::new();
        let (bridge, runtime) =
            runtime_refusing_with(&identity, &session, unavailable(Unavailable::Absent)).await?;

        runtime.materialize(&identity).await?;
        assert_eq!(bridge.resumes.load(Ordering::SeqCst), 1);
        assert_eq!(
            bridge.creates.load(Ordering::SeqCst),
            1,
            "an Absent last session is replaced by exactly one fresh spawn"
        );
        let status = runtime.status(&identity).await?;
        assert_eq!(
            status.replaced_session,
            Some(ReplacedSession {
                session_id: session,
                reason: ReplacedSessionReason::Absent,
            }),
            "the fresh spawn names the session it replaced"
        );
        assert_eq!(
            status.wake_refusal, None,
            "a fresh spawn records no refusal"
        );
        Ok(())
    }

    /// A bridge over a Core that holds nothing for the identity, whose
    /// placement host refuses every spawn because its realm backend is not
    /// persistent, or, with `durable_profile`, whose mob refuses every spawn
    /// of a durable profile because its member sessions do not outlive the
    /// process. It counts create calls.
    #[derive(Default)]
    struct NonPersistentHostBridge {
        creates: AtomicUsize,
        durable_profile: bool,
    }

    #[async_trait::async_trait]
    impl SessionBridge for NonPersistentHostBridge {
        async fn deliver_admitted(
            &self,
            _runtime_id: &AgentRuntimeId,
            _delivery: super::super::bridge::BridgeDelivery,
        ) -> Result<SessionId, BridgeError> {
            Err(BridgeError::Mob(
                "deliver is not used in this test".to_string(),
            ))
        }

        async fn checkpoint_session(
            &self,
            _runtime_id: &AgentRuntimeId,
            _session_id: &SessionId,
        ) -> Result<super::super::types::SessionSnapshot, BridgeError> {
            Err(BridgeError::Mob(
                "checkpoint is not used in this test".to_string(),
            ))
        }

        async fn create_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<MemberRuntimeBinding, BridgeError> {
            self.creates.fetch_add(1, Ordering::SeqCst);
            if self.durable_profile {
                return Err(BridgeError::NonDurableService {
                    profile: meerkat_mob::ProfileName::from("durable-worker"),
                    detail: "create_session: member durable uses durable profile \
                             'durable-worker', but the mob's member sessions do not outlive \
                             the process: run the mob on a persistent realm, or drop \
                             `durable` from the profile"
                        .to_string(),
                });
            }
            Err(BridgeError::RealmBackendUnavailable {
                detail: "create_session: materialize rejected: durable sessions required but \
                         the realm backend is not persistent"
                    .to_string(),
            })
        }

        async fn resume_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            _session_id: &SessionId,
            _resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            Err(BridgeError::Mob(
                "resume is not used in this test".to_string(),
            ))
        }

        async fn retire_member(&self, _runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn live_member(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
            Ok(None)
        }

        async fn live_member_scope(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
            Ok(None)
        }

        async fn last_member_binding(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
            Ok(None)
        }

        async fn identity_management(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
            Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
        }

        async fn member_binding_history(
            &self,
        ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
            Ok(meerkat_mob::MemberBindingHistory::for_test([], [], [], 0))
        }
    }

    /// A bridge over a Core that holds the identity's member live, whose
    /// delivery repair finds the member's first session absent from every
    /// durable authority: deliveries to that incarnation refuse `Absent`,
    /// deliveries to any later one land. A reset successor is bound at the
    /// next generation on a fresh session. `queue_events` records the
    /// deliveries, the successor and the queue capture and readmission in
    /// order; `capture_fails` makes the member's queue unobservable.
    #[derive(Default)]
    struct AbsentSessionBridge {
        core: super::super::core_test_members::CoreTestMembers,
        absent_runtime_id: std::sync::Mutex<Option<AgentRuntimeId>>,
        delivered_to: std::sync::Mutex<Vec<AgentRuntimeId>>,
        successors: AtomicUsize,
        queue_events: std::sync::Mutex<Vec<String>>,
        capture_fails: std::sync::atomic::AtomicBool,
    }

    impl AbsentSessionBridge {
        fn record(&self, event: String) {
            self.queue_events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event);
        }
    }

    #[async_trait::async_trait]
    impl SessionBridge for AbsentSessionBridge {
        async fn deliver_admitted(
            &self,
            runtime_id: &AgentRuntimeId,
            _delivery: super::super::bridge::BridgeDelivery,
        ) -> Result<SessionId, BridgeError> {
            self.delivered_to
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(runtime_id.clone());
            self.record("deliver".to_string());
            let absent = self
                .absent_runtime_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let identity = AgentIdentity::parse("absent")
                .map_err(|error| BridgeError::Mob(error.to_string()))?;
            let session_id = self
                .core
                .live_member(&identity)?
                .and_then(|binding| binding.session_id)
                .ok_or_else(|| BridgeError::Mob("no live session".to_string()))?;
            if absent.as_ref() == Some(runtime_id) {
                return Err(BridgeError::ResumeRejected {
                    kind: ResumeRejectionKind::Unavailable {
                        reason: Unavailable::Absent,
                    },
                    session_id,
                    detail: "no durable authority knows the session".to_string(),
                });
            }
            Ok(session_id)
        }

        async fn checkpoint_session(
            &self,
            _runtime_id: &AgentRuntimeId,
            _session_id: &SessionId,
        ) -> Result<super::super::types::SessionSnapshot, BridgeError> {
            Err(BridgeError::Mob(
                "checkpoint is not used in this test".to_string(),
            ))
        }

        async fn create_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<MemberRuntimeBinding, BridgeError> {
            Err(BridgeError::Mob(
                "create is not used in this test".to_string(),
            ))
        }

        async fn resume_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            _session_id: &SessionId,
            _resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            Err(BridgeError::Mob(
                "resume is not used in this test".to_string(),
            ))
        }

        async fn reset_member_to_successor(
            &self,
            identity: &AgentIdentity,
            spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<MemberRuntimeBinding, BridgeError> {
            let generation = self.successors.fetch_add(1, Ordering::SeqCst) as u64 + 1;
            let agent_runtime_id = super::super::core_test_bridge::bind_live(
                &self.core,
                identity,
                &spec.profile,
                generation,
                Some(SessionId::new()),
            )?;
            self.record("successor".to_string());
            Ok(MemberRuntimeBinding {
                session_id: self
                    .core
                    .live_member(identity)?
                    .and_then(|binding| binding.session_id),
                agent_runtime_id,
            })
        }

        async fn capture_queued_member_inputs(
            &self,
            _identity: &AgentIdentity,
            session_id: &SessionId,
        ) -> Result<super::super::bridge::QueuedMemberInputs, BridgeError> {
            if self.capture_fails.load(Ordering::SeqCst) {
                return Err(BridgeError::Mob(
                    "the runtime owner refused to serve".to_string(),
                ));
            }
            self.record(format!("capture {session_id}"));
            Ok(super::super::bridge::QueuedMemberInputs::default())
        }

        async fn readmit_queued_member_inputs(
            &self,
            _identity: &AgentIdentity,
            session_id: &SessionId,
            _inputs: super::super::bridge::QueuedMemberInputs,
        ) {
            self.record(format!("readmit {session_id}"));
        }

        async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            self.core.retire(runtime_id);
            Ok(())
        }

        async fn live_member(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn ensure_member_live(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
            self.core.live_member(identity)
        }

        async fn live_member_scope(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
            self.core.live_member_scope(identity)
        }

        async fn last_member_binding(
            &self,
            identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
            self.core.last_member_binding(identity)
        }

        async fn identity_management(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
            Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
        }

        async fn member_binding_history(
            &self,
        ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
            self.core.history()
        }
    }

    /// A delivery whose member's session Core holds no durable copy of is
    /// refused typed (`IdentitySessionAbsent`, retryable), never an internal
    /// error, and never a loop: the next delivery starts the identity's fresh
    /// life first (one successor), names the absent session as the replaced
    /// session, and lands on the fresh member.
    #[tokio::test]
    async fn a_delivery_to_an_absent_session_refuses_once_then_starts_the_fresh_life()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("absent")?;
        let bridge = Arc::new(AbsentSessionBridge::default());
        let first_session = SessionId::new();
        let first = super::super::core_test_bridge::bind_live(
            &bridge.core,
            &identity,
            &spec(&identity).profile,
            0,
            Some(first_session.clone()),
        )?;
        *bridge
            .absent_runtime_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(first.clone());
        let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "absent-session".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone() as Arc<dyn SessionBridge>),
            default_timeout: None,
        });
        runtime.register(spec(&identity)).await;

        let refused = match runtime
            .dispatch(&identity, &DispatchInput::system("first"))
            .await
        {
            Err(error) => error,
            Ok(admission) => panic!("the absent session must refuse, got {admission:?}"),
        };
        let IdentityRuntimeError::IdentitySessionAbsent { session_id, .. } = &refused else {
            panic!("the typed absent-session refusal, got {refused:?}");
        };
        assert_eq!(session_id, &first_session);
        assert_eq!(
            crate::rpc::identity_error_code(&refused),
            crate::rpc::IDENTITY_HISTORY_UNAVAILABLE_CODE
        );
        assert_eq!(bridge.successors.load(Ordering::SeqCst), 0);

        runtime
            .dispatch(&identity, &DispatchInput::system("second"))
            .await?;
        assert_eq!(
            bridge.successors.load(Ordering::SeqCst),
            1,
            "the second delivery starts exactly one fresh life"
        );
        let delivered_to = bridge
            .delivered_to
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(delivered_to.len(), 2, "{delivered_to:?}");
        assert_eq!(delivered_to[0], first);
        assert_ne!(
            delivered_to[1], first,
            "the second delivery lands on the fresh member"
        );
        let status = runtime.status(&identity).await?;
        assert_eq!(
            status.replaced_session,
            Some(ReplacedSession {
                session_id: first_session,
                reason: ReplacedSessionReason::Absent,
            })
        );
        assert_eq!(
            status.member_id.as_deref(),
            Some(crate::member_comms_id::roster_member_id_for_identity(identity.as_str()).as_str()),
            "the status names Core's member of the binding"
        );
        Ok(())
    }

    /// The absent-session fixture: `bridge`'s Core holds the identity's
    /// member live on a first session it has no durable copy of. Returns
    /// the registered runtime, the identity and that session.
    async fn absent_session_runtime(
        bridge: &Arc<AbsentSessionBridge>,
    ) -> Result<(IdentityRuntime, AgentIdentity, SessionId), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("absent")?;
        let first_session = SessionId::new();
        let first = super::super::core_test_bridge::bind_live(
            &bridge.core,
            &identity,
            &spec(&identity).profile,
            0,
            Some(first_session.clone()),
        )?;
        *bridge
            .absent_runtime_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(first);
        let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "absent-session-queue".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone() as Arc<dyn SessionBridge>),
            default_timeout: None,
        });
        runtime.register(spec(&identity)).await;
        Ok((runtime, identity, first_session))
    }

    /// The wedged member an absent session leaves live may hold inputs
    /// other senders queued behind it. The fresh life captures that queue
    /// from the absent session before Core's successor respawn and admits it
    /// to the fresh member's session as soon as the successor is committed,
    /// before the delivery that started the fresh life.
    #[tokio::test]
    async fn the_fresh_life_after_an_absent_session_carries_the_wedged_members_queue()
    -> Result<(), Box<dyn std::error::Error>> {
        let bridge = Arc::new(AbsentSessionBridge::default());
        let (runtime, identity, first_session) = absent_session_runtime(&bridge).await?;

        assert!(
            runtime
                .dispatch(&identity, &DispatchInput::system("first"))
                .await
                .is_err(),
            "the first delivery refuses the absent session"
        );
        runtime
            .dispatch(&identity, &DispatchInput::system("second"))
            .await?;

        let fresh_session = bridge
            .core
            .live_member(&identity)?
            .and_then(|binding| binding.session_id)
            .ok_or("the fresh member has a session")?;
        assert_ne!(fresh_session, first_session);
        let events = bridge
            .queue_events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            events,
            vec![
                "deliver".to_string(),
                format!("capture {first_session}"),
                "successor".to_string(),
                format!("readmit {fresh_session}"),
                "deliver".to_string(),
            ],
            "the queue is captured before the successor and admitted to it first"
        );
        Ok(())
    }

    /// A wedged member whose queue cannot be observed is not replaced: the
    /// fresh life is deferred and the delivery refused retryable, so no input
    /// another sender queued is destroyed unseen. Nothing is delivered.
    #[tokio::test]
    async fn an_unobservable_queue_defers_the_fresh_life_after_an_absent_session()
    -> Result<(), Box<dyn std::error::Error>> {
        let bridge = Arc::new(AbsentSessionBridge::default());
        bridge.capture_fails.store(true, Ordering::SeqCst);
        let (runtime, identity, first_session) = absent_session_runtime(&bridge).await?;

        assert!(
            runtime
                .dispatch(&identity, &DispatchInput::system("first"))
                .await
                .is_err(),
            "the first delivery refuses the absent session"
        );
        let deferred = match runtime
            .dispatch(&identity, &DispatchInput::system("second"))
            .await
        {
            Err(error) => error,
            Ok(admission) => panic!("the fresh life must be deferred, got {admission:?}"),
        };
        assert!(
            matches!(
                &deferred,
                IdentityRuntimeError::IdentityWakeRetryable { session_id, .. }
                    if session_id.as_ref() == Some(&first_session)
            ),
            "{deferred:?}"
        );
        assert_eq!(
            crate::rpc::identity_error_code(&deferred),
            crate::rpc::IDENTITY_HISTORY_UNAVAILABLE_CODE
        );
        assert_eq!(
            bridge.successors.load(Ordering::SeqCst),
            0,
            "the member is not replaced"
        );
        assert_eq!(
            bridge
                .core
                .live_member(&identity)?
                .and_then(|binding| binding.session_id),
            Some(first_session),
            "the wedged member stays with its queue"
        );
        assert_eq!(
            bridge
                .delivered_to
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1,
            "only the first, refused delivery reached Core"
        );
        Ok(())
    }

    /// The typed remedy for a durable identity placed on a host whose
    /// realm backend is not persistent: the wake answers `HostRejectedBuild`
    /// naming the remedy, on every attempt (one spawn each, no retry loop),
    /// and records nothing.
    #[tokio::test]
    async fn a_host_without_a_persistent_realm_refuses_the_wake_typed_with_its_remedy()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("placed")?;
        let bridge = Arc::new(NonPersistentHostBridge::default());
        let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "non-persistent-host".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone() as Arc<dyn SessionBridge>),
            default_timeout: None,
        });
        runtime.register(spec(&identity)).await;

        for attempt in 1..=2 {
            let error = match runtime.materialize(&identity).await {
                Err(error) => error,
                Ok(scope) => panic!("attempt {attempt}: the wake must refuse, got {scope:?}"),
            };
            let IdentityRuntimeError::HostRejectedBuild {
                identity: refused,
                reason,
            } = &error
            else {
                panic!("attempt {attempt}: the typed build refusal, got {error:?}");
            };
            assert_eq!(refused, &identity, "attempt {attempt}");
            assert!(
                reason.contains("the realm backend is not persistent"),
                "attempt {attempt}: Core's refusal is kept: {reason}"
            );
            assert!(
                error.to_string().contains(
                    "run that host with a persistent realm backend, or remove the identity's \
                     placement"
                ),
                "attempt {attempt}: the answer names the remedy: {error}"
            );
            assert_eq!(
                bridge.creates.load(Ordering::SeqCst),
                attempt,
                "attempt {attempt}: one spawn per wake, no retry loop"
            );
        }
        let status = runtime.status(&identity).await?;
        assert_eq!(
            status.wake_refusal, None,
            "the build refusal is answered on every attempt, never recorded"
        );

        // No supervisor retries the identity: none runs any more. The only
        // automatic re-attempt is best-effort hydration, which records the
        // refusal and backs off for MATERIALIZATION_FAILURE_BACKOFF.
        assert!(runtime.materialize_all().await?.is_empty());
        assert_eq!(
            bridge.creates.load(Ordering::SeqCst),
            3,
            "the first hydration pass attempts the spawn once"
        );
        assert!(runtime.materialize_all().await?.is_empty());
        assert_eq!(
            bridge.creates.load(Ordering::SeqCst),
            3,
            "the backoff holds the next pass inside its window"
        );
        Ok(())
    }

    /// The typed remedy for a durable profile on a mob whose member sessions
    /// do not outlive the process: the wake answers `HostRejectedBuild`
    /// naming the remedy, on every attempt (one spawn each, no retry loop),
    /// and records nothing.
    #[tokio::test]
    async fn a_durable_profile_without_persistent_sessions_refuses_the_wake_typed_with_its_remedy()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("durable")?;
        let bridge = Arc::new(NonPersistentHostBridge {
            durable_profile: true,
            ..NonPersistentHostBridge::default()
        });
        let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "non-durable-service".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone() as Arc<dyn SessionBridge>),
            default_timeout: None,
        });
        runtime.register(spec(&identity)).await;

        for attempt in 1..=2 {
            let Err(error) = runtime.materialize(&identity).await else {
                return Err(format!("attempt {attempt}: the wake must refuse").into());
            };
            let IdentityRuntimeError::HostRejectedBuild {
                identity: refused,
                reason,
            } = &error
            else {
                return Err(
                    format!("attempt {attempt}: the typed build refusal, got {error:?}").into(),
                );
            };
            assert_eq!(refused, &identity, "attempt {attempt}");
            assert!(
                reason.contains("member sessions do not outlive the process"),
                "attempt {attempt}: Core's refusal is kept: {reason}"
            );
            assert!(
                error.to_string().contains(
                    "run the mob on a persistent realm, or drop durable from the profile"
                ),
                "attempt {attempt}: the answer names the remedy: {error}"
            );
            assert_eq!(
                bridge.creates.load(Ordering::SeqCst),
                attempt,
                "attempt {attempt}: one spawn per wake, no retry loop"
            );
        }
        let status = runtime.status(&identity).await?;
        assert_eq!(
            status.wake_refusal, None,
            "the build refusal is answered on every attempt, never recorded"
        );
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod respawn_window_tests {
    //! The respawn window: Core retires the member and then wakes it
    //! onto the same session in a second step, and a status read between the
    //! two reads the retired binding. It may show Dormant; it never shows
    //! Broken or a replaced session, even when an earlier wake left either
    //! note on the member.
    use super::*;

    use super::super::bridge::{MemberRuntimeBinding, ResumeSessionOutcome};
    use super::super::local_lease::LocalLeaseProvider;
    use super::super::local_store::LocalContinuityStore;
    use super::super::types::{
        ReplacedSession, ReplacedSessionReason, WakeRefusal, WakeRefusalRemedy,
    };

    const PROFILE: &str = "worker";

    fn spec(identity: &AgentIdentity) -> DurableAgentSpec {
        DurableAgentSpec {
            identity: identity.clone(),
            profile: meerkat_mob::ProfileName::from(PROFILE),
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
        }
    }

    /// A Core that holds the member live on one session, retires it on
    /// request, and holds the resume that follows until the test opens the
    /// gate.
    struct GatedRespawnBridge {
        identity: AgentIdentity,
        session: SessionId,
        live: AtomicBool,
        generation: AtomicU64,
        resume_entered: tokio::sync::Notify,
        resume_gate: tokio::sync::Notify,
    }

    impl GatedRespawnBridge {
        fn new(identity: AgentIdentity, session: SessionId) -> Self {
            Self {
                identity,
                session,
                live: AtomicBool::new(true),
                generation: AtomicU64::new(3),
                resume_entered: tokio::sync::Notify::new(),
                resume_gate: tokio::sync::Notify::new(),
            }
        }

        fn binding(&self) -> meerkat_mob::MemberBinding {
            let state = if self.live.load(Ordering::SeqCst) {
                meerkat_mob::MemberBindingState::Live
            } else {
                meerkat_mob::MemberBindingState::Retired
            };
            meerkat_mob::MemberBinding::for_test(
                crate::member_comms_id::roster_member_id_for_identity(self.identity.as_str()),
                meerkat_mob::Generation::new(self.generation.load(Ordering::SeqCst)),
                Some(self.session.clone()),
                state,
                meerkat_mob::ProfileName::from(PROFILE),
                meerkat_mob::SpawnContinuityIntent::DurableIdentity {
                    continuity_key: self.identity.as_str().to_string(),
                },
                meerkat_mob::MemberIdentityManagement::CommandDriven,
            )
        }

        fn runtime_binding(&self) -> Result<MemberRuntimeBinding, BridgeError> {
            Ok(MemberRuntimeBinding {
                agent_runtime_id: AgentRuntimeId::parse(&format!(
                    "rt:{}:{}",
                    self.identity.as_str(),
                    self.generation.load(Ordering::SeqCst)
                ))
                .map_err(|error| BridgeError::InvalidInput(error.to_string()))?,
                session_id: Some(self.session.clone()),
            })
        }
    }

    #[async_trait::async_trait]
    impl SessionBridge for GatedRespawnBridge {
        async fn deliver_admitted(
            &self,
            _runtime_id: &AgentRuntimeId,
            _delivery: super::super::bridge::BridgeDelivery,
        ) -> Result<SessionId, BridgeError> {
            Err(BridgeError::Mob(
                "deliver is not used in this test".to_string(),
            ))
        }

        async fn checkpoint_session(
            &self,
            _runtime_id: &AgentRuntimeId,
            _session_id: &SessionId,
        ) -> Result<super::super::types::SessionSnapshot, BridgeError> {
            Err(BridgeError::Mob(
                "checkpoint is not used in this test".to_string(),
            ))
        }

        async fn create_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<MemberRuntimeBinding, BridgeError> {
            Err(BridgeError::InvalidInput(
                "a respawn resumes the member's session; it never creates one".to_string(),
            ))
        }

        async fn resume_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            _session_id: &SessionId,
            _resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            self.resume_entered.notify_one();
            self.resume_gate.notified().await;
            self.generation.fetch_add(1, Ordering::SeqCst);
            self.live.store(true, Ordering::SeqCst);
            Ok(ResumeSessionOutcome::Resumed {
                session_id: self.session.clone(),
                agent_runtime_id: self.runtime_binding()?.agent_runtime_id,
            })
        }

        async fn retire_member(&self, _runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            self.live.store(false, Ordering::SeqCst);
            Ok(())
        }

        async fn live_member(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
            if self.live.load(Ordering::SeqCst) {
                self.runtime_binding().map(Some)
            } else {
                Ok(None)
            }
        }

        async fn live_member_scope(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
            Ok(None)
        }

        async fn last_member_binding(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
            Ok(Some(self.binding()))
        }

        async fn identity_management(
            &self,
            _identity: &AgentIdentity,
        ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
            Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
        }

        async fn member_binding_history(
            &self,
        ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
            let binding = self.binding();
            Ok(meerkat_mob::MemberBindingHistory::for_test(
                [binding.clone()],
                [binding],
                [],
                1,
            ))
        }
    }

    #[tokio::test]
    async fn a_status_read_between_the_respawn_steps_is_dormant_never_broken()
    -> Result<(), Box<dyn std::error::Error>> {
        let identity = AgentIdentity::parse("keeper")?;
        let session = SessionId::new();
        let bridge = Arc::new(GatedRespawnBridge::new(identity.clone(), session.clone()));
        let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "respawn-window".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone() as Arc<dyn SessionBridge>),
            default_timeout: None,
        }));
        runtime.register(spec(&identity)).await;
        // Notes an earlier wake left on a member Core now holds live: a final
        // refusal and a replaced session. Neither describes the live member.
        {
            let mut entries = runtime.entries.write().await;
            let entry = entries
                .get_mut(&identity)
                .ok_or("the identity is registered")?;
            entry.wake_refusal = Some(WakeRefusal {
                code: crate::rpc::IDENTITY_UNRESUMABLE_CODE,
                session_id: session.clone(),
                remedy: WakeRefusalRemedy::DeleteIdentity,
            });
            entry.replaced_session = Some(ReplacedSession {
                session_id: SessionId::new(),
                reason: ReplacedSessionReason::Absent,
            });
        }
        assert_eq!(
            runtime.status(&identity).await?.state,
            IdentityLifecycleState::Active
        );

        let respawn = tokio::spawn({
            let runtime = Arc::clone(&runtime);
            let identity = identity.clone();
            async move { runtime.respawn(&identity).await }
        });
        bridge.resume_entered.notified().await;

        let between = runtime.status(&identity).await?;
        assert_eq!(
            between.state,
            IdentityLifecycleState::Dormant,
            "between Core's retire and the wake the member reads Dormant, never Broken"
        );
        assert_eq!(
            between.wake_refusal, None,
            "no refusal describes the member"
        );
        assert_eq!(
            between.replaced_session, None,
            "the respawn replaced no session"
        );
        assert_eq!(
            between.session_id,
            Some(session.clone()),
            "the read names the session the respawn wakes"
        );

        bridge.resume_gate.notify_one();
        respawn.await??;
        let after = runtime.status(&identity).await?;
        assert_eq!(after.state, IdentityLifecycleState::Active);
        assert_eq!(after.session_id, Some(session));
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod history_read_fault_tests {
    //! No default-healthy status: a view of an identity reads Core's
    //! member history at request time or fails typed. A failed read never
    //! yields an empty list, a guessed status or a healthy default.
    use super::*;

    use super::super::bridge::{MemberRuntimeBinding, ResumeSessionOutcome};
    use super::super::local_lease::LocalLeaseProvider;
    use super::super::local_store::LocalContinuityStore;

    /// A bridge over a Core whose member history cannot be read.
    struct UnreadableHistoryBridge;

    #[async_trait::async_trait]
    impl SessionBridge for UnreadableHistoryBridge {
        async fn deliver_admitted(
            &self,
            _runtime_id: &AgentRuntimeId,
            _delivery: super::super::bridge::BridgeDelivery,
        ) -> Result<SessionId, BridgeError> {
            Err(BridgeError::Mob(
                "deliver is not used in this test".to_string(),
            ))
        }

        async fn checkpoint_session(
            &self,
            _runtime_id: &AgentRuntimeId,
            _session_id: &SessionId,
        ) -> Result<super::super::types::SessionSnapshot, BridgeError> {
            Err(BridgeError::Mob(
                "checkpoint is not used in this test".to_string(),
            ))
        }

        async fn create_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
        ) -> Result<MemberRuntimeBinding, BridgeError> {
            Err(BridgeError::Mob(
                "no member is created in this test".to_string(),
            ))
        }

        async fn resume_session(
            &self,
            _identity: &AgentIdentity,
            _spec: &DurableAgentSpec,
            _draft: &AgentBuildDraft,
            _session_id: &SessionId,
            _resume_from_role: Option<&meerkat_mob::ProfileName>,
        ) -> Result<ResumeSessionOutcome, BridgeError> {
            Err(BridgeError::Mob(
                "no member is resumed in this test".to_string(),
            ))
        }

        async fn retire_member(&self, _runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
            Ok(())
        }

        async fn member_binding_history(
            &self,
        ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
            Err(BridgeError::Mob(
                "the mob journal could not be read".to_string(),
            ))
        }
    }

    fn spec(identity: &AgentIdentity) -> DurableAgentSpec {
        DurableAgentSpec {
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
        }
    }

    #[tokio::test]
    async fn an_unreadable_member_history_fails_every_view_typed()
    -> Result<(), Box<dyn std::error::Error>> {
        let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "unreadable-history".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(UnreadableHistoryBridge) as Arc<dyn SessionBridge>),
            default_timeout: None,
        });
        let keeper = AgentIdentity::parse("keeper")?;
        runtime.register(spec(&keeper)).await;

        let error = match runtime.statuses().await {
            Err(error) => error,
            Ok(statuses) => panic!("a failed history read must not list statuses: {statuses:?}"),
        };
        assert!(
            matches!(error, IdentityRuntimeError::MemberHistoryUnavailable { .. }),
            "{error:?}"
        );
        assert_eq!(
            crate::rpc::identity_error_code(&error),
            crate::rpc::IDENTITY_HISTORY_UNAVAILABLE_CODE
        );
        assert_eq!(
            error.structured_data(),
            Some(serde_json::json!({
                "kind": "identity_history_unavailable", "retryable": true,
            }))
        );
        assert!(
            matches!(
                runtime.roster_inspect().await,
                Err(IdentityRuntimeError::MemberHistoryUnavailable { .. })
            ),
            "roster_inspect must fail typed, not answer an empty roster"
        );
        assert!(
            matches!(
                runtime.status(&keeper).await,
                Err(IdentityRuntimeError::IdentityHistoryUnavailable { .. })
            ),
            "status must fail typed, not report a default state"
        );
        assert!(
            matches!(
                runtime.member_health(&keeper).await,
                Err(IdentityRuntimeError::IdentityHistoryUnavailable { .. })
            ),
            "member_health must fail typed, not report a healthy member"
        );
        Ok(())
    }
}

/// The tracked (owned) operations of an exported identity runtime. Each one
/// takes its own public owner for the operation's whole life (see
/// `IdentityRuntime::run_tracked_foreground`); the view keeps the mob owned
/// in between. The view never hands out the inner `Arc`.
impl super::PublicIdentityRuntime {
    /// A keyed send to `identity` in this view's mob: `content` committed as
    /// a member continuation under the caller's `idempotency_key`, answered
    /// with the continuation's receipt and current delivery status. A replay
    /// answers the first receipt; other content under the key is refused.
    /// The content runs after the member's current turn (`Queue`); see
    /// [`Self::send_keyed_with_handling_mode`] for a steer.
    pub async fn send_keyed(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        idempotency_key: &str,
    ) -> Result<meerkat_contracts::WireContinuationReceipt, IdentityRuntimeError> {
        self.send_keyed_with_handling_mode(
            identity,
            expected_alias,
            content,
            HandlingMode::Queue,
            idempotency_key,
        )
        .await
    }

    /// [`Self::send_keyed`] with the caller's `handling_mode`, committed with
    /// the content: `Steer` delivers it into the member's running turn at
    /// its next boundary, `Queue` after that turn, and either starts a turn
    /// on an idle member. The receipt is Core's, as for `send_keyed`. The
    /// mode is part of what the key commits: a replay in the same mode
    /// answers the first receipt and never delivers twice, and a replay in
    /// another mode is refused as other content is.
    pub async fn send_keyed_with_handling_mode(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        idempotency_key: &str,
    ) -> Result<meerkat_contracts::WireContinuationReceipt, IdentityRuntimeError> {
        self.runtime
            .send_keyed_tracked(
                &self.owner,
                identity,
                expected_alias,
                content,
                handling_mode,
                idempotency_key,
            )
            .await
    }

    /// The state of an input in a session `identity` has had, as Core's
    /// `session/input_status` reports it; any other session is refused.
    pub async fn send_status(
        &self,
        identity: &AgentIdentity,
        session_id: &SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<meerkat_contracts::SessionInputStateResult, IdentityRuntimeError> {
        self.runtime
            .send_status(&self.owner, identity, session_id, input_id)
            .await
    }

    pub async fn send_expecting_registered_spec(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        track_turn: bool,
        expected_spec_digest: &str,
    ) -> Result<(SendAdmission, Option<TurnTracking>), IdentityRuntimeError> {
        self.runtime
            .send_expecting_registered_spec(
                identity,
                expected_alias,
                content,
                handling_mode,
                track_turn,
                expected_spec_digest,
            )
            .await
    }

    pub async fn materialize_tracked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        self.runtime.materialize_tracked(identity).await
    }

    pub async fn retire_tracked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<(), IdentityRuntimeError> {
        self.runtime.retire_tracked(identity).await
    }

    pub async fn retire_member_alias_tracked(
        &self,
        identity: &AgentIdentity,
        expected_alias: &str,
    ) -> Result<(), IdentityRuntimeError> {
        self.runtime
            .retire_member_alias_tracked(identity, expected_alias)
            .await
    }

    /// See [`IdentityRuntime::retire_at_scope_tracked`].
    pub async fn retire_at_scope_tracked(
        &self,
        identity: &AgentIdentity,
        scope: &super::DeliveryScope,
    ) -> Result<FencingToken, super::ScopedRetireError> {
        self.runtime.retire_at_scope_tracked(identity, scope).await
    }

    pub async fn respawn_tracked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        self.runtime.respawn_tracked(identity).await
    }

    pub async fn respawn_member_alias_tracked(
        &self,
        identity: &AgentIdentity,
        expected_alias: &str,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        self.runtime
            .respawn_member_alias_tracked(identity, expected_alias)
            .await
    }

    pub async fn reset_tracked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberBinding, IdentityRuntimeError> {
        self.runtime.reset_tracked(identity).await
    }

    pub async fn reset_member_alias_tracked(
        &self,
        identity: &AgentIdentity,
        expected_alias: &str,
    ) -> Result<meerkat_mob::MemberBinding, IdentityRuntimeError> {
        self.runtime
            .reset_member_alias_tracked(identity, expected_alias)
            .await
    }

    pub async fn delete_identity_tracked(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, IdentityRuntimeError> {
        self.runtime.delete_identity_tracked(identity).await
    }

    pub async fn delete_identity_member_alias_tracked(
        &self,
        identity: &AgentIdentity,
        expected_alias: &str,
    ) -> Result<Option<meerkat_mob::MemberBinding>, IdentityRuntimeError> {
        self.runtime
            .delete_identity_member_alias_tracked(identity, expected_alias)
            .await
    }

    pub async fn materialize_all_required_tracked(
        &self,
    ) -> Result<Vec<meerkat_mob::MemberDeliveryScope>, IdentityRuntimeError> {
        self.runtime.materialize_all_required_tracked().await
    }

    pub async fn send_tracked(
        &self,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.runtime.send_tracked(identity, content).await
    }

    pub async fn send_admission_tracked(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<SendAdmission, IdentityRuntimeError> {
        self.runtime
            .send_admission_tracked(
                identity,
                expected_alias,
                content,
                handling_mode,
                interaction_id,
            )
            .await
    }

    pub async fn send_with_turn_ticket(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<Ticketed<SendAdmission>, IdentityRuntimeError> {
        self.runtime
            .send_with_turn_ticket(
                identity,
                expected_alias,
                content,
                handling_mode,
                interaction_id,
            )
            .await
    }

    pub async fn send_governed_with_turn_ticket(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        input: meerkat_runtime::Input,
        binder: Arc<dyn super::bridge::GovernedInputBinder>,
    ) -> Result<GovernedSendOutcome, IdentityRuntimeError> {
        self.runtime
            .send_governed_with_turn_ticket(identity, expected_alias, input, binder)
            .await
    }

    /// The trusted host's own retained, already-bound input to a member it
    /// composed, through the internal door (see
    /// [`IdentityRuntime::dispatch_governed_with_turn_ticket`]). This facade
    /// is the host's existing trusted boundary, not caller authentication:
    /// the host checks the retained task's identity, alias and binder at its
    /// own call site. External callers use
    /// [`Self::send_governed_with_turn_ticket`].
    pub async fn dispatch_governed_with_turn_ticket(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        input: meerkat_runtime::Input,
        binder: Arc<dyn super::bridge::GovernedInputBinder>,
    ) -> Result<GovernedSendOutcome, IdentityRuntimeError> {
        self.runtime
            .dispatch_governed_with_turn_ticket(identity, expected_alias, input, binder)
            .await
    }

    pub async fn send_with_mode_tracked(
        &self,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.runtime
            .send_with_mode_tracked(identity, content, handling_mode)
            .await
    }

    pub async fn send_with_mode_and_interaction_tracked(
        &self,
        identity: &AgentIdentity,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.runtime
            .send_with_mode_and_interaction_tracked(
                identity,
                content,
                handling_mode,
                interaction_id,
            )
            .await
    }

    pub async fn send_with_mode_and_interaction_member_alias_tracked(
        &self,
        identity: &AgentIdentity,
        expected_alias: &str,
        content: &meerkat_core::ContentInput,
        handling_mode: HandlingMode,
        interaction_id: Option<&str>,
    ) -> Result<FencingToken, IdentityRuntimeError> {
        self.runtime
            .send_with_mode_and_interaction_member_alias_tracked(
                identity,
                expected_alias,
                content,
                handling_mode,
                interaction_id,
            )
            .await
    }

    pub async fn dispatch_tracked(
        &self,
        identity: &AgentIdentity,
        input: &DispatchInput,
    ) -> Result<(FencingToken, bool), IdentityRuntimeError> {
        self.runtime.dispatch_tracked(identity, input).await
    }

    pub async fn dispatch_member_alias_tracked(
        &self,
        identity: &AgentIdentity,
        expected_alias: &str,
        input: &DispatchInput,
    ) -> Result<(FencingToken, bool), IdentityRuntimeError> {
        self.runtime
            .dispatch_member_alias_tracked(identity, expected_alias, input)
            .await
    }

    pub async fn dispatch_admission_tracked(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        input: &DispatchInput,
    ) -> Result<DispatchAdmission, IdentityRuntimeError> {
        self.runtime
            .dispatch_admission_tracked(identity, expected_alias, input)
            .await
    }

    pub async fn dispatch_with_turn_ticket(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
        input: &DispatchInput,
    ) -> Result<Ticketed<DispatchAdmission>, IdentityRuntimeError> {
        self.runtime
            .dispatch_with_turn_ticket(identity, expected_alias, input)
            .await
    }

    pub async fn reload_member_alias_tracked(
        &self,
        identity: &AgentIdentity,
        expected_alias: Option<&str>,
    ) -> Result<MemberReloadOutcome, IdentityRuntimeError> {
        self.runtime
            .reload_member_alias_tracked(identity, expected_alias)
            .await
    }
}

/// The tracked operations of an exported identity-first context (see
/// [`super::PublicIdentityRuntime`]).
impl super::PublicIdentityFirstContext {
    pub async fn refresh_desired_topology_tracked(
        &self,
    ) -> Result<super::orchestrator::RestoreFlowResult, IdentityRuntimeError> {
        self.context.refresh_desired_topology_tracked().await
    }

    /// See [`IdentityFirstRuntimeContext::add_member_held`].
    pub async fn add_member_held<F, Fut>(
        &self,
        spec: DurableAgentSpec,
        deadline: meerkat_mob::HeldSpawnDeadline,
        complete: F,
    ) -> Result<super::held_bootstrap::HeldBootstrapStart, IdentityRuntimeError>
    where
        F: FnOnce(
                Vec<super::held_bootstrap::HeldIdentityMember>,
                super::held_bootstrap::HeldCompletion,
            ) -> Fut
            + Send
            + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
    {
        self.context.add_member_held(spec, deadline, complete).await
    }
}

tokio::task_local! {
    /// The public owner the current tracked foreground operation holds
    /// (D67), visible to work it spawns on its behalf.
    static FOREGROUND_OWNER: Option<meerkat_mob::MobHandle>;
}

/// Where a tracked foreground operation's public owner comes from (D67).
#[allow(
    clippy::large_enum_variant,
    reason = "a short-lived argument; boxing the handle would only add an allocation per operation"
)]
pub(crate) enum ForegroundOwnerSource {
    /// Started through an owner-bearing entry point: acquire an owner while
    /// the mob's admission is open, or be refused.
    Acquire,
    /// Spawned on behalf of owned work (or running inside admitted native
    /// work): carry that owner, never acquire.
    Carried(Option<meerkat_mob::MobHandle>),
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod candidate_deferral_tests {
    use super::super::local_lease::LocalLeaseProvider;
    use super::super::local_store::LocalContinuityStore;
    use super::*;

    /// A runtime over `state_dir`'s first-boot manifest, a candidate launch's
    /// when `candidate` is set.
    async fn first_boot_runtime(state_dir: &std::path::Path, candidate: bool) -> IdentityRuntime {
        let runtime = IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("store")),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "candidate-first-boot".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: None,
            default_timeout: None,
        });
        runtime.set_first_boot_state_dir(Some(state_dir.to_path_buf()));
        if candidate {
            runtime.mark_candidate_launch();
        }
        runtime
            .load_first_boot_manifest()
            .await
            .expect("the manifest reads");
        runtime
    }

    /// A candidate launch settles a first-boot row (one Core's history shows
    /// done; it carries out none) in its own process only: the row leaves
    /// its pending rows, and the manifest file stays byte for byte as it
    /// found it, for the authoritative launch to rewrite. Control: an
    /// authoritative launch that settles the same row removes the file.
    #[tokio::test]
    async fn a_candidate_launch_settles_a_first_boot_row_without_rewriting_the_manifest() {
        let state_dir = tempfile::tempdir().expect("state dir");
        let identity = AgentIdentity::parse("personal:alice").expect("identity");
        FirstBootManifest::write_new(
            state_dir.path(),
            vec![FirstBootRow::Resume {
                identity: identity.clone(),
                session: meerkat_core::types::SessionId::new(),
            }],
        )
        .expect("storage-migrate-native writes the manifest");
        let path = FirstBootManifest::path(state_dir.path());
        let written = std::fs::read(&path).expect("read the manifest");

        let candidate = first_boot_runtime(state_dir.path(), true).await;
        candidate.settle_first_boot_row(&identity).await;
        assert_eq!(
            (
                candidate
                    .identity_bootstrap_status()
                    .first_boot_pending
                    .len(),
                std::fs::read(&path).ok(),
            ),
            (0, Some(written)),
            "the candidate settles the row in process and leaves the file"
        );

        let authoritative = first_boot_runtime(state_dir.path(), false).await;
        authoritative.settle_first_boot_row(&identity).await;
        assert!(!path.exists(), "the authoritative launch removes the file");
    }

    /// Only a candidate launch's refusal of a roster change defers an
    /// identity, with the reason the refusal names. Every other error stays
    /// the identity's own failure, a store error included.
    #[test]
    fn only_a_candidate_roster_refusal_defers_an_identity() {
        let identity = AgentIdentity::parse("agent:deferred").expect("identity");
        let invalid = |operation: &'static str| IdentityRuntimeError::InvalidState {
            identity: identity.clone(),
            state: IdentityLifecycleState::Dormant,
            operation,
        };
        let first_boot = |operation: &'static str| IdentityRuntimeError::FirstBootRowPending {
            identity: identity.clone(),
            kind: FirstBootRowKind::Fresh,
            waiting_for: FirstBootWait::CarryOut,
            operation,
        };
        assert_eq!(
            [
                candidate_deferral_of(&invalid(CANDIDATE_CREATE_MEMBER)),
                candidate_deferral_of(&invalid(CANDIDATE_MOVE_ROLE)),
                candidate_deferral_of(&invalid(CANDIDATE_FRESH_SESSION)),
                candidate_deferral_of(&first_boot(CANDIDATE_FIRST_BOOT_FRESH)),
                candidate_deferral_of(&invalid(CANDIDATE_RESUME_MEMBER)),
            ],
            [
                Some(DeferredIdentityReason::CreateMember),
                Some(DeferredIdentityReason::MoveRole),
                Some(DeferredIdentityReason::FreshSession),
                Some(DeferredIdentityReason::FirstBootFresh),
                Some(DeferredIdentityReason::ResumeMember),
            ]
        );
        assert_eq!(
            [
                candidate_deferral_of(&invalid("send")),
                candidate_deferral_of(&first_boot("wake")),
                candidate_deferral_of(&IdentityRuntimeError::Internal(
                    "candidate launch: whether Core holds the stored session could not be read"
                        .to_string()
                )),
                candidate_deferral_of(&IdentityRuntimeError::UnknownIdentity(identity.clone())),
            ],
            [None, None, None, None]
        );
    }
}
