//! Temporary-council wire contracts (issue #159, phase 5 — public surfaces).
//!
//! A *temporary council* seats source-owned forked-participant capabilities as
//! ordinary members of a REAL, short-lived mob, runs a bounded sequential
//! discussion, applies ONE explicit merge-back policy, and tears the mob down.
//! `meerkat-mob(-mcp)` owns every lifecycle decision; this module owns only the
//! canonical wire projection every shipping surface (RPC, REST, CLI, public
//! MCP, SDKs) renders through.
//!
//! # What this module deliberately does not carry
//!
//! * **No capability bearer material.** A council's custody holds a full
//!   [`ForkedParticipantRef`]-shaped capability with a bearer token; the wire
//!   carries only the non-secret provenance projection (owner route, fork
//!   session, source session + prefix digest/count, scope, reuse, expiry, and
//!   the non-secret correlation hint). There is no wire field that could hold a
//!   capability id, bearer token, revocation id, or cleanup id.
//! * **No transcript body.** A seated fork session opens with the inherited
//!   source prefix, so a transcript projection would leak source context the
//!   council never produced. The only text on the wire is the council's OWN
//!   receiver-bounded exchange output and the explicit merge outcome.
//! * **No mutable coordinator internals.** The record projection carries no
//!   store revision, no persisted machine state, and no claim lease: those are
//!   coordinator authority, not a caller-observable contract.
//!
//! Every request type is `deny_unknown_fields`: an unrecognized field is a
//! typed rejection before any mob, capability, or turn exists.
//!
//! Timestamps are RFC 3339 strings (`chrono` renders and parses them at the
//! conversion boundary in `meerkat-mob-mcp`), so this crate stays free of a
//! schema-side chrono dependency.
//!
//! [`ForkedParticipantRef`]: https://docs.rs/meerkat-mob

use serde::{Deserialize, Serialize};

use super::mob::{MobDefinitionInput, WireMobBackendKind};
use super::supervisor_bridge::{WireHostBindingDescriptor, WireOpaqueJson};
use crate::error::ErrorCode;

// ===========================================================================
// Shared vocabulary
// ===========================================================================

/// Whether a council's custody must survive a process restart.
///
/// This is an explicit caller declaration, never an inference. `Durable` is
/// refused when the serving runtime's council store is process-bound;
/// `ProcessBound` is the explicit opt-in that says a process death loses the
/// record and the source capability TTL is the only remaining backstop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireTemporaryCouncilDurability {
    /// Custody must survive process restart; crash recovery is guaranteed.
    Durable,
    /// Custody is process-bound; a process death loses the record.
    ProcessBound,
}

/// Operations a council participant's capability grants its holder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireTemporaryCouncilScope {
    /// Send bounded work into the seated participant.
    Invoke,
    /// Observe the seated participant's output only.
    Observe,
    /// Both invoke and observe.
    InvokeAndObserve,
}

/// How many times one capability may be attached over its lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireTemporaryCouncilReusePolicy {
    /// Exactly one attachment; the capability exhausts on its release.
    OneShot,
    /// A bounded number of sequential attachments.
    BoundedReuse {
        /// Attachment budget.
        max_uses: u32,
    },
}

/// Typed route to the runtime that owns a participant's capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireTemporaryCouncilOwnerRoute {
    /// The source member runs in this runtime, inside the named realm.
    Local {
        /// Realm that owns the source member and the fork.
        realm_id: String,
    },
    /// The source member runs on a bound member host inside the named realm.
    Host {
        /// Realm that owns the source member and the fork.
        realm_id: String,
        /// Typed host identity of the owning runtime host.
        host_id: String,
    },
}

/// Stable discriminant of the caller's merge-back policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireTemporaryCouncilMergePolicyKind {
    /// Bounded text summary.
    BoundedTextSummary,
    /// Strict-JSON structured result.
    StructuredResult,
    /// Selected council exchange sequences.
    SelectedTranscript,
    /// Durable artifact reference claim.
    DurableArtifactReference,
    /// No merge.
    NoMerge,
}

// ===========================================================================
// Request
// ===========================================================================

/// Absolute or relative bound on how long a council may run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireTemporaryCouncilDeadline {
    /// An explicit wall-clock instant, as an RFC 3339 timestamp.
    Absolute {
        /// The instant after which no further work may start or continue.
        at: String,
    },
    /// A duration measured from acceptance, in milliseconds.
    Relative {
        /// How long the council may run, in milliseconds.
        after_millis: u64,
    },
}

/// Bounded budget for one council.
///
/// Every field is validated before any side effect: an over-budget request
/// never creates a mob, a capability, or a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireTemporaryCouncilBounds {
    /// Absolute or relative deadline.
    pub deadline: WireTemporaryCouncilDeadline,
    /// Maximum number of sequential rounds.
    pub max_rounds: u32,
    /// Maximum number of individual participant exchanges across all rounds.
    pub max_exchanges: u32,
    /// Receiver bound applied to each exchange result, in bytes.
    pub max_result_bytes: u64,
}

