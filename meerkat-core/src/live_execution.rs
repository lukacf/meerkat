//! Provider-neutral semantic vocabulary for channel-scoped live execution.
//!
//! Provider adapters carry opaque correlation into these types, but cannot use
//! provider identifiers as Meerkat authority. Exact interaction, channel, and
//! operation identities remain the control-plane fence.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::interaction::InteractionId;

const LIVE_CONTEXT_PREFIX_DOMAIN: &[u8] = b"meerkat.live-context-prefix.v1\0";
const LIVE_NORMALIZED_USER_INPUT_DOMAIN: &[u8] = b"meerkat.live-normalized-user-input.v1\0";
const LIVE_BRIDGE_REQUEST_DOMAIN: &[u8] = b"meerkat.live-bridge-request.v1\0";

/// Typed transcript-content revision minted only from one exact Session
/// snapshot. Provider and surface strings cannot construct this witness.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CanonicalContextRevision(String);

impl std::fmt::Debug for CanonicalContextRevision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CanonicalContextRevision([REDACTED])")
    }
}

impl CanonicalContextRevision {
    pub(crate) fn from_transcript_revision(revision: String) -> Self {
        Self(revision)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque identity of one live channel binding.
///
/// A replacement channel receives a new value. Semantic observations retain
/// this identity so a delayed callback from the old binding fails its fence.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveChannelId(String);

impl LiveChannelId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn random_uuid() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LiveChannelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Construction failure for one semantic live-execution carrier.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LiveExecutionIdentityError {
    #[error("live execution identity field '{field}' must not be empty")]
    EmptyField { field: &'static str },
}

fn require_nonempty(
    value: impl Into<String>,
    field: &'static str,
) -> Result<String, LiveExecutionIdentityError> {
    let value = value.into();
    if value.trim().is_empty() {
        return Err(LiveExecutionIdentityError::EmptyField { field });
    }
    Ok(value)
}

/// Opaque provider correlation joined before live delegation admission.
///
/// The strings are retained for exact comparison and provider round trips.
/// They are not Meerkat control handles and must not be logged.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct OpaqueProviderCorrelation {
    delegation_item_id: String,
    user_turn_id: String,
}

impl std::fmt::Debug for OpaqueProviderCorrelation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpaqueProviderCorrelation")
            .field("delegation_item_id", &"[REDACTED]")
            .field("user_turn_id", &"[REDACTED]")
            .finish()
    }
}

impl OpaqueProviderCorrelation {
    pub fn new(
        delegation_item_id: impl Into<String>,
        user_turn_id: impl Into<String>,
    ) -> Result<Self, LiveExecutionIdentityError> {
        Ok(Self {
            delegation_item_id: require_nonempty(delegation_item_id, "delegation_item_id")?,
            user_turn_id: require_nonempty(user_turn_id, "user_turn_id")?,
        })
    }

    #[must_use]
    pub fn delegation_item_id(&self) -> &str {
        &self.delegation_item_id
    }

    #[must_use]
    pub fn user_turn_id(&self) -> &str {
        &self.user_turn_id
    }
}

/// Exact provider-neutral correlation for one live user interaction.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct LiveUserTurnCorrelation {
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    provider: OpaqueProviderCorrelation,
}

impl std::fmt::Debug for LiveUserTurnCorrelation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveUserTurnCorrelation")
            .field("channel_id", &self.channel_id)
            .field("interaction_id", &self.interaction_id)
            .field("provider", &"[REDACTED]")
            .finish()
    }
}

impl LiveUserTurnCorrelation {
    pub fn new(
        channel_id: LiveChannelId,
        interaction_id: InteractionId,
        provider: OpaqueProviderCorrelation,
    ) -> Result<Self, LiveExecutionIdentityError> {
        if channel_id.as_str().trim().is_empty() {
            return Err(LiveExecutionIdentityError::EmptyField {
                field: "channel_id",
            });
        }
        Ok(Self {
            channel_id,
            interaction_id,
            provider,
        })
    }

    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub const fn interaction_id(&self) -> InteractionId {
        self.interaction_id
    }

