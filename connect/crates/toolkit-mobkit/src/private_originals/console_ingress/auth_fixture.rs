//! Controlled public fixture signing material; verification remains in Meta.
use super::*;
use meerkat_core::{PrincipalKind, auth::principal::TrustDomainId};
use sha2::{Digest, Sha256};
// Meta deliberately permits HS256 only for development loopback issuers.
// This is a fixture issuer, not a relaxation of the configured verifier.
pub(crate) const ISSUER: &str = "http://controlled-console.localhost";
pub(crate) const SUBJECT: &str = "controlled-subject";
pub(crate) const EMAIL: &str = "different-display@example.invalid";
pub(crate) const SCOPE: &str = "controlled-login-a";
pub(crate) const AUDIENCE: &str = "controlled-console";
// Public fixture material only. No deployed signing key or user credential.
pub(crate) const KEY: &[u8] = b"controlled-console-fixture-key-only";

pub(crate) fn selected_human() -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        SUBJECT,
        TrustDomainId::new(format!("oidc-issuer:{ISSUER}")).unwrap(),
    )
    .unwrap()
}

// Minimal fixture encoding uses the target's existing sha2 dependency. Native
// verification is always performed by Meta; this is not a second verifier.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut result = String::new();
    for chunk in bytes.chunks(3) {
        result.push(ALPHABET[(chunk[0] >> 2) as usize] as char);
        result.push(
            ALPHABET[(((chunk[0] & 3) << 4) | chunk.get(1).copied().unwrap_or(0) >> 4) as usize]
                as char,
        );
        if chunk.len() > 1 {
            result.push(
                ALPHABET
                    [(((chunk[1] & 15) << 2) | chunk.get(2).copied().unwrap_or(0) >> 6) as usize]
                    as char,
            );
        }
        if chunk.len() > 2 {
            result.push(ALPHABET[(chunk[2] & 63) as usize] as char);
        }
    }
    result
}
pub(crate) fn signed_fixture(issuer: &str, scope: &str, expires: u64) -> String {
    let header = base64url(br#"{"alg":"HS256","typ":"JWT"}"#);
    let claims = base64url(
        &serde_json::to_vec(&serde_json::json!({
            "iss": issuer, "aud": AUDIENCE, "sub": SUBJECT, "email": EMAIL,
            "email_verified": true, "sid": scope, "exp": expires,
        }))
        .unwrap(),
    );
    let unsigned = format!("{header}.{claims}");
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    assert!(KEY.len() <= inner_pad.len());
    for (index, byte) in KEY.iter().enumerate() {
        inner_pad[index] ^= *byte;
        outer_pad[index] ^= *byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(unsigned.as_bytes());
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner.finalize());
    format!("Bearer {unsigned}.{}", base64url(&outer.finalize()))
}
