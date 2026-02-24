use crate::manifest::MobpackManifest;
use crate::targz::extract_targz_safe;
use crate::validate::PackValidationError;
use meerkat_mob::MobDefinition;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobpackArchive {
    pub manifest: MobpackManifest,
    pub definition: MobDefinition,
    pub skills: BTreeMap<String, Vec<u8>>,
    pub hooks: BTreeMap<String, Vec<u8>>,
    pub mcp: BTreeMap<String, Vec<u8>>,
    pub config: BTreeMap<String, Vec<u8>>,
}

impl MobpackArchive {
    pub fn new(
        manifest: MobpackManifest,
        definition: MobDefinition,
        skills: BTreeMap<String, Vec<u8>>,
        hooks: BTreeMap<String, Vec<u8>>,
        mcp: BTreeMap<String, Vec<u8>>,
        config: BTreeMap<String, Vec<u8>>,
    ) -> Self {
        Self {
            manifest,
            definition,
            skills,
            hooks,
            mcp,
            config,
        }
    }

    pub fn from_archive_bytes(bytes: &[u8]) -> Result<Self, PackValidationError> {
        let files = extract_targz_safe(bytes)?;
        let manifest_bytes = files
            .get("manifest.toml")
            .ok_or(PackValidationError::MissingManifest)?;
        let definition_bytes = files
            .get("definition.json")
            .ok_or(PackValidationError::MissingDefinition)?;
        let manifest_text = String::from_utf8_lossy(manifest_bytes);
        let manifest: MobpackManifest = toml::from_str(&manifest_text)
            .map_err(|err| PackValidationError::InvalidManifest(err.to_string()))?;
        let definition: MobDefinition = serde_json::from_slice(definition_bytes)
            .map_err(|err| PackValidationError::BadDefinition(err.to_string()))?;

        let mut skills = BTreeMap::new();
        let mut hooks = BTreeMap::new();
        let mut mcp = BTreeMap::new();
        let mut config = BTreeMap::new();
        for (path, bytes) in files {
            if path.starts_with("skills/") {
                skills.insert(path, bytes);
            } else if path.starts_with("hooks/") {
                hooks.insert(path, bytes);
            } else if path.starts_with("mcp/") {
                mcp.insert(path, bytes);
            } else if path.starts_with("config/") {
                config.insert(path, bytes);
            }
        }

        Ok(Self::new(manifest, definition, skills, hooks, mcp, config))
    }

    pub fn from_archive_path(path: &Path) -> Result<Self, PackValidationError> {
        let bytes = std::fs::read(path)?;
        Self::from_archive_bytes(&bytes)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::manifest::{MobpackManifest, MobpackSection};
    use crate::targz::create_targz;
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
                surfaces: vec!["cli".to_string()],
            },
            serde_json::from_str::<MobDefinition>("{\"id\":\"mob\"}").unwrap(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        );

        assert_eq!(archive.manifest.mobpack.name, "example");
        assert_eq!(archive.definition.id.as_str(), "mob");
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
        assert!(archive.config.contains_key("config/defaults.toml"));
    }

    #[test]
    fn test_archive_reader_rejects_unsafe_entries() {
        let traversal = make_raw_tar_gz_entry("../evil", tar::EntryType::Regular, b"x");
        let absolute = make_raw_tar_gz_entry("/etc/passwd", tar::EntryType::Regular, b"x");
        let symlink = make_raw_tar_gz_entry("hooks/run", tar::EntryType::Symlink, b"");
        let hardlink = make_raw_tar_gz_entry("hooks/run", tar::EntryType::Link, b"");

        for bytes in [traversal, absolute, symlink, hardlink] {
            let err = MobpackArchive::from_archive_bytes(&bytes).unwrap_err();
            assert!(matches!(err, PackValidationError::UnsafeEntry { .. }));
        }
    }

    fn make_raw_tar_gz_entry(path: &str, entry_type: tar::EntryType, body: &[u8]) -> Vec<u8> {
        let typeflag = if entry_type.is_symlink() {
            b'2'
        } else if entry_type.is_hard_link() {
            b'1'
        } else {
            b'0'
        };

        let mut header = [0u8; 512];
        let path_bytes = path.as_bytes();
        let name_len = path_bytes.len().min(100);
        header[..name_len].copy_from_slice(&path_bytes[..name_len]);
        write_octal(&mut header[100..108], 0o644);
        write_octal(&mut header[108..116], 0);
        write_octal(&mut header[116..124], 0);
        write_octal(&mut header[124..136], body.len() as u64);
        write_octal(&mut header[136..148], 0);
        header[148..156].fill(b' ');
        header[156] = typeflag;
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let checksum = header.iter().map(|byte| u32::from(*byte)).sum::<u32>();
        write_checksum(&mut header[148..156], checksum);

        let mut tar_bytes = Vec::new();
        tar_bytes.extend_from_slice(&header);
        tar_bytes.extend_from_slice(body);
        let pad_len = (512 - (body.len() % 512)) % 512;
        if pad_len > 0 {
            tar_bytes.extend(std::iter::repeat_n(0u8, pad_len));
        }
        tar_bytes.extend(std::iter::repeat_n(0u8, 1024));

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn write_octal(dst: &mut [u8], value: u64) {
        let width = dst.len();
        let mut encoded = format!("{value:o}");
        if encoded.len() + 1 > width {
            encoded = "0".repeat(width - 1);
        }
        let padded = format!("{encoded:0>width$}", width = width - 1);
        dst[..width - 1].copy_from_slice(padded.as_bytes());
        dst[width - 1] = 0;
    }

    fn write_checksum(dst: &mut [u8], checksum: u32) {
        let width = dst.len();
        let mut encoded = format!("{checksum:o}");
        if encoded.len() + 2 > width {
            encoded = "0".repeat(width - 2);
        }
        let padded = format!("{encoded:0>width$}", width = width - 2);
        dst[..width - 2].copy_from_slice(padded.as_bytes());
        dst[width - 2] = 0;
        dst[width - 1] = b' ';
    }
}