    #[must_use]
    pub fn provider(&self) -> &OpaqueProviderCorrelation {
        &self.provider
    }

    /// Check the currently authorized channel binding before any semantic use.
    #[must_use]
    pub fn is_fenced_to(&self, authorized_channel: &LiveChannelId) -> bool {
        &self.channel_id == authorized_channel
    }
}

/// Opaque structural provider correlation for one Responses bridge call.
///
/// These are adapter keys already scoped to the current channel binding. They
/// are equality material only and never replace Meerkat operation identity.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LiveBridgeProviderCorrelation {
    provider_turn_ref: String,
    provider_delegation_ref: String,
    provider_call_ref: String,
}

impl std::fmt::Debug for LiveBridgeProviderCorrelation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveBridgeProviderCorrelation")
            .field("provider_turn_ref", &"[REDACTED]")
            .field("provider_delegation_ref", &"[REDACTED]")
            .field("provider_call_ref", &"[REDACTED]")
            .finish()
    }
}

impl LiveBridgeProviderCorrelation {
    pub fn new(
        provider_turn_ref: impl Into<String>,
        provider_delegation_ref: impl Into<String>,
        provider_call_ref: impl Into<String>,
    ) -> Result<Self, LiveExecutionIdentityError> {
        Ok(Self {
            provider_turn_ref: require_nonempty(provider_turn_ref, "provider_turn_ref")?,
            provider_delegation_ref: require_nonempty(
                provider_delegation_ref,
                "provider_delegation_ref",
            )?,
            provider_call_ref: require_nonempty(provider_call_ref, "provider_call_ref")?,
        })
    }

    #[must_use]
    pub fn provider_turn_ref(&self) -> &str {
        &self.provider_turn_ref
    }

    #[must_use]
    pub fn provider_delegation_ref(&self) -> &str {
        &self.provider_delegation_ref
    }

    #[must_use]
    pub fn provider_call_ref(&self) -> &str {
        &self.provider_call_ref
    }
}

/// Exact domain correlation for one durable-member Responses bridge
/// operation. Durable member identity and context revision are sealed in the
/// generated admission receipt, not accepted as provider tool arguments.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LiveBridgeOperationCorrelation {
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    provider: LiveBridgeProviderCorrelation,
}

impl std::fmt::Debug for LiveBridgeOperationCorrelation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveBridgeOperationCorrelation")
            .field("channel_id", &self.channel_id)
            .field("interaction_id", &self.interaction_id)
            .field("provider", &"[REDACTED]")
            .finish()
    }
}

impl LiveBridgeOperationCorrelation {
    pub fn new(
        channel_id: LiveChannelId,
        interaction_id: InteractionId,
        provider: LiveBridgeProviderCorrelation,
    ) -> Result<Self, LiveExecutionIdentityError> {
        if channel_id.as_str().trim().is_empty() {
            return Err(LiveExecutionIdentityError::EmptyField {
                field: "channel_id",
            });
        }
        Ok(Self {
            channel_id,
            interaction_id,
            provider,
        })
    }

    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub const fn interaction_id(&self) -> InteractionId {
        self.interaction_id
    }

    #[must_use]
    pub fn provider(&self) -> &LiveBridgeProviderCorrelation {
        &self.provider
    }
}

/// Content-safe digest of provider-authored bridge request bytes.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveBridgeRequestDigest(String);

