use std::sync::Arc;

use meerkat_core::service::DurableResumeHold;
use serde::{Deserialize, Serialize};
use std::fmt;

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Error returned when an identity string fails validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidIdentity {
    pub input: String,
    pub reason: String,
}

impl fmt::Display for InvalidIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid identity {:?}: {}", self.input, self.reason)
    }
}

impl std::error::Error for InvalidIdentity {}

fn validate_identity_string(s: &str) -> Result<(), InvalidIdentity> {
    if s.is_empty() {
        return Err(InvalidIdentity {
            input: s.to_string(),
            reason: "must not be empty".to_string(),
        });
    }
    if s.contains(char::is_whitespace) {
        return Err(InvalidIdentity {
            input: s.to_string(),
            reason: "must not contain whitespace".to_string(),
        });
    }
    if s.contains('/') {
        return Err(InvalidIdentity {
            input: s.to_string(),
            reason: "must not contain slashes".to_string(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Macro for validated string newtypes (AgentIdentity, AgentRuntimeId)
// ---------------------------------------------------------------------------

macro_rules! validated_string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Parse and validate a string into this type.
            ///
            /// # Errors
            ///
            /// Returns `InvalidIdentity` if the input is empty, contains whitespace,
            /// or contains slashes.
            pub fn parse(s: &str) -> Result<Self, InvalidIdentity> {
                validate_identity_string(s)?;
                Ok(Self(s.to_string()))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let s = String::deserialize(deserializer)?;
                Self::parse(&s).map_err(serde::de::Error::custom)
            }
        }
    };
}

validated_string_newtype!(
    /// The primary app-facing identity handle for all Meta control-plane operations.
    AgentIdentity
);

validated_string_newtype!(
    /// Internal runtime-level ID minted at first-create.
    AgentRuntimeId
);

// ---------------------------------------------------------------------------
// AgentAddressability
// ---------------------------------------------------------------------------

/// Whether an agent accepts `send()` (addressable) or only `dispatch()` (internal-only).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentAddressability {
    #[default]
    Addressable,
    InternalOnly,
}

// ---------------------------------------------------------------------------
// DisplayName
// ---------------------------------------------------------------------------

/// Human-facing display name. Non-empty.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DisplayName(String);

impl DisplayName {
    /// # Errors
    ///
    /// Returns `InvalidIdentity` if the input is empty.
    pub fn parse(s: &str) -> Result<Self, InvalidIdentity> {
        if s.is_empty() {
            return Err(InvalidIdentity {
                input: s.to_string(),
                reason: "display name must not be empty".to_string(),
            });
        }
        Ok(Self(s.to_string()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DisplayName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for DisplayName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DisplayName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Monotonic u64 newtypes
// ---------------------------------------------------------------------------

macro_rules! monotonic_u64_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            #[must_use]
            pub const fn new(value: u64) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

monotonic_u64_newtype!(
    /// Monotonic generation counter for continuity. Starts at 0, incremented by `reset()`.
    ContinuityGeneration
);

monotonic_u64_newtype!(
    /// Monotonic checkpoint counter scoped to `(AgentIdentity, ContinuityGeneration)`.
    CheckpointVersion
);

monotonic_u64_newtype!(
    /// Monotonic fence token. The identity runtime fills it with Core's
    /// fence of an identity's member incarnation; the `LeaseProvider`
    /// that also mints it is no longer read.
    FencingToken
);

// ---------------------------------------------------------------------------
// CompletionCursor — correlated turn-completion identity
// ---------------------------------------------------------------------------

/// Comparable completion identity for one identity's stream of turns.
///
/// Waiting for an agent's next answer must never compare output TEXT. Two
/// consecutive turns can legitimately produce byte-identical output (`ACK`
/// twice is the production case that produced a phantom 962-second turn), and
/// a text comparison then reports "no new turn" for the whole configured wait.
/// This cursor is the comparable atom instead. It is never derived from output
/// content, a content hash, a wall-clock timestamp, or a uuid regenerated per
/// poll.
///
/// `epoch` is Core's fence of the identity's member incarnation, as a
/// [`FencingToken`]. Core restores it across a process restart while this
/// process counts turns from zero, so [`Self::progress_since`] reads a
/// baseline that counts more turns than the cursor under the same epoch as
/// another incarnation. `turns` counts turns observed as completed within
/// that incarnation.
///
/// Ordering is lexicographic (`epoch`, then `turns`). Turn counts are NOT
/// comparable across incarnations, which is why callers classify with
/// [`Self::progress_since`] rather than a bare `>`: an incarnation change is
/// reported, not silently read as progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CompletionCursor {
    /// Core's fence of the member incarnation this count belongs to
    /// (`0` while Core holds no session-backed member).
    pub epoch: FencingToken,
    /// Turns observed as completed within `epoch`.
    pub turns: u64,
}

impl Default for CompletionCursor {
    /// The cursor while Core holds no session-backed member: epoch 0, no
    /// completed turns.
    fn default() -> Self {
        Self::start(FencingToken::new(0))
    }
}

impl CompletionCursor {
    /// The zero cursor for `epoch`: no completed turn observed yet.
    #[must_use]
    pub const fn start(epoch: FencingToken) -> Self {
        Self { epoch, turns: 0 }
    }

    #[must_use]
    pub const fn new(epoch: FencingToken, turns: u64) -> Self {
        Self { epoch, turns }
    }

    /// Advance by one completed turn within the same incarnation.
    #[must_use]
    pub const fn advanced(self) -> Self {
        Self {
            epoch: self.epoch,
            turns: self.turns.saturating_add(1),
        }
    }

    /// Re-anchor onto `epoch` when the identity's member incarnation moved on.
    ///
    /// A stale or equal epoch leaves the cursor untouched, so a caller
    /// presenting an older token can never rewind what has been published.
    #[must_use]
    pub const fn rebased(self, epoch: FencingToken) -> Self {
        if epoch.get() > self.epoch.get() {
            Self::start(epoch)
        } else {
            self
        }
    }

    /// Classify this cursor against a `baseline` captured before a delivery.
    ///
    /// A baseline under this epoch that counts MORE turns than this cursor
    /// came from an earlier process (Core restores a member's fence across a
    /// restart while this process counts turns from zero) or is forged: it is
    /// `IncarnationChanged`, never `Pending`.
    #[must_use]
    pub const fn progress_since(self, baseline: Self) -> CompletionProgress {
        if self.epoch.get() != baseline.epoch.get() || baseline.turns > self.turns {
            CompletionProgress::IncarnationChanged
        } else if self.turns > baseline.turns {
            CompletionProgress::Completed
        } else {
            CompletionProgress::Pending
        }
    }
}

impl fmt::Display for CompletionCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.epoch, self.turns)
    }
}

/// How an observed [`CompletionCursor`] relates to a baseline captured when a
/// delivery was admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionProgress {
    /// Same incarnation, nothing completed since the baseline.
    Pending,
    /// Same incarnation, at least one turn completed since the baseline.
    Completed,
    /// The identity's member incarnation changed (Core moved its fence on a
    /// respawn, reset or fresh spawn), or a process restart lost this
    /// process's count. Turn counts do not carry across incarnations, so the
    /// caller must re-establish a baseline rather than infer either
    /// completion or continued waiting.
    IncarnationChanged,
}

/// How a completion wait ([`await_completion`]) ended. Each variant carries
/// the cursor it observed last.
///
/// [`await_completion`]: super::runtime::IdentityRuntime::await_completion
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CompletionWait {
    /// The cursor moved past the baseline (or, with no baseline, counts at
    /// least one completed turn).
    Completed(CompletionCursor),
    /// The identity's runtime incarnation changed; see
    /// [`CompletionProgress::IncarnationChanged`].
    IncarnationChanged(CompletionCursor),
    /// A run on the identity failed (or was cancelled) during the wait. Only
    /// failures observed while the wait runs count: one that ended before the
    /// wait started is not visible to it.
    RunFailed(CompletionCursor),
    /// The identity was registered when the wait started and no longer is
    /// (deleted, or removed from the roster).
    IdentityGone(CompletionCursor),
    /// The identity is parked Broken.
    Broken(CompletionCursor),
    /// The identity is retiring or retired.
    Retiring(CompletionCursor),
    /// The runtime is shutting down.
    ShuttingDown(CompletionCursor),
    /// The caller's deadline passed first.
    TimedOut(CompletionCursor),
}

impl CompletionWait {
    /// Wire token of the outcome.
    pub fn wire_str(&self) -> &'static str {
        match self {
            Self::Completed(_) => "completed",
            Self::IncarnationChanged(_) => "incarnation_changed",
            Self::RunFailed(_) => "run_failed",
            Self::IdentityGone(_) => "identity_gone",
            Self::Broken(_) => "broken",
            Self::Retiring(_) => "retiring",
            Self::ShuttingDown(_) => "shutting_down",
            Self::TimedOut(_) => "timed_out",
        }
    }

    /// The cursor the wait observed last.
    pub fn cursor(&self) -> CompletionCursor {
        match self {
            Self::Completed(cursor)
            | Self::IncarnationChanged(cursor)
            | Self::RunFailed(cursor)
            | Self::IdentityGone(cursor)
            | Self::Broken(cursor)
            | Self::Retiring(cursor)
            | Self::ShuttingDown(cursor)
            | Self::TimedOut(cursor) => *cursor,
        }
    }
}

/// How a ticketed turn wait ([`wait_for_turn_outcome`]) ended.
///
/// [`wait_for_turn_outcome`]: super::runtime::IdentityRuntime::wait_for_turn_outcome
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TurnWait {
    /// The turn left pending (or the ticket is unknown).
    Settled(TurnOutcome),
    /// The caller's deadline passed with the turn still pending.
    TimedOut(TurnOutcome),
    /// The runtime is shutting down; the turn's state at that moment.
    ShuttingDown(TurnOutcome),
}

