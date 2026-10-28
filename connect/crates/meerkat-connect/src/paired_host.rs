//! A captured-origin client for the paired host's existing console pairing and
//! frontend inventory, source inspection, preparation, owner change requests and nonsecret
//! extension-secret intents.
//!
//! The presenter supplies the human-selected origin and pairing code outside
//! the model conversation. This client has no configurable routes, token
//! export, dev-harness fallback or activation action. Submission records an
//! exact prepared change for the host operator; it does not apply it. The human presenter can
//! remember only a confirmed device cookie through the OS custody adapter.
//!
//! Each call is a single attempt. Dropping its future stops local observation,
//! not work already accepted by the paired host. In particular, a lost pairing reply
//! can follow a delivered code or a registered device. Do not automatically
//! retry or describe cancellation as host rollback. The paired host rechecks pairing,
//! network eligibility and current configuration on every candidate request.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::Arc,
    time::Duration,
};

use crate::paired_host_device::{ConfirmedDevice, DeviceStoreError};
use reqwest::{
    Client, Response, Url,
    cookie::{CookieStore, Jar},
    header::{ACCEPT, CACHE_CONTROL, CONTENT_TYPE, HeaderName, HeaderValue, ORIGIN, SET_COOKIE},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::value::RawValue;

const MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_COOKIE_BYTES: usize = 4096;
const MAX_COOKIE_HEADERS: usize = 3;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PAIR_REQUEST: &str = "/console/pair/request";
const PAIR_CONFIRM: &str = "/console/pair/confirm";
const PAIR_TERMINAL: &str = "/console/pair/terminal";
const FRONTEND_INVENTORY: &str = "/console/extensions/frontend/inventory";
const FRONTEND_PREPARE: &str = "/console/extensions/frontend/prepare";
const FRONTEND_CHANGES: &str = "/console/extensions/frontend/change-requests";
const FRONTEND_REVIEW: &str = "/console/extensions/frontend/review";
const FRONTEND_SOURCE_TAGS: &str = "/console/extensions/frontend/sources/tags";
const FRONTEND_SOURCE_INSPECT: &str = "/console/extensions/frontend/sources/inspect";
const MAX_SOURCE_BODY_BYTES: usize = 256 * 1024;
// The owner currently resolves for up to 20s, then fetches/reads for up to 90s.
// Its serialized queue can take longer; this bounds our observation, not its work.
const SOURCE_INSPECT_TIMEOUT: Duration = Duration::from_secs(120);
const SECRET_INTENTS: &str = "/console/extensions/secrets/intents";
const MAX_SECRET_INTENT_REQUEST_BYTES: usize = 8 * 1024;
const CHANGE_DIGEST_PREFIX: &str = "paired-host-change-v1:sha256:";

/// A human-entered code, deliberately neither serializable nor printable.
/// This does not promise memory zeroization or protection from the process owner.
pub struct PairingCode(String);

impl PairingCode {
    pub fn new(code: String) -> Result<Self, PairedHostError> {
        if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(PairedHostError::InvalidPairingCode);
        }
        Ok(Self(code))
    }
}

impl fmt::Debug for PairingCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingCode([redacted])")
    }
}

/// The one-use code issued on the selected host's protected terminal.
/// It is never a model argument, printable value or serializable credential.
pub struct TerminalPairingCode(String);

impl TerminalPairingCode {
    pub fn new(code: String) -> Result<Self, PairedHostError> {
        const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
        let grouped = code.len() == 14 && code.as_bytes()[4] == b'-' && code.as_bytes()[9] == b'-';
        if code.len() != 12 && !grouped {
            return Err(PairedHostError::InvalidTerminalPairingCode);
        }
        let normalized: String = code
            .bytes()
            .enumerate()
            .filter(|(index, _)| !grouped || !matches!(*index, 4 | 9))
            .map(|(_, byte)| char::from(byte.to_ascii_uppercase()))
            .collect();
        if normalized.len() != 12 || !normalized.bytes().all(|byte| ALPHABET.contains(&byte)) {
            return Err(PairedHostError::InvalidTerminalPairingCode);
        }
        Ok(Self(normalized))
    }
}

impl fmt::Debug for TerminalPairingCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TerminalPairingCode([redacted])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairedHostOperation {
    RequestPairing,
    ConfirmPairing,
    ConfirmTerminalPairing,
    FrontendInventory,
    PrepareCandidate,
    SubmitFrontendChange,
    FrontendChangeStatus,
    FrontendSourceTags,
    FrontendSourceInspect,
    CreateExtensionSecretIntent,
    ExtensionSecretIntentStatus,
    CancelExtensionSecretIntent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnconfirmedReason {
    Transport,
    Timeout,
    Redirect,
    ResponseTooLarge,
    UnexpectedResponse,
    HostFailure,
}

/// Only known, value-free host refusal codes are exposed as errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairedHostRefusal {
    EndpointUnavailable,
    UnsupportedMediaType,
    ConsoleOwnerRequired,
    PairingUnavailable,
    PairingLocked,
    TooManyRequests,
    NotifyFailed,
    NoPendingRequest,
    TooManyAttempts,
    WrongCode,
}

/// No variant retains a URL, response body, HTTP header, code or source error.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PairedHostError {
    #[error("select an HTTP or HTTPS origin without credentials, path, query or fragment")]
    InvalidOrigin,
    #[error("the private paired host presenter credential is invalid")]
    InvalidPresenterCredential,
    #[error("the paired host presenter credential does not admit this operation")]
    PresenterOperationUnavailable,
    #[error("the pairing label must contain 1-60 characters and no control characters")]
    InvalidPairingLabel,
    #[error("enter the six-digit pairing code outside the agent conversation")]
    InvalidPairingCode,
    #[error("enter the 12-symbol host-terminal pairing code outside the agent conversation")]
    InvalidTerminalPairingCode,
    #[error("the frontend change digest is not in the supported form")]
    InvalidChangeDigest,
    #[error(
        "use a canonical HTTPS repository URL ending in .git, without credentials, port, query or fragment"
    )]
    InvalidSourceRepository,
    #[error("the source tag is not in the host's supported form")]
    InvalidSourceTag,
    #[error("use a canonical relative package path, or omit it for the repository root")]
    InvalidPackagePath,
    #[error("choose the configuration advertised by this source inspection")]
    InvalidFrontendConfiguration,
    #[error("the extension secret intent ID is not in the supported form")]
    InvalidSecretIntentId,
    #[error("the extension secret intent request is not in the supported form")]
    InvalidSecretIntentRequest,
    #[error("the frontend change was not captured by this paired host client")]
    UnboundFrontendChange,
    #[error(
        "The paired host returned a different new-extension proposal; no change was captured; use the owner's canonical distribution, version and repository spellings"
    )]
    NewExtensionProposalMismatch,
    #[error("the frontend request exceeds the supported size limit")]
    RequestTooLarge,
    #[error("the paired host HTTP client could not be constructed")]
    ClientUnavailable,
    #[error("The paired host refused {operation:?}: {reason:?}")]
    Refused {
        operation: PairedHostOperation,
        reason: PairedHostRefusal,
    },
    #[error(
        "the paired host {operation:?} outcome is unconfirmed ({reason:?}); do not retry automatically"
    )]
    Unconfirmed {
        operation: PairedHostOperation,
        reason: UnconfirmedReason,
    },
}

/// The closed host request. Identifiers, roster membership, group validity and
/// compilation remain judgments of the paired host, not a duplicate Toolkit policy.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendCandidateRequest {
    pub extension_id: String,
    #[serde(
        default,
        deserialize_with = "present_text",
        skip_serializing_if = "Option::is_none"
    )]
    pub channel_id: Option<String>,
    pub ingress: FrontendIngress,
    pub participants: FrontendParticipants,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendIngress {
    Polling,
    Webhook,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendParticipants {
    pub approved_groups: Vec<String>,
}

/// An acquisition proposal, not a wheel, ledger row or installation command.
/// Use the owner's canonical spellings: capture requires the returned proposal
/// to equal these inputs. The paired host owns source eligibility and normalization.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "NewExtensionRequestWire")]
pub struct FrontendNewExtensionRequest {
    pub distribution: String,
    pub version: String,
    pub source: FrontendExtensionSource,
    #[serde(flatten)]
    pub configuration: FrontendExtensionConfiguration,
}

/// Exactly one configuration family, as selected by the source owner.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendExtensionConfiguration {
    Channel(FrontendExtensionChannel),
    Roles(FrontendExtensionRoles),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewExtensionRequestWire {
    distribution: String,
    version: String,
    source: FrontendExtensionSource,
    #[serde(default, deserialize_with = "present_field")]
    channel: Option<FrontendExtensionChannel>,
    #[serde(default, deserialize_with = "present_field")]
    roles: Option<FrontendExtensionRoles>,
}

impl TryFrom<NewExtensionRequestWire> for FrontendNewExtensionRequest {
    type Error = &'static str;
    fn try_from(wire: NewExtensionRequestWire) -> Result<Self, Self::Error> {
        let configuration = match (wire.channel, wire.roles) {
            (Some(channel), None) => FrontendExtensionConfiguration::Channel(channel),
            (None, Some(roles)) => FrontendExtensionConfiguration::Roles(roles),
            _ => return Err("expected exactly one channel or roles configuration"),
        };
        Ok(Self {
            distribution: wire.distribution,
            version: wire.version,
            source: wire.source,
            configuration,
        })
    }
}

/// Omission means the root package. A present value is never normalized.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct FrontendPackagePath(String);

impl FrontendPackagePath {
    pub fn new(value: impl Into<String>) -> Result<Self, PairedHostError> {
        let value = value.into();
        let segments: Vec<_> = value.split('/').collect();
        if value.len() > 200
            || value.contains("..")
            || segments.len() > 8
            || segments.iter().any(|part| {
                part.is_empty()
                    || part.len() > 100
                    || !part.as_bytes()[0].is_ascii_alphanumeric()
                    || !part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            })
        {
            return Err(PairedHostError::InvalidPackagePath);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for FrontendPackagePath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?)
            .map_err(|_| serde::de::Error::custom("invalid package path"))
    }
}

pub(crate) fn present_field<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendExtensionSource {
    pub repo: String,
    pub tag: String,
    pub commit: String,
    #[serde(
        default,
        deserialize_with = "present_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub package_path: Option<FrontendPackagePath>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendExtensionChannel {
    pub ingress: FrontendIngress,
    pub approved_groups: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendRoleDaemon {
    pub voice_mode: String,
}

/// Shape only. The paired host's role owner judges eligibility and host bindings.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "ExtensionRolesWire")]
pub struct FrontendExtensionRoles {
    pub selected_roles: Vec<String>,
    pub daemon: FrontendRoleDaemon,
    pub required_facades: Vec<String>,
    pub voice_map: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtensionRolesWire {
    selected_roles: Vec<String>,
    daemon: FrontendRoleDaemon,
    required_facades: Vec<String>,
    #[serde(deserialize_with = "unique_voice_map")]
    voice_map: BTreeMap<String, String>,
}

fn unique_voice_map<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a unique voice map")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, String>()? {
                if result.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate voice map key"));
                }
                if result.len() > 16 {
                    return Err(serde::de::Error::custom("voice map exceeds limit"));
                }
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(Visitor)
}

fn role_shape(roles: &[String], daemon: &FrontendRoleDaemon, facades: &[String]) -> bool {
    fn text(value: &str, max: usize) -> bool {
        !value.is_empty() && value.chars().count() <= max
    }
    !roles.is_empty()
        && roles.len() <= 8
        && roles.iter().all(|v| text(v, 64))
        && text(&daemon.voice_mode, 32)
        && facades.len() <= 16
        && facades.iter().all(|facade| {
            let Some((name, version)) = facade.split_once(".v") else {
                return false;
            };
            !name.is_empty()
                && name.len() <= 64
                && name.as_bytes()[0].is_ascii_lowercase()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                && !version.is_empty()
                && version.len() <= 3
                && version.bytes().all(|b| b.is_ascii_digit())
                && facade.len() <= 64
        })
}

impl TryFrom<ExtensionRolesWire> for FrontendExtensionRoles {
    type Error = &'static str;
    fn try_from(wire: ExtensionRolesWire) -> Result<Self, Self::Error> {
        if !role_shape(&wire.selected_roles, &wire.daemon, &wire.required_facades)
            || wire.voice_map.len() > 16
            || wire.voice_map.iter().any(|(key, value)| {
                key.is_empty()
                    || key.chars().count() > 128
                    || value.is_empty()
                    || value.chars().count() > 128
            })
        {
            return Err("invalid role configuration shape");
        }
        Ok(Self {
            selected_roles: wire.selected_roles,
            daemon: wire.daemon,
            required_facades: wire.required_facades,
            voice_map: wire.voice_map,
        })
    }
}

/// The advertised choices are host observations, not a local role registry.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendSourceRoles {
    pub selections: Vec<FrontendRoleSelection>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendRoleSelection {
    pub selected_roles: Vec<String>,
    pub daemon: FrontendRoleDaemon,
    pub required_facades: Vec<String>,
    pub voice_map_entries: usize,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "outcome", content = "result", rename_all = "snake_case")]
pub enum FrontendSourceRead<T> {
    Observed(T),
    Rejected(FrontendSourceRejection),
}

/// Ordered host observations, capped at 500. A full list does not establish
/// that the repository has no other tags, or that these objects are packages.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendSourceTags {
    pub repo: String,
    pub tags: Vec<FrontendSourceTag>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendSourceTag {
    pub tag: String,
    pub commit: String,
}

/// Static metadata read by the paired host at its resolved commit. This neither
/// executes package code nor proves the wheel manifest or frontend readiness.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendSourceInspection {
    #[serde(
        default,
        deserialize_with = "present_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub package_path: Option<FrontendPackagePath>,
    #[serde(
        default,
        deserialize_with = "present_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub roles: Option<FrontendSourceRoles>,
    pub repo: String,
    pub tag: String,
    pub commit: String,
    pub distribution: String,
    pub version: String,
    pub entry_point: FrontendSourceEntryPoint,
    pub declared: FrontendSourceDeclared,
    pub unverified: Vec<FrontendExtensionUnverified>,
    pub already_declared: bool,
    pub prepare: FrontendSourcePrepare,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendSourceEntryPoint {
    pub group: String,
    pub name: String,
    pub object: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendSourceDeclared {}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendSourcePrepare {
    pub new_extension: FrontendSourceDraft,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "SourceDraftWire")]