impl LiveBridgeRequestDigest {
    pub fn derive(request: &str) -> Result<Self, LiveExecutionIdentityError> {
        require_nonempty(request, "request")?;
        let mut hasher = Sha256::new();
        hasher.update(LIVE_BRIDGE_REQUEST_DOMAIN);
        hasher.update((request.len() as u64).to_be_bytes());
        hasher.update(request.as_bytes());
        Ok(Self(format!("sha256:{:x}", hasher.finalize())))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveExecutionChannelPhase {
    Pending,
    Active,
    Revoked,
}

/// Provider-neutral execution strategy selected by the Meerkat profile.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveExecutionMode {
    FunctionBridge,
    ClientContext,
}

/// Independent runtime capability atoms. A mode is admissible only when its
/// corresponding atom is genuinely available in the current composition.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LiveExecutionCapabilities {
    pub function_bridge: bool,
    pub client_context: bool,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveBridgeOperationPhase {
    PreFinalInference,
    FinalInputAuthorized,
    CancellationAuthorized,
    ExecutionTerminal,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveBridgeEffectKind {
    ModelComputation,
    ReadOnlyMemorySnapshot,
    ToolDispatch,
    DurableMemoryMutation,
    Comms,
    HelperSpawn,
    ExternalIo,
}

/// Terminal observation for one consumed live bridge effect authority.
///
/// `Unknown` means dispatch began but its physical outcome cannot be proven.
/// It is terminal and must never be retried or relabeled as committed.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveBridgeEffectOutcome {
    Committed,
    Failed,
    Unknown,
}

impl LiveBridgeEffectKind {
    #[must_use]
    pub const fn allowed_before_final_input(self) -> bool {
        matches!(self, Self::ModelComputation | Self::ReadOnlyMemorySnapshot)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeerkatExecutionTerminal {
    Completed,
    Rejected,
    Failed,
    TimedOut,
    Unrecoverable,
    Cancelled,
    Superseded,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveBridgeCancellationReason {
    BargeIn,
    ChannelClose,
    Restart,
    ProtocolDrift,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveBridgeOutputKind {
    Success,
    FailureProjection,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveBridgeSubmissionState {
    SubmissionAuthorized,
    SubmissionAttemptClaimed,
    LocalWriteCompletedAwaitingProof,
    ProviderProcessed,
    ProviderRejected,
    SubmissionAmbiguous,
    CallExpired,
    CallAbandonedByClose,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveBridgeSubmissionObservation {
    ProviderProcessed,
    ProviderRejected,
    SubmissionAmbiguous,
    CallExpired,
    CallAbandonedByClose,
}

/// Provenance of actionable input joined to a provider delegation identity.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveHandoffInputProvenance {
    NormalizedHandoff,
    ProvisionalTranscriptSnapshot,
}

/// Actionable input staged before provider-final user admission.
#[derive(Clone, PartialEq, Eq)]
pub struct ProvisionalLiveHandoff {
    correlation: LiveUserTurnCorrelation,
    executor_input: String,
    provenance: LiveHandoffInputProvenance,
}

impl std::fmt::Debug for ProvisionalLiveHandoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionalLiveHandoff")
            .field("channel_id", self.correlation.channel_id())
            .field("interaction_id", &self.correlation.interaction_id())
            .field("provider_correlation", &"[REDACTED]")
            .field("executor_input", &"[REDACTED]")
            .field("provenance", &self.provenance)
            .finish()
    }
}

impl ProvisionalLiveHandoff {
    pub fn new(
        correlation: LiveUserTurnCorrelation,
        executor_input: impl Into<String>,
        provenance: LiveHandoffInputProvenance,
    ) -> Result<Self, LiveExecutionIdentityError> {
        Ok(Self {
            correlation,
            executor_input: require_nonempty(executor_input, "executor_input")?,
            provenance,
        })
    }

    #[must_use]
    pub fn correlation(&self) -> &LiveUserTurnCorrelation {
        &self.correlation
    }

    #[must_use]
    pub fn executor_input(&self) -> &str {
        &self.executor_input
    }

    #[must_use]
    pub const fn provenance(&self) -> LiveHandoffInputProvenance {
        self.provenance
    }

    #[must_use]
    pub fn normalized_input_digest(&self) -> NormalizedLiveUserInputDigest {
        NormalizedLiveUserInputDigest::derive_nonempty(&self.executor_input)
    }
}

/// Content-safe digest of already-normalized live user input.
///
/// Normalization policy is owned by the caller that produces the canonical
/// final transcript. This type binds the exact normalized bytes without
/// carrying those bytes into authority receipts or diagnostics.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NormalizedLiveUserInputDigest(String);

impl NormalizedLiveUserInputDigest {
    fn derive_nonempty(normalized_input: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(LIVE_NORMALIZED_USER_INPUT_DOMAIN);
        hasher.update((normalized_input.len() as u64).to_be_bytes());
        hasher.update(normalized_input.as_bytes());
        Self(format!("sha256:{:x}", hasher.finalize()))
    }

    pub fn derive(normalized_input: &str) -> Result<Self, LiveExecutionIdentityError> {
        require_nonempty(normalized_input, "normalized_input")?;
        Ok(Self::derive_nonempty(normalized_input))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Final classification of provisional handoff compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveHandoffReconciliation {
    Confirmed,
    MaterialConflict,
    Missing,
}

/// Machine-owned terminal disposition of canonical final live user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FinalLiveUserTranscriptDisposition {
    Committed,
    Missing,
}

/// Typed playback-prefix observation for an interrupted live assistant turn.
///
/// A reported prefix is evidence about the playback path only. `Unmeasured`
/// means no prefix evidence was available. Neither variant asserts delivery to
/// a person or biological hearing.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LiveAssistantPlaybackEvidence {
    PlaybackComplete,
    ReportedPrefix(String),
    Unmeasured,
}

impl LiveAssistantPlaybackEvidence {
    #[must_use]
    pub fn reported_prefix(&self) -> Option<&str> {
        match self {
            Self::ReportedPrefix(prefix) => Some(prefix),
            Self::PlaybackComplete | Self::Unmeasured => None,
        }
    }
}

/// SessionDocument-owned disposition of one playback-prefix observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiveAssistantPlaybackTruncationDisposition {
    PlaybackComplete,
    CommittedReportedPrefix,
    Unmeasured,
}

/// Sealed evidence that SessionDocument authority classified one exact live
/// playback observation before the session owner mutated canonical transcript.
#[derive(Clone, PartialEq, Eq)]
pub struct LiveAssistantPlaybackTruncationEvidence {
    session_id: crate::types::SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    response_id: String,
    item_id: String,
    content_index: u32,
    disposition: LiveAssistantPlaybackTruncationDisposition,
    canonical_prefix_chars: Option<u64>,
}

impl std::fmt::Debug for LiveAssistantPlaybackTruncationEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveAssistantPlaybackTruncationEvidence")
            .field("session_id", &self.session_id)
            .field("channel_id", &self.channel_id)
            .field("interaction_id", &self.interaction_id)
            .field("disposition", &self.disposition)
            .field("canonical_prefix_chars", &self.canonical_prefix_chars)
            .field("biological_hearing_claimed", &false)
            .finish()
    }
}

impl LiveAssistantPlaybackTruncationEvidence {
    #[must_use]
    pub fn session_id(&self) -> &crate::types::SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub const fn interaction_id(&self) -> InteractionId {
        self.interaction_id
    }

