//! Provider-authored request and token-accounting evidence.
//!
//! Provider adapters mint these values while lowering an exact request. Shared
//! consumers may inspect them, but must never reconstruct them from raw cache
//! counters or rendered prompt text.

use crate::{Message, Provider};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Exact provider request encoding measured after all lowering.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoweredRequestEncoding {
    AnthropicMessagesJson,
    OpenAiResponsesJson,
    OpenAiChatCompletionsJson,
    GeminiGenerateContentJson,
}

/// Identity of the fully lowered provider request body used for pressure and
/// context evidence.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct LoweredRequestProvenance {
    pub provider: Provider,
    pub encoding: LoweredRequestEncoding,
    pub body_sha256: [u8; 32],
}

impl LoweredRequestProvenance {
    pub fn from_body(
        provider: Provider,
        encoding: LoweredRequestEncoding,
        encoded_body: &[u8],
    ) -> Self {
        Self {
            provider,
            encoding,
            body_sha256: Sha256::digest(encoded_body).into(),
        }
    }
}

/// Provider-native convention used to normalize tokens presented to a model.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentedTokenConvention {
    /// Anthropic reports uncached, cache-write, and cache-read input as
    /// disjoint components. The normalized total is their saturating sum.
    AnthropicDisjointInputComponents,
    /// OpenAI prompt/input tokens already include the cached-token subset.
    OpenAiInputIncludesCachedSubset,
    /// Gemini prompt tokens already include the cached-content subset.
    GeminiPromptIncludesCachedSubset,
    /// OpenAI-compatible prompt tokens are treated as the provider's inclusive
    /// prompt total; cache detail fields are observational subsets only.
    OpenAiCompatiblePromptIncludesCacheDetails,
    /// A custom/embedded client explicitly declares its `input_tokens` field
    /// as an inclusive presented-input total. This is never inferred from
    /// cache detail fields.
    HostDeclaredInclusiveInputTotal,
}

/// How the provider adapter obtained the normalized presented-token total.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenAggregationProvenance {
    /// Sum of provider-documented disjoint input components.
    SumDisjointProviderComponents,
    /// Provider-issued inclusive prompt/input total copied without adding
    /// cache detail fields.
    ProviderInclusiveInputTotal,
}

/// One provider adapter's normalized accounting for a single model turn.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ProviderTokenAccounting {
    pub provider: Provider,
    pub model: String,
    /// Tokens presented to the model for this request. Output tokens are not
    /// included.
    pub presented_tokens: u64,
    pub convention: PresentedTokenConvention,
    pub aggregation: TokenAggregationProvenance,
}

impl ProviderTokenAccounting {
    pub fn anthropic(
        model: impl Into<String>,
        uncached_input: u64,
        cache_creation_input: u64,
        cache_read_input: u64,
    ) -> Self {
        Self {
            provider: Provider::Anthropic,
            model: model.into(),
            presented_tokens: uncached_input
                .saturating_add(cache_creation_input)
                .saturating_add(cache_read_input),
            convention: PresentedTokenConvention::AnthropicDisjointInputComponents,
            aggregation: TokenAggregationProvenance::SumDisjointProviderComponents,
        }
    }

    pub fn openai(model: impl Into<String>, input_tokens: u64) -> Self {
        Self {
            provider: Provider::OpenAI,
            model: model.into(),
            presented_tokens: input_tokens,
            convention: PresentedTokenConvention::OpenAiInputIncludesCachedSubset,
            aggregation: TokenAggregationProvenance::ProviderInclusiveInputTotal,
        }
    }

    pub fn gemini(model: impl Into<String>, prompt_tokens: u64) -> Self {
        Self {
            provider: Provider::Gemini,
            model: model.into(),
            presented_tokens: prompt_tokens,
            convention: PresentedTokenConvention::GeminiPromptIncludesCachedSubset,
            aggregation: TokenAggregationProvenance::ProviderInclusiveInputTotal,
        }
    }

    pub fn openai_compatible(model: impl Into<String>, prompt_tokens: u64) -> Self {
        Self::openai_compatible_for(Provider::SelfHosted, model, prompt_tokens)
    }

