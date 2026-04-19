//! Unsigned JWT payload extraction.
//!
//! Reference-CLI parity: Codex `codex-rs/login/src/token_data.rs:71-160`
//! decodes the ID token's payload WITHOUT verifying the signature — the
//! bearer is already trusted (it came from the token endpoint over TLS
//! with a valid client credential). We only extract well-known claims:
//! `chatgpt_plan_type`, `chatgpt_user_id`, `chatgpt_account_id`,
//! `chatgpt_account_is_fedramp`, `email`, `sub`, `exp`.
//!
//! This module is intentionally zero-dep on `jsonwebtoken`: decoding the
//! payload is trivial base64 + JSON. Verification (which we don't need
//! for identity extraction of our own OAuth tokens) is added per-provider
//! under the `oauth` feature.

use base64::Engine;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Extracted well-known claims. Not all claims exist on all ID tokens;
/// unknown fields are ignored.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JwtClaims {
    #[serde(default)]
    pub sub: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub exp: Option<i64>,
    #[serde(default)]
    pub iat: Option<i64>,
    #[serde(default)]
    pub aud: Option<serde_json::Value>,
    #[serde(default)]
    pub iss: Option<String>,

    // OpenAI/ChatGPT claims (S5b).
    #[serde(default)]
    pub chatgpt_plan_type: Option<String>,
    #[serde(default)]
    pub chatgpt_user_id: Option<String>,
    #[serde(default)]
    pub chatgpt_account_id: Option<String>,
    #[serde(default)]
    pub chatgpt_account_is_fedramp: Option<bool>,

    /// Raw payload for provider-specific extraction when structured
    /// claims don't match.
    #[serde(skip)]
    pub raw: serde_json::Value,
}

#[derive(Debug, Error)]
pub enum JwtError {
    #[error("jwt must have exactly 3 segments, got {0}")]
    InvalidFormat(usize),
    #[error("payload base64 decode failed: {0}")]
    Base64(String),
    #[error("payload json parse failed: {0}")]
    Json(String),
}

/// Decode an unsigned JWT payload. Does NOT verify the signature.
pub fn decode_payload(jwt: &str) -> Result<JwtClaims, JwtError> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return Err(JwtError::InvalidFormat(parts.len()));
    }
    let payload_b64 = parts[1];
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload_b64))
        .map_err(|e| JwtError::Base64(e.to_string()))?;
    let raw: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| JwtError::Json(e.to_string()))?;
    let mut claims: JwtClaims =
        serde_json::from_value(raw.clone()).map_err(|e| JwtError::Json(e.to_string()))?;
    claims.raw = raw;
    Ok(claims)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn encode_jwt(payload: &serde_json::Value) -> String {
        let header = serde_json::json!({"alg": "none"});
        let h = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(&header).unwrap().as_bytes());
        let p = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_string(payload).unwrap().as_bytes());
        format!("{h}.{p}.signature")
    }

    #[test]
    fn decode_well_known_chatgpt_claims() {
        let payload = serde_json::json!({
            "sub": "user_123",
            "email": "a@b.com",
            "exp": 1_700_000_000,
            "chatgpt_plan_type": "pro",
            "chatgpt_user_id": "user_123",
            "chatgpt_account_id": "acct_abc",
            "chatgpt_account_is_fedramp": false,
        });
        let token = encode_jwt(&payload);
        let claims = decode_payload(&token).unwrap();
        assert_eq!(claims.sub.as_deref(), Some("user_123"));
        assert_eq!(claims.email.as_deref(), Some("a@b.com"));
        assert_eq!(claims.exp, Some(1_700_000_000));
        assert_eq!(claims.chatgpt_plan_type.as_deref(), Some("pro"));
        assert_eq!(claims.chatgpt_account_id.as_deref(), Some("acct_abc"));
        assert_eq!(claims.chatgpt_account_is_fedramp, Some(false));
    }

    #[test]
    fn decode_ignores_unknown_claims() {
        let payload = serde_json::json!({
            "sub": "u",
            "custom_x": [1, 2, 3],
            "another": "y",
        });
        let token = encode_jwt(&payload);
        let claims = decode_payload(&token).unwrap();
        assert_eq!(claims.sub.as_deref(), Some("u"));
        assert_eq!(claims.raw["custom_x"], serde_json::json!([1, 2, 3]));
    }

    #[test]
    fn rejects_invalid_format() {
        let err = decode_payload("not.a.valid.jwt").unwrap_err();
        assert!(matches!(err, JwtError::InvalidFormat(4)));
        let err = decode_payload("single").unwrap_err();
        assert!(matches!(err, JwtError::InvalidFormat(1)));
    }
}
