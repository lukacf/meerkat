use crate::digest::{MobpackDigest, canonical_digest_from_map};
use crate::manifest::MobpackManifest;
use crate::signing::{PackSignature, sign_digest};
use crate::targz::{create_targz, extract_targz_safe, normalize_for_archive};
use crate::validate::PackValidationError;
use chrono::{SecondsFormat, Utc};
use ed25519_dalek::SigningKey;
use meerkat_mob::{MobDefinition, definition::SkillSource};
use std::collections::BTreeMap;
use std::path::Path;

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
    validate_extracted_pack_files(&files)?;

    if let Some(request) = signing {
        let unsigned_archive = create_targz(&files)?;
        let digest = compute_archive_digest(&unsigned_archive)?;
        let signing_key = load_signing_key(request.key_path)?;
        let signature = sign_digest(&signing_key, digest);
        let signature_toml = toml::to_string(&PackSignature {
            signer_id: request.signer_id.to_string(),
            public_key: hex::encode(signing_key.verifying_key().to_bytes()),
            digest,
            signature: hex::encode(signature.to_bytes()),
            timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
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
    let definition = parse_definition(files)?;
    reject_realm_refs(&definition)?;
    validate_skill_paths(&definition, files)?;
    Ok(ValidatedPackFiles {
        manifest,
        definition,
    })
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
        if !normalized_path.starts_with("skills/") {
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
    use ed25519_dalek::{Signature, VerifyingKey};
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
        assert_eq!(signature.signer_id, "test-signer");

        let vk_bytes: [u8; 32] = hex::decode(signature.public_key)
            .unwrap()
            .try_into()
            .expect("public key bytes");
        let verifying_key = VerifyingKey::from_bytes(&vk_bytes).unwrap();
        let sig_bytes: [u8; 64] = hex::decode(signature.signature)
            .unwrap()
            .try_into()
            .expect("sig bytes");
        let ed_sig = Signature::from_bytes(&sig_bytes);
        verify_digest(&verifying_key, signature.digest, &ed_sig).unwrap();
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
