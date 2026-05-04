//! Short-lived OAuth login flow authority.
//!
//! Runtime surfaces own an explicit authority instance for state -> PKCE
//! verifier and device-code lifecycle correlation. Start records a flow before
//! returning it to the client; complete must verify and consume that state
//! through the same authority before committing terminal login state.

use std::collections::HashMap;
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use base64::Engine as _;
use meerkat_core::{AuthProfile, BackendProfile, ConnectionRef, CredentialSourceSpec, Provider};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::auth_oauth::OAuthEndpoints;
use crate::auth_store::{
    PersistedAuthMode, credential_source_uses_persisted_store, persisted_auth_mode_for_auth_method,
};

const DEFAULT_MAX_OUTSTANDING_FLOWS: usize = 1024;

const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const ANTHROPIC_AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const ANTHROPIC_CONSOLE_AUTHORIZE_URL: &str = "https://platform.claude.com/oauth/authorize";
const ANTHROPIC_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const ANTHROPIC_SCOPES: &[&str] = &[
    "user:profile",
    "user:inference",
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
];
const ANTHROPIC_CONSOLE_SCOPES: &[&str] = &["org:create_api_key", "user:profile"];
const ANTHROPIC_BETA_HEADER_NAME: &str = "anthropic-beta";
const ANTHROPIC_BETA_HEADER_VALUE: &str = "oauth-2025-04-20";

const OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const OPENAI_TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const OPENAI_SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    "offline_access",
    "api.connectors.read",
    "api.connectors.invoke",
];

const GOOGLE_CLIENT_ID: &str = concat!(
    "6812558",
    "09395-oo8ft2oprdrnp9e3aqf6av3hmdib135j",
    ".apps.googleusercontent.com",
);
const GOOGLE_CLIENT_SECRET: &str = concat!("GOCSP", "X-4uHgMPm", "-1o7Sk-geV6Cu5clXFsxl");
const GOOGLE_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_DEVICE_CODE_URL: &str = "https://oauth2.googleapis.com/device/code";
const GOOGLE_SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
];

const TEST_OAUTH_ENDPOINT_OVERRIDE_ENV: &str = "MEERKAT_TEST_OAUTH_ENDPOINT_OVERRIDE";
const TEST_OAUTH_BASE_URL_ENV: &str = "MEERKAT_TEST_OAUTH_BASE_URL";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OAuthProviderIdentity {
    AnthropicClaudeAi,
    AnthropicConsoleApiKey,
    OpenAiChatGpt,
    GoogleCodeAssist,
}

impl OAuthProviderIdentity {
    pub fn from_alias(alias: &str) -> Option<Self> {
        match alias {
            "anthropic" | "claude" | "claude.ai" => Some(Self::AnthropicClaudeAi),
            "anthropic_console_api_key" => Some(Self::AnthropicConsoleApiKey),
            "openai" | "chatgpt" => Some(Self::OpenAiChatGpt),
            "google" | "gemini" | "code_assist" => Some(Self::GoogleCodeAssist),
            _ => None,
        }
    }

    pub fn canonical_alias(self) -> &'static str {
        match self {
            Self::AnthropicClaudeAi => "anthropic",
            Self::AnthropicConsoleApiKey => "anthropic_console_api_key",
            Self::OpenAiChatGpt => "openai",
            Self::GoogleCodeAssist => "google",
        }
    }

    pub fn provider(self) -> Provider {
        match self {
            Self::AnthropicClaudeAi | Self::AnthropicConsoleApiKey => Provider::Anthropic,
            Self::OpenAiChatGpt => Provider::OpenAI,
            Self::GoogleCodeAssist => Provider::Gemini,
        }
    }

    pub fn auth_mode(self) -> PersistedAuthMode {
        match self {
            Self::AnthropicClaudeAi => PersistedAuthMode::ClaudeAiOauth,
            Self::AnthropicConsoleApiKey => PersistedAuthMode::OauthToApiKey,
            Self::OpenAiChatGpt => PersistedAuthMode::ChatgptOauth,
            Self::GoogleCodeAssist => PersistedAuthMode::GoogleOauth,
        }
    }

    pub fn backend_kind(self) -> &'static str {
        match self {
            Self::AnthropicClaudeAi | Self::AnthropicConsoleApiKey => "anthropic_api",
            Self::OpenAiChatGpt => "chatgpt_backend",
            Self::GoogleCodeAssist => "google_code_assist",
        }
    }

    pub fn client_secret(self) -> Option<&'static str> {
        match self {
            Self::AnthropicClaudeAi | Self::AnthropicConsoleApiKey | Self::OpenAiChatGpt => None,
            Self::GoogleCodeAssist => Some(GOOGLE_CLIENT_SECRET),
        }
    }

    pub fn endpoints(self, redirect_uri: impl Into<String>) -> OAuthEndpoints {
        let endpoints = match self {
            Self::AnthropicClaudeAi => OAuthEndpoints {
                client_id: ANTHROPIC_CLIENT_ID.into(),
                authorize_url: ANTHROPIC_AUTHORIZE_URL.into(),
                token_url: ANTHROPIC_TOKEN_URL.into(),
                device_code_url: None,
                redirect_uri: redirect_uri.into(),
                scopes: strings(ANTHROPIC_SCOPES),
                extra_headers: vec![(
                    ANTHROPIC_BETA_HEADER_NAME.into(),
                    ANTHROPIC_BETA_HEADER_VALUE.into(),
                )],
            },
            Self::AnthropicConsoleApiKey => OAuthEndpoints {
                client_id: ANTHROPIC_CLIENT_ID.into(),
                authorize_url: ANTHROPIC_CONSOLE_AUTHORIZE_URL.into(),
                token_url: ANTHROPIC_TOKEN_URL.into(),
                device_code_url: None,
                redirect_uri: redirect_uri.into(),
                scopes: strings(ANTHROPIC_CONSOLE_SCOPES),
                extra_headers: vec![(
                    ANTHROPIC_BETA_HEADER_NAME.into(),
                    ANTHROPIC_BETA_HEADER_VALUE.into(),
                )],
            },
            Self::OpenAiChatGpt => OAuthEndpoints {
                client_id: OPENAI_CLIENT_ID.into(),
                authorize_url: OPENAI_AUTHORIZE_URL.into(),
                token_url: OPENAI_TOKEN_URL.into(),
                device_code_url: None,
                redirect_uri: redirect_uri.into(),
                scopes: strings(OPENAI_SCOPES),
                extra_headers: Vec::new(),
            },
            Self::GoogleCodeAssist => OAuthEndpoints {
                client_id: GOOGLE_CLIENT_ID.into(),
                authorize_url: GOOGLE_AUTHORIZE_URL.into(),
                token_url: GOOGLE_TOKEN_URL.into(),
                device_code_url: Some(GOOGLE_DEVICE_CODE_URL.into()),
                redirect_uri: redirect_uri.into(),
                scopes: strings(GOOGLE_SCOPES),
                extra_headers: Vec::new(),
            },
        };
        apply_test_oauth_endpoint_override(self, endpoints)
    }
}

impl std::fmt::Display for OAuthProviderIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.canonical_alias())
    }
}

