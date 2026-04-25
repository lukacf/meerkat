//! Short-lived OAuth state registry.
//!
//! The server owns the state -> PKCE verifier correlation. Start records a
//! flow before returning the authorize URL; complete must consume that state
//! before exchanging the authorization code.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use base64::Engine as _;
use parking_lot::Mutex;

const DEFAULT_MAX_OUTSTANDING_FLOWS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthFlowRecord {
    pub provider: String,
    pub redirect_uri: String,
    pub pkce_verifier: String,
    pub created_at: Instant,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OAuthFlowError {
    #[error("oauth state is missing or expired")]
    Missing,
    #[error("oauth state provider mismatch: expected {expected}, got {actual}")]
    ProviderMismatch { expected: String, actual: String },
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
        provider: impl Into<String>,
        redirect_uri: impl Into<String>,
        pkce_verifier: impl Into<String>,
    ) -> Result<String, OAuthFlowError> {
        let state = new_state_token()?;
        let record = OAuthFlowRecord {
            provider: provider.into(),
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
        provider: &str,
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
                actual: provider.to_string(),
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

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn oauth_state_pkce_round_trip() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = registry
            .start("openai", "http://127.0.0.1/callback", "verifier")
            .expect("state generation succeeds");
        let record = registry.consume(&state, "openai", "http://127.0.0.1/callback");
        assert!(
            record.is_ok(),
            "state should resolve once: {:?}",
            record.err()
        );
        if let Ok(record) = record {
            assert_eq!(record.pkce_verifier, "verifier");
        }
        assert!(matches!(
            registry.consume(&state, "openai", "http://127.0.0.1/callback"),
            Err(OAuthFlowError::Missing)
        ));
    }

    #[test]
    fn oauth_state_rejects_mismatch() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = registry
            .start("openai", "http://127.0.0.1/callback", "verifier")
            .expect("state generation succeeds");
        assert!(matches!(
            registry.consume(&state, "anthropic", "http://127.0.0.1/callback"),
            Err(OAuthFlowError::ProviderMismatch { .. })
        ));
    }

    #[test]
    fn oauth_state_rejects_redirect_uri_mismatch() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let state = registry
            .start("openai", "http://127.0.0.1/callback", "verifier")
            .expect("state generation succeeds");
        assert!(matches!(
            registry.consume(&state, "openai", "http://127.0.0.1/other"),
            Err(OAuthFlowError::RedirectUriMismatch)
        ));
    }

    #[test]
    fn oauth_state_random_tokens_are_urlsafe_and_unique() {
        let registry = OAuthFlowRegistry::new(Duration::from_secs(60));
        let first = registry
            .start("openai", "http://127.0.0.1/callback", "verifier-a")
            .expect("state generation succeeds");
        let second = registry
            .start("openai", "http://127.0.0.1/callback", "verifier-b")
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
                provider: "openai".to_string(),
                redirect_uri: "http://127.0.0.1/callback".to_string(),
                pkce_verifier: "verifier".to_string(),
                created_at: Instant::now()
                    .checked_sub(Duration::from_secs(61))
                    .expect("test duration is representable"),
            },
        );
        assert!(matches!(
            registry.consume("st-old", "openai", "http://127.0.0.1/callback"),
            Err(OAuthFlowError::Missing)
        ));
    }

    #[test]
    fn oauth_state_registry_rejects_start_at_capacity() {
        let registry = OAuthFlowRegistry::new_with_capacity(Duration::from_secs(60), 2);
        let first = registry
            .start("openai", "http://127.0.0.1/callback", "first")
            .expect("state generation succeeds");
        let second = registry
            .start("openai", "http://127.0.0.1/callback", "second")
            .expect("state generation succeeds");
        let third = registry.start("openai", "http://127.0.0.1/callback", "third");

        assert!(matches!(
            third,
            Err(OAuthFlowError::CapacityExceeded { max_outstanding: 2 })
        ));
        assert!(
            registry
                .consume(&first, "openai", "http://127.0.0.1/callback")
                .is_ok()
        );
        assert!(
            registry
                .consume(&second, "openai", "http://127.0.0.1/callback")
                .is_ok()
        );
    }
}
