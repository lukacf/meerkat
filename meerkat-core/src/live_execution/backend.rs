//! Provider-neutral backend attribution facts, never execution permission.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::request::{LiveDelegationAttribution, LiveProviderReference};

/// The tracker's response association. `None` says no associated delegation
/// is known; it does not erase absent versus null in the original envelope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LiveBackendResponseKey {
    pub response: LiveProviderReference,
    pub delegation: Option<LiveProviderReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveBackendOwnership {
    Owned { response: LiveBackendResponseKey },
    Unowned {},
    Ambiguous { candidates: LiveBackendCandidates },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(try_from = "Vec<LiveBackendResponseKey>")]
pub struct LiveBackendCandidates(Vec<LiveBackendResponseKey>);

impl LiveBackendCandidates {
    pub fn new(candidates: Vec<LiveBackendResponseKey>) -> Result<Self, LiveBackendValueError> {
        if !(2..=32).contains(&candidates.len()) {
            return Err(LiveBackendValueError::InvalidCandidateCount);
        }
        let distinct: HashSet<_> = candidates.iter().collect();
        if distinct.len() != candidates.len() {
            return Err(LiveBackendValueError::DuplicateCandidate);
        }
        Ok(Self(candidates))
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &LiveBackendResponseKey> {
        self.0.iter()
    }
}

impl TryFrom<Vec<LiveBackendResponseKey>> for LiveBackendCandidates {
    type Error = LiveBackendValueError;