#[derive(Debug, Clone)]
pub struct OAuthProviderResolution {
    pub identity: OAuthProviderIdentity,
    pub provider: Provider,
    pub endpoints: OAuthEndpoints,
    pub auth_mode: PersistedAuthMode,
    pub client_secret: Option<&'static str>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("Unknown provider '{provider}'. Supported: anthropic, openai, google.")]
pub struct OAuthProviderResolutionError {
    pub provider: String,
}

pub fn resolve_oauth_provider(
    provider: &str,
    redirect_uri: impl Into<String>,
) -> Result<OAuthProviderResolution, OAuthProviderResolutionError> {
    let identity = OAuthProviderIdentity::from_alias(provider).ok_or_else(|| {
        OAuthProviderResolutionError {
            provider: provider.to_string(),
        }
    })?;
    Ok(OAuthProviderResolution {
        identity,
        provider: identity.provider(),
        endpoints: identity.endpoints(redirect_uri),
        auth_mode: identity.auth_mode(),
        client_secret: identity.client_secret(),
    })
}

/// Apply the local OAuth fixture endpoint override used by release-grade auth
/// smoke tests. Production code only observes this when both explicit
/// `MEERKAT_TEST_*` environment variables are set.
#[doc(hidden)]
pub fn apply_test_oauth_endpoint_override(
    identity: OAuthProviderIdentity,
    mut endpoints: OAuthEndpoints,
) -> OAuthEndpoints {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let enabled = std::env::var(TEST_OAUTH_ENDPOINT_OVERRIDE_ENV)
            .map(|value| {
                matches!(
                    value.as_str(),
                    "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
                )
            })
            .unwrap_or(false);
        if !enabled {
            return endpoints;
        }
        let Ok(base_url) = std::env::var(TEST_OAUTH_BASE_URL_ENV) else {
            return endpoints;
        };
        let base_url = base_url.trim_end_matches('/');
        let provider = identity.canonical_alias();
        endpoints.authorize_url = format!("{base_url}/{provider}/authorize");
        endpoints.token_url = format!("{base_url}/{provider}/token");
        if endpoints.device_code_url.is_some() {
            endpoints.device_code_url = Some(format!("{base_url}/{provider}/device/code"));
        }
    }
    endpoints
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OAuthTargetValidationError {
    #[error("OAuth target backend provider mismatch: expected {expected:?}, got {actual:?}")]
    BackendProviderMismatch {
        expected: Provider,
        actual: Provider,
    },
    #[error(
        "OAuth target backend_kind '{backend_kind}' cannot store credential mode {expected_mode:?}; expected backend_kind '{expected_backend_kind}'"
    )]
    BackendKindMismatch {
        backend_kind: String,
        expected_backend_kind: &'static str,
        expected_mode: PersistedAuthMode,
    },
    #[error("OAuth target provider mismatch: expected {expected:?}, got {actual:?}")]
    ProviderMismatch {
        expected: Provider,
        actual: Provider,
    },
    #[error(
        "OAuth target auth_method '{auth_method}' cannot store credential mode {expected_mode:?}"
    )]
    AuthMethodMismatch {
        auth_method: String,
        expected_mode: PersistedAuthMode,
    },
    #[error(
        "OAuth target source '{source_kind}' cannot store OAuth credentials; expected source.kind = 'managed_store' or 'platform_default'"
    )]
    SourceMismatch { source_kind: &'static str },
}

pub fn validate_oauth_login_target(
    auth_profile: &AuthProfile,
    identity: OAuthProviderIdentity,
) -> Result<(), OAuthTargetValidationError> {
    validate_oauth_target_for_auth_mode(auth_profile, identity.provider(), identity.auth_mode())
}

pub fn validate_oauth_login_binding(
    backend_profile: &BackendProfile,
    auth_profile: &AuthProfile,
    identity: OAuthProviderIdentity,
) -> Result<(), OAuthTargetValidationError> {
    validate_oauth_target_binding_for_auth_mode(
        backend_profile,
        auth_profile,
        identity.provider(),
        identity.auth_mode(),
        identity.backend_kind(),
    )
}

pub fn validate_oauth_target_for_auth_mode(
    auth_profile: &AuthProfile,
    expected_provider: Provider,
    expected_mode: PersistedAuthMode,
) -> Result<(), OAuthTargetValidationError> {
    if auth_profile.provider != expected_provider {
        return Err(OAuthTargetValidationError::ProviderMismatch {
            expected: expected_provider,
            actual: auth_profile.provider,
        });
    }
    match persisted_auth_mode_for_auth_method(&auth_profile.auth_method) {
        Some(actual_mode) if actual_mode == expected_mode => {}
        _ => {
            return Err(OAuthTargetValidationError::AuthMethodMismatch {
                auth_method: auth_profile.auth_method.clone(),
                expected_mode,
            });
        }
    }
    if !oauth_source_can_store_flow_credentials(&auth_profile.source) {
        return Err(OAuthTargetValidationError::SourceMismatch {
            source_kind: source_kind_label(&auth_profile.source),
        });
    }
    Ok(())
}

pub fn validate_oauth_target_binding_for_auth_mode(
    backend_profile: &BackendProfile,
    auth_profile: &AuthProfile,
    expected_provider: Provider,
    expected_mode: PersistedAuthMode,
    expected_backend_kind: &'static str,
) -> Result<(), OAuthTargetValidationError> {
    validate_oauth_target_for_auth_mode(auth_profile, expected_provider, expected_mode)?;
    if backend_profile.provider != expected_provider {
        return Err(OAuthTargetValidationError::BackendProviderMismatch {
            expected: expected_provider,
            actual: backend_profile.provider,
        });
    }
    if backend_profile.backend_kind != expected_backend_kind {
        return Err(OAuthTargetValidationError::BackendKindMismatch {
            backend_kind: backend_profile.backend_kind.clone(),
            expected_backend_kind,
            expected_mode,
        });
    }
    Ok(())
}

fn oauth_source_can_store_flow_credentials(source: &CredentialSourceSpec) -> bool {
    credential_source_uses_persisted_store(source)
}

