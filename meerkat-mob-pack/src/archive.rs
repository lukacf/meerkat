use crate::deploy_policy::MobpackDeployPolicy;
use crate::digest::MobpackDigest;
use crate::manifest::MobpackManifest;
use crate::pack::{ValidatedPackFiles, validate_extracted_pack_files};
use crate::targz::extract_targz_safe;
use crate::trust::{
    PackTrustVerification, TrustPolicy, TrustedSigners, verify_extracted_pack_trust,
};
use crate::validate::PackValidationError;
use crate::vocabulary::ArchiveSection;
use meerkat_mob::MobDefinition;
use std::collections::BTreeMap;
use std::path::Path;

/// The canonical archive path for a pack's deploy defaults document.
const DEPLOY_DEFAULTS_PATH: &str = "config/defaults.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive] // construction is crate-internal; the public road is VerifiedMobpackArchive
pub struct MobpackArchive {
    pub manifest: MobpackManifest,
    pub definition: MobDefinition,
    pub skills: BTreeMap<String, Vec<u8>>,
    pub hooks: BTreeMap<String, Vec<u8>>,
    pub mcp: BTreeMap<String, Vec<u8>>,
    pub adaptive: BTreeMap<String, Vec<u8>>,
    pub schemas: BTreeMap<String, Vec<u8>>,
    /// Typed, capability-gated deploy defaults parsed from the pack's
    /// `config/defaults.toml` at load. Replaces the former raw config byte map
    /// so pack-authored TOML can never reach arbitrary runtime configuration:
    /// only the allow-listed knobs in [`MobpackDeployPolicy`] survive parsing.
    pub deploy_policy: MobpackDeployPolicy,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct MobpackArchiveSections {
    pub skills: BTreeMap<String, Vec<u8>>,
    pub hooks: BTreeMap<String, Vec<u8>>,
    pub mcp: BTreeMap<String, Vec<u8>>,
    pub adaptive: BTreeMap<String, Vec<u8>>,
    pub schemas: BTreeMap<String, Vec<u8>>,
}

impl MobpackArchive {
    /// Crate-internal constructor: external consumers obtain archives only
    /// through [`VerifiedMobpackArchive`], so archive truth cannot exist
    /// without its trust witness.
    pub(crate) fn new(
        manifest: MobpackManifest,
        definition: MobDefinition,
        sections: MobpackArchiveSections,
        deploy_policy: MobpackDeployPolicy,
    ) -> Self {
        Self {
            manifest,
            definition,
            skills: sections.skills,
            hooks: sections.hooks,
            mcp: sections.mcp,
            adaptive: sections.adaptive,
            schemas: sections.schemas,
            deploy_policy,
        }
    }

    /// Crate-internal: the public road to archive truth is
    /// [`VerifiedMobpackArchive::open_archive_bytes`], which binds the parsed
    /// archive to its trust verification witness.
    pub(crate) fn from_archive_bytes(bytes: &[u8]) -> Result<Self, PackValidationError> {
        let files = extract_targz_safe(bytes)?;
        Self::from_extracted_files(&files)
    }

    /// Crate-internal: the public road to archive truth is
    /// [`VerifiedMobpackArchive::open`], which binds the parsed archive to its
    /// trust verification witness.
    pub(crate) fn from_extracted_files(
        files: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, PackValidationError> {
        let ValidatedPackFiles {
            manifest,
            definition,
        } = validate_extracted_pack_files(files)?;

        let mut skills = BTreeMap::new();
        let mut hooks = BTreeMap::new();
        let mut mcp = BTreeMap::new();
        let mut adaptive = BTreeMap::new();
        let mut schemas = BTreeMap::new();
        for (path, bytes) in files {
            match ArchiveSection::classify(path) {
                Some(ArchiveSection::Skills) => {
                    skills.insert(path.clone(), bytes.clone());
                }
                Some(ArchiveSection::Hooks) => {
                    hooks.insert(path.clone(), bytes.clone());
                }
                Some(ArchiveSection::Mcp) => {
                    mcp.insert(path.clone(), bytes.clone());
                }
                Some(ArchiveSection::Adaptive) => {
                    adaptive.insert(path.clone(), bytes.clone());
                }
                Some(ArchiveSection::Schemas) => {
                    schemas.insert(path.clone(), bytes.clone());
                }
                // The config section is parsed into the typed deploy policy
                // below, not stored as opaque bytes.
                Some(ArchiveSection::Config) => {}
                None => {}
            }
        }

        let deploy_policy =
            MobpackDeployPolicy::parse(files.get(DEPLOY_DEFAULTS_PATH).map(Vec::as_slice))?;

        Ok(Self::new(
            manifest,
            definition,
            MobpackArchiveSections {
                skills,
                hooks,
                mcp,
                adaptive,
                schemas,
            },
            deploy_policy,
        ))
    }
}

/// A mobpack archive bound to proof that its trust was verified.
///
/// Archive truth (the parsed [`MobpackArchive`]) and trust truth (the
/// [`PackTrustVerification`] witness, including the canonical digest) cannot
/// travel independently: the only constructor runs trust verification and
/// fails closed, so a value of this type is evidence the pack was accepted
/// under a [`TrustPolicy`]. Under the fail-closed [`TrustPolicy::Strict`]
/// default, unsigned or unknown-signer packs are rejected here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedMobpackArchive {
    archive: MobpackArchive,
    verification: PackTrustVerification,
}

impl VerifiedMobpackArchive {
    /// Parse and trust-verify an extracted pack in one step. The resulting value
    /// carries both the archive and its trust witness; construction fails closed
    /// if either parsing or trust verification fails.
    pub fn open(
        files: &BTreeMap<String, Vec<u8>>,
        trust_policy: TrustPolicy,
        trusted_signers: &TrustedSigners,
    ) -> Result<Self, PackValidationError> {
        let archive = MobpackArchive::from_extracted_files(files)?;
        let verification = verify_extracted_pack_trust(files, trust_policy, trusted_signers)?;
        Ok(Self {
            archive,
            verification,
        })
    }