    #[must_use]
    pub const fn disposition(&self) -> LiveAssistantPlaybackTruncationDisposition {
        self.disposition
    }

    #[must_use]
    pub fn response_id(&self) -> &str {
        &self.response_id
    }

    #[must_use]
    pub fn item_id(&self) -> &str {
        &self.item_id
    }

    #[must_use]
    pub const fn content_index(&self) -> u32 {
        self.content_index
    }

    #[must_use]
    pub const fn canonical_prefix_chars(&self) -> Option<u64> {
        self.canonical_prefix_chars
    }

    /// Playback evidence never proves what a person biologically heard.
    #[must_use]
    pub const fn biological_hearing_claimed(&self) -> bool {
        false
    }
}

/// Sealed evidence that SessionDocument authority reconciled one canonical
/// final live user transcript after the session owner committed it.
///
/// The evidence is deliberately non-serializable and has no public
/// constructor. Meerkat runtime can compare the committed digest with its
/// staged provisional handoff, but callers cannot select `Confirmed`.
#[derive(Clone, PartialEq, Eq)]
pub struct FinalLiveUserTranscriptCommitEvidence {
    session_id: crate::types::SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    disposition: FinalLiveUserTranscriptDisposition,
    normalized_final_input_digest: Option<NormalizedLiveUserInputDigest>,
    committed_message_count: Option<usize>,
}

impl std::fmt::Debug for FinalLiveUserTranscriptCommitEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinalLiveUserTranscriptCommitEvidence")
            .field("session_id", &self.session_id)
            .field("channel_id", &self.channel_id)
            .field("interaction_id", &self.interaction_id)
            .field("disposition", &self.disposition)
            .finish_non_exhaustive()
    }
}

