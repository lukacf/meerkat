use crate::digest::MobpackDigest;
use crate::vocabulary::{Ed25519PublicKeyHex, Ed25519SignatureHex, Rfc3339Timestamp, SignerId};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSignature {
    pub signer_id: SignerId,
    pub public_key: Ed25519PublicKeyHex,
    pub digest: MobpackDigest,
    pub signature: Ed25519SignatureHex,
    pub timestamp: Rfc3339Timestamp,
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
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let digest = MobpackDigest::from_str(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        let sig = PackSignature {
            signer_id: SignerId::from_str("ci").unwrap(),
            public_key: Ed25519PublicKeyHex::from_verifying_key(signing_key.verifying_key()),
            digest,
            signature: Ed25519SignatureHex::from_signature(sign_digest(&signing_key, digest)),
            timestamp: Rfc3339Timestamp::from_str("2026-02-24T00:00:00Z").unwrap(),
        };

        let encoded = toml::to_string(&sig).unwrap();
        let decoded: PackSignature = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, sig);
    }

    #[test]
    fn test_signature_fields_fail_closed_on_malformed_hex() {
        // Malformed public_key / signature hex is rejected at deserialization,
        // never surviving as an opaque string that fails only at verify time.
        let bad_key = r#"
signer_id = "ci"
public_key = "nothex"
digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
signature = "deadbeef"
timestamp = "2026-02-24T00:00:00Z"
"#;
        assert!(toml::from_str::<PackSignature>(bad_key).is_err());

        let bad_sig = r#"
signer_id = "ci"
public_key = "0000000000000000000000000000000000000000000000000000000000000000"
digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
signature = "zz"
timestamp = "2026-02-24T00:00:00Z"
"#;
        assert!(toml::from_str::<PackSignature>(bad_sig).is_err());
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
