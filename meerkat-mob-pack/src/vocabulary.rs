//! Typed vocabulary for mobpack archive structure and identity.
//!
//! This module is the single owner of the archive's structural vocabulary:
//! which top-level section a path belongs to ([`ArchiveSection`]) and whether
//! an entry is executable ([`ExecPolicy`]). It exists so that classification is
//! a typed decision made in exactly one place, rather than string-prefix or
//! shebang folklore re-implemented at every call site.
//!
//! It also owns the typed identity newtypes used by the signing/trust boundary
//! ([`SignerId`], [`Ed25519PublicKeyHex`], [`Ed25519SignatureHex`],
//! [`Rfc3339Timestamp`]) and the manifest selector newtypes ([`ModelAlias`],
//! [`ModelRef`], [`SurfaceSelector`], [`SkillPath`]). All of these parse-validate
//! once at the TOML boundary so downstream policy reads a type, not raw text.

use ed25519_dalek::VerifyingKey;
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Top-level sections of a mobpack archive.
///
/// The section vocabulary is owned here; production code classifies a path with
/// [`ArchiveSection::classify`] rather than testing `path.starts_with(...)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ArchiveSection {
    Skills,
    Hooks,
    Mcp,
    Config,
}

impl ArchiveSection {
    /// All archive sections, in stable order. Drives table-driven classification
    /// and routing so no string literal prefix appears at the call site.
    pub const ALL: [ArchiveSection; 4] = [
        ArchiveSection::Skills,
        ArchiveSection::Hooks,
        ArchiveSection::Mcp,
        ArchiveSection::Config,
    ];

    /// The canonical directory prefix (including the trailing slash) that
    /// identifies this section inside the archive.
    pub const fn prefix(self) -> &'static str {
        match self {
            ArchiveSection::Skills => "skills/",
            ArchiveSection::Hooks => "hooks/",
            ArchiveSection::Mcp => "mcp/",
            ArchiveSection::Config => "config/",
        }
    }

    /// Classify a canonical archive path into its owning section, if any. Paths
    /// outside the known section vocabulary (e.g. `manifest.toml`,
    /// `definition.json`, `signature.toml`) return `None`.
    pub fn classify(path: &str) -> Option<ArchiveSection> {
        ArchiveSection::ALL
            .into_iter()
            .find(|section| path.starts_with(section.prefix()))
    }
}

/// Whether a mobpack archive entry should carry the executable bit.
///
/// Owns the executable-inference vocabulary that previously lived as scattered
/// suffix/shebang/path checks. Only entries inside [`ArchiveSection::Hooks`] are
/// eligible; eligibility is then decided by suffix or shebang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecPolicy {
    /// The entry is executable (a hook script).
    Executable,
    /// The entry is not executable.
    NonExecutable,
}

impl ExecPolicy {
    /// Executable script suffixes recognized inside the hooks section.
    const HOOK_SUFFIXES: [&'static str; 2] = [".sh", ".bash"];

    /// Classify an archive entry's executable status from its path and content.
    pub fn classify(path: &str, bytes: &[u8]) -> ExecPolicy {
        let normalized_path = path.replace('\\', "/");
        if ArchiveSection::classify(&normalized_path) != Some(ArchiveSection::Hooks) {
            return ExecPolicy::NonExecutable;
        }
        let has_known_suffix = ExecPolicy::HOOK_SUFFIXES
            .iter()
            .any(|suffix| normalized_path.ends_with(*suffix));
        if has_known_suffix || bytes.starts_with(b"#!") {
            ExecPolicy::Executable
        } else {
            ExecPolicy::NonExecutable
        }
    }

    pub fn is_executable(self) -> bool {
        matches!(self, ExecPolicy::Executable)
    }
}

/// A canonicalized archive path: identity under which an entry is digested,
/// classified, and deduplicated. Normalization (backslash to forward slash,
/// stripping a leading `./` or `/`) happens once when the value is constructed,
/// so digest identity is a typed decision in one place rather than raw-string
/// normalization re-implemented at every call site.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CanonicalPath(String);

impl CanonicalPath {
    /// The reserved entry name carrying the pack signature. It is excluded from
    /// the canonical digest (the signature signs the digest, so it cannot be
    /// part of it).
    pub const SIGNATURE: &'static str = "signature.toml";

