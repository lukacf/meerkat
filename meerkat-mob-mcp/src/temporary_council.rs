//! Temporary-council orchestration core (issue #159, phase 4).
//!
//! A *temporary council* seats source-owned forked-participant capabilities as
//! ordinary members of a REAL, short-lived mob, runs a bounded sequential
//! discussion, applies one explicit merge-back policy, and tears the mob down.
//!
//! # What this module is, and is not
//!
//! It is orchestration, idempotency, and cleanup custody. It is NOT a
//! sub-agent subsystem and NOT a second lifecycle:
//!
//! * the temporary mob is created from the caller's own explicit
//!   [`MobDefinition`] through the ordinary
//!   [`MobMcpState::mob_create_definition`] path — no hidden `AgentBuilder`;
//! * `MobMachine`, the member machines, and `ForkedParticipantLifecycleMachine`
//!   remain the canonical lifecycle owners, and live member truth is always
//!   read back from [`MobHandle`] rather than mirrored into the council record;
//! * capability lifecycle remains source-owned, while the council record
//!   deliberately persists the immutable capability it holds so remote-owner
//!   cleanup can survive coordinator loss. Debug output redacts the bearer and
//!   no public wire projection exposes it.
//!
//! # Cancellation and crash safety
//!
//! [`TemporaryCouncilCoordinator::run`] starts an OWNED bounded task keyed by
//! council id. Dropping the caller's future does not cancel that task: it runs
//! to its absolute deadline, seals its immutable result, and performs cleanup.
//! A second caller presenting the same council id and the same request joins
//! the same task instead of starting a second one; the same id with a different
//! request is rejected.
//!
//! A process crash leaves a record with no result. That is recovered by
//! [`TemporaryCouncilCoordinator::recover_unfinished`] (and same-id admission
//! recovery) as a typed
//! [`TemporaryCouncilExitReason::CoordinatorInterrupted`] terminal plus
//! cleanup. It is never silently re-executed, because re-execution would
//! duplicate model work and result delivery.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::FutureExt as _;
use meerkat_core::service::{SessionHistoryQuery, SessionServiceHistoryExt};
use meerkat_core::types::HandlingMode;
use meerkat_core::types::{ContentInput, SessionId};
use meerkat_mob::forked_participant::{
    ForkedParticipantOperationScope, ForkedParticipantOwnerRoute, ForkedParticipantRef,
    ForkedParticipantReusePolicy, MAX_FORKED_PARTICIPANT_TTL,
};
use meerkat_mob::machines::temporary_council_lifecycle::{
    TemporaryCouncilClaimDenial, TemporaryCouncilLifecycleEffect, TemporaryCouncilLifecycleInput,
    TemporaryCouncilLifecycleMachineAuthority, TemporaryCouncilLifecycleMachineMutator,
    TemporaryCouncilLifecycleMachineState,
};
use meerkat_mob::runtime::{
    BoundedResultSpec, BoundedTurnFailure, HostBindRequest, SpawnMemberSpec,
};
use meerkat_mob::store::{
    ForkedParticipantStore, TemporaryCouncilRecord, TemporaryCouncilRecoveryVerdict,
    TemporaryCouncilStore,
};
use meerkat_mob::temporary_council::{
    MAX_TEMPORARY_COUNCIL_DURATION, MAX_TEMPORARY_COUNCIL_EXCHANGES,
    MAX_TEMPORARY_COUNCIL_PARTICIPANTS, MAX_TEMPORARY_COUNCIL_RESULT_BYTES,
    MAX_TEMPORARY_COUNCIL_ROUNDS, MIN_TEMPORARY_COUNCIL_RESULT_BYTES,
    TEMPORARY_COUNCIL_FINGERPRINT_VERSION, TemporaryCouncilAcquisition,
    TemporaryCouncilArtifactClaim, TemporaryCouncilCapabilityProvenance,
    TemporaryCouncilCleanupDebt, TemporaryCouncilCleanupReceipt, TemporaryCouncilCleanupStatus,
    TemporaryCouncilDurability, TemporaryCouncilExchangeOutcome, TemporaryCouncilExchangeReceipt,
    TemporaryCouncilExitReason, TemporaryCouncilId, TemporaryCouncilMergeOutcome,
    TemporaryCouncilMergePolicyKind, TemporaryCouncilParticipantCustody,
    TemporaryCouncilParticipantProvenance, TemporaryCouncilResult,
    TemporaryCouncilSelectedExchange, TemporaryCouncilStructuredContractIdentity,
};
use meerkat_mob::{
    AgentIdentity, MobDefinition, MobError, MobHandle, MobId, MobStoreError, ProfileBinding,
    ProfileName, WorkOrigin, WorkSpec,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::MobMcpState;
// On wasm32 the crate aliases `tokio` to `tokio_with_wasm`; a bare `tokio::`
// path inside a submodule resolves to an extern crate, so import the alias.
#[cfg(target_arch = "wasm32")]
use crate::tokio;

/// Maximum number of prior exchanges replayed as typed injected context.
///
/// Every entry is already receiver-bounded, so this is a second, explicit
/// bound on the request size rather than the only one.
pub const TEMPORARY_COUNCIL_MAX_INJECTED_CONTEXT_ENTRIES: usize = 16;

/// Maximum length of a council topic/prompt accepted from a caller.
pub const MAX_TEMPORARY_COUNCIL_TOPIC_BYTES: usize = 8 * 1024;

/// Maximum number of transcript indices a `SelectedTranscript` merge may name.
pub const MAX_TEMPORARY_COUNCIL_SELECTED_INDICES: usize = 32;

/// Bounded budget for one cleanup attempt.
///
/// Cleanup runs AFTER the immutable result is sealed, so it must never hold a
/// caller past its deadline. When this budget expires the sealed result is
/// published with an explicit pending receipt and the obligation stays durable
/// for a later sweep.
pub const TEMPORARY_COUNCIL_CLEANUP_BUDGET: Duration = Duration::from_secs(30);

/// Minimum renewal window for a coordinator claim.
///
/// A second process may only take a council over after observing this lease
/// expired; the takeover advances the machine's claim epoch, which fences the
/// previous executor. Active execution is protected through the council's
/// absolute deadline plus its cleanup budget, so a healthy bounded turn cannot
/// outlive its claim.
pub const TEMPORARY_COUNCIL_CLAIM_LEASE: Duration = Duration::from_secs(120);

/// UUIDv5 namespace for deterministic council delivery correlation ids.
///
/// Delivery correlation ids must be canonical UUIDs, and they must be derived
/// (not minted) so a retry of the same exchange presents the same identity.
const COUNCIL_DELIVERY_NAMESPACE: uuid::Uuid =
    uuid::Uuid::from_u128(0x9f2c_51ab_7d64_4c19_9c3a_1f77_0e5b_6a24_u128);

// ===========================================================================
// Request vocabulary
// ===========================================================================

/// Absolute or relative bound on how long a council may run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TemporaryCouncilDeadline {
    /// An explicit wall-clock instant.
    Absolute {
        /// The instant after which no further work may start or continue.
        at: DateTime<Utc>,
    },
    /// A duration measured from acceptance.
    Relative {
        /// How long the council may run.
        after: Duration,
    },
}

/// Bounded budget for one council. Every field is validated before any side
/// effect: an over-budget request never creates a mob, a capability, or a turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TemporaryCouncilBounds {
    /// Absolute or relative deadline, capped at [`MAX_TEMPORARY_COUNCIL_DURATION`].
    pub deadline: TemporaryCouncilDeadline,
    /// Maximum number of sequential rounds.
    pub max_rounds: u32,
    /// Maximum number of individual participant exchanges across all rounds.
    pub max_exchanges: u32,
    /// Receiver-bound applied to each exchange result.
    pub max_result_bytes: usize,
}

impl TemporaryCouncilBounds {
    /// Convenience constructor for the common relative-deadline shape.
    #[must_use]
    pub fn relative(after: Duration, max_rounds: u32, max_result_bytes: usize) -> Self {
        Self {
            deadline: TemporaryCouncilDeadline::Relative { after },
            max_rounds,
            max_exchanges: MAX_TEMPORARY_COUNCIL_EXCHANGES,
            max_result_bytes,
        }
    }
}

/// One council participant: which source member is forked, and how the branch
/// is seated in the temporary mob.
///
/// There is deliberately no credential, auth override, or mutable session
/// state here. Tool, auth, realm, and filesystem boundaries stay those of the
/// source execution context, which is exactly what the capability layer
/// guarantees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TemporaryCouncilParticipantSpec {
    /// Deterministic slot order, also the turn order within a round.
    pub order: u32,
    /// Deterministic role label carried into provenance and prompts.
    pub role: String,
    /// Mob that owns the source member.
    pub source_mob_id: MobId,
    /// Source member identity.
    pub source_identity: AgentIdentity,
    /// Identity the branch is seated under in the temporary mob.
    pub target_identity: AgentIdentity,
    /// Profile in the caller's definition template the branch is seated from.
    pub target_profile: ProfileName,
    /// Complete-boundary prefix length to fork; `None` selects the head.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_message_count: Option<usize>,
    /// Granted operation scope. Must admit invocation and observation.
    pub scope: ForkedParticipantOperationScope,
}

impl TemporaryCouncilParticipantSpec {
    /// Minimal participant using the invoke-and-observe scope.
    #[must_use]
    pub fn new(
        order: u32,
        role: impl Into<String>,
        source_mob_id: MobId,
        source_identity: AgentIdentity,
        target_identity: AgentIdentity,
        target_profile: ProfileName,
    ) -> Self {
        Self {
            order,
            role: role.into(),
            source_mob_id,
            source_identity,
            target_identity,
            target_profile,
            prefix_message_count: None,
            scope: ForkedParticipantOperationScope::InvokeAndObserve,
        }
    }

    /// Fork a bounded prefix of the source transcript.
    #[must_use]
    pub fn with_prefix_message_count(mut self, prefix_message_count: usize) -> Self {
        self.prefix_message_count = Some(prefix_message_count);
        self
    }
}

/// The single explicit merge-back policy for one council.
///
/// No variant merges a whole transcript, and no variant mutates the caller's
/// session: the outcome is returned, never written back implicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MergeBackPolicy {
    /// One final bounded turn on `finalizer` asking for a prose summary.
    BoundedTextSummary {
        /// Participant asked to produce the summary.
        finalizer: AgentIdentity,
        /// Receiver-bound applied to that final turn.
        max_bytes: usize,
    },
    /// One final bounded turn on `finalizer` whose output is parsed as
    /// STRICT JSON and then validated against the caller's declared contract.
    ///
    /// Syntactic validity alone is not a result: a value that parses but does
    /// not satisfy the contract is a typed merge failure, not a success.
    StructuredResult {
        /// Participant asked to produce the value.
        finalizer: AgentIdentity,
        /// The contract the value must satisfy.
        contract: TemporaryCouncilStructuredContract,
        /// Receiver-bound applied to that final turn.
        max_bytes: usize,
    },
    /// Explicitly selected COUNCIL EXCHANGES from one participant.
    ///
    /// The selection domain is the council's own bounded exchange receipts,
    /// never the seated fork session's transcript. A fork session opens with
    /// the inherited source prefix, so indexing it would let a low index
    /// exfiltrate source-context content the council never produced. No final
    /// turn is taken.
    SelectedTranscript {
        /// Participant whose council exchanges are selected.
        participant: AgentIdentity,
        /// Exact council exchange sequences to select.
        exchange_sequences: Vec<u32>,
        /// Total byte cap across the whole selection.
        max_bytes: usize,
    },
    /// One final bounded turn on `participant` whose output is parsed as a
    /// typed [`TemporaryCouncilArtifactClaim`].
    ///
    /// The claim is parsed and bounded, never resolved: the council does not
    /// look the artifact up, so the outcome is an explicit participant claim
    /// rather than a verified Meerkat artifact handle.
    DurableArtifactReference {
        /// Participant asked to produce the handle.
        participant: AgentIdentity,
        /// Receiver-bound applied to that final turn.
        max_bytes: usize,
    },
    /// Observation only: provenance and confirmation, no content.
    NoMerge,
}

impl MergeBackPolicy {
    /// Stable discriminant of this policy.
    #[must_use]
    pub const fn kind(&self) -> TemporaryCouncilMergePolicyKind {
        match self {
            Self::BoundedTextSummary { .. } => TemporaryCouncilMergePolicyKind::BoundedTextSummary,
            Self::StructuredResult { .. } => TemporaryCouncilMergePolicyKind::StructuredResult,
            Self::SelectedTranscript { .. } => TemporaryCouncilMergePolicyKind::SelectedTranscript,
            Self::DurableArtifactReference { .. } => {
                TemporaryCouncilMergePolicyKind::DurableArtifactReference
            }
            Self::NoMerge => TemporaryCouncilMergePolicyKind::NoMerge,
        }
    }

    /// The participant this policy names, when it names one.
    #[must_use]
    pub const fn subject(&self) -> Option<&AgentIdentity> {
        match self {
            Self::BoundedTextSummary { finalizer, .. }
            | Self::StructuredResult { finalizer, .. } => Some(finalizer),
            Self::SelectedTranscript { participant, .. }
            | Self::DurableArtifactReference { participant, .. } => Some(participant),
            Self::NoMerge => None,
        }
    }

    /// The typed structured contract, when this policy declares one.
    #[must_use]
    pub const fn structured_contract(&self) -> Option<&TemporaryCouncilStructuredContract> {
        match self {
            Self::StructuredResult { contract, .. } => Some(contract),
            _ => None,
        }
    }

    fn max_bytes(&self) -> Option<usize> {
        match self {
            Self::BoundedTextSummary { max_bytes, .. }
            | Self::StructuredResult { max_bytes, .. }
            | Self::SelectedTranscript { max_bytes, .. }
            | Self::DurableArtifactReference { max_bytes, .. } => Some(*max_bytes),
            Self::NoMerge => None,
        }
    }
}

/// One-time transport bootstrap for the hosts a council must reach.
///
/// Deliberately NOT part of [`TemporaryCouncilRequest`]: a
/// [`HostBindRequest`] carries a ONE-TIME ceremony token, so folding it into
/// the request would (a) make an honest retry present a different fingerprint
/// and look like a conflicting request, and (b) put credential-like material
/// into the durable council record. Nothing here is fingerprinted, and nothing
/// here is persisted.
///
/// A HOST-owned participant can only be seated in the temporary mob when its
/// owning host is bound there: `spawn_attached_forked_participant` refuses a
/// host route on an unbound target BEFORE any bridge traffic. The coordinator
/// cannot mint or copy that binding from the source mob (the token is spent),
/// so the caller — which holds the descriptors — declares it here.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
struct TemporaryCouncilHostBootstrap {
    /// Hosts to bind into the temporary mob before any participant is seated.
    pub host_bindings: Vec<HostBindRequest>,
}

impl TemporaryCouncilHostBootstrap {
    /// Bootstrap that binds no hosts. Local-only councils use this.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }
}

