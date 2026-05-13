use crate::validate::PackValidationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MobpackArchiveSection {
    Manifest,
    Definition,
    Signature,
    Skills,
    Hooks,
    Mcp,
    Config,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MobpackArchivePath {
    normalized: String,
    section: MobpackArchiveSection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobpackExecutablePolicy {
    Executable(MobpackExecutableReason),
    NotExecutable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobpackExecutableReason {
    HookScriptExtension,
    HookShebang,
}

impl MobpackArchiveSection {
    pub fn is_signature(self) -> bool {
        matches!(self, Self::Signature)
    }
}

impl MobpackArchivePath {
    pub const MANIFEST_FILE: &'static str = "manifest.toml";
    pub const DEFINITION_FILE: &'static str = "definition.json";
    pub const SIGNATURE_FILE: &'static str = "signature.toml";

    pub fn parse(path: &str) -> Result<Self, PackValidationError> {
        Ok(Self::from_normalized(normalize_archive_path(path)?))
    }

    pub fn for_digest_path(path: &str) -> Self {
        Self::from_normalized(normalize_digest_path(path))
    }

    pub(crate) fn from_normalized(normalized: impl Into<String>) -> Self {
        let normalized = normalized.into();
        let section = classify_section(&normalized);
        Self {
            normalized,
            section,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.normalized
    }

    pub fn into_string(self) -> String {
        self.normalized
    }

    pub fn section(&self) -> MobpackArchiveSection {
        self.section
    }

    pub fn executable_policy(&self, bytes: &[u8]) -> MobpackExecutablePolicy {
        if !matches!(self.section, MobpackArchiveSection::Hooks) {
            return MobpackExecutablePolicy::NotExecutable;
        }

        if self.has_hook_script_extension() {
            return MobpackExecutablePolicy::Executable(
                MobpackExecutableReason::HookScriptExtension,
            );
        }

        if bytes.starts_with(b"#!") {
            return MobpackExecutablePolicy::Executable(MobpackExecutableReason::HookShebang);
        }

        MobpackExecutablePolicy::NotExecutable
    }

    fn has_hook_script_extension(&self) -> bool {
        let file_name = self
            .normalized
            .rsplit('/')
            .next()
            .unwrap_or(self.normalized.as_str());
        matches!(
            file_name.rsplit_once('.').map(|(_, ext)| ext),
            Some("sh" | "bash")
        )
    }
}

impl MobpackExecutablePolicy {
    pub fn is_executable(self) -> bool {
        matches!(self, Self::Executable(_))
    }
}

fn normalize_archive_path(path: &str) -> Result<String, PackValidationError> {
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

fn classify_section(normalized: &str) -> MobpackArchiveSection {
    match normalized {
        MobpackArchivePath::MANIFEST_FILE => MobpackArchiveSection::Manifest,
        MobpackArchivePath::DEFINITION_FILE => MobpackArchiveSection::Definition,
        MobpackArchivePath::SIGNATURE_FILE => MobpackArchiveSection::Signature,
        _ => match normalized.split_once('/') {
            Some(("skills", rest)) if !rest.is_empty() => MobpackArchiveSection::Skills,
            Some(("hooks", rest)) if !rest.is_empty() => MobpackArchiveSection::Hooks,
            Some(("mcp", rest)) if !rest.is_empty() => MobpackArchiveSection::Mcp,
            Some(("config", rest)) if !rest.is_empty() => MobpackArchiveSection::Config,
            _ => MobpackArchiveSection::Other,
        },
    }
}

fn normalize_digest_path(path: &str) -> String {
    let replaced = path.replace('\\', "/");
    replaced
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

fn looks_like_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn classifies_archive_sections_as_typed_path_facts() {
        assert_eq!(
            MobpackArchivePath::parse("manifest.toml")
                .unwrap()
                .section(),
            MobpackArchiveSection::Manifest
        );
        assert_eq!(
            MobpackArchivePath::parse("definition.json")
                .unwrap()
                .section(),
            MobpackArchiveSection::Definition
        );
        assert_eq!(
            MobpackArchivePath::parse("signature.toml")
                .unwrap()
                .section(),
            MobpackArchiveSection::Signature
        );
        assert_eq!(
            MobpackArchivePath::parse("skills/review.md")
                .unwrap()
                .section(),
            MobpackArchiveSection::Skills
        );
        assert_eq!(
            MobpackArchivePath::parse("hooks/run.sh").unwrap().section(),
            MobpackArchiveSection::Hooks
        );
        assert_eq!(
            MobpackArchivePath::parse("mcp/server.toml")
                .unwrap()
                .section(),
            MobpackArchiveSection::Mcp
        );
        assert_eq!(
            MobpackArchivePath::parse("config/defaults.toml")
                .unwrap()
                .section(),
            MobpackArchiveSection::Config
        );
        assert_eq!(
            MobpackArchivePath::parse("hooks").unwrap().section(),
            MobpackArchiveSection::Other
        );
    }

    #[test]
    fn executable_policy_is_owned_by_archive_path_type() {
        let extension_path = MobpackArchivePath::parse("hooks/run.sh").unwrap();
        assert_eq!(
            extension_path.executable_policy(b"echo hi\n"),
            MobpackExecutablePolicy::Executable(MobpackExecutableReason::HookScriptExtension)
        );

        let shebang_path = MobpackArchivePath::parse("hooks/setup").unwrap();
        assert_eq!(
            shebang_path.executable_policy(b"#!/usr/bin/env bash\nset -e\n"),
            MobpackExecutablePolicy::Executable(MobpackExecutableReason::HookShebang)
        );

        let skill_path = MobpackArchivePath::parse("skills/review.sh").unwrap();
        assert_eq!(
            skill_path.executable_policy(b"#!/bin/sh\n"),
            MobpackExecutablePolicy::NotExecutable
        );
    }
}