/// One council participant: which source member is forked, and how the branch
/// is seated in the temporary mob.
///
/// There is deliberately no credential, auth override, or mutable session state
/// here. Tool, auth, realm, and filesystem boundaries stay those of the source
/// execution context, which is exactly what the capability layer guarantees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireTemporaryCouncilParticipant {
    /// Deterministic slot order, also the turn order within a round.
    pub order: u32,
    /// Deterministic role label carried into provenance and prompts.
    pub role: String,
    /// Mob that owns the source member.
    pub source_mob_id: String,
    /// Source member identity.
    pub source_identity: String,
    /// Identity the branch is seated under in the temporary mob.
    pub target_identity: String,
    /// Profile in the caller's definition template the branch is seated from.
    pub target_profile: String,
    /// Optional portable backend override for the seated member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_backend: Option<WireMobBackendKind>,
    /// Complete-boundary prefix length to fork; omitted selects the head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_message_count: Option<u64>,
    /// Granted operation scope. Must admit invocation and observation.
    pub scope: WireTemporaryCouncilScope,
}

/// The typed contract a `structured_result` merge must satisfy.
///
/// A council does not accept "any syntactically valid JSON" as a structured
/// result: the caller declares an identity, a version, and a JSON Schema, and
/// the finalizer's output is validated against it before the result is sealed.
/// Schema compilation stays a coordinator preflight — the wire carries the
/// declared schema, never a compiled or trusted one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireTemporaryCouncilStructuredContract {
    /// Caller-stable contract identity.
    pub schema_id: String,
    /// Caller-declared contract version.
    pub schema_version: u32,
    /// The JSON Schema the finalizer's output is validated against.
    pub json_schema: WireOpaqueJson,
}

/// The single explicit merge-back policy for one council.
///
/// No variant merges a whole transcript, and no variant mutates the caller's
/// session: the outcome is RETURNED, never written back implicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "policy", rename_all = "snake_case", deny_unknown_fields)]
pub enum WireTemporaryCouncilMergeBack {
    /// One final bounded turn on `finalizer` asking for a prose summary.
    BoundedTextSummary {
        /// Participant asked to produce the summary.
        finalizer: String,
        /// Receiver bound applied to that final turn, in bytes.
        max_bytes: u64,
    },
    /// One final bounded turn on `finalizer` whose output is parsed as STRICT
    /// JSON and then validated against the caller's declared contract.
    StructuredResult {
        /// Participant asked to produce the value.
        finalizer: String,
        /// The contract the value must satisfy.
        contract: WireTemporaryCouncilStructuredContract,
        /// Receiver bound applied to that final turn, in bytes.
        max_bytes: u64,
    },
    /// Explicitly selected COUNCIL EXCHANGE SEQUENCES from one participant.
    ///
    /// The selection domain is the council's own bounded exchange receipts,
    /// never the seated fork session's transcript: a raw transcript index
    /// could name an inherited source-prefix message the council never
    /// produced.
    SelectedTranscript {
        /// Participant whose council exchanges are selected.
        participant: String,
        /// Exact council exchange sequences to select.
        exchange_sequences: Vec<u32>,
        /// Total byte cap across the whole selection.
        max_bytes: u64,
    },
    /// One final bounded turn whose output is parsed as a typed, UNVERIFIED
    /// artifact claim. The council performs no store lookup or fetch.
    DurableArtifactReference {
        /// Participant asked to produce the claim.
        participant: String,
        /// Receiver bound applied to that final turn, in bytes.
        max_bytes: u64,
    },
    /// Observation only: provenance and confirmation, no content.
    NoMerge,
}

/// One complete temporary-council request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireTemporaryCouncilRequest {
    /// Caller-stable council identity. The same id plus the same request is a
    /// retry; the same id plus a different request is a conflict.
    pub council_id: String,
    /// Caller-supplied explicit definition template for the temporary mob.
    ///
    /// Its `id` is REPLACED with the council's deterministic temporary mob id
    /// so a retry after a crash finds the same real mob instead of creating a
    /// second one. Everything else is validated through the ordinary public
    /// mob-definition decoder and used verbatim.
    pub definition_template: MobDefinitionInput,
    /// Participants, in caller-declared order.
    pub participants: Vec<WireTemporaryCouncilParticipant>,
    /// Initial topic/prompt for round 0.
    pub topic: String,
    /// Bounded budget.
    pub bounds: WireTemporaryCouncilBounds,
    /// The single explicit merge-back policy.
    pub merge_back: WireTemporaryCouncilMergeBack,
    /// Whether this council's custody must survive a process restart.
    pub durability: WireTemporaryCouncilDurability,
}

/// Request payload for `mob/temporary_council_run`.
///
/// `host_bindings` is deliberately OUTSIDE the request: a host binding
/// descriptor carries a one-time ceremony token, so folding it into the request
/// would make an honest retry present a different fingerprint and would put
/// credential-like material into the durable council record. Nothing in it is
/// fingerprinted or persisted, and a replay or joined caller ignores it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobTemporaryCouncilRunParams {
    /// The council request.
    pub request: WireTemporaryCouncilRequest,
    /// One-time host bootstrap for the hosts this council must reach before
    /// any HOST-owned participant can be seated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_bindings: Vec<WireHostBindingDescriptor>,
}