/// The typed contract a `StructuredResult` merge must satisfy.
///
/// A council does not accept "any syntactically valid JSON" as a structured
/// result: the caller declares an identity, a version, and a JSON Schema, and
/// the finalizer's output is validated against it before the result is sealed.
/// The sealed result carries the contract identity and the schema digest, so a
/// consumer can tell exactly what was checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemporaryCouncilStructuredContract {
    /// Caller-stable contract identity.
    pub schema_id: String,
    /// Caller-declared contract version.
    pub schema_version: u32,
    /// The JSON Schema the finalizer's output is validated against.
    pub json_schema: serde_json::Value,
}

impl TemporaryCouncilStructuredContract {
    /// Declare a contract.
    #[must_use]
    pub fn new(
        schema_id: impl Into<String>,
        schema_version: u32,
        json_schema: serde_json::Value,
    ) -> Self {
        Self {
            schema_id: schema_id.into(),
            schema_version,
            json_schema,
        }
    }

    /// Non-secret identity of this contract, for the sealed result.
    fn identity(
        &self,
    ) -> Result<TemporaryCouncilStructuredContractIdentity, TemporaryCouncilError> {
        let bytes = serde_json::to_vec(&self.json_schema)
            .map_err(|error| TemporaryCouncilError::invalid(error.to_string()))?;
        Ok(TemporaryCouncilStructuredContractIdentity {
            schema_id: self.schema_id.clone(),
            schema_version: self.schema_version,
            schema_digest: digest(&bytes),
        })
    }
}

/// One complete temporary-council request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TemporaryCouncilRequest {
    /// Caller-stable council identity. The same id plus the same request is a
    /// retry; the same id plus a different request is a conflict.
    pub council_id: TemporaryCouncilId,
    /// Caller-supplied explicit definition template for the temporary mob.
    ///
    /// Its `id` is REPLACED with the council's deterministic temporary mob id
    /// so a retry after a crash finds the same real mob instead of creating a
    /// second one. Everything else is used verbatim.
    pub definition_template: MobDefinition,
    /// Participants, in caller-declared order.
    pub participants: Vec<TemporaryCouncilParticipantSpec>,
    /// Initial topic/prompt for round 0.
    pub topic: String,
    /// Bounded budget.
    pub bounds: TemporaryCouncilBounds,
    /// The single explicit merge-back policy.
    pub merge_back: MergeBackPolicy,
    /// Whether this council's custody must survive a process restart.
    ///
    /// `Durable` is refused unless the state's council store actually is
    /// durable; `ProcessBound` is the explicit opt-in that says a process
    /// death loses the record and the source capability TTL is the only
    /// remaining backstop.
    pub durability: TemporaryCouncilDurability,
}

impl TemporaryCouncilRequest {
    /// Build a request with the invoke-and-observe defaults.
    #[must_use]
    pub fn new(
        council_id: TemporaryCouncilId,
        definition_template: MobDefinition,
        participants: Vec<TemporaryCouncilParticipantSpec>,
        topic: impl Into<String>,
        bounds: TemporaryCouncilBounds,
        merge_back: MergeBackPolicy,
    ) -> Self {
        Self {
            council_id,
            definition_template,
            participants,
            topic: topic.into(),
            bounds,
            merge_back,
            durability: TemporaryCouncilDurability::Durable,
        }
    }

    /// Declare this council process-bound: no crash recovery is promised.
    #[must_use]
    pub fn process_bound(mut self) -> Self {
        self.durability = TemporaryCouncilDurability::ProcessBound;
        self
    }
}

// ===========================================================================
// Errors and outcome
// ===========================================================================

/// Typed failure of a council operation.
///
/// `Clone` because a single owned execution task publishes one outcome to
/// every joined caller.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TemporaryCouncilError {
    /// The request was rejected before any side effect.
    #[error("invalid temporary council request: {detail}")]
    InvalidRequest {
        /// What was wrong.
        detail: String,
    },
    /// The council id is bound to a materially different request.
    #[error(
        "temporary council {council_id} is already bound to a different request \
         (stored {stored_fingerprint}, presented {presented_fingerprint})"
    )]
    ConflictingRequest {
        /// The bound council id.
        council_id: TemporaryCouncilId,
        /// Fingerprint durably bound to the id.
        stored_fingerprint: String,
        /// Fingerprint of the presented request.
        presented_fingerprint: String,
    },
    /// Durable council custody could not be read or written.
    #[error("temporary council custody failed: {detail}")]
    Store {
        /// Typed store detail.
        detail: String,
    },
    /// A mob operation failed in a way that prevented orchestration.
    #[error("temporary council mob operation failed: {detail}")]
    Mob {
        /// Typed mob detail.
        detail: String,
    },
    /// The canonical council lifecycle machine refused a command, or the
    /// persisted machine state could not be recovered.
    #[error("temporary council lifecycle refused: {detail}")]
    Lifecycle {
        /// Typed machine detail.
        detail: String,
    },
    /// Another coordinator holds a live claim on this council.
    #[error(
        "temporary council {council_id} is held by another coordinator (claim epoch \
         {current_claim_epoch}); its lease has not been observed expired"
    )]
    HeldByAnotherCoordinator {
        /// The contested council id.
        council_id: TemporaryCouncilId,
        /// Claim epoch currently recorded.
        current_claim_epoch: u64,
    },
    /// This executor's claim was superseded; it may not continue or seal.
    #[error(
        "temporary council {council_id} fenced this executor (current claim epoch \
         {current_claim_epoch})"
    )]
    Fenced {
        /// The council this executor lost.
        council_id: TemporaryCouncilId,
        /// Claim epoch that superseded it.
        current_claim_epoch: u64,
    },
    /// The caller declared durable custody the state cannot provide.
    #[error(
        "temporary council {council_id} declared durable custody, but this state's council \
         store is process-bound; declare TemporaryCouncilDurability::ProcessBound to run \
         without crash recovery"
    )]
    DurabilityUnavailable {
        /// The refused council id.
        council_id: TemporaryCouncilId,
    },
    /// The owned execution task ended without publishing an outcome.
    #[error("temporary council coordinator became unavailable: {detail}")]
    CoordinatorUnavailable {
        /// Typed detail.
        detail: String,
    },
}

impl TemporaryCouncilError {
    fn store(error: impl std::fmt::Display) -> Self {
        Self::Store {
            detail: error.to_string(),
        }
    }

    fn invalid(detail: impl Into<String>) -> Self {
        Self::InvalidRequest {
            detail: detail.into(),
        }
    }
}

/// The immutable result plus the separately-reported cleanup status.
///
/// The result stays valid even when cleanup fails; the two are never folded
/// into one verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TemporaryCouncilOutcome {
    /// The immutable council result.
    pub result: TemporaryCouncilResult,
    /// The most recent cleanup receipt.
    pub cleanup: TemporaryCouncilCleanupReceipt,
    /// Whether this outcome was replayed from durable custody rather than
    /// produced by this call.
    pub replayed: bool,
}

/// What one recovery sweep did to one unfinished council.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TemporaryCouncilRecoveryReport {
    /// Council that was recovered.
    pub council_id: TemporaryCouncilId,
    /// Whether this sweep sealed a terminal interrupted result.
    pub sealed_interrupted_result: bool,
    /// Whether the record is now fully settled.
    pub settled: bool,
    /// The cleanup receipt this sweep committed.
    pub cleanup: TemporaryCouncilCleanupReceipt,
}

// ===========================================================================
// Validation
// ===========================================================================

/// A request that has passed every bound check, with its derived identities.
#[derive(Debug, Clone)]
struct ValidatedRequest {
    request: TemporaryCouncilRequest,
    fingerprint: String,
    definition: MobDefinition,
    temporary_mob_id: MobId,
    /// Absolute deadline. Present only once the request has been admitted as a
    /// NEW council; a replay never resolves one.
    deadline: Option<DateTime<Utc>>,
    participants: Vec<TemporaryCouncilParticipantCustody>,
}

