use crate::digest::{MobpackDigest, canonical_digest_from_map};
use crate::signing::{MobpackPublicKey, MobpackSignerId, PackSignature};
use crate::validate::PackValidationError;
use ed25519_dalek::Verifier;
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
    pub signers: BTreeMap<MobpackSignerId, MobpackPublicKey>,
}

impl TrustedSigners {
    pub fn lookup(&self, signer_id: &MobpackSignerId) -> Option<&MobpackPublicKey> {
        self.signers.get(signer_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackTrustVerification {
    pub digest: MobpackDigest,
    pub warnings: Vec<String>,
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

pub fn verify_extracted_pack_trust(
    files: &BTreeMap<String, Vec<u8>>,
    trust_policy: TrustPolicy,
    trusted_signers: &TrustedSigners,
) -> Result<PackTrustVerification, PackValidationError> {
    let digest = canonical_digest_from_map(files);
    let warnings = verify_pack_trust(files, digest, trust_policy, trusted_signers)?;
    Ok(PackTrustVerification { digest, warnings })
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
    let embedded_key = pack_signature
        .public_key
        .to_verifying_key()
        .map_err(|err| PackValidationError::InvalidSignature(err.to_string()))?;
    let sig = pack_signature.signature.to_signature();

    if let Some(trusted_hex) = trusted_signers.lookup(&pack_signature.signer_id) {
        let trusted_key = trusted_hex
            .to_verifying_key()
            .map_err(|err| PackValidationError::InvalidSignature(err.to_string()))?;
        if trusted_key.to_bytes() != embedded_key.to_bytes() {
            return Err(PackValidationError::SignerKeyMismatch(
                pack_signature.signer_id.to_string(),
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
            pack_signature.signer_id.to_string(),
        ));
    }
    warnings.push("signature valid but signer is unknown in permissive mode".to_string());
    Ok(warnings)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::digest::MobpackDigest;
    use ed25519_dalek::{Signer, SigningKey};
    use std::str::FromStr;
    use tempfile::TempDir;

    fn signer_id(value: &str) -> MobpackSignerId {
        MobpackSignerId::new(value).unwrap()
    }

    fn public_key(signing_key: &SigningKey) -> MobpackPublicKey {
        MobpackPublicKey::from_verifying_key(&signing_key.verifying_key())
    }

    fn timestamp() -> crate::signing::MobpackSignatureTimestamp {
        crate::signing::MobpackSignatureTimestamp::parse("2026-02-24T00:00:00Z").unwrap()
    }

    fn signature_doc(
        signer: &str,
        signing_key: &SigningKey,
        digest: MobpackDigest,
        signature: ed25519_dalek::Signature,
    ) -> String {
        toml::to_string(&PackSignature {
            signer_id: signer_id(signer),
            public_key: public_key(signing_key),
            digest,
            signature: crate::signing::MobpackSignatureBytes::from_signature(&signature),
            timestamp: timestamp(),
        })
        .unwrap()
    }

    #[test]
    fn test_trust_store_toml_roundtrip() {
        let ci_key = SigningKey::from_bytes(&[1u8; 32]);
        let release_key = SigningKey::from_bytes(&[2u8; 32]);
        let store = TrustedSigners {
            signers: BTreeMap::from([
                (signer_id("ci"), public_key(&ci_key)),
                (signer_id("release"), public_key(&release_key)),
            ]),
        };

        let encoded = toml::to_string(&store).unwrap();
        let decoded: TrustedSigners = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded, store);
        assert_eq!(decoded.lookup(&signer_id("ci")), Some(&public_key(&ci_key)));
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
            merged.lookup(&signer_id("ci")),
            Some(&MobpackPublicKey::from_bytes([0xbb; 32]))
        );
        assert_eq!(
            merged.lookup(&signer_id("release")),
            Some(&MobpackPublicKey::from_bytes([0xcc; 32]))
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
        let signature_doc = signature_doc("ci", &signing_key, digest, signature);
        let files = BTreeMap::from([("signature.toml".to_string(), signature_doc.into_bytes())]);
        let trusted = TrustedSigners {
            signers: BTreeMap::from([(signer_id("ci"), public_key(&signing_key))]),
        };
        verify_pack_trust(&files, digest, TrustPolicy::Strict, &trusted).unwrap();
    }

    #[test]
    fn test_strict_rejects_unknown_signer() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let digest = MobpackDigest::from_bytes([4u8; 32]);
        let signature = signing_key.sign(digest.as_bytes());
        let signature_doc = signature_doc("ci", &signing_key, digest, signature);
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
        let signature_doc = signature_doc("ci", &signing_key, digest, signature);
        let files = BTreeMap::from([("signature.toml".to_string(), signature_doc.into_bytes())]);
        let trusted = TrustedSigners {
            signers: BTreeMap::from([(signer_id("ci"), public_key(&trusted_key))]),
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
            signer_id: signer_id("ci"),
            public_key: public_key(&signing_key),
            digest,
            signature: crate::signing::MobpackSignatureBytes::from_bytes(signature_bytes),
            timestamp: timestamp(),
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
        let signature_doc = signature_doc("ci", &signing_key, digest, signature);
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