impl FinalLiveUserTranscriptCommitEvidence {
    #[must_use]
    pub fn session_id(&self) -> &crate::types::SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub const fn interaction_id(&self) -> InteractionId {
        self.interaction_id
    }

    #[must_use]
    pub const fn disposition(&self) -> FinalLiveUserTranscriptDisposition {
        self.disposition
    }

    #[must_use]
    pub fn normalized_final_input_digest(&self) -> Option<&NormalizedLiveUserInputDigest> {
        self.normalized_final_input_digest.as_ref()
    }

    /// Exact canonical transcript boundary observed by the SessionDocument
    /// owner immediately after committing the final user transcript.
    #[must_use]
    pub const fn committed_message_count(&self) -> Option<usize> {
        self.committed_message_count
    }
}

#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]
#[allow(improper_ctypes_definitions, unsafe_code)]
unsafe extern "Rust" {
    #[link_name = concat!(
        "__meerkat_session_generated_authority_bridge_token_is_valid_v1_live_user_transcript_",
        env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
    )]
    fn session_live_user_transcript_generated_authority_bridge_token_is_valid(
        token: &(dyn std::any::Any + Send + Sync),
    ) -> bool;
}

/// Construct canonical final-user-input evidence only for the matching
/// SessionDocument generated effect and the session owner's opaque bridge
/// token. This symbol is intentionally unavailable without the internal
/// generated-authority feature.
#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_core_session_generated_live_user_transcript_commit_build_v2_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) extern "Rust" fn session_generated_live_user_transcript_commit_build(
    token: &'static (dyn std::any::Any + Send + Sync),
    session_id: crate::types::SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    normalized_final_input_digest: Option<NormalizedLiveUserInputDigest>,
    committed_message_count: Option<usize>,
    effect: &crate::generated::session_document::SessionDocumentEffect,
) -> Result<FinalLiveUserTranscriptCommitEvidence, FinalLiveUserTranscriptCommitError> {
    #[allow(unsafe_code)]
    let valid =
        unsafe { session_live_user_transcript_generated_authority_bridge_token_is_valid(token) };
    if !valid {
        return Err(FinalLiveUserTranscriptCommitError::InvalidAuthorityToken);
    }

    use crate::generated::session_document::{
        LiveTranscriptReconciliation, SessionDocumentEffect, SessionDocumentKey,
    };
    let SessionDocumentEffect::LiveFinalUserTranscriptReconciled {
        session_id: effect_session_id,
        channel_id: effect_channel_id,
        interaction_id: effect_interaction_id,
        reconciliation,
    } = effect
    else {
        return Err(FinalLiveUserTranscriptCommitError::MissingTerminalEffect);
    };
    if effect_session_id != &SessionDocumentKey::new(session_id.to_string())
        || effect_channel_id != channel_id.as_str()
        || effect_interaction_id != &interaction_id.to_string()
    {
        return Err(FinalLiveUserTranscriptCommitError::Transition(
            "terminal effect did not match the exact session interaction".to_string(),
        ));
    }

    let disposition = match (
        reconciliation,
        normalized_final_input_digest.as_ref(),
        committed_message_count,
    ) {
        (LiveTranscriptReconciliation::Committed, Some(_), Some(message_count))
            if message_count > 0 =>
        {
            FinalLiveUserTranscriptDisposition::Committed
        }
        (LiveTranscriptReconciliation::Missing, None, None) => {
            FinalLiveUserTranscriptDisposition::Missing
        }
        _ => {
            return Err(FinalLiveUserTranscriptCommitError::Transition(
                "terminal effect and committed digest had inconsistent shape".to_string(),
            ));
        }
    };

    Ok(FinalLiveUserTranscriptCommitEvidence {
        session_id,
        channel_id,
        interaction_id,
        disposition,
        normalized_final_input_digest,
        committed_message_count,
    })
}