    pub fn openai_compatible_for(
        provider: Provider,
        model: impl Into<String>,
        prompt_tokens: u64,
    ) -> Self {
        Self {
            provider,
            model: model.into(),
            presented_tokens: prompt_tokens,
            convention: PresentedTokenConvention::OpenAiCompatiblePromptIncludesCacheDetails,
            aggregation: TokenAggregationProvenance::ProviderInclusiveInputTotal,
        }
    }

    pub fn host_declared(provider: Provider, model: impl Into<String>, input_tokens: u64) -> Self {
        Self {
            provider,
            model: model.into(),
            presented_tokens: input_tokens,
            convention: PresentedTokenConvention::HostDeclaredInclusiveInputTotal,
            aggregation: TokenAggregationProvenance::ProviderInclusiveInputTotal,
        }
    }
}

/// Operator marker prefix for a dimension this build declares it could not
/// measure.
///
/// This is the same vocabulary `runtime/health` publishes as
/// `unmeasured:<dimension>`; the facade's `RUNTIME_HEALTH_UNMEASURED_PREFIX` is
/// this constant, because the facade depends on core and not the reverse. An
/// unmeasured dimension is an honest absence, never a reading: nothing about
/// the underlying value is implied by it.
pub const UNMEASURED_MARKER_PREFIX: &str = "unmeasured:";

/// Operator marker prefix for a fact two owners state incompatibly.
///
/// Unlike [`UNMEASURED_MARKER_PREFIX`] a disputed fact HAS a value; what is in
/// question is who it belongs to. The value is passed through exactly as its
/// author stated it and is never rewritten to match the disputing side.
pub const DISPUTED_MARKER_PREFIX: &str = "disputed:";

/// Marker dimension: provider token accounting for one model turn.
pub const TURN_USAGE_ACCOUNTING_DIMENSION: &str = "turn_usage_accounting";

/// Marker dimension: whose model/provider one turn's accounting describes.
pub const TURN_USAGE_ACCOUNTING_IDENTITY_DIMENSION: &str = "turn_usage_accounting_identity";

/// Provider token accounting for one model turn was absent.
///
/// # What this makes untrue, and what it does not
///
/// A provider that streams a complete answer and no usage event has stated
/// nothing about tokens. The turn's SEMANTIC facts - what the model said,
/// which tools it asked for, what may be committed to the transcript - are
/// untouched by that silence, so this absence terminalizes none of them. Only
/// the accounting axis is affected, and it is affected by being left exactly
/// where it was: no counter advances, no per-call row is published, and no
/// value is substituted for the one the provider did not send.
///
/// This value states the absence and nothing more. It does not assert that the
/// turn completed: the turn's own terminal fact has its own owner, and a turn
/// whose accounting went missing can still fail afterwards on unrelated
/// grounds.
///
/// # Why the identity here is not accounting
///
/// `provider` and `model` name the REQUEST this turn was lowered for. They are
/// the address of the missing measurement, not a reconstruction of it, and
/// carry no counters. This is exactly the line
/// [`ProviderTokenAccounting::host_declared`] would cross: it would mint
/// provider attribution for numbers no provider issued.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct UnmeasuredTurnUsageAccounting {
    /// Provider of the request that produced the unaccounted turn.
    pub provider: Provider,
    /// Model of the request that produced the unaccounted turn.
    pub model: String,
}

impl UnmeasuredTurnUsageAccounting {
    /// Canonical operator marker for this dimension:
    /// `unmeasured:turn_usage_accounting`.
    ///
    /// Composed from [`UNMEASURED_MARKER_PREFIX`] and
    /// [`TURN_USAGE_ACCOUNTING_DIMENSION`]; the composition is pinned by
    /// `unmeasured_turn_usage_marker_composes_from_the_shared_vocabulary`.
    pub const MARKER: &'static str = "unmeasured:turn_usage_accounting";

    pub fn new(provider: Provider, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
        }
    }

    /// The operator-facing marker string. One owner; emit sites read it rather
    /// than spelling it.
    pub const fn marker(&self) -> &'static str {
        Self::MARKER
    }
}

impl std::fmt::Display for UnmeasuredTurnUsageAccounting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{{provider={}, model={}}}",
            Self::MARKER,
            self.provider.as_str(),
            self.model
        )
    }
}