/// Delivery receipt for [`dispatch_admission_tracked`], carrying what a caller
/// needs to wait for the specific turn it just submitted.
///
/// [`dispatch_admission_tracked`]: super::runtime::IdentityRuntime::dispatch_admission_tracked
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DispatchAdmission {
    pub fencing_token: FencingToken,
    /// Whether the dispatch is backed by a runtime store (REQ-04).
    pub durable: bool,
    /// Cursor read before delivery was attempted. The turn this dispatch
    /// starts can only complete after this point, so waiting for a cursor
    /// whose [`CompletionCursor::progress_since`] against this baseline
    /// reports [`CompletionProgress::Completed`] cannot miss it.
    pub completion_baseline: CompletionCursor,
}

/// Delivery receipt for [`send_admission_tracked`]. Same baseline contract as
/// [`DispatchAdmission`].
///
/// [`send_admission_tracked`]: super::runtime::IdentityRuntime::send_admission_tracked
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendAdmission {
    pub fencing_token: FencingToken,
    pub completion_baseline: CompletionCursor,
}

// ---------------------------------------------------------------------------
// Turn tickets: per-admission completion
// ---------------------------------------------------------------------------

/// Names one admitted turn so a caller can wait for THAT turn and read ITS
/// output, however much other traffic the identity receives.
///
/// Minted by the runtime once per admission and never taken from the caller:
/// interaction ids and delivery correlation ids may legitimately repeat across
/// admissions (a fan-out of one correlation to several identities, a reused
/// interaction id), so a ticket derived from either could name two turns. The
/// runtime holds the admitted turn's own completion handle under the ticket,
/// so the outcome it reports is typed state of that one turn, never a
/// correlation by output text or by completion count. The identity-wide
/// [`CompletionCursor`] cannot name one turn: a peer message, a scheduled turn
/// or a fork completion wake advances it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnTicket(uuid::Uuid);

impl TurnTicket {
    /// A fresh ticket. Only the runtime's outcome registry mints tickets, and
    /// it re-mints on the (astronomically unlikely) collision.
    #[must_use]
    pub(crate) fn mint() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Parse a ticket from its wire form (a UUID).
    pub fn parse(raw: &str) -> Result<Self, uuid::Error> {
        raw.parse().map(Self)
    }

    #[must_use]
    pub const fn as_uuid(&self) -> uuid::Uuid {
        self.0
    }
}

impl fmt::Display for TurnTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// What a completed ticketed turn yielded, projected from that turn's own
/// exact completion (never the session's latest output).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutput {
    /// The turn's own final assistant text, bounded to
    /// [`TURN_OUTPUT_MAX_BYTES`]. When `truncated`, the text was cut at the
    /// bound (meerkat also appends a truncation marker to it).
    Text { text: String, truncated: bool },
    /// The turn ran and committed a run result with no text.
    Empty,
    /// The admission reached a successful terminal without a run result of its
    /// own: the runtime folded the input into a run already in progress, or
    /// deduplicated it onto an earlier admission whose turn had already
    /// ended. No output belongs to this admission; reporting "no text" here
    /// would be a guess.
    NoOwnResult,
    /// The session bridge cannot report per-turn output (a bridge that built
    /// its receipt with
    /// [`BridgeTurnReceipt::new`](super::bridge::BridgeTurnReceipt::new)).
    Unavailable,
}

impl TurnOutput {
    /// The turn's own text, when it has one.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text, .. } => Some(text),
            Self::Empty | Self::NoOwnResult | Self::Unavailable => None,
        }
    }

    /// Stable wire code (`output_status` on `meta/turn_result`).
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Empty => "empty",
            Self::NoOwnResult => "no_own_result",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Byte bound on one tracked turn's output text.
pub const TURN_OUTPUT_MAX_BYTES: usize = 256 * 1024;

/// Where a ticketed turn stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// Admitted; the turn has not reached its terminal yet.
    Pending,
    /// The turn reached a successful terminal; `output` says what it yielded.
    Completed { output: TurnOutput },
    /// The turn ran and failed, or its terminal could not be observed.
    Failed { reason: String },
    /// No turn with this ticket is known for this identity: it was never
    /// admitted here, it belongs to another identity, its record aged out, or
    /// the runtime restarted since. Never a guess in either direction.
    Unknown,
}

/// Why a delivery's turn cannot be tracked by ticket.
///
/// Delivery semantics differ by variant: [`Self::NotDelivered`] means nothing
/// was delivered. Every other variant means the delivery happened, exactly
/// once, on the ingress lane (a refused tracked admission is refused before
/// anything is submitted, so the ingress delivery is the only one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnUntrackable {
    /// The member runs in `autonomous_host` mode, read from the runtime's
    /// LIVE roster entry at delivery (the mode it was spawned with; a changed
    /// `runtime_mode` takes effect on respawn). meerkat does not report
    /// per-turn completion for autonomous inbox delivery. Delivered once on
    /// the ingress lane.
    AutonomousHost,
    /// meerkat refused the tracked admission for the member's live mode for
    /// another reason (the member runs `mode`), before anything was
    /// submitted. Delivered once on the ingress lane.
    RefusedByRuntime { mode: meerkat_mob::MobRuntimeMode },
    /// The member is externally bound; its deliveries are not ticketed.
    /// Delivered once on the ingress lane.
    ExternallyBound,
    /// Host human input rides the console human lane, which is not ticketed.
    /// Delivered once on that lane.
    HostHumanInput,
    /// The session bridge cannot report a turn's own output. Delivered once
    /// on the ingress lane.
    BridgeCannotReportOutput,
    /// A re-dispatch meant to be deduplicated onto an earlier tracked
    /// admission landed on a different session (delivery repair or a respawn
    /// moved the member), whose runtime ledger had never seen the idempotency
    /// key, so it ran as a turn of its own that this delivery did not track.
    /// Delivered once on the ingress lane.
    SessionRotated,
    /// NOTHING was delivered: the runtime has no session bridge, or the
    /// identity has no bound agent runtime. There is no turn to wait for.
    NotDelivered,
}

impl TurnUntrackable {
    /// Stable wire code (`turn_unavailable.code`).
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::AutonomousHost => "autonomous_host",
            Self::RefusedByRuntime { .. } => "runtime_refused",
            Self::ExternallyBound => "externally_bound",
            Self::HostHumanInput => "host_human_input",
            Self::BridgeCannotReportOutput => "bridge_cannot_report_output",
            Self::SessionRotated => "session_rotated",
            Self::NotDelivered => "not_delivered",
        }
    }

    /// Whether the delivery happened (exactly once) despite being untracked.
    /// `false` only for [`Self::NotDelivered`].
    #[must_use]
    pub const fn delivered(self) -> bool {
        !matches!(self, Self::NotDelivered)
    }
}

impl fmt::Display for TurnUntrackable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AutonomousHost => f.write_str(
                "the member runs in autonomous_host mode (its mode as spawned; a changed \
                 runtime_mode takes effect on respawn), and the runtime does not report \
                 per-turn completion for autonomous inbox delivery; delivered once, untracked",
            ),
            Self::RefusedByRuntime { mode } => write!(
                f,
                "the runtime refused per-turn completion for this member, which runs in {mode} \
                 mode; delivered once, untracked"
            ),
            Self::ExternallyBound => f.write_str(
                "the member is externally bound; its deliveries are not ticketed; delivered \
                 once, untracked",
            ),
            Self::HostHumanInput => f.write_str(
                "host human input rides the console lane, which is not ticketed; delivered \
                 once, untracked",
            ),
            Self::BridgeCannotReportOutput => f.write_str(
                "the session bridge cannot report a turn's own output; delivered once, \
                 untracked",
            ),
            Self::SessionRotated => f.write_str(
                "the re-dispatch landed on a different session than the admission it was to be \
                 deduplicated onto, whose runtime had not seen its idempotency key, so it ran as \
                 a turn of its own; delivered once, untracked",
            ),
            Self::NotDelivered => {
                f.write_str("nothing was delivered (no session bridge or no bound agent runtime)")
            }
        }
    }
}

/// Whether a delivery can be awaited by ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnTracking {
    /// The turn is tracked under this ticket.
    Tracked(TurnTicket),
    /// The turn cannot be tracked, and the variant says whether the delivery
    /// happened ([`TurnUntrackable::delivered`]). When it did, a caller that
    /// needs a completion falls back to the identity-wide
    /// [`CompletionCursor`], knowing it is not request-correlated; when it
    /// did not ([`TurnUntrackable::NotDelivered`]), there is nothing to wait
    /// for.
    Unavailable(TurnUntrackable),
}

/// An admission receipt plus the ticket that names the admitted turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ticketed<A> {
    pub admission: A,
    pub turn: TurnTracking,
}

// ---------------------------------------------------------------------------
// Lightweight string newtypes (no validation beyond serde)
// ---------------------------------------------------------------------------

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new(s: impl Into<String>) -> Self {
                Self(s.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_newtype!(
    /// Correlation ID for dispatch tracing.
    CorrelationId
);

string_newtype!(
    /// Idempotency key for dispatch deduplication.
    DispatchIdempotencyKey
);

// ---------------------------------------------------------------------------
// ContinuityRecord
// ---------------------------------------------------------------------------

/// The authoritative continuity record for a durable agent identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuityRecord {
    pub identity: AgentIdentity,
    pub agent_runtime_id: AgentRuntimeId,
    pub session_id: meerkat_core::types::SessionId,
    pub generation: ContinuityGeneration,
    pub checkpoint_version: CheckpointVersion,
}

// ---------------------------------------------------------------------------
// ContinuityFailure + ContinuityFailureKind
// ---------------------------------------------------------------------------

/// Kind of continuity failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuityFailureKind {
    SnapshotMissing,
    SnapshotCorrupted,
    GenerationMismatch,
    StoreUnavailable,
    /// The bridge refused a resume while the durable session row exists (e.g.
    /// a transcript-continuity rejection). The identity → session binding is
    /// intact; the identity is degraded until a reconcile retry succeeds.
    ResumeRejected,
    /// This identity's concrete embodiment transaction failed after the
    /// fleet-level roster, topology, and continuity gates succeeded. The
    /// failure is scoped to one member: eager restore parks that identity as
    /// Broken and continues materializing the rest of the roster.
    EmbodimentFailed,
    /// A terminal typed verdict stands against this identity: the heal
    /// authority proved the durable session head unrecoverable (proof inputs
    /// absent), or the resume precondition is provably terminal (the typed
    /// `ArchivedNotRevivable` refusal - the production heal/refusal loop shape).
    /// Unlike `ResumeRejected` this is NOT retried by the continuity repair
    /// supervisor — retrying is exactly the 2026-07-29 heal/re-Break loop.
    /// The identity stays Broken until an operator intervenes.
    CheckpointUnrecoverable,
    /// The durable session's committed WholeBlob document fails its own
    /// audited-endpoint guard (meerkat's typed
    /// `SessionError::WholeBlobAuditedEndpointDivergence`, resume hold
    /// `audited_endpoint_divergence`). Every row is intact and every read
    /// refuses, so no retry, heal, or reconcile can change the verdict: the
    /// sanctioned repair (`rkat session repair-wholeblob`) is the only way
    /// forward. The continuity repair supervisor does not retry it; the
    /// operator repairs the document and runs `meta/reload_member`, which
    /// resumes the same session with no extra flag. The typed hold rides
    /// [`SessionRepairRequired`] on the identity status and member health.
    RepairRequired,
    /// A role migration this activation declared for the identity was not
    /// applied, so the member was not attached under the undeclared role (the
    /// bridge's `ResumeRejectionKind::RoleMigrationNotApplied`): Meerkat
    /// refused the restamp, the declared predecessor is not the member's
    /// durable role, or a Present identity intent owns the member and pins
    /// its profile. The durable session is preserved; the activation's
    /// `identity_bootstrap` reports the identity Broken with this kind, so a
    /// host never deploys believing the migration landed.
    RoleMigrationNotApplied,
}