    /// Canonicalize a raw archive path into its digest identity.
    pub fn new(path: &str) -> Self {
        let replaced = path.replace('\\', "/");
        let normalized = replaced
            .trim_start_matches("./")
            .trim_start_matches('/')
            .to_string();
        CanonicalPath(normalized)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the reserved signature entry, which is excluded from the
    /// canonical digest.
    pub fn is_signature(&self) -> bool {
        self.0 == Self::SIGNATURE
    }
}

impl fmt::Display for CanonicalPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Error raised when a typed mobpack vocabulary value fails to parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseVocabularyError(&'static str);

impl ParseVocabularyError {
    pub fn message(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for ParseVocabularyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ParseVocabularyError {}

/// Semantic identity of a mobpack signer, recorded in `signature.toml` and
/// looked up against the trust store. Parsed once at the TOML boundary; never
/// re-derived from filesystem layout.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SignerId(String);

impl SignerId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl FromStr for SignerId {
    type Err = ParseVocabularyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().is_empty() {
            return Err(ParseVocabularyError("signer id must not be empty"));
        }
        Ok(SignerId(s.to_string()))
    }
}

impl fmt::Display for SignerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for SignerId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SignerId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        SignerId::from_str(&raw).map_err(D::Error::custom)
    }
}

/// An Ed25519 public key, stored as validated lowercase hex and backed by a
/// parsed [`VerifyingKey`]. Hex is decoded and the key parsed once at
/// deserialization, so lookup/verify never re-parse raw text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ed25519PublicKeyHex {
    hex: String,
    key: VerifyingKey,
}

impl Ed25519PublicKeyHex {
    pub fn from_verifying_key(key: VerifyingKey) -> Self {
        Self {
            hex: hex::encode(key.to_bytes()),
            key,
        }
    }

    pub fn verifying_key(&self) -> &VerifyingKey {
        &self.key
    }

    pub fn as_hex(&self) -> &str {
        &self.hex
    }
}

impl FromStr for Ed25519PublicKeyHex {
    type Err = ParseVocabularyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes: [u8; 32] = hex::decode(s)
            .map_err(|_| ParseVocabularyError("public key must be valid hex"))?
            .try_into()
            .map_err(|_| ParseVocabularyError("public key must decode to 32 bytes"))?;
        let key = VerifyingKey::from_bytes(&bytes)
            .map_err(|_| ParseVocabularyError("public key is not a valid Ed25519 point"))?;
        Ok(Self {
            hex: s.to_string(),
            key,
        })
    }
}

impl fmt::Display for Ed25519PublicKeyHex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex)
    }
}

impl Serialize for Ed25519PublicKeyHex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.hex)
    }
}

impl<'de> Deserialize<'de> for Ed25519PublicKeyHex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ed25519PublicKeyHex::from_str(&raw).map_err(D::Error::custom)
    }
}

/// An Ed25519 signature, stored as validated lowercase hex and backed by a
/// parsed [`ed25519_dalek::Signature`]. Decoded once at deserialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ed25519SignatureHex {
    hex: String,
    signature: ed25519_dalek::Signature,
}

impl Ed25519SignatureHex {
    pub fn from_signature(signature: ed25519_dalek::Signature) -> Self {
        Self {
            hex: hex::encode(signature.to_bytes()),
            signature,
        }
    }

    pub fn signature(&self) -> &ed25519_dalek::Signature {
        &self.signature
    }

    pub fn as_hex(&self) -> &str {
        &self.hex
    }
}

impl FromStr for Ed25519SignatureHex {
    type Err = ParseVocabularyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes: [u8; 64] = hex::decode(s)
            .map_err(|_| ParseVocabularyError("signature must be valid hex"))?
            .try_into()
            .map_err(|_| ParseVocabularyError("signature must decode to 64 bytes"))?;
        Ok(Self {
            hex: s.to_string(),
            signature: ed25519_dalek::Signature::from_bytes(&bytes),
        })
    }
}

impl fmt::Display for Ed25519SignatureHex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.hex)
    }
}

impl Serialize for Ed25519SignatureHex {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.hex)
    }
}

impl<'de> Deserialize<'de> for Ed25519SignatureHex {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Ed25519SignatureHex::from_str(&raw).map_err(D::Error::custom)
    }
}

/// An RFC 3339 timestamp recorded on a pack signature. Validated once at the
/// TOML boundary so it cannot survive as an arbitrary string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rfc3339Timestamp(String);

impl Rfc3339Timestamp {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Rfc3339Timestamp {
    type Err = ParseVocabularyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        chrono::DateTime::parse_from_rfc3339(s)
            .map_err(|_| ParseVocabularyError("timestamp must be RFC 3339"))?;
        Ok(Rfc3339Timestamp(s.to_string()))
    }
}

