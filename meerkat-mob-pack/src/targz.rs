use crate::exec_bits::normalize_executable_bit;
use crate::validate::PackValidationError;
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use tar::{Archive, Builder, EntryType, Header};

pub fn create_targz(files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, PackValidationError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    {
        let mut builder = Builder::new(&mut encoder);
        for (path, bytes) in files {
            let normalized_path = normalize_for_archive(path)?;
            let exec = normalize_executable_bit(&normalized_path, bytes);
            let mut header = Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_entry_type(EntryType::Regular);
            header.set_mode(if exec { 0o755 } else { 0o644 });
            header.set_cksum();
            builder
                .append_data(&mut header, normalized_path, Cursor::new(bytes))
                .map_err(|err| PackValidationError::Archive(err.to_string()))?;
        }
        builder
            .finish()
            .map_err(|err| PackValidationError::Archive(err.to_string()))?;
    }
    encoder.finish().map_err(PackValidationError::from)
}

pub fn extract_targz_safe(input: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, PackValidationError> {
    let reader = GzDecoder::new(Cursor::new(input));
    let mut archive = Archive::new(reader);
    let mut out = BTreeMap::new();

    let entries = archive
        .entries()
        .map_err(|err| PackValidationError::Archive(err.to_string()))?;
    for entry_result in entries {
        let mut entry =
            entry_result.map_err(|err| PackValidationError::Archive(err.to_string()))?;
        let entry_type = entry.header().entry_type();
        let raw_path = entry
            .path()
            .map_err(|err| PackValidationError::Archive(err.to_string()))?;
        let raw_path_string = raw_path.to_string_lossy().to_string();

        if entry_type.is_symlink() {
            return Err(PackValidationError::UnsafeEntry {
                path: raw_path_string,
                reason: "symlink entries are not allowed".to_string(),
            });
        }
        if entry_type.is_hard_link() {
            return Err(PackValidationError::UnsafeEntry {
                path: raw_path_string,
                reason: "hardlink entries are not allowed".to_string(),
            });
        }
        if entry_type.is_dir() {
            continue;
        }
        if !entry_type.is_file() {
            return Err(PackValidationError::UnsafeEntry {
                path: raw_path_string,
                reason: "unsupported tar entry type".to_string(),
            });
        }

        let normalized_path = normalize_for_archive(&raw_path_string)?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        out.insert(normalized_path, bytes);
    }

    Ok(out)
}

fn normalize_for_archive(path: &str) -> Result<String, PackValidationError> {
    let replaced = path.replace('\\', "/");
    if replaced.starts_with('/') || looks_like_windows_absolute(&replaced) {
        return Err(PackValidationError::UnsafeEntry {
            path: path.to_string(),
            reason: "absolute paths are not allowed".to_string(),
        });
    }

    let mut parts = Vec::new();
    for segment in replaced.split('/') {
        if segment.is_empty() || segment == "." {
            continue;
        }
        if segment == ".." {
            return Err(PackValidationError::UnsafeEntry {
                path: path.to_string(),
                reason: "path traversal is not allowed".to_string(),
            });
        }
        parts.push(segment);
    }

    if parts.is_empty() {
        return Err(PackValidationError::UnsafeEntry {
            path: path.to_string(),
            reason: "empty archive paths are not allowed".to_string(),
        });
    }

    Ok(parts.join("/"))
}

fn looks_like_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::test_utils::build_custom_archive;
    use crate::validate::PackValidationError;

    #[test]
    fn test_targz_roundtrip() {
        let files = BTreeMap::from([
            (
                "manifest.toml".to_string(),
                b"[mobpack]\nname=\"x\"".to_vec(),
            ),
            ("definition.json".to_string(), br#"{"id":"mob"}"#.to_vec()),
            (
                "skills/review.md".to_string(),
                b"Review everything".to_vec(),
            ),
        ]);

        let archive = create_targz(&files).unwrap();
        let extracted = extract_targz_safe(&archive).unwrap();
        assert_eq!(extracted, files);
    }

    #[test]
    fn test_targz_rejects_path_traversal() {
        let bytes = build_custom_archive("../evil.txt", EntryType::Regular, b"bad");
        let err = extract_targz_safe(&bytes).unwrap_err();
        assert!(matches!(err, PackValidationError::UnsafeEntry { .. }));
    }

    #[test]
    fn test_targz_rejects_absolute_path() {
        let bytes = build_custom_archive("/etc/passwd", EntryType::Regular, b"bad");
        let err = extract_targz_safe(&bytes).unwrap_err();
        assert!(matches!(err, PackValidationError::UnsafeEntry { .. }));
    }

    #[test]
    fn test_targz_rejects_symlinks() {
        let bytes = build_custom_archive("hooks/run", EntryType::Symlink, b"");
        let err = extract_targz_safe(&bytes).unwrap_err();
        assert!(matches!(err, PackValidationError::UnsafeEntry { .. }));
    }

    #[test]
    fn test_targz_rejects_hardlinks() {
        let bytes = build_custom_archive("hooks/run", EntryType::Link, b"");
        let err = extract_targz_safe(&bytes).unwrap_err();
        assert!(matches!(err, PackValidationError::UnsafeEntry { .. }));
    }

    #[test]
    fn test_mob_definition_json_roundtrip() {
        let definition: meerkat_mob::MobDefinition =
            serde_json::from_str(r#"{"id":"mobpack-test"}"#).unwrap();
        let encoded = serde_json::to_string(&definition).unwrap();
        let decoded: meerkat_mob::MobDefinition = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, definition);
    }
}