/// Request payload for `mob/temporary_council_get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MobTemporaryCouncilGetParams {
    /// Council identity to read.
    pub council_id: String,
}

// ===========================================================================
// Result
// ===========================================================================

/// Why the council stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum WireTemporaryCouncilExitReason {
    /// The configured round schedule ran to completion within bounds.
    Completed,
    /// The exchange budget cut the round schedule short.
    MaxExchangesReached,
    /// The absolute deadline elapsed.
    DeadlineExceeded,
    /// One participant could not be seated.
    ParticipantSeatingFailed {
        /// Slot that failed.
        participant_order: u32,
        /// Typed detail.
        detail: String,
    },
    /// The requested topology could not be fully wired.
    WiringIncomplete {
        /// Typed detail, including the observed wiring report.
        detail: String,
    },
    /// One exchange failed terminally.
    ExchangeFailed {
        /// Round the failure occurred in.
        round: u32,
        /// Identity the turn was delivered to.
        target_identity: String,
        /// Typed detail.
        detail: String,
    },
    /// The coordinator process died before committing a result.
    ///
    /// Recovery commits this reason rather than silently re-executing: a re-run
    /// would duplicate model work and result delivery.
    CoordinatorInterrupted,
}

/// Terminal classification of one bounded exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WireTemporaryCouncilExchangeOutcome {
    /// The delivery identity is durably recorded but no terminal was observed.
    Pending,
    /// The exchange committed an exact receiver-bounded turn.
    Completed {
        /// Receiver-bounded compact text the participant produced IN THIS
        /// COUNCIL. Never an inherited source-prefix message.
        text: String,
        /// Whether the receiver bound truncated the member's output.
        truncated: bool,
        /// Session the committed turn belongs to.
        session_id: String,
        /// When the terminal was observed (RFC 3339).
        completed_at: String,
    },
    /// The exchange failed, timed out, or was cancelled.
    Failed {
        /// Typed detail, already bounded.
        detail: String,
        /// When the failure was observed (RFC 3339).
        failed_at: String,
    },
}

/// Durable receipt of one participant turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilExchange {
    /// Zero-based round index.
    pub round: u32,
    /// Monotonic exchange sequence across the whole council.
    pub sequence: u32,
    /// Participant slot order.
    pub participant_order: u32,
    /// Identity the turn was delivered to.
    pub target_identity: String,
    /// Deterministic delivery idempotency key persisted BEFORE the send.
    pub delivery_idempotency_key: String,
    /// Deterministic delivery correlation id persisted BEFORE the send.
    pub delivery_correlation_id: String,
    /// When the send was recorded (RFC 3339).
    pub started_at: String,
    /// Terminal classification.
    pub outcome: WireTemporaryCouncilExchangeOutcome,
}

/// Exact selected-prefix provenance of one fork.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilSourceProvenance {
    /// Session the fork was taken from.
    pub source_session_id: String,
    /// Number of source messages selected into the child.
    pub prefix_message_count: u64,
    /// Content digest of the selected prefix.
    pub prefix_digest: String,
}

/// Non-secret provenance of the exact capability one participant was seated
/// under.
///
/// Deliberately carries NO capability bearer token, capability id, revocation
/// id, or cleanup id: the bearer stays in coordinator custody and in the
/// capability store, never in a returned result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilCapabilityProvenance {
    /// Typed route to the runtime that owns the capability.
    pub owner_route: WireTemporaryCouncilOwnerRoute,
    /// Fork session the seated branch resumed.
    pub fork_session_id: String,
    /// Exact source transcript provenance.
    pub source: WireTemporaryCouncilSourceProvenance,
    /// Operations the holder was granted.
    pub scope: WireTemporaryCouncilScope,
    /// Reuse policy the capability was minted under.
    pub reuse: WireTemporaryCouncilReusePolicy,
    /// Absolute expiry of the capability (RFC 3339).
    pub expires_at: String,
    /// Non-secret correlation hint of the capability.
    pub correlation_hint: String,
}

/// Non-secret provenance of one participant, carried in the immutable result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilParticipantProvenance {
    /// Deterministic slot order.
    pub order: u32,
    /// Deterministic role label.
    pub role: String,
    /// Mob that owned the source member.
    pub source_mob_id: String,
    /// Source member identity.
    pub source_identity: String,
    /// Identity the participant was seated under.
    pub target_identity: String,
    /// Granted operation scope.
    pub scope: WireTemporaryCouncilScope,
    /// Deterministic capability request id (non-secret idempotency
    /// correlation, not custody).
    pub capability_request_id: String,
    /// Exact non-secret capability provenance, once a capability was acquired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<WireTemporaryCouncilCapabilityProvenance>,
    /// Attachment id the temporary mob held.
    pub attachment_id: String,
    /// Whether the participant was actually seated.
    pub seated: bool,
}

