//! Application-facing live channel contracts.
//!
//! Meerkat owns live execution semantics. This module only owns Meta's
//! versioned request envelope, feature negotiation atom, and SDK-facing
//! channel handle. The experimental execution identity capability is not
//! advertised until the host admission and upstream orchestration seams are
//! available.

use std::fmt;

use meerkat_contracts::{
    LiveOpenResult, WireLiveChannelCapabilities, WireLiveContinuityMode, WireLiveOpenAdvisory,
    WireLiveTransportBootstrap,
};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Capability required before a client may send `execution_identity`.
///
/// Kept as Meta's local wire atom until the minimum published Meerkat
/// dependency exports the matching constant. Experimental builds verify the
/// same value through the upstream qualified capability projection.
pub const LIVE_EXECUTION_IDENTITY_V1: &str = "live.execution_identity.v1";

/// Dormant compatibility vocabulary for provider-neutral FunctionBridge.
///
/// Production advertisement remains fail-closed until Meerkat qualifies the
/// direct raw function-call and settlement lifecycle.
pub const LIVE_EXECUTION_FUNCTION_BRIDGE_V1: &str = "live.execution.function_bridge.v1";

/// Catalog-qualified execution through provider client-context delegation.
pub const LIVE_EXECUTION_CLIENT_CONTEXT_V1: &str = "live.execution.client_context.v1";

/// Provider-neutral execution mode resolved from the selected model profile.
///
/// This vocabulary is output-only. A live-open caller selects a catalog model,
/// never a provider-native delegation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveExecutionMode {
    FunctionBridge,
    ClientContext,
}

impl LiveExecutionMode {
    #[must_use]
    pub const fn capability(self) -> &'static str {
        match self {
            Self::FunctionBridge => LIVE_EXECUTION_FUNCTION_BRIDGE_V1,
            Self::ClientContext => LIVE_EXECUTION_CLIENT_CONTEXT_V1,
        }
    }
}

/// Exact version discriminator carried inside the v1 request envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveExecutionIdentityVersion {
    V1,
}

/// A forward-compatible feature capability advertised by `meta/capabilities`.
///
/// The newtype keeps capability lists typed without rejecting future feature
/// names that an older client does not yet understand.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct FeatureCapability(String);

impl<'de> Deserialize<'de> for FeatureCapability {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.is_empty() {
            return Err(D::Error::custom("feature capability must be non-empty"));
        }
        Ok(Self(value))
    }
}

impl FeatureCapability {
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.is_empty() {
            return Err("feature capability must be non-empty");
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn live_execution_identity_v1() -> Self {
        Self(LIVE_EXECUTION_IDENTITY_V1.to_string())
    }

    #[must_use]
    pub fn live_execution_function_bridge_v1() -> Self {
        Self(LIVE_EXECUTION_FUNCTION_BRIDGE_V1.to_string())
    }

    #[must_use]
    pub fn live_execution_client_context_v1() -> Self {
        Self(LIVE_EXECUTION_CLIENT_CONTEXT_V1.to_string())
    }
}

fn deserialize_non_empty_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.trim().is_empty() {
        return Err(D::Error::custom("value must be a non-empty string"));
    }
    Ok(value)
}

/// Version 1 host-registered channel execution profile selection.
///
/// Provider, model, auth binding, provider mode, tools, and instructions stay
/// host-owned. This request changes only the opened live channel and never
/// mutates durable member or session identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveExecutionIdentityV1 {
    pub version: LiveExecutionIdentityVersion,
    /// Stable host-registered profile identity. Provider mode, tools, and
    /// approved top-level GPT Live session instructions remain behind this
    /// catalog selection and cannot be supplied as raw open parameters.
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub profile_id: String,
}

/// SDK/API handle returned by a successful live open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveChannelHandle {
    pub channel_id: String,
    pub target_identity: String,
    pub transport: WireLiveTransportBootstrap,
    pub capabilities: WireLiveChannelCapabilities,
    pub continuity: WireLiveContinuityMode,
    /// What the open went ahead without, as Meerkat's open result reported
    /// it (`staged_prompt_refused`: a run-start hook refused the session's
    /// staged create-time prompt, which stays staged for its first text
    /// turn). Advisories, not failures; omitted when there are none. The
    /// vocabulary may grow: an advisory this version does not know is kept
    /// as it was sent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub advisories: Vec<WireLiveOpenAdvisory>,
}

