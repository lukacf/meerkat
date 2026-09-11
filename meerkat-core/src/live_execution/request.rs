//! Stable public Live request-source vocabulary.
//!
//! Source identity is lookup material, not an admission receipt. Payload
//! digests, snapshot watermarks, permission and execution outcomes deliberately
//! do not participate in the key.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};

use super::{LiveChannelId, LiveExecutionIdentityError, require_nonempty};
use crate::SessionId;

/// Opaque provider equality material. It is not a Meerkat control handle.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct LiveProviderReference(Arc<str>);

impl LiveProviderReference {
    pub fn new(value: impl Into<String>) -> Result<Self, LiveExecutionIdentityError> {
        require_nonempty(value, "provider_reference").map(|value| Self(Arc::from(value)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for LiveProviderReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LiveProviderReference([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for LiveProviderReference {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// The outer scope's presence is independent of the known response identity.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveDelegationAttribution {
    Absent {},
    ExplicitNull {},
    Known { delegation: LiveProviderReference },
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveResponseIdentity {
    pub response: LiveProviderReference,
    pub attribution: LiveDelegationAttribution,
}

impl LiveResponseIdentity {
    /// Form only a structurally scoped source, never function readiness or
    /// permission. The provider tracker and generated admission still own
    /// their separate evidence and authorization barriers.
    pub fn function_source(
        &self,
        call: LiveProviderReference,
    ) -> Result<LiveSourceIdentity, LiveSourceScopeError> {
        let LiveDelegationAttribution::Known { delegation } = &self.attribution else {
            return Err(LiveSourceScopeError::MissingDelegationAttribution);
        };
        Ok(LiveSourceIdentity::FunctionCall {
            delegation: delegation.clone(),
            response: self.response.clone(),
            call,
        })
    }
}

/// Application-selected identity for an explicit snapshot resubmission.
///
/// A new UUID is a new source, not proof that its content may execute.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LiveApplicationRequestId(
    #[cfg_attr(feature = "schema", schemars(with = "String"))] uuid::Uuid,
);

impl LiveApplicationRequestId {
    #[must_use]
    pub const fn from_uuid(value: uuid::Uuid) -> Self {
        Self(value)
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveSourceIdentity {
    ClientDelegation {
        delegation: LiveProviderReference,
    },
    FunctionCall {
        delegation: LiveProviderReference,
        response: LiveProviderReference,
        call: LiveProviderReference,
    },
    ApplicationRequest {
        request_id: LiveApplicationRequestId,
    },
}

/// Exact durable lookup key, independent of when admission is observed.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "LiveSourceKeyWire")]
pub struct LiveSourceKey {
    session_id: SessionId,
    channel_id: LiveChannelId,
    source: LiveSourceIdentity,
}

impl LiveSourceKey {
    pub fn new(
        session_id: SessionId,
        channel_id: LiveChannelId,
        source: LiveSourceIdentity,
    ) -> Result<Self, LiveExecutionIdentityError> {
        if channel_id.as_str().trim().is_empty() {
            return Err(LiveExecutionIdentityError::EmptyField {
                field: "channel_id",
            });
        }
        Ok(Self {
            session_id,
            channel_id,
            source,
        })
    }

    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    #[must_use]
    pub fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }

    #[must_use]
    pub fn source(&self) -> &LiveSourceIdentity {
        &self.source
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveSourceKeyWire {
    session_id: SessionId,
    channel_id: LiveChannelId,
    source: LiveSourceIdentity,
}

impl TryFrom<LiveSourceKeyWire> for LiveSourceKey {
    type Error = LiveExecutionIdentityError;

    fn try_from(value: LiveSourceKeyWire) -> Result<Self, Self::Error> {
        Self::new(value.session_id, value.channel_id, value.source)
    }
}

/// Which content evidence a trusted grant may admit. Neither variant is
/// provider-final user speech or an effect permission by itself.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveRequestEvidenceKind {
    ApplicationSnapshot,
    StructuredFunctionRequest,
}

/// Explicit work-cancellation reasons. Voice close and speech interruption
/// deliberately do not belong to this vocabulary.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveRequestCancellationReason {
    OperatorRequested,
    GrantRevoked,
    SessionArchived,
    ExplicitSupersession,
}

/// Source-keyed cancellation content, valid before input admission is known.
///
/// This carries intent only. Generated authority binds it to an exact
/// admitted input/run and owns cancellation dispatch; there is no
/// interrupt-current fallback.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveRequestCancelIntent {
    pub source: LiveSourceKey,
    pub reason: LiveRequestCancellationReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveSourceScopeError {
    #[error("a function source requires known outer delegation attribution")]
    MissingDelegationAttribution,
}