/// A participant's UNVERIFIED, typed claim about an artifact it says it
/// produced. The council performs no store lookup, fetch, or existence check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilArtifactClaim {
    /// Participant-reported location. Not resolved or validated.
    pub uri: String,
    /// Optional media type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Optional content digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Optional byte length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_len: Option<u64>,
}

/// One selected council exchange, with its own provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilSelectedExchange {
    /// Council-wide exchange sequence.
    pub sequence: u32,
    /// Round the exchange belongs to.
    pub round: u32,
    /// Participant slot that produced it.
    pub participant_order: u32,
    /// Identity that produced it.
    pub target_identity: String,
    /// Bounded excerpt of the exchange's own receiver-bounded text.
    pub text: String,
    /// Whether this excerpt was cut by the selection's total byte cap.
    pub truncated: bool,
}

/// Non-secret identity of the structured-result contract a value satisfied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilStructuredContractIdentity {
    /// Caller-stable contract identity.
    pub schema_id: String,
    /// Caller-declared contract version.
    pub schema_version: u32,
    /// Digest of the exact JSON Schema the value was validated against.
    pub schema_digest: String,
}

/// What the explicit merge-back policy actually produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireTemporaryCouncilMergeOutcome {
    /// Observation only: provenance and confirmation, no content.
    NoMerge {
        /// Participants confirmed seated at merge time.
        confirmed_participants: Vec<String>,
    },
    /// One bounded natural-language summary from the named finalizer.
    BoundedTextSummary {
        /// Participant that produced the summary.
        finalizer: String,
        /// Receiver-bounded summary text.
        text: String,
        /// Whether the receiver bound truncated the summary.
        truncated: bool,
    },
    /// One strict-JSON structured result, validated against the declared
    /// contract.
    StructuredResult {
        /// Participant that produced the value.
        finalizer: String,
        /// Contract the value was validated against.
        contract: WireTemporaryCouncilStructuredContractIdentity,
        /// Strictly parsed and schema-valid JSON value.
        value: WireOpaqueJson,
        /// Whether the receiver bound truncated the raw output before parsing.
        truncated: bool,
    },
    /// Explicitly selected council exchanges, by their exchange sequence.
    SelectedTranscript {
        /// Participant whose council exchanges were selected.
        participant: String,
        /// The exact exchange sequences that were requested and found.
        exchange_sequences: Vec<u32>,
        /// One bounded excerpt per returned sequence, in the same order.
        excerpts: Vec<WireTemporaryCouncilSelectedExchange>,
        /// Whether the total byte cap truncated the selection.
        truncated: bool,
    },
    /// A typed, participant-reported artifact reference. UNVERIFIED.
    DurableArtifactReference {
        /// Participant that produced the claim.
        participant: String,
        /// Parsed typed claim.
        claim: WireTemporaryCouncilArtifactClaim,
    },
    /// The council terminated before merge was attempted.
    NotAttempted {
        /// Why the merge never ran.
        reason: String,
    },
    /// The merge ran and failed in a typed, non-fatal way.
    Failed {
        /// Which policy failed.
        policy: WireTemporaryCouncilMergePolicyKind,
        /// Typed detail.
        detail: String,
    },
}

/// Immutable result of one temporary council.
///
/// Sealed BEFORE cleanup runs, so it stays valid even when cleanup later fails.
/// Cleanup status is reported separately and never folded into this value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilResult {
    /// Council identity.
    pub council_id: String,
    /// Canonical fingerprint of the request that produced this result.
    pub request_fingerprint: String,
    /// The real temporary mob the council ran in.
    pub temporary_mob_id: String,
    /// Why the council stopped.
    pub exit_reason: WireTemporaryCouncilExitReason,
    /// Number of rounds that completed at least one exchange.
    pub rounds_completed: u32,
    /// Every bounded exchange receipt, in order.
    pub exchanges: Vec<WireTemporaryCouncilExchange>,
    /// Explicit merge-back outcome.
    pub merge: WireTemporaryCouncilMergeOutcome,
    /// Non-secret provenance for each participant slot.
    pub participants: Vec<WireTemporaryCouncilParticipantProvenance>,
    /// Number of exchanges whose text was truncated by the receiver bound.
    pub truncated_exchange_count: u32,
    /// Whether the merge output was truncated by its own byte cap.
    pub merge_truncated: bool,
    /// The durability the caller declared and the store honoured.
    pub durability: WireTemporaryCouncilDurability,
    /// When the result was sealed (RFC 3339).
    pub concluded_at: String,
}

/// One unpaid cleanup obligation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilCleanupDebt {
    /// What could not be cleaned (mob id, participant slot, capability id).
    pub subject: String,
    /// Typed detail of the failure.
    pub detail: String,
}

/// How a cleanup attempt ended, as a single typed verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireTemporaryCouncilCleanupStatus {
    /// Every obligation was discharged.
    Settled,
    /// Cleanup ran to completion and retained typed debt.
    Debt,
    /// The bounded cleanup budget expired with work still outstanding.
    Pending,
}