/// Pending experimental live channel returned before media activation.
///
/// The receipt is opaque phase custody for the exact current binding. It is
/// not reconstructed from `channel_id` and is never a provider identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingLiveChannelHandle {
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub channel_id: String,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub target_identity: String,
    pub execution_mode: LiveExecutionMode,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub pending_receipt: String,
    pub transport: WireLiveTransportBootstrap,
    pub capabilities: WireLiveChannelCapabilities,
    pub continuity: WireLiveContinuityMode,
    /// What the open went ahead without, as on [`LiveChannelHandle`];
    /// omitted when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub advisories: Vec<WireLiveOpenAdvisory>,
}

impl PendingLiveChannelHandle {
    #[must_use]
    pub fn new(
        target_identity: impl Into<String>,
        execution_mode: LiveExecutionMode,
        pending_receipt: impl Into<String>,
        result: LiveOpenResult,
    ) -> Self {
        Self {
            channel_id: result.channel_id,
            target_identity: target_identity.into(),
            execution_mode,
            pending_receipt: pending_receipt.into(),
            transport: result.transport,
            capabilities: result.capabilities,
            continuity: result.continuity,
            advisories: result.advisories,
        }
    }
}

/// Active experimental live channel minted only after generated activation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveLiveChannelHandle {
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub channel_id: String,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub target_identity: String,
    pub execution_mode: LiveExecutionMode,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub activation_receipt: String,
}

/// Generated playback-owner readiness for one exact pending binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LivePlaybackOwnerReadiness {
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub channel_id: String,
    #[serde(deserialize_with = "deserialize_non_empty_string")]
    pub readiness_receipt: String,
}

/// Strict phase projection returned by experimental live status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExperimentalLiveChannelStatus {
    Pending,
    Active { handle: ActiveLiveChannelHandle },
    Revoked,
    Closed,
}

impl LiveChannelHandle {
    #[must_use]
    pub fn from_open_result(target_identity: impl Into<String>, result: LiveOpenResult) -> Self {
        Self {
            channel_id: result.channel_id,
            target_identity: target_identity.into(),
            transport: result.transport,
            capabilities: result.capabilities,
            continuity: result.continuity,
            advisories: result.advisories,
        }
    }
}

/// Why an experimental live channel needs a fresh signaling bootstrap.
///
/// The reason remains typed because canonical-context ambiguity and
/// delegation-result ambiguity carry different recovery authority upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveReplacementReason {
    CanonicalContext,
    DelegationResult,
}

/// Strict result of the retryable `meta/live/replacement_required` read.
///
/// Construction is private so `required = false` cannot accidentally carry a
/// bootstrap and `required = true` cannot omit one. The wire remains a plain
/// object for Python and TypeScript SDK parity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LiveReplacementRequiredResult {
    required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<LiveReplacementReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replacement: Option<LiveChannelHandle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    canonical_seed_cursor: Option<u64>,
}

impl LiveReplacementRequiredResult {
    #[must_use]
    pub fn not_required() -> Self {
        Self {
            required: false,
            reason: None,
            replacement: None,
            canonical_seed_cursor: None,
        }
    }

    #[must_use]
    pub fn required(
        reason: LiveReplacementReason,
        replacement: LiveChannelHandle,
        canonical_seed_cursor: u64,
    ) -> Self {
        Self {
            required: true,
            reason: Some(reason),
            replacement: Some(replacement),
            canonical_seed_cursor: Some(canonical_seed_cursor),
        }
    }

    #[must_use]
    pub fn is_required(&self) -> bool {
        self.required
    }

    #[must_use]
    pub fn reason(&self) -> Option<LiveReplacementReason> {
        self.reason
    }

    #[must_use]
    pub fn replacement(&self) -> Option<&LiveChannelHandle> {
        self.replacement.as_ref()
    }