#[derive(Serialize)]
struct CanonicalCouncilFingerprint<'a> {
    fingerprint_version: u32,
    council_id: &'a str,
    temporary_mob_id: &'a str,
    definition_digest: String,
    topic_digest: String,
    participants: &'a [TemporaryCouncilParticipantSpec],
    deadline: &'a TemporaryCouncilDeadline,
    max_rounds: u32,
    max_exchanges: u32,
    max_result_bytes: u64,
    merge_back: &'a MergeBackPolicy,
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// Resolve a request's absolute deadline.
///
/// Deliberately SEPARATE from shape validation: an exact replay of a lost
/// response can legitimately arrive after the original deadline has passed,
/// and the caller must then get the sealed result rather than a
/// "deadline is in the past" refusal for a council that already ran. Only a
/// NEW admission resolves a deadline, and only a new admission may be refused
/// for one.
fn resolve_deadline(
    bounds: &TemporaryCouncilBounds,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, TemporaryCouncilError> {
    match bounds.deadline {
        TemporaryCouncilDeadline::Absolute { at } => {
            if at <= now {
                return Err(TemporaryCouncilError::invalid(
                    "an absolute council deadline must be in the future",
                ));
            }
            let cap = chrono::Duration::from_std(MAX_TEMPORARY_COUNCIL_DURATION)
                .map_err(|error| TemporaryCouncilError::invalid(error.to_string()))?;
            let ceiling = now
                .checked_add_signed(cap)
                .ok_or_else(|| TemporaryCouncilError::invalid("council deadline overflowed"))?;
            if at > ceiling {
                return Err(TemporaryCouncilError::invalid(format!(
                    "a council deadline may not exceed {} seconds from acceptance",
                    MAX_TEMPORARY_COUNCIL_DURATION.as_secs()
                )));
            }
            Ok(at)
        }
        TemporaryCouncilDeadline::Relative { after } => {
            if after.is_zero() {
                return Err(TemporaryCouncilError::invalid(
                    "a relative council deadline must be positive",
                ));
            }
            if after > MAX_TEMPORARY_COUNCIL_DURATION {
                return Err(TemporaryCouncilError::invalid(format!(
                    "a council deadline may not exceed {} seconds (requested {})",
                    MAX_TEMPORARY_COUNCIL_DURATION.as_secs(),
                    after.as_secs()
                )));
            }
            let delta = chrono::Duration::from_std(after)
                .map_err(|error| TemporaryCouncilError::invalid(error.to_string()))?;
            now.checked_add_signed(delta)
                .ok_or_else(|| TemporaryCouncilError::invalid("council deadline overflowed"))
        }
    }
}

fn validate(request: &TemporaryCouncilRequest) -> Result<ValidatedRequest, TemporaryCouncilError> {
    let council_id = request.council_id.clone();

    if request.participants.is_empty() {
        return Err(TemporaryCouncilError::invalid(
            "a council needs at least one participant",
        ));
    }
    if request.participants.len() > MAX_TEMPORARY_COUNCIL_PARTICIPANTS {
        return Err(TemporaryCouncilError::invalid(format!(
            "a council may seat at most {MAX_TEMPORARY_COUNCIL_PARTICIPANTS} participants \
             (requested {})",
            request.participants.len()
        )));
    }
    if request.topic.trim().is_empty() {
        return Err(TemporaryCouncilError::invalid(
            "a council needs a non-empty topic",
        ));
    }
    if request.topic.len() > MAX_TEMPORARY_COUNCIL_TOPIC_BYTES {
        return Err(TemporaryCouncilError::invalid(format!(
            "council topic must not exceed {MAX_TEMPORARY_COUNCIL_TOPIC_BYTES} bytes"
        )));
    }

    let bounds = &request.bounds;
    if bounds.max_rounds == 0 || bounds.max_rounds > MAX_TEMPORARY_COUNCIL_ROUNDS {
        return Err(TemporaryCouncilError::invalid(format!(
            "max_rounds must be 1..={MAX_TEMPORARY_COUNCIL_ROUNDS} (requested {})",
            bounds.max_rounds
        )));
    }
    if bounds.max_exchanges == 0 || bounds.max_exchanges > MAX_TEMPORARY_COUNCIL_EXCHANGES {
        return Err(TemporaryCouncilError::invalid(format!(
            "max_exchanges must be 1..={MAX_TEMPORARY_COUNCIL_EXCHANGES} (requested {})",
            bounds.max_exchanges
        )));
    }
    if bounds.max_result_bytes < MIN_TEMPORARY_COUNCIL_RESULT_BYTES
        || bounds.max_result_bytes > MAX_TEMPORARY_COUNCIL_RESULT_BYTES
    {
        return Err(TemporaryCouncilError::invalid(format!(
            "max_result_bytes must be {MIN_TEMPORARY_COUNCIL_RESULT_BYTES}..=\
             {MAX_TEMPORARY_COUNCIL_RESULT_BYTES} (requested {})",
            bounds.max_result_bytes
        )));
    }

    let mut definition = request.definition_template.clone();
    let temporary_mob_id = council_id.temporary_mob_id();
    definition.id = temporary_mob_id.clone();

    let mut orders = BTreeSet::new();
    let mut sources = BTreeSet::new();
    let mut targets = BTreeSet::new();
    let mut participants = Vec::with_capacity(request.participants.len());
    for spec in &request.participants {
        if spec.role.trim().is_empty() {
            return Err(TemporaryCouncilError::invalid(
                "every participant needs a non-empty role label",
            ));
        }
        if !orders.insert(spec.order) {
            return Err(TemporaryCouncilError::invalid(format!(
                "duplicate participant order {}",
                spec.order
            )));
        }
        if !sources.insert((spec.source_mob_id.clone(), spec.source_identity.clone())) {
            return Err(TemporaryCouncilError::invalid(format!(
                "duplicate participant source {}/{}",
                spec.source_mob_id, spec.source_identity
            )));
        }
        if !targets.insert(spec.target_identity.clone()) {
            return Err(TemporaryCouncilError::invalid(format!(
                "duplicate participant target identity {}",
                spec.target_identity
            )));
        }
        if !matches!(
            spec.scope,
            ForkedParticipantOperationScope::InvokeAndObserve
        ) {
            return Err(TemporaryCouncilError::invalid(format!(
                "participant {} needs the invoke-and-observe scope to hold a discussion",
                spec.target_identity
            )));
        }
        match definition.profiles.get(&spec.target_profile) {
            None => {
                return Err(TemporaryCouncilError::invalid(format!(
                    "participant {} names profile '{}', which the definition template does not \
                     declare",
                    spec.target_identity, spec.target_profile
                )));
            }
            Some(ProfileBinding::Inline(_) | ProfileBinding::RealmRef { .. }) => {}
        }

        participants.push(TemporaryCouncilParticipantCustody {
            order: spec.order,
            role: spec.role.clone(),
            source_mob_id: spec.source_mob_id.clone(),
            source_identity: spec.source_identity.clone(),
            target_identity: spec.target_identity.clone(),
            target_profile: spec.target_profile.clone(),
            scope: spec.scope,
            capability_request_id: council_id
                .capability_request_id(spec.order)
                .map_err(|error| TemporaryCouncilError::invalid(error.to_string()))?,
            capability_correlation_hint: None,
            capability_ref: None,
            acquisition: TemporaryCouncilAcquisition::NotAttempted,
            attachment_id: council_id
                .attachment_id(spec.order)
                .map_err(|error| TemporaryCouncilError::invalid(error.to_string()))?,
            seated: false,
            seated_session_id: None,
        });
    }
    participants.sort_by_key(|participant| participant.order);

    if let Some(subject) = request.merge_back.subject()
        && !targets.contains(subject)
    {
        return Err(TemporaryCouncilError::invalid(format!(
            "merge-back names {subject}, which is not a council participant"
        )));
    }
    if let Some(max_bytes) = request.merge_back.max_bytes()
        && !(MIN_TEMPORARY_COUNCIL_RESULT_BYTES..=MAX_TEMPORARY_COUNCIL_RESULT_BYTES)
            .contains(&max_bytes)
    {
        return Err(TemporaryCouncilError::invalid(format!(
            "merge-back max_bytes must be {MIN_TEMPORARY_COUNCIL_RESULT_BYTES}..=\
             {MAX_TEMPORARY_COUNCIL_RESULT_BYTES} (requested {max_bytes})"
        )));
    }
    if let MergeBackPolicy::SelectedTranscript {
        exchange_sequences, ..
    } = &request.merge_back
    {
        if exchange_sequences.is_empty() {
            return Err(TemporaryCouncilError::invalid(
                "a selected-transcript merge must name at least one exchange sequence",
            ));
        }
        if exchange_sequences.len() > MAX_TEMPORARY_COUNCIL_SELECTED_INDICES {
            return Err(TemporaryCouncilError::invalid(format!(
                "a selected-transcript merge may name at most \
                 {MAX_TEMPORARY_COUNCIL_SELECTED_INDICES} sequences (requested {})",
                exchange_sequences.len()
            )));
        }
        let unique: BTreeSet<u32> = exchange_sequences.iter().copied().collect();
        if unique.len() != exchange_sequences.len() {
            return Err(TemporaryCouncilError::invalid(
                "a selected-transcript merge must not repeat an exchange sequence",
            ));
        }
    }
    // A structured contract is validated BEFORE any side effect: an
    // uncompilable schema can never reach a council turn.
    if let Some(contract) = request.merge_back.structured_contract() {
        if contract.schema_id.trim().is_empty() {
            return Err(TemporaryCouncilError::invalid(
                "a structured-result contract needs a non-empty schema id",
            ));
        }
        jsonschema::Validator::new(&contract.json_schema).map_err(|error| {
            TemporaryCouncilError::invalid(format!(
                "structured-result contract '{}' is not a compilable JSON Schema: {error}",
                contract.schema_id
            ))
        })?;
    }

    let definition_bytes = serde_json::to_vec(&definition)
        .map_err(|error| TemporaryCouncilError::invalid(error.to_string()))?;
    let shape = CanonicalCouncilFingerprint {
        fingerprint_version: TEMPORARY_COUNCIL_FINGERPRINT_VERSION,
        council_id: council_id.as_str(),
        temporary_mob_id: temporary_mob_id.as_str(),
        definition_digest: digest(&definition_bytes),
        topic_digest: digest(request.topic.as_bytes()),
        participants: &request.participants,
        deadline: &bounds.deadline,
        max_rounds: bounds.max_rounds,
        max_exchanges: bounds.max_exchanges,
        max_result_bytes: bounds.max_result_bytes as u64,
        merge_back: &request.merge_back,
    };
    let fingerprint = format!(
        "tcf1:{}",
        digest(
            &serde_json::to_vec(&shape)
                .map_err(|error| TemporaryCouncilError::invalid(error.to_string()))?
        )
    );

    Ok(ValidatedRequest {
        request: request.clone(),
        fingerprint,
        definition,
        temporary_mob_id,
        deadline: None,
        participants,
    })
}

fn same_source_execution_profile(target: &ProfileBinding, source: &meerkat_mob::Profile) -> bool {
    match target {
        ProfileBinding::Inline(target) => target.as_ref() == source,
        ProfileBinding::RealmRef { .. } => false,
    }
}

// ===========================================================================
// Canonical lifecycle driver
// ===========================================================================

/// Advance one council record's CANONICAL machine state.
///
/// Every phase change in this module goes through here. The coordinator never
/// assigns a phase, never matches on one, and never derives "unfinished" for
/// itself: the generated `TemporaryCouncilLifecycleMachine` decides, and this
/// function only mirrors its verdict into the record.
fn advance_machine(
    state: &TemporaryCouncilLifecycleMachineState,
    input: TemporaryCouncilLifecycleInput,
) -> Result<
    (
        TemporaryCouncilLifecycleMachineState,
        Vec<TemporaryCouncilLifecycleEffect>,
    ),
    TemporaryCouncilError,
> {
    let mut authority = TemporaryCouncilLifecycleMachineAuthority::recover_from_state(
        state.clone(),
    )
    .map_err(|error| TemporaryCouncilError::Lifecycle {
        detail: format!("persisted council lifecycle state is not recoverable: {error:?}"),
    })?;
    let transition = TemporaryCouncilLifecycleMachineMutator::apply(&mut authority, input)
        .map_err(|error| TemporaryCouncilError::Lifecycle {
            detail: format!("council lifecycle refused the command: {error:?}"),
        })?;
    let effects = transition.effects().to_vec();
    Ok((authority.state().clone(), effects))
}

/// One coordinator's execution claim on a council record.
#[derive(Debug, Clone)]
struct CouncilClaim {
    claim_id: String,
    claim_epoch: u64,
}

/// Outcome of presenting a claim to a council record.
enum ClaimOutcome {
    Granted(CouncilClaim),
    Busy { current_claim_epoch: u64 },
    Settled,
}

/// Present a claim, mirroring the machine's verdict.
///
/// `lease_expired` is the shell's ONE clock read: whether the persisted lease
/// deadline has passed. The machine owns what that observation means.
fn present_claim(
    state: &TemporaryCouncilLifecycleMachineState,
    claim_id: &str,
    lease_expired: bool,
) -> Result<(TemporaryCouncilLifecycleMachineState, ClaimOutcome), TemporaryCouncilError> {
    let (next, effects) = advance_machine(
        state,
        TemporaryCouncilLifecycleInput::Claim {
            claim_id: claim_id.to_string(),
            lease_expired,
        },
    )?;
    for effect in &effects {
        match effect {
            TemporaryCouncilLifecycleEffect::ClaimGranted {
                claim_id,
                claim_epoch,
                ..
            }
            | TemporaryCouncilLifecycleEffect::ClaimRenewed {
                claim_id,
                claim_epoch,
            } => {
                return Ok((
                    next,
                    ClaimOutcome::Granted(CouncilClaim {
                        claim_id: claim_id.clone(),
                        claim_epoch: *claim_epoch,
                    }),
                ));
            }
            TemporaryCouncilLifecycleEffect::ClaimDenied {
                reason,
                current_claim_epoch,
            } => {
                return Ok((
                    next,
                    match reason {
                        TemporaryCouncilClaimDenial::AlreadySettled => ClaimOutcome::Settled,
                        _ => ClaimOutcome::Busy {
                            current_claim_epoch: *current_claim_epoch,
                        },
                    },
                ));
            }
            _ => {}
        }
    }
    Err(TemporaryCouncilError::Lifecycle {
        detail: format!("council lifecycle produced no claim verdict: {effects:?}"),
    })
}

/// Reject a fenced command instead of letting a stale executor continue.
fn reject_if_fenced(
    council_id: &TemporaryCouncilId,
    effects: &[TemporaryCouncilLifecycleEffect],
) -> Result<(), TemporaryCouncilError> {
    for effect in effects {
        if let TemporaryCouncilLifecycleEffect::CommandFenced {
            current_claim_epoch,
        } = effect
        {
            return Err(TemporaryCouncilError::Fenced {
                council_id: council_id.clone(),
                current_claim_epoch: *current_claim_epoch,
            });
        }
    }
    Ok(())
}

/// A fresh council machine state bound to one request fingerprint.
fn opened_machine_state(
    fingerprint: &str,
) -> Result<TemporaryCouncilLifecycleMachineState, TemporaryCouncilError> {
    let (state, effects) = advance_machine(
        &TemporaryCouncilLifecycleMachineState::default(),
        TemporaryCouncilLifecycleInput::Open {
            request_fingerprint: fingerprint.to_string(),
        },
    )?;
    if effects.iter().any(|effect| {
        matches!(
            effect,
            TemporaryCouncilLifecycleEffect::CouncilOpened { .. }
        )
    }) {
        Ok(state)
    } else {
        Err(TemporaryCouncilError::Lifecycle {
            detail: format!("council lifecycle refused to open the record: {effects:?}"),
        })
    }
}

/// Present a request fingerprint to a bound record and read the machine's own
/// replay-versus-conflict verdict.
fn classify_open(
    state: &TemporaryCouncilLifecycleMachineState,
    fingerprint: &str,
) -> Result<bool, TemporaryCouncilError> {
    let (_next, effects) = advance_machine(
        state,
        TemporaryCouncilLifecycleInput::Open {
            request_fingerprint: fingerprint.to_string(),
        },
    )?;
    for effect in &effects {
        match effect {
            TemporaryCouncilLifecycleEffect::CouncilOpenReplayed { .. }
            | TemporaryCouncilLifecycleEffect::CouncilOpened { .. } => return Ok(true),
            TemporaryCouncilLifecycleEffect::CouncilOpenRejected { .. } => return Ok(false),
            _ => {}
        }
    }
    Err(TemporaryCouncilError::Lifecycle {
        detail: format!("council lifecycle produced no open verdict: {effects:?}"),
    })
}

// ===========================================================================
// Coordinator
// ===========================================================================

/// Published outcome of one owned council execution task.
pub(crate) type CouncilPublish = Arc<Result<TemporaryCouncilOutcome, TemporaryCouncilError>>;

/// One in-flight council execution, joinable by any number of callers.
pub(crate) struct InflightCouncil {
    pub(crate) fingerprint: String,
    pub(crate) completion: tokio::sync::watch::Receiver<Option<CouncilPublish>>,
}

pub(crate) enum InflightReservation {
    Owned,
    Existing {
        fingerprint: String,
        completion: tokio::sync::watch::Receiver<Option<CouncilPublish>>,
    },
}

/// Trusted in-process Rust API for temporary councils.
///
/// Obtained from [`MobMcpState::temporary_council`]. The agent-facing `council`
/// mob tool is the product invocation boundary; this coordinator remains its
/// trusted in-process orchestration substrate.
#[derive(Clone)]
pub struct TemporaryCouncilCoordinator {
    state: Arc<MobMcpState>,
}

impl std::fmt::Debug for TemporaryCouncilCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TemporaryCouncilCoordinator").finish()
    }
}

impl TemporaryCouncilCoordinator {
    pub(crate) fn new(state: Arc<MobMcpState>) -> Self {
        Self { state }
    }

    fn store(&self) -> Arc<dyn TemporaryCouncilStore> {
        self.state.temporary_council_store().clone()
    }

    /// Read one council record.
    pub async fn load(
        &self,
        council_id: &TemporaryCouncilId,
    ) -> Result<Option<TemporaryCouncilRecord>, TemporaryCouncilError> {
        self.store()
            .load(council_id)
            .await
            .map_err(TemporaryCouncilError::store)
    }

    /// Run one temporary council to a durable terminal outcome.
    ///
    /// The execution is owned by a task registered under `council_id`.
    /// Dropping the returned future does NOT cancel it: it continues to the
    /// absolute deadline and performs cleanup. Presenting the same id with the
    /// same request joins that task or replays its sealed result; presenting a
    /// different request under a bound id is rejected.
    pub async fn run(
        &self,
        request: TemporaryCouncilRequest,
    ) -> Result<TemporaryCouncilOutcome, TemporaryCouncilError> {
        self.run_with_host_bootstrap(request, TemporaryCouncilHostBootstrap::none())
            .await
    }

    /// [`Self::run`] with the one-time host bootstrap a council needs to seat
    /// HOST-owned participants.
    ///
    /// The bootstrap belongs to the ATTEMPT, not to the council identity: a
    /// replay or a joined caller reuses the owning task's bootstrap and this
    /// one is ignored, and nothing from it is fingerprinted or persisted.
    async fn run_with_host_bootstrap(
        &self,
        request: TemporaryCouncilRequest,
        host_bootstrap: TemporaryCouncilHostBootstrap,
    ) -> Result<TemporaryCouncilOutcome, TemporaryCouncilError> {
        // Shape validation only. Deadline resolution is deliberately NOT done
        // here: a replay of a lost response may legitimately arrive after the
        // original deadline, and must return the sealed result.
        let validated = validate(&request)?;
        let key = validated.request.council_id.as_str().to_string();
        let (owner_tx, candidate_rx) = tokio::sync::watch::channel(None);
        let (mut completion, owned) = match self.state.temporary_council_reserve_inflight(
            key.clone(),
            validated.fingerprint.clone(),
            candidate_rx.clone(),
        ) {
            InflightReservation::Owned => (candidate_rx, true),
            InflightReservation::Existing {
                fingerprint,
                completion,
            } => {
                if fingerprint != validated.fingerprint {
                    return Err(TemporaryCouncilError::ConflictingRequest {
                        council_id: validated.request.council_id.clone(),
                        stored_fingerprint: fingerprint,
                        presented_fingerprint: validated.fingerprint.clone(),
                    });
                }
                (completion, false)
            }
        };
        if owned {
            // Registration happens atomically before any await. The guard
            // removes it if admission is cancelled or fails before ownership
            // transfers to the spawned execution task.
            let mut guard = InflightGuard::new(self.state.clone(), key.clone());
            match self
                .admit_owned(
                    &validated,
                    host_bootstrap,
                    key,
                    owner_tx.clone(),
                    completion.clone(),
                )
                .await
            {
                Ok(Admission::Replay(outcome)) => {
                    let outcome = *outcome;
                    let _ = owner_tx.send(Some(Arc::new(Ok(outcome.clone()))));
                    return Ok(outcome);
                }
                Ok(Admission::Join(owned_completion)) => {
                    completion = owned_completion;
                    guard.disarm();
                }
                Err(error) => {
                    let _ = owner_tx.send(Some(Arc::new(Err(error.clone()))));
                    return Err(error);
                }
            }
        }

        // Awaiting a watch channel is cancel-safe for the CALLER only: the
        // owned task holds the sender and keeps running if this future drops.
        loop {
            if let Some(published) = completion.borrow_and_update().clone() {
                return match published.as_ref() {
                    Ok(outcome) => Ok(outcome.clone()),
                    Err(error) => Err(error.clone()),
                };
            }
            if completion.changed().await.is_err() {
                return Err(TemporaryCouncilError::CoordinatorUnavailable {
                    detail: format!(
                        "the owned execution task for council {} ended without publishing an \
                         outcome",
                        validated.request.council_id
                    ),
                });
            }
        }
    }