/// Durable receipt of one cleanup attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilCleanup {
    /// Typed verdict for this attempt.
    pub status: WireTemporaryCouncilCleanupStatus,
    /// When this attempt ran (RFC 3339).
    pub attempted_at: String,
    /// Number of cleanup attempts including this one.
    pub attempts: u32,
    /// Whether the temporary mob is gone.
    pub temporary_mob_destroyed: bool,
    /// Participant slots whose seated member was retired.
    pub released_participants: Vec<u32>,
    /// Participant slots whose acquired-but-unattached capability was
    /// explicitly revoked.
    pub revoked_participants: Vec<u32>,
    /// Outstanding obligations. Empty means the attempt fully converged.
    pub debts: Vec<WireTemporaryCouncilCleanupDebt>,
    /// Whether the bounded cleanup budget expired with work outstanding.
    pub budget_exhausted: bool,
}

/// Response payload for `mob/temporary_council_run`.
///
/// The immutable result and the cleanup verdict are reported separately: a
/// sealed result stays valid even when its cleanup retained debt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobTemporaryCouncilRunResult {
    /// The immutable council result.
    pub result: WireTemporaryCouncilResult,
    /// The most recent cleanup receipt.
    pub cleanup: WireTemporaryCouncilCleanup,
    /// Whether this outcome was replayed from durable custody rather than
    /// produced by this call.
    pub replayed: bool,
}

// ===========================================================================
// Record projection
// ===========================================================================

/// How far capability acquisition got for one participant slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireTemporaryCouncilAcquisition {
    /// No create call has been issued for this slot.
    NotAttempted,
    /// The create call was issued; its outcome is not durably known.
    Pending,
    /// The exact capability reference is durably held.
    Acquired,
    /// A capability WAS returned but its custody commit did not succeed.
    Ambiguous,
}

/// Non-secret projection of one participant custody slot.
///
/// The durable record additionally holds the FULL capability reference,
/// including its bearer token. That field has no wire representation here and
/// never reaches a caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilParticipantCustody {
    /// Deterministic slot order.
    pub order: u32,
    /// Deterministic role label.
    pub role: String,
    /// Mob that owns the source member being forked.
    pub source_mob_id: String,
    /// Source member identity.
    pub source_identity: String,
    /// Identity this participant is seated under in the temporary mob.
    pub target_identity: String,
    /// Profile the branch is seated from.
    pub target_profile: String,
    /// Granted operation scope.
    pub scope: WireTemporaryCouncilScope,
    /// Deterministic capability request id.
    pub capability_request_id: String,
    /// Non-secret correlation hint of the acquired capability, once created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_correlation_hint: Option<String>,
    /// Deterministic attachment id used for the attached spawn.
    pub attachment_id: String,
    /// How far capability acquisition got for this slot.
    pub acquisition: WireTemporaryCouncilAcquisition,
    /// Whether the attached spawn was durably observed to succeed.
    pub seated: bool,
    /// Fork session the seated member resumed, once seated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seated_session_id: Option<String>,
}

/// Sealed projection of one durable council record.
///
/// Carries no store revision, no persisted machine state, and no coordinator
/// claim lease: those are coordinator authority, not caller-observable state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilRecord {
    /// Council identity.
    pub council_id: String,
    /// Canonical fingerprint of the accepted request.
    pub request_fingerprint: String,
    /// Deterministic temporary mob this council owns.
    pub temporary_mob_id: String,
    /// Absolute deadline computed once, before any work (RFC 3339).
    pub deadline: String,
    /// Durability the caller declared for this council.
    pub durability: WireTemporaryCouncilDurability,
    /// Whether the canonical lifecycle machine still owes this record work.
    pub unfinished: bool,
    /// Per-participant custody projection in deterministic slot order.
    pub participants: Vec<WireTemporaryCouncilParticipantCustody>,
    /// Ordered exchange receipts.
    pub exchanges: Vec<WireTemporaryCouncilExchange>,
    /// The immutable result, once sealed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<WireTemporaryCouncilResult>,
    /// The most recent cleanup attempt's receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup: Option<WireTemporaryCouncilCleanup>,
    /// Creation instant (RFC 3339).
    pub created_at: String,
    /// Last mutation instant (RFC 3339).
    pub updated_at: String,
}

/// Response payload for `mob/temporary_council_get`.
///
/// `council` is absent when no record is bound to the requested id. An unknown
/// council is an ordinary absence, not an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobTemporaryCouncilGetResult {
    /// The sealed record projection, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub council: Option<WireTemporaryCouncilRecord>,
}

/// What one recovery sweep did to one unfinished council.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WireTemporaryCouncilRecoveryReport {
    /// Council that was recovered.
    pub council_id: String,
    /// Whether this sweep sealed a terminal interrupted result.
    pub sealed_interrupted_result: bool,
    /// Whether the record is now fully settled.
    pub settled: bool,
    /// The cleanup receipt this sweep committed.
    pub cleanup: WireTemporaryCouncilCleanup,
}

/// Response payload for `mob/temporary_council_recover`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MobTemporaryCouncilRecoverResult {
    /// One report per unfinished council this sweep converged.
    pub reports: Vec<WireTemporaryCouncilRecoveryReport>,
}

// ===========================================================================
// Errors
// ===========================================================================

