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
        Self::from_extracted_files(&files)
    }

    pub fn from_extracted_files(
        files: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, PackValidationError> {
        let manifest_bytes = files
            .get("manifest.toml")
            .ok_or(PackValidationError::MissingManifest)?;
        let definition_bytes = files
            .get("definition.json")
            .ok_or(PackValidationError::MissingDefinition)?;
        let manifest_text = String::from_utf8(manifest_bytes.clone())
            .map_err(|err| PackValidationError::InvalidManifest(err.to_string()))?;
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
                skills.insert(path.clone(), bytes.clone());
            } else if path.starts_with("hooks/") {
                hooks.insert(path.clone(), bytes.clone());
            } else if path.starts_with("mcp/") {
                mcp.insert(path.clone(), bytes.clone());
            } else if path.starts_with("config/") {
                config.insert(path.clone(), bytes.clone());
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
                surfaces: std::collections::BTreeSet::from(["cli".to_string()]),
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
        let traversal = build_custom_archive("../evil", tar::EntryType::Regular, b"x");
        let absolute = build_custom_archive("/etc/passwd", tar::EntryType::Regular, b"x");
        let symlink = build_custom_archive("hooks/run", tar::EntryType::Symlink, b"");
        let hardlink = build_custom_archive("hooks/run", tar::EntryType::Link, b"");

        for bytes in [traversal, absolute, symlink, hardlink] {
            let err = MobpackArchive::from_archive_bytes(&bytes).unwrap_err();
            assert!(matches!(err, PackValidationError::UnsafeEntry { .. }));
        }
    }
}