    /// Admission body for a caller that owns the process-local reservation.
    async fn admit_owned(
        &self,
        validated: &ValidatedRequest,
        host_bootstrap: TemporaryCouncilHostBootstrap,
        key: String,
        tx: tokio::sync::watch::Sender<Option<CouncilPublish>>,
        rx: tokio::sync::watch::Receiver<Option<CouncilPublish>>,
    ) -> Result<Admission, TemporaryCouncilError> {
        let mut validated = validated.clone();
        let council_id = validated.request.council_id.clone();

        let store = self.store();
        let record = match store
            .load(&council_id)
            .await
            .map_err(TemporaryCouncilError::store)?
        {
            Some(existing) => {
                return self.admit_existing_record(&validated, existing).await;
            }
            None => {
                // NEW admission only: this is where a deadline is resolved and
                // where an already-elapsed one is refused.
                let now = self.state.temporary_council_now();
                let deadline = resolve_deadline(&validated.request.bounds, now)?;
                validated.deadline = Some(deadline);
                // Durability is an explicit contract, never an inference.
                if !store.durability().satisfies(validated.request.durability) {
                    return Err(TemporaryCouncilError::DurabilityUnavailable { council_id });
                }
                let machine_state = opened_machine_state(&validated.fingerprint)?;
                let record = TemporaryCouncilRecord {
                    council_id: council_id.clone(),
                    request_fingerprint: validated.fingerprint.clone(),
                    temporary_mob_id: validated.temporary_mob_id.clone(),
                    deadline,
                    durability: validated.request.durability,
                    claim_lease_expires_at: now,
                    machine_state,
                    participants: validated.participants.clone(),
                    exchanges: Vec::new(),
                    result: None,
                    cleanup: None,
                    revision: 0,
                    created_at: now,
                    updated_at: now,
                };
                // Side effects start only AFTER the record is durable.
                match store.insert_new(&record).await {
                    Ok(inserted) => inserted,
                    Err(MobStoreError::CasConflict(_)) => {
                        let winner = store
                            .load(&council_id)
                            .await
                            .map_err(TemporaryCouncilError::store)?
                            .ok_or_else(|| TemporaryCouncilError::CoordinatorUnavailable {
                                detail: format!(
                                    "council {council_id} insert conflicted but no winning record \
                                     could be loaded"
                                ),
                            })?;
                        if !classify_open(&winner.machine_state, &validated.fingerprint)? {
                            return Err(TemporaryCouncilError::ConflictingRequest {
                                council_id,
                                stored_fingerprint: winner.request_fingerprint,
                                presented_fingerprint: validated.fingerprint.clone(),
                            });
                        }
                        if winner.result.is_some() {
                            return self.admit_existing_record(&validated, winner).await;
                        }
                        // Another coordinator won the insert but has not
                        // necessarily claimed execution yet. Continue through
                        // the ordinary durable claim below: exactly one side
                        // wins, and a live winner is reported as busy rather
                        // than being misclassified as interrupted recovery.
                        winner
                    }
                    Err(error) => return Err(TemporaryCouncilError::store(error)),
                }
            }
        };

        // Take the execution claim before any side effect. A live claim held
        // by another coordinator is refused; only an observed-expired lease
        // admits a takeover, and the takeover fences the previous executor.
        let mut record = record;
        let claim_id = self.state.coordinator_id().to_string();
        let lease_expired = record.claim_lease_expires_at <= self.state.temporary_council_now();
        let (next_state, outcome) = present_claim(&record.machine_state, &claim_id, lease_expired)?;
        let claim = match outcome {
            ClaimOutcome::Granted(claim) => claim,
            ClaimOutcome::Busy {
                current_claim_epoch,
            } => {
                return Err(TemporaryCouncilError::HeldByAnotherCoordinator {
                    council_id,
                    current_claim_epoch,
                });
            }
            ClaimOutcome::Settled => {
                return Err(TemporaryCouncilError::CoordinatorUnavailable {
                    detail: format!("council {council_id} is settled but carries no result"),
                });
            }
        };
        record.machine_state = next_state;
        record.claim_lease_expires_at = active_claim_lease_expiry(&self.state, record.deadline);
        // The deadline the record carries is authority; a takeover inherits it.
        validated.deadline = Some(record.deadline);
        let record = match store.commit(&record).await {
            Ok(record) => record,
            Err(MobStoreError::CasConflict(_)) => {
                let winner = store
                    .load(&council_id)
                    .await
                    .map_err(TemporaryCouncilError::store)?
                    .ok_or_else(|| TemporaryCouncilError::CoordinatorUnavailable {
                        detail: format!(
                            "council {council_id} claim conflicted but no winning record could be loaded"
                        ),
                    })?;
                if winner.result.is_some() {
                    return self.admit_existing_record(&validated, winner).await;
                }
                let lease_expired =
                    winner.claim_lease_expires_at <= self.state.temporary_council_now();
                let (_, outcome) = present_claim(&winner.machine_state, &claim_id, lease_expired)?;
                return match outcome {
                    ClaimOutcome::Busy {
                        current_claim_epoch,
                    } => Err(TemporaryCouncilError::HeldByAnotherCoordinator {
                        council_id,
                        current_claim_epoch,
                    }),
                    ClaimOutcome::Settled => Err(TemporaryCouncilError::CoordinatorUnavailable {
                        detail: format!(
                            "council {council_id} settled during claim arbitration without a result"
                        ),
                    }),
                    ClaimOutcome::Granted(_) => {
                        Err(TemporaryCouncilError::CoordinatorUnavailable {
                            detail: format!(
                                "council {council_id} claim conflict had no durable winning claim"
                            ),
                        })
                    }
                };
            }
            Err(error) => return Err(TemporaryCouncilError::store(error)),
        };

        let state = self.state.clone();
        // OWNED task: the caller's await may be dropped; this may not.
        tokio::spawn(async move {
            let guard = InflightGuard::new(state.clone(), key);
            let council_id = record.council_id.clone();
            let supervised = {
                let run = CouncilRun {
                    state: state.clone(),
                    validated,
                    record,
                    host_bootstrap,
                    claim: claim.clone(),
                };
                // A panic inside execution must not be a silent hang: it is
                // caught here, converted to a typed terminal, and then sealed
                // by the SAME supervisor that would have sealed a crash.
                std::panic::AssertUnwindSafe(run.execute())
                    .catch_unwind()
                    .await
            };
            let published: CouncilPublish = Arc::new(match supervised {
                Ok(outcome) => outcome,
                Err(panic) => {
                    let detail = panic_detail(&panic);
                    tracing::error!(
                        council_id = %council_id,
                        detail = %detail,
                        "temporary council execution panicked; sealing an interrupted terminal"
                    );
                    supervise_panicked_council(&state, &council_id, &claim, &detail).await
                }
            });
            let _ = tx.send(Some(published));
            drop(guard);
        });

        Ok(Admission::Join(rx))
    }

    async fn admit_existing_record(
        &self,
        validated: &ValidatedRequest,
        existing: TemporaryCouncilRecord,
    ) -> Result<Admission, TemporaryCouncilError> {
        let council_id = validated.request.council_id.clone();
        // The machine, not a string compare in the shell, decides replay
        // versus conflict for a bound council identity.
        if !classify_open(&existing.machine_state, &validated.fingerprint)? {
            return Err(TemporaryCouncilError::ConflictingRequest {
                council_id,
                stored_fingerprint: existing.request_fingerprint,
                presented_fingerprint: validated.fingerprint.clone(),
            });
        }
        if let Some(result) = existing.result.clone() {
            let cleanup =
                existing
                    .cleanup
                    .clone()
                    .unwrap_or_else(|| TemporaryCouncilCleanupReceipt {
                        attempted_at: existing.updated_at,
                        attempts: 0,
                        temporary_mob_destroyed: false,
                        released_participants: Vec::new(),
                        revoked_participants: Vec::new(),
                        debts: vec![TemporaryCouncilCleanupDebt {
                            subject: format!("council:{council_id}"),
                            detail: "no cleanup attempt is durably recorded".to_string(),
                        }],
                        budget_exhausted: false,
                    });
            return Ok(Admission::Replay(Box::new(TemporaryCouncilOutcome {
                result,
                cleanup,
                replayed: true,
            })));
        }

        // Same-id admission may recover exactly this record. It never sweeps
        // unrelated councils or exercises realm-wide admin authority through
        // the run surface.
        self.recover_record(existing).await?;
        let recovered = self
            .store()
            .load(&council_id)
            .await
            .map_err(TemporaryCouncilError::store)?
            .ok_or_else(|| TemporaryCouncilError::CoordinatorUnavailable {
                detail: format!("council {council_id} disappeared after same-id recovery"),
            })?;
        let result =
            recovered
                .result
                .ok_or_else(|| TemporaryCouncilError::CoordinatorUnavailable {
                    detail: format!(
                        "council {council_id} recovery completed without sealing a result"
                    ),
                })?;
        let cleanup = recovered
            .cleanup
            .unwrap_or_else(|| TemporaryCouncilCleanupReceipt {
                attempted_at: recovered.updated_at,
                attempts: 0,
                temporary_mob_destroyed: false,
                released_participants: Vec::new(),
                revoked_participants: Vec::new(),
                debts: vec![TemporaryCouncilCleanupDebt {
                    subject: format!("council:{council_id}"),
                    detail: "same-id recovery sealed no cleanup receipt".to_string(),
                }],
                budget_exhausted: false,
            });
        Ok(Admission::Replay(Box::new(TemporaryCouncilOutcome {
            result,
            cleanup,
            replayed: true,
        })))
    }

    /// Converge every unfinished council record.
    ///
    /// A record with no result is sealed as
    /// [`TemporaryCouncilExitReason::CoordinatorInterrupted`] and cleaned up.
    /// A record with a result but unsettled cleanup gets another cleanup
    /// attempt, so retained debt converges instead of being lost.
    /// Councils owned by a live task in this process are skipped.
    pub async fn recover_unfinished(
        &self,
    ) -> Result<Vec<TemporaryCouncilRecoveryReport>, TemporaryCouncilError> {
        let store = self.store();
        let unfinished = store
            .list_unfinished()
            .await
            .map_err(TemporaryCouncilError::store)?;
        let mut reports = Vec::new();
        for record in unfinished {
            let key = record.council_id.as_str().to_string();
            let (tx, rx) = tokio::sync::watch::channel(None);
            if matches!(
                self.state.temporary_council_reserve_inflight(
                    key.clone(),
                    record.request_fingerprint.clone(),
                    rx,
                ),
                InflightReservation::Existing { .. }
            ) {
                continue;
            }
            let _guard = InflightGuard::new(self.state.clone(), key);
            match self.recover_record(record).await {
                Ok(report) => {
                    let publication = match store
                        .load(&report.council_id)
                        .await
                        .map_err(TemporaryCouncilError::store)
                    {
                        Ok(Some(record)) => replay_outcome(record),
                        Ok(None) => Err(TemporaryCouncilError::CoordinatorUnavailable {
                            detail: format!(
                                "council {} disappeared after recovery",
                                report.council_id
                            ),
                        }),
                        Err(error) => Err(error),
                    };
                    let _ = tx.send(Some(Arc::new(publication.clone())));
                    publication?;
                    reports.push(report);
                }
                Err(error @ TemporaryCouncilError::HeldByAnotherCoordinator { .. }) => {
                    // A live foreign coordinator owns this record. Its lease
                    // expiry will admit takeover later; unrelated recovery
                    // work must continue.
                    let _ = tx.send(Some(Arc::new(Err(error))));
                }
                Err(error) => {
                    let _ = tx.send(Some(Arc::new(Err(error.clone()))));
                    return Err(error);
                }
            }
        }
        Ok(reports)
    }

    async fn recover_record(
        &self,
        record: TemporaryCouncilRecord,
    ) -> Result<TemporaryCouncilRecoveryReport, TemporaryCouncilError> {
        let store = self.store();
        let mut record = record;
        let verdict =
            record
                .recovery_verdict()
                .map_err(|error| TemporaryCouncilError::Lifecycle {
                    detail: format!(
                        "persisted council lifecycle state is not recoverable: {error:?}"
                    ),
                })?;
        // Recovery is an EXECUTION, so it takes the claim like any other. A
        // claim whose lease has NOT been observed expired is left alone; only
        // an observed-expired lease admits a takeover, and that takeover
        // advances the epoch, which fences the previous executor.
        let claim_id = self.state.coordinator_id().to_string();
        let lease_expired = record.claim_lease_expires_at <= self.state.temporary_council_now();
        let (next_state, outcome) = present_claim(&record.machine_state, &claim_id, lease_expired)?;
        let claim = match outcome {
            ClaimOutcome::Granted(claim) => claim,
            ClaimOutcome::Busy {
                current_claim_epoch,
            } => {
                return Err(TemporaryCouncilError::HeldByAnotherCoordinator {
                    council_id: record.council_id.clone(),
                    current_claim_epoch,
                });
            }
            ClaimOutcome::Settled => {
                return Ok(TemporaryCouncilRecoveryReport {
                    council_id: record.council_id.clone(),
                    sealed_interrupted_result: false,
                    settled: true,
                    cleanup: record
                        .cleanup
                        .clone()
                        .unwrap_or_else(|| settled_receipt(&self.state)),
                });
            }
        };
        record.machine_state = next_state;
        record.claim_lease_expires_at = bounded_time_add(
            self.state.temporary_council_now(),
            TEMPORARY_COUNCIL_CLAIM_LEASE,
        );
        record = store
            .commit(&record)
            .await
            .map_err(TemporaryCouncilError::store)?;

        let sealed_interrupted_result = !verdict.result_sealed;
        if sealed_interrupted_result {
            // A crashed coordinator is sealed as a typed interrupted terminal
            // by the machine; it is never re-executed.
            let (next, effects) = advance_machine(
                &record.machine_state,
                TemporaryCouncilLifecycleInput::SealInterruptedResult {
                    claim_id: claim.claim_id.clone(),
                    claim_epoch: claim.claim_epoch,
                },
            )?;
            reject_if_fenced(&record.council_id, &effects)?;
            record.machine_state = next;
            record.result = Some(interrupted_result(&self.state, &record));
            record = store
                .commit(&record)
                .await
                .map_err(TemporaryCouncilError::store)?;
        }
        let cleanup = cleanup_council(&self.state, &record, cleanup_budget(&self.state)).await;
        record.cleanup = Some(cleanup.clone());
        let (next, effects) = advance_machine(
            &record.machine_state,
            if cleanup.settled() {
                TemporaryCouncilLifecycleInput::RecordCleanupSettled {
                    claim_id: claim.claim_id.clone(),
                    claim_epoch: claim.claim_epoch,
                }
            } else {
                TemporaryCouncilLifecycleInput::RecordCleanupDebt {
                    claim_id: claim.claim_id.clone(),
                    claim_epoch: claim.claim_epoch,
                }
            },
        )?;
        reject_if_fenced(&record.council_id, &effects)?;
        record.machine_state = next;
        let record = store
            .commit(&record)
            .await
            .map_err(TemporaryCouncilError::store)?;
        Ok(TemporaryCouncilRecoveryReport {
            council_id: record.council_id.clone(),
            sealed_interrupted_result,
            settled: cleanup.settled(),
            cleanup,
        })
    }
}

/// Human-readable detail of a caught panic payload.
fn panic_detail(panic: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = panic.downcast_ref::<&'static str>() {
        (*text).to_string()
    } else if let Some(text) = panic.downcast_ref::<String>() {
        text.clone()
    } else {
        "council execution panicked".to_string()
    }
}