impl fmt::Display for Rfc3339Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Rfc3339Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Rfc3339Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Rfc3339Timestamp::from_str(&raw).map_err(D::Error::custom)
    }
}

/// A profile-level alias under which a model is referenced in a manifest (the
/// `[models]` table key and the `[profiles.*].model` value).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModelAlias(String);

impl ModelAlias {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ModelAlias {
    type Err = ParseVocabularyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().is_empty() {
            return Err(ParseVocabularyError("model alias must not be empty"));
        }
        Ok(ModelAlias(s.to_string()))
    }
}

impl fmt::Display for ModelAlias {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ModelAlias {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ModelAlias {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        ModelAlias::from_str(&raw).map_err(D::Error::custom)
    }
}

/// A concrete model identifier referenced by a manifest (the `[models]` table
/// value).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModelRef(String);

impl ModelRef {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ModelRef {
    type Err = ParseVocabularyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().is_empty() {
            return Err(ParseVocabularyError("model reference must not be empty"));
        }
        Ok(ModelRef(s.to_string()))
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ModelRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ModelRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        ModelRef::from_str(&raw).map_err(D::Error::custom)
    }
}

/// A surface selector a mobpack declares support for. The known surface
/// vocabulary is closed; an unknown selector fails closed at deserialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SurfaceSelector {
    Cli,
    Rpc,
    Rest,
    Mcp,
    Web,
}

impl SurfaceSelector {
    pub const fn as_str(self) -> &'static str {
        match self {
            SurfaceSelector::Cli => "cli",
            SurfaceSelector::Rpc => "rpc",
            SurfaceSelector::Rest => "rest",
            SurfaceSelector::Mcp => "mcp",
            SurfaceSelector::Web => "web",
        }
    }
}

impl FromStr for SurfaceSelector {
    type Err = ParseVocabularyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "cli" => Ok(SurfaceSelector::Cli),
            "rpc" => Ok(SurfaceSelector::Rpc),
            "rest" => Ok(SurfaceSelector::Rest),
            "mcp" => Ok(SurfaceSelector::Mcp),
            "web" => Ok(SurfaceSelector::Web),
            _ => Err(ParseVocabularyError("unknown surface selector")),
        }
    }
}

impl fmt::Display for SurfaceSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for SurfaceSelector {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SurfaceSelector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        SurfaceSelector::from_str(&raw).map_err(D::Error::custom)
    }
}

/// A skill path declared by a manifest profile. Must be a canonical archive
/// path under the skills section; this is validated once at deserialization.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SkillPath(String);

impl SkillPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for SkillPath {
    type Err = ParseVocabularyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.replace('\\', "/");
        if normalized != s {
            return Err(ParseVocabularyError(
                "skill path must use canonical forward slashes",
            ));
        }
        if ArchiveSection::classify(s) != Some(ArchiveSection::Skills) {
            return Err(ParseVocabularyError("skill path must be under skills/"));
        }
        Ok(SkillPath(s.to_string()))
    }
}