/// Seal one generated playback classification against the exact evidence the
/// session owner observed before canonical mutation.
#[cfg(all(meerkat_internal_generated_authority_bridge, not(test)))]
#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!(
    "__meerkat_core_session_generated_live_playback_truncation_build_v1_",
    env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")
))]
pub(crate) extern "Rust" fn session_generated_live_playback_truncation_build(
    token: &'static (dyn std::any::Any + Send + Sync),
    session_id: crate::types::SessionId,
    channel_id: LiveChannelId,
    interaction_id: InteractionId,
    response_id: &str,
    item_id: &str,
    content_index: u32,
    evidence: &LiveAssistantPlaybackEvidence,
    effect: &crate::generated::session_document::SessionDocumentEffect,
) -> Result<LiveAssistantPlaybackTruncationEvidence, LiveAssistantPlaybackTruncationError> {
    #[allow(unsafe_code)]
    let valid =
        unsafe { session_live_user_transcript_generated_authority_bridge_token_is_valid(token) };
    if !valid {
        return Err(LiveAssistantPlaybackTruncationError::InvalidAuthorityToken);
    }

    use crate::generated::session_document::{
        LiveAssistantPlaybackTerminalDisposition, SessionDocumentEffect, SessionDocumentKey,
    };
    let SessionDocumentEffect::LiveAssistantPlaybackTerminalResolved {
        session_id: effect_session_id,
        channel_id: effect_channel_id,
        interaction_id: effect_interaction_id,
        response_id: effect_response_id,
        item_id: effect_item_id,
        content_index: effect_content_index,
        disposition,
        canonical_chars,
        canonical_text_digest: _,
        biological_hearing_claimed,
    } = effect
    else {
        return Err(LiveAssistantPlaybackTruncationError::MissingTerminalEffect);
    };
    if effect_session_id != &SessionDocumentKey::new(session_id.to_string())
        || effect_channel_id != channel_id.as_str()
        || effect_interaction_id != &interaction_id.to_string()
        || effect_response_id != response_id
        || effect_item_id != item_id
        || *effect_content_index != u64::from(content_index)
    {
        return Err(LiveAssistantPlaybackTruncationError::Transition(
            "terminal effect did not match the exact session interaction".to_string(),
        ));
    }
    if *biological_hearing_claimed {
        return Err(LiveAssistantPlaybackTruncationError::Transition(
            "playback authority attempted to claim biological hearing".to_string(),
        ));
    }

    let (disposition, canonical_prefix_chars) = match (disposition, evidence, canonical_chars) {
        (
            LiveAssistantPlaybackTerminalDisposition::PlaybackComplete,
            LiveAssistantPlaybackEvidence::PlaybackComplete,
            Some(chars),
        ) => (
            LiveAssistantPlaybackTruncationDisposition::PlaybackComplete,
            Some(*chars),
        ),
        (
            LiveAssistantPlaybackTerminalDisposition::TruncateToReportedPrefix,
            LiveAssistantPlaybackEvidence::ReportedPrefix(prefix),
            Some(chars),
        ) if *chars == prefix.chars().count() as u64 => (
            LiveAssistantPlaybackTruncationDisposition::CommittedReportedPrefix,
            Some(*chars),
        ),
        (
            LiveAssistantPlaybackTerminalDisposition::Unmeasured,
            LiveAssistantPlaybackEvidence::Unmeasured,
            None,
        ) => (LiveAssistantPlaybackTruncationDisposition::Unmeasured, None),
        _ => {
            return Err(LiveAssistantPlaybackTruncationError::Transition(
                "terminal effect and playback evidence had inconsistent shape".to_string(),
            ));
        }
    };

    Ok(LiveAssistantPlaybackTruncationEvidence {
        session_id,
        channel_id,
        interaction_id,
        response_id: response_id.to_string(),
        item_id: item_id.to_string(),
        content_index,
        disposition,
        canonical_prefix_chars,
    })
}

