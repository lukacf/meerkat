use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PackValidationError {
    #[error("manifest.toml is missing")]
    MissingManifest,
    #[error("definition.json is missing")]
    MissingDefinition,
    #[error("definition.json is invalid: {0}")]
    BadDefinition(String),
    #[error("manifest.toml is invalid: {0}")]
    InvalidManifest(String),
    #[error("manifest.toml field `{field}` is invalid: {reason}")]
    InvalidManifestField { field: String, reason: String },
    #[error("manifest.toml may not contain [trust]")]
    TrustSectionForbidden,
    #[error("unsafe archive entry `{path}`: {reason}")]
    UnsafeEntry { path: String, reason: String },
    #[error("duplicate archive entry after path normalization: `{path}`")]
    DuplicateArchiveEntry { path: String },
    #[error("unsigned pack rejected in strict trust mode")]
    UnsignedStrict,
    #[error("signature is invalid: {0}")]
    InvalidSignature(String),
    #[error("unknown signer `{0}` in strict trust mode")]
    UnknownSignerStrict(String),
    #[error("embedded public key mismatches trust store for signer `{0}`")]
    SignerKeyMismatch(String),
    #[error("signature digest does not match archive content digest")]
    SignatureDigestMismatch,
    #[error("required capability missing: {0}")]
    CapabilityMismatch(String),
    #[error("archive parse failed: {0}")]
    Archive(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("signing key is invalid: {0}")]
    InvalidSigningKey(String),
    #[error("profile `{profile_name}` uses a realm reference, which is forbidden in packs")]
    RealmRefForbidden { profile_name: String },
    #[error("mobpack skill path '{path}' for '{skill_name}' is invalid: {reason}")]
    InvalidSkillPath {
        skill_name: String,
        path: String,
        reason: String,
    },
    #[error("mobpack skill path '{path}' for '{skill_name}' missing from archive")]
    MissingSkillFile { skill_name: String, path: String },
    #[error("mobpack skill path '{path}' for '{skill_name}' is not valid UTF-8: {reason}")]
    InvalidSkillUtf8 {
        skill_name: String,
        path: String,
        reason: String,
    },
}

impl From<std::io::Error> for PackValidationError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::PackValidationError;

    #[test]
    fn test_pack_validation_error_variants_constructible() {
        let errors = vec![
            PackValidationError::MissingManifest,
            PackValidationError::MissingDefinition,
            PackValidationError::BadDefinition("bad json".to_string()),
            PackValidationError::UnsafeEntry {
                path: "hooks/../../etc/passwd".to_string(),
                reason: "path traversal".to_string(),
            },
            PackValidationError::UnsignedStrict,
            PackValidationError::InvalidSignature("bad".to_string()),
            PackValidationError::UnknownSignerStrict("ci".to_string()),
            PackValidationError::SignerKeyMismatch("ci".to_string()),
            PackValidationError::SignatureDigestMismatch,
            PackValidationError::InvalidManifest("bad toml".to_string()),
            PackValidationError::InvalidManifestField {
                field: "mobpack.name".to_string(),
                reason: "must not be empty".to_string(),
            },
            PackValidationError::TrustSectionForbidden,
            PackValidationError::DuplicateArchiveEntry {
                path: "skills/review.md".to_string(),
            },
            PackValidationError::CapabilityMismatch("comms".to_string()),
            PackValidationError::Archive("bad tar".to_string()),
            PackValidationError::Io("disk full".to_string()),
            PackValidationError::InvalidSigningKey("bad key".to_string()),
            PackValidationError::RealmRefForbidden {
                profile_name: "worker".to_string(),
            },
            PackValidationError::InvalidSkillPath {
                skill_name: "review".to_string(),
                path: "../review.md".to_string(),
                reason: "path traversal is not allowed".to_string(),
            },
            PackValidationError::MissingSkillFile {
                skill_name: "review".to_string(),
                path: "skills/review.md".to_string(),
            },
            PackValidationError::InvalidSkillUtf8 {
                skill_name: "review".to_string(),
                path: "skills/review.md".to_string(),
                reason: "invalid utf-8 sequence".to_string(),
            },
        ];

        assert_eq!(errors.len(), 21);
    }
}