/// Typed "session needs repair" hold recorded against a Broken identity.
///
/// Produced when a resume, a reload, or the heal authority reports meerkat's
/// typed [`DurableResumeHold::AuditedEndpointDivergence`]: the committed
/// WholeBlob document preserves every message but its live transcript no
/// longer preserves the graph-proved audited endpoint, so every read refuses
/// (the downstream app's 2026-09-22 wedge). The hold is matched on the typed variant
/// or the typed wire token only, never on display text.
///
/// While recorded, the continuity repair supervisor skips the identity and
/// reconcile keeps its Broken projection (a retry storm against a stable
/// verdict is exactly the behaviour this replaces). The operator runs
/// `diagnose_command`, then `apply_command`, then `meta/reload_member`,
/// which resumes the same durable session and clears the hold. The hold is
/// process-local (entry state, not durable): a gateway restart re-attempts
/// the resume once and re-records it if the document is still refused.
/// A restored member whose customizer tools are not published yet: the
/// pre-activation `customize_build` for its identity failed, so meerkat
/// restored it without them. It advertises no customizer tools until its
/// materialization publishes them (#563). Process-local: cleared by the next
/// successful publication, and absent after a restart that publishes early.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomizerToolsPending {
    /// Why the early publication did not happen (the customizer error).
    pub reason: String,
    /// The typed refusal, when the customizer's callback tools were refused
    /// for their installed-artifact references.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_refusal: Option<meerkat_core::ToolArtifactRefusal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRepairRequired {
    /// The durable session whose committed document needs the repair.
    pub session_id: meerkat_core::types::SessionId,
    /// meerkat's typed resume hold class, on the wire as its stable token
    /// (`audited_endpoint_divergence`).
    pub hold: DurableResumeHold,
    /// `rkat ... session repair-wholeblob <id> --json`: diagnose only, writes
    /// nothing.
    pub diagnose_command: String,
    /// `rkat ... session repair-wholeblob <id> --apply --json`: re-anchors the
    /// document on its live rows without dropping a message. Add
    /// `--accept-shorter` only when the diagnose report says the live
    /// transcript is shorter than the audited endpoint.
    pub apply_command: String,
    /// The producing error, verbatim, for operators.
    pub detail: String,
}

impl SessionRepairRequired {
    /// The repair CLI subcommand every hold points operators at.
    pub const REPAIR_SUBCOMMAND: &'static str = "session repair-wholeblob";

    /// Build the hold for meerkat's audited-endpoint divergence.
    ///
    /// The commands name the exact session and the exact runtime store: a
    /// realm scope renders as `rkat --state-root <root> --realm <realm> ...`,
    /// a directly opened database as `... --runtime-store <path>`, and a
    /// scope the host did not declare stays as explicit `<state-root>` /
    /// `<realm>` placeholders. Meta never guesses a filesystem path it did
    /// not open.
    pub fn audited_endpoint_divergence(
        session_id: meerkat_core::types::SessionId,
        scope: Option<&SessionRepairScope>,
        detail: impl Into<String>,
    ) -> Self {
        let (prefix, suffix) = Self::rkat_scope(scope);
        let diagnose_command = format!(
            "{prefix} {} {session_id}{suffix} --json",
            Self::REPAIR_SUBCOMMAND
        );
        let apply_command = format!(
            "{prefix} {} {session_id}{suffix} --apply --json",
            Self::REPAIR_SUBCOMMAND
        );
        Self {
            session_id,
            hold: DurableResumeHold::AuditedEndpointDivergence,
            diagnose_command,
            apply_command,
            detail: detail.into(),
        }
    }

    /// `(prefix, suffix)` around `session repair-wholeblob <id>`: a realm
    /// scope goes before the subcommand, a direct runtime store after the id.
    fn rkat_scope(scope: Option<&SessionRepairScope>) -> (String, String) {
        match scope {
            Some(SessionRepairScope {
                state_root: Some(state_root),
                realm: Some(realm),
                ..
            }) => (
                format!("rkat --state-root {} --realm {realm}", state_root.display()),
                String::new(),
            ),
            Some(SessionRepairScope {
                runtime_store: Some(runtime_store),
                ..
            }) => (
                "rkat".to_string(),
                format!(" --runtime-store {}", runtime_store.display()),
            ),
            _ => {
                let state_root = scope
                    .and_then(|scope| scope.state_root.as_deref())
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "<state-root>".to_string());
                let realm = scope
                    .and_then(|scope| scope.realm.as_deref())
                    .unwrap_or("<realm>");
                (
                    format!("rkat --state-root {state_root} --realm {realm}"),
                    String::new(),
                )
            }
        }
    }

    /// One operator-facing sentence: what stands, and the two commands.
    pub fn operator_reason(&self) -> String {
        format!(
            "session {} needs the sanctioned audited-endpoint repair (typed resume hold `{}`; every message is intact and every read refuses; no retry or heal can change this). Diagnose: `{}`; repair: `{}`; then run meta/reload_member on this identity. Refusal: {}",
            self.session_id, self.hold, self.diagnose_command, self.apply_command, self.detail
        )
    }
}

/// Where the operator's `rkat` scope for [`SessionRepairRequired`] commands
/// points: the meerkat state root and realm whose runtime store holds the
/// identity's durable sessions. Declared by the host at composition time
/// (Meta embedded in a meerkat realm knows the state root from
/// `MetaStorageLayout::meerkat_state_root`); a field the host did not
/// declare renders as an explicit placeholder in the commands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionRepairScope {
    /// meerkat state root of a realm-layout runtime store (`<state_root>/<realm>/runtime.sqlite3`).
    pub state_root: Option<std::path::PathBuf>,
    /// Realm of a realm-layout runtime store.
    pub realm: Option<String>,
    /// The runtime database Meta opened directly (the default persistent
    /// layout, `<state_dir>/runtime.sqlite`); rendered as `--runtime-store`
    /// when no realm scope is declared.
    pub runtime_store: Option<std::path::PathBuf>,
}

/// A typed failure payload for broken continuity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuityFailure {
    pub identity: AgentIdentity,
    pub kind: ContinuityFailureKind,
    pub record: Option<ContinuityRecord>,
    pub detail: String,
}

/// Terminal repair verdict recorded against a Broken identity.
///
/// Three producers mint it:
///
/// - The session bridge's heal authority reporting that the durable session
///   head is provably NOT recoverable to a strict-resume-acceptable
///   committed boundary (`CommittedBoundaryRepair::Unprovable`). The verdict
///   is stable across calls, so the continuity repair supervisor must not
///   retry-loop it: before this marker existed, every repair pass
///   cosmetically re-registered the identity and the next materialization
///   re-Broke it (measured in production on 2026-07-29 as an infinite
///   heal/re-Break cycle).
/// - The typed `ArchivedNotRevivable` resume refusal, recorded on the FIRST
///   refusal at either resume door (eager restore or on-demand
///   materialize). The refusal is a stable materialize precondition the
///   roster heal cannot change, so without this verdict the repair
///   supervisor "healed" the roster every cycle and the next inbound turn
///   re-Broke it (production rehearsal, 4 identities) - the N=3 identical-failure
///   park below never engaged because the heal itself kept succeeding.
/// - The repair supervisor's bounded-identical-retry park: three consecutive
///   byte-identical repair failures prove a deterministic wall, and each
///   blind retry re-executes destructive dispose steps against it (production
///   0.8.12-era field evidence).
///
/// The park is process-local (entry state, not durable): after the operator
/// fixes the blocking cause, a gateway restart re-attempts repair once, and
/// `meta/reset` remains the deliberate fresh-start path. Any non-Broken
/// lifecycle projection also clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContinuityUnrecoverable {
    /// The producing authority's reason, verbatim, for operators.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// ContinuityResolveState
// ---------------------------------------------------------------------------

/// The resolve result for a single identity from the continuity store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum ContinuityResolveState {
    Uninitialized,
    Ready { record: ContinuityRecord },
    Broken { failure: ContinuityFailure },
}

// ---------------------------------------------------------------------------
// LeaseGrant
// ---------------------------------------------------------------------------

/// A lease grant returned by `LeaseProvider`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseGrant {
    pub identity: AgentIdentity,
    pub fencing_token: FencingToken,
    #[serde(
        serialize_with = "serde_duration_ms::serialize",
        deserialize_with = "serde_duration_ms::deserialize"
    )]
    pub ttl: std::time::Duration,
}

/// Custom serde for `Duration` as integer milliseconds.
mod serde_duration_ms {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let ms = duration.as_millis();
        // u128 -> u64 is safe for any reasonable TTL
        serializer.serialize_u64(ms as u64)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let ms = u64::deserialize(deserializer)?;
        Ok(Duration::from_millis(ms))
    }
}