/// One turn's provider-authored accounting named a different provider/model
/// than the request it answered.
///
/// # Why this is not the same fault as absent accounting
///
/// A mismatched identity still arrives with a complete, internally consistent
/// measurement: the [`PresentedTokenConvention`] travels with the number, so
/// the counters mean what they say regardless of which name is attached. The
/// disputed fact is attribution alone, so the token axis still advances on the
/// number the provider actually sent. Absent accounting has no number at all
/// and therefore cannot advance anything. Collapsing the two would either kill
/// correct work over a name or fabricate counters over silence.
///
/// # Why the reported identity is preserved verbatim
///
/// Rewriting `reported_*` to the active identity would publish an agreement
/// that was never observed - a guess laundered as evidence. Both sides are
/// carried so a host can see exactly who disagreed with whom.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct DisputedTurnUsageAccountingIdentity {
    /// Provider of the request this turn was lowered for.
    pub active_provider: Provider,
    /// Model of the request this turn was lowered for.
    pub active_model: String,
    /// Provider the accounting claims, exactly as the adapter minted it.
    pub reported_provider: Provider,
    /// Model the accounting claims, exactly as the adapter minted it.
    pub reported_model: String,
}

impl DisputedTurnUsageAccountingIdentity {
    /// Canonical operator marker for this dimension:
    /// `disputed:turn_usage_accounting_identity`.
    pub const MARKER: &'static str = "disputed:turn_usage_accounting_identity";

    pub const fn marker(&self) -> &'static str {
        Self::MARKER
    }
}

impl std::fmt::Display for DisputedTurnUsageAccountingIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{{active={}/{}, reported={}/{}}}",
            Self::MARKER,
            self.active_provider.as_str(),
            self.active_model,
            self.reported_provider.as_str(),
            self.reported_model
        )
    }
}

/// Stable provider-independent identity of an authored cache breakpoint.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CacheBreakpointBoundary {
    /// Stable leading system/profile prefix. `message_count` is the exclusive
    /// canonical transcript boundary.
    SystemProfilePrefix { message_count: u64 },
    /// Explicit breakpoint after one canonical transcript message.
    TranscriptAfter { message_count: u64 },
}

impl CacheBreakpointBoundary {
    pub const fn message_count(self) -> u64 {
        match self {
            Self::SystemProfilePrefix { message_count }
            | Self::TranscriptAfter { message_count } => message_count,
        }
    }
}

/// Provider cache lifetime explicitly selected for this authored breakpoint.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCacheTtl {
    FiveMinutes,
    OneHour,
    ThirtyMinutes,
    TwentyFourHours,
    ProviderDefault,
}

/// Proof that an exact provider lowering authored a cache breakpoint at one
/// canonical transcript boundary.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AuthoredCacheBreakpoint {
    provider: Provider,
    model: String,
    boundary: CacheBreakpointBoundary,
    /// Canonical lowercase `sha256:<hex>` over the canonical transcript prefix.
    canonical_prefix_sha256: String,
    /// Canonical serialized prefix byte count, retained to make accidental
    /// digest-domain mismatches fail closed.
    canonical_prefix_bytes: u64,
    /// Exact provider-native cache-prefix projection authored by the lowering
    /// that inserted this breakpoint. Canonical transcript identity maps the
    /// boundary; only this rendered identity can prove cache byte reuse.
    rendered_prefix_sha256: String,
    rendered_prefix_bytes: u64,
    /// Identity of the complete lowered request body from which the rendered
    /// prefix was projected. This prevents a prefix witness from floating
    /// free of its actual provider request.
    lowered_request_provenance: LoweredRequestProvenance,
    ttl: ProviderCacheTtl,
}