/// Failure while SessionDocument authority seals canonical live user input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FinalLiveUserTranscriptCommitError {
    #[error("session-document generated-authority bridge token was rejected")]
    InvalidAuthorityToken,
    #[error("session-document live transcript transition was rejected: {0}")]
    Transition(String),
    #[error("session-document live transcript transition emitted no exact terminal effect")]
    MissingTerminalEffect,
}

/// Failure while SessionDocument authority classifies a live playback prefix.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LiveAssistantPlaybackTruncationError {
    #[error("session-document generated-authority bridge token was rejected")]
    InvalidAuthorityToken,
    #[error("session-document live playback transition was rejected: {0}")]
    Transition(String),
    #[error("session-document live playback transition emitted no exact terminal effect")]
    MissingTerminalEffect,
}

/// Digest of the exact ordered canonical transcript prefix mirrored to live.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CanonicalTranscriptPrefixDigest(String);

impl CanonicalTranscriptPrefixDigest {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn derive(canonical_rows: &[impl AsRef<[u8]>]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(LIVE_CONTEXT_PREFIX_DOMAIN);
        for row in canonical_rows {
            let row = row.as_ref();
            hasher.update((row.len() as u64).to_be_bytes());
            hasher.update(row);
        }
        Self(format!("sha256:{:x}", hasher.finalize()))
    }
}

/// Exact canonical context prefix position for a live channel.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LiveContextCursor {
    transcript_revision: String,
    canonical_row_count: u64,
    prefix_digest: CanonicalTranscriptPrefixDigest,
}

impl LiveContextCursor {
    pub fn derive(
        transcript_revision: impl Into<String>,
        canonical_rows: &[impl AsRef<[u8]>],
    ) -> Result<Self, LiveExecutionIdentityError> {
        Ok(Self {
            transcript_revision: require_nonempty(transcript_revision, "transcript_revision")?,
            canonical_row_count: canonical_rows.len() as u64,
            prefix_digest: CanonicalTranscriptPrefixDigest::derive(canonical_rows),
        })
    }

    #[must_use]
    pub fn transcript_revision(&self) -> &str {
        &self.transcript_revision
    }

    #[must_use]
    pub const fn canonical_row_count(&self) -> u64 {
        self.canonical_row_count
    }

    #[must_use]
    pub fn prefix_digest(&self) -> &CanonicalTranscriptPrefixDigest {
        &self.prefix_digest
    }
}

/// Provider evidence for one context or result append attempt.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveAppendDeliveryOutcome {
    Acknowledged,
    Rejected,
    Ambiguous,
}

/// Evidence that the exact append attempt may have reached the provider and
/// therefore must not be retried blindly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AmbiguousDeliveryNoRetryEvidence {
    channel_id: LiveChannelId,
    cursor: LiveContextCursor,
}

impl AmbiguousDeliveryNoRetryEvidence {
    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub fn cursor(&self) -> &LiveContextCursor {
        &self.cursor
    }

    /// Ambiguity means this exact append may already have committed remotely.
    #[must_use]
    pub const fn permits_same_append_retry(&self) -> bool {
        false
    }
}

/// Typed receipt for a provider context/result append attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveAppendDeliveryReceipt {
    channel_id: LiveChannelId,
    cursor: LiveContextCursor,
    outcome: LiveAppendDeliveryOutcome,
}

impl LiveAppendDeliveryReceipt {
    #[must_use]
    pub fn new(
        channel_id: LiveChannelId,
        cursor: LiveContextCursor,
        outcome: LiveAppendDeliveryOutcome,
    ) -> Self {
        Self {
            channel_id,
            cursor,
            outcome,
        }
    }

    #[must_use]
    pub const fn outcome(&self) -> LiveAppendDeliveryOutcome {
        self.outcome
    }

    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub fn cursor(&self) -> &LiveContextCursor {
        &self.cursor
    }

    #[must_use]
    pub fn ambiguous_no_retry_evidence(&self) -> Option<AmbiguousDeliveryNoRetryEvidence> {
        matches!(self.outcome, LiveAppendDeliveryOutcome::Ambiguous).then(|| {
            AmbiguousDeliveryNoRetryEvidence {
                channel_id: self.channel_id.clone(),
                cursor: self.cursor.clone(),
            }
        })
    }
}