// ---------------------------------------------------------------------------
// LeaseAcquireResult + LeaseRenewResult
// ---------------------------------------------------------------------------

/// Result of a lease acquisition attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum LeaseAcquireResult {
    Acquired(LeaseGrant),
    AlreadyHeld {
        identity: AgentIdentity,
        holder: String,
    },
}

/// Result of a lease renewal attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum LeaseRenewResult {
    Renewed(LeaseGrant),
    Lost { identity: AgentIdentity },
}

// ---------------------------------------------------------------------------
// DispatchOrigin + DispatchInput
// ---------------------------------------------------------------------------

/// Origin of a dispatch request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchOrigin {
    Connector,
    Scheduler,
    Policy,
    Flow,
    System,
}

/// Input for a dispatch operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DispatchInput {
    pub content: meerkat_core::ContentInput,
    pub origin: DispatchOrigin,
    pub correlation_id: Option<CorrelationId>,
    pub idempotency_key: Option<DispatchIdempotencyKey>,
}

impl DispatchInput {
    /// System-origin dispatch from plain text. The common case.
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            content: meerkat_core::ContentInput::Text(text.into()),
            origin: DispatchOrigin::System,
            correlation_id: None,
            idempotency_key: None,
        }
    }

    /// Dispatch with an explicit origin from plain text.
    pub fn with_origin(text: impl Into<String>, origin: DispatchOrigin) -> Self {
        Self {
            content: meerkat_core::ContentInput::Text(text.into()),
            origin,
            correlation_id: None,
            idempotency_key: None,
        }
    }

    /// Attach a correlation ID (builder pattern).
    pub fn with_correlation(mut self, id: impl Into<String>) -> Self {
        self.correlation_id = Some(CorrelationId::new(id));
        self
    }

    /// Attach an idempotency key (builder pattern).
    pub fn with_idempotency(mut self, key: impl Into<String>) -> Self {
        self.idempotency_key = Some(DispatchIdempotencyKey::new(key));
        self
    }
}

// ---------------------------------------------------------------------------
// ManagedPeerEdge
// ---------------------------------------------------------------------------

/// Error returned when constructing an invalid `ManagedPeerEdge`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedPeerEdgeError {
    SelfEdge,
}

impl fmt::Display for ManagedPeerEdgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelfEdge => write!(f, "self-edges are not allowed"),
        }
    }
}

impl std::error::Error for ManagedPeerEdgeError {}

/// A managed dynamic topology edge between two agent identities.
///
/// Canonical ordering: `a < b`. Self-edges are rejected at construction time.
/// Deserialization enforces the same invariant as `new()` via `TryFrom`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(into = "ManagedPeerEdgeRaw")]
pub struct ManagedPeerEdge {
    a: AgentIdentity,
    b: AgentIdentity,
}

/// Raw deserialization target for `ManagedPeerEdge`.
#[derive(Serialize, Deserialize)]
struct ManagedPeerEdgeRaw {
    a: AgentIdentity,
    b: AgentIdentity,
}

impl From<ManagedPeerEdge> for ManagedPeerEdgeRaw {
    fn from(edge: ManagedPeerEdge) -> Self {
        Self {
            a: edge.a,
            b: edge.b,
        }
    }
}

impl TryFrom<ManagedPeerEdgeRaw> for ManagedPeerEdge {
    type Error = ManagedPeerEdgeError;

    fn try_from(raw: ManagedPeerEdgeRaw) -> Result<Self, Self::Error> {
        Self::new(raw.a, raw.b)
    }
}

impl<'de> Deserialize<'de> for ManagedPeerEdge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = ManagedPeerEdgeRaw::deserialize(deserializer)?;
        Self::try_from(raw).map_err(serde::de::Error::custom)
    }
}

impl ManagedPeerEdge {
    /// Construct a managed peer edge with canonical ordering enforcement.
    ///
    /// # Errors
    ///
    /// Returns `ManagedPeerEdgeError::SelfEdge` if `a == b`.
    pub fn new(a: AgentIdentity, b: AgentIdentity) -> Result<Self, ManagedPeerEdgeError> {
        if a == b {
            return Err(ManagedPeerEdgeError::SelfEdge);
        }
        if a < b {
            Ok(Self { a, b })
        } else {
            Ok(Self { a: b, b: a })
        }
    }

    #[must_use]
    pub fn a(&self) -> &AgentIdentity {
        &self.a
    }

    #[must_use]
    pub fn b(&self) -> &AgentIdentity {
        &self.b
    }
}

// ---------------------------------------------------------------------------
// NotAddressable error
// ---------------------------------------------------------------------------

/// Error returned when `send()` targets an `InternalOnly` agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotAddressable {
    pub identity: AgentIdentity,
    pub addressability: AgentAddressability,
}

impl fmt::Display for NotAddressable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "agent {:?} is not addressable (current: {:?})",
            self.identity, self.addressability
        )
    }
}

impl std::error::Error for NotAddressable {}

// ---------------------------------------------------------------------------
// RoleMigrationDeclaration
// ---------------------------------------------------------------------------

/// A host assertion that one identity's durable member role changed, and that
/// this activation is authorized to migrate it.
///
/// Activation-scoped by construction. It arrives in the gateway boot payload
/// (`InitParams.role_migrations`), is held for the life of one
/// [`crate::identity_first::bridge::MobSessionBridge`] - which lives for
/// exactly one boot - and is never written to the continuity store. Omitting
/// it from the next boot payload is how it goes away; there is no durable copy
/// that could bring it back.
///
/// Meta never infers a declaration. It does not compare the requested role
/// against durable state, does not read stored role metadata to synthesize
/// one, and does not retry a refused resume. If the host did not declare a
/// migration for an exact identity, the resume carries no migration authority
/// and Meerkat's typed refusal (`MobError::MemberRoleMigrationRequired`)
/// stands - which is the point: an unintended role change must fail closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleMigrationDeclaration {
    /// The exact identity being migrated. No wildcards, no prefix matching.
    pub identity: AgentIdentity,
    /// The role this identity is migrating FROM.
    ///
    /// Meerkat verifies this against the durable predecessor role and refuses
    /// with `MobError::MemberRoleMigrationRejected` on mismatch, so a mistyped
    /// value cannot authorize an unintended restamp.
    pub from_role: meerkat_mob::ProfileName,
}

/// The first identity a payload declares twice with CONFLICTING predecessor
/// roles, as `(identity, first_role, second_role)`.
///
/// Two identical declarations are redundant but unambiguous, so they are
/// allowed: refusing them would turn a harmless repetition into a boot failure.
/// Do NOT tighten this later. The downstream app's inertness witness works by deliberately
/// carrying the SAME declaration into the next activation and showing nothing
/// moves, so refusing an identical repeat would make that experiment
/// impossible to run.
/// Two conflicting declarations have no defensible resolution - collecting them
/// into a map would let hash order decide which predecessor role becomes
/// migration authority - so the payload must be fixed by its author.
#[must_use]
pub fn conflicting_role_migration_declaration(
    declarations: &[RoleMigrationDeclaration],
) -> Option<(
    &AgentIdentity,
    &meerkat_mob::ProfileName,
    &meerkat_mob::ProfileName,
)> {
    let mut seen: std::collections::HashMap<&AgentIdentity, &meerkat_mob::ProfileName> =
        std::collections::HashMap::new();
    for declaration in declarations {
        if let Some(existing) = seen.insert(&declaration.identity, &declaration.from_role)
            && existing != &declaration.from_role
        {
            return Some((&declaration.identity, existing, &declaration.from_role));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// DurableAgentSpec
// ---------------------------------------------------------------------------

/// The preferred roster/spawn specification for identity-first continuity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableAgentSpec {
    pub identity: AgentIdentity,
    pub profile: meerkat_mob::ProfileName,
    #[serde(default)]
    pub addressability: AgentAddressability,
    pub display_name: Option<DisplayName>,
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
    pub context: Option<serde_json::Value>,
    #[serde(default)]
    pub additional_instructions: Vec<String>,
    #[serde(default)]
    pub initial_message: Option<meerkat_core::ContentInput>,
    #[serde(default)]
    pub runtime_mode_override: Option<meerkat_mob::MobRuntimeMode>,
    #[serde(default)]
    pub backend: Option<meerkat_mob::MobBackendKind>,
    #[serde(default)]
    pub binding: Option<meerkat_contracts::WireRuntimeBinding>,
    /// Exact Meerkat host placement. Once present it must propagate unchanged
    /// and may never degrade to local because the selected host is unavailable.
    #[serde(default)]
    pub placement: Option<meerkat_contracts::WireHostRef>,
}

/// An identity's REGISTERED roster row: the row the identity runtime holds
/// for it and delivers under, with the row's canonical digest.
///
/// This is neither the desired row (the roster provider's current answer,
/// adopted only by a roster pass or a reset) nor proof that a materialized
/// actor adopted every field. Reading it is informational: a host that
/// authors a turn from it passes `spec_digest` back with the send, and the
/// send re-checks it against the registered row under the identity's
/// lifecycle lock before anything is submitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegisteredSpec {
    pub spec: DurableAgentSpec,
    /// [`registered_spec_digest`] of `spec`.
    pub spec_digest: String,
}

/// Canonical digest of a full roster row: SHA-256 over its JSON with object
/// keys sorted, as `sha256:<hex>`. Equal rows have equal digests; this is a
/// full-row equality check, not a revision or a generation.
pub fn registered_spec_digest(spec: &DurableAgentSpec) -> Result<String, serde_json::Error> {
    use sha2::Digest as _;
    let mut canonical = String::new();
    write_canonical_json(&serde_json::to_value(spec)?, &mut canonical)?;
    Ok(format!(
        "sha256:{:x}",
        sha2::Sha256::digest(canonical.as_bytes())
    ))
}

fn write_canonical_json(
    value: &serde_json::Value,
    out: &mut String,
) -> Result<(), serde_json::Error> {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            out.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key)?);
                out.push(':');
                write_canonical_json(item, out)?;
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical_json(item, out)?;
            }
            out.push(']');
        }
        scalar => out.push_str(&serde_json::to_string(scalar)?),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// IdentityStatus + supporting types
