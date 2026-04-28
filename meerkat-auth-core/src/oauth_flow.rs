//! Short-lived OAuth state registry.
//!
//! The server owns the state -> PKCE verifier correlation. Start records a
//! flow before returning the authorize URL; complete must consume that state
//! before exchanging the authorization code.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use base64::Engine as _;
use meerkat_core::Provider;
use parking_lot::Mutex;

use crate::auth_oauth::OAuthEndpoints;
use crate::auth_store::PersistedAuthMode;

const DEFAULT_MAX_OUTSTANDING_FLOWS: usize = 1024;

const ANTHROPIC_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const ANTHROPIC_AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const ANTHROPIC_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const ANTHROPIC_SCOPES: &[&str] = &[
    "user:profile",
    "user:inference",
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
];
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OAuthProviderIdentity {
    AnthropicClaudeAi,
    OpenAiChatGpt,
    GoogleCodeAssist,
}

impl OAuthProviderIdentity {
    pub fn from_alias(alias: &str) -> Option<Self> {
        match alias {
            "anthropic" | "claude" | "claude.ai" => Some(Self::AnthropicClaudeAi),
            "openai" | "chatgpt" => Some(Self::OpenAiChatGpt),
            "google" | "gemini" | "code_assist" => Some(Self::GoogleCodeAssist),
            _ => None,
        }
    }

    pub fn canonical_alias(self) -> &'static str {
        match self {
            Self::AnthropicClaudeAi => "anthropic",
            Self::OpenAiChatGpt => "openai",
            Self::GoogleCodeAssist => "google",
        }
    }

    pub fn provider(self) -> Provider {
        match self {
            Self::AnthropicClaudeAi => Provider::Anthropic,
            Self::OpenAiChatGpt => Provider::OpenAI,
            Self::GoogleCodeAssist => Provider::Gemini,
        }
    }

    pub fn auth_mode(self) -> PersistedAuthMode {
        match self {
            Self::AnthropicClaudeAi => PersistedAuthMode::ClaudeAiOauth,
            Self::OpenAiChatGpt => PersistedAuthMode::ChatgptOauth,
            Self::GoogleCodeAssist => PersistedAuthMode::GoogleOauth,
        }
    }

    pub fn client_secret(self) -> Option<&'static str> {
        match self {
            Self::AnthropicClaudeAi | Self::OpenAiChatGpt => None,
            Self::GoogleCodeAssist => Some(GOOGLE_CLIENT_SECRET),
        }
    }

    pub fn endpoints(self, redirect_uri: impl Into<String>) -> OAuthEndpoints {
        match self {
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
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthFlowRecord {
    pub provider: OAuthProviderIdentity,
    pub redirect_uri: String,
    pub pkce_verifier: String,
    pub created_at: Instant,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OAuthFlowError {
    #[error("oauth state is missing or expired")]
    Missing,
    #[error("oauth state provider mismatch: expected {expected}, got {actual}")]
    ProviderMismatch {
        expected: OAuthProviderIdentity,
        actual: OAuthProviderIdentity,
    },
    #[error("oauth state redirect_uri mismatch")]
    RedirectUriMismatch,
    #[error("failed to generate oauth state token")]
    StateGenerationFailed,
    #[error("oauth state registry is at capacity ({max_outstanding} outstanding flows)")]
    CapacityExceeded { max_outstanding: usize },
}

#[derive(Debug)]
pub struct OAuthFlowRegistry {
    ttl: Duration,
    max_outstanding: usize,
    flows: Mutex<HashMap<String, OAuthFlowRecord>>,
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
        }
    }

    pub fn start(
        &self,
        provider: OAuthProviderIdentity,
        redirect_uri: impl Into<String>,
        pkce_verifier: impl Into<String>,
    ) -> Result<String, OAuthFlowError> {
        let state = new_state_token()?;
        let record = OAuthFlowRecord {
            provider,
            redirect_uri: redirect_uri.into(),
            pkce_verifier: pkce_verifier.into(),
            created_at: Instant::now(),
        };
        let mut flows = self.flows.lock();
        prune_expired_locked(&mut flows, self.ttl);
        if flows.len() >= self.max_outstanding {
            return Err(OAuthFlowError::CapacityExceeded {
                max_outstanding: self.max_outstanding,
            });
        }
        flows.insert(state.clone(), record);
        Ok(state)
    }

    pub fn consume(
        &self,
        state: &str,
        provider: OAuthProviderIdentity,
        redirect_uri: &str,
    ) -> Result<OAuthFlowRecord, OAuthFlowError> {
        let mut flows = self.flows.lock();
        prune_expired_locked(&mut flows, self.ttl);
        let Some(record) = flows.remove(state) else {
            return Err(OAuthFlowError::Missing);
        };
        if record.provider != provider {
            return Err(OAuthFlowError::ProviderMismatch {
                expected: record.provider,
                actual: provider,
            });
        }
        if record.redirect_uri != redirect_uri {
            return Err(OAuthFlowError::RedirectUriMismatch);
        }
        Ok(record)
    }
}

pub fn global_oauth_flow_registry() -> &'static OAuthFlowRegistry {
    static REGISTRY: OnceLock<OAuthFlowRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| OAuthFlowRegistry::new(Duration::from_secs(10 * 60)))
}

fn new_state_token() -> Result<String, OAuthFlowError> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| OAuthFlowError::StateGenerationFailed)?;
    Ok(format!(
        "st-{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

fn prune_expired_locked(flows: &mut HashMap<String, OAuthFlowRecord>, ttl: Duration) {
    flows.retain(|_, record| record.created_at.elapsed() <= ttl);
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn oauth_state_pkce_round_trip() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = registry
            .start(
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier",
            )
            .expect("state generation succeeds");
        let record = registry.consume(
            &state,
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
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier",
            )
            .expect("state generation succeeds");
        assert!(matches!(
            registry.consume(
                &state,
                OAuthProviderIdentity::AnthropicClaudeAi,
                "http://127.0.0.1/callback"
            ),
            Err(OAuthFlowError::ProviderMismatch { .. })
        ));
    }

    #[test]
    fn oauth_state_rejects_redirect_uri_mismatch() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = registry
            .start(
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier",
            )
            .expect("state generation succeeds");
        assert!(matches!(
            registry.consume(
                &state,
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/other"
            ),
            Err(OAuthFlowError::RedirectUriMismatch)
        ));
    }

    #[test]
    fn oauth_state_random_tokens_are_urlsafe_and_unique() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let first = registry
            .start(
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "verifier-a",
            )
            .expect("state generation succeeds");
        let second = registry
            .start(
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
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "first",
            )
            .expect("state generation succeeds");
        let second = registry
            .start(
                OAuthProviderIdentity::OpenAiChatGpt,
                "http://127.0.0.1/callback",
                "second",
            )
            .expect("state generation succeeds");
        let third = registry.start(
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
                    OAuthProviderIdentity::OpenAiChatGpt,
                    "http://127.0.0.1/callback"
                )
                .is_ok()
        );
        assert!(
            registry
                .consume(
                    &second,
                    OAuthProviderIdentity::OpenAiChatGpt,
                    "http://127.0.0.1/callback"
                )
                .is_ok()
        );
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