/// Placement of an admitted executor result in the live provider conversation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveResultDisposition {
    OpenTurn,
    DeferredContext,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interaction(seed: u128) -> InteractionId {
        InteractionId(uuid::Uuid::from_u128(seed))
    }

    fn correlation(channel: &str, turn: &str) -> LiveUserTurnCorrelation {
        LiveUserTurnCorrelation::new(
            LiveChannelId::new(channel),
            interaction(7),
            OpaqueProviderCorrelation::new("delegation-1", turn).expect("provider correlation"),
        )
        .expect("live correlation")
    }

    #[test]
    fn live_turn_correlation_rejects_replaced_channel_fence() {
        let correlation = correlation("channel-a", "turn-1");
        assert!(correlation.is_fenced_to(&LiveChannelId::new("channel-a")));
        assert!(!correlation.is_fenced_to(&LiveChannelId::new("channel-b")));
    }

    #[test]
    fn context_cursor_binds_revision_count_order_and_exact_prefix_bytes() {
        let rows = [b"row-a".as_slice(), b"row-b".as_slice()];
        let cursor = LiveContextCursor::derive("revision-1", &rows).expect("cursor");

        assert_eq!(cursor.transcript_revision(), "revision-1");
        assert_eq!(cursor.canonical_row_count(), 2);
        assert_eq!(
            cursor,
            LiveContextCursor::derive("revision-1", &rows).expect("same cursor")
        );
        assert_ne!(
            cursor,
            LiveContextCursor::derive("revision-2", &rows).expect("other revision")
        );
        assert_ne!(
            cursor,
            LiveContextCursor::derive("revision-1", &[rows[1], rows[0]]).expect("reordered cursor")
        );
        assert_ne!(
            cursor,
            LiveContextCursor::derive("revision-1", &rows[..1]).expect("shorter cursor")
        );
    }

    #[test]
    fn ambiguous_append_mints_no_retry_evidence_for_exact_channel_and_cursor() {
        let cursor =
            LiveContextCursor::derive("revision-1", &[b"row-a".as_slice()]).expect("cursor");
        let receipt = LiveAppendDeliveryReceipt::new(
            LiveChannelId::new("channel-a"),
            cursor.clone(),
            LiveAppendDeliveryOutcome::Ambiguous,
        );

        let evidence = receipt
            .ambiguous_no_retry_evidence()
            .expect("ambiguity evidence");
        assert!(!evidence.permits_same_append_retry());
        assert_eq!(evidence.channel_id().as_str(), "channel-a");
        assert_eq!(evidence.cursor(), &cursor);

        let acknowledged = LiveAppendDeliveryReceipt::new(
            LiveChannelId::new("channel-a"),
            cursor,
            LiveAppendDeliveryOutcome::Acknowledged,
        );
        assert!(acknowledged.ambiguous_no_retry_evidence().is_none());
    }

    #[test]
    fn debug_output_redacts_provider_ids_and_executor_input() {
        let correlation = correlation("channel-a", "provider-turn-secret");
        let provisional = ProvisionalLiveHandoff::new(
            correlation,
            "executor-input-secret",
            LiveHandoffInputProvenance::NormalizedHandoff,
        )
        .expect("provisional");
        let rendered = format!("{provisional:?}");

        assert!(!rendered.contains("provider-turn-secret"));
        assert!(!rendered.contains("delegation-1"));
        assert!(!rendered.contains("executor-input-secret"));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn normalized_user_input_digest_binds_exact_normalized_bytes() {
        let provisional = ProvisionalLiveHandoff::new(
            correlation("channel-a", "turn-1"),
            "normalized input",
            LiveHandoffInputProvenance::NormalizedHandoff,
        )
        .expect("provisional");
        assert_eq!(
            provisional.normalized_input_digest(),
            NormalizedLiveUserInputDigest::derive("normalized input").expect("digest")
        );
        assert_ne!(
            provisional.normalized_input_digest(),
            NormalizedLiveUserInputDigest::derive("normalized  input").expect("other digest")
        );
    }
}