pub struct FrontendSourceDraft {
    pub distribution: String,
    pub version: String,
    pub source: FrontendExtensionSource,
    #[serde(flatten)]
    pub configuration: FrontendSourceConfiguration,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendSourceConfiguration {
    Channel(FrontendSourceUnselectedChannel),
    Roles(FrontendSourceUnselectedRoles),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceDraftWire {
    distribution: String,
    version: String,
    source: FrontendExtensionSource,
    #[serde(default, deserialize_with = "present_field")]
    channel: Option<FrontendSourceUnselectedChannel>,
    #[serde(default, deserialize_with = "present_field")]
    roles: Option<FrontendSourceUnselectedRoles>,
}

impl TryFrom<SourceDraftWire> for FrontendSourceDraft {
    type Error = &'static str;
    fn try_from(wire: SourceDraftWire) -> Result<Self, Self::Error> {
        let configuration = match (wire.channel, wire.roles) {
            (Some(channel), None) => FrontendSourceConfiguration::Channel(channel),
            (None, Some(roles)) => FrontendSourceConfiguration::Roles(roles),
            _ => return Err("expected exactly one unselected channel or roles configuration"),
        };
        Ok(Self {
            distribution: wire.distribution,
            version: wire.version,
            source: wire.source,
            configuration,
        })
    }
}

/// Required nulls are owner placeholders, not default choices.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendSourceUnselectedChannel {
    pub ingress: (),
    pub approved_groups: (),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendSourceUnselectedRoles {
    pub selected_roles: (),
    pub daemon: FrontendSourceUnselectedDaemon,
    pub required_facades: (),
    pub voice_map: (),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendSourceUnselectedDaemon {
    pub voice_mode: (),
}

impl FrontendSourceInspection {
    fn request(
        &self,
        configuration: FrontendExtensionConfiguration,
    ) -> FrontendNewExtensionRequest {
        FrontendNewExtensionRequest {
            distribution: self.prepare.new_extension.distribution.clone(),
            version: self.prepare.new_extension.version.clone(),
            source: self.prepare.new_extension.source.clone(),
            configuration,
        }
    }

    /// Fill only the channel choices of a channel-owned draft.
    pub fn with_channel(
        &self,
        channel: FrontendExtensionChannel,
    ) -> Result<FrontendNewExtensionRequest, PairedHostError> {
        if !matches!(
            self.prepare.new_extension.configuration,
            FrontendSourceConfiguration::Channel(_)
        ) || self.roles.is_some()
        {
            return Err(PairedHostError::InvalidFrontendConfiguration);
        }
        Ok(self.request(FrontendExtensionConfiguration::Channel(channel)))
    }

    /// Copy one advertised selection and the person's corresponding map.
    /// This does not judge whether the identities, daemon or binding exist.
    pub fn with_roles(
        &self,
        roles: FrontendExtensionRoles,
    ) -> Result<FrontendNewExtensionRequest, PairedHostError> {
        if !matches!(
            self.prepare.new_extension.configuration,
            FrontendSourceConfiguration::Roles(_)
        ) || !self.roles.as_ref().is_some_and(|choices| {
            choices.selections.iter().any(|choice| {
                choice.selected_roles == roles.selected_roles
                    && choice.daemon == roles.daemon
                    && choice.required_facades == roles.required_facades
                    && choice.voice_map_entries == roles.voice_map.len()
            })
        }) {
            return Err(PairedHostError::InvalidFrontendConfiguration);
        }
        Ok(self.request(FrontendExtensionConfiguration::Roles(roles)))
    }

    fn has_configuration_shape(&self) -> bool {
        match (&self.prepare.new_extension.configuration, &self.roles) {
            (FrontendSourceConfiguration::Channel(_), None) => {
                self.unverified == channel_unverified()
            }
            (FrontendSourceConfiguration::Roles(_), Some(roles)) => {
                self.unverified == role_unverified()
                    && !roles.selections.is_empty()
                    && roles.selections.iter().enumerate().all(|(index, choice)| {
                        role_shape(
                            &choice.selected_roles,
                            &choice.daemon,
                            &choice.required_facades,
                        ) && choice
                            .selected_roles
                            .windows(2)
                            .all(|pair| pair[0] < pair[1])
                            && choice.voice_map_entries <= 16
                            && !roles.selections[..index].contains(choice)
                    })
            }
            _ => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendSourceRejectionKind {
    InvalidRequest,
    Forbidden,
    NotFound,
    Unprocessable,
    BadGateway,
    Unavailable,
    InternalError,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendSourceUnavailableReason {
    HostCredentialInvalid,
    Timeout,
    LimitExceeded,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendSourceProblem {
    pub code: String,
    pub path: String,
    pub message: String,
    #[serde(
        default,
        deserialize_with = "present_source_reason",
        skip_serializing_if = "Option::is_none"
    )]
    pub reason: Option<FrontendSourceUnavailableReason>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FrontendSourceRejection {
    pub http_status: u16,
    pub error: FrontendSourceRejectionKind,
    pub problems: Vec<FrontendSourceProblem>,
}

fn present_source_reason<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<FrontendSourceUnavailableReason>, D::Error> {
    FrontendSourceUnavailableReason::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "outcome", content = "result", rename_all = "snake_case")]
pub enum FrontendNewExtensionPreparation {
    Candidate(Box<FrontendNewExtensionCandidateReceipt>),
    Rejected(FrontendRequestRejection),
}

/// `valid` means eligible for operator build and review. No package, facade,
/// channel template or secret has been verified by this preparation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendNewExtensionCandidateReceipt {
    pub valid: bool,
    pub problems: Vec<FrontendProblem>,
    #[serde(deserialize_with = "required_nullable_new_change")]
    pub change: Option<FrontendNewExtensionChange>,
    /// Optional host-owned human review path, relative to the captured origin.
    /// Retention and review do not authorize submission or activation.
    #[serde(
        default,
        deserialize_with = "present_text",
        skip_serializing_if = "Option::is_none"
    )]
    pub review_url: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendNewExtensionKind {
    NewExtension,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendExtensionUnverified {
    Manifest,
    Facades,
    ChannelTemplate,
    RoleContract,
    HostBindings,
    Secrets,
}

fn channel_unverified() -> [FrontendExtensionUnverified; 4] {
    use FrontendExtensionUnverified::*;
    [Manifest, Facades, ChannelTemplate, Secrets]
}

fn role_unverified() -> [FrontendExtensionUnverified; 5] {
    use FrontendExtensionUnverified::*;
    [Manifest, Facades, RoleContract, HostBindings, Secrets]
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendBuildReviewState {
    Pending,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrontendExtensionPrerequisite {
    BuildAndReview {
        state: FrontendBuildReviewState,
    },
    /// Host environment-name presence only, not provider authentication or
    /// successful protected secret entry.
    Secret {
        name: String,
        provisioned: bool,
    },
}

fn pending_build_review(prerequisites: &[FrontendExtensionPrerequisite]) -> bool {
    matches!(
        prerequisites,
        [FrontendExtensionPrerequisite::BuildAndReview {
            state: FrontendBuildReviewState::Pending
        }]
    )
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontendNewExtensionMetadata {
    change_digest: FrontendChangeDigest,
    kind: FrontendNewExtensionKind,
    base: FrontendChangeBase,
    proposal: FrontendNewExtensionRequest,
    unverified: Vec<FrontendExtensionUnverified>,
    prerequisites: Vec<FrontendExtensionPrerequisite>,
    unified_summary: String,
}

/// Immutable owner bytes, bound only by this client's preparation response.
/// The summary and digest are neither generated nor reconstructed locally.
#[derive(Clone)]
pub struct FrontendNewExtensionChange {
    raw: Box<RawValue>,
    metadata: FrontendNewExtensionMetadata,
    client_binding: Option<Arc<()>>,
}

impl FrontendNewExtensionChange {
    pub fn change_digest(&self) -> &FrontendChangeDigest {
        &self.metadata.change_digest
    }

    pub fn base(&self) -> &FrontendChangeBase {
        &self.metadata.base
    }

    pub fn proposal(&self) -> &FrontendNewExtensionRequest {
        &self.metadata.proposal
    }

    pub fn unified_summary(&self) -> &str {
        &self.metadata.unified_summary
    }
}

impl fmt::Debug for FrontendNewExtensionChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrontendNewExtensionChange")
            .field("change_digest", self.change_digest())
            .finish_non_exhaustive()
    }
}

impl PartialEq for FrontendNewExtensionChange {
    fn eq(&self, other: &Self) -> bool {
        self.raw.get() == other.raw.get()
    }
}

impl Eq for FrontendNewExtensionChange {}

impl Serialize for FrontendNewExtensionChange {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FrontendNewExtensionChange {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        serde_json::from_str::<UniqueJson>(raw.get())
            .map_err(|_| serde::de::Error::custom("invalid change JSON"))?;
        let metadata: FrontendNewExtensionMetadata = serde_json::from_str(raw.get())
            .map_err(|_| serde::de::Error::custom("unsupported change shape"))?;
        let expected = match metadata.proposal.configuration {
            FrontendExtensionConfiguration::Channel(_) => channel_unverified().to_vec(),
            FrontendExtensionConfiguration::Roles(_) => role_unverified().to_vec(),
        };
        if metadata.kind != FrontendNewExtensionKind::NewExtension
            || metadata.unverified != expected
            || !pending_build_review(&metadata.prerequisites)
        {
            return Err(serde::de::Error::custom("unsupported change prerequisites"));
        }
        Ok(Self {
            raw,
            metadata,
            client_binding: None,
        })
    }
}

fn required_nullable_new_change<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<FrontendNewExtensionChange>, D::Error> {
    Option::<FrontendNewExtensionChange>::deserialize(deserializer)
}

/// Current host declarations, not readiness or a destination-agent selector.
/// Secret presence covers every listed ingress; only a candidate check judges
/// prerequisites for the selected mode. Sender identities are informational.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendInventory {
    pub current: FrontendCurrent,
    /// The host's own extension ledger file. A client that has read it accepts
    /// only channel changes that patch exactly this file.
    pub ledger_path: String,
    pub extensions: Vec<FrontendExtension>,
    pub routing: FrontendRouting,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendExtension {
    pub extension_id: String,
    pub enabled: bool,
    pub wheel_present: bool,
    pub channels: Vec<FrontendChannel>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendChannel {
    pub channel_id: String,
    pub ingress: Vec<FrontendIngress>,
    pub secrets: Vec<FrontendSecretPresence>,
    pub approved_groups: FrontendDeclaredGroups,
    pub senders: FrontendSenders,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged, deny_unknown_fields)]
pub enum FrontendSecretPresence {
    Legacy {
        name: String,
        present: bool,
    },
    Environment {
        name: String,
        present: bool,
        source: FrontendEnvironmentSecretSource,
        generation: (),
    },
    Vault {
        name: String,
        present: bool,
        source: FrontendVaultSecretSource,
        #[serde(deserialize_with = "required_nullable_generation")]
        generation: Option<u64>,
        #[serde(deserialize_with = "required_nullable_binding_status")]
        binding_status: Option<FrontendSecretBindingStatus>,
    },
}

impl FrontendSecretPresence {
    pub fn name(&self) -> &str {
        match self {
            Self::Legacy { name, .. }
            | Self::Environment { name, .. }
            | Self::Vault { name, .. } => name,
        }
    }

    /// Host-reported presence is not authentication or frontend readiness.
    pub fn present(&self) -> bool {
        match self {
            Self::Legacy { present, .. }
            | Self::Environment { present, .. }
            | Self::Vault { present, .. } => *present,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum FrontendEnvironmentSecretSource {
    #[serde(rename = "env")]
    Environment,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum FrontendVaultSecretSource {
    #[serde(rename = "vault")]
    Vault,
}

/// The paired host's process-local binding observation, not provider authentication.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendSecretBindingStatus {
    Live,
    CommittedNotLive,
}

fn required_nullable_generation<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    Option::<u64>::deserialize(deserializer)
}

fn required_nullable_binding_status<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<FrontendSecretBindingStatus>, D::Error> {
    Option::<FrontendSecretBindingStatus>::deserialize(deserializer)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendDeclaredGroups {
    pub declared: Vec<String>,
    pub discovery: FrontendGroupDiscovery,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendGroupDiscovery {
    Unavailable,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendSenders {
    pub identities: Vec<String>,
    pub informational: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendRouting {
    SenderIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontendInventoryRead {
    Inventory(FrontendInventory),
    Rejected(FrontendInventoryRejection),
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FrontendInventoryRejection {
    pub http_status: u16,
    pub error: FrontendInventoryRejectionKind,
    pub problems: Vec<FrontendProblem>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendInventoryRejectionKind {
    Conflict,
    Unavailable,
    InternalError,
}

/// Either exact candidate evidence or the host's structured request rejection.
/// Neither variant authorizes installation, approval, activation or task resume.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontendPreparation {
    Candidate(Box<FrontendCandidateReceipt>),
    Rejected(FrontendRequestRejection),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendCandidateReceipt {
    pub valid: bool,
    pub problems: Vec<FrontendProblem>,
    pub missing_prerequisites: Vec<FrontendPrerequisite>,
    pub current: FrontendCurrent,
    pub candidate: FrontendCandidate,
    /// Older read-only hosts may omit this. Absence is never a change that can
    /// be submitted, and a valid compile alone never supplies one locally.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change: Option<FrontendChange>,
    /// Absent on older hosts or when the host did not retain a review draft.
    /// This is a same-origin human review path, never submission authority.
    #[serde(
        default,
        deserialize_with = "present_text",
        skip_serializing_if = "Option::is_none"
    )]
    pub review_url: Option<String>,
    pub compiled_is_not_ready: bool,
}

/// A host digest, not an arbitrary URL/path or an authorization capability.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct FrontendChangeDigest(String);

impl FrontendChangeDigest {
    pub fn new(value: &str) -> Result<Self, PairedHostError> {
        let Some(hex) = value.strip_prefix(CHANGE_DIGEST_PREFIX) else {
            return Err(PairedHostError::InvalidChangeDigest);
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(PairedHostError::InvalidChangeDigest);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for FrontendChangeDigest {
    type Error = PairedHostError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<FrontendChangeDigest> for String {
    fn from(value: FrontendChangeDigest) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendChangeBase {
    #[serde(deserialize_with = "required_nullable_text")]
    pub activation_id: Option<String>,
    #[serde(deserialize_with = "required_nullable_text")]
    pub policy_digest: Option<String>,
    pub ledger_digest: String,
}

/// A host observation reference, never a credential or permission to enter/commit.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(try_from = "String", into = "String")]
pub struct ExtensionSecretIntentId(String);

impl ExtensionSecretIntentId {
    pub fn new(value: &str) -> Result<Self, PairedHostError> {
        let valid = value.strip_prefix("esi_").is_some_and(|suffix| {
            suffix.len() == 20
                && suffix
                    .bytes()
                    .all(|byte| b"abcdefghijkmnpqrstuvwxyz23456789".contains(&byte))
        });
        if !valid {
            return Err(PairedHostError::InvalidSecretIntentId);
        }
        Ok(Self(value.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ExtensionSecretIntentId {
    type Error = PairedHostError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl From<ExtensionSecretIntentId> for String {
    fn from(value: ExtensionSecretIntentId) -> Self {
        value.0
    }
}

/// Exact nonsecret owner inputs. Creating an intent may discard another pending
/// intent for this binding. It is an effect, not an availability probe.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged, deny_unknown_fields)]
pub enum ExtensionSecretIntentRequest {
    Ledger {
        extension_id: String,
        binding_name: String,
        base: FrontendChangeBase,
    },
    Candidate {
        candidate: ExtensionSecretIntentCandidate,
        binding_name: String,
    },
}

impl ExtensionSecretIntentRequest {
    // The client owns local validity. Consent composition uses the same check
    // before minting a decision; the transport still enforces it independently.
    pub(crate) fn checked_body(&self) -> Result<Vec<u8>, PairedHostError> {
        let binding = match self {
            Self::Ledger {
                extension_id,
                binding_name,
                ..
            } => {
                if extension_id.is_empty() {
                    return Err(PairedHostError::InvalidSecretIntentRequest);
                }
                binding_name
            }
            Self::Candidate {
                candidate,
                binding_name,
            } => {
                if !lower_hex(&candidate.wheel_sha256, 64) {
                    return Err(PairedHostError::InvalidSecretIntentRequest);
                }
                binding_name
            }
        };
        if binding.is_empty() {
            return Err(PairedHostError::InvalidSecretIntentRequest);
        }
        let body = serde_json::to_vec(self).map_err(|_| PairedHostError::RequestTooLarge)?;
        if body.len() > MAX_SECRET_INTENT_REQUEST_BYTES {
            return Err(PairedHostError::RequestTooLarge);
        }
        Ok(body)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExtensionSecretIntentCandidate {
    pub change_digest: FrontendChangeDigest,
    pub wheel_sha256: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionSecretIntentState {
    AwaitingEntry,
    AwaitingOwner,
    Committed,
    Rejected,
    Cancelled,
    Superseded,
    StaleBase,
}

impl ExtensionSecretIntentState {
    fn closed(self) -> bool {
        !matches!(self, Self::AwaitingEntry | Self::AwaitingOwner)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionSecretPresenter {
    Terminal,
    Telegram,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionSecretIntentClosure {
    OwnerRejected,
    StaleBase,
    BindingChanged,
    ConsumerUnresolved,
    GenerationConflict,
    PendingMissing,
    FingerprintMismatch,
}

/// The host's observation of this intent's committed generation. This is
/// separate from inventory's current-slot status and is not provider readiness.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionSecretIntentBindingStatus {
    Live,
    CommittedNotLive,
    SupersededGeneration,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExtensionSecretIntentTarget {
    pub host: String,
    #[serde(deserialize_with = "required_nullable_text")]
    pub activation_id: Option<String>,
    pub extension_id: String,
    pub binding_name: String,
    pub channel_ids: Vec<String>,
    pub consumer: String,
    /// False remains unresolved; the client never infers a consumer.
    pub consumer_verified: bool,
    pub required: bool,
}

/// Nonsecret host observations only. Presence is captured at creation. The
/// committed generation's binding status is supplied by the host, not inferred
/// from inventory. Neither commitment nor host liveness proves provider auth
/// or successful message delivery.
#[derive(Clone, Serialize, PartialEq, Eq)]
pub struct ExtensionSecretIntent {
    pub intent_id: ExtensionSecretIntentId,
    pub state: ExtensionSecretIntentState,
    pub target: ExtensionSecretIntentTarget,
    pub present: bool,
    pub requires_overwrite: bool,
    pub presenter: ExtensionSecretPresenter,
    pub fingerprint: Option<String>,
    pub overwrite: Option<bool>,
    pub generation: Option<u64>,
    pub binding_status: Option<ExtensionSecretIntentBindingStatus>,
    pub reason: Option<ExtensionSecretIntentClosure>,
    #[serde(skip_serializing)]
    entry_path: String,
}

impl fmt::Debug for ExtensionSecretIntent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExtensionSecretIntent")
            .field("intent_id", &self.intent_id)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl ExtensionSecretIntent {
    /// Only for the protected human presenter. This exact relative path is
    /// validated against the host-returned ID; combine it only with this client's
    /// captured origin. Opening it does not pair a browser or authorize commitment.
    pub fn entry_path_for_presenter(&self) -> &str {
        &self.entry_path
    }
}

/// Retains what was requested. The candidate response does not echo source pins;
/// this retained request is not independent proof of a wheel or binding.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ExtensionSecretIntentCreated {
    pub request: ExtensionSecretIntentRequest,
    pub intent: ExtensionSecretIntent,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "outcome", content = "result", rename_all = "snake_case")]
pub enum ExtensionSecretIntentCreation {
    Created(Box<ExtensionSecretIntentCreated>),
    Rejected(ExtensionSecretIntentRejection),
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "outcome", content = "result", rename_all = "snake_case")]
pub enum ExtensionSecretIntentRead {
    Status(ExtensionSecretIntent),
    NotFound,
    Rejected(ExtensionSecretIntentRejection),
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "outcome", content = "result", rename_all = "snake_case")]
pub enum ExtensionSecretIntentCancellation {
    Cancelled(ExtensionSecretIntent),
    Rejected(ExtensionSecretIntentRejection),
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ExtensionSecretIntentRejection {
    pub http_status: u16,
    pub refusal: ExtensionSecretIntentRefusal,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionSecretInvalidRequest {
    InvalidJson,
    BindingName,
    InvalidField,
    UnknownField,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExtensionSecretAdmission {
    #[serde(rename = "paired_console_device_required")]
    PairedConsoleDeviceRequired,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "error", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtensionSecretIntentRefusal {
    InvalidRequest {
        code: ExtensionSecretInvalidRequest,
    },
    InvalidJson,
    BodyTooLarge,
    Forbidden {
        code: ExtensionSecretAdmission,
    },
    StaleBase {
        #[serde(deserialize_with = "required_nullable_secret_base")]
        current: Option<FrontendChangeBase>,
    },
    BindingNotFound,
    ChangeRequestNotFound,
    BindingNotVault,
    NotNewExtension,
    CandidateNotBuilt,
    StaleCandidate,
    ConsumerUnresolved,
    NotFound,
    IntentClosed {
        state: ExtensionSecretIntentState,
    },
    #[serde(rename = "expected application/json")]
    ExpectedJson,
}

fn required_nullable_secret_base<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<FrontendChangeBase>, D::Error> {
    Option::<FrontendChangeBase>::deserialize(deserializer)
}

/// Opaque owner rows remain JSON, including extension-defined registration
/// values. Only the paired host compiles them and renders their canonical TOML diff.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontendLedgerPatch {
    pub path: String,
    pub extension_id: String,
    pub channel_id: String,
    #[serde(deserialize_with = "required_nullable_raw")]
    pub before: Option<Box<RawValue>>,
    pub after: Box<RawValue>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontendChangeMetadata {
    change_digest: FrontendChangeDigest,
    base: FrontendChangeBase,
    ledger_patch: FrontendLedgerPatch,
    unified_diff: String,
}

/// The exact owner-produced JSON, never reconstructed from editable fields.
/// Only `prepare_candidate` can bind this object to its captured client.
/// Deserializing or cloning wire JSON alone grants no submission capability.
#[derive(Clone)]
pub struct FrontendChange {
    raw: Box<RawValue>,
    metadata: FrontendChangeMetadata,
    client_binding: Option<Arc<()>>,
}

impl FrontendChange {
    pub fn change_digest(&self) -> &FrontendChangeDigest {
        &self.metadata.change_digest
    }

    pub fn base(&self) -> &FrontendChangeBase {
        &self.metadata.base
    }

    pub fn ledger_patch(&self) -> &FrontendLedgerPatch {
        &self.metadata.ledger_patch
    }

    pub fn unified_diff(&self) -> &str {
        &self.metadata.unified_diff
    }
}

impl fmt::Debug for FrontendChange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrontendChange")
            .field("change_digest", self.change_digest())
            .finish_non_exhaustive()
    }
}

impl PartialEq for FrontendChange {
    fn eq(&self, other: &Self) -> bool {
        self.raw.get() == other.raw.get()
    }
}

impl Eq for FrontendChange {}

impl Serialize for FrontendChange {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FrontendChange {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        // RawValue preserves floats and every other owner byte. Validate
        // duplicate keys before inspecting metadata, without normalizing it.
        serde_json::from_str::<UniqueJson>(raw.get())
            .map_err(|_| serde::de::Error::custom("invalid change JSON"))?;
        let metadata: FrontendChangeMetadata = serde_json::from_str(raw.get())
            .map_err(|_| serde::de::Error::custom("unsupported change shape"))?;
        if !valid_ledger_path(&metadata.ledger_patch.path)
            || !json_object(&metadata.ledger_patch.after)
            || metadata
                .ledger_patch
                .before
                .as_deref()
                .is_some_and(|row| !json_object(row))
        {
            return Err(serde::de::Error::custom("unsupported ledger patch"));
        }
        Ok(Self {
            raw,
            metadata,
            client_binding: None,
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FrontendSubmitter {
    PairedDevice,
    Harness,
    TrustedNetwork,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendSubmittedState {
    Submitted,
}

/// A durable host request acknowledgment, never an activation acknowledgment.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FrontendSubmissionReceipt {
    pub http_status: u16,
    pub request_id: FrontendChangeDigest,
    pub state: FrontendSubmittedState,
    pub submitted_at: String,
    pub change: FrontendChange,
    pub submitted_by: FrontendSubmitter,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "outcome", content = "result", rename_all = "snake_case")]
pub enum FrontendSubmission {
    Recorded(Box<FrontendSubmissionReceipt>),
    Rejected(FrontendChangeRejection),
}

/// A host-reported later attempt for the same new-extension request digest.
/// This observation is neither a retry grant nor a local attempt counter.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct FrontendAttempt(u16);

impl FrontendAttempt {
    pub fn get(self) -> u16 {
        self.0
    }
}

impl<'de> Deserialize<'de> for FrontendAttempt {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let attempt = u16::deserialize(deserializer)?;
        if !(2..=999).contains(&attempt) {
            return Err(serde::de::Error::custom("unsupported host attempt"));
        }
        Ok(Self(attempt))
    }
}

// Missing is permitted, but an explicitly present null is not an observation.
fn reported_attempt<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<FrontendAttempt>, D::Error> {
    FrontendAttempt::deserialize(deserializer).map(Some)
}

/// A recorded request for host-owned operator build/review, never installation.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FrontendNewExtensionSubmissionReceipt {
    pub http_status: u16,
    pub request_id: FrontendChangeDigest,
    pub state: FrontendSubmittedState,
    pub submitted_at: String,
    pub change: FrontendNewExtensionChange,
    pub submitted_by: FrontendSubmitter,
    /// Older and idempotent host responses may omit this even on a later
    /// attempt. Absence does not establish attempt 1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<FrontendAttempt>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "outcome", content = "result", rename_all = "snake_case")]
pub enum FrontendNewExtensionSubmission {
    Recorded(Box<FrontendNewExtensionSubmissionReceipt>),
    Rejected(FrontendChangeRejection),
}

/// The host-recorded consumer for one declared role secret. This is evidence,
/// not a client-derived role choice or secret-entry authorization.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "consumer", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrontendRoleSecretConsumer {
    Extension { consumer_id: String },
}

/// Host-reported build evidence. The extension-owned template remains opaque.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrontendNewExtensionBuilt {
    pub extension_id: String,
    pub distribution: String,
    pub version: String,
    pub source_commit: String,
    pub wheel_sha256: String,
    pub release_evidence_sha256: String,
    pub facades: Vec<String>,
    #[serde(
        default,
        deserialize_with = "present_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub package_path: Option<FrontendPackagePath>,
    #[serde(deserialize_with = "owner_object_or_null")]
    pub channel_template: Box<RawValue>,
    pub secret_names: Vec<String>,
    /// Present only when the host recorded role-owned consumer facts. Absence
    /// preserves older builds and does not authorize secret entry.
    #[serde(
        default,
        deserialize_with = "present_field",
        skip_serializing_if = "Option::is_none"
    )]
    pub role_consumers: Option<BTreeMap<String, FrontendRoleSecretConsumer>>,
}

impl PartialEq for FrontendNewExtensionBuilt {
    fn eq(&self, other: &Self) -> bool {
        self.extension_id == other.extension_id
            && self.distribution == other.distribution
            && self.version == other.version
            && self.source_commit == other.source_commit
            && self.wheel_sha256 == other.wheel_sha256
            && self.release_evidence_sha256 == other.release_evidence_sha256
            && self.facades == other.facades
            && self.package_path == other.package_path
            && self.channel_template.get() == other.channel_template.get()
            && self.secret_names == other.secret_names
            && self.role_consumers == other.role_consumers
    }
}

impl Eq for FrontendNewExtensionBuilt {}

fn owner_object_or_null<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Box<RawValue>, D::Error> {
    let raw = Box::<RawValue>::deserialize(deserializer)?;
    serde_json::from_str::<UniqueJson>(raw.get())
        .map_err(|_| serde::de::Error::custom("invalid owner JSON"))?;
    if !json_object(&raw) && raw.get().trim() != "null" {
        return Err(serde::de::Error::custom("expected owner object or null"));
    }
    Ok(raw)
}

fn required_nullable_built<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<FrontendNewExtensionBuilt>, D::Error> {
    Option::<FrontendNewExtensionBuilt>::deserialize(deserializer)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendExtensionFailureStage {
    Building,
    Review,
    Activation,
}

/// A fresh host stage observation, not a local workflow or activation grant.
/// `Applied` still does not establish provider authentication or delivery.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrontendNewExtensionStatus {
    Submitted {
        request_id: FrontendChangeDigest,
        submitted_at: String,
        kind: FrontendNewExtensionKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<FrontendAttempt>,
        #[serde(deserialize_with = "required_nullable_text")]
        staged_at: Option<String>,
        #[serde(deserialize_with = "required_nullable_built")]
        built: Option<FrontendNewExtensionBuilt>,
        prerequisites: Vec<FrontendExtensionPrerequisite>,
    },
    Building {
        request_id: FrontendChangeDigest,
        submitted_at: String,
        kind: FrontendNewExtensionKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<FrontendAttempt>,
        #[serde(deserialize_with = "required_nullable_text")]
        staged_at: Option<String>,
        #[serde(deserialize_with = "required_nullable_built")]
        built: Option<FrontendNewExtensionBuilt>,
        prerequisites: Vec<FrontendExtensionPrerequisite>,
    },
    Built {
        request_id: FrontendChangeDigest,
        submitted_at: String,
        kind: FrontendNewExtensionKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<FrontendAttempt>,
        #[serde(deserialize_with = "required_nullable_text")]
        staged_at: Option<String>,
        #[serde(deserialize_with = "required_nullable_built")]
        built: Option<FrontendNewExtensionBuilt>,
        prerequisites: Vec<FrontendExtensionPrerequisite>,
    },
    UnderReview {
        request_id: FrontendChangeDigest,
        submitted_at: String,
        kind: FrontendNewExtensionKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<FrontendAttempt>,
        #[serde(deserialize_with = "required_nullable_text")]
        staged_at: Option<String>,
        #[serde(deserialize_with = "required_nullable_built")]
        built: Option<FrontendNewExtensionBuilt>,
        prerequisites: Vec<FrontendExtensionPrerequisite>,
    },
    Activating {
        request_id: FrontendChangeDigest,
        submitted_at: String,
        kind: FrontendNewExtensionKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<FrontendAttempt>,
        #[serde(deserialize_with = "required_nullable_text")]
        staged_at: Option<String>,
        #[serde(deserialize_with = "required_nullable_built")]
        built: Option<FrontendNewExtensionBuilt>,
        prerequisites: Vec<FrontendExtensionPrerequisite>,
        activation_id: String,
    },
    ActivationUncertain {
        request_id: FrontendChangeDigest,
        submitted_at: String,
        kind: FrontendNewExtensionKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<FrontendAttempt>,
        #[serde(deserialize_with = "required_nullable_text")]
        staged_at: Option<String>,
        #[serde(deserialize_with = "required_nullable_built")]
        built: Option<FrontendNewExtensionBuilt>,
        prerequisites: Vec<FrontendExtensionPrerequisite>,
        activation_id: String,
        journal_phase: String,
    },
    Applied {
        request_id: FrontendChangeDigest,
        submitted_at: String,
        kind: FrontendNewExtensionKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<FrontendAttempt>,
        #[serde(deserialize_with = "required_nullable_text")]
        staged_at: Option<String>,
        #[serde(deserialize_with = "required_nullable_built")]
        built: Option<FrontendNewExtensionBuilt>,
        prerequisites: Vec<FrontendExtensionPrerequisite>,
        activation_id: String,
    },
    Failed {
        request_id: FrontendChangeDigest,
        submitted_at: String,
        kind: FrontendNewExtensionKind,
        #[serde(skip_serializing_if = "Option::is_none")]
        attempt: Option<FrontendAttempt>,
        #[serde(deserialize_with = "required_nullable_text")]
        staged_at: Option<String>,
        #[serde(deserialize_with = "required_nullable_built")]
        built: Option<FrontendNewExtensionBuilt>,
        prerequisites: Vec<FrontendExtensionPrerequisite>,
        stage: FrontendExtensionFailureStage,
        reason: String,
    },
    Rejected {
        request_id: FrontendChangeDigest,
        submitted_at: String,
        kind: FrontendNewExtensionKind,
        reason: String,
        decided_at: String,
    },
}

impl<'de> Deserialize<'de> for FrontendNewExtensionStatus {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        serde_json::from_str::<UniqueJson>(raw.get())
            .map_err(|_| serde::de::Error::custom("invalid status JSON"))?;
        let envelope: BTreeMap<String, Box<RawValue>> = serde_json::from_str(raw.get())
            .map_err(|_| serde::de::Error::custom("unsupported status shape"))?;
        let state = envelope
            .get("state")
            .and_then(|value| serde_json::from_str::<String>(value.get()).ok())
            .ok_or_else(|| serde::de::Error::custom("unsupported status state"))?;
        // Decode each closed variant directly from JSON. Serde's internally
        // tagged enum buffering cannot preserve an opaque RawValue template.
        macro_rules! progress {
            ($variant:ident $(, $extra:ident : $ty:ty)*) => {{
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Wire {
                    #[serde(rename = "state")]
                    _state: String,
                    request_id: FrontendChangeDigest,
                    submitted_at: String,
                    kind: FrontendNewExtensionKind,
                    #[serde(default, deserialize_with = "reported_attempt")]
                    attempt: Option<FrontendAttempt>,
                    #[serde(deserialize_with = "required_nullable_text")]
                    staged_at: Option<String>,
                    #[serde(deserialize_with = "required_nullable_built")]
                    built: Option<FrontendNewExtensionBuilt>,
                    prerequisites: Vec<FrontendExtensionPrerequisite>,
                    $($extra: $ty,)*
                }
                let wire: Wire = serde_json::from_str(raw.get())
                    .map_err(|_| serde::de::Error::custom("unsupported status shape"))?;
                Self::$variant {
                    request_id: wire.request_id,
                    submitted_at: wire.submitted_at,
                    kind: wire.kind,
                    attempt: wire.attempt,
                    staged_at: wire.staged_at,
                    built: wire.built,
                    prerequisites: wire.prerequisites,
                    $($extra: wire.$extra,)*
                }
            }};
        }
        let status = match state.as_str() {
            "submitted" => progress!(Submitted),
            "building" => progress!(Building),
            "built" => progress!(Built),
            "under_review" => progress!(UnderReview),
            "activating" => progress!(Activating, activation_id: String),
            "activation_uncertain" => {
                progress!(ActivationUncertain, activation_id: String, journal_phase: String)
            }
            "applied" => progress!(Applied, activation_id: String),
            "failed" => progress!(Failed, stage: FrontendExtensionFailureStage, reason: String),
            "rejected" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Rejected {
                    #[serde(rename = "state")]
                    _state: String,
                    request_id: FrontendChangeDigest,
                    submitted_at: String,
                    kind: FrontendNewExtensionKind,
                    reason: String,
                    decided_at: String,
                }
                let wire: Rejected = serde_json::from_str(raw.get())
                    .map_err(|_| serde::de::Error::custom("unsupported rejected status"))?;
                Self::Rejected {
                    request_id: wire.request_id,
                    submitted_at: wire.submitted_at,
                    kind: wire.kind,
                    reason: wire.reason,
                    decided_at: wire.decided_at,
                }
            }
            _ => return Err(serde::de::Error::custom("unsupported status state")),
        };
        if !status.has_host_shape() {
            return Err(serde::de::Error::custom("inconsistent status fields"));
        }
        Ok(status)
    }
}

impl FrontendNewExtensionStatus {
    pub fn request_id(&self) -> &FrontendChangeDigest {
        match self {
            Self::Submitted { request_id, .. }
            | Self::Building { request_id, .. }
            | Self::Built { request_id, .. }
            | Self::UnderReview { request_id, .. }
            | Self::Activating { request_id, .. }
            | Self::ActivationUncertain { request_id, .. }
            | Self::Applied { request_id, .. }
            | Self::Failed { request_id, .. }
            | Self::Rejected { request_id, .. } => request_id,
        }
    }

    /// Correlate a status with an already retained proposal. Standalone reads
    /// remain owner observations and do not infer a mode from a null template.
    pub fn matches_proposal(&self, proposal: &FrontendNewExtensionRequest) -> bool {
        let built = match self {
            Self::Submitted { built, .. }
            | Self::Building { built, .. }
            | Self::Built { built, .. }
            | Self::UnderReview { built, .. }
            | Self::Activating { built, .. }
            | Self::ActivationUncertain { built, .. }
            | Self::Applied { built, .. }
            | Self::Failed { built, .. } => built.as_ref(),
            Self::Rejected { .. } => None,
        };
        built.is_none_or(|built| {
            built.distribution == proposal.distribution
                && built.version == proposal.version
                && built.source_commit == proposal.source.commit
                && built.package_path == proposal.source.package_path
                && match &proposal.configuration {
                    FrontendExtensionConfiguration::Channel(_) => {
                        json_object(&built.channel_template)
                    }
                    FrontendExtensionConfiguration::Roles(roles) => {
                        built.channel_template.get().trim() == "null"
                            && roles
                                .required_facades
                                .iter()
                                .all(|facade| built.facades.contains(facade))
                    }
                }
        })
    }

    fn has_host_shape(&self) -> bool {
        let (staged_at, built, prerequisites) = match self {
            Self::Submitted {
                staged_at,
                built,
                prerequisites,
                ..
            }
            | Self::Building {
                staged_at,
                built,
                prerequisites,
                ..
            }
            | Self::Built {
                staged_at,
                built,
                prerequisites,
                ..
            }
            | Self::UnderReview {
                staged_at,
                built,
                prerequisites,
                ..
            }
            | Self::Activating {
                staged_at,
                built,
                prerequisites,
                ..
            }
            | Self::ActivationUncertain {
                staged_at,
                built,
                prerequisites,
                ..
            }
            | Self::Applied {
                staged_at,
                built,
                prerequisites,
                ..
            }
            | Self::Failed {
                staged_at,
                built,
                prerequisites,
                ..
            } => (staged_at, built, prerequisites),
            Self::Rejected { .. } => return true,
        };
        if staged_at.is_none() != matches!(self, Self::Submitted { .. }) {
            return false;
        }
        match self {
            Self::Submitted { .. } | Self::Building { .. } if built.is_some() => return false,
            Self::Failed { .. } => (),
            Self::Submitted { .. } | Self::Building { .. } => (),
            _ if built.is_none() => return false,
            _ => (),
        }
        let Some(built) = built else {
            return pending_build_review(prerequisites);
        };
        let names: BTreeSet<_> = built.secret_names.iter().map(String::as_str).collect();
        let observed: Option<BTreeSet<_>> = prerequisites
            .iter()
            .map(|item| match item {
                FrontendExtensionPrerequisite::Secret { name, .. } => Some(name.as_str()),
                FrontendExtensionPrerequisite::BuildAndReview { .. } => None,
            })
            .collect();
        names.len() == built.secret_names.len()
            && prerequisites.len() == names.len()
            && observed.as_ref() == Some(&names)
            && built.role_consumers.as_ref().is_none_or(|consumers| {
                built.channel_template.get().trim() == "null"
                    && consumers
                        .keys()
                        .map(String::as_str)
                        .collect::<BTreeSet<_>>()
                        == names
                    && consumers.values().all(|consumer| match consumer {
                        FrontendRoleSecretConsumer::Extension { consumer_id } => {
                            !consumer_id.is_empty() && consumer_id == &built.extension_id
                        }
                    })
            })
    }
}

/// Status is host-reported evidence. In particular, `Applied` neither proves
/// provider readiness nor independently verifies the host's running process.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrontendChangeStatus {
    Submitted {
        request_id: FrontendChangeDigest,
        submitted_at: String,
    },
    Applied {
        request_id: FrontendChangeDigest,
        submitted_at: String,
        activation_id: String,
    },
    Superseded {
        request_id: FrontendChangeDigest,
        submitted_at: String,
        #[serde(deserialize_with = "required_nullable_text")]
        activation_id: Option<String>,
    },
    Rejected {
        request_id: FrontendChangeDigest,
        submitted_at: String,
        reason: String,
        decided_at: String,
    },
}

impl FrontendChangeStatus {
    pub fn request_id(&self) -> &FrontendChangeDigest {
        match self {
            Self::Submitted { request_id, .. }
            | Self::Applied { request_id, .. }
            | Self::Superseded { request_id, .. }
            | Self::Rejected { request_id, .. } => request_id,
        }
    }
}

/// A retained host draft with no submitted request at the instant of the GET.
/// Its change is unbound readback, even if this client prepared it earlier.
/// `base_current` is the host's observation, not approval or readiness.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendPreparedDraft {
    pub state: FrontendPreparedState,
    pub change: FrontendPreparedChange,
    pub base_current: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendPreparedState {
    Prepared,
}

/// Exact stored owner data. Deserialization never recovers a preparation's
/// captured-client binding; the ordinary submit methods refuse this object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrontendPreparedChange {
    Channel(FrontendChange),
    NewExtension(FrontendNewExtensionChange),
}

impl FrontendPreparedChange {
    pub fn change_digest(&self) -> &FrontendChangeDigest {
        match self {
            Self::Channel(change) => change.change_digest(),
            Self::NewExtension(change) => change.change_digest(),
        }
    }
}

impl Serialize for FrontendPreparedChange {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Channel(change) => change.serialize(serializer),
            Self::NewExtension(change) => change.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for FrontendPreparedChange {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Decode directly from owner bytes: serde's tagged/untagged buffering
        // cannot preserve RawValue. A present kind must never fall back.
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        serde_json::from_str::<UniqueJson>(raw.get())
            .map_err(|_| serde::de::Error::custom("invalid prepared change JSON"))?;
        let fields: BTreeMap<String, Box<RawValue>> = serde_json::from_str(raw.get())
            .map_err(|_| serde::de::Error::custom("unsupported prepared change shape"))?;
        if fields.contains_key("kind") {
            serde_json::from_str(raw.get()).map(Self::NewExtension)
        } else {
            serde_json::from_str(raw.get()).map(Self::Channel)
        }
        .map_err(|_| serde::de::Error::custom("unsupported prepared change shape"))
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "outcome", content = "result", rename_all = "snake_case")]
pub enum FrontendChangeRead {
    Status(FrontendChangeStatus),
    NewExtensionStatus(FrontendNewExtensionStatus),
    /// A draft observation cannot settle an uncertain POST or permit replay.
    Prepared(FrontendPreparedDraft),
    /// A point-in-time absence only. After an uncertain POST this remains
    /// unresolved: the POST may still commit. Do not infer rollback or replay.
    NotFound,
    Rejected(FrontendChangeRejection),
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FrontendChangeRejection {
    pub http_status: u16,
    pub error: FrontendChangeRejectionKind,
    pub problems: Vec<FrontendProblem>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendChangeRejectionKind {
    InvalidRequest,
    DiffMismatch,
    SummaryMismatch,
    DigestMismatch,
    CandidateInvalid,
    StaleBase,
    Conflict,
    Unavailable,
    InternalError,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FrontendRequestRejection {
    pub http_status: u16,
    pub error: FrontendRejectionKind,
    pub problems: Vec<FrontendProblem>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendRejectionKind {
    InvalidRequest,
    InternalError,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendCurrent {
    #[serde(deserialize_with = "required_nullable_text")]
    pub activation_id: Option<String>,
    #[serde(deserialize_with = "required_nullable_text")]
    pub policy_digest: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendCandidate {
    #[serde(deserialize_with = "required_nullable_text")]
    pub channel_row_digest: Option<String>,
    #[serde(deserialize_with = "required_nullable_text")]
    pub policy_digest: Option<String>,
    pub compile_depth: FrontendCompileDepth,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendCompileDepth {
    None,
    ChannelRow,
    ChannelBinding,
    PolicyBundle,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendPrerequisite {
    pub kind: FrontendPrerequisiteKind,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FrontendPrerequisiteKind {
    Secret,
    ExtensionWheel,
    Route,
}

/// Paired host promises static messages, never compiler exception text. Checking
/// that wire promise prevents a mismatched endpoint from returning raw errors.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrontendProblem {
    pub code: String,
    pub path: String,
    pub message: String,
}

impl FrontendProblem {
    fn has_host_shape(&self) -> bool {
        let message = match self.code.as_str() {
            "unknown_field" => "The request contains a field this action does not accept.",
            "missing_field" => "A required request field is absent.",
            "invalid_type" => "A request field has the wrong type.",
            "invalid_value" => "A request field has a value outside its allowed form.",
            "invalid_json" => "The request body is not a JSON object.",
            "body_too_large" => "The request body exceeds the size limit.",
            "unknown_extension" => "No extension with this id is declared in the current ledger.",
            "extension_disabled" => "The extension is declared but disabled in the current ledger.",
            "extension_wheel_missing" => {
                "The extension's pinned wheel is not present in the release."
            }
            "channel_not_declared" => "The extension declares no channel with this id.",
            "channel_row_shape" => "The candidate channel row has unknown or missing fields.",
            "ingress_config_missing" => {
                "The selected ingress has no configuration block in the extension's channel row."
            }
            "channel_row_invalid" => "The candidate channel row fails the ledger channel contract.",
            "duplicate_entry" => "A list field contains duplicate entries.",
            "channel_binding_invalid" => {
                "The candidate channel row does not join the extension's channel declaration."
            }
            "webhook_schema_unresolved" => "The webhook registration schema does not resolve.",
            "webhook_registration_invalid" => "The webhook registration fails its schema.",
            "webhook_route_missing" => {
                "The webhook ingress names a route the extension does not declare."
            }
            "secret_binding_absent" => "A secret the channel needs has no ledger secret binding.",
            "secret_binding_out_of_scope" => "A secret binding belongs to a different consumer.",
            "policy_compile_failed" => "The candidate policy does not compile.",
            "surfaces_lock_stale" => {
                "The candidate changes the desired extension surfaces; the host's surfaces lock must be regenerated and reviewed first."
            }
            "baseline_policy_compile_failed" => {
                "The current policy inputs do not compile in isolation, so the candidate cannot be judged at full depth."
            }
            "current_inputs_invalid" => {
                "The current ledger or compiled family contacts cannot be read."
            }
            "snapshot_changed" => {
                "The current activation, policy or ledger changed during validation."
            }
            "digest_mismatch" => "The change digest does not match the submitted change.",
            "diff_mismatch" => {
                "The change's unified diff is not the rendering of its ledger patch."
            }
            "summary_mismatch" => {
                "The change's unified summary is not the rendering of its proposal."
            }
            "source_not_allowed" => {
                "The source repository is not on the host's extension source allowlist."
            }
            "extension_sources_invalid" => "The host's extension source allowlist cannot be read.",
            "package_unreadable" => {
                "The package at the requested path could not be read from the source commit."
            }
            "package_path_mismatch" => {
                "The source commit's build recipe does not build the package at the requested path."
            }
            "package_identity_mismatch" => {
                "The package at the requested path is a different distribution or version."
            }
            "roles_unsupported" => "This host has no role configuration for this extension.",
            "roles_required" => "This extension is configured by a role selection, not a channel.",
            "role_unknown" => "The role selection names a role this extension does not define.",
            "role_mode_incompatible" => {
                "The daemon voice mode is not the one the selected roles require."
            }
            "role_contract_missing" => {
                "The required facades are not exactly the selected roles' host contract."
            }
            "voice_map_invalid" => "The voice map does not fit the selected roles.",
            "roles_incompatible" => "The role owner refuses this role configuration.",
            "not_a_paired_host_extension" => {
                "The source declares no extension entry point for this host."
            }
            "metadata_invalid" => {
                "The source's package metadata is not static, valid extension metadata for this host."
            }
            "tag_unresolved" => {
                "The tag could not be resolved to a commit in the source repository."
            }
            "tag_commit_mismatch" => {
                "The tag resolves to a different commit than the one requested."
            }
            "distribution_declared" => "An extension with this distribution is already declared.",
            "extension_id_declared" => {
                "The distribution name collides with a declared extension id."
            }
            "stale_base" => "The change was prepared against a snapshot that is no longer current.",
            "change_not_reproducible" => {
                "The submitted change is not the change the host derives for this candidate from the current snapshot."
            }
            "internal_error" => "Validation stopped on an unexpected error.",
            _ => return false,
        };
        self.message == message
            && self.path.len() <= 160
            && (self.path == "$"
                || self
                    .path
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.[]".contains(&byte)))
    }
}

// Use the standard cookie parser/store for expiry, domain and path rules.
// Add an exact-origin boundary (cookies alone do not distinguish ports), a
// small header bound, and sensitive request headers. No cookie is exported.
struct OriginCookies {
    origin: String,
    jar: Jar,
}

impl CookieStore for OriginCookies {
    fn set_cookies(&self, headers: &mut dyn Iterator<Item = &HeaderValue>, url: &Url) {
        let headers: Vec<_> = headers.take(MAX_COOKIE_HEADERS + 1).collect();
        if url.origin().ascii_serialization() == self.origin
            && headers.len() <= MAX_COOKIE_HEADERS
            && headers
                .iter()
                .all(|header| header.as_bytes().len() <= MAX_COOKIE_BYTES)
        {
            self.jar.set_cookies(&mut headers.into_iter(), url);
        }
    }

    fn cookies(&self, url: &Url) -> Option<HeaderValue> {
        if url.origin().ascii_serialization() != self.origin {
            return None;
        }
        self.jar.cookies(url).map(|mut header| {
            header.set_sensitive(true);
            header
        })
    }
}

/// One origin, one private in-memory cookie store, and serialized presenter
/// calls. It deliberately cannot be cloned or constructed from another client.
pub struct PairedHostClient {
    origin: Url,
    origin_header: HeaderValue,
    http: Client,
    cookies: Arc<OriginCookies>,
    presenter_credential: Option<(HeaderName, HeaderValue)>,
    confirmed_device: Option<ConfirmedDevice>,
    // The ledger file the host declared in its latest inventory read.
    declared_ledger_path: Option<String>,
    // Correlates immutable preparations with this exact client. This is not
    // authorization: the paired host rechecks the live credentials on every call.
    change_binding: Arc<()>,
}

impl fmt::Debug for PairedHostClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairedHostClient([captured origin; private cookies])")
    }
}

impl PairedHostClient {
    /// Host/presenter composition must obtain this origin directly from the
    /// human, not from tool arguments. HTTP is supported for the paired host's trusted
    /// LAN profile; it does not provide TLS confidentiality. Eligibility and
    /// any HTTPS/network rollout are the paired host owner's decisions.
    pub fn new(human_selected_origin: &str) -> Result<Self, PairedHostError> {
        Self::with_credential(human_selected_origin, None)
    }

    /// Private host bootstrap only. No cookie store is attached to this client;
    /// the paired host owns credential expiry, live-device checks and retirement.
    /// The host names the request header that carries its credential.
    pub(crate) fn for_presenter(
        origin: &str,
        header: &str,
        value: &str,
    ) -> Result<Self, PairedHostError> {
        let name = presenter_credential_header(header)?;
        let mut credential = HeaderValue::from_str(value)
            .map_err(|_| PairedHostError::InvalidPresenterCredential)?;
        if value.is_empty() || value.chars().any(char::is_whitespace) {
            return Err(PairedHostError::InvalidPresenterCredential);
        }
        credential.set_sensitive(true);
        let client = Self::with_credential(origin, Some((name, credential)))?;
        if client.selected_origin() != origin {
            return Err(PairedHostError::InvalidOrigin);
        }
        Ok(client)
    }

    fn with_credential(
        human_selected_origin: &str,
        presenter_credential: Option<(HeaderName, HeaderValue)>,
    ) -> Result<Self, PairedHostError> {
        let origin =
            Url::parse(human_selected_origin).map_err(|_| PairedHostError::InvalidOrigin)?;
        // Url normalizes dot segments and backslashes. Refuse an input path
        // before that normalization can turn a non-origin input into '/'.
        let authority = human_selected_origin
            .split_once("://")
            .map(|(_, rest)| rest)
            .ok_or(PairedHostError::InvalidOrigin)?;
        if !matches!(origin.scheme(), "http" | "https")
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || authority.contains(['@', '\\'])
            || authority
                .find('/')
                .is_some_and(|index| &authority[index..] != "/")
            || human_selected_origin.len() > 2048
            || human_selected_origin
                .chars()
                .any(|ch| ch.is_control() || ch.is_whitespace())
        {
            return Err(PairedHostError::InvalidOrigin);
        }
        let serialized_origin = origin.origin().ascii_serialization();
        let origin_header = HeaderValue::from_str(&serialized_origin)
            .map_err(|_| PairedHostError::InvalidOrigin)?;
        let cookies = Arc::new(OriginCookies {
            origin: serialized_origin,
            jar: Jar::default(),
        });
        let builder = Client::builder();
        let builder = if presenter_credential.is_none() {
            builder.cookie_provider(cookies.clone())
        } else {
            builder
        };
        let http = builder
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .referer(false)
            .no_gzip()
            .no_brotli()
            .no_zstd()
            .no_deflate()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| PairedHostError::ClientUnavailable)?;
        Ok(Self {
            origin,
            origin_header,
            http,
            cookies,
            presenter_credential,
            confirmed_device: None,
            declared_ledger_path: None,
            change_binding: Arc::new(()),
        })
    }

    fn authorize(
        &self,
        operation: PairedHostOperation,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, PairedHostError> {
        let Some((header, credential)) = &self.presenter_credential else {
            return Ok(request);
        };
        if !matches!(
            operation,
            PairedHostOperation::FrontendInventory
                | PairedHostOperation::FrontendSourceTags
                | PairedHostOperation::FrontendSourceInspect
                | PairedHostOperation::PrepareCandidate
                | PairedHostOperation::FrontendChangeStatus
        ) {
            return Err(PairedHostError::PresenterOperationUnavailable);
        }
        Ok(request.header(header.clone(), credential.clone()))
    }

    /// The nonsecret, canonical human-selected origin for protected review.
    pub fn selected_origin(&self) -> &str {
        self.origin_header
            .to_str()
            .expect("validated origin header")
    }

    pub(crate) fn canonical_origin(&self) -> String {
        self.origin.origin().ascii_serialization()
    }

    pub(crate) fn restore_device(
        &mut self,
        record: ConfirmedDevice,
    ) -> Result<(), DeviceStoreError> {
        let header = record.header(&self.canonical_origin(), crate::paired_host_device::now()?)?;
        self.cookies.jar.add_cookie_str(&header, &self.origin);
        Ok(())
    }

    pub(crate) fn take_confirmed_device(&mut self) -> Option<ConfirmedDevice> {
        self.confirmed_device.take()
    }

    /// The paired host sends a code to its owner. This is a real host-side effect,
    /// intended for an explicit presenter action, never an automatic probe.
    pub async fn request_pairing(&mut self, label: &str) -> Result<(), PairedHostError> {
        if label.trim().is_empty()
            || label.chars().count() > 60
            || label.chars().any(char::is_control)
        {
            return Err(PairedHostError::InvalidPairingLabel);
        }
        self.pair(
            PairedHostOperation::RequestPairing,
            &serde_json::json!({"label": label}),
        )
        .await
    }

    /// The presenter consumes human input directly into this call, without
    /// placing it in a native session, model tool argument, log or transcript.
    /// Success acknowledges this host response, not durable local credentials.
    pub async fn confirm_pairing(&mut self, code: PairingCode) -> Result<(), PairedHostError> {
        self.pair(
            PairedHostOperation::ConfirmPairing,
            &serde_json::json!({"code": code.0}),
        )
        .await
    }

    /// Consume a code already issued on the host terminal. This neither asks
    /// Telegram for a code nor enables the paired host's feature flag. The same
    /// captured origin, scoped cookie and optional OS custody owner apply.
    pub async fn confirm_terminal_pairing(
        &mut self,
        code: TerminalPairingCode,
    ) -> Result<(), PairedHostError> {
        self.pair(
            PairedHostOperation::ConfirmTerminalPairing,
            &serde_json::json!({"code": code.0}),
        )
        .await
    }

    /// Read the host's bounded tag observations in its own order. Repository
    /// input must already be canonical; the paired host owns allowlisting and access.
    pub async fn frontend_source_tags(
        &mut self,
        repo: &str,
    ) -> Result<FrontendSourceRead<FrontendSourceTags>, PairedHostError> {
        validate_source_repo(repo)?;
        let operation = PairedHostOperation::FrontendSourceTags;
        let (status, bytes) = self.source_read(operation, repo, None, None).await?;
        if status != 200 {
            return source_rejection(operation, status, &bytes, None)
                .map(FrontendSourceRead::Rejected);
        }
        let observed: FrontendSourceTags = decode(&bytes, operation)?;
        let unique: BTreeSet<_> = observed.tags.iter().map(|entry| &entry.tag).collect();
        if observed.repo != repo
            || observed.tags.is_empty()
            || observed.tags.len() > 500
            || unique.len() != observed.tags.len()
            || observed
                .tags
                .iter()
                .any(|entry| !valid_source_tag(&entry.tag) || !lower_hex(&entry.commit, 40))
        {
            return Err(unconfirmed(
                operation,
                UnconfirmedReason::UnexpectedResponse,
            ));
        }
        Ok(FrontendSourceRead::Observed(observed))
    }

    /// Ask the paired host to inspect static metadata at its resolved tag. No source
    /// code, metadata compiler, Git process or credential fallback runs here.
    pub async fn inspect_frontend_source(
        &mut self,
        repo: &str,
        tag: &str,
    ) -> Result<FrontendSourceRead<FrontendSourceInspection>, PairedHostError> {
        self.inspect_frontend_package(repo, tag, None).await
    }

    /// Select an exact package path supplied by evidence, never scan or fall
    /// back to a different package. Omission retains the root inspection API.
    pub async fn inspect_frontend_package(
        &mut self,
        repo: &str,
        tag: &str,
        package_path: Option<&FrontendPackagePath>,
    ) -> Result<FrontendSourceRead<FrontendSourceInspection>, PairedHostError> {
        validate_source_repo(repo)?;
        if !valid_source_tag(tag) {
            return Err(PairedHostError::InvalidSourceTag);
        }
        let operation = PairedHostOperation::FrontendSourceInspect;
        let (status, bytes) = self
            .source_read(operation, repo, Some(tag), package_path)
            .await?;
        if status != 200 {
            return source_rejection(operation, status, &bytes, package_path)
                .map(FrontendSourceRead::Rejected);
        }
        let observed: FrontendSourceInspection = decode(&bytes, operation)?;
        let draft = &observed.prepare.new_extension;
        if observed.repo != repo
            || observed.tag != tag
            || !lower_hex(&observed.commit, 40)
            || observed.distribution.is_empty()
            || observed.version.is_empty()
            || !valid_entry_point_group(&observed.entry_point.group)
            || observed.entry_point.name.is_empty()
            || observed.entry_point.object.is_empty()
            || !observed.has_configuration_shape()
            || observed.package_path.as_ref() != package_path
            || draft.source.package_path != observed.package_path
            || draft.distribution != observed.distribution
            || draft.version != observed.version
            || draft.source.repo != observed.repo
            || draft.source.tag != observed.tag
            || draft.source.commit != observed.commit
        {
            return Err(unconfirmed(
                operation,
                UnconfirmedReason::UnexpectedResponse,
            ));
        }
        Ok(FrontendSourceRead::Observed(observed))
    }

    async fn source_read(
        &self,
        operation: PairedHostOperation,
        repo: &str,
        tag: Option<&str>,
        package_path: Option<&FrontendPackagePath>,
    ) -> Result<(u16, Vec<u8>), PairedHostError> {
        let mut url = self.origin.clone();
        url.set_path(match (operation, tag) {
            (PairedHostOperation::FrontendSourceTags, None) => FRONTEND_SOURCE_TAGS,
            (PairedHostOperation::FrontendSourceInspect, Some(_)) => FRONTEND_SOURCE_INSPECT,
            _ => unreachable!("only the two fixed source reads call this transport"),
        });
        url.query_pairs_mut().append_pair("repo", repo);
        if let Some(tag) = tag {
            url.query_pairs_mut().append_pair("tag", tag);
        }
        if let Some(path) = package_path {
            url.query_pairs_mut()
                .append_pair("package_path", path.as_str());
        }
        let mut request = self
            .http
            .get(url)
            .header(ORIGIN, self.origin_header.clone())
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store");
        if operation == PairedHostOperation::FrontendSourceInspect {
            request = request.timeout(SOURCE_INSPECT_TIMEOUT);
        }
        let response = self
            .authorize(operation, request)?
            .send()
            .await
            .map_err(|error| transport_error(operation, &error))?;
        let response = checked_response(response, operation)?;
        require_no_store(&response, operation)?;
        let status = response.status().as_u16();
        let bytes = read_body_with_limit(response, operation, MAX_SOURCE_BODY_BYTES).await?;
        if status == 403 {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Denied {
                error: String,
                code: String,
            }
            if let Ok(denied) = decode::<Denied>(&bytes, operation)
                && denied.error == "forbidden"
                && denied.code == "console_owner_required"
            {
                return Err(PairedHostError::Refused {
                    operation,
                    reason: PairedHostRefusal::ConsoleOwnerRequired,
                });
            }
        }
        Ok((status, bytes))
    }

    /// Read a fresh host-owned snapshot. No cache, retry, provider-group
    /// discovery or readiness inference is performed by this client.
    pub async fn frontend_inventory(&mut self) -> Result<FrontendInventoryRead, PairedHostError> {
        let operation = PairedHostOperation::FrontendInventory;
        let mut url = self.origin.clone();
        url.set_path(FRONTEND_INVENTORY);
        let request = self
            .http
            .get(url)
            .header(ORIGIN, self.origin_header.clone())
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store");
        let response = self
            .authorize(operation, request)?
            .send()
            .await
            .map_err(|error| transport_error(operation, &error))?;
        let response = checked_response(response, operation)?;
        let no_store = response
            .headers()
            .get_all(CACHE_CONTROL)
            .iter()
            .filter_map(|header| header.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|directive| directive.trim().eq_ignore_ascii_case("no-store"));
        if !no_store {
            return Err(unconfirmed(
                operation,
                UnconfirmedReason::UnexpectedResponse,
            ));
        }
        let status = response.status().as_u16();
        let bytes = read_body(response, operation).await?;
        if status == 200 {
            let inventory: FrontendInventory = decode(&bytes, operation)?;
            if !valid_ledger_path(&inventory.ledger_path)
                || inventory
                    .extensions
                    .iter()
                    .flat_map(|extension| &extension.channels)
                    .any(|channel| !channel.senders.informational)
            {
                return Err(unconfirmed(
                    operation,
                    UnconfirmedReason::UnexpectedResponse,
                ));
            }
            self.declared_ledger_path = Some(inventory.ledger_path.clone());
            return Ok(FrontendInventoryRead::Inventory(inventory));
        }
        let expected = match status {
            409 => Some((FrontendInventoryRejectionKind::Conflict, "snapshot_changed")),
            503 => Some((
                FrontendInventoryRejectionKind::Unavailable,
                "current_inputs_invalid",
            )),
            500 => Some((
                FrontendInventoryRejectionKind::InternalError,
                "internal_error",
            )),
            _ => None,
        };
        if let Some((kind, code)) = expected {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Rejection {
                error: FrontendInventoryRejectionKind,
                problems: Vec<FrontendProblem>,
            }
            let rejection: Rejection = decode(&bytes, operation)?;
            if rejection.error != kind
                || rejection.problems.len() != 1
                || rejection.problems[0].code != code
                || !rejection.problems[0].has_host_shape()
            {
                return Err(unconfirmed(
                    operation,
                    UnconfirmedReason::UnexpectedResponse,
                ));
            }
            return Ok(FrontendInventoryRead::Rejected(
                FrontendInventoryRejection {
                    http_status: status,
                    error: kind,
                    problems: rejection.problems,
                },
            ));
        }
        Err(host_refusal(operation, status, &bytes))
    }

    pub async fn prepare_candidate(
        &mut self,
        candidate: &FrontendCandidateRequest,
    ) -> Result<FrontendPreparation, PairedHostError> {
        let operation = PairedHostOperation::PrepareCandidate;
        let response = self.post(operation, candidate).await?;
        let status = response.status().as_u16();
        let bytes = read_body(response, operation).await?;
        if status == 200 {
            let mut receipt: FrontendCandidateReceipt = decode(&bytes, operation)?;
            if !receipt.compiled_is_not_ready
                || receipt.valid != receipt.problems.is_empty()
                || !receipt.problems.iter().all(FrontendProblem::has_host_shape)
                || !valid_review_url(
                    receipt.review_url.as_deref(),
                    receipt.valid,
                    receipt.change.as_ref().map(FrontendChange::change_digest),
                )
            {
                return Err(unconfirmed(
                    operation,
                    UnconfirmedReason::UnexpectedResponse,
                ));
            }
            if let Some(change) = &mut receipt.change {
                let patch = change.ledger_patch();
                // Correlate the selected inputs only; the rest of the owner
                // row and its extension-defined registration stay opaque.
                #[derive(Deserialize)]
                struct Selection {
                    id: String,
                    ingress: FrontendIngress,
                    approved_groups: Vec<String>,
                }
                let selected: Selection = decode(patch.after.get().as_bytes(), operation)?;
                if !receipt.valid
                    || receipt.candidate.compile_depth != FrontendCompileDepth::PolicyBundle
                    || change.base().activation_id != receipt.current.activation_id
                    || change.base().policy_digest != receipt.current.policy_digest
                    || self
                        .declared_ledger_path
                        .as_ref()
                        .is_some_and(|declared| *declared != patch.path)
                    || patch.extension_id != candidate.extension_id
                    || candidate
                        .channel_id
                        .as_ref()
                        .is_some_and(|id| *id != patch.channel_id)
                    || selected.id != patch.channel_id
                    || selected.ingress != candidate.ingress
                    || selected.approved_groups != candidate.participants.approved_groups
                {
                    return Err(unconfirmed(
                        operation,
                        UnconfirmedReason::UnexpectedResponse,
                    ));
                }
                change.client_binding = Some(self.change_binding.clone());
            }
            return Ok(FrontendPreparation::Candidate(Box::new(receipt)));
        }
        if matches!(status, 400 | 413 | 500) {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Rejection {
                error: FrontendRejectionKind,
                problems: Vec<FrontendProblem>,
            }
            let rejected: Rejection = decode(&bytes, operation)?;
            let expected = if status == 500 {
                FrontendRejectionKind::InternalError
            } else {
                FrontendRejectionKind::InvalidRequest
            };
            if rejected.error != expected
                || rejected.problems.is_empty()
                || !rejected
                    .problems
                    .iter()
                    .all(FrontendProblem::has_host_shape)
            {
                return Err(unconfirmed(
                    operation,
                    UnconfirmedReason::UnexpectedResponse,
                ));
            }
            return Ok(FrontendPreparation::Rejected(FrontendRequestRejection {
                http_status: status,
                error: rejected.error,
                problems: rejected.problems,
            }));
        }
        Err(host_refusal(operation, status, &bytes))
    }

    /// Ask the same host to judge a new package proposal. This performs no
    /// local fetch, build, normalization or compilation. `valid` means only
    /// eligible for the host operator's build/review workflow.
    pub async fn prepare_new_extension(
        &mut self,
        proposal: &FrontendNewExtensionRequest,
    ) -> Result<FrontendNewExtensionPreparation, PairedHostError> {
        #[derive(Serialize)]
        struct Request<'a> {
            new_extension: &'a FrontendNewExtensionRequest,
        }
        let operation = PairedHostOperation::PrepareCandidate;
        let response = self
            .post(
                operation,
                &Request {
                    new_extension: proposal,
                },
            )
            .await?;
        require_no_store(&response, operation)?;
        let status = response.status().as_u16();
        let bytes = read_body(response, operation).await?;
        if status == 200 {
            let mut receipt: FrontendNewExtensionCandidateReceipt = decode(&bytes, operation)?;
            if receipt.valid != receipt.problems.is_empty()
                || receipt.valid != receipt.change.is_some()
                || !receipt.problems.iter().all(FrontendProblem::has_host_shape)
                || !valid_review_url(
                    receipt.review_url.as_deref(),
                    receipt.valid,
                    receipt
                        .change
                        .as_ref()
                        .map(FrontendNewExtensionChange::change_digest),
                )
            {
                return Err(unconfirmed(
                    operation,
                    UnconfirmedReason::UnexpectedResponse,
                ));
            }
            if let Some(change) = &mut receipt.change {
                // The owner normalizes inputs. Refuse a different proposal
                // rather than implementing PEP 440 or selecting it silently.
                if change.proposal() != proposal {
                    return Err(PairedHostError::NewExtensionProposalMismatch);
                }
                change.client_binding = Some(self.change_binding.clone());
            }
            return Ok(FrontendNewExtensionPreparation::Candidate(Box::new(
                receipt,
            )));
        }
        if matches!(status, 400 | 413 | 500) {
            return prepare_rejection(status, &bytes)
                .map(FrontendNewExtensionPreparation::Rejected);
        }
        Err(host_refusal(operation, status, &bytes))
    }

    /// Submit the exact object this client received from preparation, once per
    /// explicit invocation. The caller must obtain the human's approval of the
    /// captured origin and exact diff before invoking this effect. A successful
    /// response records a request for the operator, not installation/activation.
    pub async fn submit_frontend_change(
        &mut self,
        change: &FrontendChange,
    ) -> Result<FrontendSubmission, PairedHostError> {
        let operation = PairedHostOperation::SubmitFrontendChange;
        let (status, bytes) = self
            .submit_captured_change(
                change.change_digest(),
                &change.raw,
                change.client_binding.as_ref(),
            )
            .await?;
        if matches!(status, 200 | 201) {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Recorded {
                request_id: FrontendChangeDigest,
                state: FrontendSubmittedState,
                submitted_at: String,
                change: FrontendChange,
                submitted_by: FrontendSubmitter,
            }
            let record: Recorded = decode(&bytes, operation)?;
            if record.request_id != *change.change_digest()
                || !same_owner_json(&record.change.raw, &change.raw)
            {
                return Err(unconfirmed(
                    operation,
                    UnconfirmedReason::UnexpectedResponse,
                ));
            }
            return Ok(FrontendSubmission::Recorded(Box::new(
                FrontendSubmissionReceipt {
                    http_status: status,
                    request_id: record.request_id,
                    state: record.state,
                    submitted_at: record.submitted_at,
                    change: record.change,
                    submitted_by: record.submitted_by,
                },
            )));
        }
        if matches!(status, 400 | 409 | 413) {
            return change_rejection(operation, status, &bytes).map(FrontendSubmission::Rejected);
        }
        Err(host_refusal(operation, status, &bytes))
    }

    /// Submit only after explicit human approval of the captured origin and
    /// exact owner summary. This records a request, not a built/installed
    /// extension. The invocation owner supplies one-use approval and keeps an
    /// uncertain POST closed to further effects; this client never retries.
    pub async fn submit_frontend_new_extension_change(
        &mut self,
        change: &FrontendNewExtensionChange,
    ) -> Result<FrontendNewExtensionSubmission, PairedHostError> {
        let operation = PairedHostOperation::SubmitFrontendChange;
        let (status, bytes) = self
            .submit_captured_change(
                change.change_digest(),
                &change.raw,
                change.client_binding.as_ref(),
            )
            .await?;
        if matches!(status, 200 | 201) {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Recorded {
                request_id: FrontendChangeDigest,
                state: FrontendSubmittedState,
                submitted_at: String,
                change: FrontendNewExtensionChange,
                submitted_by: FrontendSubmitter,
                #[serde(default, deserialize_with = "reported_attempt")]
                attempt: Option<FrontendAttempt>,
            }
            let record: Recorded = decode(&bytes, operation)?;
            if record.request_id != *change.change_digest()
                || !same_owner_json(&record.change.raw, &change.raw)
            {
                return Err(unconfirmed(
                    operation,
                    UnconfirmedReason::UnexpectedResponse,
                ));
            }
            return Ok(FrontendNewExtensionSubmission::Recorded(Box::new(
                FrontendNewExtensionSubmissionReceipt {
                    http_status: status,
                    request_id: record.request_id,
                    state: record.state,
                    submitted_at: record.submitted_at,
                    change: record.change,
                    submitted_by: record.submitted_by,
                    attempt: record.attempt,
                },
            )));
        }
        if matches!(status, 400 | 409 | 413) {
            return change_rejection(operation, status, &bytes)
                .map(FrontendNewExtensionSubmission::Rejected);
        }
        Err(host_refusal(operation, status, &bytes))
    }

    async fn submit_captured_change(
        &self,
        digest: &FrontendChangeDigest,
        raw: &RawValue,
        binding: Option<&Arc<()>>,
    ) -> Result<(u16, Vec<u8>), PairedHostError> {
        if !binding.is_some_and(|binding| Arc::ptr_eq(binding, &self.change_binding)) {
            return Err(PairedHostError::UnboundFrontendChange);
        }
        #[derive(Serialize)]
        struct Submission<'a> {
            change_digest: &'a FrontendChangeDigest,
            change: &'a RawValue,
        }
        let operation = PairedHostOperation::SubmitFrontendChange;
        let response = self
            .post(
                operation,
                &Submission {
                    change_digest: digest,
                    change: raw,
                },
            )
            .await?;
        require_no_store(&response, operation)?;
        let status = response.status().as_u16();
        if status >= 500 {
            // Publication can precede a failing fsync or cleanup. A server
            // error cannot establish that the request had no effect.
            return Err(unconfirmed(operation, UnconfirmedReason::HostFailure));
        }
        Ok((status, read_body(response, operation).await?))
    }

    /// One explicit, fresh digest read. No POST replay, polling loop or local
    /// completion is inferred. `Prepared` and `NotFound` following an uncertain
    /// submission remain unresolved: an in-flight POST may commit after the GET.
    pub async fn frontend_change_status(
        &mut self,
        digest: &FrontendChangeDigest,
    ) -> Result<FrontendChangeRead, PairedHostError> {
        let operation = PairedHostOperation::FrontendChangeStatus;
        let mut url = self.origin.clone();
        url.set_path(&format!("{FRONTEND_CHANGES}/{}", digest.as_str()));
        let request = self
            .http
            .get(url)
            .header(ORIGIN, self.origin_header.clone())
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store");
        let response = self
            .authorize(operation, request)?
            .send()
            .await
            .map_err(|error| transport_error(operation, &error))?;
        let response = checked_response(response, operation)?;
        require_no_store(&response, operation)?;
        let status = response.status().as_u16();
        let bytes = read_body(response, operation).await?;
        if status == 200 {
            // Inspect only the discriminator before the closed typed decode.
            // Presence (including null/unknown) must not fall back to legacy.
            let envelope: BTreeMap<String, Box<RawValue>> = decode(&bytes, operation)?;
            let state: Option<String> = envelope
                .get("state")
                .map(|raw| decode(raw.get().as_bytes(), operation))
                .transpose()?;
            if state.as_deref() == Some("prepared") {
                let observation: FrontendPreparedDraft = decode(&bytes, operation)?;
                let crossed_ledger = match &observation.change {
                    FrontendPreparedChange::Channel(change) => self
                        .declared_ledger_path
                        .as_ref()
                        .is_some_and(|declared| *declared != change.ledger_patch().path),
                    FrontendPreparedChange::NewExtension(_) => false,
                };
                if observation.change.change_digest() != digest || crossed_ledger {
                    return Err(unconfirmed(
                        operation,
                        UnconfirmedReason::UnexpectedResponse,
                    ));
                }
                return Ok(FrontendChangeRead::Prepared(observation));
            }
            if envelope.contains_key("kind") {
                let observation: FrontendNewExtensionStatus = decode(&bytes, operation)?;
                if observation.request_id() != digest || !observation.has_host_shape() {
                    return Err(unconfirmed(
                        operation,
                        UnconfirmedReason::UnexpectedResponse,
                    ));
                }
                return Ok(FrontendChangeRead::NewExtensionStatus(observation));
            }
            let observation: FrontendChangeStatus = decode(&bytes, operation)?;
            if observation.request_id() != digest {
                return Err(unconfirmed(
                    operation,
                    UnconfirmedReason::UnexpectedResponse,
                ));
            }
            return Ok(FrontendChangeRead::Status(observation));
        }
        if status == 404 {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Missing {
                state: MissingState,
            }
            #[derive(Deserialize)]
            enum MissingState {
                #[serde(rename = "not_found")]
                NotFound,
            }
            // A route/network 404 is not this digest-scoped observation.
            let Missing {
                state: MissingState::NotFound,
            } = decode(&bytes, operation)?;
            return Ok(FrontendChangeRead::NotFound);
        }
        if matches!(status, 400 | 409 | 500 | 503) {
            return change_rejection(operation, status, &bytes).map(FrontendChangeRead::Rejected);
        }
        Err(host_refusal(operation, status, &bytes))
    }

    /// Create one nonsecret entry intent. This can supersede an older pending
    /// intent. Authorization belongs to the caller and the paired host; no retry, secret
    /// entry, browser pairing or owner confirmation is performed here.
    pub async fn create_extension_secret_intent(
        &mut self,
        request: &ExtensionSecretIntentRequest,
    ) -> Result<ExtensionSecretIntentCreation, PairedHostError> {
        let operation = PairedHostOperation::CreateExtensionSecretIntent;
        let body = request.checked_body()?;
        let binding = match request {
            ExtensionSecretIntentRequest::Ledger { binding_name, .. }
            | ExtensionSecretIntentRequest::Candidate { binding_name, .. } => binding_name,
        };
        let (status, bytes) = self
            .secret_intent_request(operation, None, Some(body))
            .await?;
        if status != 201 {
            return secret_intent_refusal(operation, status, &bytes)
                .map(ExtensionSecretIntentCreation::Rejected);
        }
        let intent = decode_secret_intent(&bytes, operation)?;
        let matches_target = intent.target.binding_name == *binding
            && match request {
                ExtensionSecretIntentRequest::Ledger {
                    extension_id, base, ..
                } => {
                    intent.target.extension_id == *extension_id
                        && intent.target.activation_id == base.activation_id
                }
                ExtensionSecretIntentRequest::Candidate { .. } => true,
            };
        if intent.state != ExtensionSecretIntentState::AwaitingEntry || !matches_target {
            return Err(unconfirmed(
                operation,
                UnconfirmedReason::UnexpectedResponse,
            ));
        }
        Ok(ExtensionSecretIntentCreation::Created(Box::new(
            ExtensionSecretIntentCreated {
                request: request.clone(),
                intent,
            },
        )))
    }

    /// Explicit host readback. NotFound is an observation, not proof of no effect
    /// or a permission to recreate an intent whose creation reply was lost.
    pub async fn extension_secret_intent_status(
        &mut self,
        id: &ExtensionSecretIntentId,
    ) -> Result<ExtensionSecretIntentRead, PairedHostError> {
        let operation = PairedHostOperation::ExtensionSecretIntentStatus;
        let (status, bytes) = self
            .secret_intent_request(operation, Some(id), None)
            .await?;
        if status == 200 {
            let intent = decode_secret_intent(&bytes, operation)?;
            if intent.intent_id != *id {
                return Err(unconfirmed(
                    operation,
                    UnconfirmedReason::UnexpectedResponse,
                ));
            }
            return Ok(ExtensionSecretIntentRead::Status(intent));
        }
        let refusal = secret_intent_refusal(operation, status, &bytes)?;
        if refusal.refusal == ExtensionSecretIntentRefusal::NotFound {
            Ok(ExtensionSecretIntentRead::NotFound)
        } else {
            Ok(ExtensionSecretIntentRead::Rejected(refusal))
        }
    }

    /// Cancel one intent at the owner, potentially discarding a pending value.
    /// Closed/committed observations are never relabeled as successful rollback.
    pub async fn cancel_extension_secret_intent(
        &mut self,
        id: &ExtensionSecretIntentId,
    ) -> Result<ExtensionSecretIntentCancellation, PairedHostError> {
        let operation = PairedHostOperation::CancelExtensionSecretIntent;
        let (status, bytes) = self
            .secret_intent_request(operation, Some(id), Some(b"{}".to_vec()))
            .await?;
        if status == 200 {
            let intent = decode_secret_intent(&bytes, operation)?;
            if intent.intent_id != *id || intent.state != ExtensionSecretIntentState::Cancelled {
                return Err(unconfirmed(
                    operation,
                    UnconfirmedReason::UnexpectedResponse,
                ));
            }
            return Ok(ExtensionSecretIntentCancellation::Cancelled(intent));
        }
        secret_intent_refusal(operation, status, &bytes)
            .map(ExtensionSecretIntentCancellation::Rejected)
    }

    async fn secret_intent_request(
        &self,
        operation: PairedHostOperation,
        id: Option<&ExtensionSecretIntentId>,
        body: Option<Vec<u8>>,
    ) -> Result<(u16, Vec<u8>), PairedHostError> {
        let path = match (operation, id) {
            (PairedHostOperation::CreateExtensionSecretIntent, None) => SECRET_INTENTS.into(),
            (PairedHostOperation::ExtensionSecretIntentStatus, Some(id)) => {
                format!("{SECRET_INTENTS}/{}", id.as_str())
            }
            (PairedHostOperation::CancelExtensionSecretIntent, Some(id)) => {
                format!("{SECRET_INTENTS}/{}/cancel", id.as_str())
            }
            _ => unreachable!("only the three fixed intent methods call this transport"),
        };
        let mut url = self.origin.clone();
        url.set_path(&path);
        let request = if let Some(body) = body {
            self.http
                .post(url)
                .header(ORIGIN, self.origin_header.clone())
                .header(CONTENT_TYPE, "application/json")
                .body(body)
        } else {
            self.http.get(url)
        };
        let response = self
            .authorize(operation, request)?
            .header(ACCEPT, "application/json")
            .header(CACHE_CONTROL, "no-store")
            .send()
            .await
            .map_err(|error| transport_error(operation, &error))?;
        let response = checked_response(response, operation)?;
        // Failure may follow durable intent creation/cancellation. Never turn
        // any 5xx into a settled refusal or reconstruct state from its body.
        if response.status().is_server_error() {
            return Err(unconfirmed(operation, UnconfirmedReason::HostFailure));
        }
        require_no_store(&response, operation)?;
        let status = response.status().as_u16();
        Ok((status, read_body(response, operation).await?))
    }

    async fn pair<T: Serialize + ?Sized>(
        &mut self,
        operation: PairedHostOperation,
        body: &T,
    ) -> Result<(), PairedHostError> {
        // A failed or cancelled new confirmation must not retain an older
        // save candidate, even if reqwest has already updated its live jar.
        self.confirmed_device = None;
        let response = self.post(operation, body).await?;
        let status = response.status().as_u16();
        let (name, path) = match operation {
            PairedHostOperation::RequestPairing => ("paired_host_pair", "/console/pair"),
            _ => ("paired_host_device", "/"),
        };
        let cookie_present = response
            .cookies()
            .filter(|cookie| cookie.name() == name)
            .count()
            == 1
            && response.cookies().any(|cookie| {
                cookie.name() == name
                    && !cookie.value().is_empty()
                    && cookie.path() == Some(path)
                    && cookie.domain().is_none()
                    && cookie.http_only()
            });
        let confirmed_device = if matches!(
            operation,
            PairedHostOperation::ConfirmPairing | PairedHostOperation::ConfirmTerminalPairing
        ) && cookie_present
        {
            let received_at = crate::paired_host_device::now().ok();
            // Do not choose the first valid credential among conflicting
            // headers. The standard parser used by the live jar must see one
            // device cookie, and only that single header can be remembered.
            let mut headers = response
                .headers()
                .get_all(SET_COOKIE)
                .iter()
                .filter_map(|header| {
                    let text = header.to_str().ok()?;
                    let parsed = cookie::Cookie::parse(text).ok()?;
                    (parsed.name() == "paired_host_device").then_some(text)
                });
            match (headers.next(), headers.next(), received_at) {
                (Some(header), None, Some(at)) => {
                    ConfirmedDevice::from_cookie(&self.canonical_origin(), header, at)
                }
                _ => None,
            }
        } else {
            None
        };
        let bytes = read_body(response, operation).await?;
        if status != 200 {
            return Err(host_refusal(operation, status, &bytes));
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Paired {
            ok: bool,
        }
        let result: Paired = decode(&bytes, operation)?;
        if !result.ok || !cookie_present {
            return Err(unconfirmed(
                operation,
                UnconfirmedReason::UnexpectedResponse,
            ));
        }
        self.confirmed_device = confirmed_device;
        Ok(())
    }

    async fn post<T: Serialize + ?Sized>(
        &self,
        operation: PairedHostOperation,
        body: &T,
    ) -> Result<Response, PairedHostError> {
        let bytes = serde_json::to_vec(body).map_err(|_| PairedHostError::RequestTooLarge)?;
        if bytes.len() > MAX_BODY_BYTES {
            return Err(PairedHostError::RequestTooLarge);
        }
        let path = match operation {
            PairedHostOperation::RequestPairing => PAIR_REQUEST,
            PairedHostOperation::ConfirmPairing => PAIR_CONFIRM,
            PairedHostOperation::ConfirmTerminalPairing => PAIR_TERMINAL,
            PairedHostOperation::PrepareCandidate => FRONTEND_PREPARE,
            PairedHostOperation::SubmitFrontendChange => FRONTEND_CHANGES,
            PairedHostOperation::FrontendInventory
            | PairedHostOperation::FrontendChangeStatus
            | PairedHostOperation::FrontendSourceTags
            | PairedHostOperation::FrontendSourceInspect
            | PairedHostOperation::CreateExtensionSecretIntent
            | PairedHostOperation::ExtensionSecretIntentStatus
            | PairedHostOperation::CancelExtensionSecretIntent => {
                unreachable!("read and intent operations use their dedicated methods")
            }
        };
        let mut url = self.origin.clone();
        url.set_path(path);
        let request = self
            .http
            .post(url)
            .header(ORIGIN, self.origin_header.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .body(bytes);
        let response = self
            .authorize(operation, request)?
            .send()
            .await
            .map_err(|error| transport_error(operation, &error))?;
        checked_response(response, operation)
    }
}

/// The trusted host chooses the header that carries its presenter credential.
/// Only a lowercase `x-` extension header is accepted, so the credential can
/// never replace a standard header such as authorization, cookie or origin.
fn presenter_credential_header(name: &str) -> Result<HeaderName, PairedHostError> {
    let valid = (3..=64).contains(&name.len())
        && name.starts_with("x-")
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !valid {
        return Err(PairedHostError::InvalidPresenterCredential);
    }
    HeaderName::from_bytes(name.as_bytes()).map_err(|_| PairedHostError::InvalidPresenterCredential)
}

fn validate_source_repo(repo: &str) -> Result<(), PairedHostError> {
    let parsed = Url::parse(repo).map_err(|_| PairedHostError::InvalidSourceRepository)?;
    // A deliberately canonical input contract, not an implementation of the
    // owner's normalization or source-eligibility policy.
    if repo.len() > 2048
        || parsed.as_str() != repo
        || parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.path().ends_with(".git")
        || parsed
            .path()
            .split('/')
            .filter(|part| !part.is_empty())
            .count()
            < 2
        || repo.chars().any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(PairedHostError::InvalidSourceRepository);
    }
    Ok(())
}

/// The host names its own extension entry-point group; Connect checks only that
/// it is a bounded dotted name, never that it equals one host's group.
fn valid_entry_point_group(group: &str) -> bool {
    group.len() <= 128
        && group.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        })
}

/// The host's extension ledger file, by the host's own relative path. The host
/// derives and re-derives every change, so Connect checks only the shape.
fn valid_ledger_path(path: &str) -> bool {
    path.len() <= 128
        && path.ends_with(".toml")
        && path.split('/').count() <= 8
        && path.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
}

/// The host's build-recipe file at the repository root, by the host's own name.
fn valid_release_recipe_name(path: &str) -> bool {
    path.strip_suffix(".json").is_some_and(|stem| {
        (1..=64).contains(&stem.len())
            && (stem.as_bytes()[0].is_ascii_lowercase() || stem.as_bytes()[0].is_ascii_digit())
            && stem.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
            })
    })
}

fn valid_source_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 100
        && tag.as_bytes()[0].is_ascii_alphanumeric()
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
        && !tag.contains("..")
        && !tag.ends_with(".lock")
}

fn source_rejection(
    operation: PairedHostOperation,
    status: u16,
    bytes: &[u8],
    package_path: Option<&FrontendPackagePath>,
) -> Result<FrontendSourceRejection, PairedHostError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Rejection {
        error: FrontendSourceRejectionKind,
        problems: Vec<FrontendSourceProblem>,
    }
    let rejected: Rejection = decode(bytes, operation)?;
    let unexpected = || unconfirmed(operation, UnconfirmedReason::UnexpectedResponse);
    let [problem] = rejected.problems.as_slice() else {
        return Err(unexpected());
    };
    use FrontendSourceRejectionKind as Kind;
    let metadata_path = package_path.map_or_else(
        || "pyproject.toml".to_owned(),
        |path| format!("{}/pyproject.toml", path.as_str()),
    );
    let known = match (
        status,
        rejected.error,
        problem.code.as_str(),
        problem.path.as_str(),
    ) {
        (400, Kind::InvalidRequest, "invalid_value", "repo") => true,
        (400, Kind::InvalidRequest, "invalid_value", "tag")
        | (404, Kind::NotFound, "tag_unresolved", "tag") => {
            operation == PairedHostOperation::FrontendSourceInspect
        }
        (400, Kind::InvalidRequest, "invalid_value", "package_path")
        | (422, Kind::Unprocessable, "package_path_mismatch", "package_path") => {
            operation == PairedHostOperation::FrontendSourceInspect && package_path.is_some()
        }
        (403, Kind::Forbidden, "source_not_allowed", "repo") => true,
        (404, Kind::NotFound, "no_release_tags", "repo") => {
            operation == PairedHostOperation::FrontendSourceTags
        }
        (422, Kind::Unprocessable, "not_a_paired_host_extension", path) => {
            operation == PairedHostOperation::FrontendSourceInspect && path == metadata_path
        }
        (422, Kind::Unprocessable, "metadata_invalid", path) => {
            operation == PairedHostOperation::FrontendSourceInspect
                && (valid_release_recipe_name(path)
                    || [
                        "",
                        ":project",
                        ":project.dynamic",
                        ":project.version",
                        ":project.entry-points",
                        ":project.name",
                    ]
                    .iter()
                    .any(|suffix| path == format!("{metadata_path}{suffix}"))
                    || path
                        .strip_prefix(&format!("{metadata_path}:project.entry-points."))
                        .is_some_and(valid_entry_point_group))
        }
        (502, Kind::BadGateway, "repo_unreachable", "repo") => true,
        (503, Kind::Unavailable, "extension_sources_invalid" | "current_inputs_invalid", "") => {
            true
        }
        (500, Kind::InternalError, "internal_error", "") => true,
        _ => false,
    };
    let message_matches = match problem.code.as_str() {
        "repo_unreachable" => {
            problem.message
                == "The source repository could not be read with the host's credentials."
        }
        "package_path_mismatch" => {
            problem.message
                == "The source commit's build recipe does not build the package at the requested path."
        }
        "no_release_tags" => {
            problem.message == "The source repository has no tags a request can pin."
        }
        "not_a_paired_host_extension" => {
            problem.message == "The source declares no extension entry point for this host."
        }
        "metadata_invalid" => {
            problem.message
                == "The source's package metadata is not static, valid extension metadata for this host."
        }
        _ => FrontendProblem {
            code: problem.code.clone(),
            path: problem.path.clone(),
            message: problem.message.clone(),
        }
        .has_host_shape(),
    };
    if !known
        || !message_matches
        || (problem.reason.is_some() && problem.code != "repo_unreachable")
    {
        return Err(unexpected());
    }
    Ok(FrontendSourceRejection {
        http_status: status,
        error: rejected.error,
        problems: rejected.problems,
    })
}

fn secret_intent_operation(operation: PairedHostOperation) -> bool {
    matches!(
        operation,
        PairedHostOperation::CreateExtensionSecretIntent
            | PairedHostOperation::ExtensionSecretIntentStatus
            | PairedHostOperation::CancelExtensionSecretIntent
    )
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn decode_secret_intent(
    bytes: &[u8],
    operation: PairedHostOperation,
) -> Result<ExtensionSecretIntent, PairedHostError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        intent_id: ExtensionSecretIntentId,
        state: ExtensionSecretIntentState,
        target: ExtensionSecretIntentTarget,
        present: bool,
        requires_overwrite: bool,
        presenter: ExtensionSecretPresenter,
        entry_url: String,
        fingerprint: Option<String>,
        overwrite: Option<bool>,
        generation: Option<u64>,
        binding_status: Option<ExtensionSecretIntentBindingStatus>,
        reason: Option<ExtensionSecretIntentClosure>,
    }
    let wire: Wire = decode(bytes, operation)?;
    let fields: BTreeMap<String, Box<RawValue>> = decode(bytes, operation)?;
    let entered = wire
        .fingerprint
        .as_deref()
        .is_some_and(|value| lower_hex(value, 8))
        && wire.overwrite.is_some();
    let absent_entry = !fields.contains_key("fingerprint") && !fields.contains_key("overwrite");
    let committed = wire.state == ExtensionSecretIntentState::Committed;
    let generations_valid = if committed {
        wire.generation.is_some_and(|value| value > 0) && fields.contains_key("binding_status")
    } else {
        !fields.contains_key("generation") && !fields.contains_key("binding_status")
    };
    let no_reason = !fields.contains_key("reason");
    let state_valid = match wire.state {
        ExtensionSecretIntentState::AwaitingEntry => absent_entry && no_reason,
        ExtensionSecretIntentState::AwaitingOwner | ExtensionSecretIntentState::Committed => {
            entered && no_reason
        }
        ExtensionSecretIntentState::Cancelled | ExtensionSecretIntentState::Superseded => {
            (entered || absent_entry) && no_reason
        }
        ExtensionSecretIntentState::Rejected => {
            entered && wire.reason == Some(ExtensionSecretIntentClosure::OwnerRejected)
        }
        ExtensionSecretIntentState::StaleBase => {
            (entered || absent_entry)
                && wire
                    .reason
                    .is_some_and(|reason| reason != ExtensionSecretIntentClosure::OwnerRejected)
        }
    };
    let expected_path = format!("{SECRET_INTENTS}/{}/entry", wire.intent_id.as_str());
    if !state_valid
        || !generations_valid
        || wire.present != wire.requires_overwrite
        || wire.entry_url != expected_path
        || wire.target.extension_id.is_empty()
        || wire.target.binding_name.is_empty()
        || wire.target.consumer.is_empty()
    {
        return Err(unconfirmed(
            operation,
            UnconfirmedReason::UnexpectedResponse,
        ));
    }
    Ok(ExtensionSecretIntent {
        intent_id: wire.intent_id,
        state: wire.state,
        target: wire.target,
        present: wire.present,
        requires_overwrite: wire.requires_overwrite,
        presenter: wire.presenter,
        fingerprint: wire.fingerprint,
        overwrite: wire.overwrite,
        generation: wire.generation,
        binding_status: wire.binding_status,
        reason: wire.reason,
        entry_path: wire.entry_url,
    })
}

fn secret_intent_refusal(
    operation: PairedHostOperation,
    status: u16,
    bytes: &[u8],
) -> Result<ExtensionSecretIntentRejection, PairedHostError> {
    use ExtensionSecretIntentRefusal as Refusal;
    use PairedHostOperation as Operation;
    let refusal: Refusal = decode(bytes, operation)?;
    // Serde's internally tagged unit variants can ignore extra fields even
    // with deny_unknown_fields. Check this closed wire shape explicitly.
    let fields: BTreeMap<String, Box<RawValue>> = decode(bytes, operation)?;
    let expected: &[&str] = match &refusal {
        Refusal::InvalidRequest { .. } | Refusal::Forbidden { .. } => &["error", "code"],
        Refusal::StaleBase { .. } => &["error", "current"],
        Refusal::IntentClosed { .. } => &["error", "state"],
        Refusal::InvalidJson
        | Refusal::BodyTooLarge
        | Refusal::BindingNotFound
        | Refusal::ChangeRequestNotFound
        | Refusal::BindingNotVault
        | Refusal::NotNewExtension
        | Refusal::CandidateNotBuilt
        | Refusal::StaleCandidate
        | Refusal::ConsumerUnresolved
        | Refusal::NotFound
        | Refusal::ExpectedJson => &["error"],
    };
    if fields.len() != expected.len() || !expected.iter().all(|key| fields.contains_key(*key)) {
        return Err(unconfirmed(
            operation,
            UnconfirmedReason::UnexpectedResponse,
        ));
    }
    let known = match (operation, status, &refusal) {
        (
            Operation::CreateExtensionSecretIntent,
            400,
            Refusal::InvalidRequest { .. } | Refusal::InvalidJson,
        )
        | (Operation::CreateExtensionSecretIntent, 413, Refusal::BodyTooLarge)
        | (_, 403, Refusal::Forbidden { .. })
        | (
            Operation::CreateExtensionSecretIntent,
            404,
            Refusal::BindingNotFound | Refusal::ChangeRequestNotFound,
        )
        | (
            Operation::CreateExtensionSecretIntent,
            409,
            Refusal::StaleBase { .. }
            | Refusal::BindingNotVault
            | Refusal::NotNewExtension
            | Refusal::CandidateNotBuilt
            | Refusal::StaleCandidate
            | Refusal::ConsumerUnresolved,
        )
        | (
            Operation::ExtensionSecretIntentStatus | Operation::CancelExtensionSecretIntent,
            404,
            Refusal::NotFound,
        )
        | (
            Operation::CreateExtensionSecretIntent | Operation::CancelExtensionSecretIntent,
            415,
            Refusal::ExpectedJson,
        ) => true,
        (Operation::CancelExtensionSecretIntent, 409, Refusal::IntentClosed { state }) => {
            state.closed()
        }
        _ => false,
    };
    if !known {
        return Err(unconfirmed(
            operation,
            UnconfirmedReason::UnexpectedResponse,
        ));
    }
    Ok(ExtensionSecretIntentRejection {
        http_status: status,
        refusal,
    })
}

fn checked_response(
    response: Response,
    operation: PairedHostOperation,
) -> Result<Response, PairedHostError> {
    if response.status().is_redirection() {
        return Err(unconfirmed(operation, UnconfirmedReason::Redirect));
    }
    let refusal = match response.status().as_u16() {
        // An absent route and a currently ineligible network both use 404.
        404 if operation != PairedHostOperation::FrontendChangeStatus
            && !matches!(
                operation,
                PairedHostOperation::FrontendSourceTags
                    | PairedHostOperation::FrontendSourceInspect
            )
            && !secret_intent_operation(operation) =>
        {
            Some(PairedHostRefusal::EndpointUnavailable)
        }
        415 if !secret_intent_operation(operation) => Some(PairedHostRefusal::UnsupportedMediaType),
        _ => None,
    };
    if let Some(reason) = refusal {
        return Err(PairedHostError::Refused { operation, reason });
    }
    let cookies = response.headers().get_all(SET_COOKIE);
    if cookies.iter().count() > MAX_COOKIE_HEADERS
        || cookies
            .iter()
            .any(|header| header.as_bytes().len() > MAX_COOKIE_BYTES)
    {
        return Err(unconfirmed(operation, UnconfirmedReason::ResponseTooLarge));
    }
    Ok(response)
}

fn require_no_store(
    response: &Response,
    operation: PairedHostOperation,
) -> Result<(), PairedHostError> {
    let no_store = response
        .headers()
        .get_all(CACHE_CONTROL)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|directive| directive.trim().eq_ignore_ascii_case("no-store"));
    if no_store {
        Ok(())
    } else {
        Err(unconfirmed(
            operation,
            UnconfirmedReason::UnexpectedResponse,
        ))
    }
}

fn change_rejection(
    operation: PairedHostOperation,
    status: u16,
    bytes: &[u8],
) -> Result<FrontendChangeRejection, PairedHostError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Rejection {
        error: FrontendChangeRejectionKind,
        problems: Vec<FrontendProblem>,
    }
    let rejected: Rejection = decode(bytes, operation)?;
    use FrontendChangeRejectionKind as Kind;
    let expected_code = match (operation, status, rejected.error) {
        (_, 400, Kind::InvalidRequest) => None,
        (PairedHostOperation::SubmitFrontendChange, 413, Kind::InvalidRequest) => {
            Some("body_too_large")
        }
        (PairedHostOperation::SubmitFrontendChange, 400, Kind::DiffMismatch) => {
            Some("diff_mismatch")
        }
        (PairedHostOperation::SubmitFrontendChange, 400, Kind::SummaryMismatch) => {
            Some("summary_mismatch")
        }
        (PairedHostOperation::SubmitFrontendChange, 400, Kind::DigestMismatch) => {
            Some("digest_mismatch")
        }
        (PairedHostOperation::SubmitFrontendChange, 409, Kind::StaleBase) => Some("stale_base"),
        (PairedHostOperation::SubmitFrontendChange, 409, Kind::CandidateInvalid) => None,
        (_, 409, Kind::Conflict) => Some("snapshot_changed"),
        (_, 503, Kind::Unavailable) => Some("current_inputs_invalid"),
        (_, 500, Kind::InternalError) => Some("internal_error"),
        _ => {
            return Err(unconfirmed(
                operation,
                UnconfirmedReason::UnexpectedResponse,
            ));
        }
    };
    if rejected.problems.is_empty()
        || !rejected
            .problems
            .iter()
            .all(FrontendProblem::has_host_shape)
        || expected_code
            .is_some_and(|code| rejected.problems.len() != 1 || rejected.problems[0].code != code)
    {
        return Err(unconfirmed(
            operation,
            UnconfirmedReason::UnexpectedResponse,
        ));
    }
    Ok(FrontendChangeRejection {
        http_status: status,
        error: rejected.error,
        problems: rejected.problems,
    })
}

fn prepare_rejection(
    status: u16,
    bytes: &[u8],
) -> Result<FrontendRequestRejection, PairedHostError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Rejection {
        error: FrontendRejectionKind,
        problems: Vec<FrontendProblem>,
    }
    let operation = PairedHostOperation::PrepareCandidate;
    let rejected: Rejection = decode(bytes, operation)?;
    let expected = if status == 500 {
        FrontendRejectionKind::InternalError
    } else {
        FrontendRejectionKind::InvalidRequest
    };
    if rejected.error != expected
        || rejected.problems.is_empty()
        || !rejected
            .problems
            .iter()
            .all(FrontendProblem::has_host_shape)
    {
        return Err(unconfirmed(
            operation,
            UnconfirmedReason::UnexpectedResponse,
        ));
    }
    Ok(FrontendRequestRejection {
        http_status: status,
        error: rejected.error,
        problems: rejected.problems,
    })
}

fn required_nullable_raw<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Box<RawValue>>, D::Error> {
    Option::<Box<RawValue>>::deserialize(deserializer)
}

fn json_object(value: &RawValue) -> bool {
    value.get().trim_start().starts_with('{')
}

// Compare the returned record with what was submitted without rebuilding the
// payload, re-rendering TOML or normalizing floats. Owner JSON member order and
// string escaping may change after durable storage; number tokens must not.
fn same_owner_json(left: &RawValue, right: &RawValue) -> bool {
    let a = left.get().trim();
    let b = right.get().trim();
    if a == b {
        return true;
    }
    if a.starts_with('{') && b.starts_with('{') {
        type Object = BTreeMap<String, Box<RawValue>>;
        let (Ok(a), Ok(b)) = (
            serde_json::from_str::<Object>(a),
            serde_json::from_str::<Object>(b),
        ) else {
            return false;
        };
        return a.len() == b.len()
            && a.iter().all(|(key, value)| {
                b.get(key)
                    .is_some_and(|other| same_owner_json(value, other))
            });
    }
    if a.starts_with('[') && b.starts_with('[') {
        type Array = Vec<Box<RawValue>>;
        let (Ok(a), Ok(b)) = (
            serde_json::from_str::<Array>(a),
            serde_json::from_str::<Array>(b),
        ) else {
            return false;
        };
        return a.len() == b.len() && a.iter().zip(&b).all(|(a, b)| same_owner_json(a, b));
    }
    if a.starts_with('"') && b.starts_with('"') {
        return serde_json::from_str::<String>(a).ok() == serde_json::from_str::<String>(b).ok();
    }
    false
}

// A validation-only serde visitor, not a new JSON representation. In
// particular it cannot replace the retained RawValue with rounded numbers or
// let duplicate keys disappear into a map before closed-shape decoding.
struct UniqueJson;

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;
            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("unique-key JSON")
            }
            fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson)
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<UniqueJson, A::Error> {
                while access.next_element::<UniqueJson>()?.is_some() {}
                Ok(UniqueJson)
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut access: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut keys = BTreeSet::new();
                while let Some(key) = access.next_key::<String>()? {
                    if !keys.insert(key) {
                        return Err(serde::de::Error::custom("duplicate key"));
                    }
                    access.next_value::<UniqueJson>()?;
                }
                Ok(UniqueJson)
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

fn unconfirmed(operation: PairedHostOperation, reason: UnconfirmedReason) -> PairedHostError {
    PairedHostError::Unconfirmed { operation, reason }
}

fn transport_error(operation: PairedHostOperation, error: &reqwest::Error) -> PairedHostError {
    unconfirmed(
        operation,
        if error.is_timeout() {
            UnconfirmedReason::Timeout
        } else {
            UnconfirmedReason::Transport
        },
    )
}

async fn read_body(
    response: Response,
    operation: PairedHostOperation,
) -> Result<Vec<u8>, PairedHostError> {
    read_body_with_limit(response, operation, MAX_BODY_BYTES).await
}

async fn read_body_with_limit(
    mut response: Response,
    operation: PairedHostOperation,
    limit: usize,
) -> Result<Vec<u8>, PairedHostError> {
    let json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
    if !json {
        return Err(unconfirmed(
            operation,
            UnconfirmedReason::UnexpectedResponse,
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(unconfirmed(operation, UnconfirmedReason::ResponseTooLarge));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| transport_error(operation, &error))?
    {
        if chunk.len() > limit - bytes.len() {
            return Err(unconfirmed(operation, UnconfirmedReason::ResponseTooLarge));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn decode<T: DeserializeOwned>(
    bytes: &[u8],
    operation: PairedHostOperation,
) -> Result<T, PairedHostError> {
    serde_json::from_slice::<UniqueJson>(bytes)
        .map_err(|_| unconfirmed(operation, UnconfirmedReason::UnexpectedResponse))?;
    // Direct typed decoding and RawValue preserve exact owner data. Never
    // retain serde's input-bearing error.
    serde_json::from_slice(bytes)
        .map_err(|_| unconfirmed(operation, UnconfirmedReason::UnexpectedResponse))
}

fn required_nullable_text<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

fn present_text<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

fn valid_review_url(
    path: Option<&str>,
    valid: bool,
    digest: Option<&FrontendChangeDigest>,
) -> bool {
    path.is_none_or(|path| {
        valid
            && digest.is_some_and(|digest| path == format!("{FRONTEND_REVIEW}/{}", digest.as_str()))
    })
}

fn host_refusal(operation: PairedHostOperation, status: u16, bytes: &[u8]) -> PairedHostError {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Refusal {
        error: String,
        code: Option<String>,
    }
    let Ok(reply) = serde_json::from_slice::<Refusal>(bytes) else {
        return unconfirmed(operation, UnconfirmedReason::UnexpectedResponse);
    };
    let reason = match (
        operation,
        status,
        reply.error.as_str(),
        reply.code.as_deref(),
    ) {
        (
            PairedHostOperation::PrepareCandidate
            | PairedHostOperation::FrontendInventory
            | PairedHostOperation::SubmitFrontendChange
            | PairedHostOperation::FrontendChangeStatus,
            403,
            "forbidden",
            Some("console_owner_required"),
        ) => PairedHostRefusal::ConsoleOwnerRequired,
        (
            PairedHostOperation::RequestPairing
            | PairedHostOperation::ConfirmPairing
            | PairedHostOperation::ConfirmTerminalPairing,
            503,
            "pairing_unavailable",
            None,
        ) => PairedHostRefusal::PairingUnavailable,
        (PairedHostOperation::ConfirmTerminalPairing, 403, "pairing_unavailable", None) => {
            PairedHostRefusal::PairingUnavailable
        }
        (PairedHostOperation::RequestPairing, 429, "pairing_locked", None)
        | (PairedHostOperation::ConfirmTerminalPairing, 429, "pairing_locked", None)
        | (PairedHostOperation::ConfirmPairing, 403, "pairing_locked", None) => {
            PairedHostRefusal::PairingLocked
        }
        (PairedHostOperation::RequestPairing, 429, "too_many_requests", None) => {
            PairedHostRefusal::TooManyRequests
        }
        (PairedHostOperation::RequestPairing, 503, "notify_failed", None) => {
            PairedHostRefusal::NotifyFailed
        }
        (
            PairedHostOperation::ConfirmPairing | PairedHostOperation::ConfirmTerminalPairing,
            403,
            "no_pending_request",
            None,
        ) => PairedHostRefusal::NoPendingRequest,
        (PairedHostOperation::ConfirmPairing, 403, "too_many_attempts", None) => {
            PairedHostRefusal::TooManyAttempts
        }
        (
            PairedHostOperation::ConfirmPairing | PairedHostOperation::ConfirmTerminalPairing,
            403,
            "wrong_code",
            None,
        ) => PairedHostRefusal::WrongCode,
        _ => return unconfirmed(operation, UnconfirmedReason::UnexpectedResponse),
    };
    PairedHostError::Refused { operation, reason }
}

#[cfg(test)]
mod presenter_transport_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn presenter_credential_is_route_limited_and_never_adopts_cookies() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut client = PairedHostClient::for_presenter(
            &origin,
            "x-paired-host-presenter-credential",
            "private-test-only",
        )
        .unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    assert!(request.len() < 8192);
                    request.push(socket.read_u8().await.unwrap());
                }
                requests.push(String::from_utf8(request).unwrap());
                let body = r#"{"error":"forbidden","code":"console_owner_required"}"#;
                let reply = format!(
                    "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nSet-Cookie: paired_host_device=unselected; Path=/; HttpOnly\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
                socket.shutdown().await.unwrap();
            }
            requests
        });
        assert_eq!(
            client.request_pairing("no enrollment").await.unwrap_err(),
            PairedHostError::PresenterOperationUnavailable
        );
        assert_eq!(
            client
                .confirm_pairing(PairingCode::new("123456".into()).unwrap())
                .await
                .unwrap_err(),
            PairedHostError::PresenterOperationUnavailable
        );
        for _ in 0..2 {
            assert!(matches!(
                client.frontend_inventory().await,
                Err(PairedHostError::Refused {
                    operation: PairedHostOperation::FrontendInventory,
                    reason: PairedHostRefusal::ConsoleOwnerRequired
                })
            ));
        }
        let requests = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        for request in requests {
            assert!(request.starts_with("GET /console/extensions/frontend/inventory HTTP/1.1\r\n"));
            let request = request.to_ascii_lowercase();
            assert!(
                request.contains("\r\nx-paired-host-presenter-credential: private-test-only\r\n")
            );
            assert!(request.contains(&format!("\r\norigin: {origin}\r\n")));
            assert!(!request.contains("\r\ncookie:") && !request.contains("\r\nauthorization:"));
        }
        for operation in [
            PairedHostOperation::FrontendInventory,
            PairedHostOperation::FrontendSourceTags,
            PairedHostOperation::FrontendSourceInspect,
            PairedHostOperation::PrepareCandidate,
            PairedHostOperation::FrontendChangeStatus,
        ] {
            let request = client
                .authorize(operation, client.http.get(&origin))
                .unwrap()
                .build()
                .unwrap();
            assert!(request.headers()["x-paired-host-presenter-credential"].is_sensitive());
        }
        for operation in [
            PairedHostOperation::RequestPairing,
            PairedHostOperation::ConfirmPairing,
            PairedHostOperation::ConfirmTerminalPairing,
            PairedHostOperation::SubmitFrontendChange,
            PairedHostOperation::CreateExtensionSecretIntent,
            PairedHostOperation::ExtensionSecretIntentStatus,
            PairedHostOperation::CancelExtensionSecretIntent,
        ] {
            assert!(matches!(
                client.authorize(operation, client.http.post(&origin)),
                Err(PairedHostError::PresenterOperationUnavailable)
            ));
        }
        assert!(client.take_confirmed_device().is_none());
        assert!(!format!("{client:?}").contains("private-test-only"));
    }

    #[tokio::test]
    async fn presenter_prepare_redirect_never_forwards_the_credential() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let destination = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let redirect = format!(
            "http://{}/private-destination",
            destination.local_addr().unwrap()
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                assert!(request.len() < 8192);
                request.push(socket.read_u8().await.unwrap());
            }
            assert!(
                String::from_utf8_lossy(&request)
                    .to_ascii_lowercase()
                    .contains("\r\ncontent-length: 2\r\n")
            );
            let mut body = [0; 2];
            socket.read_exact(&mut body).await.unwrap();
            assert_eq!(&body, b"{}");
            let reply = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: {redirect}\r\nSet-Cookie: paired_host_device=unselected; Path=/; HttpOnly\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            socket.write_all(reply.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
            String::from_utf8(request).unwrap()
        });
        let client = PairedHostClient::for_presenter(
            &origin,
            "x-paired-host-presenter-credential",
            "private-test-only",
        )
        .unwrap();
        let error = client
            .post(
                PairedHostOperation::PrepareCandidate,
                &serde_json::json!({}),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error,
            PairedHostError::Unconfirmed {
                operation: PairedHostOperation::PrepareCandidate,
                reason: UnconfirmedReason::Redirect,
            }
        );
        let request = tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
        assert!(request.starts_with("POST /console/extensions/frontend/prepare HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("\r\nx-paired-host-presenter-credential: private-test-only\r\n")
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(80), destination.accept())
                .await
                .is_err()
        );
        assert!(!error.to_string().contains("private-test-only"));
        assert!(!error.to_string().contains("private-destination"));
    }

    #[test]
    fn presenter_credential_travels_in_the_extension_header_the_host_names() {
        let origin = "https://paired-host.example";
        for header in [
            "x-paired-host-presenter-credential",
            "x-host-session",
            "x-a1",
        ] {
            let client = PairedHostClient::for_presenter(origin, header, "private-test-only")
                .expect("a lowercase x- header is the host's choice");
            let (name, _) = client.presenter_credential.as_ref().unwrap();
            assert_eq!(name.as_str(), header);
        }
        for header in [
            "authorization",
            "cookie",
            "origin",
            "X-Upper",
            "x-",
            "x--double",
            "x-trailing-",
            "x-with space",
            "x-under_score",
            "",
        ] {
            assert_eq!(
                PairedHostClient::for_presenter(origin, header, "private-test-only").unwrap_err(),
                PairedHostError::InvalidPresenterCredential,
                "{header}"
            );
        }
    }
}