/// Which coordinator-side failure a [`WireTemporaryCouncilFailureDetail`]
/// reports.
///
/// The JSON-RPC/HTTP code alone cannot separate a refused request from a
/// custody write failure once several causes share one code, so the `kind`
/// discriminant is carried in `data` and is the stable machine-readable fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum WireTemporaryCouncilFailureKind {
    /// The request was rejected before any side effect.
    InvalidRequest,
    /// Durable council custody could not be read or written.
    Store,
    /// The canonical council lifecycle machine refused a command, or the
    /// persisted machine state could not be recovered.
    Lifecycle,
    /// A mob operation failed in a way that prevented orchestration.
    Mob,
    /// The owned execution task ended without publishing an outcome.
    CoordinatorUnavailable,
}

/// Typed `data` payload for the string-detail council failures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireTemporaryCouncilFailureDetail {
    /// Which coordinator-side failure this is.
    pub kind: WireTemporaryCouncilFailureKind,
    /// Typed detail from the coordinator.
    pub detail: String,
}

/// Typed `data` payload for a council id bound to a materially different
/// request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireTemporaryCouncilConflictDetail {
    /// The bound council id.
    pub council_id: String,
    /// Fingerprint durably bound to the id.
    pub stored_fingerprint: String,
    /// Fingerprint of the presented request.
    pub presented_fingerprint: String,
}

/// Typed `data` payload for a contested or superseded coordinator claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireTemporaryCouncilClaimDetail {
    /// The contested council id.
    pub council_id: String,
    /// Claim epoch currently recorded by the canonical lifecycle machine.
    pub current_claim_epoch: u64,
}

/// Typed `data` payload for a durability declaration the runtime cannot meet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WireTemporaryCouncilDurabilityDetail {
    /// The refused council id.
    pub council_id: String,
    /// Durability the caller declared.
    pub required: WireTemporaryCouncilDurability,
    /// Durability this runtime's council store actually provides.
    pub available: WireTemporaryCouncilDurability,
}

/// Closed pairing of every temporary-council failure with its wire
/// [`ErrorCode`] and typed detail payload.
///
/// Deliberately NOT serde-derived: the enum itself is never serialized. It
/// exists so the code↔detail pairing has exactly one compile-forced owner.
/// [`Self::code`] is exhaustive over the variants and [`Self::detail_value`]
/// serializes the BARE inner struct, matching the `WireMobErrorDetail`
/// precedent every console surface already consumes.
///
/// Cleanup debt is deliberately NOT an error: a council whose result sealed
/// and whose cleanup retained debt is a SUCCESS carrying
/// [`WireTemporaryCouncilCleanupStatus::Debt`] or `Pending`. Folding it into an
/// error would discard the immutable result the caller is owed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireTemporaryCouncilErrorDetail {
    /// Refused before any mob, capability, or turn existed.
    InvalidRequest(WireTemporaryCouncilFailureDetail),
    /// The council id is bound to a materially different request.
    ConflictingRequest(WireTemporaryCouncilConflictDetail),
    /// Another coordinator holds a live claim whose lease has not expired.
    HeldByAnotherCoordinator(WireTemporaryCouncilClaimDetail),
    /// This executor's claim was superseded; it may not continue or seal.
    Fenced(WireTemporaryCouncilClaimDetail),
    /// The caller declared durable custody this runtime cannot provide.
    DurabilityUnavailable(WireTemporaryCouncilDurabilityDetail),
    /// A coordinator-side failure (store, lifecycle, mob, or an owned task
    /// that ended without publishing).
    Coordinator(WireTemporaryCouncilFailureDetail),
}

impl WireTemporaryCouncilErrorDetail {
    /// The wire [`ErrorCode`] paired with this detail shape. Exhaustive by
    /// construction — adding a variant forces a new arm.
    pub const fn code(&self) -> ErrorCode {
        match self {
            // Shape rejection before any side effect.
            Self::InvalidRequest(_) => ErrorCode::InvalidParams,
            // Same idempotency key, different content: the existing
            // duplicate-input conflict class (409), not a fresh code.
            Self::ConflictingRequest(_) => ErrorCode::DuplicateInput,
            // A live claim is a busy resource, resolvable by retrying later.
            Self::HeldByAnotherCoordinator(_) => ErrorCode::SessionBusy,
            // A superseded claim epoch is exactly the stale-fence class.
            Self::Fenced(_) => ErrorCode::StaleFence,
            // The runtime cannot provide the declared capability.
            Self::DurabilityUnavailable(_) => ErrorCode::CapabilityUnavailable,
            // Custody/lifecycle/mob/task failures are server-side faults; the
            // `kind` discriminant in `data` separates them.
            Self::Coordinator(_) => ErrorCode::InternalError,
        }
    }