/// Seal a typed interrupted terminal for a council whose execution panicked.
///
/// This is the in-process half of crash recovery: it uses the SAME machine
/// seal and the SAME bounded cleanup a restarted process would, so a panic and
/// a process death converge on one terminal shape. It never panics itself —
/// every failure here degrades to a typed error for the watchers, and the
/// durable record stays visible to the recovery sweep.
async fn supervise_panicked_council(
    state: &Arc<MobMcpState>,
    council_id: &TemporaryCouncilId,
    claim: &CouncilClaim,
    detail: &str,
) -> Result<TemporaryCouncilOutcome, TemporaryCouncilError> {
    let store = state.temporary_council_store().clone();
    let mut record = store
        .load(council_id)
        .await
        .map_err(TemporaryCouncilError::store)?
        .ok_or_else(|| TemporaryCouncilError::CoordinatorUnavailable {
            detail: format!("council {council_id} vanished after a panic: {detail}"),
        })?;

    let verdict = record
        .recovery_verdict()
        .map_err(|error| TemporaryCouncilError::Lifecycle {
            detail: format!("persisted council lifecycle state is not recoverable: {error:?}"),
        })?;
    if !verdict.result_sealed {
        let (next, effects) = advance_machine(
            &record.machine_state,
            TemporaryCouncilLifecycleInput::SealInterruptedResult {
                claim_id: claim.claim_id.clone(),
                claim_epoch: claim.claim_epoch,
            },
        )?;
        reject_if_fenced(council_id, &effects)?;
        record.machine_state = next;
        record.result = Some(interrupted_result(state, &record));
        record = store
            .commit(&record)
            .await
            .map_err(TemporaryCouncilError::store)?;
    }

    let cleanup = cleanup_council(state, &record, cleanup_budget(state)).await;
    record.cleanup = Some(cleanup.clone());
    let (next, effects) = advance_machine(
        &record.machine_state,
        if cleanup.settled() {
            TemporaryCouncilLifecycleInput::RecordCleanupSettled {
                claim_id: claim.claim_id.clone(),
                claim_epoch: claim.claim_epoch,
            }
        } else {
            TemporaryCouncilLifecycleInput::RecordCleanupDebt {
                claim_id: claim.claim_id.clone(),
                claim_epoch: claim.claim_epoch,
            }
        },
    )?;
    reject_if_fenced(council_id, &effects)?;
    record.machine_state = next;
    let record = store
        .commit(&record)
        .await
        .map_err(TemporaryCouncilError::store)?;

    Ok(TemporaryCouncilOutcome {
        result: record.result.clone().ok_or_else(|| {
            TemporaryCouncilError::CoordinatorUnavailable {
                detail: format!("council {council_id} sealed no result after a panic: {detail}"),
            }
        })?,
        cleanup,
        replayed: false,
    })
}

enum Admission {
    Replay(Box<TemporaryCouncilOutcome>),
    Join(tokio::sync::watch::Receiver<Option<CouncilPublish>>),
}

fn replay_outcome(
    record: TemporaryCouncilRecord,
) -> Result<TemporaryCouncilOutcome, TemporaryCouncilError> {
    let council_id = record.council_id.clone();
    let result = record
        .result
        .ok_or_else(|| TemporaryCouncilError::CoordinatorUnavailable {
            detail: format!("council {council_id} recovery completed without sealing a result"),
        })?;
    let cleanup = record
        .cleanup
        .unwrap_or_else(|| TemporaryCouncilCleanupReceipt {
            attempted_at: record.updated_at,
            attempts: 0,
            temporary_mob_destroyed: false,
            released_participants: Vec::new(),
            revoked_participants: Vec::new(),
            debts: vec![TemporaryCouncilCleanupDebt {
                subject: format!("council:{council_id}"),
                detail: "no cleanup attempt is durably recorded".to_string(),
            }],
            budget_exhausted: false,
        });
    Ok(TemporaryCouncilOutcome {
        result,
        cleanup,
        replayed: true,
    })
}

/// Removes the single-flight registration even when the owned task unwinds.
struct InflightGuard {
    state: Arc<MobMcpState>,
    key: String,
    armed: bool,
}

impl InflightGuard {
    fn new(state: Arc<MobMcpState>, key: String) -> Self {
        Self {
            state,
            key,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        if self.armed {
            self.state.temporary_council_remove_inflight(&self.key);
        }
    }
}

fn interrupted_result(
    state: &MobMcpState,
    record: &TemporaryCouncilRecord,
) -> TemporaryCouncilResult {
    let exchanges = record.exchanges.clone();
    let truncated_exchange_count = exchanges
        .iter()
        .filter(|receipt| receipt.truncated())
        .count() as u32;
    let rounds_completed = exchanges
        .iter()
        .filter_map(|receipt| {
            matches!(
                receipt.outcome,
                TemporaryCouncilExchangeOutcome::Completed { .. }
            )
            .then_some(receipt.round)
        })
        .collect::<BTreeSet<_>>()
        .len() as u32;
    TemporaryCouncilResult {
        council_id: record.council_id.clone(),
        request_fingerprint: record.request_fingerprint.clone(),
        temporary_mob_id: record.temporary_mob_id.clone(),
        exit_reason: TemporaryCouncilExitReason::CoordinatorInterrupted,
        rounds_completed,
        exchanges,
        merge: TemporaryCouncilMergeOutcome::NotAttempted {
            reason: "the coordinator did not survive to apply the merge-back policy".to_string(),
        },
        participants: record.participants.iter().map(provenance).collect(),
        truncated_exchange_count,
        merge_truncated: false,
        durability: record.durability,
        concluded_at: state.temporary_council_now(),
    }
}

fn provenance(
    custody: &TemporaryCouncilParticipantCustody,
) -> TemporaryCouncilParticipantProvenance {
    TemporaryCouncilParticipantProvenance {
        order: custody.order,
        role: custody.role.clone(),
        source_mob_id: custody.source_mob_id.clone(),
        source_identity: custody.source_identity.clone(),
        target_identity: custody.target_identity.clone(),
        scope: custody.scope,
        capability_request_id: custody.capability_request_id.clone(),
        capability: custody
            .capability_ref
            .as_ref()
            .map(TemporaryCouncilCapabilityProvenance::from_reference),
        attachment_id: custody.attachment_id.clone(),
        seated: custody.seated,
    }
}

// ===========================================================================
// Owned execution
// ===========================================================================

struct CouncilRun {
    state: Arc<MobMcpState>,
    validated: ValidatedRequest,
    record: TemporaryCouncilRecord,
    host_bootstrap: TemporaryCouncilHostBootstrap,
    /// This executor's claim. Every machine advance carries it, so a takeover
    /// by another process fences this task at its very next commit.
    claim: CouncilClaim,
}

/// Terminal of the discussion phase, before merge and cleanup.
struct DiscussionOutcome {
    exit_reason: TemporaryCouncilExitReason,
    rounds_completed: u32,
}

impl CouncilRun {
    async fn execute(mut self) -> Result<TemporaryCouncilOutcome, TemporaryCouncilError> {
        let store = self.state.temporary_council_store().clone();

        // 1. The record is already durable (admission persisted it before any
        //    side effect). Everything below is recorded as it happens.
        let seating = self.seat_participants().await;
        let discussion = match seating {
            Err(exit) => DiscussionOutcome {
                exit_reason: exit,
                rounds_completed: 0,
            },
            Ok(()) => match self.wire_full_mesh().await {
                Err(exit) => DiscussionOutcome {
                    exit_reason: exit,
                    rounds_completed: 0,
                },
                Ok(()) => {
                    self.advance(self.start_discussion_input()).await?;
                    self.run_rounds().await?
                }
            },
        };

        // 5. Explicit merge. A council that never reached a runnable
        //    discussion still enters merge so the policy can produce its typed
        //    not-attempted outcome — the machine admits that arm explicitly.
        self.advance(self.start_merge_input()).await?;
        let (merge, merge_truncated) = self.apply_merge(&discussion.exit_reason).await?;

        // 6. Seal the immutable result BEFORE cleanup, so a later cleanup
        //    failure cannot invalidate it.
        let truncated_exchange_count = self
            .record
            .exchanges
            .iter()
            .filter(|receipt| receipt.truncated())
            .count() as u32;
        let result = TemporaryCouncilResult {
            council_id: self.record.council_id.clone(),
            request_fingerprint: self.record.request_fingerprint.clone(),
            temporary_mob_id: self.record.temporary_mob_id.clone(),
            exit_reason: discussion.exit_reason,
            rounds_completed: discussion.rounds_completed,
            exchanges: self.record.exchanges.clone(),
            merge,
            participants: self.record.participants.iter().map(provenance).collect(),
            truncated_exchange_count,
            merge_truncated,
            durability: self.record.durability,
            concluded_at: self.state.temporary_council_now(),
        };
        self.record.result = Some(result.clone());
        self.advance(self.seal_input()).await?;

        // 7. Cleanup, reported separately from the result.
        let cleanup = cleanup_council(&self.state, &self.record, cleanup_budget(&self.state)).await;
        self.record.cleanup = Some(cleanup.clone());
        let (next, effects) = advance_machine(
            &self.record.machine_state,
            self.cleanup_input(cleanup.settled()),
        )?;
        reject_if_fenced(&self.record.council_id, &effects)?;
        self.record.machine_state = next;
        self.record.claim_lease_expires_at =
            active_claim_lease_expiry(&self.state, self.record.deadline);
        self.record = store
            .commit(&self.record)
            .await
            .map_err(TemporaryCouncilError::store)?;

        Ok(TemporaryCouncilOutcome {
            result,
            cleanup,
            replayed: false,
        })
    }

    /// Advance the canonical machine under THIS executor's claim, then persist.
    ///
    /// A fenced command aborts the task: another coordinator took the council
    /// over after observing this executor's lease expired, and a stale
    /// executor must not advance, seal, or settle anything.
    async fn advance(
        &mut self,
        input: TemporaryCouncilLifecycleInput,
    ) -> Result<(), TemporaryCouncilError> {
        let (next, effects) = advance_machine(&self.record.machine_state, input)?;
        reject_if_fenced(&self.record.council_id, &effects)?;
        self.record.machine_state = next;
        self.commit().await
    }

    fn start_discussion_input(&self) -> TemporaryCouncilLifecycleInput {
        TemporaryCouncilLifecycleInput::StartDiscussion {
            claim_id: self.claim.claim_id.clone(),
            claim_epoch: self.claim.claim_epoch,
        }
    }

    fn start_merge_input(&self) -> TemporaryCouncilLifecycleInput {
        TemporaryCouncilLifecycleInput::StartMerge {
            claim_id: self.claim.claim_id.clone(),
            claim_epoch: self.claim.claim_epoch,
        }
    }

    fn seal_input(&self) -> TemporaryCouncilLifecycleInput {
        TemporaryCouncilLifecycleInput::SealResult {
            claim_id: self.claim.claim_id.clone(),
            claim_epoch: self.claim.claim_epoch,
        }
    }

    fn cleanup_input(&self, settled: bool) -> TemporaryCouncilLifecycleInput {
        if settled {
            TemporaryCouncilLifecycleInput::RecordCleanupSettled {
                claim_id: self.claim.claim_id.clone(),
                claim_epoch: self.claim.claim_epoch,
            }
        } else {
            TemporaryCouncilLifecycleInput::RecordCleanupDebt {
                claim_id: self.claim.claim_id.clone(),
                claim_epoch: self.claim.claim_epoch,
            }
        }
    }

    async fn commit(&mut self) -> Result<(), TemporaryCouncilError> {
        self.record.claim_lease_expires_at =
            active_claim_lease_expiry(&self.state, self.record.deadline);
        self.record = self
            .state
            .temporary_council_store()
            .commit(&self.record)
            .await
            .map_err(TemporaryCouncilError::store)?;
        Ok(())
    }

    fn remaining(&self) -> Option<Duration> {
        (self.record.deadline - self.state.temporary_council_now())
            .to_std()
            .ok()
    }

    async fn temporary_handle(&self) -> Result<MobHandle, MobError> {
        self.state.handle_for(&self.record.temporary_mob_id).await
    }

    /// 2. Create the real temporary mob, then acquire and seat each capability.
    async fn seat_participants(&mut self) -> Result<(), TemporaryCouncilExitReason> {
        let mut bound_hosts = BTreeSet::new();
        // The temporary mob is an ordinary explicit mob created from the
        // caller's own definition template through the ordinary path.
        if self
            .state
            .handle_for(&self.record.temporary_mob_id)
            .await
            .is_err()
            && let Err(error) = self
                .state
                .mob_create_definition_with_owner_bridge_session(
                    self.validated.definition.clone(),
                    SessionId::new(),
                    false,
                    false,
                )
                .await
        {
            return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                participant_order: 0,
                detail: format!("temporary mob creation failed: {error}"),
            });
        }

