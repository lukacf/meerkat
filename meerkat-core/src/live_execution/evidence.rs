//! Immutable public Live request content. Evidence never grants permission.

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::request::LiveRequestEvidenceKind;
use super::request::{LiveSourceIdentity, LiveSourceKey};

pub const LIVE_REQUEST_TEXT_MAX_BYTES: usize = 16 * 1024;

/// Non-human transcript provenance. No grant, tool policy or final-speech
/// assertion can be serialized into this content descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "DelegatedRequestProvenanceParts")]
pub struct DelegatedRequestProvenance {
    request_id: crate::ops::OperationId,
    source: LiveSourceKey,
    evidence_kind: LiveRequestEvidenceKind,
    request_digest: LiveContentDigest,
}

impl DelegatedRequestProvenance {
    pub fn new(
        request_id: crate::ops::OperationId,
        source: LiveSourceKey,
        evidence_kind: LiveRequestEvidenceKind,
        request_digest: LiveContentDigest,
    ) -> Result<Self, LiveEvidenceError> {
        if !matches!(
            (source.source(), evidence_kind),
            (
                LiveSourceIdentity::ClientDelegation { .. }
                    | LiveSourceIdentity::ApplicationRequest { .. },
                LiveRequestEvidenceKind::ApplicationSnapshot
            ) | (
                LiveSourceIdentity::FunctionCall { .. },
                LiveRequestEvidenceKind::StructuredFunctionRequest
            )
        ) {
            return Err(LiveEvidenceError::SourceKindMismatch);
        }
        Ok(Self {
            request_id,
            source,
            evidence_kind,
            request_digest,
        })
    }
    pub fn source(&self) -> &LiveSourceKey {
        &self.source
    }
    pub fn request_id(&self) -> &crate::ops::OperationId {
        &self.request_id
    }
    pub const fn request_digest(&self) -> LiveContentDigest {
        self.request_digest
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegatedRequestProvenanceParts {
    request_id: crate::ops::OperationId,
    source: LiveSourceKey,
    evidence_kind: LiveRequestEvidenceKind,
    request_digest: LiveContentDigest,
}

impl TryFrom<DelegatedRequestProvenanceParts> for DelegatedRequestProvenance {
    type Error = LiveEvidenceError;
    fn try_from(value: DelegatedRequestProvenanceParts) -> Result<Self, Self::Error> {
        Self::new(
            value.request_id,
            value.source,
            value.evidence_kind,
            value.request_digest,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct LiveContentDigest([u8; 32]);

impl LiveContentDigest {
    pub fn of_request_bytes(bytes: &[u8]) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"meerkat.live-request-content.v1\0");
        digest.update(bytes);
        Self(digest.finalize().into())
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// An observation interval is `(after, through]`. Empty intervals exist for
/// runless refusals; it is not proof that the selected records are durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LiveObservationIntervalWire")]
pub struct LiveObservationInterval {
    after: u64,
    through: u64,
}

impl LiveObservationInterval {
    pub fn new(after: u64, through: u64) -> Result<Self, LiveEvidenceError> {
        if through < after {
            return Err(LiveEvidenceError::ReversedInterval);
        }
        Ok(Self { after, through })
    }

    pub const fn after(self) -> u64 {
        self.after
    }

    pub const fn through(self) -> u64 {
        self.through
    }

    pub const fn is_empty(self) -> bool {
        self.after == self.through
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveObservationIntervalWire {
    after: u64,
    through: u64,
}

impl TryFrom<LiveObservationIntervalWire> for LiveObservationInterval {
    type Error = LiveEvidenceError;

    fn try_from(value: LiveObservationIntervalWire) -> Result<Self, Self::Error> {
        Self::new(value.after, value.through)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct LiveRequestText(Box<str>);

impl LiveRequestText {
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, LiveEvidenceError> {
        let value = value.into();
        if value.len() > LIVE_REQUEST_TEXT_MAX_BYTES {
            return Err(LiveEvidenceError::RequestTooLarge);
        }
        if value.trim().is_empty() {
            return Err(LiveEvidenceError::EmptyRequest);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn digest(&self) -> LiveContentDigest {
        LiveContentDigest::of_request_bytes(self.0.as_bytes())
    }
}

impl TryFrom<String> for LiveRequestText {
    type Error = LiveEvidenceError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value.into_boxed_str())
    }
}

impl fmt::Debug for LiveRequestText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiveRequestText")
            .field("bytes", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// Neither form is finalized human speech. An application snapshot retains
/// its provisional grade after successful execution or provider delivery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveRequestEvidence {
    ApplicationSnapshot {
        observations: LiveObservationInterval,
        request: LiveRequestText,
    },
    StructuredFunctionRequest {
        request: LiveRequestText,
    },
}

impl LiveRequestEvidence {
    pub const fn kind(&self) -> LiveRequestEvidenceKind {
        match self {
            Self::ApplicationSnapshot { .. } => LiveRequestEvidenceKind::ApplicationSnapshot,
            Self::StructuredFunctionRequest { .. } => {
                LiveRequestEvidenceKind::StructuredFunctionRequest
            }
        }
    }

    pub fn request(&self) -> &LiveRequestText {
        match self {
            Self::ApplicationSnapshot { request, .. }
            | Self::StructuredFunctionRequest { request } => request,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveEvidenceError {
    #[error("live request provenance does not match its source kind")]
    SourceKindMismatch,
    #[error("live observation interval is reversed")]
    ReversedInterval,
    #[error("live request is empty or contains only whitespace")]
    EmptyRequest,
    #[error("live request exceeds its UTF-8 byte bound")]
    RequestTooLarge,
}