// ---------------------------------------------------------------------------

/// Highest supported fan-out for background identity hydration.
///
/// Restore already caps concurrent resume work at sixteen because every
/// materialization can contend on the session/continuity stores.  Keep the
/// public background-warm control inside that same operational envelope.
pub const MAX_IDENTITY_BACKGROUND_WARM_CONCURRENCY: usize = 16;

/// Controls how identity-first durable agents are materialized at startup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum IdentityBootstrapMode {
    /// Compatibility mode: startup synchronously creates or resumes every
    /// identity in the roster.
    #[default]
    EagerMaterialize,
    /// Register roster/topology/continuity metadata only. A concrete member is
    /// created or resumed on first use or explicit materialization.
    LazyMaterialize,
    /// Return after metadata registration and hydrate identities in a tracked
    /// background task with bounded concurrency.
    LazyWithBackgroundWarm { concurrency: usize },
}

impl IdentityBootstrapMode {
    /// Validate the operational concurrency contract.
    pub fn validate(&self) -> Result<(), String> {
        if let Self::LazyWithBackgroundWarm { concurrency } = self {
            if *concurrency == 0 {
                return Err("LazyWithBackgroundWarm concurrency must be greater than 0".to_string());
            }
            if *concurrency > MAX_IDENTITY_BACKGROUND_WARM_CONCURRENCY {
                return Err(format!(
                    "LazyWithBackgroundWarm concurrency must be at most {MAX_IDENTITY_BACKGROUND_WARM_CONCURRENCY}"
                ));
            }
        }
        Ok(())
    }

    pub fn is_lazy(&self) -> bool {
        !matches!(self, Self::EagerMaterialize)
    }
}

/// Transient startup-hydration state. This is deliberately separate from
/// [`IdentityLifecycleState`]: warming is coordination progress, not a durable
/// identity lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityBootstrapState {
    Dormant,
    Warming,
    Active,
    Broken,
}

/// Where one identity's eager restore stands, published on the bootstrap
/// status while the restore pass runs, so a host can follow each member
/// instead of waiting on the whole pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum IdentityRestoreProgress {
    /// The roster metadata is registered; the wake has not started.
    Registered,
    /// The identity's wake (a resume, or a fresh spawn) is running.
    Resuming,
    /// Core spawned the identity fresh.
    Minted,
    /// Core resumed the identity onto its committed session.
    Resumed,
    /// The wake was refused: `code` is the refusal's JSON-RPC
    /// code and `kind` its structured kind, when it has one. Nothing is
    /// parked: the next wake asks Core again.
    Refused {
        code: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
    },
}

/// Bootstrap progress for one durable identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityBootstrapEntry {
    pub state: IdentityBootstrapState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Typed eager-restore progress; `None` outside an eager restore pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<IdentityRestoreProgress>,
}

/// Aggregate bootstrap-state counts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityBootstrapCounts {
    pub dormant: usize,
    pub warming: usize,
    pub active: usize,
    pub broken: usize,
    /// The identities a candidate launch deferred to the authoritative
    /// launch ([`IdentityBootstrapStatus::deferred`]). Omitted from the JSON
    /// when zero, so every other launch answers as before.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub deferred: usize,
}

/// serde's `skip_serializing_if` for a count, which it passes by reference.
fn is_zero(count: &usize) -> bool {
    *count == 0
}

/// Why a candidate launch (`mob_composition.authority = "candidate"`) left
/// an identity to the authoritative launch: waking it would change the
/// stored roster, which a candidate does not do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DeferredIdentityReason {
    /// The store holds no member for the identity: its wake would create
    /// one.
    CreateMember,
    /// Its stored member runs under another role than the roster names,
    /// and the launch declared that migration: its wake would move it.
    MoveRole,
    /// Its stored session is not in the runtime store: its wake would start
    /// a fresh session in place of it.
    FreshSession,
    /// Its first-boot manifest row starts it on a fresh session, replacing
    /// its live member.
    FirstBootFresh,
    /// A member would be spawned onto its stored session, which Core holds
    /// and journals the member of: Core holds no live member for it (a
    /// dormant or retired identity, or a first-boot `Resume` row), or its
    /// live member is one Meta 0.9.3 spawned without the durable-identity
    /// marker, which the roster pass respawns onto the same session. The
    /// lowest precedence: an identity another reason also covers reports
    /// that one.
    ResumeMember,
}

impl DeferredIdentityReason {
    /// The reason's wire name: `create_member`, `move_role`,
    /// `fresh_session`, `first_boot_fresh` or `resume_member`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CreateMember => "create_member",
            Self::MoveRole => "move_role",
            Self::FreshSession => "fresh_session",
            Self::FirstBootFresh => "first_boot_fresh",
            Self::ResumeMember => "resume_member",
        }
    }
}

/// An identity a candidate launch deferred to the authoritative launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DeferredIdentity {
    pub identity: AgentIdentity,
    pub reason: DeferredIdentityReason,
}

/// Typed snapshot for bootstrap observability and readiness barriers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityBootstrapStatus {
    pub mode: IdentityBootstrapMode,
    /// True once the configured startup coordination pass has stopped running.
    /// Lazy materialization can therefore be complete while `ready` remains
    /// false because dormant identities intentionally remain.
    pub complete: bool,
    /// True exactly when every tracked roster identity is materialized.
    pub ready: bool,
    /// Pass-level failure, for example a roster/topology provider or restore
    /// error that is not attributable to one durable identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub counts: IdentityBootstrapCounts,
    pub identities: std::collections::BTreeMap<AgentIdentity, IdentityBootstrapEntry>,
    /// The first-boot manifest rows `storage-migrate-native` left that are
    /// not carried out yet, in identity order. Absent when none is
    /// pending.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub first_boot_pending: Vec<FirstBootPendingRow>,
    /// The roster identities a candidate launch deferred to the
    /// authoritative launch, in identity order, each with its reason:
    /// waking one would create a member, move it to another role, start a
    /// fresh session or spawn a member onto its stored session, which only
    /// the authoritative launch does. They are
    /// not tracked in `identities`, so they count neither as broken nor
    /// against `ready` or the bootstrap's completion; `counts.deferred`
    /// counts them. Absent when none is deferred, as on every launch that
    /// speaks for its composition.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred: Vec<DeferredIdentity>,
}

/// A first-boot manifest row that is not carried out yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirstBootPendingRow {
    pub identity: AgentIdentity,
    pub kind: super::first_boot_manifest::FirstBootRowKind,
    pub waiting_for: FirstBootWait,
}

/// What a pending first-boot row waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FirstBootWait {
    /// The roster does not list the identity. The roster compile that lists
    /// it carries the row out before anything wakes the identity.
    Roster,
    /// The roster lists the identity, but carrying the row out failed or
    /// was refused; the identity's next wake, or the next roster compile,
    /// tries again. Its wake answers why.
    CarryOut,
}

impl IdentityBootstrapStatus {
    pub fn empty(mode: IdentityBootstrapMode) -> Self {
        Self {
            mode,
            complete: true,
            ready: true,
            error: None,
            counts: IdentityBootstrapCounts::default(),
            identities: std::collections::BTreeMap::new(),
            first_boot_pending: Vec::new(),
            deferred: Vec::new(),
        }
    }

    pub(crate) fn refresh_aggregates(&mut self) {
        let mut counts = IdentityBootstrapCounts::default();
        for entry in self.identities.values() {
            match entry.state {
                IdentityBootstrapState::Dormant => counts.dormant += 1,
                IdentityBootstrapState::Warming => counts.warming += 1,
                IdentityBootstrapState::Active => counts.active += 1,
                IdentityBootstrapState::Broken => counts.broken += 1,
            }
        }
        counts.deferred = self.deferred.len();
        self.ready = self.complete
            && self.error.is_none()
            && counts.dormant == 0
            && counts.warming == 0
            && counts.broken == 0;
        self.counts = counts;
    }

    /// Report `identity` as deferred to the authoritative launch for
    /// `reason`: it leaves the tracked identities and joins
    /// [`Self::deferred`], replacing an earlier reason.
    pub(crate) fn defer(&mut self, identity: &AgentIdentity, reason: DeferredIdentityReason) {
        self.identities.remove(identity);
        if let Some(existing) = self
            .deferred
            .iter_mut()
            .find(|deferred| &deferred.identity == identity)
        {
            existing.reason = reason;
        } else {
            self.deferred.push(DeferredIdentity {
                identity: identity.clone(),
                reason,
            });
            self.deferred
                .sort_by(|left, right| left.identity.cmp(&right.identity));
        }
        self.refresh_aggregates();
    }

    /// All identities have reached a terminal hydration result. Broken is
    /// terminal but not ready, allowing barriers to return a truthful failure
    /// instead of waiting forever.
    pub fn materialization_terminal(&self) -> bool {
        self.counts.dormant == 0 && self.counts.warming == 0
    }
}

/// Lifecycle state of an identity, as a view of Core's facts.
///
/// Meta keeps no lifecycle state of its own: every read derives this
/// from Core. An identity is `Active` while MobMachine holds a member for it,
/// `Dormant` while it holds none (the identity was retired, or never
/// spawned), and `Broken` while it holds none and the last wake was refused
/// for good ([`IdentityRuntimeError::IdentityUnresumable`](super::IdentityRuntimeError::IdentityUnresumable)):
/// its last session cannot be resumed, and only `delete_identity` starts it
/// again. `Broken` too while MobMachine holds the identity's member Broken:
/// Core could not restore it at this start (a restored member whose comms
/// identity is missing on this host, say), and it serves nothing until it is
/// repaired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityLifecycleState {
    /// Registered and addressable; Core holds no member for the identity.
    Dormant,
    /// Core holds a member for the identity, and that member serves.
    Active,
    /// Core holds no member and the last wake was refused for good, or Core
    /// holds the identity's member Broken: it could not restore it.
    Broken,
}