    fn try_from(value: Vec<LiveBackendResponseKey>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveBackendScope {
    pub envelope_attribution: LiveDelegationAttribution,
    pub ownership: LiveBackendOwnership,
}

/// Identity of the complete ordered call set, including scope, IDs, names and
/// exact argument bytes. This comparison value is not batch-readiness proof.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveFunctionBatchDigest([u8; 32]);

impl LiveFunctionBatchDigest {
    pub fn of<'a>(
        response: &LiveBackendResponseKey,
        calls: impl Iterator<Item = LiveCompletedFunctionView<'a>>,
    ) -> Result<Self, LiveBackendValueError> {
        if response.response.as_str().len() > 128
            || response
                .delegation
                .as_ref()
                .is_some_and(|id| id.as_str().len() > 128)
        {
            return Err(LiveBackendValueError::InvalidResponseIdentity);
        }
        let mut hash = Sha256::new();
        hash.update(b"meerkat.live-function-batch.v1\0");
        let part = |hash: &mut Sha256, text: &str| {
            hash.update((text.len() as u64).to_be_bytes());
            hash.update(text.as_bytes());
        };
        part(&mut hash, response.response.as_str());
        match &response.delegation {
            Some(delegation) => {
                hash.update([1]);
                part(&mut hash, delegation.as_str());
            }
            None => hash.update([0]),
        }
        let mut seen = HashSet::new();
        let mut count = 0_u64;
        for call in calls {
            if count == 128 {
                return Err(LiveBackendValueError::InvalidCallCount);
            }
            if call.call_id.is_empty() || call.call_id.len() > 128 || !seen.insert(call.call_id) {
                return Err(LiveBackendValueError::InvalidCallIdentity);
            }
            for text in [call.call_id, call.name, call.arguments] {
                part(&mut hash, text);
            }
            count += 1;
        }
        hash.update(count.to_be_bytes());
        Ok(Self(hash.finalize().into()))
    }
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Borrowed completed function content. Arguments remain exact provider text,
/// including malformed JSON; the function decoder owns the bounded refusal.
/// Neither this view nor a complete batch is an execution grant.
#[derive(Clone, Copy, Serialize)]
pub struct LiveCompletedFunctionView<'a> {
    pub call_id: &'a str,
    pub name: &'a str,
    pub arguments: &'a str,
}

impl std::fmt::Debug for LiveCompletedFunctionView<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LiveCompletedFunctionView")
            .field("call_id", &"[REDACTED]")
            .field("name", &"[REDACTED]")
            .field("argument_bytes", &self.arguments.len())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveBackendValueError {
    #[error("completed batch response identity exceeds its byte bound")]
    InvalidResponseIdentity,
    #[error("a completed function batch exceeds 128 calls")]
    InvalidCallCount,
    #[error("a completed function batch has an invalid or repeated call identity")]
    InvalidCallIdentity,
    #[error("ambiguous live backend ownership requires 2-32 distinct candidates")]
    InvalidCandidateCount,
    #[error("live backend ambiguity repeats a response candidate")]
    DuplicateCandidate,
    #[error("safe Live diagnostic exceeds its 1024-byte encoded bound")]
    DiagnosticTooLarge,
    #[error("safe Live diagnostic cannot be encoded")]
    DiagnosticEncoding,
}

pub const LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES: usize = 1024;

/// A safe, nonterminal diagnostic carries no arbitrary provider message, raw
/// snapshot, error body, tool argument, or instruction string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(extend("x-max-encoded-bytes" = LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES)))]
#[serde(try_from = "LiveProviderDiagnosticParts")]
pub struct LiveProviderDiagnostic {
    category: LiveProviderDiagnosticCategory,
    attribution: LiveBackendOwnership,
    occurrences: std::num::NonZeroU64,
}

impl LiveProviderDiagnostic {
    pub fn new(
        category: LiveProviderDiagnosticCategory,
        attribution: LiveBackendOwnership,
        occurrences: std::num::NonZeroU64,
    ) -> Result<Self, LiveBackendValueError> {
        let validate = |response: &LiveBackendResponseKey| {
            if response.response.as_str().len() > 128
                || response
                    .delegation
                    .as_ref()
                    .is_some_and(|id| id.as_str().len() > 128)
            {
                Err(LiveBackendValueError::InvalidResponseIdentity)
            } else {
                Ok(())
            }
        };
        match &attribution {
            LiveBackendOwnership::Owned { response } => validate(response)?,
            LiveBackendOwnership::Ambiguous { candidates } => {
                for candidate in candidates.iter() {
                    validate(candidate)?;
                }
            }
            LiveBackendOwnership::Unowned {} => {}
        }
        let record = Self {
            category,
            attribution,
            occurrences,
        };
        let mut counter = DiagnosticByteCounter {
            bytes: 0,
            exceeded: false,
        };
        if serde_json::to_writer(&mut counter, &record).is_err() {
            return Err(if counter.exceeded {
                LiveBackendValueError::DiagnosticTooLarge
            } else {
                LiveBackendValueError::DiagnosticEncoding
            });
        }
        Ok(record)
    }

    pub const fn category(&self) -> LiveProviderDiagnosticCategory {
        self.category
    }

    pub fn attribution(&self) -> &LiveBackendOwnership {
        &self.attribution
    }

    pub const fn occurrences(&self) -> std::num::NonZeroU64 {
        self.occurrences
    }
}

struct DiagnosticByteCounter {
    bytes: usize,
    exceeded: bool,
}

impl std::io::Write for DiagnosticByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > LIVE_PUBLIC_DIAGNOSTIC_MAX_BYTES - self.bytes {
            self.exceeded = true;
            return Err(std::io::Error::other("Live diagnostic byte limit"));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[derive(Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
struct LiveProviderDiagnosticParts {
    pub category: LiveProviderDiagnosticCategory,
    pub attribution: LiveBackendOwnership,
    pub occurrences: std::num::NonZeroU64,
}

impl TryFrom<LiveProviderDiagnosticParts> for LiveProviderDiagnostic {
    type Error = LiveBackendValueError;

    fn try_from(parts: LiveProviderDiagnosticParts) -> Result<Self, Self::Error> {
        Self::new(parts.category, parts.attribution, parts.occurrences)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum LiveProviderDiagnosticCategory {
    BackendAdvisoryError,
    ProtocolInconsistency,
    AccountingUnmeasured,
    AccountingDisputed,
    UnsupportedProviderEvent,
    UncorrelatedContextAcknowledgment,
}
