use crate::digest::{MobpackDigest, canonical_digest_from_map};
use crate::manifest::MobpackManifest;
use crate::signing::{PackSignature, sign_digest};
use crate::targz::{create_targz, extract_targz_safe, normalize_for_archive};
use crate::validate::PackValidationError;
use crate::vocabulary::{
    ArchiveSection, Ed25519PublicKeyHex, Ed25519SignatureHex, Rfc3339Timestamp, SignerId,
};
use chrono::{SecondsFormat, Utc};
use ed25519_dalek::SigningKey;
use meerkat_contracts::capability::{
    CapabilityId, MobpackCapabilityId, known_mobpack_capability_tokens,
    mobpack_capability_known_to_host,
};
use meerkat_mob::adaptive::AdaptivePolicy;
use meerkat_mob::{MobDefinition, definition::SkillSource};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::str::FromStr;

/// Canonical archive path of the bundled LayerDecision schema artifact.
const LAYER_DECISION_SCHEMA_PATH: &str = "adaptive/layer-decision.schema.json";

/// Canonical archive path of the adaptive policy document.
const ADAPTIVE_POLICIES_PATH: &str = "adaptive/policies.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackResult {
    pub archive_bytes: Vec<u8>,
    pub digest: MobpackDigest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectResult {
    pub name: String,
    pub version: String,
    pub file_count: usize,
    pub files: Vec<String>,
    pub digest: MobpackDigest,
}

/// Inputs required to sign a mobpack.
///
/// `signer_id` is the semantic identity recorded in `signature.toml` and looked
/// up against the trust store. `key_path` is key material used to authenticate
/// that identity; it is deliberately separate because the filesystem layout of
/// the key file is not a source of identity.
#[derive(Debug, Clone, Copy)]
pub struct SigningRequest<'a> {
    pub signer_id: &'a str,
    pub key_path: &'a Path,
}

#[derive(Debug)]
pub(crate) struct ValidatedPackFiles {
    pub manifest: MobpackManifest,
    pub definition: MobDefinition,
}

pub fn pack_directory(
    directory: &Path,
    signing: Option<SigningRequest<'_>>,
) -> Result<PackResult, PackValidationError> {
    pack_directory_with_excludes(directory, signing, &[])
}

pub fn pack_directory_with_excludes(
    directory: &Path,
    signing: Option<SigningRequest<'_>>,
    excluded_paths: &[&Path],
) -> Result<PackResult, PackValidationError> {
    let mut files = scan_directory(directory)?;
    exclude_paths_from_pack(directory, excluded_paths, &mut files);
    if let Some(request) = signing {
        exclude_paths_from_pack(directory, &[request.key_path], &mut files);
    }
    prepare_adaptive_build_outputs(&mut files)?;
    validate_extracted_pack_files(&files)?;

    if let Some(request) = signing {
        let unsigned_archive = create_targz(&files)?;
        let digest = compute_archive_digest(&unsigned_archive)?;
        let signing_key = load_signing_key(request.key_path)?;
        let signature = sign_digest(&signing_key, digest);
        let signer_id = SignerId::from_str(request.signer_id).map_err(|err| {
            PackValidationError::InvalidSignature(format!("invalid signer id: {err}"))
        })?;
        let timestamp =
            Rfc3339Timestamp::from_str(&Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true))
                .map_err(|err| PackValidationError::InvalidSignature(err.to_string()))?;
        let signature_toml = toml::to_string(&PackSignature {
            signer_id,
            public_key: Ed25519PublicKeyHex::from_verifying_key(signing_key.verifying_key()),
            digest,
            signature: Ed25519SignatureHex::from_signature(signature),
            timestamp,
        })
        .map_err(|err| PackValidationError::Archive(err.to_string()))?;
        files.insert("signature.toml".to_string(), signature_toml.into_bytes());
    }

    let archive_bytes = create_targz(&files)?;
    let digest = compute_archive_digest(&archive_bytes)?;
    Ok(PackResult {
        archive_bytes,
        digest,
    })
}

pub fn inspect_archive_bytes(bytes: &[u8]) -> Result<InspectResult, PackValidationError> {
    let files = extract_targz_safe(bytes)?;
    let ValidatedPackFiles { manifest, .. } = validate_extracted_pack_files(&files)?;
    let mut file_list: Vec<String> = files.keys().cloned().collect();
    file_list.sort();
    Ok(InspectResult {
        name: manifest.mobpack.name,
        version: manifest.mobpack.version,
        file_count: files.len(),
        files: file_list,
        digest: canonical_digest_from_map(&files),
    })
}

pub fn compute_archive_digest(bytes: &[u8]) -> Result<MobpackDigest, PackValidationError> {
    let files = extract_targz_safe(bytes)?;
    Ok(canonical_digest_from_map(&files))
}