impl IdentityLifecycleState {
    /// Canonical wire vocabulary for identity lifecycle states.
    ///
    /// meerkat 0.7 moved the member rows (`meta/get_member`,
    /// `list_members`, `ensure_member`, `find_members`) to lowercase state
    /// strings, matching the published SDK constants
    /// (`MEMBER_STATE_ACTIVE = "active"`) and the console vocabulary. The
    /// identity-first status/inspect surfaces must speak the same casing so
    /// the two member-state surfaces never disagree on the same wire.
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Dormant => "dormant",
            Self::Active => "active",
            Self::Broken => "broken",
        }
    }
}

/// Why one incarnation of a durable identity's member was retired, as a
/// gateway host's retirement hook reads it
/// (`GatewayHostComposition::identity_member_retired`).
///
/// Every cause ends exactly that incarnation. Only [`Self::Unlisted`] ends
/// the identity's place in the roster: after [`Self::Retire`] or
/// [`Self::Scoped`] the identity stays registered and dormant, Core keeps its
/// session, and while the roster lists the identity the next roster pass,
/// boot or wake resumes it at the next generation on the same session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RetiredIdentityCause {
    /// A roster pass retired the member because the roster no longer lists
    /// the identity (an identity this runtime registered, or a live member
    /// Meta spawned for an identity it never registered).
    Unlisted,
    /// An explicit retire of the identity.
    Retire,
    /// A retire at a captured delivery scope that named the live member.
    Scoped,
}

impl RetiredIdentityCause {
    /// The cause's wire name: `unlisted`, `retire` or `scoped`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unlisted => "unlisted",
            Self::Retire => "retire",
            Self::Scoped => "scoped",
        }
    }
}

/// One retired incarnation of a durable identity's member, read from Core
/// before the retire: the binding the incarnation ran under. Handed to a
/// gateway host's retirement hook once, after Core's retire returned.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RetiredIdentityMember {
    /// The durable identity.
    pub identity: AgentIdentity,
    /// Core's member id of the incarnation: the member a host's registration
    /// hook saw.
    pub member: String,
    /// The profile Core ran the incarnation under.
    pub role: meerkat_mob::ProfileName,
    /// The incarnation's runtime id (`rt:{identity}:{generation}`).
    pub agent_runtime_id: AgentRuntimeId,
    /// Core's member generation of the incarnation.
    pub generation: u64,
    /// The incarnation's session document; `None` for a peer-only member.
    pub session_id: Option<meerkat_core::types::SessionId>,
    /// The exact Core registration the incarnation held when the retire
    /// began: the one a host's registration hook was given for its build
    /// (`MemberRuntimeBuildIdentity::registration`), and never a later
    /// registration, even one that resumes the same session. `None` names
    /// no registration: this gateway observed none for the incarnation (no
    /// local registration, as for a peer-only or placed member, or a bridge
    /// without a runtime machine), or the registration was still current
    /// after Core's retire answered, so the retire did not end it. Compare
    /// registrations with `==` only: a successor that resumes the same
    /// session can carry the same session id and epoch (all its `Debug`
    /// output shows), and only `==` tells the two apart.
    pub registration: Option<meerkat_runtime::RuntimeSessionRegistrationWitness>,
    /// Why it was retired.
    pub cause: RetiredIdentityCause,
}

/// Durability policy declared by the continuity store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum DurabilityPolicy {
    SyncWriteThrough,
    AsyncReplicated,
    BufferedExport { max_loss_window_ms: u64 },
}

/// Full status response for an identity.
///
/// Every Core fact here (state, runtime id, session, generation) is read
/// from Core when the status is built; Meta caches none of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityStatus {
    pub identity: AgentIdentity,
    pub state: IdentityLifecycleState,
    /// Core's runtime id of the identity's member, while Core holds one.
    pub agent_runtime_id: Option<AgentRuntimeId>,
    /// The session Core binds the identity to: the live member's, or the
    /// last binding of a retired identity. `None` when the identity has no
    /// binding.
    pub session_id: Option<meerkat_core::types::SessionId>,
    /// Core's mob member id of the binding `session_id` comes from: the
    /// roster member Core's member tokens name. `None` when the identity has
    /// no binding. Additive and optional on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
    pub profile: Option<meerkat_mob::ProfileName>,
    pub runtime_mode: Option<meerkat_mob::MobRuntimeMode>,
    pub addressability: AgentAddressability,
    pub display_name: Option<DisplayName>,
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
    /// Core's generation of the binding `session_id` comes from.
    pub generation: Option<meerkat_mob::Generation>,
    /// Whether Core holds an identity intent row for the identity: a host
    /// adopted it, and Meta's lifecycle verbs refuse it.
    #[serde(default)]
    pub intent_managed: bool,
    /// Core's convergence diagnostics for an intent-managed identity, when
    /// Core holds a readable status row (`agent_identity` names the Meta
    /// identity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub convergence: Option<meerkat_contracts::wire::WireIdentityConvergenceStatus>,
    /// The last refused wake, until the next successful one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_refusal: Option<WakeRefusal>,
    /// The session the last wake replaced with a fresh one because Core
    /// holds no durable session for it, until the next wake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_session: Option<ReplacedSession>,
    /// The exact `rkat session repair-wholeblob` commands while the last
    /// wake is held for audited-endpoint repair (`wake_refusal` remedy
    /// `repair_session`), scoped to this runtime's store. Derived from
    /// `wake_refusal` on every read. Additive and optional on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_repair_required: Option<SessionRepairRequired>,
    /// Present while a restored member's customizer tools are unpublished
    /// because its pre-activation `customize_build` failed (#563). Additive
    /// and optional on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub customizer_tools_pending: Option<CustomizerToolsPending>,
}

/// A refused wake an operator has to act on, as
/// `meta/status_identity` and `meta/member_health` show it until the next
/// successful wake.
///
/// Recorded for the two refusals no retry changes: the last session cannot be
/// resumed (final; the identity's view is `broken`), and Core held the resume
/// for the operator (the durable copy is kept; the view stays `dormant`).
/// Transient refusals are not recorded: the next wake retries them.
/// Process-local: it records what this process's last wake answered and is
/// never read back as truth; every wake asks Core again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WakeRefusal {
    /// The JSON-RPC code the refused wake answered with.
    pub code: i64,
    /// The session the refused wake tried to resume.
    pub session_id: meerkat_core::types::SessionId,
    /// What the operator does next.
    pub remedy: WakeRefusalRemedy,
}

/// A wake that spawned the identity fresh in place of its last session
///: Core answered that no durable session exists for that id, so
/// there was nothing to resume or to lose. Shown on the identity's status
/// until the next wake. Process-local; never read as truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplacedSession {
    /// The last session Core could not find.
    pub session_id: meerkat_core::types::SessionId,
    /// Why it was replaced.
    pub reason: ReplacedSessionReason,
}

/// Why a wake replaced the identity's last session with a fresh one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ReplacedSessionReason {
    /// Core holds no durable session for the id
    /// (`SessionResumeUnavailableReason::Absent`).
    Absent,
}

impl WakeRefusal {
    /// The identity's last session cannot be resumed: only `delete_identity`
    /// starts it again.
    #[must_use]
    pub fn is_final(&self) -> bool {
        self.remedy == WakeRefusalRemedy::DeleteIdentity
    }
}

/// The operator's next step after a refused wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum WakeRefusalRemedy {
    /// Core cannot resume the last session (archived beyond revival, or its
    /// durable authority contradicts its body): `delete_identity` starts a
    /// fresh life.
    DeleteIdentity,
    /// The session's committed document fails its audited-endpoint guard:
    /// repair it (`rkat session repair-wholeblob`), then
    /// `meta/reload_member`.
    RepairSession,
    /// The roster profile moved off the member's durable role: declare the
    /// migration for this activation (`role_migrations`), or restore the
    /// profile.
    ResolveRoleMigration,
    /// Core holds the session's tail or evidence for reconciliation. Core
    /// re-runs its recovery on every wake and has no command that clears the
    /// hold; it stays until the durable evidence changes. The content is
    /// retained; `delete_identity` starts a fresh life and leaves the held
    /// session in Core.
    OperatorReview,
}

// ---------------------------------------------------------------------------
// Member reload + member health (operator verbs, 2026-09-04 production stall)
// ---------------------------------------------------------------------------

/// What a non-destructive member reload found.
///
/// Mirrors meerkat 0.8.34's `MemberReloadOutcome` disposition vocabulary so
/// the wire shape does not move when the verb switches from the
/// retire-then-rematerialize implementation (0.8.33 API) to the mob primitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberReloadDisposition {
    /// The live registration was discarded and the same durable session was
    /// re-materialized from durable truth (same session id, same continuity
    /// generation, same identity alias).
    Discarded,
    /// The registration was not durability-degraded; nothing was discarded.
    /// A success no-op. (Only observable once meerkat exposes durability
    /// state; the 0.8.33 implementation cannot produce it.)
    NotDegraded,
    /// There is no current live registration to reload (the identity is
    /// dormant or uninitialized). Nothing was done; the next send
    /// materializes lazily.
    NotCurrent,
    /// The registration had lost its runtime attachment (a teardown whose
    /// unregister never completed); the reload completed the pending
    /// unregister and re-attached the same durable session.
    Reattached,
}

/// Result of `meta/reload_member` / `IdentityRuntime::reload_member`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberReloadOutcome {
    /// Whether a live registration was actually replaced. `false` for
    /// `not_degraded` and `not_current`.
    pub reloaded: bool,
    pub disposition: MemberReloadDisposition,
    /// The durable session the identity is bound to after the reload. For a
    /// `discarded` reload this is the SAME id as before; a differing id means
    /// meerkat reported the durable snapshot typed-absent and the resume fell
    /// back to a fresh spawn (logged at WARN).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<meerkat_core::types::SessionId>,
    /// The continuity generation after the reload; unchanged by a reload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<ContinuityGeneration>,
}

/// Durability verdict of a member's live runtime registration.
///
/// Wire: `"ready"` or `{"reload_required": {"operation": ..., "reason": ...}}`.
/// Omitted when the bridge cannot establish a verdict. A missing degradation
/// marker alone cannot distinguish a ready registration from an absent one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberDurability {
    Ready,
    ReloadRequired { operation: String, reason: String },
}