fn source_kind_label(source: &CredentialSourceSpec) -> &'static str {
    match source {
        CredentialSourceSpec::InlineSecret { .. } => "inline_secret",
        CredentialSourceSpec::ManagedStore => "managed_store",
        CredentialSourceSpec::Env { .. } => "env",
        CredentialSourceSpec::ExternalResolver { .. } => "external_resolver",
        CredentialSourceSpec::PlatformDefault => "platform_default",
        CredentialSourceSpec::Command { .. } => "command",
        CredentialSourceSpec::FileDescriptor { .. } => "file_descriptor",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthFlowRecord {
    pub target: ConnectionRef,
    pub provider: OAuthProviderIdentity,
    pub redirect_uri: String,
    pub pkce_verifier: String,
    pub created_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthDeviceFlowRecord {
    pub target: ConnectionRef,
    pub provider: OAuthProviderIdentity,
    pub device_code: String,
    pub created_at: Instant,
    pub expires_at: Instant,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuthPrunedFlows {
    pub browser: Vec<(String, ConnectionRef)>,
    pub device: Vec<(String, ConnectionRef)>,
}

impl OAuthPrunedFlows {
    fn from_expired(
        browser: Vec<(String, OAuthFlowRecord)>,
        device: Vec<OAuthDeviceFlowRecord>,
    ) -> Self {
        Self {
            browser: browser
                .into_iter()
                .map(|(flow_id, record)| (flow_id, record.target))
                .collect(),
            device: device
                .into_iter()
                .map(|record| (record.device_code, record.target))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthFlowRegistrySnapshot {
    #[serde(default)]
    pub browser: Vec<PersistedOAuthBrowserFlow>,
    #[serde(default)]
    pub device: Vec<PersistedOAuthDeviceFlow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedOAuthBrowserFlow {
    pub state: String,
    pub target: ConnectionRef,
    pub provider: String,
    pub redirect_uri: String,
    pub pkce_verifier: String,
    pub created_at_millis: u64,
    pub expires_at_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedOAuthDeviceFlow {
    pub target: ConnectionRef,
    pub provider: String,
    pub device_code: String,
    pub created_at_millis: u64,
    pub expires_at_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OAuthDeviceFlowState {
    record: OAuthDeviceFlowRecord,
    poll_lease: Option<OAuthDevicePollLeaseState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OAuthDevicePollLeaseState {
    id: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OAuthFlowError {
    #[error("oauth state is missing or expired")]
    Missing,
    #[error("oauth registry projection payload missing after AuthMachine accepted {operation}")]
    RegistryProjectionMissing { operation: &'static str },
    #[error("oauth state provider mismatch: expected {expected}, got {actual}")]
    ProviderMismatch {
        expected: OAuthProviderIdentity,
        actual: OAuthProviderIdentity,
    },
    #[error("oauth state redirect_uri mismatch")]
    RedirectUriMismatch,
    #[error("oauth state target mismatch: expected {expected:?}, got {actual:?}")]
    TargetMismatch {
        expected: Box<ConnectionRef>,
        actual: Box<ConnectionRef>,
    },
    #[error("failed to generate oauth state token")]
    StateGenerationFailed,
    #[error("oauth state registry is at capacity ({max_outstanding} outstanding flows)")]
    CapacityExceeded { max_outstanding: usize },
    #[error("oauth device code poll is already in progress")]
    DevicePollInProgress,
    #[error("oauth device code is already admitted")]
    DeviceCodeAlreadyAdmitted,
    #[error("oauth device code expiry is out of range")]
    DeviceExpiryOutOfRange,
    #[error("oauth flow lifecycle transition rejected during {operation}: {detail}")]
    LifecycleRejected {
        operation: &'static str,
        detail: String,
    },
    #[error("oauth flow durable persistence failed during {operation}: {detail}")]
    PersistenceFailed {
        operation: &'static str,
        detail: String,
    },
}

pub trait OAuthDevicePollLifecycle: Send + Sync {
    fn device_flow_state_is_authmachine_owned(&self) -> bool {
        false
    }

    fn finish_device_poll(
        &self,
        target: &ConnectionRef,
        device_code: &str,
    ) -> Result<(), OAuthFlowError>;

    fn consume_device_flow(
        &self,
        target: &ConnectionRef,
        device_code: &str,
        provider: OAuthProviderIdentity,
    ) -> Result<(), OAuthFlowError>;

    fn expire_device_flow(
        &self,
        target: &ConnectionRef,
        device_code: &str,
    ) -> Result<(), OAuthFlowError>;

    fn restore_device_flow(&self, _record: &OAuthDeviceFlowRecord) -> Result<(), OAuthFlowError> {
        Ok(())
    }

    fn device_flow_payloads_changed(&self) -> Result<(), OAuthFlowError> {
        Ok(())
    }

    fn device_flow_payload_removed(
        &self,
        _record: &OAuthDeviceFlowRecord,
    ) -> Result<(), OAuthFlowError> {
        self.device_flow_payloads_changed()
    }
}

pub struct OAuthDevicePollLease {
    device_flows: Arc<Mutex<HashMap<String, OAuthDeviceFlowState>>>,
    target: ConnectionRef,
    device_code: String,
    provider: OAuthProviderIdentity,
    lease_id: u64,
    lifecycle: Option<Arc<dyn OAuthDevicePollLifecycle>>,
    operation_lock: Option<Arc<StdMutex<()>>>,
    active: bool,
}

impl std::fmt::Debug for OAuthDevicePollLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthDevicePollLease")
            .field("target", &self.target)
            .field("device_code", &self.device_code)
            .field("provider", &self.provider)
            .field("lease_id", &self.lease_id)
            .field("has_lifecycle", &self.lifecycle.is_some())
            .field("has_operation_lock", &self.operation_lock.is_some())
            .field("active", &self.active)
            .finish()
    }
}

impl OAuthDevicePollLease {
    fn new(
        device_flows: Arc<Mutex<HashMap<String, OAuthDeviceFlowState>>>,
        target: ConnectionRef,
        device_code: String,
        provider: OAuthProviderIdentity,
        lease_id: u64,
    ) -> Self {
        Self {
            device_flows,
            target,
            device_code,
            provider,
            lease_id,
            lifecycle: None,
            operation_lock: None,
            active: true,
        }
    }

    pub fn terminal_flow_state_is_authmachine_owned(&self) -> bool {
        self.lifecycle
            .as_ref()
            .map(|lifecycle| lifecycle.device_flow_state_is_authmachine_owned())
            .unwrap_or(false)
    }

    fn local_missing_error(&self, operation: &'static str) -> OAuthFlowError {
        if self.terminal_flow_state_is_authmachine_owned() {
            OAuthFlowError::RegistryProjectionMissing { operation }
        } else {
            OAuthFlowError::Missing
        }
    }

    pub fn with_lifecycle(mut self, lifecycle: Arc<dyn OAuthDevicePollLifecycle>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    pub fn with_operation_lock(mut self, operation_lock: Arc<StdMutex<()>>) -> Self {
        self.operation_lock = Some(operation_lock);
        self
    }

    pub fn finish(mut self) -> Result<(), OAuthFlowError> {
        let _operation_guard = self.operation_lock.as_ref().map(|lock| {
            lock.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });
        let verify_result = {
            let mut flows = self.device_flows.lock();
            prune_expired_device_locked(&mut flows);
            verify_device_poll_lease_locked(
                &mut flows,
                &self.device_code,
                self.provider,
                self.lease_id,
            )
            .map(|_| ())
        };
        if matches!(verify_result, Err(OAuthFlowError::Missing)) {
            if self.terminal_flow_state_is_authmachine_owned()
                && let Some(lifecycle) = &self.lifecycle
            {
                let _ = lifecycle.finish_device_poll(&self.target, &self.device_code);
            }
            return Err(self.local_missing_error("finish_oauth_device_poll"));
        }
        verify_result?;

        if let Some(lifecycle) = &self.lifecycle {
            lifecycle.finish_device_poll(&self.target, &self.device_code)?;
        }

        let result = {
            let mut flows = self.device_flows.lock();
            let result = release_device_poll_lease_locked(
                &mut flows,
                &self.device_code,
                self.provider,
                self.lease_id,
            );
            prune_expired_device_locked(&mut flows);
            result
        };
        match result {
            Ok(()) => {
                self.active = false;
                if let Some(lifecycle) = &self.lifecycle {
                    lifecycle.device_flow_payloads_changed()?;
                }
                Ok(())
            }
            Err(OAuthFlowError::Missing) => {
                Err(self.local_missing_error("finish_oauth_device_poll"))
            }
            Err(err) => Err(err),
        }
    }

    pub fn verify(&self) -> Result<OAuthDeviceFlowRecord, OAuthFlowError> {
        let mut flows = self.device_flows.lock();
        prune_expired_device_locked(&mut flows);
        let result = verify_device_poll_lease_locked(
            &mut flows,
            &self.device_code,
            self.provider,
            self.lease_id,
        );
        prune_expired_device_locked(&mut flows);
        if matches!(result, Err(OAuthFlowError::Missing)) {
            return Err(self.local_missing_error("verify_oauth_device_poll"));
        }
        result
    }

    pub fn consume(mut self) -> Result<OAuthDeviceFlowRecord, OAuthFlowError> {
        let _operation_guard = self.operation_lock.as_ref().map(|lock| {
            lock.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        });
        let verified = {
            let mut flows = self.device_flows.lock();
            prune_expired_device_locked(&mut flows);
            verify_device_poll_lease_locked(
                &mut flows,
                &self.device_code,
                self.provider,
                self.lease_id,
            )
        };
        let verified = match verified {
            Ok(verified) => verified,
            Err(OAuthFlowError::Missing) => {
                if self.terminal_flow_state_is_authmachine_owned()
                    && let Some(lifecycle) = &self.lifecycle
                {
                    let _ = lifecycle.finish_device_poll(&self.target, &self.device_code);
                }
                return Err(self.local_missing_error("consume_oauth_device_flow"));
            }
            Err(err) => return Err(err),
        };

        if let Some(lifecycle) = &self.lifecycle {
            lifecycle.consume_device_flow(&self.target, &self.device_code, self.provider)?;
        }

        let result = {
            let mut flows = self.device_flows.lock();
            let result = consume_device_poll_lease_locked(
                &mut flows,
                &self.device_code,
                self.provider,
                self.lease_id,
            );
            prune_expired_device_locked(&mut flows);
            result
        };
        match result {
            Ok(record) => {
                self.active = false;
                if let Some(lifecycle) = &self.lifecycle
                    && let Err(err) = lifecycle.device_flow_payload_removed(&record)
                {
                    if matches!(
                        err,
                        OAuthFlowError::Missing | OAuthFlowError::RegistryProjectionMissing { .. }
                    ) {
                        return Err(err);
                    }
                    let _ = lifecycle.restore_device_flow(&verified);
                    let mut flows = self.device_flows.lock();
                    flows.insert(
                        verified.device_code.clone(),
                        OAuthDeviceFlowState {
                            record: verified,
                            poll_lease: None,
                        },
                    );
                    return Err(err);
                }
                Ok(record)
            }
            Err(err) => {
                if matches!(err, OAuthFlowError::Missing)
                    && self.terminal_flow_state_is_authmachine_owned()
                {
                    return Err(self.local_missing_error("consume_oauth_device_flow"));
                }
                if let Some(lifecycle) = &self.lifecycle {
                    let _ = lifecycle.restore_device_flow(&verified);
                    if matches!(err, OAuthFlowError::Missing) {
                        let _ = lifecycle.expire_device_flow(&self.target, &self.device_code);
                    }
                }
                Err(err)
            }
        }
    }
}

impl Drop for OAuthDevicePollLease {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut flows = self.device_flows.lock();
        prune_expired_device_locked(&mut flows);
        let result = release_device_poll_lease_locked(
            &mut flows,
            &self.device_code,
            self.provider,
            self.lease_id,
        );
        prune_expired_device_locked(&mut flows);
        if let Some(lifecycle) = &self.lifecycle {
            if matches!(result, Err(OAuthFlowError::Missing)) {
                if lifecycle.device_flow_state_is_authmachine_owned() {
                    let _ = lifecycle.finish_device_poll(&self.target, &self.device_code);
                } else {
                    let _ = lifecycle.expire_device_flow(&self.target, &self.device_code);
                }
            } else {
                let _ = lifecycle.finish_device_poll(&self.target, &self.device_code);
            }
        }
    }
}

pub trait OAuthFlowAuthority: Send + Sync {
    fn terminal_flow_state_is_authmachine_owned(&self) -> bool {
        false
    }

    fn start(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: String,
        pkce_verifier: String,
    ) -> Result<String, OAuthFlowError>;

    fn verify(
        &self,
        state: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
    ) -> Result<OAuthFlowRecord, OAuthFlowError>;

    fn consume(
        &self,
        state: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
    ) -> Result<OAuthFlowRecord, OAuthFlowError>;

    fn admit_device_code(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        device_code: String,
        expires_in: Duration,
    ) -> Result<(), OAuthFlowError>;

    fn verify_device_code(
        &self,
        device_code: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
    ) -> Result<OAuthDeviceFlowRecord, OAuthFlowError>;

    fn begin_device_code_poll(
        &self,
        device_code: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
    ) -> Result<OAuthDevicePollLease, OAuthFlowError>;
}

#[derive(Debug)]
pub struct OAuthFlowRegistry {
    ttl: Duration,
    max_outstanding: usize,
    flows: Mutex<HashMap<String, OAuthFlowRecord>>,
    device_flows: Arc<Mutex<HashMap<String, OAuthDeviceFlowState>>>,
    next_device_poll_lease_id: AtomicU64,
}

impl OAuthFlowRegistry {
    pub fn new(ttl: Duration) -> Self {
        Self::new_with_capacity(ttl, DEFAULT_MAX_OUTSTANDING_FLOWS)
    }

    pub fn new_with_capacity(ttl: Duration, max_outstanding: usize) -> Self {
        Self {
            ttl,
            max_outstanding: max_outstanding.max(1),
            flows: Mutex::new(HashMap::new()),
            device_flows: Arc::new(Mutex::new(HashMap::new())),
            next_device_poll_lease_id: AtomicU64::new(1),
        }
    }

    pub fn max_outstanding(&self) -> usize {
        self.max_outstanding
    }

    pub fn ttl(&self) -> Duration {
        self.ttl
    }

    pub fn new_state() -> Result<String, OAuthFlowError> {
        new_state_token()
    }

    pub fn start(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: impl Into<String>,
        pkce_verifier: impl Into<String>,
    ) -> Result<String, OAuthFlowError> {
        <Self as OAuthFlowAuthority>::start(
            self,
            target,
            provider,
            redirect_uri.into(),
            pkce_verifier.into(),
        )
    }

    pub fn verify(
        &self,
        state: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
    ) -> Result<OAuthFlowRecord, OAuthFlowError> {
        <Self as OAuthFlowAuthority>::verify(self, state, target, provider, redirect_uri)
    }

    pub fn consume(
        &self,
        state: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
    ) -> Result<OAuthFlowRecord, OAuthFlowError> {
        <Self as OAuthFlowAuthority>::consume(self, state, target, provider, redirect_uri)
    }

    pub fn admit_device_code(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        device_code: impl Into<String>,
        expires_in: Duration,
    ) -> Result<(), OAuthFlowError> {
        <Self as OAuthFlowAuthority>::admit_device_code(
            self,
            target,
            provider,
            device_code.into(),
            expires_in,
        )
    }

    pub fn verify_device_code(
        &self,
        device_code: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
    ) -> Result<OAuthDeviceFlowRecord, OAuthFlowError> {
        <Self as OAuthFlowAuthority>::verify_device_code(self, device_code, target, provider)
    }

    pub fn begin_device_code_poll(
        &self,
        device_code: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
    ) -> Result<OAuthDevicePollLease, OAuthFlowError> {
        <Self as OAuthFlowAuthority>::begin_device_code_poll(self, device_code, target, provider)
    }

    pub fn expire_device_code(
        &self,
        device_code: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
    ) -> Result<(), OAuthFlowError> {
        let mut flows = self.device_flows.lock();
        prune_expired_device_locked(&mut flows);
        let Some(state) = flows.get(device_code) else {
            return Err(OAuthFlowError::Missing);
        };
        verify_device_record(&state.record, target, provider)?;
        flows.remove(device_code);
        Ok(())
    }

    pub fn prune_expired_browser_flows(&self) -> Vec<(String, ConnectionRef)> {
        let mut flows = self.flows.lock();
        take_expired_locked(&mut flows, self.ttl)
            .into_iter()
            .map(|(flow_id, record)| (flow_id, record.target))
            .collect()
    }

    pub fn prune_expired_device_flows(&self) -> Vec<(String, ConnectionRef)> {
        let mut flows = self.device_flows.lock();
        take_expired_device_locked(&mut flows)
            .into_iter()
            .map(|record| (record.device_code, record.target))
            .collect()
    }

    pub fn retain_flows_with_lifecycle(
        &self,
        mut browser_active: impl FnMut(&ConnectionRef, &str) -> bool,
        mut device_active: impl FnMut(&ConnectionRef, &str) -> bool,
    ) -> OAuthPrunedFlows {
        let mut flows = self.flows.lock();
        let mut browser = Vec::new();
        flows.retain(|flow_id, record| {
            let keep = browser_active(&record.target, flow_id);
            if !keep {
                browser.push((flow_id.clone(), record.target.clone()));
            }
            keep
        });

        let mut device_flows = self.device_flows.lock();
        let mut device = Vec::new();
        device_flows.retain(|device_code, state| {
            let keep = device_active(&state.record.target, device_code);
            if !keep {
                device.push((device_code.clone(), state.record.target.clone()));
            }
            keep
        });
        OAuthPrunedFlows { browser, device }
    }

    pub fn snapshot_for_persistence(&self, now_millis: u64) -> OAuthFlowRegistrySnapshot {
        let flows = self.flows.lock();
        let browser = flows
            .iter()
            .filter_map(|(state, record)| {
                let elapsed = record.created_at.elapsed();
                if elapsed > self.ttl {
                    return None;
                }
                let elapsed_millis = duration_millis_u64(elapsed);
                let ttl_millis = duration_millis_u64(self.ttl);
                Some(PersistedOAuthBrowserFlow {
                    state: state.clone(),
                    target: record.target.clone(),
                    provider: record.provider.canonical_alias().to_string(),
                    redirect_uri: record.redirect_uri.clone(),
                    pkce_verifier: record.pkce_verifier.clone(),
                    created_at_millis: now_millis.saturating_sub(elapsed_millis),
                    expires_at_millis: now_millis
                        .saturating_add(ttl_millis.saturating_sub(elapsed_millis)),
                })
            })
            .collect::<Vec<_>>();
        drop(flows);

        let device_flows = self.device_flows.lock();
        let now = Instant::now();
        let device = device_flows
            .values()
            .filter_map(|state| {
                let remaining = state.record.expires_at.checked_duration_since(now)?;
                let created_elapsed = state.record.created_at.elapsed();
                Some(PersistedOAuthDeviceFlow {
                    target: state.record.target.clone(),
                    provider: state.record.provider.canonical_alias().to_string(),
                    device_code: state.record.device_code.clone(),
                    created_at_millis: now_millis
                        .saturating_sub(duration_millis_u64(created_elapsed)),
                    expires_at_millis: now_millis.saturating_add(duration_millis_u64(remaining)),
                })
            })
            .collect();

        OAuthFlowRegistrySnapshot { browser, device }
    }

    pub fn insert_restored_browser_flow(
        &self,
        state: String,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: String,
        pkce_verifier: String,
        created_at: Instant,
    ) -> Result<(), OAuthFlowError> {
        let mut flows = self.flows.lock();
        let device_flows = self.device_flows.lock();
        if flows.len() + device_flows.len() >= self.max_outstanding {
            return Err(OAuthFlowError::CapacityExceeded {
                max_outstanding: self.max_outstanding,
            });
        }
        flows.insert(
            state,
            OAuthFlowRecord {
                target,
                provider,
                redirect_uri,
                pkce_verifier,
                created_at,
            },
        );
        Ok(())
    }

    pub fn insert_restored_device_flow(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        device_code: String,
        created_at: Instant,
        expires_at: Instant,
    ) -> Result<(), OAuthFlowError> {
        let flows = self.flows.lock();
        let mut device_flows = self.device_flows.lock();
        if device_flows.contains_key(&device_code) {
            return Err(OAuthFlowError::DeviceCodeAlreadyAdmitted);
        }
        if flows.len() + device_flows.len() >= self.max_outstanding {
            return Err(OAuthFlowError::CapacityExceeded {
                max_outstanding: self.max_outstanding,
            });
        }
        let record = OAuthDeviceFlowRecord {
            target,
            provider,
            device_code: device_code.clone(),
            created_at,
            expires_at,
        };
        device_flows.insert(
            device_code,
            OAuthDeviceFlowState {
                record,
                poll_lease: None,
            },
        );
        Ok(())
    }

    pub fn start_with_pruned(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: String,
        pkce_verifier: String,
    ) -> Result<(String, OAuthPrunedFlows), OAuthFlowError> {
        let state = new_state_token()?;
        let record = OAuthFlowRecord {
            target,
            provider,
            redirect_uri,
            pkce_verifier,
            created_at: Instant::now(),
        };
        let mut flows = self.flows.lock();
        let mut device_flows = self.device_flows.lock();
        let expired_browser = take_expired_locked(&mut flows, self.ttl);
        let expired_device = take_expired_device_locked(&mut device_flows);
        if flows.len() + device_flows.len() >= self.max_outstanding {
            return Err(OAuthFlowError::CapacityExceeded {
                max_outstanding: self.max_outstanding,
            });
        }
        flows.insert(state.clone(), record);
        Ok((
            state,
            OAuthPrunedFlows::from_expired(expired_browser, expired_device),
        ))
    }

    pub fn insert_browser_flow_with_pruned(
        &self,
        state: String,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: String,
        pkce_verifier: String,
    ) -> Result<OAuthPrunedFlows, OAuthFlowError> {
        let record = OAuthFlowRecord {
            target,
            provider,
            redirect_uri,
            pkce_verifier,
            created_at: Instant::now(),
        };
        let mut flows = self.flows.lock();
        let mut device_flows = self.device_flows.lock();
        let expired_browser = take_expired_locked(&mut flows, self.ttl);
        let expired_device = take_expired_device_locked(&mut device_flows);
        if flows.len() + device_flows.len() >= self.max_outstanding {
            return Err(OAuthFlowError::CapacityExceeded {
                max_outstanding: self.max_outstanding,
            });
        }
        flows.insert(state, record);
        Ok(OAuthPrunedFlows::from_expired(
            expired_browser,
            expired_device,
        ))
    }

    pub fn admit_device_code_with_pruned(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        device_code: String,
        expires_in: Duration,
    ) -> Result<OAuthPrunedFlows, OAuthFlowError> {
        let mut flows = self.flows.lock();
        let mut device_flows = self.device_flows.lock();
        let expired_browser = take_expired_locked(&mut flows, self.ttl);
        let expired_device = take_expired_device_locked(&mut device_flows);
        if device_flows.contains_key(&device_code) {
            return Err(OAuthFlowError::DeviceCodeAlreadyAdmitted);
        }
        if flows.len() + device_flows.len() >= self.max_outstanding {
            return Err(OAuthFlowError::CapacityExceeded {
                max_outstanding: self.max_outstanding,
            });
        }
        let now = Instant::now();
        let expires_at = now
            .checked_add(expires_in)
            .ok_or(OAuthFlowError::DeviceExpiryOutOfRange)?;
        let record = OAuthDeviceFlowRecord {
            target,
            provider,
            device_code: device_code.clone(),
            created_at: now,
            expires_at,
        };
        device_flows.insert(
            device_code,
            OAuthDeviceFlowState {
                record,
                poll_lease: None,
            },
        );
        Ok(OAuthPrunedFlows::from_expired(
            expired_browser,
            expired_device,
        ))
    }

    pub fn admit_device_code_with_pruned_without_capacity(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        device_code: String,
        expires_in: Duration,
    ) -> Result<OAuthPrunedFlows, OAuthFlowError> {
        let mut flows = self.flows.lock();
        let mut device_flows = self.device_flows.lock();
        let expired_browser = take_expired_locked(&mut flows, self.ttl);
        let expired_device = take_expired_device_locked(&mut device_flows);
        if device_flows.contains_key(&device_code) {
            return Err(OAuthFlowError::DeviceCodeAlreadyAdmitted);
        }
        let now = Instant::now();
        let expires_at = now
            .checked_add(expires_in)
            .ok_or(OAuthFlowError::DeviceExpiryOutOfRange)?;
        let record = OAuthDeviceFlowRecord {
            target,
            provider,
            device_code: device_code.clone(),
            created_at: now,
            expires_at,
        };
        device_flows.insert(
            device_code,
            OAuthDeviceFlowState {
                record,
                poll_lease: None,
            },
        );
        Ok(OAuthPrunedFlows::from_expired(
            expired_browser,
            expired_device,
        ))
    }
}

impl Default for OAuthFlowRegistry {
    fn default() -> Self {
        Self::new(Duration::from_secs(10 * 60))
    }
}

impl OAuthFlowAuthority for OAuthFlowRegistry {
    fn start(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: String,
        pkce_verifier: String,
    ) -> Result<String, OAuthFlowError> {
        self.start_with_pruned(target, provider, redirect_uri, pkce_verifier)
            .map(|(state, _)| state)
    }

    fn verify(
        &self,
        state: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
    ) -> Result<OAuthFlowRecord, OAuthFlowError> {
        let mut flows = self.flows.lock();
        prune_expired_locked(&mut flows, self.ttl);
        let Some(record) = flows.get(state) else {
            return Err(OAuthFlowError::Missing);
        };
        verify_browser_record(record, target, provider, redirect_uri)?;
        Ok(record.clone())
    }

    fn consume(
        &self,
        state: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
    ) -> Result<OAuthFlowRecord, OAuthFlowError> {
        let mut flows = self.flows.lock();
        prune_expired_locked(&mut flows, self.ttl);
        let Some(record) = flows.get(state) else {
            return Err(OAuthFlowError::Missing);
        };
        verify_browser_record(record, target, provider, redirect_uri)?;
        flows.remove(state).ok_or(OAuthFlowError::Missing)
    }

    fn admit_device_code(
        &self,
        target: ConnectionRef,
        provider: OAuthProviderIdentity,
        device_code: String,
        expires_in: Duration,
    ) -> Result<(), OAuthFlowError> {
        self.admit_device_code_with_pruned(target, provider, device_code, expires_in)
            .map(|_| ())
    }

    fn verify_device_code(
        &self,
        device_code: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
    ) -> Result<OAuthDeviceFlowRecord, OAuthFlowError> {
        let mut flows = self.device_flows.lock();
        prune_expired_device_locked(&mut flows);
        let Some(record) = flows.get(device_code) else {
            return Err(OAuthFlowError::Missing);
        };
        verify_device_record(&record.record, target, provider)?;
        Ok(record.record.clone())
    }

    fn begin_device_code_poll(
        &self,
        device_code: &str,
        target: &ConnectionRef,
        provider: OAuthProviderIdentity,
    ) -> Result<OAuthDevicePollLease, OAuthFlowError> {
        let mut flows = self.device_flows.lock();
        prune_expired_device_locked(&mut flows);
        let Some(state) = flows.get_mut(device_code) else {
            return Err(OAuthFlowError::Missing);
        };
        verify_device_record(&state.record, target, provider)?;
        if state.poll_lease.is_some() {
            return Err(OAuthFlowError::DevicePollInProgress);
        }
        let lease_id = self
            .next_device_poll_lease_id
            .fetch_add(1, Ordering::Relaxed);
        state.poll_lease = Some(OAuthDevicePollLeaseState { id: lease_id });
        Ok(OAuthDevicePollLease::new(
            Arc::clone(&self.device_flows),
            target.clone(),
            device_code.to_string(),
            provider,
            lease_id,
        ))
    }
}

fn new_state_token() -> Result<String, OAuthFlowError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| OAuthFlowError::StateGenerationFailed)?;
    Ok(format!(
        "st-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

fn duration_millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn prune_expired_locked(flows: &mut HashMap<String, OAuthFlowRecord>, ttl: Duration) {
    let _ = take_expired_locked(flows, ttl);
}

fn take_expired_locked(
    flows: &mut HashMap<String, OAuthFlowRecord>,
    ttl: Duration,
) -> Vec<(String, OAuthFlowRecord)> {
    let expired = flows
        .iter()
        .filter(|(_, record)| record.created_at.elapsed() > ttl)
        .map(|(flow_id, _)| flow_id.clone())
        .collect::<Vec<_>>();
    expired
        .into_iter()
        .filter_map(|flow_id| flows.remove(&flow_id).map(|record| (flow_id, record)))
        .collect()
}

fn release_device_poll_lease_locked(
    flows: &mut HashMap<String, OAuthDeviceFlowState>,
    device_code: &str,
    provider: OAuthProviderIdentity,
    lease_id: u64,
) -> Result<(), OAuthFlowError> {
    let Some(state) = flows.get_mut(device_code) else {
        return Err(OAuthFlowError::Missing);
    };
    if state.record.provider != provider {
        return Err(OAuthFlowError::ProviderMismatch {
            expected: state.record.provider,
            actual: provider,
        });
    }
    match state.poll_lease {
        Some(lease) if lease.id == lease_id => {
            state.poll_lease = None;
            Ok(())
        }
        Some(_) => Err(OAuthFlowError::DevicePollInProgress),
        None => Err(OAuthFlowError::Missing),
    }
}

fn verify_browser_record(
    record: &OAuthFlowRecord,
    target: &ConnectionRef,
    provider: OAuthProviderIdentity,
    redirect_uri: &str,
) -> Result<(), OAuthFlowError> {
    if &record.target != target {
        return Err(OAuthFlowError::TargetMismatch {
            expected: Box::new(record.target.clone()),
            actual: Box::new(target.clone()),
        });
    }
    if record.provider != provider {
        return Err(OAuthFlowError::ProviderMismatch {
            expected: record.provider,
            actual: provider,
        });
    }
    if record.redirect_uri != redirect_uri {
        return Err(OAuthFlowError::RedirectUriMismatch);
    }
    Ok(())
}

fn verify_device_record(
    record: &OAuthDeviceFlowRecord,
    target: &ConnectionRef,
    provider: OAuthProviderIdentity,
) -> Result<(), OAuthFlowError> {
    if &record.target != target {
        return Err(OAuthFlowError::TargetMismatch {
            expected: Box::new(record.target.clone()),
            actual: Box::new(target.clone()),
        });
    }
    if record.provider != provider {
        return Err(OAuthFlowError::ProviderMismatch {
            expected: record.provider,
            actual: provider,
        });
    }
    Ok(())
}

fn verify_device_poll_lease_locked(
    flows: &mut HashMap<String, OAuthDeviceFlowState>,
    device_code: &str,
    provider: OAuthProviderIdentity,
    lease_id: u64,
) -> Result<OAuthDeviceFlowRecord, OAuthFlowError> {
    let Some(state) = flows.get(device_code) else {
        return Err(OAuthFlowError::Missing);
    };
    if state.record.provider != provider {
        return Err(OAuthFlowError::ProviderMismatch {
            expected: state.record.provider,
            actual: provider,
        });
    }
    match state.poll_lease {
        Some(lease) if lease.id == lease_id => {}
        Some(_) => return Err(OAuthFlowError::DevicePollInProgress),
        None => return Err(OAuthFlowError::Missing),
    }
    Ok(state.record.clone())
}

fn consume_device_poll_lease_locked(
    flows: &mut HashMap<String, OAuthDeviceFlowState>,
    device_code: &str,
    provider: OAuthProviderIdentity,
    lease_id: u64,
) -> Result<OAuthDeviceFlowRecord, OAuthFlowError> {
    verify_device_poll_lease_locked(flows, device_code, provider, lease_id)?;
    flows
        .remove(device_code)
        .map(|state| state.record)
        .ok_or(OAuthFlowError::Missing)
}

fn prune_expired_device_locked(flows: &mut HashMap<String, OAuthDeviceFlowState>) {
    let _ = take_expired_device_locked(flows);
}

fn take_expired_device_locked(
    flows: &mut HashMap<String, OAuthDeviceFlowState>,
) -> Vec<OAuthDeviceFlowRecord> {
    let now = Instant::now();
    let expired = flows
        .iter()
        .filter(|(_, state)| state.record.expires_at < now)
        .map(|(device_code, _)| device_code.clone())
        .collect::<Vec<_>>();
    expired
        .into_iter()
        .filter_map(|device_code| flows.remove(&device_code).map(|state| state.record))
        .collect()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn target() -> ConnectionRef {
        ConnectionRef {
            realm: meerkat_core::RealmId::parse("dev").expect("valid realm"),
            binding: meerkat_core::BindingId::parse("default_openai").expect("valid binding"),
            profile: None,
        }
    }

    fn alternate_target() -> ConnectionRef {
        ConnectionRef {
            realm: meerkat_core::RealmId::parse("dev").expect("valid realm"),
            binding: meerkat_core::BindingId::parse("alternate_openai").expect("valid binding"),
            profile: None,
        }
    }

    #[test]
    fn oauth_state_pkce_round_trip() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = registry
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier",
            )
            .expect("state generation succeeds");
        let record = registry.consume(
            &state,
            &target(),
            OAuthProviderIdentity::OpenAiChatGpt,
            "http://127.0.0.1/callback",
        );
        assert!(
            record.is_ok(),
            "state should resolve once: {:?}",
            record.err()
        );
        if let Ok(record) = record {
            assert_eq!(record.pkce_verifier, "verifier");
        }
        assert!(matches!(
            registry.consume(
                &state,
                &target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback"
            ),
            Err(OAuthFlowError::Missing)
        ));
    }

    #[test]
    fn oauth_state_rejects_mismatch() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = registry
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier",
            )
            .expect("state generation succeeds");
        assert!(matches!(
            registry.consume(
                &state,
                &target(),
                OAuthProviderIdentity::AnthropicClaudeAi,
                "http://127.0.0.1/callback"
            ),
            Err(OAuthFlowError::ProviderMismatch { .. })
        ));
    }

    #[test]
    fn oauth_state_provider_mismatch_does_not_consume_state() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = registry
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier",
            )
            .expect("state generation succeeds");
        assert!(matches!(
            registry.consume(
                &state,
                &target(),
                OAuthProviderIdentity::AnthropicClaudeAi,
                "http://127.0.0.1/callback"
            ),
            Err(OAuthFlowError::ProviderMismatch { .. })
        ));

        let record = registry
            .consume(
                &state,
                &target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
            )
            .expect("provider mismatch must leave state retryable");
        assert_eq!(record.pkce_verifier, "verifier");
    }

    #[test]
    fn oauth_state_rejects_redirect_uri_mismatch() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = registry
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier",
            )
            .expect("state generation succeeds");
        assert!(matches!(
            registry.consume(
                &state,
                &target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/other"
            ),
            Err(OAuthFlowError::RedirectUriMismatch)
        ));
    }

    #[test]
    fn oauth_state_redirect_uri_mismatch_does_not_consume_state() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = registry
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier",
            )
            .expect("state generation succeeds");
        assert!(matches!(
            registry.consume(
                &state,
                &target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/other"
            ),
            Err(OAuthFlowError::RedirectUriMismatch)
        ));

        let record = registry
            .consume(
                &state,
                &target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
            )
            .expect("redirect mismatch must leave state retryable");
        assert_eq!(record.pkce_verifier, "verifier");
    }

    #[test]
    fn oauth_state_target_mismatch_does_not_consume_state() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = registry
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier",
            )
            .expect("state generation succeeds");
        assert!(matches!(
            registry.consume(
                &state,
                &alternate_target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback"
            ),
            Err(OAuthFlowError::TargetMismatch { .. })
        ));

        let record = registry
            .consume(
                &state,
                &target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
            )
            .expect("target mismatch must leave state retryable");
        assert_eq!(record.pkce_verifier, "verifier");
    }

    #[test]
    fn oauth_state_verify_does_not_consume_state() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = registry
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier",
            )
            .expect("state generation succeeds");

        let verified = registry
            .verify(
                &state,
                &target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
            )
            .expect("verify should read state before terminal commit");

        assert_eq!(verified.pkce_verifier, "verifier");
        registry
            .consume(
                &state,
                &target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
            )
            .expect("verified browser state remains available for terminal consume");
    }

    #[test]
    fn oauth_state_random_tokens_are_urlsafe_and_unique() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let first = registry
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier-a",
            )
            .expect("state generation succeeds");
        let second = registry
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier-b",
            )
            .expect("state generation succeeds");
        assert_ne!(first, second);
        assert!(first.starts_with("st-"));
        assert!(
            first[3..]
                .chars()
                .all(|ch| { ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' })
        );
    }

    #[test]
    fn oauth_state_expired_records_are_pruned() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        registry.flows.lock().insert(
            "st-old".to_string(),
            OAuthFlowRecord {
                target: target(),
                provider: OAuthProviderIdentity::OpenAiChatGpt,
                redirect_uri: "http://127.0.0.1/callback".to_string(),
                pkce_verifier: "verifier".to_string(),
                created_at: Instant::now()
                    .checked_sub(Duration::from_secs(61))
                    .expect("test duration is representable"),
            },
        );
        assert!(matches!(
            registry.consume(
                "st-old",
                &target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback"
            ),
            Err(OAuthFlowError::Missing)
        ));
    }

    #[test]
    fn oauth_state_registry_rejects_start_at_capacity() {
        let registry = OAuthFlowRegistry::new_with_capacity(Duration::from_secs(60), 2);
        let first = registry
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "first",
            )
            .expect("state generation succeeds");
        let second = registry
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "second",
            )
            .expect("state generation succeeds");
        let third = registry.start(
            target(),
            OAuthProviderIdentity::OpenAiChatGpt,
            "http://127.0.0.1/callback",
            "third",
        );

        assert!(matches!(
            third,
            Err(OAuthFlowError::CapacityExceeded { max_outstanding: 2 })
        ));
        assert!(
            registry
                .consume(
                    &first,
                    &target(),
                    OAuthProviderIdentity::OpenAiChatGpt,
                    "http://127.0.0.1/callback"
                )
                .is_ok()
        );
        assert!(
            registry
                .consume(
                    &second,
                    &target(),
                    OAuthProviderIdentity::OpenAiChatGpt,
                    "http://127.0.0.1/callback"
                )
                .is_ok()
        );
    }

    #[test]
    fn oauth_state_cannot_cross_login_lifecycle_authorities() {
        let admitting_authority = OAuthFlowRegistry::new(Duration::from_secs(60));
        let unrelated_authority = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = admitting_authority
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier",
            )
            .expect("state generation succeeds");

        assert!(matches!(
            unrelated_authority.consume(
                &state,
                &target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback"
            ),
            Err(OAuthFlowError::Missing)
        ));

        let record = admitting_authority
            .consume(
                &state,
                &target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
            )
            .expect("admitting authority owns the flow");
        assert_eq!(record.pkce_verifier, "verifier");
    }

    #[test]
    fn oauth_device_flow_is_retained_until_terminal_consume() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        registry
            .admit_device_code(
                target(),
                OAuthProviderIdentity::GoogleCodeAssist,
                "device-code",
                Duration::from_secs(600),
            )
            .expect("device code admitted");

        let observed = registry
            .verify_device_code(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("pending device flow remains visible");
        assert_eq!(observed.device_code, "device-code");

        let poll = registry
            .begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("poll begins");
        let consumed = poll.consume().expect("terminal device flow consumes");
        assert_eq!(consumed.provider, OAuthProviderIdentity::GoogleCodeAssist);
        assert!(matches!(
            registry.verify_device_code(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist
            ),
            Err(OAuthFlowError::Missing)
        ));
    }

    #[test]
    fn oauth_device_poll_verify_keeps_terminal_consume_available() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        registry
            .admit_device_code(
                target(),
                OAuthProviderIdentity::GoogleCodeAssist,
                "device-code",
                Duration::from_secs(600),
            )
            .expect("device code admitted");
        let poll = registry
            .begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("poll begins");

        let verified = poll
            .verify()
            .expect("terminal preflight verifies the current lease");

        assert_eq!(verified.device_code, "device-code");
        assert!(matches!(
            registry.begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist
            ),
            Err(OAuthFlowError::DevicePollInProgress)
        ));
        let consumed = poll.consume().expect("verified lease still consumes");
        assert_eq!(consumed.device_code, "device-code");
    }

    #[test]
    fn oauth_device_admission_does_not_replace_active_poll_lease() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        registry
            .admit_device_code(
                target(),
                OAuthProviderIdentity::GoogleCodeAssist,
                "device-code",
                Duration::from_secs(600),
            )
            .expect("device code admitted");
        let poll = registry
            .begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("poll begins");

        let duplicate = registry.admit_device_code(
            target(),
            OAuthProviderIdentity::GoogleCodeAssist,
            "device-code",
            Duration::from_secs(600),
        );

        assert_eq!(duplicate, Err(OAuthFlowError::DeviceCodeAlreadyAdmitted));
        let consumed = poll
            .consume()
            .expect("duplicate admission must not replace the active poll lease");
        assert_eq!(consumed.device_code, "device-code");
    }

    #[test]
    fn oauth_device_flow_rejects_provider_mismatch() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        registry
            .admit_device_code(
                target(),
                OAuthProviderIdentity::GoogleCodeAssist,
                "device-code",
                Duration::from_secs(600),
            )
            .expect("device code admitted");

        assert!(matches!(
            registry.verify_device_code(
                "device-code",
                &target(),
                OAuthProviderIdentity::OpenAiChatGpt
            ),
            Err(OAuthFlowError::ProviderMismatch { .. })
        ));
        let poll = registry
            .begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("correct provider poll begins after mismatch");
        assert!(poll.consume().is_ok());
    }

    #[test]
    fn oauth_device_flow_rejects_target_mismatch_without_consuming() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        registry
            .admit_device_code(
                target(),
                OAuthProviderIdentity::GoogleCodeAssist,
                "device-code",
                Duration::from_secs(600),
            )
            .expect("device code admitted");

        assert!(matches!(
            registry.verify_device_code(
                "device-code",
                &alternate_target(),
                OAuthProviderIdentity::GoogleCodeAssist
            ),
            Err(OAuthFlowError::TargetMismatch { .. })
        ));
        let poll = registry
            .begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("correct target poll begins after mismatch");
        assert!(poll.consume().is_ok());
    }

    #[test]
    fn oauth_device_flow_expired_records_are_pruned() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        registry.device_flows.lock().insert(
            "device-code".to_string(),
            OAuthDeviceFlowState {
                record: OAuthDeviceFlowRecord {
                    target: target(),
                    provider: OAuthProviderIdentity::GoogleCodeAssist,
                    device_code: "device-code".to_string(),
                    created_at: Instant::now()
                        .checked_sub(Duration::from_secs(601))
                        .expect("test duration is representable"),
                    expires_at: Instant::now()
                        .checked_sub(Duration::from_secs(1))
                        .expect("test duration is representable"),
                },
                poll_lease: None,
            },
        );

        assert!(matches!(
            registry.verify_device_code(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist
            ),
            Err(OAuthFlowError::Missing)
        ));
    }

    #[test]
    fn oauth_device_flow_rejects_unrepresentable_expiry() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let err = registry
            .admit_device_code(
                target(),
                OAuthProviderIdentity::GoogleCodeAssist,
                "device-code",
                Duration::MAX,
            )
            .expect_err("unrepresentable device expiry should be rejected");

        assert_eq!(err, OAuthFlowError::DeviceExpiryOutOfRange);
        assert!(matches!(
            registry.verify_device_code(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist
            ),
            Err(OAuthFlowError::Missing)
        ));
    }

    #[test]
    fn oauth_device_terminal_consume_rejects_local_expiry_boundary() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        registry
            .admit_device_code(
                target(),
                OAuthProviderIdentity::GoogleCodeAssist,
                "device-code",
                Duration::from_secs(600),
            )
            .expect("device code admitted");
        let poll = registry
            .begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("poll begins before local expiry boundary");
        {
            let mut flows = registry.device_flows.lock();
            flows
                .get_mut("device-code")
                .expect("device flow exists")
                .record
                .expires_at = Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("test duration is representable");
        }

        assert!(matches!(poll.consume(), Err(OAuthFlowError::Missing)));
        assert!(matches!(
            registry.verify_device_code(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist
            ),
            Err(OAuthFlowError::Missing)
        ));
    }

    #[test]
    fn oauth_device_terminal_consume_rejects_intervening_prune_while_polling() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        registry
            .admit_device_code(
                target(),
                OAuthProviderIdentity::GoogleCodeAssist,
                "device-code",
                Duration::from_secs(600),
            )
            .expect("device code admitted");
        let poll = registry
            .begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("poll begins");
        assert!(matches!(
            registry.begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist
            ),
            Err(OAuthFlowError::DevicePollInProgress)
        ));
        {
            let mut flows = registry.device_flows.lock();
            flows
                .get_mut("device-code")
                .expect("device flow exists")
                .record
                .expires_at = Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("test duration is representable");
        }

        registry
            .start(
                target(),
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier",
            )
            .expect("unrelated start prunes expired device flow");
        assert!(matches!(poll.consume(), Err(OAuthFlowError::Missing)));
    }

    #[test]
    fn oauth_device_poll_drop_releases_in_progress_lifecycle() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        registry
            .admit_device_code(
                target(),
                OAuthProviderIdentity::GoogleCodeAssist,
                "device-code",
                Duration::from_secs(600),
            )
            .expect("device code admitted");

        let poll = registry
            .begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("poll begins");
        assert!(matches!(
            registry.begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist
            ),
            Err(OAuthFlowError::DevicePollInProgress)
        ));

        drop(poll);

        registry
            .begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("dropped poll lease releases the in-progress lifecycle");
    }

    #[test]
    fn oauth_device_poll_drop_prunes_expired_in_progress_record() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        registry
            .admit_device_code(
                target(),
                OAuthProviderIdentity::GoogleCodeAssist,
                "device-code",
                Duration::from_secs(600),
            )
            .expect("device code admitted");
        let poll = registry
            .begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("poll begins");
        {
            let mut flows = registry.device_flows.lock();
            flows
                .get_mut("device-code")
                .expect("device flow exists")
                .record
                .expires_at = Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("test duration is representable");
        }

        drop(poll);

        assert!(matches!(
            registry.verify_device_code(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist
            ),
            Err(OAuthFlowError::Missing)
        ));
    }

    struct RejectConsumeLifecycle;

    impl OAuthDevicePollLifecycle for RejectConsumeLifecycle {
        fn finish_device_poll(
            &self,
            _target: &ConnectionRef,
            _device_code: &str,
        ) -> Result<(), OAuthFlowError> {
            Ok(())
        }

        fn consume_device_flow(
            &self,
            _target: &ConnectionRef,
            _device_code: &str,
            _provider: OAuthProviderIdentity,
        ) -> Result<(), OAuthFlowError> {
            Err(OAuthFlowError::LifecycleRejected {
                operation: "consume_oauth_device_flow",
                detail: "injected failure".to_string(),
            })
        }

        fn expire_device_flow(
            &self,
            _target: &ConnectionRef,
            _device_code: &str,
        ) -> Result<(), OAuthFlowError> {
            Ok(())
        }
    }

    #[test]
    fn oauth_device_lifecycle_consume_failure_keeps_flow_retryable() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        registry
            .admit_device_code(
                target(),
                OAuthProviderIdentity::GoogleCodeAssist,
                "device-code",
                Duration::from_secs(600),
            )
            .expect("device code admitted");
        let poll = registry
            .begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("poll begins")
            .with_lifecycle(std::sync::Arc::new(RejectConsumeLifecycle));

        assert!(matches!(
            poll.consume(),
            Err(OAuthFlowError::LifecycleRejected {
                operation: "consume_oauth_device_flow",
                ..
            })
        ));

        let retry = registry
            .begin_device_code_poll(
                "device-code",
                &target(),
                OAuthProviderIdentity::GoogleCodeAssist,
            )
            .expect("failed lifecycle consume keeps device flow retryable");
        assert!(retry.consume().is_ok());
    }

    #[test]
    fn oauth_provider_resolution_preserves_aliases() {
        let cases = [
            (
                "anthropic",
                OAuthProviderIdentity::AnthropicClaudeAi,
                Provider::Anthropic,
                PersistedAuthMode::ClaudeAiOauth,
            ),
            (
                "claude",
                OAuthProviderIdentity::AnthropicClaudeAi,
                Provider::Anthropic,
                PersistedAuthMode::ClaudeAiOauth,
            ),
            (
                "claude.ai",
                OAuthProviderIdentity::AnthropicClaudeAi,
                Provider::Anthropic,
                PersistedAuthMode::ClaudeAiOauth,
            ),
            (
                "openai",
                OAuthProviderIdentity::OpenAiChatGpt,
                Provider::OpenAI,
                PersistedAuthMode::ChatgptOauth,
            ),
            (
                "chatgpt",
                OAuthProviderIdentity::OpenAiChatGpt,
                Provider::OpenAI,
                PersistedAuthMode::ChatgptOauth,
            ),
            (
                "google",
                OAuthProviderIdentity::GoogleCodeAssist,
                Provider::Gemini,
                PersistedAuthMode::GoogleOauth,
            ),
            (
                "gemini",
                OAuthProviderIdentity::GoogleCodeAssist,
                Provider::Gemini,
                PersistedAuthMode::GoogleOauth,
            ),
            (
                "code_assist",
                OAuthProviderIdentity::GoogleCodeAssist,
                Provider::Gemini,
                PersistedAuthMode::GoogleOauth,
            ),
        ];

        for (alias, identity, provider, auth_mode) in cases {
            let resolved =
                resolve_oauth_provider(alias, "http://127.0.0.1/callback").expect("alias resolves");
            assert_eq!(resolved.identity, identity);
            assert_eq!(resolved.provider, provider);
            assert_eq!(resolved.auth_mode, auth_mode);
            assert_eq!(resolved.endpoints.redirect_uri, "http://127.0.0.1/callback");
        }
    }

    #[test]
    fn oauth_provider_resolution_exposes_google_device_secret() {
        let resolved =
            resolve_oauth_provider("code_assist", "").expect("google code assist resolves");

        assert_eq!(resolved.identity, OAuthProviderIdentity::GoogleCodeAssist);
        assert!(resolved.endpoints.device_code_url.is_some());
        assert_eq!(resolved.client_secret, Some(GOOGLE_CLIENT_SECRET));
    }
}