/// Failure to bind provider authoring to a canonical transcript prefix.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CacheBreakpointEvidenceError {
    #[error("cache breakpoint boundary {message_count} exceeds transcript length {message_len}")]
    BoundaryOutOfRange {
        message_count: u64,
        message_len: usize,
    },
    #[error("cache breakpoint prefix could not be canonically encoded: {detail}")]
    CanonicalEncodingFailed { detail: String },
    #[error("persisted cache-breakpoint evidence is malformed: {detail}")]
    PersistedEvidenceMalformed { detail: String },
    #[error("cache-breakpoint evidence does not match the canonical transcript prefix")]
    CanonicalPrefixMismatch,
    #[error("cache-breakpoint rendered-prefix evidence is malformed")]
    RenderedPrefixMalformed,
    #[error("cache-breakpoint provider and lowered-request encoding are incoherent")]
    ProviderEncodingMismatch,
}

/// Why one provider-authored cache breakpoint was discarded instead of
/// retained as durable session evidence.
///
/// A cache breakpoint is an optimization artifact anchored to the exact
/// transcript head a provider lowered. Ordinary transcript motion - a
/// synthetic-notice refresh, a compaction, a re-materialized prefix - moves
/// that anchor without saying anything about the transcript's own integrity.
/// Every variant here therefore describes the ARTIFACT. Faults that describe
/// the committed TRANSCRIPT stay [`CacheBreakpointEvidenceError`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum CacheBreakpointDiscardReason {
    /// The anchored boundary is past the end of the committed transcript.
    BoundaryOutsideCommittedTranscript {
        message_count: u64,
        message_len: u64,
    },
    /// The committed transcript prefix at the anchored boundary is no longer
    /// the one the provider lowered.
    CanonicalPrefixMoved,
    /// The proof itself is not usable evidence, independently of any
    /// transcript: a malformed rendered identity, an incoherent
    /// provider/encoding pairing, or an undecodable persisted row.
    EvidenceUnusable { detail: String },
    /// Durable history retains superseded prompt versions, so the projected
    /// provider boundary has no raw-to-projected map to rebind through.
    ProjectedBoundaryUnmappable,
}

impl CacheBreakpointDiscardReason {
    /// Stable observation code for logs and host metrics.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::BoundaryOutsideCommittedTranscript { .. } => {
                "boundary_outside_committed_transcript"
            }
            Self::CanonicalPrefixMoved => "canonical_prefix_moved",
            Self::EvidenceUnusable { .. } => "evidence_unusable",
            Self::ProjectedBoundaryUnmappable => "projected_boundary_unmappable",
        }
    }
}

impl std::fmt::Display for CacheBreakpointDiscardReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BoundaryOutsideCommittedTranscript {
                message_count,
                message_len,
            } => write!(
                f,
                "anchored boundary {message_count} is outside committed transcript length {message_len}"
            ),
            Self::CanonicalPrefixMoved => {
                f.write_str("committed transcript prefix moved under the anchored boundary")
            }
            Self::EvidenceUnusable { detail } => {
                write!(f, "cache-breakpoint proof is unusable: {detail}")
            }
            Self::ProjectedBoundaryUnmappable => {
                f.write_str("durable prompt-version history has no raw-to-projected boundary map")
            }
        }
    }
}

/// Which side of a turn commit produced the discarded proof.
///
/// Load-bearing for hosts: `PersistedEvidence` is inherited poison from an
/// earlier turn, `AuthoredThisTurn` is this turn's own lowering disagreeing
/// with the committed transcript. The two have different root causes and are
/// otherwise indistinguishable from the outside.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheBreakpointDiscardOrigin {
    /// Freshly authored by the lowering that produced this turn's request.
    AuthoredThisTurn,
    /// Persisted by an earlier turn and re-checked before the merge.
    PersistedEvidence,
}

impl CacheBreakpointDiscardOrigin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthoredThisTurn => "authored_this_turn",
            Self::PersistedEvidence => "persisted_evidence",
        }
    }
}

/// Identity of a discarded proof, when the proof itself could be decoded.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct DiscardedCacheBreakpointIdentity {
    pub provider: Provider,
    pub model: String,
    pub boundary: CacheBreakpointBoundary,
}

/// One provider-authored cache proof that could not be retained.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct DiscardedCacheBreakpoint {
    origin: CacheBreakpointDiscardOrigin,
    /// Absent only when the persisted evidence row itself could not be
    /// decoded, so no individual proof identity exists to name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity: Option<DiscardedCacheBreakpointIdentity>,
    reason: CacheBreakpointDiscardReason,
}

