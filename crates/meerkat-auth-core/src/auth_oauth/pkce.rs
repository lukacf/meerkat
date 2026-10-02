//! PKCE S256 code-challenge generator.
//!
//! RFC 7636. The verifier is a random 32-43 byte URL-safe string; the
//! challenge is `base64url(sha256(verifier))`. Used by both the ChatGPT
//! OAuth flow (Codex `codex-rs/login/src/server.rs`) and Claude.ai OAuth
//! (Claude Code `src/services/oauth/client.ts`).

use oauth2::{PkceCodeChallenge, PkceCodeVerifier};

/// PKCE pair: the challenge is sent to the authorize URL; the verifier is
/// sent back to the token endpoint to prove possession.
pub struct PkcePair {
    pub verifier: PkceCodeVerifier,
    pub challenge: PkceChallenge,
}

pub struct PkceChallenge {
    pub code: String,
    pub method: &'static str,
}

impl PkcePair {
    /// Generate a fresh PKCE pair using S256 (the only method any of the
    /// reference providers accept).
    pub fn generate_s256() -> Self {
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        Self {
            verifier,
            challenge: PkceChallenge {
                code: challenge.as_str().to_owned(),
                method: "S256",
            },
        }
    }
}

impl PkceChallenge {
    /// The S256 challenge of an existing verifier, for re-projecting an
    /// admitted attempt without minting a new one.
    pub fn s256_for_verifier(verifier: &str) -> Self {
        let challenge = PkceCodeChallenge::from_code_verifier_sha256(&PkceCodeVerifier::new(
            verifier.to_owned(),
        ));
        Self {
            code: challenge.as_str().to_owned(),
            method: "S256",
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn s256_generates_nonempty_fields() {
        let pair = PkcePair::generate_s256();
        assert!(!pair.verifier.secret().is_empty());
        assert!(!pair.challenge.code.is_empty());
        assert_eq!(pair.challenge.method, "S256");
    }

    #[test]
    fn challenge_for_verifier_matches_generated_pair() {
        let pair = PkcePair::generate_s256();
        let again = PkceChallenge::s256_for_verifier(pair.verifier.secret());
        assert_eq!(again.code, pair.challenge.code);
        assert_eq!(again.method, "S256");
    }

    #[test]
    fn two_generated_pairs_differ() {
        let a = PkcePair::generate_s256();
        let b = PkcePair::generate_s256();
        assert_ne!(a.verifier.secret(), b.verifier.secret());
        assert_ne!(a.challenge.code, b.challenge.code);
    }
}