impl fmt::Display for SkillPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for SkillPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SkillPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        SkillPath::from_str(&raw).map_err(D::Error::custom)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn test_archive_section_classify_table_driven() {
        // Section classification is owned by the typed table; no string-prefix
        // literals appear in production. Verify every section round-trips
        // through classify(prefix()).
        for section in ArchiveSection::ALL {
            let path = format!("{}entry.bin", section.prefix());
            assert_eq!(ArchiveSection::classify(&path), Some(section));
        }
        assert_eq!(ArchiveSection::classify("manifest.toml"), None);
        assert_eq!(ArchiveSection::classify("definition.json"), None);
        assert_eq!(ArchiveSection::classify("signature.toml"), None);
    }

    #[test]
    fn test_canonical_path_owns_digest_identity() {
        // Canonical-path identity is normalized once: backslashes, leading `./`
        // and leading `/` all collapse to the same digest identity.
        assert_eq!(
            CanonicalPath::new("skills/review.md").as_str(),
            "skills/review.md"
        );
        assert_eq!(
            CanonicalPath::new(".\\skills\\review.md").as_str(),
            "skills/review.md"
        );
        assert_eq!(
            CanonicalPath::new("/skills/review.md").as_str(),
            "skills/review.md"
        );
        assert_eq!(
            CanonicalPath::new("./hooks/run.sh").as_str(),
            "hooks/run.sh"
        );

        // The signature entry is recognized as such regardless of leading prefix,
        // so it is consistently excluded from the canonical digest.
        assert!(CanonicalPath::new("signature.toml").is_signature());
        assert!(CanonicalPath::new("./signature.toml").is_signature());
        assert!(!CanonicalPath::new("skills/review.md").is_signature());
    }

    #[test]
    fn test_exec_policy_classifier() {
        assert!(ExecPolicy::classify("hooks/run.sh", b"echo hi\n").is_executable());
        assert!(ExecPolicy::classify("hooks/setup", b"#!/usr/bin/env bash\n").is_executable());
        assert!(ExecPolicy::classify("hooks/run.bash", b"echo\n").is_executable());
        // Non-hook sections are never executable, even with a shebang.
        assert!(!ExecPolicy::classify("skills/review.md", b"#!/bin/sh\n").is_executable());
        // Hook entries without a known suffix or shebang are not executable.
        assert!(!ExecPolicy::classify("hooks/data.txt", b"plain\n").is_executable());
        // Backslash paths normalize before classification.
        assert!(ExecPolicy::classify("hooks\\run.sh", b"echo\n").is_executable());
    }

    #[test]
    fn test_signer_id_rejects_empty() {
        assert!(SignerId::from_str("").is_err());
        assert!(SignerId::from_str("   ").is_err());
        assert_eq!(SignerId::from_str("ci").unwrap().as_str(), "ci");
    }

    #[test]
    fn test_public_key_hex_roundtrip_and_fail_closed() {
        let key = SigningKey::from_bytes(&[7u8; 32]).verifying_key();
        let typed = Ed25519PublicKeyHex::from_verifying_key(key);
        let reparsed = Ed25519PublicKeyHex::from_str(typed.as_hex()).unwrap();
        assert_eq!(reparsed, typed);
        assert_eq!(reparsed.verifying_key().to_bytes(), key.to_bytes());
        // Malformed hex is rejected at parse time, not at verify time.
        assert!(Ed25519PublicKeyHex::from_str("zz").is_err());
        assert!(Ed25519PublicKeyHex::from_str("abcd").is_err());
    }

    #[test]
    fn test_signature_hex_roundtrip_and_fail_closed() {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let sig = signing_key.sign(b"payload");
        let typed = Ed25519SignatureHex::from_signature(sig);
        let reparsed = Ed25519SignatureHex::from_str(typed.as_hex()).unwrap();
        assert_eq!(reparsed, typed);
        assert!(Ed25519SignatureHex::from_str("zz").is_err());
        assert!(Ed25519SignatureHex::from_str("abcd").is_err());
    }

    #[test]
    fn test_rfc3339_timestamp_fail_closed() {
        assert!(Rfc3339Timestamp::from_str("2026-02-24T00:00:00Z").is_ok());
        assert!(Rfc3339Timestamp::from_str("not-a-timestamp").is_err());
        assert!(Rfc3339Timestamp::from_str("2026-02-24").is_err());
    }

    #[test]
    fn test_surface_selector_fail_closed() {
        for surface in [
            SurfaceSelector::Cli,
            SurfaceSelector::Rpc,
            SurfaceSelector::Rest,
            SurfaceSelector::Mcp,
            SurfaceSelector::Web,
        ] {
            assert_eq!(
                SurfaceSelector::from_str(surface.as_str()).unwrap(),
                surface
            );
        }
        assert!(SurfaceSelector::from_str("ftp").is_err());
    }

    #[test]
    fn test_skill_path_fail_closed() {
        assert_eq!(
            SkillPath::from_str("skills/review.md").unwrap().as_str(),
            "skills/review.md"
        );
        assert!(SkillPath::from_str("config/review.md").is_err());
        assert!(SkillPath::from_str("skills\\review.md").is_err());
    }

    #[test]
    fn test_model_alias_and_ref_reject_empty() {
        assert!(ModelAlias::from_str("").is_err());
        assert!(ModelRef::from_str("  ").is_err());
        assert_eq!(ModelAlias::from_str("planner").unwrap().as_str(), "planner");
        assert_eq!(ModelRef::from_str("gpt-5").unwrap().as_str(), "gpt-5");
    }

    #[test]
    fn test_surface_selector_serde_fail_closed() {
        // serde deserialization must reject unknown selectors (fail-closed),
        // not coalesce them into a permissive default.
        let err = serde_json::from_str::<SurfaceSelector>("\"ftp\"");
        assert!(err.is_err());
        let ok: SurfaceSelector = serde_json::from_str("\"cli\"").unwrap();
        assert_eq!(ok, SurfaceSelector::Cli);
    }
}