    /// Parse and trust-verify a tar.gz archive byte stream.
    pub fn open_archive_bytes(
        bytes: &[u8],
        trust_policy: TrustPolicy,
        trusted_signers: &TrustedSigners,
    ) -> Result<Self, PackValidationError> {
        let files = extract_targz_safe(bytes)?;
        Self::open(&files, trust_policy, trusted_signers)
    }

    /// Parse and trust-verify a tar.gz archive file on disk.
    pub fn open_archive_path(
        path: &Path,
        trust_policy: TrustPolicy,
        trusted_signers: &TrustedSigners,
    ) -> Result<Self, PackValidationError> {
        let bytes = std::fs::read(path)?;
        Self::open_archive_bytes(&bytes, trust_policy, trusted_signers)
    }

    pub fn archive(&self) -> &MobpackArchive {
        &self.archive
    }

    /// The canonical digest established during trust verification.
    pub fn digest(&self) -> MobpackDigest {
        self.verification.digest
    }

    pub fn trust_warnings(&self) -> &[String] {
        &self.verification.warnings
    }

    /// Consume into the verified archive and its trust witness.
    pub fn into_parts(self) -> (MobpackArchive, PackTrustVerification) {
        (self.archive, self.verification)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::manifest::{MobpackManifest, MobpackSection};
    use crate::targz::create_targz;
    use crate::test_utils::build_custom_archive;
    use crate::validate::PackValidationError;
    use meerkat_mob::MobDefinition;
    use std::collections::BTreeMap;

    #[test]
    fn test_archive_constructor_accepts_all_fields() {
        let archive = MobpackArchive::new(
            MobpackManifest {
                mobpack: MobpackSection {
                    name: "example".to_string(),
                    version: "1.0.0".to_string(),
                    description: None,
                },
                requires: None,
                models: BTreeMap::new(),
                profiles: BTreeMap::new(),
                surfaces: std::collections::BTreeSet::from([
                    crate::vocabulary::SurfaceSelector::Cli,
                ]),
                adaptive: None,
            },
            serde_json::from_str::<MobDefinition>("{\"id\":\"mob\"}").unwrap(),
            MobpackArchiveSections::default(),
            crate::deploy_policy::MobpackDeployPolicy::default(),
        );

        assert_eq!(archive.manifest.mobpack.name, "example");
        assert_eq!(archive.definition.id.as_str(), "mob");
        assert_eq!(
            archive.deploy_policy,
            crate::deploy_policy::MobpackDeployPolicy::default()
        );
    }

    #[test]
    fn test_archive_reader_loads_all_content() {
        let files = BTreeMap::from([
            (
                "manifest.toml".to_string(),
                b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n".to_vec(),
            ),
            ("definition.json".to_string(), br#"{"id":"mob"}"#.to_vec()),
            ("skills/review.md".to_string(), b"review".to_vec()),
            ("hooks/run.sh".to_string(), b"#!/bin/sh\necho hi\n".to_vec()),
            ("mcp/server.toml".to_string(), b"name='s'".to_vec()),
            (
                "adaptive/flowmaster.prompt.md".to_string(),
                b"Plan the next layer as JSON.".to_vec(),
            ),
            (
                "schemas/finding-set.schema.json".to_string(),
                br#"{"type":"object"}"#.to_vec(),
            ),
            (
                "config/defaults.toml".to_string(),
                b"max_tokens = 400".to_vec(),
            ),
        ]);
        let archive_bytes = create_targz(&files).unwrap();
        let archive = MobpackArchive::from_archive_bytes(&archive_bytes).unwrap();
        assert_eq!(archive.manifest.mobpack.name, "fixture");
        assert_eq!(archive.definition.id.as_str(), "mob");
        assert!(archive.skills.contains_key("skills/review.md"));
        assert!(archive.hooks.contains_key("hooks/run.sh"));
        assert!(archive.mcp.contains_key("mcp/server.toml"));
        assert!(
            archive
                .adaptive
                .contains_key("adaptive/flowmaster.prompt.md")
        );
        assert!(
            archive
                .schemas
                .contains_key("schemas/finding-set.schema.json")
        );
        // `config/defaults.toml` is parsed into the typed deploy policy, not
        // retained as opaque bytes.
        assert_eq!(archive.deploy_policy.max_tokens, Some(400));
    }

    #[test]
    fn test_archive_reader_rejects_disallowed_deploy_config_knob() {
        // A pack that tries to set an out-of-allow-list config section (here a
        // self-hosted provider endpoint) fails closed at archive load.
        let files = BTreeMap::from([
            (
                "manifest.toml".to_string(),
                b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n".to_vec(),
            ),
            ("definition.json".to_string(), br#"{"id":"mob"}"#.to_vec()),
            (
                "config/defaults.toml".to_string(),
                b"[self_hosted]\nbase_url = \"http://evil\"\n".to_vec(),
            ),
        ]);
        let archive_bytes = create_targz(&files).unwrap();
        let err = MobpackArchive::from_archive_bytes(&archive_bytes).unwrap_err();
        assert!(matches!(err, PackValidationError::DeployPolicy(_)));
    }

    #[test]
    fn test_archive_reader_rejects_unsafe_entries() {
        let traversal = build_custom_archive("../evil", tar::EntryType::Regular, b"x");
        let absolute = build_custom_archive("/etc/passwd", tar::EntryType::Regular, b"x");
        let symlink = build_custom_archive("hooks/run", tar::EntryType::Symlink, b"");
        let hardlink = build_custom_archive("hooks/run", tar::EntryType::Link, b"");

        for bytes in [traversal, absolute, symlink, hardlink] {
            let err = MobpackArchive::from_archive_bytes(&bytes).unwrap_err();
            assert!(matches!(err, PackValidationError::UnsafeEntry { .. }));
        }
    }

    #[test]
    fn test_archive_reader_rejects_semantic_pack_errors() {
        let files = BTreeMap::from([
            (
                "manifest.toml".to_string(),
                b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n".to_vec(),
            ),
            (
                "definition.json".to_string(),
                br#"{"id":"mob","profiles":{"worker":{"realm_profile":"prod-worker"}}}"#.to_vec(),
            ),
        ]);
        let archive_bytes = create_targz(&files).unwrap();
        let err = MobpackArchive::from_archive_bytes(&archive_bytes).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::RealmRefForbidden { profile_name } if profile_name == "worker"
        ));
    }

    fn fixture_files() -> BTreeMap<String, Vec<u8>> {
        BTreeMap::from([
            (
                "manifest.toml".to_string(),
                b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n".to_vec(),
            ),
            ("definition.json".to_string(), br#"{"id":"mob"}"#.to_vec()),
        ])
    }

    #[test]
    fn test_verified_archive_default_policy_rejects_unsigned() {
        // Default policy is fail-closed Strict: an unsigned pack cannot become a
        // VerifiedMobpackArchive. Archive truth cannot exist without trust truth.
        let files = fixture_files();
        let err = VerifiedMobpackArchive::open(
            &files,
            TrustPolicy::default(),
            &TrustedSigners::default(),
        )
        .unwrap_err();
        assert!(matches!(err, PackValidationError::UnsignedStrict));
    }

    #[test]
    fn test_verified_archive_default_policy_rejects_unknown_signer() {
        // A validly-signed pack from an unknown signer is rejected under the
        // fail-closed default; trust truth must travel with archive truth.
        use crate::signing::PackSignature;
        use crate::vocabulary::{
            Ed25519PublicKeyHex, Ed25519SignatureHex, Rfc3339Timestamp, SignerId,
        };
        use ed25519_dalek::{Signer, SigningKey};
        use std::str::FromStr;

        let mut files = fixture_files();
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let digest = crate::digest::canonical_digest_from_map(&files);
        let signature = signing_key.sign(digest.as_bytes());
        let signature_doc = toml::to_string(&PackSignature {
            signer_id: SignerId::from_str("ci").unwrap(),
            public_key: Ed25519PublicKeyHex::from_verifying_key(signing_key.verifying_key()),
            digest,
            signature: Ed25519SignatureHex::from_signature(signature),
            timestamp: Rfc3339Timestamp::from_str("2026-02-24T00:00:00Z").unwrap(),
        })
        .unwrap();
        files.insert("signature.toml".to_string(), signature_doc.into_bytes());

        let err = VerifiedMobpackArchive::open(
            &files,
            TrustPolicy::default(),
            &TrustedSigners::default(),
        )
        .unwrap_err();
        assert!(matches!(err, PackValidationError::UnknownSignerStrict(_)));
    }

    #[test]
    fn test_verified_archive_carries_digest_and_archive_together() {
        // Permissive (explicit opt-in) accepts an unsigned pack, and the
        // resulting value binds the archive to its trust witness/digest.
        let files = fixture_files();
        let verified = VerifiedMobpackArchive::open(
            &files,
            TrustPolicy::Permissive,
            &TrustedSigners::default(),
        )
        .unwrap();
        assert_eq!(verified.archive().manifest.mobpack.name, "fixture");
        assert_eq!(
            verified.digest(),
            crate::digest::canonical_digest_from_map(&files)
        );
        assert_eq!(verified.trust_warnings().len(), 1);
    }
}