impl DiscardedCacheBreakpoint {
    pub(crate) fn proof(
        origin: CacheBreakpointDiscardOrigin,
        breakpoint: &AuthoredCacheBreakpoint,
        reason: CacheBreakpointDiscardReason,
    ) -> Self {
        Self {
            origin,
            identity: Some(DiscardedCacheBreakpointIdentity {
                provider: breakpoint.provider(),
                model: breakpoint.model().to_string(),
                boundary: breakpoint.boundary(),
            }),
            reason,
        }
    }

    pub(crate) const fn persisted_row(reason: CacheBreakpointDiscardReason) -> Self {
        Self {
            origin: CacheBreakpointDiscardOrigin::PersistedEvidence,
            identity: None,
            reason,
        }
    }

    pub const fn origin(&self) -> CacheBreakpointDiscardOrigin {
        self.origin
    }

    pub const fn identity(&self) -> Option<&DiscardedCacheBreakpointIdentity> {
        self.identity.as_ref()
    }

    pub const fn reason(&self) -> &CacheBreakpointDiscardReason {
        &self.reason
    }
}

impl std::fmt::Display for DiscardedCacheBreakpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.identity {
            Some(identity) => write!(
                f,
                "provider={} model={} boundary={} ({}): {}",
                identity.provider.as_str(),
                identity.model,
                identity.boundary.message_count(),
                self.origin.as_str(),
                self.reason
            ),
            None => write!(
                f,
                "persisted cache-breakpoint evidence row ({}): {}",
                self.origin.as_str(),
                self.reason
            ),
        }
    }
}

/// Outcome of merging this turn's provider-authored proofs into durable
/// session evidence.
///
/// A degraded retention means the turn proceeds with less caching than the
/// provider offered. It never means the turn failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthoredCacheBreakpointRetention {
    retained: usize,
    discarded: Vec<DiscardedCacheBreakpoint>,
}

impl AuthoredCacheBreakpointRetention {
    /// Number of proofs that still bind and remain durable evidence.
    pub const fn retained(&self) -> usize {
        self.retained
    }

    pub fn discarded(&self) -> &[DiscardedCacheBreakpoint] {
        &self.discarded
    }

    /// Take the discards for routing onto an observable boundary event.
    pub fn into_discarded(self) -> Vec<DiscardedCacheBreakpoint> {
        self.discarded
    }

    /// True when at least one proof was dropped, so caching for this turn is
    /// weaker than the provider authored.
    pub fn is_degraded(&self) -> bool {
        !self.discarded.is_empty()
    }

    pub(crate) fn set_retained(&mut self, retained: usize) {
        self.retained = retained;
    }

    pub(crate) fn push_discard(&mut self, discard: DiscardedCacheBreakpoint) {
        self.discarded.push(discard);
    }
}

/// Classify one binding failure as recoverable artifact motion or an
/// unrecoverable statement about the committed transcript.
///
/// The propagating variants describe the transcript we are binding AGAINST:
/// they would fail identically for every boundary and for a transcript that
/// carries no cache evidence at all. The dropping variants describe the proof
/// anchored to a head the transcript has since left.
pub(crate) fn classify_cache_breakpoint_binding_failure(
    error: CacheBreakpointEvidenceError,
) -> Result<CacheBreakpointDiscardReason, CacheBreakpointEvidenceError> {
    match error {
        CacheBreakpointEvidenceError::BoundaryOutOfRange {
            message_count,
            message_len,
        } => Ok(
            CacheBreakpointDiscardReason::BoundaryOutsideCommittedTranscript {
                message_count,
                message_len: message_len as u64,
            },
        ),
        CacheBreakpointEvidenceError::CanonicalPrefixMismatch => {
            Ok(CacheBreakpointDiscardReason::CanonicalPrefixMoved)
        }
        error @ (CacheBreakpointEvidenceError::RenderedPrefixMalformed
        | CacheBreakpointEvidenceError::ProviderEncodingMismatch) => {
            Ok(CacheBreakpointDiscardReason::EvidenceUnusable {
                detail: error.to_string(),
            })
        }
        // `CanonicalEncodingFailed` says the committed transcript cannot be
        // canonically encoded at all. `PersistedEvidenceMalformed` never
        // reaches this classifier from a bind: its decode and serialize call
        // sites own their own handling, and a serialize failure is our own
        // write failing, not a stale artifact.
        error @ (CacheBreakpointEvidenceError::CanonicalEncodingFailed { .. }
        | CacheBreakpointEvidenceError::PersistedEvidenceMalformed { .. }) => Err(error),
    }
}