/// The class of the most recent delivery failure for an identity, so an
/// operator can read WHY sends fail before spending an admission budget
/// finding out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryErrorClass {
    /// meerkat refused admission: the member needs a cold reload.
    ReloadRequired,
    /// The delivery was refused because the actor loop had an open stall.
    ActorLoopStalled,
    /// The delivery was refused because the actor loop terminated.
    ActorTerminated,
    /// The actor round trip exceeded the admission budget.
    AdmissionTimeout,
    /// The member's per-member admission lane was full (retryable
    /// backpressure, meerkat 0.8.34).
    AdmissionBacklogFull,
    /// The automatic reload was refused: store not healthy yet (retry later).
    ReloadRefused,
    /// The automatic reload timed out at a stage (inspect before retrying).
    ReloadTimedOut,
    /// Any other pre-admission failure.
    AdmissionFailed,
    /// A post-admission failure (the turn ran).
    Completion,
}

/// Outcome class of the most recent reload attempt for an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReloadAttemptOutcome {
    Discarded,
    NotDegraded,
    NotCurrent,
    /// The registration had lost its runtime attachment and was re-attached
    /// to the same durable session.
    Reattached,
    /// meerkat refused: store not healthy yet; registration retained.
    Refused,
    /// meerkat's bounded reload timed out at `detail` (the stage).
    TimedOut,
    /// Any other failure (detail carries the error text).
    Failed,
    /// The durable session needs the sanctioned audited-endpoint repair
    /// before it can be resumed; `data` carries the typed hold and the
    /// member health carries the exact commands. Nothing was discarded.
    RepairRequired,
}

/// The most recent reload attempt (verb or automatic), so an operator can see
/// why a member is still reload-required (for example a refusal naming the
/// unreadable store) without re-running the reload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReloadAttemptRecord {
    pub outcome: ReloadAttemptOutcome,
    /// Reason (refused), stage (timed out) or error text (failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Structured timeout/probe observations; not an execution-fate verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Unix epoch milliseconds when the attempt finished.
    pub at_unix_ms: u64,
}

/// One recorded delivery failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryErrorRecord {
    pub class: DeliveryErrorClass,
    pub detail: String,
    /// Structured observations from the error owner. Missing execution or
    /// retry flags must not be interpreted as a verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Unix epoch milliseconds when the failure was recorded.
    pub at_unix_ms: u64,
}

/// `meta/member_health` response: the identity's lifecycle and delivery
/// health as far as the runtime can observe it WITHOUT touching the mob actor
/// (every field is an in-process read, so this answers while the loop is
/// stalled - which is exactly when it is needed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberHealthReport {
    pub identity: AgentIdentity,
    /// Core's runtime id of the live member, when Core holds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<AgentRuntimeId>,
    pub state: IdentityLifecycleState,
    /// Startup-hydration projection (`warming` while a materialization is in
    /// flight), when the bootstrap roster tracks this identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_state: Option<IdentityBootstrapState>,
    /// True while a materialization for this identity is in flight.
    pub materialization_in_flight: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<meerkat_core::types::SessionId>,
    /// Core's generation of the live member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<meerkat_mob::Generation>,
    /// The most recent delivery failure, cleared by the next successful
    /// delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_delivery_error: Option<DeliveryErrorRecord>,
    /// The most recent reload attempt (verb or automatic) and how it ended;
    /// a `refused` outcome carries meerkat's reason so the operator can judge
    /// store health.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reload: Option<ReloadAttemptRecord>,
    /// The actor-loop probe's current verdict (shared across every member:
    /// the loop is one).
    pub actor_loop: crate::actor_loop_health::ActorLoopHealthReport,
    /// The open `ErrorEvent::ActorLoopStalled { stall_id }`, when a stall is
    /// open. Duplicates `actor_loop.stall_id` for one-field readers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_stall_id: Option<u64>,
    /// Durability verdict of the live registration. Absent until meerkat
    /// exposes it (0.8.34).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability: Option<MemberDurability>,
    /// The last refused wake (see [`IdentityStatus::wake_refusal`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_refusal: Option<WakeRefusal>,
    /// Typed "session needs repair" hold (see
    /// [`IdentityStatus::session_repair_required`]): the exact repair
    /// commands for this member's durable session, present until the
    /// operator repairs it and `meta/reload_member` resumes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_repair_required: Option<SessionRepairRequired>,
}

// ---------------------------------------------------------------------------
// AgentBuildContext + AgentBuildDraft + ExternalToolDef
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct AgentRuntimeServices {
    mob_handle: Option<meerkat_mob::MobHandle>,
}

impl AgentRuntimeServices {
    pub fn new(mob_handle: meerkat_mob::MobHandle) -> Self {
        Self {
            mob_handle: Some(mob_handle),
        }
    }

    pub fn empty() -> Self {
        Self { mob_handle: None }
    }

    pub fn mob_handle(&self) -> Option<meerkat_mob::MobHandle> {
        self.mob_handle.clone()
    }

    pub fn has_mob_handle(&self) -> bool {
        self.mob_handle.is_some()
    }
}

impl std::fmt::Debug for AgentRuntimeServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRuntimeServices")
            .field("mob_handle", &self.mob_handle.is_some())
            .finish()
    }
}

impl PartialEq for AgentRuntimeServices {
    fn eq(&self, other: &Self) -> bool {
        self.mob_handle.is_some() == other.mob_handle.is_some()
    }
}

impl Eq for AgentRuntimeServices {}

/// Read-only context provided to `AgentCustomizer` at build time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentBuildContext {
    pub identity: AgentIdentity,
    pub active_peers: Vec<AgentIdentity>,
    pub managed_edges: Vec<ManagedPeerEdge>,
    #[serde(default, skip)]
    pub runtime_services: AgentRuntimeServices,
}

/// Tool definition for the customizer boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    /// The installed artifact this tool's handler came from, verified against
    /// the host's installed artifacts at build. Attribution only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<meerkat_core::ToolArtifactRef>,
}

/// Mutable draft that `AgentCustomizer` modifies.
#[derive(Clone, Default)]
pub struct LocalExternalToolOverlay {
    dispatcher: Option<Arc<dyn meerkat_core::agent::AgentToolDispatcher>>,
}

impl LocalExternalToolOverlay {
    pub fn new(dispatcher: Arc<dyn meerkat_core::agent::AgentToolDispatcher>) -> Self {
        Self {
            dispatcher: Some(dispatcher),
        }
    }

    pub fn empty() -> Self {
        Self { dispatcher: None }
    }

    pub fn dispatcher(&self) -> Option<Arc<dyn meerkat_core::agent::AgentToolDispatcher>> {
        self.dispatcher.clone()
    }

    pub fn is_some(&self) -> bool {
        self.dispatcher.is_some()
    }
}

impl std::fmt::Debug for LocalExternalToolOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalExternalToolOverlay")
            .field("dispatcher", &self.dispatcher.is_some())
            .finish()
    }
}

impl PartialEq for LocalExternalToolOverlay {
    fn eq(&self, other: &Self) -> bool {
        self.dispatcher.is_some() == other.dispatcher.is_some()
    }
}

impl Eq for LocalExternalToolOverlay {}

/// In-process overlay carrying a host-supplied `CompactionCurator` for an
/// identity's agent build.
///
/// Same shape and reasons as [`LocalExternalToolOverlay`]: a trait object is
/// not `Debug`/`PartialEq`/`Serialize`, and a curator is host code that
/// cannot cross a wire boundary, so the slot is `#[serde(default, skip)]` on
/// [`AgentBuildDraft`] and compares by presence only.
///
/// The ordinary identity-first lowering copies this overlay into Meerkat's
/// in-process `SpawnMemberSpec` carrier for both fresh and resumed builds.
#[derive(Clone, Default)]
pub struct CompactionCuratorOverlay {
    curator: Option<Arc<dyn meerkat_core::compact::CompactionCurator>>,
}

impl CompactionCuratorOverlay {
    pub fn new(curator: Arc<dyn meerkat_core::compact::CompactionCurator>) -> Self {
        Self {
            curator: Some(curator),
        }
    }

    pub fn empty() -> Self {
        Self { curator: None }
    }

    pub fn curator(&self) -> Option<Arc<dyn meerkat_core::compact::CompactionCurator>> {
        self.curator.clone()
    }

    pub fn is_some(&self) -> bool {
        self.curator.is_some()
    }
}

impl std::fmt::Debug for CompactionCuratorOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompactionCuratorOverlay")
            .field("curator", &self.curator.is_some())
            .finish()
    }
}

impl PartialEq for CompactionCuratorOverlay {
    fn eq(&self, other: &Self) -> bool {
        self.curator.is_some() == other.curator.is_some()
    }
}

impl Eq for CompactionCuratorOverlay {}

/// Mutable draft that `AgentCustomizer` modifies.
///
/// `external_tools` remains the serializable SDK/gateway declaration surface.
/// `local_external_tools` is intentionally skipped by serde and is the
/// in-process Rust overlay for apps that can supply a real dispatcher.
///
/// `Eq` is not derived: `provider_params` carries float sampling knobs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentBuildDraft {
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub additional_instructions: Vec<String>,
    #[serde(default)]
    pub labels: std::collections::BTreeMap<String, String>,
    pub app_context: Option<serde_json::Value>,
    #[serde(default)]
    pub external_tools: Vec<ExternalToolDef>,
    #[serde(default, skip)]
    pub local_external_tools: LocalExternalToolOverlay,
    /// Per-identity provider parameter overrides.
    ///
    /// This is meerkat's own `ProviderParamsOverride` — the exact type behind
    /// `AgentBuildConfig.provider_params` — so provider knobs that have no
    /// Meta-local vocabulary (OpenAI `prompt_cache_key` /
    /// `prompt_cache_options` / `prompt_cache_retention`, Anthropic
    /// `cache_control`) are reachable from a profile, gateway or SDK caller.
    /// Reusing the meerkat type inherits its `deny_unknown_fields` ingress:
    /// an unknown or mistyped knob rejects the draft at deserialize instead
    /// of being ferried as untyped JSON and dropped later.
    ///
    /// Optional and `#[serde(default)]`: every profile, persisted draft and
    /// wire payload written before this field existed still deserializes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_params: Option<meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    /// Host-supplied compaction curator for this identity.
    ///
    /// The ordinary identity-first fresh and resume paths lower this into the
    /// matching in-process Meerkat build carrier. Serde-skipped like
    /// `local_external_tools`, so no persisted draft or wire payload changes.
    ///
    /// IN-PROCESS ONLY, and that is a real limit, not a formality: the gateway
    /// customizer round-trips the whole draft through
    /// `serde_json::to_value` / `from_value` (see `provider_params` above), so
    /// a curator set on the far side of that hop is dropped, exactly like
    /// `local_external_tools`. Making this usable from a gateway or SDK
    /// customizer would require a different executable-host registration
    /// contract; this field does not invent one.
    #[serde(default, skip)]
    pub compaction_curator: CompactionCuratorOverlay,
}