pub(crate) fn validate_extracted_pack_files(
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<ValidatedPackFiles, PackValidationError> {
    validate_required_files(files)?;
    ensure_no_trust_section(files)?;
    let manifest = parse_manifest(files)?;
    validate_manifest_identity(&manifest)?;
    validate_required_capabilities_known(&manifest)?;
    validate_adaptive_files(&manifest, files)?;
    let definition = parse_definition(files)?;
    reject_realm_refs(&definition)?;
    validate_skill_paths(&definition, files)?;
    Ok(ValidatedPackFiles {
        manifest,
        definition,
    })
}

/// Fail-closed capability knowledge gate, run at every pack load.
///
/// Every `[requires]` capability must classify into the typed vocabulary this
/// host build knows ([`mobpack_capability_known_to_host`]); a token from the
/// future (or a vendor token) is rejected with the host's known set instead
/// of being silently ignored and the pack downgraded. Additionally, a
/// manifest that declares `[adaptive]` must carry the stamped
/// `adaptive_flow` capability so pre-stamp packs cannot smuggle adaptive
/// payloads past hosts that gate on `[requires]`.
fn validate_required_capabilities_known(
    manifest: &MobpackManifest,
) -> Result<(), PackValidationError> {
    if let Some(requires) = &manifest.requires {
        for capability in &requires.capabilities {
            if !mobpack_capability_known_to_host(capability.id()) {
                return Err(PackValidationError::UnknownRequiredCapability {
                    capability: capability.token().to_string(),
                    supported: known_mobpack_capability_tokens(),
                });
            }
        }
    }
    if manifest.adaptive.is_some() && !manifest_declares_adaptive_capability(manifest) {
        return Err(PackValidationError::MissingAdaptiveCapability {
            capability: CapabilityId::AdaptiveFlow.to_string(),
        });
    }
    Ok(())
}

fn manifest_declares_adaptive_capability(manifest: &MobpackManifest) -> bool {
    manifest.requires.as_ref().is_some_and(|requires| {
        requires
            .capability_ids()
            .any(|id| id == MobpackCapabilityId::Known(CapabilityId::AdaptiveFlow))
    })
}

/// Build-side preparation for adaptive packs, run before validation when
/// packing a directory.
///
/// For a manifest that declares `[adaptive]` this
/// 1. stamps the `adaptive_flow` capability into `[requires]` (idempotent —
///    an already-stamped manifest is left byte-identical), and
/// 2. emits the host's canonical LayerDecision schema as
///    `adaptive/layer-decision.schema.json`, overwriting any hand-supplied
///    file (authors no longer maintain this artifact; the archive digest and
///    signature cover the emitted bytes).
///
/// Non-adaptive packs and packs whose manifest does not parse are left
/// untouched; validation reports those errors with full context.
fn prepare_adaptive_build_outputs(
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), PackValidationError> {
    let Some(manifest_bytes) = files.get("manifest.toml") else {
        return Ok(());
    };
    let Ok(manifest_text) = std::str::from_utf8(manifest_bytes) else {
        return Ok(());
    };
    let Ok(manifest) = toml::from_str::<MobpackManifest>(manifest_text) else {
        return Ok(());
    };
    if manifest.adaptive.is_none() {
        return Ok(());
    }

    if !manifest_declares_adaptive_capability(&manifest) {
        let stamped = stamp_adaptive_capability(manifest_text)?;
        files.insert("manifest.toml".to_string(), stamped.into_bytes());
    }
    files.insert(
        LAYER_DECISION_SCHEMA_PATH.to_string(),
        canonical_layer_decision_schema_bytes()?,
    );
    Ok(())
}

/// Append `adaptive_flow` to `[requires].capabilities`, preserving the rest
/// of the manifest document (comments, ordering, unknown entries) via
/// `toml_edit`.
fn stamp_adaptive_capability(manifest_text: &str) -> Result<String, PackValidationError> {
    let mut doc: toml_edit::DocumentMut = manifest_text
        .parse()
        .map_err(|err| PackValidationError::InvalidManifest(format!("{err}")))?;
    let requires = doc
        .entry("requires")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    // `as_table_like_mut` accepts both `[requires]` tables and
    // `requires = { ... }` inline tables — the typed manifest parser does.
    let requires_table = requires.as_table_like_mut().ok_or_else(|| {
        PackValidationError::InvalidManifest("`requires` must be a table".to_string())
    })?;
    if requires_table.get("capabilities").is_none() {
        requires_table.insert(
            "capabilities",
            toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new())),
        );
    }
    let array = requires_table
        .get_mut("capabilities")
        .and_then(toml_edit::Item::as_array_mut)
        .ok_or_else(|| {
            PackValidationError::InvalidManifest(
                "`requires.capabilities` must be an array".to_string(),
            )
        })?;
    array.push(CapabilityId::AdaptiveFlow.to_string());
    Ok(doc.to_string())
}

