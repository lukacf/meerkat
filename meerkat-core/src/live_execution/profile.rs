//! Provider-neutral declarations for named Live execution profiles.
//!
//! A configured profile describes a channel; it does not activate a grant or
//! replace the durable executor's identity. Defaults and provider-specific
//! validation belong to the composing facade/provider.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};

use super::LiveExecutionMode;
use crate::{Provider, SessionLlmIdentity};

/// Declarative profiles only. Trusted activation lives in a separate host
/// source and cannot be written through ordinary config mutation.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LiveProfilesConfig {
    pub profiles: BTreeMap<LiveProfileId, LiveProfileEntry>,
}

impl LiveProfilesConfig {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct LiveProfileId(Arc<str>);

impl LiveProfileId {
    pub fn parse(value: impl Into<String>) -> Result<Self, LiveProfileDeclarationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(LiveProfileDeclarationError::InvalidProfileId);
        }
        Ok(Self(Arc::from(value)))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for LiveProfileId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("LiveProfileId")
            .field(&self.0)
            .finish()
    }
}

impl<'de> Deserialize<'de> for LiveProfileId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Absence of an entry inherits its owner profile; Disabled explicitly blocks
/// that inherited entry. Neither state grants permission to execute work.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "definition",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum LiveProfileEntry {
    Disabled,
    Configured(Box<LiveProfileDefinition>),
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveProfileDefinition {
    #[serde(deserialize_with = "deserialize_voice_identity")]
    #[cfg_attr(feature = "schema", schemars(with = "StrictLiveVoiceIdentity"))]
    pub voice_identity: SessionLlmIdentity,
    pub execution: LiveProfileExecution,
    /// Provider-owned voice selector; omission leaves selection to the
    /// provider declaration, not to the surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    /// Trusted voice guidance, never copied from executor System/tool state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub context_projection: LiveContextProjectionPolicy,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(rename = "LiveProfileVoiceIdentity"))]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLiveVoiceIdentity {
    model: String,
    provider: Provider,
    #[serde(default)]
    self_hosted_server_id: Option<String>,
    #[serde(default)]
    provider_params: Option<crate::lifecycle::run_primitive::ProviderParamsOverride>,
    #[serde(default)]
    auth_binding: Option<StrictLiveAuthBinding>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(rename = "LiveProfileAuthBinding"))]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StrictLiveAuthBinding {
    realm: crate::connection::RealmId,
    binding: crate::connection::BindingId,
    #[serde(default)]
    profile: Option<crate::connection::ProfileId>,
    #[serde(default)]
    origin: crate::connection::BindingOrigin,
}

impl From<StrictLiveAuthBinding> for crate::AuthBindingRef {
    fn from(value: StrictLiveAuthBinding) -> Self {
        Self {
            realm: value.realm,
            binding: value.binding,
            profile: value.profile,
            origin: value.origin,
        }
    }
}

fn deserialize_voice_identity<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<SessionLlmIdentity, D::Error> {
    let value = StrictLiveVoiceIdentity::deserialize(deserializer)?;
    Ok(SessionLlmIdentity {
        model: value.model,
        provider: value.provider,
        self_hosted_server_id: value.self_hosted_server_id,
        provider_params: value.provider_params,
        auth_binding: value.auth_binding.map(Into::into),
    })
}

impl fmt::Debug for LiveProfileDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LiveProfileDefinition")
            .field("voice_identity", &"[REDACTED]")
            .field("execution", &self.execution)
            .field("voice", &self.voice.as_ref().map(|_| "[REDACTED]"))
            .field(
                "instructions",
                &self.instructions.as_ref().map(|_| "[REDACTED]"),
            )
            .field("context_projection", &self.context_projection)
            .finish()
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveProfileExecution {
    ClientContext {
        request_policy: LiveClientRequestPolicy,
    },
    FunctionBridge {
        backend: LiveManagedBackendModel,
    },
}

impl LiveProfileExecution {
    #[must_use]
    pub const fn mode(&self) -> LiveExecutionMode {
        match self {
            Self::ClientContext { .. } => LiveExecutionMode::ClientContext,
            Self::FunctionBridge { .. } => LiveExecutionMode::FunctionBridge,
        }
    }
}

/// Managed backend selection does not carry a second credential. Whether the
/// selected provider/model can share the voice connection is provider-owned.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveManagedBackendModel {
    pub provider: Provider,
    pub model: String,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveClientRequestPolicy {
    SnapshotAtDelegation,
    ExplicitApplicationRequest,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveContextProjectionPolicy {
    Reject,
    RecentAuthorizedSuffix,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LiveProfileDeclarationError {
    #[error("live profile id must contain 1-128 ASCII letters, digits, '.', '_' or '-'")]
    InvalidProfileId,
}