    #[must_use]
    pub fn canonical_seed_cursor(&self) -> Option<u64> {
        self.canonical_seed_cursor
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveExecutionIdentityContractError {
    InvalidEnvelope(String),
    LegacyFieldConflict(&'static str),
    InvalidExperimentalTarget(&'static str),
    ProviderNativeField(&'static str),
}

impl fmt::Display for LiveExecutionIdentityContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEnvelope(detail) => {
                write!(f, "invalid execution_identity: {detail}")
            }
            Self::LegacyFieldConflict(field) => write!(
                f,
                "execution_identity conflicts with legacy top-level `{field}`"
            ),
            Self::InvalidExperimentalTarget(detail) => {
                write!(f, "invalid experimental live target: {detail}")
            }
            Self::ProviderNativeField(field) => write!(
                f,
                "experimental live/open does not accept provider-native `{field}`"
            ),
        }
    }
}

impl std::error::Error for LiveExecutionIdentityContractError {}

/// Parse and validate the optional execution identity portion of live/open.
///
/// This intentionally validates only the new nested field. Existing live
/// params remain owned by the upstream live-open contract while Meta is
/// converging on the shared orchestrator.
pub fn parse_live_open_execution_identity(
    params: &Value,
) -> Result<Option<LiveExecutionIdentityV1>, LiveExecutionIdentityContractError> {
    let Some(object) = params.as_object() else {
        return Ok(None);
    };
    let Some(raw) = object.get("execution_identity") else {
        return Ok(None);
    };
    if object.contains_key("model") {
        return Err(LiveExecutionIdentityContractError::LegacyFieldConflict(
            "model",
        ));
    }
    if object.contains_key("provider") {
        return Err(LiveExecutionIdentityContractError::LegacyFieldConflict(
            "provider",
        ));
    }
    serde_json::from_value(raw.clone())
        .map(Some)
        .map_err(|error| LiveExecutionIdentityContractError::InvalidEnvelope(error.to_string()))
}

/// Validate the surface-only portion of strict experimental live targeting.
///
/// Authoritative identity resolution and stale-binding rejection remain in
/// the identity runtime. This function prevents compatibility target fields or
/// provider-native configuration from entering that resolution path.
pub fn validate_experimental_live_open_surface(
    params: &Value,
) -> Result<(), LiveExecutionIdentityContractError> {
    validate_experimental_live_target_surface(params)?;
    let object = params.as_object().ok_or(
        LiveExecutionIdentityContractError::InvalidExperimentalTarget("params must be an object"),
    )?;
    for field in [
        "mode",
        "execution_mode",
        "profile",
        "profile_id",
        "execution_profile",
        "execution_profile_id",
        "delegation",
        "delegation_type",
        "delegation_model",
        "responses",
        "responses_model",
        "responses_tools",
        "responses_instructions",
        "bridge_model",
        "bridge_tools",
        "bridge_instructions",
        "auth_binding",
        "self_hosted_server_id",
        "provider_params",
        "tools",
        "instructions",
    ] {
        if object.contains_key(field) {
            return Err(LiveExecutionIdentityContractError::ProviderNativeField(
                field,
            ));
        }
    }
    Ok(())
}

/// Validate the identity-only target carried by every strict channel handle
/// operation. The string is only a lookup claim; RPC resolution must still
/// prove its current durable lifecycle binding and exact session ownership.
pub fn validate_experimental_live_target_surface(
    params: &Value,
) -> Result<(), LiveExecutionIdentityContractError> {
    let Some(object) = params.as_object() else {
        return Err(
            LiveExecutionIdentityContractError::InvalidExperimentalTarget(
                "params must be an object",
            ),
        );
    };
    let identity = object
        .get("identity")
        .and_then(Value::as_str)
        .filter(|identity| !identity.trim().is_empty());
    let Some(identity) = identity else {
        return Err(
            LiveExecutionIdentityContractError::InvalidExperimentalTarget(
                "exactly one non-empty identity is required",
            ),
        );
    };
    if object.contains_key("member_id") || object.contains_key("session_id") {
        return Err(
            LiveExecutionIdentityContractError::InvalidExperimentalTarget(
                "member_id and session_id are not eligible",
            ),
        );
    }
    if crate::member_comms_id::is_reserved_generated_alias(identity) {
        return Err(
            LiveExecutionIdentityContractError::InvalidExperimentalTarget(
                "runtime member aliases are not eligible",
            ),
        );
    }
    Ok(())
}