impl AuthoredCacheBreakpoint {
    pub(crate) fn from_provider_claim(claim: ProviderCacheBreakpointClaim) -> Self {
        claim.evidence
    }

    pub const fn provider(&self) -> Provider {
        self.provider
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub const fn boundary(&self) -> CacheBreakpointBoundary {
        self.boundary
    }

    pub fn canonical_prefix_sha256(&self) -> &str {
        &self.canonical_prefix_sha256
    }

    pub const fn canonical_prefix_bytes(&self) -> u64 {
        self.canonical_prefix_bytes
    }

    pub fn rendered_prefix_sha256(&self) -> &str {
        &self.rendered_prefix_sha256
    }

    pub const fn rendered_prefix_bytes(&self) -> u64 {
        self.rendered_prefix_bytes
    }

    pub const fn lowered_request_provenance(&self) -> LoweredRequestProvenance {
        self.lowered_request_provenance
    }

    pub const fn ttl(&self) -> ProviderCacheTtl {
        self.ttl
    }

    /// Validate the provider-rendered identity independently of canonical
    /// boundary validation. This does not prove target compatibility; a fork
    /// still needs a fresh target lowering with the same rendered identity.
    pub fn validate_rendered_identity(&self) -> Result<(), CacheBreakpointEvidenceError> {
        let hash = self.rendered_prefix_sha256.as_bytes();
        let valid_hash = hash.len() == 71
            && hash.starts_with(b"sha256:")
            && hash[7..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
        if !valid_hash || self.rendered_prefix_bytes == 0 {
            return Err(CacheBreakpointEvidenceError::RenderedPrefixMalformed);
        }
        let coherent_encoding = matches!(
            (self.provider, self.lowered_request_provenance.encoding),
            (
                Provider::Anthropic,
                LoweredRequestEncoding::AnthropicMessagesJson
            ) | (
                Provider::Gemini,
                LoweredRequestEncoding::GeminiGenerateContentJson
            ) | (
                Provider::OpenAI | Provider::SelfHosted,
                LoweredRequestEncoding::OpenAiResponsesJson
                    | LoweredRequestEncoding::OpenAiChatCompletionsJson
            )
        ) && self.lowered_request_provenance.provider == self.provider;
        if !coherent_encoding {
            return Err(CacheBreakpointEvidenceError::ProviderEncodingMismatch);
        }
        Ok(())
    }
}

/// Non-authoritative output of one provider adapter lowering.
///
/// A claim is freely cloneable for transport from the provider crate to core,
/// but it is not durable evidence and cannot be installed in a session. Core
/// promotes it only at the successful provider-turn commit boundary, or while
/// lending a one-shot target-lowering issuer.
#[derive(Debug, Clone)]
pub struct ProviderCacheBreakpointClaim {
    evidence: AuthoredCacheBreakpoint,
}

impl ProviderCacheBreakpointClaim {
    pub const fn provider(&self) -> Provider {
        self.evidence.provider()
    }

    pub fn model(&self) -> &str {
        self.evidence.model()
    }
}

/// Revalidated source-session cache evidence authorized for one fork proof.
///
/// This capability is neither cloneable nor serializable. Raw deserialization
/// of [`AuthoredCacheBreakpoint`] remains a data-loading operation and cannot
/// satisfy the source side of [`crate::ForkPoint::prove`].
#[derive(Debug)]
pub struct ValidatedSourceCacheBreakpoint {
    evidence: AuthoredCacheBreakpoint,
}

impl ValidatedSourceCacheBreakpoint {
    pub(crate) fn new(evidence: AuthoredCacheBreakpoint) -> Self {
        Self { evidence }
    }

    pub const fn provider(&self) -> Provider {
        self.evidence.provider()
    }

    pub fn model(&self) -> &str {
        self.evidence.model()
    }

    pub const fn boundary(&self) -> CacheBreakpointBoundary {
        self.evidence.boundary()
    }

    pub(crate) fn into_authored_evidence(self) -> AuthoredCacheBreakpoint {
        self.evidence
    }
}

/// Ephemeral proof that an active provider adapter freshly lowered the target
/// request and authored this exact cache prefix.
///
/// Unlike [`AuthoredCacheBreakpoint`], this capability is deliberately not
/// cloneable, serializable, or deserializable. Persisted source evidence
/// therefore cannot be replayed as target proof. It can only be minted while
/// core lends an unconstructable [`TargetCacheLoweringIssuer`] to the active
/// adapter lowering path.
#[derive(Debug)]
pub struct TargetCacheLoweringCapability {
    evidence: AuthoredCacheBreakpoint,
}

impl TargetCacheLoweringCapability {
    pub const fn provider(&self) -> Provider {
        self.evidence.provider()
    }

    pub fn model(&self) -> &str {
        self.evidence.model()
    }

    pub const fn boundary(&self) -> CacheBreakpointBoundary {
        self.evidence.boundary()
    }

    pub fn rendered_prefix_sha256(&self) -> &str {
        self.evidence.rendered_prefix_sha256()
    }

    pub const fn rendered_prefix_bytes(&self) -> u64 {
        self.evidence.rendered_prefix_bytes()
    }

    pub const fn lowered_request_provenance(&self) -> LoweredRequestProvenance {
        self.evidence.lowered_request_provenance()
    }

    pub const fn ttl(&self) -> ProviderCacheTtl {
        self.evidence.ttl()
    }

    pub(crate) fn into_authored_evidence(self) -> AuthoredCacheBreakpoint {
        self.evidence
    }
}

/// Core-issued authority lent only for one active adapter target lowering.
///
/// There is no public constructor. Custom [`crate::agent::AgentLlmClient`]
/// implementations receive this value only when core explicitly requests a
/// fresh target lowering, making their call to [`Self::mint`] a trusted
/// backend assertion rather than a generic public constructor.
#[derive(Debug)]
pub struct TargetCacheLoweringIssuer {
    _private: (),
}

impl TargetCacheLoweringIssuer {
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }

    pub fn mint(
        &self,
        claim: ProviderCacheBreakpointClaim,
    ) -> Result<TargetCacheLoweringCapability, CacheBreakpointEvidenceError> {
        let evidence = AuthoredCacheBreakpoint::from_provider_claim(claim);
        evidence.validate_rendered_identity()?;
        Ok(TargetCacheLoweringCapability { evidence })
    }
}

/// Compute the canonical prefix identity shared by provider authoring and
/// durable fork validation.
pub fn canonical_cache_prefix_identity(
    messages: &[Message],
    message_count: u64,
) -> Result<(String, u64), CacheBreakpointEvidenceError> {
    let boundary = usize::try_from(message_count).map_err(|_| {
        CacheBreakpointEvidenceError::BoundaryOutOfRange {
            message_count,
            message_len: messages.len(),
        }
    })?;
    let prefix =
        messages
            .get(..boundary)
            .ok_or(CacheBreakpointEvidenceError::BoundaryOutOfRange {
                message_count,
                message_len: messages.len(),
            })?;
    crate::session::canonical_transcript_prefix_identity(prefix).map_err(|error| {
        CacheBreakpointEvidenceError::CanonicalEncodingFailed {
            detail: error.to_string(),
        }
    })
}

/// Exact adapter-lowering inputs needed to claim one cache breakpoint.
pub struct ProviderCacheBreakpointClaimRequest<'a> {
    pub provider: Provider,
    pub model: &'a str,
    pub messages: &'a [Message],
    pub boundary: CacheBreakpointBoundary,
    pub ttl: ProviderCacheTtl,
    pub rendered_prefix: &'a [u8],
    pub lowered_request_encoding: LoweredRequestEncoding,
    pub lowered_request_body: &'a [u8],
}