// ---------------------------------------------------------------------------
// SessionSnapshot
// ---------------------------------------------------------------------------

/// Opaque wrapper around serialized Meerkat session state.
///
/// Stored and loaded by `ContinuityStore`.
/// Wire format (JSON-RPC): `{ "data": "<base64 string>" }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub data: Vec<u8>,
}

impl Serialize for SessionSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use base64::Engine;
        use serde::ser::SerializeStruct;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&self.data);
        let mut s = serializer.serialize_struct("SessionSnapshot", 1)?;
        s.serialize_field("data", &encoded)?;
        s.end()
    }
}

impl<'de> Deserialize<'de> for SessionSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use base64::Engine;

        #[derive(Deserialize)]
        struct Wrapper {
            data: String,
        }

        let wrapper = Wrapper::deserialize(deserializer)?;
        let data = base64::engine::general_purpose::STANDARD
            .decode(&wrapper.data)
            .map_err(serde::de::Error::custom)?;
        Ok(Self { data })
    }
}

// ---------------------------------------------------------------------------
// RosterContext + TopologyContext
// ---------------------------------------------------------------------------

/// Context passed to `RosterProvider`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterContext {
    pub mob_definition: Option<meerkat_mob::MobDefinition>,
    pub previous_identities: Vec<AgentIdentity>,
}

/// Context passed to `TopologyProvider`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyContext {
    pub roster: Vec<DurableAgentSpec>,
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Error from the continuity store.
#[derive(Debug)]
pub enum ContinuityStoreError {
    #[cfg(feature = "extension-state")]
    UnsupportedAuthority {
        capability: &'static str,
    },
    StaleFencingToken {
        identity: AgentIdentity,
        presented: FencingToken,
        current: FencingToken,
    },
    StaleCheckpointVersion {
        identity: AgentIdentity,
        presented: CheckpointVersion,
        current: CheckpointVersion,
    },
    StaleContinuityGeneration {
        identity: AgentIdentity,
        presented: ContinuityGeneration,
        current: ContinuityGeneration,
    },
    NotFound {
        identity: AgentIdentity,
    },
    Io(String),
    Corruption(String),
    /// Lock contention or interruption at the storage layer: the operation
    /// did not observably complete and MAY be retried. Classification alone
    /// does not authorize a retry — per the shared storage retryability
    /// contract, automatic retry is only sound for idempotent or CAS-keyed
    /// operations (continuity writes are fencing-token CAS and qualify);
    /// an indeterminate non-idempotent write needs outcome reconciliation
    /// first. Retry policy stays with the caller.
    Transient(String),
}

impl fmt::Display for ContinuityStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(feature = "extension-state")]
            Self::UnsupportedAuthority { capability } => {
                write!(f, "continuity authority does not support {capability}")
            }
            Self::StaleFencingToken {
                identity,
                presented,
                current,
            } => write!(
                f,
                "stale fencing token for {identity}: presented {presented}, current {current}"
            ),
            Self::StaleCheckpointVersion {
                identity,
                presented,
                current,
            } => write!(
                f,
                "stale checkpoint version for {identity}: presented {presented}, current {current}"
            ),
            Self::StaleContinuityGeneration {
                identity,
                presented,
                current,
            } => write!(
                f,
                "stale continuity generation for {identity}: presented {presented}, current {current}"
            ),
            Self::NotFound { identity } => {
                write!(f, "continuity record not found for {identity}")
            }
            Self::Io(msg) => write!(f, "continuity store I/O error: {msg}"),
            Self::Corruption(msg) => write!(f, "continuity store corruption: {msg}"),
            Self::Transient(msg) => {
                write!(f, "continuity store transient failure: {msg}")
            }
        }
    }
}

impl std::error::Error for ContinuityStoreError {}

/// Error from the lease provider.
#[derive(Debug)]
pub enum LeaseError {
    ProviderUnavailable(String),
    Io(String),
}

impl fmt::Display for LeaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderUnavailable(msg) => {
                write!(f, "lease provider unavailable: {msg}")
            }
            Self::Io(msg) => write!(f, "lease I/O error: {msg}"),
        }
    }
}

impl std::error::Error for LeaseError {}

/// Error from the roster provider.
#[derive(Debug)]
pub enum RosterError {
    ProviderUnavailable(String),
    Io(String),
    /// The roster for this activation is authoritatively refused (for
    /// example a prepared application's roster projection failed): callers
    /// must not fall back to a stored spec.
    Refused(String),
}

impl fmt::Display for RosterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderUnavailable(msg) => {
                write!(f, "roster provider unavailable: {msg}")
            }
            Self::Io(msg) => write!(f, "roster I/O error: {msg}"),
            Self::Refused(msg) => write!(f, "roster refused: {msg}"),
        }
    }
}

impl std::error::Error for RosterError {}

/// Error from the topology provider.
#[derive(Debug)]
pub enum TopologyError {
    InvalidEdge(String),
    ProviderUnavailable(String),
}

impl fmt::Display for TopologyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEdge(msg) => write!(f, "invalid topology edge: {msg}"),
            Self::ProviderUnavailable(msg) => {
                write!(f, "topology provider unavailable: {msg}")
            }
        }
    }
}

impl std::error::Error for TopologyError {}

/// Error from the agent customizer.
#[derive(Debug)]
pub enum CustomizerError {
    BuildFailed(String),
    Io(String),
    /// The build's callback tools were refused for their installed-artifact
    /// references; nothing of the build was published.
    ToolArtifactRefused(Box<meerkat_core::ToolArtifactRefusal>),
}

impl fmt::Display for CustomizerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BuildFailed(msg) => write!(f, "customizer build failed: {msg}"),
            Self::Io(msg) => write!(f, "customizer I/O error: {msg}"),
            Self::ToolArtifactRefused(refusal) => write!(f, "{refusal}"),
        }
    }
}

impl std::error::Error for CustomizerError {}

#[cfg(test)]
mod completion_cursor_tests {
    use super::*;

    /// The cursor's epoch is Core's fence, which a restart keeps,
    /// while this process counts turns from zero. A baseline under the
    /// current epoch that counts more turns than the cursor came from an
    /// earlier process (or is forged): it reads `IncarnationChanged`, never
    /// `Pending`. A baseline at or below the count behaves as before.
    #[test]
    fn a_baseline_ahead_of_the_count_under_the_same_fence_is_another_incarnation() {
        let fence = FencingToken::new(7);
        let restarted = CompletionCursor::new(fence, 1);
        assert_eq!(
            restarted.progress_since(CompletionCursor::new(fence, 3)),
            CompletionProgress::IncarnationChanged
        );
        assert_eq!(
            restarted.progress_since(restarted),
            CompletionProgress::Pending
        );
        assert_eq!(
            restarted.advanced().progress_since(restarted),
            CompletionProgress::Completed
        );
        assert_eq!(
            restarted.progress_since(CompletionCursor::new(FencingToken::new(6), 0)),
            CompletionProgress::IncarnationChanged
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod bootstrap_deferral_tests {
    use super::*;

    /// A deferred identity leaves the tracked identities: the snapshot
    /// counts it as deferred, never as broken, and it holds neither
    /// readiness nor the terminal barrier. The JSON keeps the deferred list
    /// and count, and omits both when nothing is deferred.
    #[test]
    fn a_deferred_identity_is_not_tracked_and_not_broken() {
        let active = AgentIdentity::parse("agent:active").expect("identity");
        let moved = AgentIdentity::parse("agent:moved").expect("identity");
        let created = AgentIdentity::parse("agent:created").expect("identity");
        let mut status = IdentityBootstrapStatus::empty(IdentityBootstrapMode::EagerMaterialize);
        for (identity, state) in [
            (&active, IdentityBootstrapState::Active),
            (&moved, IdentityBootstrapState::Active),
            (&created, IdentityBootstrapState::Warming),
        ] {
            status.identities.insert(
                identity.clone(),
                IdentityBootstrapEntry {
                    state,
                    error: None,
                    restore: None,
                },
            );
        }
        let untouched = serde_json::to_value(&status).expect("serialize");
        assert!(untouched.get("deferred").is_none(), "{untouched}");
        assert!(untouched["counts"].get("deferred").is_none(), "{untouched}");

        status.defer(&moved, DeferredIdentityReason::MoveRole);
        status.defer(&created, DeferredIdentityReason::CreateMember);
        assert_eq!(
            (
                status.identities.keys().cloned().collect::<Vec<_>>(),
                status.counts.clone(),
                status.ready,
                status.materialization_terminal(),
            ),
            (
                vec![active],
                IdentityBootstrapCounts {
                    active: 1,
                    deferred: 2,
                    ..IdentityBootstrapCounts::default()
                },
                true,
                true,
            )
        );
        let json = serde_json::to_value(&status).expect("serialize");
        assert_eq!(
            (json["deferred"].clone(), json["counts"]["deferred"].clone()),
            (
                serde_json::json!([
                    { "identity": "agent:created", "reason": "create_member" },
                    { "identity": "agent:moved", "reason": "move_role" },
                ]),
                serde_json::json!(2),
            )
        );
        let parsed: IdentityBootstrapStatus = serde_json::from_value(json).expect("parse");
        assert_eq!(parsed, status);
    }
}
