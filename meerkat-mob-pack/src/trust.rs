use crate::digest::MobpackDigest;
use crate::signing::PackSignature;
use crate::validate::PackValidationError;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TrustPolicy {
    #[default]
    Permissive,
    Strict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TrustedSigners {
    #[serde(default)]
    pub signers: BTreeMap<String, String>,
}

impl TrustedSigners {
    pub fn lookup(&self, signer_id: &str) -> Option<&str> {
        self.signers.get(signer_id).map(String::as_str)
    }
}

pub fn load_trusted_signers(
    user_path: &Path,
    project_path: &Path,
) -> Result<TrustedSigners, PackValidationError> {
    let mut merged = TrustedSigners::default();
    if user_path.exists() {
        let user_text = std::fs::read_to_string(user_path)?;
        let user_store: TrustedSigners = toml::from_str(&user_text)
            .map_err(|err| PackValidationError::Archive(err.to_string()))?;
        merged.signers.extend(user_store.signers);
    }
    if project_path.exists() {
        let project_text = std::fs::read_to_string(project_path)?;
        let project_store: TrustedSigners = toml::from_str(&project_text)
            .map_err(|err| PackValidationError::Archive(err.to_string()))?;
        for (signer_id, project_key) in project_store.signers {
            if let Some(user_key) = merged.signers.get(&signer_id)
                && user_key != &project_key
            {
                eprintln!(
                    "warning: project trust store overrides signer '{signer_id}' from user trust store"
                );
            }
            merged.signers.insert(signer_id, project_key);
        }
    }
    Ok(merged)
}

pub fn verify_pack_trust(
    files: &BTreeMap<String, Vec<u8>>,
    digest: MobpackDigest,
    trust_policy: TrustPolicy,
    trusted_signers: &TrustedSigners,
) -> Result<Vec<String>, PackValidationError> {
    let mut warnings = Vec::new();
    let Some(signature_bytes) = files.get("signature.toml") else {
        if trust_policy == TrustPolicy::Strict {
            return Err(PackValidationError::UnsignedStrict);
        }
        warnings.push("unsigned pack accepted in permissive mode".to_string());
        return Ok(warnings);
    };

    let signature_text = String::from_utf8(signature_bytes.clone())
        .map_err(|err| PackValidationError::InvalidSignature(err.to_string()))?;
    let pack_signature: PackSignature = toml::from_str(&signature_text)
        .map_err(|err| PackValidationError::InvalidSignature(err.to_string()))?;
    if pack_signature.digest != digest {
        return Err(PackValidationError::SignatureDigestMismatch);
    }
    let embedded_key = parse_verifying_key(&pack_signature.public_key)?;
    let sig = parse_signature(&pack_signature.signature)?;

    if let Some(trusted_hex) = trusted_signers.lookup(&pack_signature.signer_id) {
        let trusted_key = parse_verifying_key(trusted_hex)?;
        if trusted_key.to_bytes() != embedded_key.to_bytes() {
            return Err(PackValidationError::SignerKeyMismatch(
                pack_signature.signer_id,
            ));
        }
        trusted_key
            .verify(digest.as_bytes(), &sig)
            .map_err(|err| PackValidationError::InvalidSignature(err.to_string()))?;
        return Ok(warnings);
    }

    embedded_key
        .verify(digest.as_bytes(), &sig)
        .map_err(|err| PackValidationError::InvalidSignature(err.to_string()))?;

    if trust_policy == TrustPolicy::Strict {
        return Err(PackValidationError::UnknownSignerStrict(
            pack_signature.signer_id,
        ));
    }
    warnings.push("signature valid but signer is unknown in permissive mode".to_string());
    Ok(warnings)
}

fn parse_verifying_key(hex_key: &str) -> Result<VerifyingKey, PackValidationError> {
    let bytes: [u8; 32] = hex::decode(hex_key)
        .map_err(|err| PackValidationError::InvalidSignature(err.to_string()))?
        .try_into()
        .map_err(|_| {
            PackValidationError::InvalidSignature("expected 32-byte public key".to_string())
        })?;
    VerifyingKey::from_bytes(&bytes)
        .map_err(|err| PackValidationError::InvalidSignature(err.to_string()))
}

fn parse_signature(hex_sig: &str) -> Result<Signature, PackValidationError> {
    let bytes: [u8; 64] = hex::decode(hex_sig)
        .map_err(|err| PackValidationError::InvalidSignature(err.to_string()))?
        .try_into()
        .map_err(|_| {
            PackValidationError::InvalidSignature("expected 64-byte signature".to_string())
        })?;
    Ok(Signature::from_bytes(&bytes))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::digest::MobpackDigest;
    use ed25519_dalek::{Signer, SigningKey};
    use std::str::FromStr;
    use tempfile::TempDir;

    #[test]
    fn test_trust_store_toml_roundtrip() {
        let store = TrustedSigners {
            signers: BTreeMap::from([
                ("ci".to_string(), "abcd1234".to_string()),
                ("release".to_string(), "beefcafe".to_string()),
            ]),
        };

        let encoded = toml::to_string(&store).unwrap();
        let decoded: TrustedSigners = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, store);
        assert_eq!(decoded.lookup("ci"), Some("abcd1234"));
    }

    #[test]
    fn test_trust_store_merges_user_and_project() {
        let temp = TempDir::new().unwrap();
        let user_path = temp.path().join("user.toml");
        let project_path = temp.path().join("project.toml");
        std::fs::write(
            &user_path,
            "[signers]\nci = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\n",
        )
        .unwrap();
        std::fs::write(
            &project_path,
            "[signers]\nci = \"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\"\nrelease = \"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\"\n",
        )
        .unwrap();

        let merged = load_trusted_signers(&user_path, &project_path).unwrap();
        assert_eq!(
            merged.lookup("ci"),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(
            merged.lookup("release"),
            Some("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc")
        );
    }

    #[test]
    fn test_strict_rejects_unsigned() {
        let files = BTreeMap::new();
        let err = verify_pack_trust(
            &files,
            MobpackDigest::from_str(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            TrustPolicy::Strict,
            &TrustedSigners::default(),
        )
        .unwrap_err();
        assert!(matches!(err, PackValidationError::UnsignedStrict));
    }

    #[test]
    fn test_permissive_warns_unsigned() {
        let warnings = verify_pack_trust(
            &BTreeMap::new(),
            MobpackDigest::from_str(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )
            .unwrap(),
            TrustPolicy::Permissive,
            &TrustedSigners::default(),
        )
        .unwrap();
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn test_strict_verifies_signed_against_trust_store() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let digest = MobpackDigest::from_bytes([3u8; 32]);
        let signature = signing_key.sign(digest.as_bytes());
        let signature_doc = toml::to_string(&PackSignature {
            signer_id: "ci".to_string(),
            public_key: hex::encode(signing_key.verifying_key().to_bytes()),
            digest,
            signature: hex::encode(signature.to_bytes()),
            timestamp: "2026-02-24T00:00:00Z".to_string(),
        })
        .unwrap();
        let files = BTreeMap::from([("signature.toml".to_string(), signature_doc.into_bytes())]);
        let trusted = TrustedSigners {
            signers: BTreeMap::from([(
                "ci".to_string(),
                hex::encode(signing_key.verifying_key().to_bytes()),
            )]),
        };
        verify_pack_trust(&files, digest, TrustPolicy::Strict, &trusted).unwrap();
    }

    #[test]
    fn test_strict_rejects_unknown_signer() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let digest = MobpackDigest::from_bytes([4u8; 32]);
        let signature = signing_key.sign(digest.as_bytes());
        let signature_doc = toml::to_string(&PackSignature {
            signer_id: "ci".to_string(),
            public_key: hex::encode(signing_key.verifying_key().to_bytes()),
            digest,
            signature: hex::encode(signature.to_bytes()),
            timestamp: "2026-02-24T00:00:00Z".to_string(),
        })
        .unwrap();
        let files = BTreeMap::from([("signature.toml".to_string(), signature_doc.into_bytes())]);
        let err = verify_pack_trust(
            &files,
            digest,
            TrustPolicy::Strict,
            &TrustedSigners::default(),
        )
        .unwrap_err();
        assert!(matches!(err, PackValidationError::UnknownSignerStrict(_)));
    }

    #[test]
    fn test_strict_rejects_mismatched_embedded_key() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let trusted_key = SigningKey::from_bytes(&[8u8; 32]);
        let digest = MobpackDigest::from_bytes([5u8; 32]);
        let signature = signing_key.sign(digest.as_bytes());
        let signature_doc = toml::to_string(&PackSignature {
            signer_id: "ci".to_string(),
            public_key: hex::encode(signing_key.verifying_key().to_bytes()),
            digest,
            signature: hex::encode(signature.to_bytes()),
            timestamp: "2026-02-24T00:00:00Z".to_string(),
        })
        .unwrap();
        let files = BTreeMap::from([("signature.toml".to_string(), signature_doc.into_bytes())]);
        let trusted = TrustedSigners {
            signers: BTreeMap::from([(
                "ci".to_string(),
                hex::encode(trusted_key.verifying_key().to_bytes()),
            )]),
        };
        let err = verify_pack_trust(&files, digest, TrustPolicy::Strict, &trusted).unwrap_err();
        assert!(matches!(err, PackValidationError::SignerKeyMismatch(_)));
    }

    #[test]
    fn test_permissive_rejects_bad_signature() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let digest = MobpackDigest::from_bytes([6u8; 32]);
        let mut signature_bytes = signing_key.sign(digest.as_bytes()).to_bytes();
        signature_bytes[0] ^= 0xFF;
        let signature_doc = toml::to_string(&PackSignature {
            signer_id: "ci".to_string(),
            public_key: hex::encode(signing_key.verifying_key().to_bytes()),
            digest,
            signature: hex::encode(signature_bytes),
            timestamp: "2026-02-24T00:00:00Z".to_string(),
        })
        .unwrap();
        let files = BTreeMap::from([("signature.toml".to_string(), signature_doc.into_bytes())]);
        let err = verify_pack_trust(
            &files,
            digest,
            TrustPolicy::Permissive,
            &TrustedSigners::default(),
        )
        .unwrap_err();
        assert!(matches!(err, PackValidationError::InvalidSignature(_)));
    }

    #[test]
    fn test_permissive_warns_unknown_signer() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let digest = MobpackDigest::from_bytes([7u8; 32]);
        let signature = signing_key.sign(digest.as_bytes());
        let signature_doc = toml::to_string(&PackSignature {
            signer_id: "ci".to_string(),
            public_key: hex::encode(signing_key.verifying_key().to_bytes()),
            digest,
            signature: hex::encode(signature.to_bytes()),
            timestamp: "2026-02-24T00:00:00Z".to_string(),
        })
        .unwrap();
        let files = BTreeMap::from([("signature.toml".to_string(), signature_doc.into_bytes())]);
        let warnings = verify_pack_trust(
            &files,
            digest,
            TrustPolicy::Permissive,
            &TrustedSigners::default(),
        )
        .unwrap();
        assert_eq!(warnings.len(), 1);
    }
}
