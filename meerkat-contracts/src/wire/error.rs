//! Shared wire conversion errors.
//!
//! Conversion error surfaced when a wire variant has no core counterpart.
//! Extracted from `wire::live` so session-wire conversions
//! (`WireAssistantBlock`, `WireTranscriptSource`) do not depend on the
//! live RPC module.

/// Conversion error surfaced when a wire variant has no core counterpart.
///
/// Fires for the explicit fail-loud `Unknown` wire variants —
/// `WireLiveTransportBootstrap::Unknown`, `WireLiveAdapterObservation::Unknown`,
/// `WireLiveContinuityMode::Unknown`, `WireLiveResponseModality::Unknown`,
/// `WireLiveAdapterStatus::Unknown`, `WireLiveAdapterErrorCode::Unknown`,
/// `WireTranscriptSource::Unknown`, `WireAssistantBlock::Unknown`. These
/// are the wire mirrors' fail-loud sentinels for forward-converted future core
/// variants. There is no meaningful inverse for `Unknown` so the inverse path
/// returns this error instead of silently fabricating a placeholder.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum WireConversionError {
    /// Wire transport is the explicit-Unknown sentinel; no inverse mapping
    /// exists. Carries the original debug payload for server logs.
    #[error("unknown wire transport variant: {debug}")]
    Transport { debug: String },
    /// Wire observation is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    #[error("unknown wire observation variant: {debug}")]
    Observation { debug: String },
    /// Wire continuity mode is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    #[error("unknown wire continuity-mode variant: {debug}")]
    Continuity { debug: String },
    /// Wire response modality is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    #[error("unknown wire response-modality variant: {debug}")]
    ResponseModality { debug: String },
    /// Wire adapter status is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    #[error("unknown wire adapter-status variant: {debug}")]
    Status { debug: String },
    /// Wire adapter error-code is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    #[error("unknown wire adapter-error-code variant: {debug}")]
    ErrorCode { debug: String },
    /// Wire config-rejection reason is the explicit-Unknown sentinel; no
    /// inverse mapping exists. Carries the original debug payload for
    /// server logs.
    #[error("unknown wire config-rejection-reason variant: {debug}")]
    ConfigRejectionReason { debug: String },
    /// Wire transcript-source is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    /// R7-4 (P3 dogma): mirrors the live-wire `Unknown` pattern for
    /// `WireTranscriptSource` so future core variants are not silently
    /// misattributed as `Spoken`.
    #[error("unknown wire transcript-source variant: {debug}")]
    TranscriptSource { debug: String },
    /// Wire assistant-block is the explicit-Unknown sentinel; no inverse
    /// mapping exists. Carries the original debug payload for server logs.
    /// R7-5 (P3 dogma): the reverse direction previously fabricated an
    /// empty `AssistantBlock::Text` from `WireAssistantBlock::Unknown`,
    /// silently producing a zero-length text block on the canonical
    /// transcript. Now surfaces as a typed error.
    #[error("unknown wire assistant-block variant: {debug}")]
    AssistantBlock { debug: String },
    /// Wire provider is the explicit-Unknown sentinel; no inverse mapping
    /// exists. Carries the original debug payload for server logs.
    #[error("unknown wire provider variant: {debug}")]
    Provider { debug: String },
    /// Wire degradation-reason is the explicit-Unknown sentinel; no inverse
    /// mapping exists.
    #[error("unknown wire degradation-reason variant: {debug}")]
    DegradationReason { debug: String },
}