        // Bind the caller-declared hosts BEFORE any seating: a HOST-owned
        // capability is refused on an unbound target before any bridge
        // traffic, so this must complete first or not at all.
        if !self.host_bootstrap.host_bindings.is_empty() {
            let temporary = match self.temporary_handle().await {
                Ok(handle) => handle,
                Err(error) => {
                    return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                        participant_order: 0,
                        detail: format!("temporary mob is unavailable: {error}"),
                    });
                }
            };
            for binding in self.host_bootstrap.host_bindings.clone() {
                let Some(remaining) = self.remaining() else {
                    return Err(TemporaryCouncilExitReason::DeadlineExceeded);
                };
                match tokio::time::timeout(remaining, temporary.bind_host(binding)).await {
                    Ok(Ok(report)) => {
                        bound_hosts.insert(report.host_id);
                    }
                    Ok(Err(error)) => {
                        return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                            participant_order: 0,
                            detail: format!("temporary mob host bind failed: {error}"),
                        });
                    }
                    Err(_) => return Err(TemporaryCouncilExitReason::DeadlineExceeded),
                }
            }
        }

        for index in 0..self.record.participants.len() {
            let custody = self.record.participants[index].clone();
            let Some(remaining) = self.remaining() else {
                return Err(TemporaryCouncilExitReason::DeadlineExceeded);
            };

            let source = match self.state.handle_for(&custody.source_mob_id).await {
                Ok(handle) => handle,
                Err(error) => {
                    return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                        participant_order: custody.order,
                        detail: format!(
                            "source mob {} is unavailable: {error}",
                            custody.source_mob_id
                        ),
                    });
                }
            };
            let source_profile = source
                .effective_member_profile_witness(&custody.source_identity)
                .await
                .map_err(
                    |error| TemporaryCouncilExitReason::ParticipantSeatingFailed {
                        participant_order: custody.order,
                        detail: format!(
                            "source member '{}' execution profile is unavailable: {error}",
                            custody.source_identity
                        ),
                    },
                )?;
            let target_profile = self
                .validated
                .definition
                .profiles
                .get(&custody.target_profile)
                .ok_or_else(|| TemporaryCouncilExitReason::ParticipantSeatingFailed {
                    participant_order: custody.order,
                    detail: format!(
                        "temporary profile '{}' disappeared before seating",
                        custody.target_profile
                    ),
                })?;
            if !same_source_execution_profile(target_profile, source_profile.profile()) {
                return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                    participant_order: custody.order,
                    detail: format!(
                        "temporary profile '{}' would widen or alter source member '{}' execution \
                         context",
                        custody.target_profile, custody.source_identity
                    ),
                });
            }

            // Cleanup starts after the council deadline, so capability custody
            // includes the configured cleanup budget. The capability layer's
            // absolute maximum remains authoritative if that margin cannot fit.
            let ttl = remaining
                .saturating_add(self.state.temporary_council_cleanup_budget())
                .max(Duration::from_secs(60))
                .min(MAX_FORKED_PARTICIPANT_TTL);
            // 2a. Persist the INTENT to acquire BEFORE calling the source
            //     owner. A crash or a failed post-create commit then leaves an
            //     explicit "may exist" marker rather than a false absence, so
            //     cleanup reconciles by the deterministic request identity
            //     instead of assuming nothing was created.
            self.record.participants[index].acquisition = TemporaryCouncilAcquisition::Pending;
            if let Err(error) = self.commit().await {
                // The create call was never issued, so nothing can exist yet:
                // this is a true absence, not an ambiguity. Roll the in-memory
                // marker back so cleanup does not invent a phantom debt.
                self.record.participants[index].acquisition =
                    TemporaryCouncilAcquisition::NotAttempted;
                return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                    participant_order: custody.order,
                    detail: format!("acquisition intent could not be persisted: {error}"),
                });
            }

            // The absolute deadline wraps this await too: capability creation
            // executes at the source owner and can block on a remote host.
            let created = tokio::time::timeout(
                remaining,
                source.create_forked_participant_with_profile_witness(
                    self.state.console_principal_snapshot(),
                    custody.source_identity.clone(),
                    source_profile,
                    custody.capability_request_id.clone(),
                    self.validated
                        .request
                        .participants
                        .iter()
                        .find(|spec| spec.order == custody.order)
                        .and_then(|spec| spec.prefix_message_count),
                    custody.scope,
                    // v1 reuse policy: one attachment, one council.
                    ForkedParticipantReusePolicy::OneShot,
                    ttl,
                ),
            )
            .await;
            let capability = match created {
                Ok(Ok(capability)) => capability,
                Ok(Err(error)) => {
                    return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                        participant_order: custody.order,
                        detail: format!("capability creation failed: {error}"),
                    });
                }
                Err(_) => return Err(TemporaryCouncilExitReason::DeadlineExceeded),
            };

            // Persist exact custody after each step, BEFORE the attach: the
            // FULL immutable reference is what makes a crash here recoverable.
            // For a HOST-owned capability the owner's record lives in the
            // remote host's store, so realm-local custody could never resolve
            // it by request id — the reference itself is the only thing that
            // routes revocation back to its owner.
            self.record.participants[index].acquisition = TemporaryCouncilAcquisition::Acquired;
            self.record.participants[index].capability_correlation_hint =
                Some(capability.capability_id().correlation_hint());
            self.record.participants[index].capability_ref = Some(capability.clone());
            if let Err(error) = self.commit().await {
                // The capability EXISTS and we hold its reference in memory.
                // Try once more to record it as explicitly ambiguous rather
                // than losing it; if that also fails the durable state stays
                // `Pending`, which cleanup reads as "may exist".
                self.record.participants[index].acquisition =
                    TemporaryCouncilAcquisition::Ambiguous;
                let reconciled = self.commit().await.is_ok();
                return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                    participant_order: custody.order,
                    detail: format!(
                        "capability custody could not be persisted ({error}); the capability \
                         was created and is recorded as {} for reconciliation",
                        if reconciled { "ambiguous" } else { "pending" }
                    ),
                });
            }

            if let ForkedParticipantOwnerRoute::Host { host_id, .. } = capability.owner_route()
                && !bound_hosts.contains(host_id.as_str())
            {
                let Some(remaining) = self.remaining() else {
                    return Err(TemporaryCouncilExitReason::DeadlineExceeded);
                };
                let temporary = match self.temporary_handle().await {
                    Ok(handle) => handle,
                    Err(error) => {
                        return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                            participant_order: custody.order,
                            detail: format!("temporary mob is unavailable: {error}"),
                        });
                    }
                };
                let target_supervisor = match temporary.host_binding_supervisor_spec().await {
                    Ok(supervisor) => supervisor,
                    Err(error) => {
                        return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                            participant_order: custody.order,
                            detail: format!(
                                "temporary mob supervisor is unavailable for host binding: {error}"
                            ),
                        });
                    }
                };
                let descriptor = match tokio::time::timeout(
                    remaining,
                    source.issue_host_binding_descriptor(
                        host_id.as_str(),
                        &self.record.temporary_mob_id,
                        target_supervisor,
                    ),
                )
                .await
                {
                    Ok(Ok(descriptor)) => descriptor,
                    Ok(Err(error)) => {
                        return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                            participant_order: custody.order,
                            detail: format!(
                                "source host descriptor handoff failed for '{}': {error}",
                                host_id.as_str()
                            ),
                        });
                    }
                    Err(_) => return Err(TemporaryCouncilExitReason::DeadlineExceeded),
                };
                let binding = match HostBindRequest::from_delegated_descriptor(
                    &descriptor.descriptor,
                    descriptor.delegated_bootstrap_proof,
                    descriptor.target_supervisor,
                ) {
                    Ok(binding) => binding,
                    Err(error) => {
                        return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                            participant_order: custody.order,
                            detail: format!(
                                "source host descriptor handoff was invalid for '{}': {error}",
                                host_id.as_str()
                            ),
                        });
                    }
                };
                let Some(remaining) = self.remaining() else {
                    return Err(TemporaryCouncilExitReason::DeadlineExceeded);
                };
                match tokio::time::timeout(remaining, temporary.bind_host(binding)).await {
                    Ok(Ok(report)) => {
                        bound_hosts.insert(report.host_id);
                    }
                    Ok(Err(error)) => {
                        return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                            participant_order: custody.order,
                            detail: format!(
                                "temporary mob could not bind source host '{}': {error}",
                                host_id.as_str()
                            ),
                        });
                    }
                    Err(_) => return Err(TemporaryCouncilExitReason::DeadlineExceeded),
                }
            }

            let temporary = match self.temporary_handle().await {
                Ok(handle) => handle,
                Err(error) => {
                    return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                        participant_order: custody.order,
                        detail: format!("temporary mob is unavailable: {error}"),
                    });
                }
            };
            let mut spec = SpawnMemberSpec::new(
                custody.target_profile.clone(),
                custody.target_identity.clone(),
            );
            // A council drives explicit bounded turns, so the seated branch is
            // turn-driven regardless of the template profile's mode: an
            // autonomous inbox loop cannot serve a tracked bounded turn.
            spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
            let Some(remaining) = self.remaining() else {
                return Err(TemporaryCouncilExitReason::DeadlineExceeded);
            };
            let spawned = tokio::time::timeout(
                remaining,
                temporary.spawn_attached_forked_participant(
                    self.state.console_principal_snapshot(),
                    &capability,
                    custody.attachment_id.clone(),
                    spec,
                ),
            )
            .await;
            let seated = match spawned {
                Ok(Ok(seated)) => seated,
                Ok(Err(error)) => {
                    return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                        participant_order: custody.order,
                        detail: format!("attached spawn failed: {error}"),
                    });
                }
                Err(_) => return Err(TemporaryCouncilExitReason::DeadlineExceeded),
            };

            self.record.participants[index].seated = true;
            self.record.participants[index].seated_session_id =
                Some(seated.capability.fork_session_id().clone());
            if let Err(error) = self.commit().await {
                return Err(TemporaryCouncilExitReason::ParticipantSeatingFailed {
                    participant_order: custody.order,
                    detail: format!("seating custody could not be persisted: {error}"),
                });
            }
        }
        Ok(())
    }

    /// 3. Wire a FULL MESH between seated participants.
    ///
    /// A council is a discussion, not a pipeline: every participant must be
    /// able to address every other one. The observed
    /// `MobWireMembersBatchReport` is checked against the requested edge set,
    /// so a partial wiring is reported as such rather than assumed complete.
    async fn wire_full_mesh(&mut self) -> Result<(), TemporaryCouncilExitReason> {
        let seated: Vec<AgentIdentity> = self
            .record
            .participants
            .iter()
            .filter(|participant| participant.seated)
            .map(|participant| participant.target_identity.clone())
            .collect();
        if seated.len() < 2 {
            // A one-participant council has no edges; that is not a wiring
            // failure.
            return Ok(());
        }

        let mut edges = Vec::new();
        for (index, left) in seated.iter().enumerate() {
            for right in seated.iter().skip(index + 1) {
                edges.push((left.clone(), right.clone()));
            }
        }
        let requested: BTreeSet<(AgentIdentity, AgentIdentity)> = edges
            .iter()
            .map(|(left, right)| normalized_edge(left, right))
            .collect();

        let handle = self.temporary_handle().await.map_err(|error| {
            TemporaryCouncilExitReason::WiringIncomplete {
                detail: format!("temporary mob is unavailable: {error}"),
            }
        })?;
        let Some(remaining) = self.remaining() else {
            return Err(TemporaryCouncilExitReason::DeadlineExceeded);
        };
        let report = tokio::time::timeout(remaining, handle.wire_members_batch(edges))
            .await
            .map_err(|_| TemporaryCouncilExitReason::DeadlineExceeded)?
            .map_err(|error| TemporaryCouncilExitReason::WiringIncomplete {
                detail: format!("wire_members_batch failed: {error}"),
            })?;
        let observed: BTreeSet<(AgentIdentity, AgentIdentity)> = report
            .wired
            .iter()
            .chain(report.already_wired.iter())
            .map(|edge| normalized_edge(&edge.a, &edge.b))
            .collect();
        let missing: Vec<String> = requested
            .difference(&observed)
            .map(|(left, right)| format!("{left}<->{right}"))
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(TemporaryCouncilExitReason::WiringIncomplete {
                detail: format!(
                    "full-mesh wiring is incomplete: {} of {} edges missing ({})",
                    missing.len(),
                    requested.len(),
                    missing.join(", ")
                ),
            })
        }
    }
}

fn normalized_edge(left: &AgentIdentity, right: &AgentIdentity) -> (AgentIdentity, AgentIdentity) {
    if left <= right {
        (left.clone(), right.clone())
    } else {
        (right.clone(), left.clone())
    }
}

// ===========================================================================
// Bounded discussion rounds
// ===========================================================================

/// One committed bounded turn, projected for the council record.
struct BoundedExchange {
    text: String,
    truncated: bool,
    session_id: meerkat_core::types::SessionId,
}

#[derive(Debug, thiserror::Error)]
enum DeliveryFailure {
    #[error("the council deadline elapsed before the turn was {phase}")]
    DeadlineExceeded { phase: &'static str },
    #[error("temporary mob is unavailable: {detail}")]
    MobUnavailable { detail: String },
    #[error("delivery identity is not canonical: {detail}")]
    InvalidDeliveryIdentity { detail: String },
    #[error("bounded result spec rejected: {detail}")]
    InvalidResultSpec { detail: String },
    #[error("turn admission failed: {detail}")]
    Admission { detail: String },
    #[error("turn failed: {detail}")]
    Turn { detail: String },
    #[error("turn did not complete: {status:?}")]
    Incomplete {
        status: meerkat_mob::BoundedHelperResultStatus,
    },
}

#[derive(Debug, thiserror::Error)]
enum MergeTurnError {
    #[error("{identity} is not a council participant")]
    NotParticipant { identity: AgentIdentity },
    #[error("the council deadline elapsed before merge-back")]
    DeadlineExceeded,
    #[error("merge {stage} custody could not be persisted: {source}")]
    Persistence {
        stage: &'static str,
        #[source]
        source: TemporaryCouncilError,
    },
    #[error(transparent)]
    Delivery(#[from] DeliveryFailure),
}

impl CouncilRun {
    /// 4. Run bounded sequential rounds.
    ///
    /// Each exchange persists its deterministic delivery identity BEFORE the
    /// send and its receiver-bounded result before the next turn starts. The
    /// absolute deadline wraps every await.
    async fn run_rounds(&mut self) -> Result<DiscussionOutcome, TemporaryCouncilError> {
        let bounds = self.validated.request.bounds.clone();
        let mut sequence: u32 = 0;
        let mut rounds_completed: u32 = 0;
        let mut exit = TemporaryCouncilExitReason::Completed;

        'rounds: for round in 0..bounds.max_rounds {
            let mut completed_in_round = false;
            for index in 0..self.record.participants.len() {
                let custody = self.record.participants[index].clone();
                if !custody.seated {
                    continue;
                }
                if sequence >= bounds.max_exchanges {
                    exit = TemporaryCouncilExitReason::MaxExchangesReached;
                    break 'rounds;
                }
                let Some(remaining) = self.remaining() else {
                    exit = TemporaryCouncilExitReason::DeadlineExceeded;
                    break 'rounds;
                };

                let receipt_index = self.record.exchanges.len();
                let idempotency_key =
                    self.record
                        .council_id
                        .delivery_idempotency_key(round, custody.order, "round");
                let correlation_id = deterministic_correlation_id(&idempotency_key);
                self.record.exchanges.push(TemporaryCouncilExchangeReceipt {
                    round,
                    sequence,
                    participant_order: custody.order,
                    target_identity: custody.target_identity.clone(),
                    delivery_idempotency_key: idempotency_key.clone(),
                    delivery_correlation_id: correlation_id.clone(),
                    started_at: self.state.temporary_council_now(),
                    outcome: TemporaryCouncilExchangeOutcome::Pending,
                });
                self.commit().await?;

                let content = self.round_prompt(round, &custody);
                let injected = self.injected_context();
                let (outcome, deadline_exceeded) = match self
                    .deliver_bounded_turn(
                        &custody.target_identity,
                        content,
                        injected,
                        &idempotency_key,
                        &correlation_id,
                        format!("council-r{round}-p{}", custody.order),
                        bounds.max_result_bytes,
                        remaining,
                    )
                    .await
                {
                    Ok(exchange) => {
                        completed_in_round = true;
                        sequence = sequence.saturating_add(1);
                        (
                            TemporaryCouncilExchangeOutcome::Completed {
                                text: exchange.text,
                                truncated: exchange.truncated,
                                session_id: exchange.session_id,
                                completed_at: self.state.temporary_council_now(),
                            },
                            false,
                        )
                    }
                    Err(error) => {
                        let deadline_exceeded =
                            matches!(&error, DeliveryFailure::DeadlineExceeded { .. });
                        (
                            TemporaryCouncilExchangeOutcome::Failed {
                                detail: error.to_string(),
                                failed_at: self.state.temporary_council_now(),
                            },
                            deadline_exceeded,
                        )
                    }
                };
                let failed = matches!(outcome, TemporaryCouncilExchangeOutcome::Failed { .. });
                let detail = match &outcome {
                    TemporaryCouncilExchangeOutcome::Failed { detail, .. } => detail.clone(),
                    _ => String::new(),
                };
                self.record.exchanges[receipt_index].outcome = outcome;
                self.commit().await?;

                if failed {
                    exit = if deadline_exceeded {
                        TemporaryCouncilExitReason::DeadlineExceeded
                    } else {
                        TemporaryCouncilExitReason::ExchangeFailed {
                            round,
                            target_identity: custody.target_identity.clone(),
                            detail,
                        }
                    };
                    break 'rounds;
                }
            }
            if completed_in_round {
                rounds_completed = rounds_completed.saturating_add(1);
            }
        }