/// Build a non-authoritative provider claim that the renderer inserted the
/// matching breakpoint into this exact lowered request.
///
/// Only core may promote this claim into durable source evidence or an
/// ephemeral target-lowering capability. Arbitrary callers therefore cannot
/// mint provider-authored session authority through this generic renderer.
pub fn provider_cache_breakpoint_claim(
    request: ProviderCacheBreakpointClaimRequest<'_>,
) -> Result<ProviderCacheBreakpointClaim, CacheBreakpointEvidenceError> {
    let (canonical_prefix_sha256, canonical_prefix_bytes) =
        canonical_cache_prefix_identity(request.messages, request.boundary.message_count())?;
    let rendered_prefix_sha256 = format!("sha256:{:x}", Sha256::digest(request.rendered_prefix));
    let rendered_prefix_bytes = u64::try_from(request.rendered_prefix.len()).unwrap_or(u64::MAX);
    let lowered_request_provenance = LoweredRequestProvenance::from_body(
        request.provider,
        request.lowered_request_encoding,
        request.lowered_request_body,
    );
    let evidence = AuthoredCacheBreakpoint {
        provider: request.provider,
        model: request.model.to_string(),
        boundary: request.boundary,
        canonical_prefix_sha256,
        canonical_prefix_bytes,
        rendered_prefix_sha256,
        rendered_prefix_bytes,
        lowered_request_provenance,
        ttl: request.ttl,
    };
    evidence.validate_rendered_identity()?;
    Ok(ProviderCacheBreakpointClaim { evidence })
}