/// The canonical `adaptive/layer-decision.schema.json` bytes for this host
/// version, rendered deterministically from
/// [`meerkat_mob::adaptive::layer_decision_schema`].
fn canonical_layer_decision_schema_bytes() -> Result<Vec<u8>, PackValidationError> {
    let schema = canonical_layer_decision_schema()?;
    let mut bytes = serde_json::to_vec_pretty(&schema).map_err(|err| {
        PackValidationError::InvalidAdaptiveFile {
            path: LAYER_DECISION_SCHEMA_PATH.to_string(),
            reason: format!("failed to render canonical schema: {err}"),
        }
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn canonical_layer_decision_schema() -> Result<serde_json::Value, PackValidationError> {
    meerkat_mob::adaptive::layer_decision_schema().map_err(|err| {
        PackValidationError::InvalidAdaptiveFile {
            path: LAYER_DECISION_SCHEMA_PATH.to_string(),
            reason: format!("failed to derive canonical schema: {err}"),
        }
    })
}

fn validate_adaptive_files(
    manifest: &MobpackManifest,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), PackValidationError> {
    let Some(adaptive) = &manifest.adaptive else {
        return Ok(());
    };
    let policy_bytes = require_adaptive_file(files, ADAPTIVE_POLICIES_PATH)?;
    let observed_policy_digest = format!("sha256:{}", hex::encode(Sha256::digest(policy_bytes)));
    if adaptive.policy_digest != observed_policy_digest {
        return Err(PackValidationError::InvalidAdaptiveFile {
            path: ADAPTIVE_POLICIES_PATH.to_string(),
            reason: format!(
                "policy_digest mismatch: manifest has `{}`, observed `{observed_policy_digest}`",
                adaptive.policy_digest
            ),
        });
    }
    validate_adaptive_policy_document(policy_bytes)?;
    require_adaptive_file(files, "adaptive/flowmaster.prompt.md")?;
    let schema_bytes = require_adaptive_file(files, LAYER_DECISION_SCHEMA_PATH)?;
    validate_layer_decision_schema(schema_bytes)?;
    let registry_bytes = require_adaptive_file(files, "schemas/registry.json")?;
    let registry: serde_json::Value = serde_json::from_slice(registry_bytes).map_err(|err| {
        PackValidationError::InvalidAdaptiveFile {
            path: "schemas/registry.json".to_string(),
            reason: err.to_string(),
        }
    })?;
    let Some(registry_obj) = registry.as_object() else {
        return Err(PackValidationError::InvalidAdaptiveFile {
            path: "schemas/registry.json".to_string(),
            reason: "must be a JSON object mapping schema names to bundled files".to_string(),
        });
    };
    for schema_name in &adaptive.required_schemas {
        let Some(schema_file) = registry_obj
            .get(schema_name)
            .and_then(serde_json::Value::as_str)
        else {
            return Err(PackValidationError::InvalidAdaptiveFile {
                path: "schemas/registry.json".to_string(),
                reason: format!("required schema `{schema_name}` is not registered"),
            });
        };
        validate_relative_adaptive_schema_path(schema_file)?;
        require_adaptive_file(files, schema_file)?;
    }
    Ok(())
}

/// Parse and structurally validate the pack's adaptive policy document at
/// pack-validation time, not first at runtime: the TOML must parse into the
/// typed [`AdaptivePolicy`] and its limit record must be complete (every
/// limit non-zero, via `AdaptiveLimitRecord::validate_complete`).
fn validate_adaptive_policy_document(policy_bytes: &[u8]) -> Result<(), PackValidationError> {
    let policy_text = std::str::from_utf8(policy_bytes).map_err(|err| {
        PackValidationError::InvalidAdaptiveFile {
            path: ADAPTIVE_POLICIES_PATH.to_string(),
            reason: format!("not valid UTF-8: {err}"),
        }
    })?;
    let policy: AdaptivePolicy =
        toml::from_str(policy_text).map_err(|err| PackValidationError::InvalidAdaptiveFile {
            path: ADAPTIVE_POLICIES_PATH.to_string(),
            reason: err.to_string(),
        })?;
    policy.limits.validate_complete("pack").map_err(|err| {
        PackValidationError::InvalidAdaptiveFile {
            path: ADAPTIVE_POLICIES_PATH.to_string(),
            reason: err.to_string(),
        }
    })?;
    Ok(())
}

/// Validate the bundled LayerDecision schema artifact against this host's
/// canonical schema.
///
/// Match rule: the bundled bytes must parse as a JSON object and be
/// structurally equal to [`meerkat_mob::adaptive::layer_decision_schema`]
/// (`serde_json::Value` equality — i.e. byte equality after canonical JSON
/// serialization, key order normalized). The builder emits exactly this
/// artifact, so any divergence means the pack was built by older or
/// hand-rolled tooling and fails closed with a regenerate hint.
fn validate_layer_decision_schema(schema_bytes: &[u8]) -> Result<(), PackValidationError> {
    let bundled: serde_json::Value = serde_json::from_slice(schema_bytes).map_err(|err| {
        PackValidationError::InvalidAdaptiveFile {
            path: LAYER_DECISION_SCHEMA_PATH.to_string(),
            reason: format!("not valid JSON: {err}"),
        }
    })?;
    if !bundled.is_object() {
        return Err(PackValidationError::InvalidAdaptiveFile {
            path: LAYER_DECISION_SCHEMA_PATH.to_string(),
            reason: "must be a JSON object schema".to_string(),
        });
    }
    if bundled != canonical_layer_decision_schema()? {
        return Err(PackValidationError::StaleAdaptiveSchema {
            path: LAYER_DECISION_SCHEMA_PATH.to_string(),
        });
    }
    Ok(())
}

fn require_adaptive_file<'a>(
    files: &'a BTreeMap<String, Vec<u8>>,
    path: &str,
) -> Result<&'a [u8], PackValidationError> {
    files
        .get(path)
        .map(Vec::as_slice)
        .ok_or_else(|| PackValidationError::MissingAdaptiveFile {
            path: path.to_string(),
        })
}

fn validate_relative_adaptive_schema_path(path: &str) -> Result<(), PackValidationError> {
    if !path.starts_with("schemas/")
        || path.contains("..")
        || path.contains('\\')
        || path.ends_with('/')
    {
        return Err(PackValidationError::InvalidAdaptiveFile {
            path: "schemas/registry.json".to_string(),
            reason: format!("schema path `{path}` must stay under schemas/"),
        });
    }
    Ok(())
}

fn parse_manifest(
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<MobpackManifest, PackValidationError> {
    let bytes = files
        .get("manifest.toml")
        .ok_or(PackValidationError::MissingManifest)?;
    let manifest_text = String::from_utf8(bytes.clone())
        .map_err(|err| PackValidationError::InvalidManifest(err.to_string()))?;
    toml::from_str(&manifest_text)
        .map_err(|err| PackValidationError::InvalidManifest(err.to_string()))
}

fn parse_definition(
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<MobDefinition, PackValidationError> {
    let bytes = files
        .get("definition.json")
        .ok_or(PackValidationError::MissingDefinition)?;
    serde_json::from_slice(bytes).map_err(|err| PackValidationError::BadDefinition(err.to_string()))
}

fn validate_manifest_identity(manifest: &MobpackManifest) -> Result<(), PackValidationError> {
    if manifest.mobpack.name.trim().is_empty() {
        return Err(PackValidationError::InvalidManifestField {
            field: "mobpack.name".to_string(),
            reason: "must not be empty".to_string(),
        });
    }
    if manifest.mobpack.version.trim().is_empty() {
        return Err(PackValidationError::InvalidManifestField {
            field: "mobpack.version".to_string(),
            reason: "must not be empty".to_string(),
        });
    }
    Ok(())
}

fn reject_realm_refs(definition: &MobDefinition) -> Result<(), PackValidationError> {
    for (name, binding) in &definition.profiles {
        if binding.realm_ref_name().is_some() {
            return Err(PackValidationError::RealmRefForbidden {
                profile_name: name.to_string(),
            });
        }
    }
    Ok(())
}

fn validate_skill_paths(
    definition: &MobDefinition,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(), PackValidationError> {
    for (skill_name, source) in &definition.skills {
        let SkillSource::Path { path } = source else {
            continue;
        };
        let normalized_path =
            normalize_for_archive(path).map_err(|err| PackValidationError::InvalidSkillPath {
                skill_name: skill_name.clone(),
                path: path.clone(),
                reason: err.to_string(),
            })?;
        if normalized_path != *path {
            return Err(PackValidationError::InvalidSkillPath {
                skill_name: skill_name.clone(),
                path: path.clone(),
                reason: format!("must use canonical archive path `{normalized_path}`"),
            });
        }
        if ArchiveSection::classify(&normalized_path) != Some(ArchiveSection::Skills) {
            return Err(PackValidationError::InvalidSkillPath {
                skill_name: skill_name.clone(),
                path: path.clone(),
                reason: "must be under skills/".to_string(),
            });
        }
        let Some(bytes) = files.get(&normalized_path) else {
            return Err(PackValidationError::MissingSkillFile {
                skill_name: skill_name.clone(),
                path: normalized_path,
            });
        };
        std::str::from_utf8(bytes).map_err(|err| PackValidationError::InvalidSkillUtf8 {
            skill_name: skill_name.clone(),
            path: normalized_path,
            reason: err.to_string(),
        })?;
    }
    Ok(())
}

fn validate_required_files(files: &BTreeMap<String, Vec<u8>>) -> Result<(), PackValidationError> {
    if !files.contains_key("manifest.toml") {
        return Err(PackValidationError::MissingManifest);
    }
    if !files.contains_key("definition.json") {
        return Err(PackValidationError::MissingDefinition);
    }
    Ok(())
}

fn ensure_no_trust_section(files: &BTreeMap<String, Vec<u8>>) -> Result<(), PackValidationError> {
    let bytes = files
        .get("manifest.toml")
        .ok_or(PackValidationError::MissingManifest)?;
    let manifest_text = String::from_utf8(bytes.clone())
        .map_err(|err| PackValidationError::InvalidManifest(err.to_string()))?;
    let value: toml::Value = toml::from_str(&manifest_text)
        .map_err(|err| PackValidationError::InvalidManifest(err.to_string()))?;
    if value.get("trust").is_some() {
        return Err(PackValidationError::TrustSectionForbidden);
    }
    Ok(())
}

fn scan_directory(directory: &Path) -> Result<BTreeMap<String, Vec<u8>>, PackValidationError> {
    let mut files = BTreeMap::new();
    scan_directory_inner(directory, directory, &mut files)?;
    Ok(files)
}

fn scan_directory_inner(
    root: &Path,
    current: &Path,
    out: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), PackValidationError> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            scan_directory_inner(root, &path, out)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|err| PackValidationError::Archive(err.to_string()))?;
        let rel_path = normalize_rel_path(&rel.to_string_lossy());
        out.insert(rel_path, std::fs::read(&path)?);
    }
    Ok(())
}

fn exclude_paths_from_pack(root: &Path, paths: &[&Path], files: &mut BTreeMap<String, Vec<u8>>) {
    for path in paths {
        if let Ok(relative) = path.strip_prefix(root) {
            files.remove(&normalize_rel_path(&relative.to_string_lossy()));
        }
    }
}

fn normalize_rel_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

fn load_signing_key(path: &Path) -> Result<SigningKey, PackValidationError> {
    let raw = std::fs::read_to_string(path)?;
    let trimmed = raw.trim();
    let bytes = hex::decode(trimmed)
        .map_err(|err| PackValidationError::InvalidSigningKey(err.to_string()))?;
    let secret: [u8; 32] = bytes.try_into().map_err(|_| {
        PackValidationError::InvalidSigningKey(
            "expected 64 hex characters (32-byte key)".to_string(),
        )
    })?;
    Ok(SigningKey::from_bytes(&secret))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::signing::verify_digest;
    use tempfile::TempDir;

    #[test]
    fn test_pack_directory_produces_valid_archive() {
        let temp = fixture_mob_dir();
        let packed = pack_directory(temp.path(), None).unwrap();
        let extracted = extract_targz_safe(&packed.archive_bytes).unwrap();
        assert_eq!(
            extracted.get("skills/review.md"),
            Some(&b"# Review skill\n".to_vec())
        );
        assert_eq!(
            extracted.get("hooks/run.sh"),
            Some(&b"#!/bin/sh\necho run\n".to_vec())
        );
        assert!(extracted.contains_key("manifest.toml"));
        assert!(extracted.contains_key("definition.json"));
    }

    #[test]
    fn test_pack_digest_is_deterministic() {
        let temp = fixture_mob_dir();
        let one = pack_directory(temp.path(), None).unwrap();
        let two = pack_directory(temp.path(), None).unwrap();
        assert_eq!(one.digest, two.digest);
    }

    #[test]
    fn test_pack_digest_deterministic_with_output_inside_source_root_when_excluded() {
        let temp = fixture_mob_dir();
        let output_path = temp.path().join("out.mobpack");
        let first =
            pack_directory_with_excludes(temp.path(), None, &[output_path.as_path()]).unwrap();
        std::fs::write(&output_path, &first.archive_bytes).unwrap();
        let second =
            pack_directory_with_excludes(temp.path(), None, &[output_path.as_path()]).unwrap();
        assert_eq!(first.digest, second.digest);
        assert_eq!(first.archive_bytes, second.archive_bytes);
    }

    #[test]
    fn test_pack_output_outside_source_root_preserves_behavior() {
        let temp = fixture_mob_dir();
        let outside = TempDir::new().unwrap();
        let outside_output = outside.path().join("out.mobpack");

        let baseline = pack_directory(temp.path(), None).unwrap();
        let with_excluded_outside =
            pack_directory_with_excludes(temp.path(), None, &[outside_output.as_path()]).unwrap();

        assert_eq!(baseline.digest, with_excluded_outside.digest);
        assert_eq!(baseline.archive_bytes, with_excluded_outside.archive_bytes);
    }

    #[test]
    fn test_pack_sign_produces_valid_signature() {
        let temp = fixture_mob_dir();
        let key_path = temp.path().join("signing.key");
        std::fs::write(
            &key_path,
            "0909090909090909090909090909090909090909090909090909090909090909",
        )
        .unwrap();

        let packed = pack_directory(
            temp.path(),
            Some(SigningRequest {
                signer_id: "test-signer",
                key_path: &key_path,
            }),
        )
        .unwrap();
        let extracted = extract_targz_safe(&packed.archive_bytes).unwrap();
        let signature_toml = extracted.get("signature.toml").expect("signature.toml");
        let signature: PackSignature =
            toml::from_str(std::str::from_utf8(signature_toml).unwrap()).unwrap();
        let recomputed = compute_archive_digest(&packed.archive_bytes).unwrap();
        assert_eq!(recomputed, packed.digest);
        assert_eq!(signature.digest, packed.digest);
        assert_eq!(signature.signer_id.as_str(), "test-signer");

        verify_digest(
            signature.public_key.verifying_key(),
            signature.digest,
            signature.signature.signature(),
        )
        .unwrap();
    }

    #[test]
    fn test_pack_rejects_missing_manifest() {
        let temp = TempDir::new().unwrap();
        std::fs::write(temp.path().join("definition.json"), "{\"id\":\"mob\"}").unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(err, PackValidationError::MissingManifest));
    }

    #[test]
    fn test_pack_rejects_missing_definition() {
        let temp = TempDir::new().unwrap();
        std::fs::write(
            temp.path().join("manifest.toml"),
            "[mobpack]\nname = \"x\"\nversion = \"1\"\n",
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(err, PackValidationError::MissingDefinition));
    }

    #[test]
    fn test_pack_rejects_manifest_trust_section() {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("manifest.toml"),
            "[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n\n[trust]\npolicy = \"strict\"\n",
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(err, PackValidationError::TrustSectionForbidden));
    }

    #[test]
    fn test_pack_rejects_blank_manifest_name() {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("manifest.toml"),
            "[mobpack]\nname = \"   \"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidManifestField { field, .. } if field == "mobpack.name"
        ));
    }

    #[test]
    fn test_pack_rejects_blank_manifest_version() {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("manifest.toml"),
            "[mobpack]\nname = \"fixture\"\nversion = \"\"\n",
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidManifestField { field, .. } if field == "mobpack.version"
        ));
    }

    #[test]
    fn test_pack_rejects_realm_profile_refs() {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("definition.json"),
            br#"{"id":"mob","profiles":{"worker":{"realm_profile":"prod-worker"}}}"#,
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::RealmRefForbidden { profile_name } if profile_name == "worker"
        ));
    }

    #[test]
    fn test_pack_rejects_missing_packed_skill_path() {
        let temp = fixture_mob_dir_with_skill_path("skills/missing.md");
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::MissingSkillFile { skill_name, path }
                if skill_name == "review" && path == "skills/missing.md"
        ));
    }

    #[test]
    fn test_pack_rejects_non_skills_skill_path() {
        let temp = fixture_mob_dir_with_skill_path("config/review.md");
        std::fs::create_dir_all(temp.path().join("config")).unwrap();
        std::fs::write(temp.path().join("config").join("review.md"), "# Review\n").unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidSkillPath { skill_name, path, .. }
                if skill_name == "review" && path == "config/review.md"
        ));
    }

    #[test]
    fn test_pack_rejects_noncanonical_skill_path() {
        let temp = fixture_mob_dir_with_skill_path("./skills/review.md");
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidSkillPath { skill_name, path, reason }
                if skill_name == "review"
                    && path == "./skills/review.md"
                    && reason.contains("canonical archive path")
        ));
    }

    /// A complete adaptive policy document: every limit non-zero so
    /// `AdaptiveLimitRecord::validate_complete` accepts it.
    const COMPLETE_POLICY: &[u8] = b"[limits]\n\
max_depth = 3\n\
max_total_decisions = 6\n\
max_repair_attempts = 2\n\
max_layer_failures = 2\n\
max_attempts_per_layer = 2\n\
max_members_per_layer = 4\n\
max_total_spawned_members = 12\n\
max_active_members = 4\n\
max_retained_layer_mobs = 1\n\
max_wall_clock_ms = 180000\n\
max_aggregate_tokens = 200000\n\
max_aggregate_tool_calls = 100\n";

    fn adaptive_manifest_text(policy: &[u8]) -> String {
        let policy_digest = format!("sha256:{}", hex::encode(Sha256::digest(policy)));
        format!(
            "[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n\n[adaptive]\nflowmaster_profile = \"flowmaster\"\nobjective_class = \"audit\"\npolicy_digest = \"{policy_digest}\"\nrequired_schemas = [\"finding-set\"]\n"
        )
    }

    /// Write a complete adaptive pack source directory. Authors do not supply
    /// `adaptive/layer-decision.schema.json` — the builder emits it.
    fn adaptive_fixture_dir(policy: &[u8]) -> TempDir {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("manifest.toml"),
            adaptive_manifest_text(policy),
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("adaptive")).unwrap();
        std::fs::create_dir_all(temp.path().join("schemas")).unwrap();
        std::fs::write(temp.path().join("adaptive/policies.toml"), policy).unwrap();
        std::fs::write(temp.path().join("adaptive/flowmaster.prompt.md"), "Plan").unwrap();
        std::fs::write(
            temp.path().join("schemas/registry.json"),
            br#"{"finding-set":"schemas/finding-set.schema.json"}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("schemas/finding-set.schema.json"),
            br#"{"type":"object","required":["findings"],"properties":{"findings":{"type":"array","items":{"type":"object"}}}}"#,
        )
        .unwrap();
        temp
    }

    /// An extracted-files map for a complete, builder-shaped adaptive pack
    /// (stamped capability + canonical schema), for load-level tampering tests.
    fn adaptive_pack_files() -> BTreeMap<String, Vec<u8>> {
        let temp = adaptive_fixture_dir(COMPLETE_POLICY);
        let packed = pack_directory(temp.path(), None).unwrap();
        extract_targz_safe(&packed.archive_bytes).unwrap()
    }

    #[test]
    fn test_adaptive_manifest_requires_typed_payload_files_and_policy_digest() {
        let temp = adaptive_fixture_dir(COMPLETE_POLICY);
        pack_directory(temp.path(), None).expect("complete adaptive payload validates");

        // Tampering with the policy after digesting fails closed.
        let mut tampered = COMPLETE_POLICY.to_vec();
        tampered.extend_from_slice(b"# drift\n");
        std::fs::write(temp.path().join("adaptive/policies.toml"), &tampered).unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidAdaptiveFile { path, reason }
                if path == "adaptive/policies.toml" && reason.contains("policy_digest mismatch")
        ));

        // Load-level: an archive missing the layer-decision schema fails
        // closed (the builder always emits it, so absence means tampering or
        // foreign tooling).
        let mut files = adaptive_pack_files();
        files.remove("adaptive/layer-decision.schema.json").unwrap();
        let err = validate_extracted_pack_files(&files).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::MissingAdaptiveFile { path }
                if path == "adaptive/layer-decision.schema.json"
        ));
    }

    #[test]
    fn test_builder_stamps_adaptive_capability_idempotently() {
        // First pack: manifest has no [requires]; the builder stamps it.
        let temp = adaptive_fixture_dir(COMPLETE_POLICY);
        let packed = pack_directory(temp.path(), None).unwrap();
        let files = extract_targz_safe(&packed.archive_bytes).unwrap();
        let manifest: MobpackManifest =
            toml::from_str(std::str::from_utf8(files.get("manifest.toml").unwrap()).unwrap())
                .unwrap();
        let stamped_tokens = manifest
            .requires
            .as_ref()
            .expect("builder must create [requires]")
            .capabilities
            .iter()
            .filter(|cap| cap.token() == "adaptive_flow")
            .count();
        assert_eq!(stamped_tokens, 1);

        // Second pack from a source dir whose manifest is already stamped:
        // no duplicate entry, and the manifest survives byte-identical.
        let already = adaptive_fixture_dir(COMPLETE_POLICY);
        let stamped_manifest =
            String::from_utf8(files.get("manifest.toml").unwrap().clone()).unwrap();
        std::fs::write(already.path().join("manifest.toml"), &stamped_manifest).unwrap();
        let repacked = pack_directory(already.path(), None).unwrap();
        let refiles = extract_targz_safe(&repacked.archive_bytes).unwrap();
        assert_eq!(
            std::str::from_utf8(refiles.get("manifest.toml").unwrap()).unwrap(),
            stamped_manifest,
            "an already-stamped manifest must pass through unchanged"
        );
    }

    #[test]
    fn test_builder_emits_canonical_layer_decision_schema() {
        let temp = adaptive_fixture_dir(COMPLETE_POLICY);
        // Even a hand-supplied (vacuous) schema is overwritten by the builder.
        std::fs::write(
            temp.path().join("adaptive/layer-decision.schema.json"),
            br#"{"type":"object"}"#,
        )
        .unwrap();
        let packed = pack_directory(temp.path(), None).unwrap();
        let files = extract_targz_safe(&packed.archive_bytes).unwrap();
        assert_eq!(
            files.get("adaptive/layer-decision.schema.json").unwrap(),
            &canonical_layer_decision_schema_bytes().unwrap(),
        );
        let emitted: serde_json::Value =
            serde_json::from_slice(files.get("adaptive/layer-decision.schema.json").unwrap())
                .unwrap();
        assert!(
            emitted.get("$defs").is_some(),
            "emitted schema must be the structural LayerDecision schema"
        );
    }

    #[test]
    fn test_load_rejects_prestamp_adaptive_pack_without_capability() {
        // Simulate a pack built by pre-stamp tooling: complete adaptive
        // payload, canonical schema, but no adaptive_flow under [requires].
        let mut files = adaptive_pack_files();
        files.insert(
            "manifest.toml".to_string(),
            adaptive_manifest_text(COMPLETE_POLICY).into_bytes(),
        );
        let err = validate_extracted_pack_files(&files).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::MissingAdaptiveCapability { capability }
                if capability == "adaptive_flow"
        ));
    }

    #[test]
    fn test_load_accepts_stamped_adaptive_pack() {
        let files = adaptive_pack_files();
        validate_extracted_pack_files(&files).expect("builder-produced adaptive pack loads");
    }

    #[test]
    fn test_load_rejects_unknown_future_capability_with_supported_set() {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("manifest.toml"),
            "[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n\n[requires]\ncapabilities = [\"comms\", \"quantum_flux\"]\n",
        )
        .unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        match err {
            PackValidationError::UnknownRequiredCapability {
                capability,
                supported,
            } => {
                assert_eq!(capability, "quantum_flux");
                assert!(supported.iter().any(|t| t == "comms"));
                assert!(supported.iter().any(|t| t == "adaptive_flow"));
            }
            other => panic!("expected UnknownRequiredCapability, got {other:?}"),
        }
    }

    #[test]
    fn test_pack_rejects_garbage_adaptive_policy_toml() {
        let policy = b"limits = \"not a table\"\nthis is not TOML at all {{{";
        let temp = adaptive_fixture_dir(policy);
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidAdaptiveFile { path, .. }
                if path == "adaptive/policies.toml"
        ));
    }

    #[test]
    fn test_pack_rejects_incomplete_adaptive_policy_limits() {
        // Zero limits parse but fail validate_complete at pack time.
        let policy = String::from_utf8(COMPLETE_POLICY.to_vec())
            .unwrap()
            .replace("max_depth = 3", "max_depth = 0");
        let temp = adaptive_fixture_dir(policy.as_bytes());
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidAdaptiveFile { path, reason }
                if path == "adaptive/policies.toml" && reason.contains("max_depth")
        ));
    }

    #[test]
    fn test_load_rejects_non_json_layer_decision_schema() {
        let mut files = adaptive_pack_files();
        files.insert(
            "adaptive/layer-decision.schema.json".to_string(),
            b"not json at all".to_vec(),
        );
        let err = validate_extracted_pack_files(&files).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidAdaptiveFile { path, reason }
                if path == "adaptive/layer-decision.schema.json"
                    && reason.contains("not valid JSON")
        ));

        let mut files = adaptive_pack_files();
        files.insert(
            "adaptive/layer-decision.schema.json".to_string(),
            b"true".to_vec(),
        );
        let err = validate_extracted_pack_files(&files).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidAdaptiveFile { path, reason }
                if path == "adaptive/layer-decision.schema.json"
                    && reason.contains("JSON object")
        ));
    }

    #[test]
    fn test_load_rejects_stale_or_vacuous_layer_decision_schema() {
        // The vacuous hand-rolled shape from pre-0.7.1 packs fails closed
        // with a regenerate hint, as does any schema diverging from the
        // host's canonical LayerDecision schema.
        let mut files = adaptive_pack_files();
        files.insert(
            "adaptive/layer-decision.schema.json".to_string(),
            br#"{"type":"object"}"#.to_vec(),
        );
        let err = validate_extracted_pack_files(&files).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::StaleAdaptiveSchema { path }
                if path == "adaptive/layer-decision.schema.json"
        ));
    }

    #[test]
    fn test_layer_decision_schema_match_is_key_order_insensitive() {
        // Match rule: structural equality (canonical-serialization byte
        // equality), so a re-serialized canonical schema with different
        // formatting still matches.
        let canonical = canonical_layer_decision_schema().unwrap();
        let compact = serde_json::to_vec(&canonical).unwrap();
        let mut files = adaptive_pack_files();
        files.insert("adaptive/layer-decision.schema.json".to_string(), compact);
        validate_extracted_pack_files(&files)
            .expect("formatting-only differences must not fail the schema match");
    }

    #[test]
    fn test_pack_rejects_invalid_utf8_skill_path_content() {
        let temp = fixture_mob_dir_with_skill_path("skills/review.md");
        std::fs::write(temp.path().join("skills").join("review.md"), [0xff, 0xfe]).unwrap();
        let err = pack_directory(temp.path(), None).unwrap_err();
        assert!(matches!(
            err,
            PackValidationError::InvalidSkillUtf8 { skill_name, path, .. }
                if skill_name == "review" && path == "skills/review.md"
        ));
    }

    #[test]
    fn test_inspect_rejects_semantically_invalid_archive() {
        let files = BTreeMap::from([
            (
                "manifest.toml".to_string(),
                b"[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n\n[trust]\npolicy = \"strict\"\n"
                    .to_vec(),
            ),
            ("definition.json".to_string(), br#"{"id":"mob"}"#.to_vec()),
        ]);
        let archive = create_targz(&files).unwrap();
        let err = inspect_archive_bytes(&archive).unwrap_err();
        assert!(matches!(err, PackValidationError::TrustSectionForbidden));
    }

    #[test]
    fn test_inspect_shows_manifest_and_digest() {
        let temp = fixture_mob_dir();
        let packed = pack_directory(temp.path(), None).unwrap();
        let inspect = inspect_archive_bytes(&packed.archive_bytes).unwrap();
        assert_eq!(inspect.name, "fixture");
        assert_eq!(inspect.version, "1.0.0");
        assert_eq!(inspect.digest, packed.digest);
        assert!(inspect.file_count >= 4);
    }

    fn fixture_mob_dir() -> TempDir {
        let temp = TempDir::new().unwrap();
        std::fs::create_dir_all(temp.path().join("skills")).unwrap();
        std::fs::create_dir_all(temp.path().join("hooks")).unwrap();
        std::fs::write(
            temp.path().join("manifest.toml"),
            "[mobpack]\nname = \"fixture\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        std::fs::write(temp.path().join("definition.json"), br#"{"id":"mob"}"#).unwrap();
        std::fs::write(
            temp.path().join("skills").join("review.md"),
            "# Review skill\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("hooks").join("run.sh"),
            "#!/bin/sh\necho run\n",
        )
        .unwrap();
        temp
    }

    fn fixture_mob_dir_with_skill_path(path: &str) -> TempDir {
        let temp = fixture_mob_dir();
        std::fs::write(
            temp.path().join("definition.json"),
            format!(
                r#"{{
  "id":"mob",
  "skills":{{"review":{{"source":"path","path":"{path}"}}}}
}}"#
            ),
        )
        .unwrap();
        temp
    }
}
