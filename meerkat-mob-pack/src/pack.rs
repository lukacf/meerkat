use crate::digest::{MobpackDigest, canonical_digest_from_map};
use crate::manifest::MobpackManifest;
use crate::signing::{PackSignature, sign_digest};
use crate::targz::{create_targz, extract_targz_safe};
use crate::validate::PackValidationError;
use chrono::{SecondsFormat, Utc};
use ed25519_dalek::SigningKey;
use meerkat_mob::MobDefinition;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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

pub fn pack_directory(
    directory: &Path,
    signing_key_file: Option<&Path>,
) -> Result<PackResult, PackValidationError> {
    pack_directory_with_excludes(directory, signing_key_file, &[])
}

pub fn pack_directory_with_excludes(
    directory: &Path,
    signing_key_file: Option<&Path>,
    excluded_paths: &[&Path],
) -> Result<PackResult, PackValidationError> {
    let mut files = scan_directory(directory)?;
    exclude_paths_from_pack(directory, excluded_paths, &mut files);
    if let Some(key_path) = signing_key_file {
        exclude_paths_from_pack(directory, &[key_path], &mut files);
    }
    validate_required_files(&files)?;

    if let Some(key_path) = signing_key_file {
        let unsigned_archive = create_targz(&files)?;
        let digest = compute_archive_digest(&unsigned_archive)?;
        let signing_key = load_signing_key(key_path)?;
        let signature = sign_digest(&signing_key, digest);
        let signature_toml = toml::to_string(&PackSignature {
            signer_id: signer_id_from_path(key_path),
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
    validate_required_files(&files)?;
    let manifest = parse_manifest(&files)?;
    parse_definition(&files)?;
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

pub fn validate_archive_bytes(bytes: &[u8]) -> Result<(), PackValidationError> {
    let files = extract_targz_safe(bytes)?;
    validate_required_files(&files)?;
    ensure_no_trust_section(&files)?;
    parse_manifest(&files)?;
    parse_definition(&files)?;
    Ok(())
}

pub fn compute_archive_digest(bytes: &[u8]) -> Result<MobpackDigest, PackValidationError> {
    let files = extract_targz_safe(bytes)?;
    Ok(canonical_digest_from_map(&files))
}

fn parse_manifest(
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<MobpackManifest, PackValidationError> {
    let bytes = files
        .get("manifest.toml")
        .ok_or(PackValidationError::MissingManifest)?;
    let manifest_text = String::from_utf8_lossy(bytes);
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
    let manifest_text = String::from_utf8_lossy(bytes);
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

fn signer_id_from_path(path: &Path) -> String {
    let file_name = path
        .file_name()
        .and_then(|v| v.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "signer".to_string());
    if let Some(stem) = PathBuf::from(file_name)
        .file_stem()
        .and_then(|v| v.to_str())
    {
        return stem.to_string();
    }
    "signer".to_string()
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

        let packed = pack_directory(temp.path(), Some(&key_path)).unwrap();
        let extracted = extract_targz_safe(&packed.archive_bytes).unwrap();
        let signature_toml = extracted.get("signature.toml").expect("signature.toml");
        let signature: PackSignature =
            toml::from_str(std::str::from_utf8(signature_toml).unwrap()).unwrap();
        let recomputed = compute_archive_digest(&packed.archive_bytes).unwrap();
        assert_eq!(recomputed, packed.digest);
        assert_eq!(signature.digest, packed.digest);

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
    fn test_inspect_shows_manifest_and_digest() {
        let temp = fixture_mob_dir();
        let packed = pack_directory(temp.path(), None).unwrap();
        let inspect = inspect_archive_bytes(&packed.archive_bytes).unwrap();
        assert_eq!(inspect.name, "fixture");
        assert_eq!(inspect.version, "1.0.0");
        assert_eq!(inspect.digest, packed.digest);
        assert!(inspect.file_count >= 4);
    }

    #[test]
    fn test_validate_rejects_invalid_pack() {
        let mut files = BTreeMap::new();
        files.insert(
            "manifest.toml".to_string(),
            b"[mobpack]\nname = \"x\"\nversion = \"1\"\n".to_vec(),
        );
        let archive = create_targz(&files).unwrap();
        let err = validate_archive_bytes(&archive).unwrap_err();
        assert!(matches!(err, PackValidationError::MissingDefinition));
    }

    #[test]
    fn test_validate_rejects_trust_in_manifest() {
        let mut files = BTreeMap::new();
        files.insert(
            "manifest.toml".to_string(),
            b"[mobpack]\nname = \"x\"\nversion = \"1\"\n[trust]\npolicy = \"permissive\"\n"
                .to_vec(),
        );
        files.insert("definition.json".to_string(), br#"{"id":"mob"}"#.to_vec());
        let archive = create_targz(&files).unwrap();
        let err = validate_archive_bytes(&archive).unwrap_err();
        assert!(matches!(err, PackValidationError::TrustSectionForbidden));
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
}