#[cfg(test)]
mod degradation_marker_tests {
    use super::{
        DISPUTED_MARKER_PREFIX, DisputedTurnUsageAccountingIdentity,
        TURN_USAGE_ACCOUNTING_DIMENSION, TURN_USAGE_ACCOUNTING_IDENTITY_DIMENSION,
        UNMEASURED_MARKER_PREFIX, UnmeasuredTurnUsageAccounting,
    };
    use crate::Provider;

    /// The marker constant is one owner's spelling of a composed vocabulary.
    /// Pinning the composition keeps a hand-edited literal from drifting away
    /// from the prefix `runtime/health` publishes.
    #[test]
    fn unmeasured_turn_usage_marker_composes_from_the_shared_vocabulary() {
        assert_eq!(
            UnmeasuredTurnUsageAccounting::MARKER,
            format!("{UNMEASURED_MARKER_PREFIX}{TURN_USAGE_ACCOUNTING_DIMENSION}")
        );
        assert_eq!(
            DisputedTurnUsageAccountingIdentity::MARKER,
            format!("{DISPUTED_MARKER_PREFIX}{TURN_USAGE_ACCOUNTING_IDENTITY_DIMENSION}")
        );
    }

    /// The operator rendering must name the address of the missing
    /// measurement, because "some turn was unaccounted" is not actionable.
    #[test]
    fn unmeasured_turn_usage_renders_the_request_identity() {
        let unmeasured = UnmeasuredTurnUsageAccounting::new(Provider::Anthropic, "claude-opus-5");
        assert_eq!(
            unmeasured.to_string(),
            "unmeasured:turn_usage_accounting{provider=anthropic, model=claude-opus-5}"
        );
        assert_eq!(unmeasured.marker(), UnmeasuredTurnUsageAccounting::MARKER);
    }

    /// A dispute names BOTH sides. Rendering only one of them would be the
    /// same laundering the type exists to prevent.
    #[test]
    fn disputed_identity_renders_both_sides() {
        // Synthetic model ids on purpose. meerkat-core carries ZERO
        // provider-specific model data, tests included (xtask's
        // no_provider_data_in_core gate bans `"gpt-N`/`"claude-N`/`"gemini-N`
        // literals anywhere under meerkat-core/src), and naming the two sides
        // for what they are reads better here than a real catalog id would.
        let dispute = DisputedTurnUsageAccountingIdentity {
            active_provider: Provider::OpenAI,
            active_model: "active-model".to_string(),
            reported_provider: Provider::OpenAI,
            reported_model: "reported-model".to_string(),
        };
        assert_eq!(
            dispute.to_string(),
            "disputed:turn_usage_accounting_identity{active=openai/active-model, \
             reported=openai/reported-model}"
        );
    }
}