    /// Serialize the BARE inner detail struct (never an enum envelope).
    pub fn detail_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        match self {
            Self::InvalidRequest(detail) | Self::Coordinator(detail) => {
                serde_json::to_value(detail)
            }
            Self::ConflictingRequest(detail) => serde_json::to_value(detail),
            Self::HeldByAnotherCoordinator(detail) | Self::Fenced(detail) => {
                serde_json::to_value(detail)
            }
            Self::DurabilityUnavailable(detail) => serde_json::to_value(detail),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request_json() -> serde_json::Value {
        json!({
            "council_id": "demo",
            "definition_template": {
                "id": "ignored",
                "profiles": {},
            },
            "participants": [{
                "order": 0,
                "role": "critic",
                "source_mob_id": "source",
                "source_identity": "alice",
                "target_identity": "alice-branch",
                "target_profile": "council",
                "scope": "invoke_and_observe",
            }],
            "topic": "should we ship?",
            "bounds": {
                "deadline": { "kind": "relative", "after_millis": 60_000 },
                "max_rounds": 1,
                "max_exchanges": 4,
                "max_result_bytes": 4096,
            },
            "merge_back": { "policy": "no_merge" },
            "durability": "process_bound",
        })
    }

    #[test]
    fn run_params_round_trip_without_host_bindings() {
        let params: MobTemporaryCouncilRunParams =
            serde_json::from_value(json!({ "request": request_json() })).expect("decode");
        assert!(params.host_bindings.is_empty());
        let encoded = serde_json::to_value(&params).expect("encode");
        assert!(
            encoded.get("host_bindings").is_none(),
            "an empty bootstrap must not be serialized"
        );
        let decoded: MobTemporaryCouncilRunParams =
            serde_json::from_value(encoded).expect("re-decode");
        assert_eq!(decoded, params);
    }

    #[test]
    fn unknown_request_fields_fail_closed() {
        let mut body = request_json();
        body["surprise"] = json!(true);
        let error = serde_json::from_value::<WireTemporaryCouncilRequest>(body)
            .expect_err("unknown fields must be refused");
        assert!(
            error.to_string().contains("surprise"),
            "rejection must name the offending field: {error}"
        );
    }

    #[test]
    fn unknown_run_params_fields_fail_closed() {
        assert!(
            serde_json::from_value::<MobTemporaryCouncilRunParams>(json!({
                "request": request_json(),
                "capability_bearer": "secret",
            }))
            .is_err(),
            "an injected bearer field must be refused, not ignored"
        );
    }

    #[test]
    fn deadline_and_merge_variants_are_tagged() {
        let absolute: WireTemporaryCouncilDeadline =
            serde_json::from_value(json!({ "kind": "absolute", "at": "2026-01-01T00:00:00Z" }))
                .expect("absolute deadline");
        assert_eq!(
            absolute,
            WireTemporaryCouncilDeadline::Absolute {
                at: "2026-01-01T00:00:00Z".to_string()
            }
        );
        let merge: WireTemporaryCouncilMergeBack = serde_json::from_value(json!({
            "policy": "selected_transcript",
            "participant": "alice-branch",
            "exchange_sequences": [0, 2],
            "max_bytes": 1024,
        }))
        .expect("selected transcript");
        match merge {
            WireTemporaryCouncilMergeBack::SelectedTranscript {
                exchange_sequences, ..
            } => assert_eq!(exchange_sequences, vec![0, 2]),
            other => panic!("unexpected merge policy: {other:?}"),
        }
    }

    #[test]
    fn structured_contract_carries_opaque_schema() {
        let contract = WireTemporaryCouncilStructuredContract {
            schema_id: "verdict".to_string(),
            schema_version: 1,
            json_schema: WireOpaqueJson::from_value(&json!({ "type": "object" })),
        };
        let encoded = serde_json::to_value(&contract).expect("encode");
        assert_eq!(encoded["json_schema"], json!(r#"{"type":"object"}"#));
        let decoded: WireTemporaryCouncilStructuredContract =
            serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded, contract);
    }

    #[test]
    fn error_details_pair_codes_with_bare_payloads() {
        let invalid =
            WireTemporaryCouncilErrorDetail::InvalidRequest(WireTemporaryCouncilFailureDetail {
                kind: WireTemporaryCouncilFailureKind::InvalidRequest,
                detail: "max_rounds must be greater than zero".to_string(),
            });
        let conflict = WireTemporaryCouncilErrorDetail::ConflictingRequest(
            WireTemporaryCouncilConflictDetail {
                council_id: "demo".to_string(),
                stored_fingerprint: "sha256:a".to_string(),
                presented_fingerprint: "sha256:b".to_string(),
            },
        );
        let busy = WireTemporaryCouncilErrorDetail::HeldByAnotherCoordinator(
            WireTemporaryCouncilClaimDetail {
                council_id: "demo".to_string(),
                current_claim_epoch: 3,
            },
        );
        let fenced = WireTemporaryCouncilErrorDetail::Fenced(WireTemporaryCouncilClaimDetail {
            council_id: "demo".to_string(),
            current_claim_epoch: 4,
        });
        let durability = WireTemporaryCouncilErrorDetail::DurabilityUnavailable(
            WireTemporaryCouncilDurabilityDetail {
                council_id: "demo".to_string(),
                required: WireTemporaryCouncilDurability::Durable,
                available: WireTemporaryCouncilDurability::ProcessBound,
            },
        );
        let coordinator =
            WireTemporaryCouncilErrorDetail::Coordinator(WireTemporaryCouncilFailureDetail {
                kind: WireTemporaryCouncilFailureKind::Store,
                detail: "custody write failed".to_string(),
            });

        for (detail, code, jsonrpc) in [
            (&invalid, ErrorCode::InvalidParams, -32602),
            (&conflict, ErrorCode::DuplicateInput, -32004),
            (&busy, ErrorCode::SessionBusy, -32002),
            (&fenced, ErrorCode::StaleFence, -32028),
            (&durability, ErrorCode::CapabilityUnavailable, -32020),
            (&coordinator, ErrorCode::InternalError, -32603),
        ] {
            assert_eq!(detail.code(), code);
            assert_eq!(detail.code().jsonrpc_code(), jsonrpc);
        }

        assert_eq!(
            conflict.detail_value().expect("conflict detail"),
            json!({
                "council_id": "demo",
                "stored_fingerprint": "sha256:a",
                "presented_fingerprint": "sha256:b",
            })
        );
        assert_eq!(
            coordinator.detail_value().expect("coordinator detail"),
            json!({ "kind": "store", "detail": "custody write failed" })
        );
        assert_eq!(
            durability.detail_value().expect("durability detail"),
            json!({
                "council_id": "demo",
                "required": "durable",
                "available": "process_bound",
            })
        );
    }

    /// The EMITTED schema — the artifact SDK codegen and every published
    /// client read — must have no property that could carry capability
    /// bearer material or a seated session's inherited transcript.
    #[cfg(feature = "schema")]
    #[test]
    fn emitted_schemas_declare_no_bearer_or_session_body_property() {
        fn property_names(value: &serde_json::Value, into: &mut Vec<String>) {
            match value {
                serde_json::Value::Object(map) => {
                    if let Some(serde_json::Value::Object(properties)) = map.get("properties") {
                        into.extend(properties.keys().cloned());
                    }
                    for nested in map.values() {
                        property_names(nested, into);
                    }
                }
                serde_json::Value::Array(items) => {
                    for nested in items {
                        property_names(nested, into);
                    }
                }
                _ => {}
            }
        }

        let schemas = [
            (
                "MobTemporaryCouncilRunResult",
                serde_json::to_value(schemars::schema_for!(MobTemporaryCouncilRunResult))
                    .expect("run result schema"),
            ),
            (
                "MobTemporaryCouncilGetResult",
                serde_json::to_value(schemars::schema_for!(MobTemporaryCouncilGetResult))
                    .expect("get result schema"),
            ),
            (
                "MobTemporaryCouncilRunParams",
                serde_json::to_value(schemars::schema_for!(MobTemporaryCouncilRunParams))
                    .expect("run params schema"),
            ),
        ];
        for (label, schema) in schemas {
            let mut names = Vec::new();
            property_names(&schema, &mut names);
            assert!(!names.is_empty(), "{label} must declare properties");
            for name in &names {
                let lowered = name.to_ascii_lowercase();
                for forbidden in [
                    "bearer",
                    "capability_id",
                    "revocation_id",
                    "cleanup_id",
                    "capability_ref",
                    "transcript",
                ] {
                    assert!(
                        !lowered.contains(forbidden),
                        "{label} schema declares `{name}`, which could carry `{forbidden}`"
                    );
                }
                assert_ne!(
                    lowered, "messages",
                    "{label} schema must not declare a transcript body"
                );
            }
        }
    }

    #[test]
    fn result_schema_has_no_bearer_or_transcript_fields() {
        let result = MobTemporaryCouncilRunResult {
            result: WireTemporaryCouncilResult {
                council_id: "demo".to_string(),
                request_fingerprint: "sha256:0".to_string(),
                temporary_mob_id: "council--demo".to_string(),
                exit_reason: WireTemporaryCouncilExitReason::Completed,
                rounds_completed: 1,
                exchanges: Vec::new(),
                merge: WireTemporaryCouncilMergeOutcome::NoMerge {
                    confirmed_participants: vec!["alice-branch".to_string()],
                },
                participants: Vec::new(),
                truncated_exchange_count: 0,
                merge_truncated: false,
                durability: WireTemporaryCouncilDurability::ProcessBound,
                concluded_at: "2026-01-01T00:00:00Z".to_string(),
            },
            cleanup: WireTemporaryCouncilCleanup {
                status: WireTemporaryCouncilCleanupStatus::Settled,
                attempted_at: "2026-01-01T00:00:00Z".to_string(),
                attempts: 1,
                temporary_mob_destroyed: true,
                released_participants: vec![0],
                revoked_participants: Vec::new(),
                debts: Vec::new(),
                budget_exhausted: false,
            },
            replayed: false,
        };
        let encoded = serde_json::to_string(&result).expect("encode");
        for forbidden in [
            "bearer",
            "capability_id",
            "revocation_id",
            "cleanup_id",
            "messages",
            "transcript",
        ] {
            assert!(
                !encoded.contains(forbidden),
                "council result must not carry `{forbidden}`: {encoded}"
            );
        }
    }
}
