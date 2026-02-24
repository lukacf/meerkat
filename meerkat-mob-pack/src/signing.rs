use crate::digest::MobpackDigest;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSignature {
    pub signer_id: String,
    pub public_key: String,
    pub digest: MobpackDigest,
    pub signature: String,
    pub timestamp: String,
}

pub fn sign_digest(signing_key: &SigningKey, digest: MobpackDigest) -> Signature {
    signing_key.sign(digest.as_bytes())
}

pub fn verify_digest(
    verifying_key: &VerifyingKey,
    digest: MobpackDigest,
    signature: &Signature,
) -> Result<(), ed25519_dalek::SignatureError> {
    verifying_key.verify(digest.as_bytes(), signature)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_signature_toml_roundtrip() {
        let sig = PackSignature {
            signer_id: "ci".to_string(),
            public_key: "abcdef".to_string(),
            digest: MobpackDigest::from_str(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            signature: "deadbeef".to_string(),
            timestamp: "2026-02-24T00:00:00Z".to_string(),
        };

        let encoded = toml::to_string(&sig).unwrap();
        let decoded: PackSignature = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, sig);
    }

    #[test]
    fn test_ed25519_sign_verify_roundtrip() {
        let signing_key_a = SigningKey::from_bytes(&[7u8; 32]);
        let signing_key_b = SigningKey::from_bytes(&[8u8; 32]);
        let digest = MobpackDigest::from_bytes([3u8; 32]);

        let sig = sign_digest(&signing_key_a, digest);
        assert!(verify_digest(&signing_key_a.verifying_key(), digest, &sig).is_ok());
        assert!(verify_digest(&signing_key_b.verifying_key(), digest, &sig).is_err());
    }
}