        Ok(DiscussionOutcome {
            exit_reason: exit,
            rounds_completed,
        })
    }

    fn round_prompt(&self, round: u32, custody: &TemporaryCouncilParticipantCustody) -> String {
        if round == 0 {
            format!(
                "Council topic: {}\n\nYou are '{}'. State your position concisely.",
                self.validated.request.topic, custody.role
            )
        } else {
            format!(
                "Council round {} on the same topic. You are '{}'. Respond to the other \
                 participants' latest contributions, which are attached as context.",
                round + 1,
                custody.role
            )
        }
    }

    /// Prior exchange content as TYPED injected context.
    ///
    /// The discussion history is delivered alongside the turn as typed
    /// injected-context messages rather than rewritten into a system prompt:
    /// the member's own instructions stay the source's, and the transcript
    /// records what was injected.
    fn injected_context(&self) -> Vec<ContentInput> {
        let mut entries: Vec<ContentInput> = Vec::new();
        let start = self
            .record
            .exchanges
            .len()
            .saturating_sub(TEMPORARY_COUNCIL_MAX_INJECTED_CONTEXT_ENTRIES);
        for receipt in &self.record.exchanges[start..] {
            let Some(text) = receipt.completed_text() else {
                continue;
            };
            let role = self
                .record
                .participant(receipt.participant_order)
                .map(|participant| participant.role.as_str())
                .unwrap_or("participant");
            entries.push(ContentInput::from(format!(
                "[council round {} | {} | {}] {text}",
                receipt.round + 1,
                role,
                receipt.target_identity
            )));
        }
        entries
    }

    #[allow(clippy::too_many_arguments)]
    async fn deliver_bounded_turn(
        &self,
        identity: &AgentIdentity,
        content: String,
        injected: Vec<ContentInput>,
        idempotency_key: &str,
        correlation_id: &str,
        label: String,
        max_bytes: usize,
        remaining: Duration,
    ) -> Result<BoundedExchange, DeliveryFailure> {
        let handle =
            self.temporary_handle()
                .await
                .map_err(|error| DeliveryFailure::MobUnavailable {
                    detail: error.to_string(),
                })?;
        let delivery =
            meerkat_mob::store::MobDeliveryIdentity::new(idempotency_key, correlation_id).map_err(
                |error| DeliveryFailure::InvalidDeliveryIdentity {
                    detail: error.to_string(),
                },
            )?;
        let result_spec = BoundedResultSpec::new(label, max_bytes).map_err(|error| {
            DeliveryFailure::InvalidResultSpec {
                detail: error.to_string(),
            }
        })?;
        let work = WorkSpec::new(content, WorkOrigin::Internal).with_injected_context(injected);

        let turn = tokio::time::timeout(
            remaining,
            handle.start_work_for_identity_with_delivery_identity_bounded(
                identity.clone(),
                work,
                HandlingMode::Queue,
                delivery,
                result_spec.clone(),
            ),
        )
        .await
        .map_err(|_| DeliveryFailure::DeadlineExceeded { phase: "admitted" })?
        .map_err(|error| DeliveryFailure::Admission {
            detail: error.to_string(),
        })?;

        let Some(remaining) = self.remaining() else {
            return Err(DeliveryFailure::DeadlineExceeded { phase: "completed" });
        };
        let bounded = tokio::time::timeout(remaining, turn.wait_bounded(result_spec))
            .await
            .map_err(|_| DeliveryFailure::DeadlineExceeded { phase: "completed" })?
            .map_err(|error| DeliveryFailure::Turn {
                detail: describe_turn_failure(error.failure()),
            })?;

        let (_receipt, result) = bounded.into_parts();
        let status = result.result().status();
        let truncated = bounded_status_is_truncated(status);
        if !bounded_status_is_completed(status) {
            return Err(DeliveryFailure::Incomplete { status });
        }
        Ok(BoundedExchange {
            text: result.result().text().to_string(),
            truncated,
            session_id: result.session_id().clone(),
        })
    }
}

fn deterministic_correlation_id(idempotency_key: &str) -> String {
    uuid::Uuid::new_v5(&COUNCIL_DELIVERY_NAMESPACE, idempotency_key.as_bytes()).to_string()
}

fn bounded_status_is_completed(status: meerkat_mob::BoundedHelperResultStatus) -> bool {
    matches!(
        status,
        meerkat_mob::BoundedHelperResultStatus::Completed
            | meerkat_mob::BoundedHelperResultStatus::CompletedTruncated
    )
}

fn bounded_status_is_truncated(status: meerkat_mob::BoundedHelperResultStatus) -> bool {
    matches!(
        status,
        meerkat_mob::BoundedHelperResultStatus::CompletedTruncated
            | meerkat_mob::BoundedHelperResultStatus::FailedTruncated
            | meerkat_mob::BoundedHelperResultStatus::InProgressTruncated
            | meerkat_mob::BoundedHelperResultStatus::UnavailableTruncated
    )
}

fn describe_turn_failure(failure: &BoundedTurnFailure) -> String {
    match failure {
        BoundedTurnFailure::MissingAdmissionSession => {
            "no admitted session was reported".to_string()
        }
        BoundedTurnFailure::CompletedWithoutResult { .. } => {
            "the turn completed without a result".to_string()
        }
        BoundedTurnFailure::CallbackPending { tool_name, .. } => {
            format!("the turn is blocked on the pending callback tool '{tool_name}'")
        }
        BoundedTurnFailure::CallbackBatchPending { .. } => {
            "the turn is blocked on a pending callback tool batch".to_string()
        }
        BoundedTurnFailure::Cancelled { .. } => "the turn was cancelled".to_string(),
        BoundedTurnFailure::Abandoned { reason, .. } => {
            format!("the turn was abandoned: {reason}")
        }
        other => format!("{other:?}"),
    }
}

// ===========================================================================
// Explicit merge-back
// ===========================================================================

impl CouncilRun {
    /// 5. Apply the single explicit merge-back policy.
    ///
    /// Summary/structured/artifact policies each take exactly ONE final
    /// bounded turn on the named participant with a policy-specific
    /// instruction. Selected-transcript reads only the requested message
    /// indices from canonical history. `NoMerge` carries provenance and
    /// confirmation only. Nothing copies a whole transcript, and nothing
    /// writes into the caller's session.
    async fn apply_merge(
        &mut self,
        exit_reason: &TemporaryCouncilExitReason,
    ) -> Result<(TemporaryCouncilMergeOutcome, bool), TemporaryCouncilError> {
        let policy = self.validated.request.merge_back.clone();
        let seated_subject_is_available = policy
            .subject()
            .map(|subject| {
                self.record.participants.iter().any(|participant| {
                    participant.seated && &participant.target_identity == subject
                })
            })
            .unwrap_or(true);

        if let MergeBackPolicy::NoMerge = policy {
            return Ok((
                TemporaryCouncilMergeOutcome::NoMerge {
                    confirmed_participants: self.confirmed_participants().await,
                },
                false,
            ));
        }
        if !seated_subject_is_available {
            return Ok((
                TemporaryCouncilMergeOutcome::NotAttempted {
                    reason: "the merge-back participant was never seated".to_string(),
                },
                false,
            ));
        }
        if self.remaining().is_none() {
            return Ok((
                TemporaryCouncilMergeOutcome::NotAttempted {
                    reason: "the council deadline elapsed before merge-back".to_string(),
                },
                false,
            ));
        }
        if matches!(
            exit_reason,
            TemporaryCouncilExitReason::ParticipantSeatingFailed { .. }
                | TemporaryCouncilExitReason::WiringIncomplete { .. }
        ) {
            return Ok((
                TemporaryCouncilMergeOutcome::NotAttempted {
                    reason: "the council never reached a runnable discussion".to_string(),
                },
                false,
            ));
        }

        match policy {
            MergeBackPolicy::NoMerge => Ok((
                TemporaryCouncilMergeOutcome::NoMerge {
                    confirmed_participants: self.confirmed_participants().await,
                },
                false,
            )),
            MergeBackPolicy::SelectedTranscript {
                participant,
                exchange_sequences,
                max_bytes,
            } => Ok((
                self.merge_selected_exchanges(&participant, &exchange_sequences, max_bytes),
                false,
            )),
            MergeBackPolicy::BoundedTextSummary {
                finalizer,
                max_bytes,
            } => {
                let instruction = format!(
                    "Council merge-back: produce ONE bounded plain-text summary of the council \
                     discussion on '{}'. Do not restate the transcript.",
                    self.validated.request.topic
                );
                match self
                    .merge_turn(&finalizer, instruction, max_bytes, "summary")
                    .await
                {
                    Ok(exchange) => Ok((
                        TemporaryCouncilMergeOutcome::BoundedTextSummary {
                            finalizer,
                            text: exchange.text,
                            truncated: exchange.truncated,
                        },
                        exchange.truncated,
                    )),
                    Err(error) => Ok((
                        TemporaryCouncilMergeOutcome::Failed {
                            policy: TemporaryCouncilMergePolicyKind::BoundedTextSummary,
                            detail: error.to_string(),
                        },
                        false,
                    )),
                }
            }
            MergeBackPolicy::StructuredResult {
                finalizer,
                contract,
                max_bytes,
            } => {
                let identity = contract.identity()?;
                let rendered_schema =
                    serde_json::to_string(&contract.json_schema).map_err(|error| {
                        TemporaryCouncilError::invalid(format!(
                            "structured-result contract '{}' could not be rendered: {error}",
                            contract.schema_id
                        ))
                    })?;
                let instruction = format!(
                    "Council merge-back: reply with ONE strict JSON document summarizing the \
                     council outcome on '{}'. It MUST validate against contract '{}' v{}. Emit \
                     JSON only, with no prose and no code fence.\n\nSchema:\n{}",
                    self.validated.request.topic,
                    contract.schema_id,
                    contract.schema_version,
                    rendered_schema
                );
                match self
                    .merge_turn(&finalizer, instruction, max_bytes, "structured")
                    .await
                {
                    Ok(exchange) => {
                        // Strict: the exact bounded text must parse as JSON.
                        // No fence stripping, no substring extraction. Then it
                        // must SATISFY the declared contract — syntactic
                        // validity alone is not a structured result.
                        match serde_json::from_str::<serde_json::Value>(exchange.text.trim()) {
                            Ok(value) => match validate_structured(&contract, &value) {
                                Ok(()) => Ok((
                                    TemporaryCouncilMergeOutcome::StructuredResult {
                                        finalizer,
                                        contract: identity,
                                        value,
                                        truncated: exchange.truncated,
                                    },
                                    exchange.truncated,
                                )),
                                Err(error) => Ok((
                                    TemporaryCouncilMergeOutcome::Failed {
                                        policy: TemporaryCouncilMergePolicyKind::StructuredResult,
                                        detail: error.to_string(),
                                    },
                                    exchange.truncated,
                                )),
                            },
                            Err(error) => Ok((
                                TemporaryCouncilMergeOutcome::Failed {
                                    policy: TemporaryCouncilMergePolicyKind::StructuredResult,
                                    detail: format!(
                                        "the finalizer's bounded output is not strict JSON: \
                                         {error}"
                                    ),
                                },
                                exchange.truncated,
                            )),
                        }
                    }
                    Err(error) => Ok((
                        TemporaryCouncilMergeOutcome::Failed {
                            policy: TemporaryCouncilMergePolicyKind::StructuredResult,
                            detail: error.to_string(),
                        },
                        false,
                    )),
                }
            }
            MergeBackPolicy::DurableArtifactReference {
                participant,
                max_bytes,
            } => {
                let instruction =
                    "Council merge-back: reply with ONE strict JSON object describing the durable \
                     artifact you produced, using the fields uri (required), media_type, digest, \
                     and byte_len. Emit JSON only."
                        .to_string();
                match self
                    .merge_turn(&participant, instruction, max_bytes, "artifact")
                    .await
                {
                    Ok(exchange) => {
                        match serde_json::from_str::<TemporaryCouncilArtifactClaim>(
                            exchange.text.trim(),
                        ) {
                            Ok(claim) if !claim.uri.trim().is_empty() => Ok((
                                TemporaryCouncilMergeOutcome::DurableArtifactReference {
                                    participant,
                                    claim,
                                },
                                exchange.truncated,
                            )),
                            Ok(_) => Ok((
                                TemporaryCouncilMergeOutcome::Failed {
                                    policy:
                                        TemporaryCouncilMergePolicyKind::DurableArtifactReference,
                                    detail: "the artifact claim carries an empty uri".to_string(),
                                },
                                exchange.truncated,
                            )),
                            Err(error) => Ok((
                                TemporaryCouncilMergeOutcome::Failed {
                                    policy:
                                        TemporaryCouncilMergePolicyKind::DurableArtifactReference,
                                    detail: format!(
                                        "the participant's bounded output is not a typed artifact \
                                         claim: {error}"
                                    ),
                                },
                                exchange.truncated,
                            )),
                        }
                    }
                    Err(error) => Ok((
                        TemporaryCouncilMergeOutcome::Failed {
                            policy: TemporaryCouncilMergePolicyKind::DurableArtifactReference,
                            detail: error.to_string(),
                        },
                        false,
                    )),
                }
            }
        }
    }

    async fn confirmed_participants(&self) -> Vec<AgentIdentity> {
        // Live roster truth, read from the mob rather than mirrored.
        let Ok(handle) = self.temporary_handle().await else {
            return Vec::new();
        };
        let roster: BTreeSet<AgentIdentity> = handle
            .list_members()
            .await
            .into_iter()
            .map(|entry| entry.agent_identity)
            .collect();
        self.record
            .participants
            .iter()
            .filter(|participant| roster.contains(&participant.target_identity))
            .map(|participant| participant.target_identity.clone())
            .collect()
    }

    async fn merge_turn(
        &mut self,
        identity: &AgentIdentity,
        instruction: String,
        max_bytes: usize,
        purpose: &str,
    ) -> Result<BoundedExchange, MergeTurnError> {
        let order = self
            .record
            .participants
            .iter()
            .find(|participant| &participant.target_identity == identity)
            .map(|participant| participant.order)
            .ok_or_else(|| MergeTurnError::NotParticipant {
                identity: identity.clone(),
            })?;
        let round = self.validated.request.bounds.max_rounds;
        let idempotency_key = self
            .record
            .council_id
            .delivery_idempotency_key(round, order, purpose);
        let correlation_id = deterministic_correlation_id(&idempotency_key);

        let receipt_index = self.record.exchanges.len();
        // Sequence is a monotonic council-wide ordinal; failed discussion
        // exchanges do not consume the budget, so derive it from the highest
        // recorded ordinal rather than from the receipt count.
        let sequence = self
            .record
            .exchanges
            .iter()
            .map(|receipt| receipt.sequence)
            .max()
            .map_or(0, |highest| highest.saturating_add(1));
        self.record.exchanges.push(TemporaryCouncilExchangeReceipt {
            round,
            sequence,
            participant_order: order,
            target_identity: identity.clone(),
            delivery_idempotency_key: idempotency_key.clone(),
            delivery_correlation_id: correlation_id.clone(),
            started_at: self.state.temporary_council_now(),
            outcome: TemporaryCouncilExchangeOutcome::Pending,
        });
        self.commit()
            .await
            .map_err(|source| MergeTurnError::Persistence {
                stage: "delivery",
                source,
            })?;

        let remaining = self.remaining().ok_or(MergeTurnError::DeadlineExceeded)?;
        let injected = self.injected_context();
        let result = self
            .deliver_bounded_turn(
                identity,
                instruction,
                injected,
                &idempotency_key,
                &correlation_id,
                format!("council-merge-{purpose}-p{order}"),
                max_bytes,
                remaining,
            )
            .await
            .map_err(MergeTurnError::from);

        self.record.exchanges[receipt_index].outcome = match &result {
            Ok(exchange) => TemporaryCouncilExchangeOutcome::Completed {
                text: exchange.text.clone(),
                truncated: exchange.truncated,
                session_id: exchange.session_id.clone(),
                completed_at: self.state.temporary_council_now(),
            },
            Err(error) => TemporaryCouncilExchangeOutcome::Failed {
                detail: error.to_string(),
                failed_at: self.state.temporary_council_now(),
            },
        };
        self.commit()
            .await
            .map_err(|source| MergeTurnError::Persistence {
                stage: "receipt",
                source,
            })?;
        result
    }

    /// Select bounded COUNCIL EXCHANGES by sequence.
    ///
    /// The selection domain is deliberately the council's own exchange
    /// receipts. It NEVER indexes a seated fork session's transcript: that
    /// transcript opens with the inherited source prefix (system instructions
    /// and prior source turns), so a low index there would exfiltrate content
    /// the council never produced. Every excerpt here is text a participant
    /// emitted in reply to a council prompt, and each one carries its own
    /// round/sequence provenance.
    fn merge_selected_exchanges(
        &self,
        participant: &AgentIdentity,
        exchange_sequences: &[u32],
        max_bytes: usize,
    ) -> TemporaryCouncilMergeOutcome {
        let mut sequences = Vec::new();
        let mut excerpts = Vec::new();
        let mut used = 0usize;
        let mut truncated = false;

        for &sequence in exchange_sequences {
            let Some(receipt) = self.record.exchanges.iter().find(|receipt| {
                receipt.sequence == sequence && &receipt.target_identity == participant
            }) else {
                // A sequence that names no council exchange of this
                // participant simply yields nothing. It can never fall through
                // to some other source of text.
                continue;
            };
            let Some(text) = receipt.completed_text() else {
                continue;
            };
            let remaining_budget = max_bytes.saturating_sub(used);
            if remaining_budget == 0 {
                truncated = true;
                break;
            }
            let (text, cut) = truncate_utf8(text.to_string(), remaining_budget);
            truncated |= cut;
            used = used.saturating_add(text.len());
            sequences.push(sequence);
            excerpts.push(TemporaryCouncilSelectedExchange {
                sequence: receipt.sequence,
                round: receipt.round,
                participant_order: receipt.participant_order,
                target_identity: receipt.target_identity.clone(),
                text,
                truncated: cut,
            });
            if cut {
                break;
            }
        }

        TemporaryCouncilMergeOutcome::SelectedTranscript {
            participant: participant.clone(),
            exchange_sequences: sequences,
            excerpts,
            truncated,
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum StructuredValidationError {
    #[error("structured-result contract '{schema_id}' is not a compilable JSON Schema: {detail}")]
    InvalidSchema { schema_id: String, detail: String },
    #[error(
        "the finalizer's JSON does not satisfy contract '{schema_id}' v{schema_version}: {detail}"
    )]
    ContractViolation {
        schema_id: String,
        schema_version: u32,
        detail: String,
    },
}

/// Validate a parsed structured result against the caller's contract.
fn validate_structured(
    contract: &TemporaryCouncilStructuredContract,
    value: &serde_json::Value,
) -> Result<(), StructuredValidationError> {
    let validator = jsonschema::Validator::new(&contract.json_schema).map_err(|error| {
        StructuredValidationError::InvalidSchema {
            schema_id: contract.schema_id.clone(),
            detail: error.to_string(),
        }
    })?;
    validator
        .validate(value)
        .map_err(|error| StructuredValidationError::ContractViolation {
            schema_id: contract.schema_id.clone(),
            schema_version: contract.schema_version,
            detail: error.to_string(),
        })
}

/// Truncate on a UTF-8 boundary, reporting whether anything was cut.
fn truncate_utf8(mut text: String, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, true)
}

// ===========================================================================
// Cleanup
// ===========================================================================

fn cleanup_budget(state: &MobMcpState) -> DateTime<Utc> {
    bounded_time_add(
        state.temporary_council_now(),
        state.temporary_council_cleanup_budget(),
    )
}

fn active_claim_lease_expiry(state: &MobMcpState, deadline: DateTime<Utc>) -> DateTime<Utc> {
    let minimum = bounded_time_add(state.temporary_council_now(), TEMPORARY_COUNCIL_CLAIM_LEASE);
    let execution_horizon = bounded_time_add(deadline, state.temporary_council_cleanup_budget());
    minimum.max(execution_horizon)
}

fn bounded_time_add(start: DateTime<Utc>, duration: Duration) -> DateTime<Utc> {
    let delta = chrono::Duration::from_std(duration).unwrap_or(chrono::Duration::MAX);
    start
        .checked_add_signed(delta)
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// Remaining cleanup budget, or `None` once it is spent.
fn budget_remaining(state: &MobMcpState, budget: DateTime<Utc>) -> Option<Duration> {
    (budget - state.temporary_council_now()).to_std().ok()
}

/// A settled receipt with no attempts, for a council that had nothing to do.
fn settled_receipt(state: &MobMcpState) -> TemporaryCouncilCleanupReceipt {
    TemporaryCouncilCleanupReceipt {
        attempted_at: state.temporary_council_now(),
        attempts: 0,
        temporary_mob_destroyed: true,
        released_participants: Vec::new(),
        revoked_participants: Vec::new(),
        debts: Vec::new(),
        budget_exhausted: false,
    }
}

async fn cleanup_council(
    state: &Arc<MobMcpState>,
    record: &TemporaryCouncilRecord,
    budget: DateTime<Utc>,
) -> TemporaryCouncilCleanupReceipt {
    let attempts = record
        .cleanup
        .as_ref()
        .map(|previous| previous.attempts.saturating_add(1))
        .unwrap_or(1);
    let mut released_participants = record
        .cleanup
        .as_ref()
        .map(|previous| previous.released_participants.clone())
        .unwrap_or_default();
    let mut revoked_participants = record
        .cleanup
        .as_ref()
        .map(|previous| previous.revoked_participants.clone())
        .unwrap_or_default();
    let mut debts = Vec::new();
    let mut budget_exhausted = false;

    let (temporary, temporary_lookup_error) = match state.handle_for(&record.temporary_mob_id).await
    {
        Ok(handle) => (Some(handle), None),
        Err(MobError::MobNotFound(_)) => (None, None),
        Err(error) => (None, Some(error.to_string())),
    };
    if let Some(error) = temporary_lookup_error.as_ref() {
        debts.push(TemporaryCouncilCleanupDebt {
            subject: format!("mob:{}", record.temporary_mob_id),
            detail: format!("temporary mob lookup failed: {error}"),
        });
    }

    if let Some(handle) = temporary.as_ref() {
        for participant in record.participants.iter().filter(|p| p.seated) {
            if released_participants.contains(&participant.order) {
                continue;
            }
            let Some(remaining) = budget_remaining(state, budget) else {
                budget_exhausted = true;
                debts.push(TemporaryCouncilCleanupDebt {
                    subject: format!("participant:{}", participant.order),
                    detail: "cleanup budget expired before this member was retired".to_string(),
                });
                continue;
            };
            match tokio::time::timeout(
                remaining,
                handle.retire(participant.target_identity.clone()),
            )
            .await
            {
                Ok(Ok(())) => released_participants.push(participant.order),
                Ok(Err(MobError::MemberNotFound(_))) => {
                    // Already gone: the association released with it.
                    released_participants.push(participant.order);
                }
                Ok(Err(error)) => debts.push(TemporaryCouncilCleanupDebt {
                    subject: format!("participant:{}", participant.order),
                    detail: format!("retire failed: {error}"),
                }),
                Err(_) => {
                    budget_exhausted = true;
                    debts.push(TemporaryCouncilCleanupDebt {
                        subject: format!("participant:{}", participant.order),
                        detail: "retire exceeded the bounded cleanup budget".to_string(),
                    });
                }
            }
        }
    }

    for participant in record
        .participants
        .iter()
        .filter(|p| p.acquired_but_unattached())
    {
        if revoked_participants.contains(&participant.order) {
            continue;
        }
        let capability = match resolve_capability(state, participant).await {
            Ok(Some(capability)) => capability,
            Ok(None) => {
                // No activated capability record: nothing to revoke.
                revoked_participants.push(participant.order);
                continue;
            }
            Err(error) => {
                debts.push(TemporaryCouncilCleanupDebt {
                    subject: format!("capability:{}", participant.capability_request_id),
                    detail: error.to_string(),
                });
                continue;
            }
        };
        // Routing is by the capability's OWN owner route inside the mob actor,
        // so any live handle in this state can serve a Local route and the
        // source mob's handle is what reaches a Host route's bound supervisor.
        // Try the source mob first, then the temporary mob, then any managed
        // mob — a council must not strand a revocation just because the mob it
        // happened to ask through is gone.
        let source_revoker = state.handle_for(&participant.source_mob_id).await.ok();
        let fallback_revoker = match temporary.clone() {
            Some(handle) => Some(handle),
            None => state.any_managed_handle().await,
        };
        if source_revoker.is_none() && fallback_revoker.is_none() {
            debts.push(TemporaryCouncilCleanupDebt {
                subject: format!("capability:{}", participant.capability_request_id),
                detail: format!(
                    "no live mob handle can serve revocation for source mob {}",
                    participant.source_mob_id
                ),
            });
            continue;
        }
        let mut revocation_error = None;
        let mut revoked = false;
        for revoker in [source_revoker, fallback_revoker].into_iter().flatten() {
            let Some(remaining) = budget_remaining(state, budget) else {
                budget_exhausted = true;
                revocation_error =
                    Some("revocation exceeded the bounded cleanup budget".to_string());
                break;
            };
            match tokio::time::timeout(
                remaining,
                revoker.revoke_forked_participant(state.console_principal_snapshot(), &capability),
            )
            .await
            {
                Ok(Ok(_)) => {
                    revoked = true;
                    break;
                }
                // A capability the owner already carried to a terminal state
                // is converged, not cleanup debt.
                Ok(Err(MobError::ForkedParticipantRefused(refusal)))
                    if matches!(
                        refusal.as_ref(),
                        meerkat_mob::forked_participant::ForkedParticipantError::RevocationDenied {
                            reason:
                                meerkat_mob::machines::forked_participant_lifecycle::ForkedParticipantRevocationDenial::AlreadyTerminal,
                        }
                    ) =>
                {
                    revoked = true;
                    break;
                }
                Ok(Err(error)) => revocation_error = Some(error.to_string()),
                Err(_) => {
                    budget_exhausted = true;
                    revocation_error =
                        Some("revocation exceeded the bounded cleanup budget".to_string());
                    break;
                }
            }
        }
        if revoked {
            revoked_participants.push(participant.order);
        } else {
            debts.push(TemporaryCouncilCleanupDebt {
                subject: format!("capability:{}", participant.capability_request_id),
                detail: format!(
                    "revocation failed: {}",
                    revocation_error
                        .unwrap_or_else(|| "no revocation handle accepted the request".to_string())
                ),
            });
        }
    }

    let temporary_mob_destroyed = if temporary.is_some() {
        // Per-mob storage removal is owned by MobMcpState destroy semantics.
        match budget_remaining(state, budget) {
            Some(remaining) => {
                match tokio::time::timeout(remaining, state.mob_destroy(&record.temporary_mob_id))
                    .await
                {
                    Ok(Ok(_)) => true,
                    Ok(Err(error)) => {
                        debts.push(TemporaryCouncilCleanupDebt {
                            subject: format!("mob:{}", record.temporary_mob_id),
                            detail: format!("destroy failed: {error}"),
                        });
                        false
                    }
                    Err(_) => {
                        budget_exhausted = true;
                        debts.push(TemporaryCouncilCleanupDebt {
                            subject: format!("mob:{}", record.temporary_mob_id),
                            detail: "destroy exceeded the bounded cleanup budget".to_string(),
                        });
                        false
                    }
                }
            }
            None => {
                budget_exhausted = true;
                debts.push(TemporaryCouncilCleanupDebt {
                    subject: format!("mob:{}", record.temporary_mob_id),
                    detail: "cleanup budget expired before the temporary mob was destroyed"
                        .to_string(),
                });
                false
            }
        }
    } else if temporary_lookup_error.is_some() {
        false
    } else {
        // `handle_for` performs durable restoration before returning
        // `MobNotFound`; authoritative absence therefore proves the mob was
        // already destroyed or never reached creation.
        true
    };

    TemporaryCouncilCleanupReceipt {
        attempted_at: state.temporary_council_now(),
        attempts,
        temporary_mob_destroyed,
        released_participants,
        revoked_participants,
        debts,
        budget_exhausted,
    }
}

/// Resolve the exact capability reference cleanup must present.
///
/// The council record's own custody is authoritative and placement-blind: it
/// holds the FULL immutable reference persisted before the attach, so a
/// HOST-owned capability resolves here exactly as a local one does. Realm-local
/// capability custody is consulted only as a fallback for records written
/// before the reference was persisted, and its absence is never fatal for a
/// remote capability.
#[derive(Debug, thiserror::Error)]
enum CapabilityResolutionError {
    #[error("capability custody read failed: {source}")]
    Store {
        #[source]
        source: MobStoreError,
    },
    #[error(
        "acquisition is ambiguous: the create call for {request_id} was issued but neither a held \
         reference nor a realm-local record resolves it; the owner may still hold a capability"
    )]
    Ambiguous {
        request_id: meerkat_mob::forked_participant::ForkedParticipantRequestId,
    },
}

async fn resolve_capability(
    state: &Arc<MobMcpState>,
    participant: &TemporaryCouncilParticipantCustody,
) -> Result<Option<ForkedParticipantRef>, CapabilityResolutionError> {
    if let Some(capability) = participant.capability_ref.clone() {
        return Ok(Some(capability));
    }
    // No held reference. For a `Pending` acquisition the create outcome is
    // genuinely unknown, so reconcile by the deterministic request identity
    // rather than reporting absence.
    let record = state
        .forked_participant_store()
        .load_by_request_id(&participant.capability_request_id)
        .await
        .map_err(|source| CapabilityResolutionError::Store { source })?;
    match record.and_then(|record| record.sidecar.capability_ref) {
        Some(capability) => Ok(Some(capability)),
        None if participant.acquisition == TemporaryCouncilAcquisition::Pending => {
            Err(CapabilityResolutionError::Ambiguous {
                request_id: participant.capability_request_id.clone(),
            })
        }
        None => Ok(None),
    }
}
